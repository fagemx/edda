use std::path::Path;

/// JSON board snapshot for `edda peers --json`.
pub(super) fn peers_json(project_id: &str) -> serde_json::Value {
    let stale_threshold = edda_bridge_claude::peers::stale_secs();
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
    let now_epoch = time::OffsetDateTime::now_utc().unix_timestamp();
    let claims: Vec<serde_json::Value> = board
        .claims
        .iter()
        .map(|claim| {
            let mut value = serde_json::to_value(claim).unwrap_or_default();
            let ts_epoch = time::OffsetDateTime::parse(
                &claim.ts,
                &time::format_description::well_known::Rfc3339,
            )
            .map(|t| t.unix_timestamp())
            .unwrap_or(0);
            let age_secs = (now_epoch - ts_epoch).max(0) as u64;
            value["age_secs"] = serde_json::json!(age_secs);
            value["stale"] = serde_json::json!(age_secs > stale_threshold);
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

    println!("Active sessions ({}):\n", active.len());
    for p in &active {
        let age = edda_bridge_claude::peers::format_age(p.age_secs);
        let scope = match (&p.claimed_subject, p.claimed_paths.is_empty()) {
            (Some(sub), false) => format!(" [{sub}; {}]", p.claimed_paths.join(", ")),
            (Some(sub), true) => format!(" [{sub}]"),
            (None, false) => format!(" [{}]", p.claimed_paths.join(", ")),
            (None, true) => String::new(),
        };
        let label = if p.label.is_empty() {
            "(no label)".to_string()
        } else {
            p.label.clone()
        };
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
    if !stale.is_empty() {
        println!(
            "\n  (+{} stale session{} not shown)",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        );
    }
    Ok(())
}
