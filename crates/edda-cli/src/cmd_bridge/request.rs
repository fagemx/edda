use std::path::Path;

/// `edda bridge claude request <to> <message>` — send cross-agent request
///
/// The target is a free-string label, so a typo used to be indistinguishable
/// from a delivered message (GH-443). Resolve it against live sessions first:
/// nobody listening is an error unless `--force`, and an ambiguous label is a
/// warning, because the message really will land in several inboxes.
pub fn request(
    repo_root: &Path,
    to: &str,
    message: &str,
    cli_session: Option<&str>,
    force: bool,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, from_label) = resolve_session_id(cli_session, &project_id, "cli")?;

    let targets = edda_bridge_claude::peers::resolve_request_targets(&project_id, to);
    if targets.is_empty() && !force {
        let active = active_labels(&project_id);
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

    edda_bridge_claude::peers::write_request(&project_id, &session_id, &from_label, to, message);
    let notify_config =
        edda_notify::NotifyConfig::load(&edda_ledger::EddaPaths::discover(repo_root));
    if !notify_config.channels.is_empty() {
        edda_notify::dispatch(
            &notify_config,
            &edda_notify::NotifyEvent::RequestPending {
                from_label: from_label.clone(),
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
