use std::io::Read;
use std::path::Path;

/// `edda bridge claude install`
pub fn install(repo_root: &Path, no_claude_md: bool) -> anyhow::Result<()> {
    edda_bridge_claude::install(repo_root, no_claude_md)
}

/// `edda bridge claude uninstall`
pub fn uninstall(repo_root: &Path) -> anyhow::Result<()> {
    edda_bridge_claude::uninstall(repo_root)
}

/// `edda hook claude` — read stdin, dispatch hook
pub fn hook_claude() -> anyhow::Result<()> {
    let mut stdin_buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin_buf) {
        debug_log(&format!("STDIN READ ERROR: {e}"));
        return Ok(());
    }

    debug_log(&format!(
        "STDIN({} bytes): {}",
        stdin_buf.len(),
        &stdin_buf[..stdin_buf.len().min(200)]
    ));

    match edda_bridge_claude::hook_entrypoint_from_stdin(&stdin_buf) {
        Ok(result) => {
            if let Some(output) = &result.stdout {
                debug_log(&format!("OK output({} bytes)", output.len()));
                print!("{output}");
            }
            if let Some(warning) = &result.stderr {
                debug_log(&format!("WARNING: {warning}"));
                eprintln!("{warning}");
                // Exit 1 = non-blocking warning; Claude Code shows stderr to user
                // but does not feed it to the model or block the conversation.
                std::process::exit(1);
            }
            if result.stdout.is_none() && result.stderr.is_none() {
                debug_log("OK (no output)");
            }
            Ok(())
        }
        Err(e) => {
            debug_log(&format!("ERROR: {e}"));
            // Exit 0 on internal errors — never block the host agent
            Ok(())
        }
    }
}

fn debug_log(msg: &str) {
    if std::env::var_os("EDDA_DEBUG").is_none() {
        return;
    }
    use std::io::Write;
    let log_path = std::env::temp_dir().join("edda-hook-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let ts = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

/// `edda doctor claude`
pub fn doctor(repo_root: &Path) -> anyhow::Result<()> {
    edda_bridge_claude::doctor(repo_root)
}

/// `edda bridge claude peers` — show active peer sessions
pub fn peers(repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let sessions = edda_bridge_claude::peers::discover_all_sessions(&project_id);

    if sessions.is_empty() {
        println!("No active sessions.");
        return Ok(());
    }

    println!("Active sessions ({}):\n", sessions.len());
    for p in &sessions {
        let age = edda_bridge_claude::peers::format_age(p.age_secs);
        let scope = if p.claimed_paths.is_empty() {
            String::new()
        } else {
            format!(" [{}]", p.claimed_paths.join(", "))
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
    Ok(())
}

/// `edda bridge claude claim <label>` — claim a coordination scope
pub fn claim(
    repo_root: &Path,
    label: &str,
    paths: &[String],
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, label);

    edda_bridge_claude::peers::write_claim(&project_id, &session_id, label, paths);
    println!("Claimed scope: {label}");
    if !paths.is_empty() {
        println!("  paths: {}", paths.join(", "));
    }
    println!("  session: {session_id}");
    Ok(())
}

/// `edda bridge claude decide <key=value>` — record a binding decision
///
/// Writes to both:
/// 1. Peers `coordination.jsonl` — real-time broadcast to active peers
/// 2. Workspace ledger — permanent record visible to all sessions
pub fn decide(
    repo_root: &Path,
    decision: &str,
    reason: Option<&str>,
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let (key, value) = decision.split_once('=').ok_or_else(|| {
        anyhow::anyhow!("decision must be in key=value format (e.g. \"auth.method=JWT RS256\")")
    })?;

    let key = key.trim();
    let value = value.trim();

    let project_id = edda_store::project_id(repo_root);
    let (session_id, label) = resolve_session_id(cli_session, &project_id, "cli");

    // L2 conflict check (coordination.jsonl) — before writing
    if let Some(conflict) =
        edda_bridge_claude::peers::find_binding_conflict(&project_id, key, value)
    {
        eprintln!(
            "\u{26a0} Conflict: key \"{key}\" already decided as \"{}\" by {} ({})",
            conflict.existing_value, conflict.by_label, conflict.ts
        );
        eprintln!("  Recording your decision \"{key}={value}\" — consider resolving with the other agent.");
    }

    // 1. Broadcast to peers (real-time)
    edda_bridge_claude::peers::write_binding(&project_id, &session_id, &label, key, value);

    // 2. Write to workspace ledger (permanent)
    let ledger = edda_ledger::Ledger::open(repo_root)?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    // Use resolved label as actor (not hardcoded "system")
    let actor = if session_id.starts_with("cli-") {
        "system"
    } else {
        &label
    };
    let dp = edda_core::types::DecisionPayload {
        key: key.to_string(),
        value: value.to_string(),
        reason: reason.map(|r| r.to_string()),
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
        }
    }

    // Re-finalize after payload/refs mutation
    edda_core::event::finalize_event(&mut event);
    ledger.append_event(&event)?;

    println!("Decision recorded: {key} = {value}");
    if let Some(r) = reason {
        println!("  reason: {r}");
    }
    Ok(())
}

/// `edda bridge claude request <to> <message>` — send cross-agent request
pub fn request(
    repo_root: &Path,
    to: &str,
    message: &str,
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, from_label) = resolve_session_id(cli_session, &project_id, "cli");

    edda_bridge_claude::peers::write_request(&project_id, &session_id, &from_label, to, message);
    println!("Request sent to [{to}]: \"{message}\"");
    Ok(())
}

/// Resolve session identity via 4-tier fallback:
///
/// 1. `--session` CLI flag (explicit override)
/// 2. `EDDA_SESSION_ID` env var (conductor path, user override)
/// 3. Heartbeat inference (auto-detect sole active session)
/// 4. `"cli-{fallback_label}"` (genuine CLI usage)
fn resolve_session_id(
    cli_session: Option<&str>,
    project_id: &str,
    fallback_label: &str,
) -> (String, String) {
    let env_label = std::env::var("EDDA_SESSION_LABEL")
        .ok()
        .filter(|v| !v.is_empty());

    // Tier 1: explicit --session flag
    if let Some(sid) = cli_session.filter(|s| !s.is_empty()) {
        let label = env_label.unwrap_or_else(|| fallback_label.to_string());
        return (sid.to_string(), label);
    }

    // Tier 2: EDDA_SESSION_ID env var
    if let Ok(sid) = std::env::var("EDDA_SESSION_ID") {
        if !sid.is_empty() {
            let label = env_label.unwrap_or_else(|| fallback_label.to_string());
            return (sid, label);
        }
    }

    // Tier 3: heartbeat inference (sole active session)
    if let Some((sid, label)) = edda_bridge_claude::peers::infer_session_id(project_id) {
        return (sid, label);
    }

    // Tier 4: fallback
    let label = env_label.unwrap_or_else(|| fallback_label.to_string());
    (format!("cli-{fallback_label}"), label)
}

/// `edda bridge claude digest --session <id>` or `--all`
pub fn digest(repo_root: &Path, session: Option<&str>, all: bool) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let cwd = repo_root.to_str().unwrap_or(".");

    if let Some(session_id) = session {
        println!("Digesting session {session_id}...");
        let event_id =
            edda_bridge_claude::digest::digest_session_manual(&project_id, session_id, cwd, true)?;
        println!("  Written: {event_id}");
        return Ok(());
    }

    if all {
        let pending = edda_bridge_claude::digest::find_all_pending_sessions(&project_id);
        if pending.is_empty() {
            println!("No pending sessions to digest.");
            return Ok(());
        }
        println!("Found {} pending sessions", pending.len());
        for session_id in &pending {
            print!("  Digesting {session_id}...");
            match edda_bridge_claude::digest::digest_session_manual(
                &project_id,
                session_id,
                cwd,
                true,
            ) {
                Ok(event_id) => println!(" OK ({event_id})"),
                Err(e) => println!(" FAILED: {e}"),
            }
        }
        return Ok(());
    }

    anyhow::bail!("must specify --session <id> or --all")
}

/// `edda index verify --project <id> --session <id> [--sample N] [--all]`
pub fn index_verify(
    project_id: &str,
    session_id: &str,
    sample: usize,
    all: bool,
) -> anyhow::Result<()> {
    let project_dir = edda_store::project_dir(project_id);
    let index_path = project_dir
        .join("index")
        .join(format!("{session_id}.jsonl"));
    let store_path = project_dir
        .join("transcripts")
        .join(format!("{session_id}.jsonl"));

    if !index_path.exists() {
        anyhow::bail!("index file not found: {}", index_path.display());
    }
    if !store_path.exists() {
        anyhow::bail!("store file not found: {}", store_path.display());
    }

    let max_lines = if all { usize::MAX } else { sample * 2 };
    let records = edda_index::read_index_tail(&index_path, max_lines, 64 * 1024 * 1024)?;

    let check_count = if all {
        records.len()
    } else {
        sample.min(records.len())
    };

    // Sample evenly from the records
    let step = if check_count == 0 {
        1
    } else {
        (records.len() as f64 / check_count as f64).ceil() as usize
    };

    let mut checked = 0;
    let mut mismatches = 0;

    for (i, rec) in records.iter().enumerate() {
        if !all && i % step != 0 && checked >= check_count {
            continue;
        }
        if checked >= check_count {
            break;
        }

        let fetched = edda_index::fetch_store_line(&store_path, rec.store_offset, rec.store_len)?;
        let parsed: serde_json::Value = serde_json::from_slice(&fetched)?;
        let fetched_uuid = parsed.get("uuid").and_then(|v| v.as_str()).unwrap_or("");

        if fetched_uuid != rec.uuid {
            println!(
                "MISMATCH at index record {}: expected uuid={}, got uuid={}",
                i, rec.uuid, fetched_uuid
            );
            mismatches += 1;
        }
        checked += 1;
    }

    if mismatches > 0 {
        anyhow::bail!("{mismatches} mismatches found in {checked} checks");
    }

    println!("OK: {checked} index records verified, 0 mismatches");
    Ok(())
}

// ── Render Commands ──

/// `edda bridge claude render-writeback`
pub fn render_writeback() -> anyhow::Result<()> {
    println!("{}", edda_bridge_claude::render::writeback());
    Ok(())
}

/// `edda bridge claude render-workspace`
pub fn render_workspace(repo_root: &Path, budget: usize) -> anyhow::Result<()> {
    let cwd = repo_root.to_str().unwrap_or(".");
    match edda_bridge_claude::render::workspace(cwd, budget) {
        Some(s) => println!("{s}"),
        None => println!("(no workspace context)"),
    }
    Ok(())
}

/// `edda bridge claude render-coordination`
pub fn render_coordination(repo_root: &Path, cli_session: Option<&str>) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, "cli");
    match edda_bridge_claude::render::coordination(&project_id, &session_id) {
        Some(s) => println!("{s}"),
        None => println!("(no coordination context)"),
    }
    Ok(())
}

/// `edda bridge claude render-pack`
pub fn render_pack(repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    match edda_bridge_claude::render::pack(&project_id) {
        Some(s) => println!("{s}"),
        None => println!("(no hot pack available)"),
    }
    Ok(())
}

/// `edda bridge claude render-plan`
pub fn render_plan(repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    match edda_bridge_claude::render::plan(Some(&project_id)) {
        Some(s) => println!("{s}"),
        None => println!("(no active plan)"),
    }
    Ok(())
}

// ── Heartbeat Commands ──

/// `edda bridge claude heartbeat-write`
pub fn heartbeat_write(
    repo_root: &Path,
    label: &str,
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, label);
    let _ = edda_store::ensure_dirs(&project_id);
    edda_bridge_claude::peers::write_heartbeat_minimal(&project_id, &session_id, label);
    println!("Heartbeat written: {label} ({session_id})");
    Ok(())
}

/// `edda bridge claude heartbeat-touch`
pub fn heartbeat_touch(repo_root: &Path, cli_session: Option<&str>) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, "cli");
    edda_bridge_claude::peers::touch_heartbeat(&project_id, &session_id);
    println!("Heartbeat touched: {session_id}");
    Ok(())
}

/// `edda bridge claude heartbeat-remove`
pub fn heartbeat_remove(repo_root: &Path, cli_session: Option<&str>) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, "cli");
    edda_bridge_claude::peers::remove_heartbeat(&project_id, &session_id);
    println!("Heartbeat removed: {session_id}");
    Ok(())
}

// ── OpenClaw Bridge ──

/// `edda bridge openclaw install`
pub fn install_openclaw(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_openclaw::install(target)
}

/// `edda bridge openclaw uninstall`
pub fn uninstall_openclaw(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_openclaw::uninstall(target)
}

/// `edda hook openclaw` — read stdin, dispatch hook
pub fn hook_openclaw() -> anyhow::Result<()> {
    let mut stdin_buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin_buf) {
        debug_log(&format!("OPENCLAW STDIN READ ERROR: {e}"));
        return Ok(());
    }

    debug_log(&format!(
        "OPENCLAW STDIN({} bytes): {}",
        stdin_buf.len(),
        &stdin_buf[..stdin_buf.len().min(200)]
    ));

    match edda_bridge_openclaw::hook_entrypoint_from_stdin(&stdin_buf) {
        Ok(result) => {
            if let Some(output) = &result.stdout {
                debug_log(&format!("OPENCLAW OK output({} bytes)", output.len()));
                print!("{output}");
            }
            if let Some(warning) = &result.stderr {
                debug_log(&format!("OPENCLAW WARNING: {warning}"));
                eprintln!("{warning}");
                std::process::exit(1);
            }
            if result.stdout.is_none() && result.stderr.is_none() {
                debug_log("OPENCLAW OK (no output)");
            }
            Ok(())
        }
        Err(e) => {
            debug_log(&format!("OPENCLAW ERROR: {e}"));
            // Exit 0 on internal errors — never block the host agent
            Ok(())
        }
    }
}

/// `edda doctor openclaw`
pub fn doctor_openclaw() -> anyhow::Result<()> {
    edda_bridge_openclaw::doctor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn setup_workspace() -> (std::path::PathBuf, edda_ledger::Ledger) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_bridge_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = edda_ledger::EddaPaths::discover(&tmp);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        let ledger = edda_ledger::Ledger::open(&tmp).unwrap();
        (tmp, ledger)
    }

    #[test]
    fn find_active_decision_returns_value() {
        let (tmp, ledger) = setup_workspace();
        let branch = ledger.head_branch().unwrap();
        let parent_hash = ledger.last_event_hash().unwrap();

        // Write a decision event with structured fields
        let tags = vec!["decision".to_string()];
        let mut event = edda_core::event::new_note_event(
            &branch,
            parent_hash.as_deref(),
            "system",
            "db.engine: postgres",
            &tags,
        )
        .unwrap();
        event.payload["decision"] = serde_json::json!({"key": "db.engine", "value": "postgres"});
        edda_core::event::finalize_event(&mut event);
        ledger.append_event(&event).unwrap();

        let result = ledger.find_active_decision(&branch, "db.engine").unwrap();
        assert!(result.is_some(), "should find active decision");
        let row = result.unwrap();
        assert!(!row.event_id.is_empty());
        assert_eq!(row.value, "postgres");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_active_decision_no_match() {
        let (tmp, ledger) = setup_workspace();
        let branch = ledger.head_branch().unwrap();

        let result = ledger
            .find_active_decision(&branch, "nonexistent.key")
            .unwrap();
        assert!(result.is_none(), "should not find anything");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── Integration: decide() end-to-end (Issue #148 Gaps 1, 2) ──

    #[test]
    fn decide_writes_binding_to_coordination_log() {
        let (tmp, _ledger) = setup_workspace();
        let pid = edda_store::project_id(&tmp);
        let _ = edda_store::ensure_dirs(&pid);
        // Clean coordination log
        let state_dir = edda_store::project_dir(&pid).join("state");
        let _ = std::fs::remove_file(state_dir.join("coordination.jsonl"));

        std::env::set_var("EDDA_SESSION_ID", "test-decide-bind-s1");
        std::env::set_var("EDDA_SESSION_LABEL", "auth");

        decide(&tmp, "db.engine=postgres", Some("need JSONB"), None).unwrap();

        // Verify binding was written via L2 conflict check API
        let conflict = edda_bridge_claude::peers::find_binding_conflict(&pid, "db.engine", "OTHER");
        assert!(
            conflict.is_some(),
            "should find existing binding via conflict check"
        );
        let c = conflict.unwrap();
        assert_eq!(c.existing_value, "postgres");
        // Verify no conflict with same value (idempotent)
        let no_conflict =
            edda_bridge_claude::peers::find_binding_conflict(&pid, "db.engine", "postgres");
        assert!(no_conflict.is_none(), "same value should not conflict");

        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
    }

    #[test]
    fn decide_writes_structured_ledger_event() {
        let (tmp, ledger) = setup_workspace();
        let pid = edda_store::project_id(&tmp);
        let _ = edda_store::ensure_dirs(&pid);

        std::env::set_var("EDDA_SESSION_ID", "test-decide-ledger-s2");
        std::env::set_var("EDDA_SESSION_LABEL", "billing");

        decide(&tmp, "auth.method=JWT RS256", Some("stateless auth"), None).unwrap();

        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 1, "should have 1 event");
        let e = &events[0];
        assert_eq!(e.event_type, "note");

        // Tags
        let tags = e.payload.get("tags").and_then(|v| v.as_array()).unwrap();
        assert!(tags.iter().any(|t| t.as_str() == Some("decision")));

        // Structured decision object
        let dec = e.payload.get("decision").unwrap();
        assert_eq!(dec["key"].as_str().unwrap(), "auth.method");
        assert_eq!(dec["value"].as_str().unwrap(), "JWT RS256");
        assert_eq!(dec["reason"].as_str().unwrap(), "stateless auth");

        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
    }

    #[test]
    fn decide_supersedes_prior_decision_same_key() {
        let (tmp, ledger) = setup_workspace();
        let pid = edda_store::project_id(&tmp);
        let _ = edda_store::ensure_dirs(&pid);

        std::env::set_var("EDDA_SESSION_ID", "test-decide-super-s3");
        std::env::set_var("EDDA_SESSION_LABEL", "infra");

        decide(&tmp, "db.engine=SQLite", None, None).unwrap();
        decide(&tmp, "db.engine=PostgreSQL", Some("need JSONB"), None).unwrap();

        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 2, "should have 2 events");

        let first_id = &events[0].event_id;
        let second = &events[1];

        // Second event should supersede the first
        assert!(
            !second.refs.provenance.is_empty(),
            "second event should have provenance"
        );
        let prov = &second.refs.provenance[0];
        assert_eq!(prov.target, *first_id, "should point to first event");
        assert_eq!(prov.rel, edda_core::types::rel::SUPERSEDES);

        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
    }

    // ── Integration: resolve_session_id 4-tier fallback (Issue #148 Gap 4) ──

    #[test]
    fn resolve_session_id_tiers() {
        let pid = "test_resolve_sid_tiers";
        let _ = edda_store::ensure_dirs(pid);

        // Clear env to avoid interference
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");

        // Tier 1: explicit cli_session
        let (sid, label) = resolve_session_id(Some("explicit-sid"), pid, "cli");
        assert_eq!(sid, "explicit-sid");
        assert_eq!(label, "cli");

        // Tier 2: EDDA_SESSION_ID env
        std::env::set_var("EDDA_SESSION_ID", "env-sid");
        let (sid, _) = resolve_session_id(None, pid, "cli");
        assert_eq!(sid, "env-sid");
        std::env::remove_var("EDDA_SESSION_ID");

        // Tier 3: heartbeat inference (single active session)
        // Clean state dir first to avoid interference from concurrent sessions
        let state_dir = edda_store::project_dir(pid).join("state");
        if state_dir.exists() {
            for entry in std::fs::read_dir(&state_dir).unwrap() {
                let entry = entry.unwrap();
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("session."))
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let _ = std::fs::create_dir_all(&state_dir);
        let now = time::OffsetDateTime::now_utc();
        let now_str = now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let hb = serde_json::json!({
            "session_id": "inferred-sess",
            "started_at": now_str,
            "last_heartbeat": now_str,
            "label": "worker",
            "focus_files": [],
            "active_tasks": [],
            "files_modified_count": 0,
            "total_edits": 0,
            "recent_commits": []
        });
        std::fs::write(
            state_dir.join("session.inferred-sess.json"),
            serde_json::to_string_pretty(&hb).unwrap(),
        )
        .unwrap();
        let (sid, label) = resolve_session_id(None, pid, "cli");
        assert_eq!(sid, "inferred-sess", "should infer from sole heartbeat");
        assert_eq!(label, "worker", "should use heartbeat label");
        let _ = std::fs::remove_file(state_dir.join("session.inferred-sess.json"));

        // Tier 4: fallback (no heartbeats, no env)
        let (sid, label) = resolve_session_id(None, pid, "cli");
        assert_eq!(sid, "cli-cli");
        assert_eq!(label, "cli");

        // Tier 1 wins over Tier 2
        std::env::set_var("EDDA_SESSION_ID", "env-sid");
        let (sid, _) = resolve_session_id(Some("explicit-wins"), pid, "cli");
        assert_eq!(sid, "explicit-wins", "tier 1 should beat tier 2");
        std::env::remove_var("EDDA_SESSION_ID");

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Render & Heartbeat CLI tests (Issue #15) ──

    #[test]
    fn render_writeback_contains_protocol() {
        let output = edda_bridge_claude::render::writeback();
        assert!(
            output.contains("Write-Back Protocol"),
            "should contain header"
        );
        assert!(output.contains("edda decide"), "should teach edda decide");
        assert!(output.contains("edda note"), "should teach edda note");
    }

    #[test]
    fn render_workspace_with_ledger() {
        let (tmp, _ledger) = setup_workspace();
        let cwd = tmp.to_str().unwrap();
        let result = edda_bridge_claude::render::workspace(cwd, 2500);
        assert!(
            result.is_some(),
            "workspace with ledger should produce output"
        );
        let text = result.unwrap();
        assert!(
            text.contains("Project") || text.contains("Branch"),
            "should contain workspace sections"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn render_workspace_no_ledger() {
        let result = edda_bridge_claude::render::workspace("/nonexistent/path", 2500);
        assert!(result.is_none(), "no workspace should return None");
    }

    #[test]
    fn render_coordination_solo_no_bindings() {
        let pid = "test_render_coord_solo";
        let _ = edda_store::ensure_dirs(pid);
        let result = edda_bridge_claude::render::coordination(pid, "solo-session");
        // Solo with no bindings → None
        assert!(
            result.is_none(),
            "solo session with no bindings should return None"
        );
        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_pack_no_pack_file() {
        let pid = "test_render_pack_empty";
        let _ = edda_store::ensure_dirs(pid);
        let result = edda_bridge_claude::render::pack(pid);
        assert!(result.is_none(), "no hot.md should return None");
        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn heartbeat_write_touch_remove_lifecycle() {
        let pid = "test_hb_lifecycle";
        let sid = "sess-lifecycle-1";
        let _ = edda_store::ensure_dirs(pid);

        // Write
        edda_bridge_claude::peers::write_heartbeat_minimal(pid, sid, "worker");
        let state_dir = edda_store::project_dir(pid).join("state");
        let hb_path = state_dir.join(format!("session.{sid}.json"));
        assert!(hb_path.exists(), "heartbeat file should exist after write");

        // Verify label
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hb_path).unwrap()).unwrap();
        assert_eq!(content["label"].as_str().unwrap(), "worker");
        assert_eq!(content["session_id"].as_str().unwrap(), sid);

        // Touch
        let _mtime_before = std::fs::metadata(&hb_path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        edda_bridge_claude::peers::touch_heartbeat(pid, sid);
        let content_after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hb_path).unwrap()).unwrap();
        // last_heartbeat string should have changed
        assert_ne!(
            content["last_heartbeat"].as_str().unwrap(),
            content_after["last_heartbeat"].as_str().unwrap(),
            "touch should update last_heartbeat"
        );

        // Remove
        edda_bridge_claude::peers::remove_heartbeat(pid, sid);
        assert!(
            !hb_path.exists(),
            "heartbeat file should be gone after remove"
        );

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }
}
