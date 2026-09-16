use std::path::Path;

use edda_ledger::node::{LaneRequest, NodeEvent, NodeEventBody, OutboundQueue};

/// `edda request <to> <message> [--status <id>]` and
/// `edda bridge claude request` — send cross-agent request.
///
/// The target is a free-string label, so a typo used to be indistinguishable
/// from a delivered message (GH-443). Resolve it against live sessions first:
/// nobody listening is an error unless `--force`, and an ambiguous label is a
/// warning, because the message really will land in several inboxes.
///
/// A `<machine>/<role>` target (GH-685) is machine-qualified: a same-machine
/// target stays local, any other machine's target is enqueued as a
/// `lane_request` into that peer's durable outbound queue instead of
/// dead-lettering. `--status <id>` is read-only and reports the sender-side
/// delivery state.
pub fn request(
    repo_root: &Path,
    to: &str,
    message: &str,
    cli_session: Option<&str>,
    force: bool,
    status: Option<&str>,
    json: bool,
) -> anyhow::Result<()> {
    if let Some(id) = status {
        return crate::cmd_inbox::print_request_status(repo_root, id, json);
    }
    let project_id = edda_store::project_id(repo_root);
    let (session_id, from_label) = resolve_session_id(cli_session, &project_id, "cli")?;

    if let Some((machine, role)) = split_machine_target(to) {
        if local_machine_alias(repo_root).as_deref() == Some(machine.as_str()) {
            // Same machine: the local path needs no wire at all.
            return send_local(
                repo_root,
                &project_id,
                &session_id,
                &from_label,
                role,
                message,
                force,
            );
        }
        ensure_configured_peer(&machine, force)?;
        return send_remote(
            repo_root,
            &project_id,
            &session_id,
            &from_label,
            &machine,
            role,
            message,
        );
    }

    send_local(
        repo_root,
        &project_id,
        &session_id,
        &from_label,
        to,
        message,
        force,
    )
}

/// Split `<machine>/<role>` into its two parts when the prefix is a valid
/// machine label and the role is non-empty. A plain label (no slash, or an
/// invalid prefix) is not machine-qualified.
fn split_machine_target(to: &str) -> Option<(String, &str)> {
    let (machine, role) = to.split_once('/')?;
    if role.is_empty() || edda_ledger::node::validate_machine_label(machine).is_err() {
        return None;
    }
    Some((machine.to_string(), role))
}

/// This machine's node alias: `node.json`'s alias when present, else the
/// best-effort `EDDA_MACHINE`/host identity. `None` means no alias is known,
/// which is a refusal for the wire path — never a guessed address.
pub(crate) fn local_machine_alias(_repo_root: &Path) -> Option<String> {
    if let Ok(config) = edda_ledger::node::load_node_config(&edda_ledger::node::node_config_path())
    {
        return Some(config.node.alias);
    }
    edda_bridge_claude::peers::machine_identity()
}

fn send_local(
    repo_root: &Path,
    project_id: &str,
    session_id: &str,
    from_label: &str,
    to: &str,
    message: &str,
    force: bool,
) -> anyhow::Result<()> {
    let targets = edda_bridge_claude::peers::resolve_request_targets(project_id, to);
    if targets.is_empty() && !force {
        let active = active_labels(project_id);
        let known = if active.is_empty() {
            "no sessions are currently active".to_string()
        } else {
            format!("active labels: {}", active.join(", "))
        };
        anyhow::bail!(
            "no active session answers to '{to}' — {known}\n\
             check the label, or pass --force to queue the request for a peer that has not started yet"
        );
    }
    if targets.len() > 1 {
        eprintln!(
            "warning: '{to}' matches {} active sessions — every one of them will see this request",
            targets.len()
        );
    }

    edda_bridge_claude::peers::write_request(project_id, session_id, from_label, to, message);
    let notify_config =
        edda_notify::NotifyConfig::load(&edda_ledger::EddaPaths::discover(repo_root));
    if !notify_config.channels.is_empty() {
        edda_notify::dispatch(
            &notify_config,
            &edda_notify::NotifyEvent::RequestPending {
                from_label: from_label.to_string(),
                to_label: to.to_string(),
                message: message.to_string(),
            },
        );
    }
    if targets.is_empty() {
        println!("Request queued for [{to}] (no active session): \"{message}\"");
    } else {
        println!("Request sent to [{to}]: \"{message}\"");
    }
    if targets.is_empty() {
        println!("The peer will see it at their next prompt.");
    } else {
        println!(
            "To wake them now, use your host's cross-session messaging (target session: {}).",
            targets.join(", ")
        );
    }
    Ok(())
}

/// A `<machine>` target that is not a configured `node.json` peer is a named
/// error unless `--force` — the same fail-closed rule the local path uses for
/// an unknown label. With `--force` it is accepted and a warning says nothing
/// will flush that queue until the peer is configured.
fn ensure_configured_peer(machine: &str, force: bool) -> anyhow::Result<()> {
    let configured = edda_ledger::node::load_node_config(&edda_ledger::node::node_config_path())
        .ok()
        .map(|config| config.peers.into_iter().any(|peer| peer.alias == machine))
        .unwrap_or(false);
    if configured {
        return Ok(());
    }
    if !force {
        anyhow::bail!(
            "machine '{machine}' is not a configured node peer in node.json — add it, or pass \
             --force to queue for a peer that is not configured yet"
        );
    }
    eprintln!(
        "warning: machine '{machine}' is not a configured node peer; nothing will flush this queue \
         until it is"
    );
    Ok(())
}

/// Enqueue a `lane_request` into the peer's durable outbound queue. The local
/// `request` coord event is written first (same id) so the sender's own board
/// and `edda request --status` both see it. The wire carries labels and a
/// machine alias only: never a session id, path, root, lease or credential.
fn send_remote(
    repo_root: &Path,
    project_id: &str,
    session_id: &str,
    from_label: &str,
    machine: &str,
    role: &str,
    message: &str,
) -> anyhow::Result<()> {
    let origin = local_machine_alias(repo_root).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot address '{machine}/{role}': this machine has no node alias (configure node.json)"
        )
    })?;
    let request_id = ulid::Ulid::new().to_string();
    if !edda_bridge_claude::peers::write_request_with_id(
        project_id,
        session_id,
        &request_id,
        from_label,
        &format!("{machine}/{role}"),
        message,
    ) {
        anyhow::bail!("request id '{request_id}' already exists; refusing to reuse it");
    }
    let event = NodeEvent::new(NodeEventBody::LaneRequest(LaneRequest {
        request_id: request_id.clone(),
        from_label: from_label.to_string(),
        to_label: role.to_string(),
        message: message.to_string(),
        ts: now_rfc3339(),
        origin_machine: origin,
    }));
    OutboundQueue::open(machine)?.enqueue(&event)?;
    println!(
        "Request queued across the wire for [{machine}/{role}]: \"{message}\" (request {request_id})"
    );
    Ok(())
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
}

/// Labels of every currently active session, for "did you mean" diagnostics.
fn active_labels(project_id: &str) -> Vec<String> {
    let mut labels: Vec<String> = edda_bridge_claude::peers::discover_all_sessions(project_id)
        .into_iter()
        .filter(|p| p.is_live && !p.label.is_empty())
        .map(|p| p.label)
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

/// `edda request-ack <from>` — acknowledge a pending request
pub fn request_ack(
    repo_root: &Path,
    from_label: &str,
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _label) = resolve_session_id(cli_session, &project_id, "cli")?;

    edda_bridge_claude::peers::write_request_ack(&project_id, &session_id, from_label);
    println!("Acknowledged request from [{from_label}]");
    Ok(())
}

/// Resolve attribution identity for a session-taking CLI verb.
///
/// 1. `--session` CLI flag (explicit override)
/// 2. Process-carried `EDDA_SESSION_ID` (bridge/conductor path, user override)
/// 3. `"cli-{fallback_label}"` only when no live session makes that ambiguous
///
/// `EDDA_SESSION_ID` proves only that the invoking process received an id; it
/// is attribution and an explicit user override, not authentication or
/// authorization. Heartbeats, branches, and working directories cannot prove
/// which process owns a session, so any live heartbeat makes an uncarried
/// identity an error. With no live sessions, the deterministic `cli-*`
/// fallback preserves genuine standalone CLI use. A carrier can preserve only
/// the identity its host exposes; Codex tool hooks, for example, attribute
/// subagent commands to the parent session (GH-503).
pub(crate) fn resolve_session_id(
    cli_session: Option<&str>,
    project_id: &str,
    fallback_label: &str,
) -> anyhow::Result<(String, String)> {
    let env_label = std::env::var("EDDA_SESSION_LABEL")
        .ok()
        .filter(|v| !v.is_empty());

    // Tier 1: explicit --session flag
    if let Some(sid) = cli_session.filter(|s| !s.is_empty()) {
        let label = env_label.unwrap_or_else(|| fallback_label.to_string());
        return Ok((sid.to_string(), label));
    }

    // Tier 2: EDDA_SESSION_ID env var
    if let Ok(sid) = std::env::var("EDDA_SESSION_ID") {
        if !sid.is_empty() {
            let label = env_label.unwrap_or_else(|| fallback_label.to_string());
            return Ok((sid, label));
        }
    }

    let live = fresh_sessions(project_id);
    if !live.is_empty() {
        anyhow::bail!(
            "cannot prove which live session belongs to this process, so --session is required \
             (or set EDDA_SESSION_ID in the invoking process).\n{}",
            format_live_sessions(&live)
        );
    }

    let label = env_label.unwrap_or_else(|| fallback_label.to_string());
    Ok((format!("cli-{fallback_label}"), label))
}

pub(super) fn has_live_sessions(project_id: &str) -> bool {
    !fresh_sessions(project_id).is_empty()
}

/// The sessions currently passing the shared liveness criterion.
fn fresh_sessions(project_id: &str) -> Vec<edda_bridge_claude::peers::PeerSummary> {
    edda_bridge_claude::peers::discover_all_sessions(project_id)
        .into_iter()
        .filter(|session| session.is_live)
        .collect()
}

/// Name the live sessions in the identity-refusal error, so the caller can
/// copy an id into `--session` — an error that demands an id without
/// showing one cannot be acted on (round-1 consequence, GH-705).
fn format_live_sessions(live: &[edda_bridge_claude::peers::PeerSummary]) -> String {
    let mut out = String::from("Live sessions (pass --session with one of these ids):");
    for session in live {
        out.push_str(&format!("\n  {} — {}", session.session_id, session.label));
    }
    out
}
