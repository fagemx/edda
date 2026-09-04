use anyhow::Context;
use std::path::Path;

use super::request::resolve_session_id;

/// `edda bridge claude decide <key=value>` — record a decision.
///
/// GH-401: the decision is agent-authored and unratified (not binding) until
/// an operator ratifies it via `edda ratify`.
///
/// Writes to both:
/// 1. Peers `coordination.jsonl` — real-time broadcast to active peers
/// 2. Workspace ledger — permanent record visible to all sessions
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)] // 209 lines at #779; split tracked in #777
pub fn decide(
    repo_root: &Path,
    decision: &str,
    reason: Option<&str>,
    refs: &[String],
    cli_session: Option<&str>,
    scope_str: Option<&str>,
    paths: &[String],
    tags: &[String],
) -> anyhow::Result<()> {
    let (key, value) = decision.split_once('=').ok_or_else(|| {
        anyhow::anyhow!("decision must be in key=value format (e.g. \"auth.method=JWT RS256\")")
    })?;

    let key = key.trim();
    let value = value.trim();

    // EDDA-SECRET-GUARD1 q331: scrub value + reason before ANY persistence
    // (peer broadcast, ledger, coordination log). Deterministic zero-LLM.
    let (safe_value, value_hits) = edda_core::secret_guard::redact(value);
    let value = safe_value.as_str();
    let (safe_reason, reason_hits) = match reason {
        Some(r) => {
            let (out, hits) = edda_core::secret_guard::redact(r);
            (Some(out), hits)
        }
        None => (None, Vec::new()),
    };
    let reason: Option<&str> = safe_reason.as_deref();
    let all_hits = value_hits.len() + reason_hits.len();
    if all_hits > 0 {
        let kinds: Vec<_> = value_hits
            .iter()
            .chain(reason_hits.iter())
            .map(|h| h.kind)
            .collect();
        eprintln!(
            "⚠ secret-guard: redacted {all_hits} secret pattern(s) before writing decision ({})",
            kinds.join(", ")
        );
    }

    let project_id = edda_store::project_id(repo_root);
    let (session_id, label) = resolve_session_id(cli_session, &project_id, "cli")?;

    // L2 conflict check (coordination.jsonl) — before writing
    if let Some(conflict) =
        edda_bridge_claude::peers::find_binding_conflict(&project_id, key, value)
    {
        eprintln!(
            "\u{26a0} Conflict: key \"{key}\" already decided as \"{}\" by {} ({})",
            conflict.existing_value, conflict.by_label, conflict.ts
        );
        eprintln!("  Recording your decision \"{key}={value}\" — consider resolving with the other agent.");
        // Postmortem supply line: SELECTOR3 病一——same label = own progression,
        // not a cross-agent conflict; only record when actors differ. Best-effort, never blocks.
        let _ = edda_postmortem::signals::record_conflict_signal_if_cross_actor(
            &project_id,
            key,
            &conflict.by_label,
            &label,
        );
    }

    // 1. Broadcast to peers (real-time)
    edda_bridge_claude::peers::write_binding(&project_id, &session_id, &label, key, value);

    // 2. Write to workspace ledger (permanent)
    let ledger = edda_ledger::Ledger::open(repo_root).context("cmd_bridge: opening ledger")?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    // Use resolved label as actor (not hardcoded "system")
    let actor = if session_id.starts_with("cli-") {
        "system"
    } else {
        &label
    };
    // GH-401: a written decision never self-declares operator authority.
    // It is tagged system (internal) or agent; operator authority is
    // conferred only by a separate `edda ratify` (decision_ratify event).
    let authority = if actor == "system" {
        edda_core::types::authority::SYSTEM
    } else {
        edda_core::types::authority::AGENT
    };
    let scope = scope_str
        .filter(|s| *s != "local")
        .map(|s| s.parse::<edda_core::types::DecisionScope>())
        .transpose()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let dp = edda_core::types::DecisionPayload {
        key: key.to_string(),
        value: value.to_string(),
        reason: reason.map(|r| r.to_string()),
        scope,
        authority: Some(authority.to_string()),
        affected_paths: if paths.is_empty() {
            None
        } else {
            Some(paths.to_vec())
        },
        tags: if tags.is_empty() {
            None
        } else {
            Some(tags.to_vec())
        },
        review_after: None,
        reversibility: None,
        village_id: None,
    };
    let mut event =
        edda_core::event::new_decision_event(&branch, parent_hash.as_deref(), actor, &dp)?;

    // Check for prior decision with same key → supersede via provenance (only if value differs)
    let prior = ledger.find_active_decision(&branch, key)?;
    if let Some(prior_row) = &prior {
        if prior_row.value != value {
            eprintln!(
                "\u{26a0} Conflict: key \"{key}\" previously decided as \"{}\" in this workspace",
                prior_row.value
            );
            eprintln!("  Recording new value \"{value}\" (supersedes prior decision)");
            event.refs.provenance.push(edda_core::types::Provenance {
                target: prior_row.event_id.clone(),
                rel: edda_core::types::rel::SUPERSEDES.to_string(),
                note: Some(format!("key '{}' re-decided", key)),
            });
            // Postmortem supply line: best-effort, never blocks the decide.
            let _ = edda_postmortem::signals::record_decision_signal(
                &project_id,
                edda_postmortem::signals::SignalKind::Superseded,
                key,
            );
        }
    }

    // Add depends_on provenance for each --refs key
    for ref_key in refs {
        if let Some(ref_row) = ledger.find_active_decision(&branch, ref_key)? {
            event.refs.provenance.push(edda_core::types::Provenance {
                target: ref_row.event_id.clone(),
                rel: edda_core::types::rel::DEPENDS_ON.to_string(),
                note: Some(ref_key.to_string()),
            });
        } else {
            eprintln!("\u{26a0} ref '{ref_key}' not found, skipping");
        }
    }

    // Re-finalize after payload/refs mutation
    edda_core::event::finalize_event(&mut event)?;
    ledger.append_event(&event)?;

    // Insert dependency edges
    let domain = edda_core::decision::extract_domain(key);

    // Explicit refs → dep edges
    for ref_key in refs {
        // Only insert if the ref key actually exists
        if ledger.find_active_decision(&branch, ref_key)?.is_some() {
            ledger.insert_dep(key, ref_key, "explicit", Some(&event.event_id))?;
        }
    }

    // Auto-link: star-shaped within same domain
    let same_domain = ledger.active_decisions(Some(&domain), None, None, None)?;
    for d in &same_domain {
        if d.key != key {
            ledger.insert_dep(key, &d.key, "auto_domain", Some(&event.event_id))?;
        }
    }

    println!("Decision recorded: {key} = {value}");
    if let Some(r) = reason {
        println!("  reason: {r}");
    }
    if let Some(s) = scope {
        println!("  scope: {s}");
    }
    if !paths.is_empty() {
        println!("  paths: {}", paths.join(", "));
    }
    if !tags.is_empty() {
        println!("  tags: {}", tags.join(", "));
    }

    // Refresh derived markdown views (log.md / main.md / commit.md) so operators
    // reading the ledger by eye see the decision immediately, not only after the
    // next `edda commit` / `edda rebuild`. Same best-effort pattern as
    // edda-serve::api::drafts.rs:508 — failure never blocks a successful decide.
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    Ok(())
}

/// `edda ratify <key>` — confer operator authority on an active decision (GH-401).
///
/// Ratification is a separate append-only fact (`decision_ratify` event),
/// never a mutation of the decision, so operator authority is conferred by a
/// deliberate act and is fully auditable via `ratified_by`. This is the
/// operator counterpart to `edda decide`; agents are taught `decide` only
/// (see the write-back protocol), so a compliant agent never self-ratifies.
///
/// Identity is not cryptographically enforced here — a session can record any
/// `ratified_by`. That enforcement is a policy-layer concern (GH-401 scope);
/// this layer delivers the separation of act, the rendering split, and the
/// audit trail.
pub fn ratify(
    repo_root: &Path,
    key: &str,
    note: Option<&str>,
    by: Option<&str>,
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let key = key.trim();
    let project_id = edda_store::project_id(repo_root);
    let (_session_id, label) = resolve_session_id(cli_session, &project_id, "cli")?;
    let ratified_by = by.unwrap_or(&label);

    let ledger = edda_ledger::Ledger::open(repo_root).context("cmd_bridge: opening ledger")?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;

    // Only an existing active decision can be ratified.
    if ledger.find_active_decision(&branch, key)?.is_none() {
        anyhow::bail!("no active decision for key '{key}' — nothing to ratify (see `edda ask`)");
    }

    let parent_hash = ledger.last_event_hash()?;
    let event = edda_core::event::new_decision_ratify_event(
        &branch,
        parent_hash.as_deref(),
        key,
        ratified_by,
        note,
    )?;
    ledger.append_event(&event)?;
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    println!("Ratified '{key}' (by {ratified_by}) — now binding.");
    if let Some(n) = note {
        println!("  note: {n}");
    }
    Ok(())
}
