use edda_bridge_claude::peers::liveness;
use edda_bridge_claude::peers::{colliding_labels, machine_identity, PeerSummary};
use std::path::Path;

/// JSON board snapshot for `edda peers --json`.
pub(super) fn peers_json(project_id: &str) -> serde_json::Value {
    let sessions: Vec<serde_json::Value> =
        edda_bridge_claude::peers::discover_all_sessions(project_id)
            .into_iter()
            .map(|peer| {
                let stale = !peer.is_live;
                let mut value = serde_json::to_value(&peer).unwrap_or_default();
                value["stale"] = serde_json::json!(stale);
                value
            })
            .collect();
    let board = edda_bridge_claude::peers::compute_board_state(project_id);
    // GH-569: claims are part of the JSON surface programs consume, so each
    // carries its age and a stale flag — otherwise a 55-day-old zombie claim
    // and a 37-second-old live claim are indistinguishable to a program.
    // The rule itself lives in `peers::liveness` beside the session criterion
    // so the dispatch guard can honour the same verdict (GH-1018).
    let now_epoch = liveness::now_epoch();
    let claims: Vec<serde_json::Value> = board
        .claims
        .iter()
        .map(|claim| {
            let mut value = serde_json::to_value(claim).unwrap_or_default();
            value["age_secs"] =
                serde_json::json!(liveness::claim_age_secs_at(&claim.ts, now_epoch));
            value["stale"] = serde_json::json!(liveness::claim_is_stale_at(&claim.ts, now_epoch));
            value
        })
        .collect();
    serde_json::json!({
        "sessions": sessions,
        "claims": claims,
        "requests": board.requests,
        "acks": board.request_acks,
    })
}

/// `edda bridge claude peers` — show active peer sessions
pub fn peers(repo_root: &Path, json: bool) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&peers_json(&project_id))?
        );
        return Ok(());
    }
    let sessions = edda_bridge_claude::peers::discover_all_sessions(&project_id);

    if sessions.is_empty() {
        println!("No active sessions.");
        return Ok(());
    }

    // Collapse stale sessions (heartbeat older than threshold) to a count so
    // dead heartbeat files do not read as live contention.
    let (active, stale): (Vec<_>, Vec<_>) = sessions.iter().partition(|p| p.is_live);

    if active.is_empty() {
        println!(
            "No active sessions ({} stale heartbeat{}).",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        );
        return Ok(());
    }

    // Every session on this board runs on this machine, so `peer_display_label`
    // is documented to resolve the suffix once per render — do that literally,
    // rather than re-reading the env chain for each peer.
    let machine = machine_identity();
    println!("Active sessions ({}):\n", active.len());
    for p in &active {
        let age = edda_bridge_claude::peers::format_age(p.age_secs);
        let scope = match (&p.claimed_subject, p.claimed_paths.is_empty()) {
            (Some(sub), false) => format!(" [{sub}; {}]", p.claimed_paths.join(", ")),
            (Some(sub), true) => format!(" [{sub}]"),
            (None, false) => format!(" [{}]", p.claimed_paths.join(", ")),
            (None, true) => String::new(),
        };
        let label = peer_display_label(p, machine.as_deref());
        println!(
            "  {} — {} ({age}){scope}",
            &p.session_id[..8.min(p.session_id.len())],
            label
        );

        if !p.task_subjects.is_empty() {
            for t in &p.task_subjects {
                println!("    task: {t}");
            }
        } else if !p.focus_files.is_empty() {
            let files: Vec<&str> = p
                .focus_files
                .iter()
                .take(3)
                .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f.as_str()))
                .collect();
            println!("    focus: {}", files.join(", "));
        }
        if p.files_modified_count > 0 {
            println!("    {} files modified", p.files_modified_count);
        }
        if !p.recent_commits.is_empty() {
            for c in &p.recent_commits {
                println!("    commit: {c}");
            }
        }
    }
    // GH-671 identity half: a label shared by live sessions is an ambiguous
    // `edda request` address — say so where the operator is already reading
    // the label, with the fix in the same breath.
    for (label, count) in colliding_labels(active.iter().copied()) {
        println!(
            "\n  ⚠ label \"{label}\" is shared by {count} live sessions — requests to it are ambiguous; set EDDA_SESSION_LABEL or claim a unique label"
        );
    }
    if !stale.is_empty() {
        println!(
            "\n  (+{} stale session{} not shown)",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// One session's label as `edda peers` shows it (GH-671 identity half):
/// `label@machine`, or the bare label when no machine identity resolves.
///
/// Every session on this board runs on this machine — the heartbeat store is
/// machine-local — so the machine suffix is resolved once per render rather
/// than recorded per heartbeat. An unidentified session keeps its marker
/// unchanged; `no label@machine` would read as a machine named "no label".
pub(super) fn peer_display_label(peer: &PeerSummary, machine: Option<&str>) -> String {
    if peer.label.is_empty() {
        return "(no label)".to_string();
    }
    match machine {
        Some(machine) => format!("{}@{machine}", peer.label),
        None => peer.label.clone(),
    }
}
