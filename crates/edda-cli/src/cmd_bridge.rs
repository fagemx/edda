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
        println!("  {} — {} ({age}){scope}", &p.session_id[..8.min(p.session_id.len())], label);

        if !p.task_subjects.is_empty() {
            for t in &p.task_subjects {
                println!("    task: {t}");
            }
        } else if !p.focus_files.is_empty() {
            let files: Vec<&str> = p.focus_files.iter().take(3).map(|f| {
                f.rsplit(['/', '\\']).next().unwrap_or(f.as_str())
            }).collect();
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
pub fn claim(repo_root: &Path, label: &str, paths: &[String], cli_session: Option<&str>) -> anyhow::Result<()> {
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
pub fn decide(repo_root: &Path, decision: &str, reason: Option<&str>, cli_session: Option<&str>) -> anyhow::Result<()> {
    let (key, value) = decision
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("decision must be in key=value format (e.g. \"auth.method=JWT RS256\")"))?;

    let key = key.trim();
    let value = value.trim();

    let project_id = edda_store::project_id(repo_root);
    let (session_id, label) = resolve_session_id(cli_session, &project_id, "cli");

    // 1. Broadcast to peers (real-time)
    edda_bridge_claude::peers::write_binding(&project_id, &session_id, &label, key, value);

    // 2. Write to workspace ledger (permanent)
    let text = match reason {
        Some(r) => format!("{key}: {value} — {r}"),
        None => format!("{key}: {value}"),
    };
    let tags = vec!["decision".to_string()];
    let ledger = edda_ledger::Ledger::open(repo_root)?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    // Build event with structured decision fields alongside text
    // Use resolved label as actor (not hardcoded "system")
    let actor = if session_id.starts_with("cli-") { "system" } else { &label };
    let mut event = edda_core::event::new_note_event(&branch, parent_hash.as_deref(), actor, &text, &tags)?;

    // Inject structured decision object into payload
    let decision_obj = match reason {
        Some(r) => serde_json::json!({"key": key, "value": value, "reason": r}),
        None => serde_json::json!({"key": key, "value": value}),
    };
    event.payload["decision"] = decision_obj;

    // Check for prior decision with same key → supersede via provenance
    let prior_event_id = find_prior_decision(&ledger, &branch, key);
    if let Some(prior_id) = &prior_event_id {
        event.refs.provenance.push(edda_core::types::Provenance {
            target: prior_id.clone(),
            rel: edda_core::types::rel::SUPERSEDES.to_string(),
            note: Some(format!("key '{}' re-decided", key)),
        });
    }

    // Re-finalize after payload/refs mutation
    edda_core::event::finalize_event(&mut event);
    ledger.append_event(&event, false)?;

    println!("Decision recorded: {key} = {value}");
    if let Some(r) = reason {
        println!("  reason: {r}");
    }
    Ok(())
}

/// `edda bridge claude request <to> <message>` — send cross-agent request
pub fn request(repo_root: &Path, to: &str, message: &str, cli_session: Option<&str>) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, from_label) = resolve_session_id(cli_session, &project_id, "cli");

    edda_bridge_claude::peers::write_request(&project_id, &session_id, &from_label, to, message);
    println!("Request sent to [{to}]: \"{message}\"");
    Ok(())
}

/// Find the most recent decision event with the same key on the given branch.
/// Returns the event_id of the prior decision, or None if no match.
fn find_prior_decision(
    ledger: &edda_ledger::Ledger,
    branch: &str,
    key: &str,
) -> Option<String> {
    let events = ledger.iter_events().ok()?;
    events
        .iter()
        .rev()
        .filter(|e| e.branch == branch && e.event_type == "note")
        .filter(|e| {
            e.payload
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().any(|t| t.as_str() == Some("decision")))
                .unwrap_or(false)
        })
        .find_map(|e| {
            // Prefer structured field, fall back to text parse
            let event_key = e
                .payload
                .get("decision")
                .and_then(|d| d.get("key"))
                .and_then(|k| k.as_str())
                .or_else(|| {
                    let text = e.payload.get("text")?.as_str()?;
                    text.split_once(": ").map(|(k, _)| k)
                });
            if event_key == Some(key) {
                Some(e.event_id.clone())
            } else {
                None
            }
        })
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
        let event_id = edda_bridge_claude::digest::digest_session_manual(
            &project_id, session_id, cwd, true,
        )?;
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
                &project_id, session_id, cwd, true,
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

    let check_count = if all { records.len() } else { sample.min(records.len()) };

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
        let fetched_uuid = parsed
            .get("uuid")
            .and_then(|v| v.as_str())
            .unwrap_or("");

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
