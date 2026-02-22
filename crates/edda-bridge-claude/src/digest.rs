//! Deterministic session digest extraction.
//!
//! Reads a session ledger (EventEnvelope JSONL) and produces a
//! `edda_core::Event` milestone summarizing the session — without LLM,
//! without touching the workspace ledger.

use std::collections::BTreeSet;
use std::io::BufRead;
use std::path::Path;

use edda_core::event::finalize_event;
use edda_core::types::{Event, Provenance, Refs, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};

/// A failed Bash command extracted from the session ledger.
#[derive(Debug, Clone)]
pub struct FailedCommand {
    pub command: String,
    pub cwd: String,
    pub exit_code: i32,
}

/// A task snapshot for digest payload (cross-session continuity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestTaskSnapshot {
    pub subject: String,
    pub status: String,
}

/// Session outcome classification.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOutcome {
    /// Normal session end.
    #[default]
    Completed,
    /// User left mid-conversation (last event is a user prompt with no response).
    Interrupted,
    /// Session ended stuck on repeated failures.
    ErrorStuck,
}

impl std::fmt::Display for SessionOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionOutcome::Completed => write!(f, "completed"),
            SessionOutcome::Interrupted => write!(f, "interrupted"),
            SessionOutcome::ErrorStuck => write!(f, "error_stuck"),
        }
    }
}

/// Statistics extracted from a session ledger.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub tool_calls: u64,
    pub tool_failures: u64,
    pub user_prompts: u64,
    pub files_modified: Vec<String>,
    pub failed_commands: Vec<String>,
    /// Rich detail for each failed command (for cmd milestone events).
    pub failed_cmds_detail: Vec<FailedCommand>,
    /// Git commits made during this session (commit messages).
    pub commits_made: Vec<String>,
    /// Task subjects + statuses at session end.
    pub tasks_snapshot: Vec<DigestTaskSnapshot>,
    /// How the session ended.
    pub outcome: SessionOutcome,
    pub duration_minutes: u64,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    /// Number of nudges emitted during this session.
    pub nudge_count: u64,
    /// Number of times agent called `edda decide`.
    pub decide_count: u64,
    /// Total number of decision-worthy signals detected (including suppressed ones).
    pub signal_count: u64,
    /// Dependency packages added during this session (for passive harvest).
    pub deps_added: Vec<String>,
}

/// Extract statistics from a session ledger file.
pub fn extract_stats(session_ledger_path: &Path) -> anyhow::Result<SessionStats> {
    let mut stats = SessionStats::default();
    let mut files_set: BTreeSet<String> = BTreeSet::new();

    // Track session outcome: last event type + trailing failure count
    let mut last_event_name = String::new();
    let mut trailing_failures: u32 = 0;

    if !session_ledger_path.exists() {
        return Ok(stats);
    }

    let file = std::fs::File::open(session_ledger_path)?;
    let reader = std::io::BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let envelope: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // skip malformed lines
        };

        // Track timestamps for duration
        let ts = envelope.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        if !ts.is_empty() {
            if stats.first_ts.is_none() {
                stats.first_ts = Some(ts.to_string());
            }
            stats.last_ts = Some(ts.to_string());
        }

        let event_name = envelope
            .get("hook_event_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Track trailing failures for outcome detection
        if event_name == "PostToolUseFailure" {
            trailing_failures += 1;
        } else if event_name == "PostToolUse" {
            trailing_failures = 0;
        }
        if !event_name.is_empty() {
            last_event_name = event_name.to_string();
        }

        match event_name {
            "PostToolUse" => {
                stats.tool_calls += 1;
                // Extract file_path from Edit/Write tool calls
                let tool_name = envelope
                    .get("tool_name")
                    .or_else(|| {
                        envelope
                            .get("raw")
                            .and_then(|r| r.get("toolName").or_else(|| r.get("tool_name")))
                    })
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if tool_name == "Edit" || tool_name == "Write" {
                    if let Some(fp) = extract_file_path(&envelope) {
                        if !crate::signals::is_noise_file(&fp) {
                            files_set.insert(fp);
                        }
                    }
                }
                if tool_name == "Bash" {
                    if let Some(cmd) = extract_bash_command(&envelope) {
                        if cmd.contains("git commit") {
                            let msg = extract_git_commit_msg(&cmd);
                            if !msg.is_empty() {
                                stats.commits_made.push(msg);
                            }
                        }
                        if let Some(pkg) = crate::nudge::extract_dependency_add(&cmd) {
                            if !stats.deps_added.contains(&pkg) {
                                stats.deps_added.push(pkg);
                            }
                        }
                    }
                }
            }
            "PostToolUseFailure" => {
                stats.tool_failures += 1;
                // Extract failed Bash commands
                let tool_name = envelope
                    .get("tool_name")
                    .or_else(|| {
                        envelope
                            .get("raw")
                            .and_then(|r| r.get("toolName").or_else(|| r.get("tool_name")))
                    })
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if tool_name == "Bash" {
                    if let Some(cmd) = extract_bash_command(&envelope) {
                        let cwd_val = extract_envelope_cwd(&envelope);
                        let exit_code = extract_exit_code(&envelope);
                        stats.failed_cmds_detail.push(FailedCommand {
                            command: cmd.clone(),
                            cwd: cwd_val,
                            exit_code,
                        });
                        stats.failed_commands.push(cmd);
                    }
                }
            }
            "UserPromptSubmit" => {
                stats.user_prompts += 1;
            }
            _ => {}
        }
    }

    stats.files_modified = files_set.into_iter().collect();
    stats.duration_minutes = compute_duration_minutes(&stats.first_ts, &stats.last_ts);

    // Determine session outcome
    stats.outcome = if trailing_failures >= 3 {
        SessionOutcome::ErrorStuck
    } else if last_event_name == "UserPromptSubmit" {
        SessionOutcome::Interrupted
    } else {
        SessionOutcome::Completed
    };

    Ok(stats)
}

/// Load tasks snapshot from state/active_tasks.json for a project.
/// Returns empty vec if file doesn't exist or can't be parsed.
pub fn load_tasks_for_digest(project_id: &str) -> Vec<DigestTaskSnapshot> {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("active_tasks.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let val: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    val.get("tasks")
        .and_then(|t| {
            t.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let subject = item.get("subject")?.as_str()?.to_string();
                        let status = item.get("status")?.as_str()?.to_string();
                        Some(DigestTaskSnapshot { subject, status })
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Build the deterministic text summary from stats.
pub fn render_digest_text(session_id: &str, stats: &SessionStats) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "Session {}: {} tool calls, {} failures, {} user prompts, {} min",
        &session_id[..session_id.len().min(8)],
        stats.tool_calls,
        stats.tool_failures,
        stats.user_prompts,
        stats.duration_minutes,
    ));

    if !stats.files_modified.is_empty() {
        lines.push(format!(
            "Files modified: {}",
            stats.files_modified.join(", ")
        ));
    }

    if !stats.commits_made.is_empty() {
        lines.push("Commits:".to_string());
        for msg in &stats.commits_made {
            let display = if msg.len() > 120 {
                let end = msg.floor_char_boundary(117);
                format!("{}...", &msg[..end])
            } else {
                msg.clone()
            };
            lines.push(format!("  - {display}"));
        }
    }

    if !stats.tasks_snapshot.is_empty() {
        let done: Vec<_> = stats
            .tasks_snapshot
            .iter()
            .filter(|t| t.status == "completed")
            .map(|t| t.subject.as_str())
            .collect();
        let wip: Vec<_> = stats
            .tasks_snapshot
            .iter()
            .filter(|t| t.status != "completed")
            .map(|t| t.subject.as_str())
            .collect();
        if !done.is_empty() {
            lines.push(format!("Done: {}", done.join(", ")));
        }
        if !wip.is_empty() {
            lines.push(format!("WIP: {}", wip.join(", ")));
        }
    }

    if !stats.failed_commands.is_empty() {
        lines.push("Failed commands:".to_string());
        for cmd in &stats.failed_commands {
            // Truncate long commands (char-boundary safe)
            let display = if cmd.len() > 120 {
                let end = cmd.floor_char_boundary(117);
                format!("{}...", &cmd[..end])
            } else {
                cmd.clone()
            };
            lines.push(format!("  - {display}"));
        }
    }

    lines.join("\n")
}

/// Build a milestone `Event` from session stats.
///
/// The returned Event has `branch` and `parent_hash` set to the provided
/// values. The caller is responsible for passing the correct values from
/// the workspace ledger (or empty for standalone use / testing).
pub fn build_digest_event(
    session_id: &str,
    stats: &SessionStats,
    branch: &str,
    parent_hash: Option<&str>,
    notes: &[String],
) -> anyhow::Result<Event> {
    let text = render_digest_text(session_id, stats);

    let payload = serde_json::json!({
        "role": "system",
        "text": text,
        "tags": ["session_digest"],
        "source": "bridge:session_digest",
        "session_id": session_id,
        "session_stats": {
            "tool_calls": stats.tool_calls,
            "tool_failures": stats.tool_failures,
            "user_prompts": stats.user_prompts,
            "files_modified": stats.files_modified,
            "failed_commands": stats.failed_commands,
            "commits_made": stats.commits_made,
            "tasks_snapshot": stats.tasks_snapshot,
            "outcome": stats.outcome.to_string(),
            "duration_minutes": stats.duration_minutes,
            "nudge_count": stats.nudge_count,
            "decide_count": stats.decide_count,
            "signal_count": stats.signal_count,
            "notes": notes,
        }
    });

    let event_id = format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase());
    let ts = now_rfc3339();

    let mut event = Event {
        event_id,
        ts,
        event_type: "note".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs {
            provenance: vec![Provenance {
                target: format!("session:{session_id}"),
                rel: "based_on".to_string(),
                note: Some(format!(
                    "bridge digest of session {}",
                    &session_id[..session_id.len().min(8)]
                )),
            }],
            ..Default::default()
        },
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize_event(&mut event);
    Ok(event)
}

/// Convenience: extract stats + build event in one call.
pub fn extract_session_digest(
    session_ledger_path: &Path,
    session_id: &str,
    branch: &str,
    parent_hash: Option<&str>,
) -> anyhow::Result<Event> {
    let stats = extract_stats(session_ledger_path)?;
    build_digest_event(session_id, &stats, branch, parent_hash, &[])
}

/// Build a `cmd` milestone event for a failed Bash command.
///
/// Each failed command gets its own event with `payload.source = "bridge:cmd"`.
pub fn build_cmd_milestone_event(
    session_id: &str,
    failed_cmd: &FailedCommand,
    branch: &str,
    parent_hash: Option<&str>,
) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "argv": [failed_cmd.command],
        "cwd": failed_cmd.cwd,
        "exit_code": failed_cmd.exit_code,
        "duration_ms": 0,
        "stdout_blob": "",
        "stderr_blob": "",
        "source": "bridge:cmd",
        "session_id": session_id,
    });

    let event_id = format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase());
    let ts = now_rfc3339();

    let mut event = Event {
        event_id,
        ts,
        event_type: "cmd".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs {
            provenance: vec![Provenance {
                target: format!("session:{session_id}"),
                rel: "based_on".to_string(),
                note: Some(format!(
                    "bridge failed cmd from session {}",
                    &session_id[..session_id.len().min(8)]
                )),
            }],
            ..Default::default()
        },
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize_event(&mut event);
    Ok(event)
}

// ── Auto-Digest Orchestration ──

/// State file tracking which sessions have been digested.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DigestState {
    /// The last session_id that was successfully digested.
    #[serde(default)]
    pub session_id: String,
    /// When the digest was written.
    #[serde(default)]
    pub digested_at: String,
    /// The event_id of the milestone written to workspace ledger.
    #[serde(default)]
    pub event_id: String,
    /// Number of consecutive failed digest attempts for the pending session.
    #[serde(default)]
    pub retry_count: u32,
    /// Session ID that failed to digest (if any).
    #[serde(default)]
    pub pending_session_id: String,
    /// Last failure message.
    #[serde(default)]
    pub last_error: String,
}

/// Load the digest state from the per-user store.
pub fn load_digest_state(project_id: &str) -> DigestState {
    let path = digest_state_path(project_id);
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => DigestState::default(),
    }
}

/// Save the digest state to the per-user store.
pub fn save_digest_state(project_id: &str, state: &DigestState) -> anyhow::Result<()> {
    let path = digest_state_path(project_id);
    let json = serde_json::to_string_pretty(state)?;
    edda_store::write_atomic(&path, json.as_bytes())
}

fn digest_state_path(project_id: &str) -> std::path::PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join("last_digested_session.json")
}

/// Find session ledger files in the store, excluding the current session.
fn find_pending_sessions(
    project_id: &str,
    current_session_id: &str,
    state: &DigestState,
) -> Vec<String> {
    let ledger_dir = edda_store::project_dir(project_id).join("ledger");
    let entries = match std::fs::read_dir(&ledger_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let session_id = name.trim_end_matches(".jsonl").to_string();
        // Skip current session (still in progress)
        if session_id == current_session_id {
            continue;
        }
        // Skip already-digested session
        if session_id == state.session_id {
            continue;
        }
        sessions.push(session_id);
    }
    // Sort for deterministic order (oldest first by ULID/name)
    sessions.sort();
    sessions
}

/// Try to acquire WorkspaceLock with a timeout (retry loop).
/// Returns None if lock cannot be acquired within the timeout.
fn try_lock_with_timeout(
    paths: &edda_ledger::EddaPaths,
    timeout_ms: u64,
) -> Option<edda_ledger::WorkspaceLock> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);
    let sleep_interval = std::time::Duration::from_millis(100);

    loop {
        match edda_ledger::WorkspaceLock::acquire(paths) {
            Ok(lock) => return Some(lock),
            Err(_) => {
                if start.elapsed() >= timeout {
                    return None;
                }
                std::thread::sleep(sleep_interval);
            }
        }
    }
}

/// Result of a digest attempt.
#[derive(Debug)]
pub enum DigestResult {
    /// Successfully wrote milestone to workspace ledger.
    Written { event_id: String },
    /// No pending sessions to digest.
    NoPending,
    /// Skipped because auto_digest is disabled.
    Disabled,
    /// Failed to acquire workspace lock.
    LockTimeout,
    /// Failed with an error (recorded in state).
    Error(String),
    /// Permanently failed (retry_count >= 3), includes warning message.
    PermanentFailure(String),
}

/// Main orchestration: digest any pending sessions into the workspace ledger.
///
/// Called from SessionStart hook dispatch. Returns a DigestResult indicating
/// what happened (for logging/warning injection).
pub fn digest_previous_sessions(
    project_id: &str,
    current_session_id: &str,
    cwd: &str,
    lock_timeout_ms: u64,
) -> DigestResult {
    // Phantom cmd events are disabled by default: the digest note already
    // records failed_commands, and separate cmd events inflate the ledger
    // without adding value (they have duration_ms=0, no stdout/stderr).
    digest_previous_sessions_with_opts(project_id, current_session_id, cwd, lock_timeout_ms, false)
}

/// Main orchestration with explicit failed-cmd option.
pub fn digest_previous_sessions_with_opts(
    project_id: &str,
    current_session_id: &str,
    cwd: &str,
    lock_timeout_ms: u64,
    digest_failed_cmds: bool,
) -> DigestResult {
    // Load state
    let mut state = load_digest_state(project_id);

    // Check for permanent failure from previous attempts
    if !state.pending_session_id.is_empty() && state.retry_count >= 3 {
        let warning = format!(
            "edda: session {} digest failed {} times ({}). Run: edda bridge digest --session {}",
            &state.pending_session_id[..state.pending_session_id.len().min(8)],
            state.retry_count,
            state.last_error,
            state.pending_session_id,
        );
        return DigestResult::PermanentFailure(warning);
    }

    // Find sessions to digest
    let pending = find_pending_sessions(project_id, current_session_id, &state);
    if pending.is_empty() {
        // Check if there's a retry pending
        if !state.pending_session_id.is_empty() && state.retry_count > 0 {
            let retry_id = state.pending_session_id.clone();
            return digest_one_session(
                project_id,
                &retry_id,
                cwd,
                lock_timeout_ms,
                digest_failed_cmds,
                &mut state,
            );
        }
        return DigestResult::NoPending;
    }

    // Digest the first pending session (one per SessionStart to keep latency low)
    let session_id = pending[0].clone();
    digest_one_session(
        project_id,
        &session_id,
        cwd,
        lock_timeout_ms,
        digest_failed_cmds,
        &mut state,
    )
}

/// Build a context hint from active tasks and latest commit for inferred decisions.
fn build_context_hint(stats: &SessionStats) -> String {
    let task_part = stats
        .tasks_snapshot
        .iter()
        .find(|t| t.status == "in_progress")
        .or_else(|| stats.tasks_snapshot.first())
        .map(|t| t.subject.as_str());
    let commit_part = stats.commits_made.last().map(|s| s.as_str());

    match (task_part, commit_part) {
        (Some(task), Some(commit)) => {
            let hint = format!("used in: {task} ({commit})");
            if hint.len() > 80 {
                format!("{}...", &hint[..hint.floor_char_boundary(77)])
            } else {
                hint
            }
        }
        (Some(task), None) => {
            let hint = format!("used in: {task}");
            if hint.len() > 80 {
                format!("{}...", &hint[..hint.floor_char_boundary(77)])
            } else {
                hint
            }
        }
        (None, Some(commit)) => {
            let hint = format!("used in: {commit}");
            if hint.len() > 80 {
                format!("{}...", &hint[..hint.floor_char_boundary(77)])
            } else {
                hint
            }
        }
        (None, None) => "(auto-inferred)".to_string(),
    }
}

/// At digest time, write inferred decision events for dependency adds not matched
/// by agent-recorded decisions. Returns the IDs of written events.
fn harvest_inferred_decisions(
    session_id: &str,
    stats: &SessionStats,
    decisions_recorded: &[String],
    ledger: &edda_ledger::Ledger,
    branch: &str,
    parent_hash: Option<&str>,
) -> Vec<String> {
    if stats.deps_added.is_empty() {
        return Vec::new();
    }

    let reason = build_context_hint(stats);
    let mut written_ids = Vec::new();
    let mut chain_hash = parent_hash.map(|s| s.to_string());

    for pkg in &stats.deps_added {
        // Skip if agent already recorded a decision containing this package name
        let pkg_lower = pkg.to_lowercase();
        let already_recorded = decisions_recorded
            .iter()
            .any(|d| d.to_lowercase().contains(&pkg_lower));
        if already_recorded {
            continue;
        }

        let payload = serde_json::json!({
            "role": "system",
            "text": format!("Inferred decision: added dependency {pkg}"),
            "tags": ["decision", "inferred"],
            "source": "bridge:passive_harvest",
            "session_id": session_id,
            "decision": {
                "key": format!("dep.{pkg}"),
                "value": pkg,
                "reason": reason,
            }
        });

        let event_id = format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase());
        let ts = now_rfc3339();

        let mut event = Event {
            event_id: event_id.clone(),
            ts,
            event_type: "note".to_string(),
            branch: branch.to_string(),
            parent_hash: chain_hash.clone(),
            hash: String::new(),
            payload,
            refs: Refs {
                provenance: vec![Provenance {
                    target: format!("session:{session_id}"),
                    rel: "inferred_from".to_string(),
                    note: Some(format!(
                        "passive harvest from session {}",
                        &session_id[..session_id.len().min(8)]
                    )),
                }],
                ..Default::default()
            },
            schema_version: SCHEMA_VERSION,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };

        finalize_event(&mut event);

        if ledger.append_event(&event).is_ok() {
            chain_hash = Some(event.hash.clone());
            written_ids.push(event_id);
        } else {
            break;
        }
    }

    written_ids
}

fn digest_one_session(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    lock_timeout_ms: u64,
    digest_failed_cmds: bool,
    state: &mut DigestState,
) -> DigestResult {
    // Build session ledger path
    let session_ledger_path = edda_store::project_dir(project_id)
        .join("ledger")
        .join(format!("{session_id}.jsonl"));

    if !session_ledger_path.exists() {
        return DigestResult::NoPending;
    }

    // Find workspace root from cwd
    let cwd_path = Path::new(cwd);
    let root = match edda_ledger::EddaPaths::find_root(cwd_path) {
        Some(r) => r,
        None => {
            record_failure(project_id, session_id, state, "no .edda workspace found");
            return DigestResult::Error("no .edda workspace found".to_string());
        }
    };

    let ledger = match edda_ledger::Ledger::open(&root) {
        Ok(l) => l,
        Err(e) => {
            record_failure(
                project_id,
                session_id,
                state,
                &format!("cannot open ledger: {e}"),
            );
            return DigestResult::Error(format!("cannot open ledger: {e}"));
        }
    };

    // Try to acquire lock with timeout
    let _lock = match try_lock_with_timeout(&ledger.paths, lock_timeout_ms) {
        Some(lock) => lock,
        None => {
            record_failure(project_id, session_id, state, "workspace lock timeout");
            return DigestResult::LockTimeout;
        }
    };

    // Read branch and last hash
    let branch = ledger.head_branch().unwrap_or_else(|_| "main".to_string());
    let parent_hash = match ledger.last_event_hash() {
        Ok(h) => h,
        Err(e) => {
            record_failure(
                project_id,
                session_id,
                state,
                &format!("cannot read last hash: {e}"),
            );
            return DigestResult::Error(format!("cannot read last hash: {e}"));
        }
    };

    // Extract stats
    let mut stats = match extract_stats(&session_ledger_path) {
        Ok(s) => s,
        Err(e) => {
            record_failure(
                project_id,
                session_id,
                state,
                &format!("extraction failed: {e}"),
            );
            return DigestResult::Error(format!("extraction failed: {e}"));
        }
    };

    // Skip empty sessions (only SessionStart, no actual work)
    if stats.tool_calls == 0 && stats.tool_failures == 0 && stats.user_prompts == 0 {
        let _ = std::fs::remove_file(&session_ledger_path);
        state.session_id = session_id.to_string();
        state.digested_at = now_rfc3339();
        state.retry_count = 0;
        state.pending_session_id = String::new();
        state.last_error = String::new();
        let _ = save_digest_state(project_id, state);
        return DigestResult::NoPending;
    }

    // Enrich with tasks snapshot from state file
    stats.tasks_snapshot = load_tasks_for_digest(project_id);

    // Collect session notes and decisions from workspace ledger
    let (decisions, notes) = collect_session_ledger_extras(cwd, stats.first_ts.as_deref());

    // Build and append session digest note
    let event =
        match build_digest_event(session_id, &stats, &branch, parent_hash.as_deref(), &notes) {
            Ok(e) => e,
            Err(e) => {
                record_failure(
                    project_id,
                    session_id,
                    state,
                    &format!("build event failed: {e}"),
                );
                return DigestResult::Error(format!("build event failed: {e}"));
            }
        };

    if let Err(e) = ledger.append_event(&event) {
        record_failure(
            project_id,
            session_id,
            state,
            &format!("append failed: {e}"),
        );
        return DigestResult::Error(format!("append failed: {e}"));
    }

    let mut last_event_id = event.event_id.clone();
    let mut last_hash = event.hash.clone();

    // Append cmd milestone events for failed commands (if enabled)
    if digest_failed_cmds && !stats.failed_cmds_detail.is_empty() {
        for failed_cmd in &stats.failed_cmds_detail {
            let cmd_event = match build_cmd_milestone_event(
                session_id,
                failed_cmd,
                &branch,
                Some(&last_hash),
            ) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if ledger.append_event(&cmd_event).is_err() {
                break;
            }
            last_hash = cmd_event.hash.clone();
            last_event_id = cmd_event.event_id.clone();
        }
    }

    // Passive harvest: write inferred decisions for unmatched dependency adds
    let harvest_ids = harvest_inferred_decisions(
        session_id,
        &stats,
        &decisions,
        &ledger,
        &branch,
        Some(&last_hash),
    );
    if let Some(last_harvest_id) = harvest_ids.last() {
        last_event_id = last_harvest_id.clone();
    }

    // Update state: success
    state.session_id = session_id.to_string();
    state.digested_at = now_rfc3339();
    state.event_id = last_event_id.clone();
    state.retry_count = 0;
    state.pending_session_id = String::new();
    state.last_error = String::new();
    let _ = save_digest_state(project_id, state);

    // Remove the session ledger file after successful digest.
    // The session data is now preserved in the workspace ledger's digest event.
    // Removing the file prevents find_pending_sessions() from re-discovering
    // and re-digesting this session in future SessionStart calls.
    let _ = std::fs::remove_file(&session_ledger_path);

    DigestResult::Written {
        event_id: last_event_id,
    }
}

/// Manually digest a specific session (CLI escape hatch).
///
/// Unlike `digest_previous_sessions`, this:
/// - Does NOT check or update state tracking
/// - Forces digest even if already digested
/// - Returns Ok(event_id) on success
pub fn digest_session_manual(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    digest_failed_cmds: bool,
) -> anyhow::Result<String> {
    let session_ledger_path = edda_store::project_dir(project_id)
        .join("ledger")
        .join(format!("{session_id}.jsonl"));

    if !session_ledger_path.exists() {
        anyhow::bail!(
            "session ledger not found: {}",
            session_ledger_path.display()
        );
    }

    let cwd_path = Path::new(cwd);
    let root = edda_ledger::EddaPaths::find_root(cwd_path)
        .ok_or_else(|| anyhow::anyhow!("no .edda workspace found"))?;
    let ledger = edda_ledger::Ledger::open(&root)?;
    let _lock = edda_ledger::WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch().unwrap_or_else(|_| "main".to_string());
    let parent_hash = ledger.last_event_hash()?;

    let mut stats = extract_stats(&session_ledger_path)?;
    stats.tasks_snapshot = load_tasks_for_digest(project_id);
    let (_decisions, notes) = collect_session_ledger_extras(cwd, stats.first_ts.as_deref());
    let event = build_digest_event(session_id, &stats, &branch, parent_hash.as_deref(), &notes)?;
    ledger.append_event(&event)?;

    let mut last_event_id = event.event_id.clone();

    if digest_failed_cmds && !stats.failed_cmds_detail.is_empty() {
        let mut chain_hash = Some(event.hash.clone());
        for failed_cmd in &stats.failed_cmds_detail {
            let cmd_event =
                build_cmd_milestone_event(session_id, failed_cmd, &branch, chain_hash.as_deref())?;
            ledger.append_event(&cmd_event)?;
            chain_hash = Some(cmd_event.hash.clone());
            last_event_id = cmd_event.event_id.clone();
        }
    }

    // Update state to mark as digested
    let mut state = load_digest_state(project_id);
    state.session_id = session_id.to_string();
    state.digested_at = now_rfc3339();
    state.event_id = last_event_id.clone();
    state.retry_count = 0;
    state.pending_session_id = String::new();
    state.last_error = String::new();
    let _ = save_digest_state(project_id, &state);

    Ok(last_event_id)
}

/// Find all undigested sessions in the store for the given project.
pub fn find_all_pending_sessions(project_id: &str) -> Vec<String> {
    let state = load_digest_state(project_id);
    let ledger_dir = edda_store::project_dir(project_id).join("ledger");
    let entries = match std::fs::read_dir(&ledger_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let session_id = name.trim_end_matches(".jsonl").to_string();
        // Skip already-digested session
        if session_id == state.session_id {
            continue;
        }
        sessions.push(session_id);
    }
    sessions.sort();
    sessions
}

fn record_failure(project_id: &str, session_id: &str, state: &mut DigestState, error: &str) {
    if state.pending_session_id == session_id {
        state.retry_count += 1;
    } else {
        state.pending_session_id = session_id.to_string();
        state.retry_count = 1;
    }
    state.last_error = error.to_string();
    let _ = save_digest_state(project_id, state);
}

/// Build a warning message if there are pending digest failures.
/// Returns None if everything is fine.
pub fn pending_failure_warning(project_id: &str) -> Option<String> {
    let state = load_digest_state(project_id);
    if state.pending_session_id.is_empty() || state.retry_count < 3 {
        return None;
    }
    Some(format!(
        "⚠ edda: session {} digest failed {} times ({}). Run: edda bridge digest --session {}",
        &state.pending_session_id[..state.pending_session_id.len().min(8)],
        state.retry_count,
        state.last_error,
        state.pending_session_id,
    ))
}

// ── Helpers ──

fn extract_file_path(envelope: &serde_json::Value) -> Option<String> {
    // Try direct tool_input (our internal format)
    if let Some(fp) = envelope
        .get("raw")
        .and_then(|r| r.get("tool_input").or_else(|| r.get("toolInput")))
        .and_then(|ti| ti.get("file_path").or_else(|| ti.get("filePath")))
        .and_then(|v| v.as_str())
    {
        return Some(normalize_path(fp));
    }
    // Try top-level tool_input (when raw is flattened)
    envelope
        .get("tool_input")
        .and_then(|ti| ti.get("file_path"))
        .and_then(|v| v.as_str())
        .map(normalize_path)
}

fn extract_envelope_cwd(envelope: &serde_json::Value) -> String {
    envelope
        .get("cwd")
        .or_else(|| envelope.get("raw").and_then(|r| r.get("cwd")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn extract_exit_code(envelope: &serde_json::Value) -> i32 {
    let raw = match envelope.get("raw") {
        Some(r) => r,
        None => return 1,
    };

    // Path 1: raw.tool_response.exitCode (if Claude Code ever adds it)
    if let Some(code) = raw
        .get("tool_response")
        .or_else(|| raw.get("toolResponse"))
        .and_then(|tr| tr.get("exitCode").or_else(|| tr.get("exit_code")))
        .and_then(|v| v.as_i64())
    {
        return code as i32;
    }

    // Path 2: raw.error = "Exit code {N}" (PostToolUseFailure format)
    if let Some(error_str) = raw.get("error").and_then(|v| v.as_str()) {
        let first_line = error_str.lines().next().unwrap_or("");
        if let Some(code_str) = first_line.strip_prefix("Exit code ") {
            if let Ok(code) = code_str.trim().parse::<i32>() {
                return code;
            }
        }
    }

    // Default: generic failure (only called for PostToolUseFailure events)
    1
}

fn extract_bash_command(envelope: &serde_json::Value) -> Option<String> {
    // Try raw.tool_input.command
    if let Some(cmd) = envelope
        .get("raw")
        .and_then(|r| r.get("tool_input").or_else(|| r.get("toolInput")))
        .and_then(|ti| ti.get("command"))
        .and_then(|v| v.as_str())
    {
        return Some(cmd.to_string());
    }
    envelope
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract commit message from a `git commit -m "..."` command string.
fn extract_git_commit_msg(cmd: &str) -> String {
    // Try to find -m "..." or -m '...' pattern
    if let Some(pos) = cmd.find("-m ") {
        let after_m = &cmd[pos + 3..];
        let trimmed = after_m.trim_start();
        if let Some(first) = trimmed.chars().next() {
            if first == '"' || first == '\'' {
                if let Some(end) = trimmed[1..].find(first) {
                    return trimmed[1..end + 1].to_string();
                }
            }
        }
    }
    String::new()
}

/// Normalize a file path: strip common prefixes for readability.
fn normalize_path(path: &str) -> String {
    // Keep the path as-is for now; downstream can shorten if needed
    path.to_string()
}

fn compute_duration_minutes(first: &Option<String>, last: &Option<String>) -> u64 {
    let (Some(first), Some(last)) = (first.as_deref(), last.as_deref()) else {
        return 0;
    };
    let fmt = &time::format_description::well_known::Rfc3339;
    let Ok(t1) = time::OffsetDateTime::parse(first, fmt) else {
        return 0;
    };
    let Ok(t2) = time::OffsetDateTime::parse(last, fmt) else {
        return 0;
    };
    let diff: time::Duration = t2 - t1;
    let secs = diff.whole_seconds().unsigned_abs();
    secs / 60
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

// ── Previous Session Digest Snapshot ──

/// Snapshot of a completed session, persisted for next session's context injection.
/// Written at SessionEnd, read at next SessionStart, deleted at that session's end.
#[derive(Debug, Serialize, Deserialize)]
pub struct PrevDigest {
    pub session_id: String,
    pub completed_at: String,
    pub outcome: String,
    pub duration_minutes: u64,
    pub completed_tasks: Vec<String>,
    pub pending_tasks: Vec<String>,
    pub commits: Vec<String>,
    pub files_modified_count: usize,
    pub total_edits: usize,
    /// Decisions recorded via `edda decide` during the session.
    #[serde(default)]
    pub decisions: Vec<String>,
    /// Notes recorded via `edda note` during the session.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Failed commands from the session (data-only, not rendered).
    #[serde(default)]
    pub failed_commands: Vec<String>,
    /// Number of nudges emitted during this session.
    #[serde(default)]
    pub nudge_count: u64,
    /// Number of times agent called `edda decide`.
    #[serde(default)]
    pub decide_count: u64,
    /// Total decision-worthy signals detected (including suppressed ones).
    #[serde(default)]
    pub signal_count: u64,
}

/// Write prev_digest.json from SessionStats + optional ledger extras.
pub fn write_prev_digest(
    project_id: &str,
    session_id: &str,
    stats: &SessionStats,
    decisions: Vec<String>,
    notes: Vec<String>,
) {
    let completed: Vec<String> = stats
        .tasks_snapshot
        .iter()
        .filter(|t| t.status == "completed")
        .map(|t| t.subject.clone())
        .collect();
    let pending: Vec<String> = stats
        .tasks_snapshot
        .iter()
        .filter(|t| t.status != "completed")
        .map(|t| t.subject.clone())
        .collect();

    // Read total_edits from state/files_modified.json (still alive at SessionEnd)
    let total_edits = read_total_edits(project_id);

    let digest = PrevDigest {
        session_id: session_id.to_string(),
        completed_at: now_rfc3339(),
        outcome: stats.outcome.to_string(),
        duration_minutes: stats.duration_minutes,
        completed_tasks: completed,
        pending_tasks: pending,
        commits: stats.commits_made.clone(),
        files_modified_count: stats.files_modified.len(),
        total_edits,
        decisions,
        notes,
        failed_commands: stats.failed_commands.clone(),
        nudge_count: stats.nudge_count,
        decide_count: stats.decide_count,
        signal_count: stats.signal_count,
    };

    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("prev_digest.json");
    if let Ok(data) = serde_json::to_string_pretty(&digest) {
        let _ = edda_store::write_atomic(&path, data.as_bytes());
    }
}

/// Read total edit count from state/files_modified.json.
fn read_total_edits(project_id: &str) -> usize {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("files_modified.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let val: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    val.get("files")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("count").and_then(|c| c.as_u64()))
                .sum::<u64>() as usize
        })
        .unwrap_or(0)
}

/// Read prev_digest.json for rendering.
pub fn read_prev_digest(project_id: &str) -> Option<PrevDigest> {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("prev_digest.json");
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Collect decisions and notes from the workspace ledger that were written
/// during this session (events with `ts >= session_first_ts`).
///
/// Returns `(decisions, notes)`. Gracefully returns empty vecs on any error.
pub fn collect_session_ledger_extras(
    cwd: &str,
    session_first_ts: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    let first_ts = match session_first_ts {
        Some(ts) if !ts.is_empty() => ts,
        _ => return (Vec::new(), Vec::new()),
    };

    let cwd_path = Path::new(cwd);
    let root = match edda_ledger::EddaPaths::find_root(cwd_path) {
        Some(r) => r,
        None => return (Vec::new(), Vec::new()),
    };
    let ledger = match edda_ledger::Ledger::open(&root) {
        Ok(l) => l,
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let events = match ledger.iter_events() {
        Ok(e) => e,
        Err(_) => return (Vec::new(), Vec::new()),
    };

    let mut decisions = Vec::new();
    let mut notes = Vec::new();

    for event in events.iter().rev() {
        // Only events from this session (by timestamp)
        if event.ts.as_str() < first_ts {
            break;
        }
        if event.event_type != "note" {
            continue;
        }

        // Skip auto-generated digest notes
        let source = event
            .payload
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if source.starts_with("bridge:") {
            continue;
        }

        let tags: Vec<&str> = event
            .payload
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|t| t.as_str()).collect())
            .unwrap_or_default();

        if tags.contains(&"decision") {
            // Structured decision: payload.decision.{key, value, reason}
            if let Some(d) = event.payload.get("decision") {
                let key = d.get("key").and_then(|v| v.as_str()).unwrap_or("?");
                let value = d.get("value").and_then(|v| v.as_str()).unwrap_or("?");
                let formatted = match d.get("reason").and_then(|v| v.as_str()) {
                    Some(r) if !r.is_empty() => format!("{key}={value} ({r})"),
                    _ => format!("{key}={value}"),
                };
                decisions.push(formatted);
            } else {
                // Fallback: parse from text "key: value"
                if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                    decisions.push(text.to_string());
                }
            }
        } else if tags.contains(&"session") {
            // Session note written by agent via `edda note --tag session`
            if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                notes.push(text.to_string());
            }
        }
    }

    // Reverse to chronological order (we iterated in reverse)
    decisions.reverse();
    notes.reverse();

    (decisions, notes)
}

/// Convenience: extract stats from stored transcript, enrich with ledger data, and write prev_digest.
pub fn write_prev_digest_from_store(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    nudge_count: u64,
    decide_count: u64,
    signal_count: u64,
) {
    let store_path = edda_store::project_dir(project_id)
        .join("ledger")
        .join(format!("{session_id}.jsonl"));
    if !store_path.exists() {
        return;
    }
    let mut stats = match extract_stats(&store_path) {
        Ok(s) => s,
        Err(_) => return,
    };
    // Supplement tasks from state (extract_stats reads from session ledger which
    // may not have task data; state/active_tasks.json is authoritative)
    if stats.tasks_snapshot.is_empty() {
        stats.tasks_snapshot = load_tasks_for_digest(project_id);
    }
    // Supplement recall rate counters from dispatch state files
    stats.nudge_count = nudge_count;
    stats.decide_count = decide_count;
    stats.signal_count = signal_count;
    // Collect decisions + notes from workspace ledger before writing
    let (decisions, notes) = collect_session_ledger_extras(cwd, stats.first_ts.as_deref());
    write_prev_digest(project_id, session_id, &stats, decisions, notes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_session_ledger(dir: &Path, lines: &[serde_json::Value]) -> std::path::PathBuf {
        let path = dir.join("test_session.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{}", serde_json::to_string(line).unwrap()).unwrap();
        }
        path
    }

    fn make_envelope(
        hook_event_name: &str,
        tool_name: &str,
        raw_extra: serde_json::Value,
    ) -> serde_json::Value {
        let mut raw = serde_json::json!({
            "hook_event_name": hook_event_name,
            "tool_name": tool_name,
        });
        if let Some(obj) = raw_extra.as_object() {
            for (k, v) in obj {
                raw[k.clone()] = v.clone();
            }
        }
        serde_json::json!({
            "ts": "2026-02-14T10:00:00Z",
            "project_id": "test_proj",
            "session_id": "test_session",
            "hook_event_name": hook_event_name,
            "tool_name": tool_name,
            "tool_use_id": "",
            "raw": raw,
        })
    }

    fn make_envelope_at(
        hook_event_name: &str,
        tool_name: &str,
        ts: &str,
        raw_extra: serde_json::Value,
    ) -> serde_json::Value {
        let mut e = make_envelope(hook_event_name, tool_name, raw_extra);
        e["ts"] = serde_json::Value::String(ts.to_string());
        e
    }

    #[test]
    fn digest_empty_session() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.jsonl");
        std::fs::write(&path, "").unwrap();

        let event = extract_session_digest(&path, "sess-empty", "main", None).unwrap();
        assert_eq!(event.event_type, "note");
        assert_eq!(event.payload["source"], "bridge:session_digest");
        assert_eq!(event.payload["session_stats"]["tool_calls"], 0);
        assert_eq!(event.payload["session_stats"]["user_prompts"], 0);
        assert!(event.event_id.starts_with("evt_"));
        assert!(!event.hash.is_empty());
    }

    #[test]
    fn digest_counts_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope("PostToolUse", "Bash", serde_json::json!({})),
            make_envelope(
                "PostToolUse",
                "Edit",
                serde_json::json!({
                    "tool_input": { "file_path": "/src/main.rs" }
                }),
            ),
            make_envelope("PostToolUse", "Read", serde_json::json!({})),
            make_envelope(
                "PostToolUseFailure",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "cargo test" }
                }),
            ),
            make_envelope("UserPromptSubmit", "", serde_json::json!({})),
            make_envelope("UserPromptSubmit", "", serde_json::json!({})),
        ];
        let path = write_session_ledger(tmp.path(), &lines);
        let stats = extract_stats(&path).unwrap();

        assert_eq!(stats.tool_calls, 3);
        assert_eq!(stats.tool_failures, 1);
        assert_eq!(stats.user_prompts, 2);
    }

    #[test]
    fn digest_extracts_files() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope(
                "PostToolUse",
                "Edit",
                serde_json::json!({
                    "tool_input": { "file_path": "/src/main.rs" }
                }),
            ),
            make_envelope(
                "PostToolUse",
                "Write",
                serde_json::json!({
                    "tool_input": { "file_path": "/src/lib.rs" }
                }),
            ),
            make_envelope(
                "PostToolUse",
                "Edit",
                serde_json::json!({
                    "tool_input": { "file_path": "/src/main.rs" }
                }),
            ),
            make_envelope("PostToolUse", "Read", serde_json::json!({})),
        ];
        let path = write_session_ledger(tmp.path(), &lines);
        let stats = extract_stats(&path).unwrap();

        assert_eq!(stats.files_modified.len(), 2);
        assert!(stats.files_modified.contains(&"/src/lib.rs".to_string()));
        assert!(stats.files_modified.contains(&"/src/main.rs".to_string()));
    }

    #[test]
    fn digest_extracts_failed_cmds() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope(
                "PostToolUseFailure",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "cargo test --all" }
                }),
            ),
            make_envelope("PostToolUseFailure", "Edit", serde_json::json!({})),
            make_envelope(
                "PostToolUseFailure",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "npm run build" }
                }),
            ),
        ];
        let path = write_session_ledger(tmp.path(), &lines);
        let stats = extract_stats(&path).unwrap();

        assert_eq!(stats.tool_failures, 3);
        assert_eq!(stats.failed_commands.len(), 2);
        assert_eq!(stats.failed_commands[0], "cargo test --all");
        assert_eq!(stats.failed_commands[1], "npm run build");
    }

    #[test]
    fn digest_event_has_provenance() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("prov.jsonl");
        std::fs::write(&path, "").unwrap();

        let event = extract_session_digest(&path, "sess-abc123", "main", None).unwrap();
        assert_eq!(event.refs.provenance.len(), 1);
        assert_eq!(event.refs.provenance[0].target, "session:sess-abc123");
        assert_eq!(event.refs.provenance[0].rel, "based_on");
        assert!(event.refs.provenance[0].note.is_some());
    }

    #[test]
    fn digest_payload_has_source() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![make_envelope("PostToolUse", "Bash", serde_json::json!({}))];
        let path = write_session_ledger(tmp.path(), &lines);

        let event = extract_session_digest(&path, "sess-src", "main", None).unwrap();
        assert_eq!(event.payload["source"], "bridge:session_digest");
        assert_eq!(event.payload["role"], "system");
        let tags = event.payload["tags"].as_array().unwrap();
        assert!(tags.iter().any(|t| t.as_str() == Some("session_digest")));
    }

    #[test]
    fn digest_duration_computed() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope_at(
                "UserPromptSubmit",
                "",
                "2026-02-14T10:00:00Z",
                serde_json::json!({}),
            ),
            make_envelope_at(
                "PostToolUse",
                "Bash",
                "2026-02-14T10:35:00Z",
                serde_json::json!({}),
            ),
        ];
        let path = write_session_ledger(tmp.path(), &lines);
        let stats = extract_stats(&path).unwrap();

        assert_eq!(stats.duration_minutes, 35);
    }

    #[test]
    fn digest_extracts_commits_from_bash() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope(
                "PostToolUse",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "git commit -m \"fix: resolve UTF-8 truncation\"" }
                }),
            ),
            make_envelope(
                "PostToolUse",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "cargo test --all" }
                }),
            ),
            make_envelope(
                "PostToolUse",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "git add . && git commit -m 'feat: add digest'" }
                }),
            ),
        ];
        let path = write_session_ledger(tmp.path(), &lines);
        let stats = extract_stats(&path).unwrap();

        assert_eq!(stats.commits_made.len(), 2);
        assert_eq!(stats.commits_made[0], "fix: resolve UTF-8 truncation");
        assert_eq!(stats.commits_made[1], "feat: add digest");
    }

    #[test]
    fn digest_commits_in_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![make_envelope(
            "PostToolUse",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "git commit -m \"fix: something\"" }
            }),
        )];
        let path = write_session_ledger(tmp.path(), &lines);
        let event = extract_session_digest(&path, "sess-commits", "main", None).unwrap();

        let commits = event.payload["session_stats"]["commits_made"]
            .as_array()
            .unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0], "fix: something");

        // Also in text
        let text = event.payload["text"].as_str().unwrap();
        assert!(text.contains("Commits:"));
        assert!(text.contains("fix: something"));
    }

    #[test]
    fn outcome_completed_normal_session() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope("UserPromptSubmit", "", serde_json::json!({})),
            make_envelope("PostToolUse", "Read", serde_json::json!({})),
            make_envelope("PostToolUse", "Edit", serde_json::json!({})),
        ];
        let path = write_session_ledger(tmp.path(), &lines);
        let stats = extract_stats(&path).unwrap();
        assert_eq!(stats.outcome, SessionOutcome::Completed);
    }

    #[test]
    fn outcome_interrupted_last_is_user_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope("PostToolUse", "Read", serde_json::json!({})),
            make_envelope("UserPromptSubmit", "", serde_json::json!({})),
        ];
        let path = write_session_ledger(tmp.path(), &lines);
        let stats = extract_stats(&path).unwrap();
        assert_eq!(stats.outcome, SessionOutcome::Interrupted);
    }

    #[test]
    fn outcome_error_stuck_three_consecutive_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope("PostToolUse", "Edit", serde_json::json!({})),
            make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
            make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
            make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
        ];
        let path = write_session_ledger(tmp.path(), &lines);
        let stats = extract_stats(&path).unwrap();
        assert_eq!(stats.outcome, SessionOutcome::ErrorStuck);
    }

    #[test]
    fn outcome_not_stuck_if_success_resets_count() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
            make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
            make_envelope("PostToolUse", "Edit", serde_json::json!({})), // resets
            make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
        ];
        let path = write_session_ledger(tmp.path(), &lines);
        let stats = extract_stats(&path).unwrap();
        assert_eq!(stats.outcome, SessionOutcome::Completed);
    }

    #[test]
    fn outcome_in_digest_payload() {
        let stats = SessionStats {
            outcome: SessionOutcome::ErrorStuck,
            ..Default::default()
        };
        let event = build_digest_event("sess-outcome", &stats, "main", None, &[]).unwrap();
        assert_eq!(
            event.payload["session_stats"]["outcome"].as_str().unwrap(),
            "error_stuck"
        );
    }

    #[test]
    fn digest_tasks_snapshot_in_payload() {
        let stats = SessionStats {
            tool_calls: 5,
            tasks_snapshot: vec![
                DigestTaskSnapshot {
                    subject: "Fix auth bug".to_string(),
                    status: "completed".to_string(),
                },
                DigestTaskSnapshot {
                    subject: "Add tests".to_string(),
                    status: "in_progress".to_string(),
                },
            ],
            ..Default::default()
        };

        let event = build_digest_event("sess-tasks", &stats, "main", None, &[]).unwrap();

        // Check payload
        let tasks = event.payload["session_stats"]["tasks_snapshot"]
            .as_array()
            .unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0]["subject"], "Fix auth bug");
        assert_eq!(tasks[0]["status"], "completed");
        assert_eq!(tasks[1]["subject"], "Add tests");
        assert_eq!(tasks[1]["status"], "in_progress");

        // Check text rendering
        let text = event.payload["text"].as_str().unwrap();
        assert!(text.contains("Done: Fix auth bug"), "text: {text}");
        assert!(text.contains("WIP: Add tests"), "text: {text}");
    }

    #[test]
    fn extract_git_commit_msg_works() {
        assert_eq!(
            extract_git_commit_msg(r#"git commit -m "fix: something""#),
            "fix: something"
        );
        assert_eq!(
            extract_git_commit_msg("git commit -m 'feat: new'"),
            "feat: new"
        );
        assert_eq!(extract_git_commit_msg("git add . && git commit"), "");
    }

    #[test]
    fn digest_nonexistent_file_returns_empty_stats() {
        let path = Path::new("/nonexistent/session.jsonl");
        let stats = extract_stats(path).unwrap();
        assert_eq!(stats.tool_calls, 0);
        assert_eq!(stats.user_prompts, 0);
    }

    #[test]
    fn digest_hash_chain_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("chain.jsonl");
        std::fs::write(&path, "").unwrap();

        let e1 = extract_session_digest(&path, "sess-1", "main", None).unwrap();
        let e2 = extract_session_digest(&path, "sess-2", "main", Some(&e1.hash)).unwrap();

        assert!(e1.parent_hash.is_none());
        assert_eq!(e2.parent_hash.as_deref(), Some(e1.hash.as_str()));
        assert_ne!(e1.hash, e2.hash);
        assert_eq!(e1.digests.len(), 1);
        assert_eq!(e2.digests.len(), 1);
    }

    // ── Auto-Digest Integration Tests ──

    /// Create a workspace (.edda/) and a fake store with a session ledger.
    /// Returns (workspace_root, fake_project_id, session_id).
    fn setup_digest_workspace(tmp: &Path) -> (std::path::PathBuf, String) {
        // Create workspace
        let workspace = tmp.join("repo");
        let paths = edda_ledger::EddaPaths::discover(&workspace);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();

        // Use the workspace path as project_id source
        let project_id = edda_store::project_id(&workspace);
        let _ = edda_store::ensure_dirs(&project_id);

        (workspace, project_id)
    }

    fn write_store_session_ledger(project_id: &str, session_id: &str, lines: &[serde_json::Value]) {
        let dir = edda_store::project_dir(project_id).join("ledger");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{session_id}.jsonl"));
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{}", serde_json::to_string(line).unwrap()).unwrap();
        }
    }

    #[test]
    fn digest_writes_to_workspace_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let (workspace, project_id) = setup_digest_workspace(tmp.path());

        // Create a previous session's ledger in the store
        let prev_session = "prev-session-001";
        write_store_session_ledger(
            &project_id,
            prev_session,
            &[
                make_envelope("PostToolUse", "Bash", serde_json::json!({})),
                make_envelope("UserPromptSubmit", "", serde_json::json!({})),
            ],
        );

        let result = digest_previous_sessions(
            &project_id,
            "current-session-002",
            workspace.to_str().unwrap(),
            2000,
        );

        assert!(matches!(result, DigestResult::Written { .. }));

        // Verify event in workspace ledger
        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "note");
        assert_eq!(events[0].payload["source"], "bridge:session_digest");
        assert_eq!(events[0].payload["session_id"], prev_session);
    }

    #[test]
    fn digest_maintains_hash_chain() {
        let tmp = tempfile::tempdir().unwrap();
        let (workspace, project_id) = setup_digest_workspace(tmp.path());

        // Write two previous sessions
        write_store_session_ledger(
            &project_id,
            "sess-aaa",
            &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
        );
        write_store_session_ledger(
            &project_id,
            "sess-bbb",
            &[make_envelope("PostToolUse", "Edit", serde_json::json!({}))],
        );

        // Digest first
        let r1 =
            digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
        assert!(matches!(r1, DigestResult::Written { .. }));

        // Digest second
        let r2 =
            digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
        assert!(matches!(r2, DigestResult::Written { .. }));

        // Verify hash chain
        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[0].parent_hash.is_none());
        assert_eq!(
            events[1].parent_hash.as_deref(),
            Some(events[0].hash.as_str())
        );
    }

    #[test]
    fn digest_skips_already_digested() {
        let tmp = tempfile::tempdir().unwrap();
        let (workspace, project_id) = setup_digest_workspace(tmp.path());

        write_store_session_ledger(
            &project_id,
            "sess-once",
            &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
        );

        let ledger_dir = edda_store::project_dir(&project_id).join("ledger");

        // Digest once
        let r1 =
            digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
        assert!(matches!(r1, DigestResult::Written { .. }));

        // Session ledger file should be deleted after successful digest
        assert!(
            !ledger_dir.join("sess-once.jsonl").exists(),
            "session ledger file should be removed after successful digest"
        );

        // Digest again — should be NoPending (file is gone, not re-discovered)
        let r2 =
            digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
        assert!(matches!(r2, DigestResult::NoPending));

        // Workspace ledger should still have exactly 1 event
        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        assert_eq!(ledger.iter_events().unwrap().len(), 1);
    }

    #[test]
    fn digest_no_reduplicate_across_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let (workspace, project_id) = setup_digest_workspace(tmp.path());

        // Create 3 session ledger files
        for sid in &["sess-001", "sess-002", "sess-003"] {
            write_store_session_ledger(
                &project_id,
                sid,
                &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
            );
        }

        let ws = workspace.to_str().unwrap();
        let ledger_dir = edda_store::project_dir(&project_id).join("ledger");

        // digest_previous_sessions processes one session per call.
        // Call it 3 times to digest all 3, then once more to confirm NoPending.
        for _ in 0..3 {
            let r = digest_previous_sessions(&project_id, "sess-A", ws, 2000);
            assert!(matches!(r, DigestResult::Written { .. }));
        }

        // All 3 session ledger files should be removed
        assert!(!ledger_dir.join("sess-001.jsonl").exists());
        assert!(!ledger_dir.join("sess-002.jsonl").exists());
        assert!(!ledger_dir.join("sess-003.jsonl").exists());

        // Next call: no pending sessions
        let r = digest_previous_sessions(&project_id, "sess-B", ws, 2000);
        assert!(matches!(r, DigestResult::NoPending));

        // Workspace ledger should have exactly 3 digest events (not more)
        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        assert_eq!(
            ledger.iter_events().unwrap().len(),
            3,
            "should have exactly 3 digest events, no duplicates"
        );
    }

    #[test]
    fn digest_no_workspace_records_failure() {
        let tmp = tempfile::tempdir().unwrap();
        // No workspace created — just a store
        let project_id = "fake_project_no_workspace";
        let _ = edda_store::ensure_dirs(project_id);
        // Reset state and ledger dir from previous test runs
        save_digest_state(project_id, &DigestState::default()).unwrap();
        let ledger_dir = edda_store::project_dir(project_id).join("ledger");
        let _ = std::fs::remove_dir_all(&ledger_dir);

        write_store_session_ledger(
            project_id,
            "sess-fail",
            &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
        );

        let result = digest_previous_sessions(
            project_id,
            "current",
            tmp.path().to_str().unwrap(), // no .edda here
            2000,
        );

        assert!(matches!(result, DigestResult::Error(_)));

        // State should record the failure
        let state = load_digest_state(project_id);
        assert_eq!(state.pending_session_id, "sess-fail");
        assert_eq!(state.retry_count, 1);
    }

    #[test]
    fn digest_permanent_failure_after_3_retries() {
        let tmp = tempfile::tempdir().unwrap();
        let project_id = "fake_project_perm_fail";
        let _ = edda_store::ensure_dirs(project_id);
        // Reset ledger dir from previous test runs
        let ledger_dir = edda_store::project_dir(project_id).join("ledger");
        let _ = std::fs::remove_dir_all(&ledger_dir);

        // Manually set state to 3 retries
        let state = DigestState {
            pending_session_id: "sess-stuck".to_string(),
            retry_count: 3,
            last_error: "lock timeout".to_string(),
            ..Default::default()
        };
        save_digest_state(project_id, &state).unwrap();

        write_store_session_ledger(
            project_id,
            "sess-stuck",
            &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
        );

        let result =
            digest_previous_sessions(project_id, "current", tmp.path().to_str().unwrap(), 2000);

        assert!(matches!(result, DigestResult::PermanentFailure(_)));
        if let DigestResult::PermanentFailure(msg) = result {
            assert!(msg.contains("sess-stu"));
            assert!(msg.contains("edda bridge digest"));
        }
    }

    #[test]
    fn digest_state_round_trip() {
        let project_id = "test_state_rt";
        let _ = edda_store::ensure_dirs(project_id);

        let state = DigestState {
            session_id: "sess-123".to_string(),
            digested_at: "2026-02-14T10:00:00Z".to_string(),
            event_id: "evt_abc".to_string(),
            retry_count: 0,
            pending_session_id: String::new(),
            last_error: String::new(),
        };
        save_digest_state(project_id, &state).unwrap();

        let loaded = load_digest_state(project_id);
        assert_eq!(loaded.session_id, "sess-123");
        assert_eq!(loaded.event_id, "evt_abc");
        assert_eq!(loaded.retry_count, 0);
    }

    // ── #32 Tests: failed cmd milestones + CLI digest ──

    #[test]
    fn failed_cmd_milestone_produced() {
        let failed = FailedCommand {
            command: "cargo test --fail".to_string(),
            cwd: "/project".to_string(),
            exit_code: 1,
        };
        let event = build_cmd_milestone_event("sess-cmd-1", &failed, "main", None).unwrap();

        assert_eq!(event.event_type, "cmd");
        assert_eq!(event.payload["source"], "bridge:cmd");
        assert_eq!(event.payload["exit_code"], 1);
        assert_eq!(event.payload["argv"][0], "cargo test --fail");
        assert_eq!(event.payload["cwd"], "/project");
        assert_eq!(event.payload["session_id"], "sess-cmd-1");
    }

    #[test]
    fn failed_cmd_milestone_has_provenance() {
        let failed = FailedCommand {
            command: "npm install".to_string(),
            cwd: ".".to_string(),
            exit_code: 127,
        };
        let event = build_cmd_milestone_event("sess-prov-1", &failed, "main", None).unwrap();

        assert!(!event.refs.provenance.is_empty());
        assert_eq!(event.refs.provenance[0].target, "session:sess-prov-1");
        assert_eq!(event.refs.provenance[0].rel, "based_on");
    }

    #[test]
    fn failed_cmd_milestone_chains_hash() {
        let failed = FailedCommand {
            command: "make build".to_string(),
            cwd: ".".to_string(),
            exit_code: 2,
        };
        let parent = "abc123";
        let event = build_cmd_milestone_event("sess-chain", &failed, "main", Some(parent)).unwrap();

        assert_eq!(event.parent_hash.as_deref(), Some("abc123"));
        assert!(!event.hash.is_empty());
    }

    #[test]
    fn extract_stats_captures_failed_cmd_detail() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sess-detail.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        // PostToolUseFailure with real Claude Code format: error field, no toolResponse
        let envelope = serde_json::json!({
            "ts": "2026-02-14T10:00:00Z",
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "cwd": "/my/project",
            "raw": {
                "tool_name": "Bash",
                "tool_input": { "command": "cargo build" },
                "error": "Exit code 101\nerror[E0308]: mismatched types"
            }
        });
        writeln!(f, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();

        let stats = extract_stats(&path).unwrap();
        assert_eq!(stats.failed_cmds_detail.len(), 1);
        assert_eq!(stats.failed_cmds_detail[0].command, "cargo build");
        assert_eq!(stats.failed_cmds_detail[0].cwd, "/my/project");
        assert_eq!(stats.failed_cmds_detail[0].exit_code, 101);
    }

    #[test]
    fn extract_exit_code_from_error_field() {
        // Real Claude Code PostToolUseFailure format
        let envelope = serde_json::json!({
            "raw": {
                "error": "Exit code 49",
                "tool_name": "Bash",
                "tool_input": { "command": "python3 --version" }
            }
        });
        assert_eq!(extract_exit_code(&envelope), 49);

        // Error with multiline detail
        let envelope2 = serde_json::json!({
            "raw": {
                "error": "Exit code 128\nfatal: not a git repository"
            }
        });
        assert_eq!(extract_exit_code(&envelope2), 128);

        // Legacy camelCase toolResponse.exitCode still works
        let envelope3 = serde_json::json!({
            "raw": {
                "toolResponse": { "exitCode": 42 }
            }
        });
        assert_eq!(extract_exit_code(&envelope3), 42);

        // No raw → default 1
        let envelope4 = serde_json::json!({});
        assert_eq!(extract_exit_code(&envelope4), 1);
    }

    #[test]
    fn digest_writes_cmd_milestones_to_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let (workspace, project_id) = setup_digest_workspace(tmp.path());

        // Write session with a failed Bash command (real Claude Code format)
        write_store_session_ledger(
            &project_id,
            "sess-cmd-ws",
            &[
                make_envelope("PostToolUse", "Bash", serde_json::json!({})),
                serde_json::json!({
                    "ts": "2026-02-14T10:01:00Z",
                    "hook_event_name": "PostToolUseFailure",
                    "tool_name": "Bash",
                    "cwd": "/proj",
                    "raw": {
                        "tool_name": "Bash",
                        "tool_input": { "command": "failing-cmd" },
                        "error": "Exit code 1"
                    }
                }),
            ],
        );

        let result = digest_previous_sessions_with_opts(
            &project_id,
            "current",
            workspace.to_str().unwrap(),
            2000,
            true,
        );
        assert!(matches!(result, DigestResult::Written { .. }));

        // Workspace should have 2 events: note digest + cmd milestone
        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "note");
        assert_eq!(events[1].event_type, "cmd");
        assert_eq!(events[1].payload["source"], "bridge:cmd");
        // Hash chain: second event parents the first
        assert_eq!(
            events[1].parent_hash.as_deref(),
            Some(events[0].hash.as_str())
        );
    }

    #[test]
    fn digest_skips_cmd_milestones_when_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let (workspace, project_id) = setup_digest_workspace(tmp.path());

        write_store_session_ledger(
            &project_id,
            "sess-no-cmd",
            &[serde_json::json!({
                "ts": "2026-02-14T10:01:00Z",
                "hook_event_name": "PostToolUseFailure",
                "tool_name": "Bash",
                "cwd": "/proj",
                "raw": {
                    "tool_name": "Bash",
                    "tool_input": { "command": "fail-cmd" },
                    "error": "Exit code 1"
                }
            })],
        );

        // digest_failed_cmds = false
        let result = digest_previous_sessions_with_opts(
            &project_id,
            "current",
            workspace.to_str().unwrap(),
            2000,
            false,
        );
        assert!(matches!(result, DigestResult::Written { .. }));

        // Only 1 event (note digest, no cmd)
        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "note");
    }

    #[test]
    fn manual_digest_specific_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (workspace, project_id) = setup_digest_workspace(tmp.path());

        write_store_session_ledger(
            &project_id,
            "sess-manual",
            &[
                make_envelope("PostToolUse", "Edit", serde_json::json!({})),
                make_envelope("PostToolUse", "Bash", serde_json::json!({})),
            ],
        );

        let event_id = digest_session_manual(
            &project_id,
            "sess-manual",
            workspace.to_str().unwrap(),
            true,
        )
        .unwrap();

        assert!(event_id.starts_with("evt_"));

        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        let events = ledger.iter_events().unwrap();
        assert!(!events.is_empty());
        assert_eq!(events[0].event_type, "note");
        assert_eq!(events[0].payload["source"], "bridge:session_digest");
    }

    // ── PrevDigest tests ──

    #[test]
    fn prev_digest_roundtrip() {
        let pid = "test_prev_digest_rt";
        let _ = edda_store::ensure_dirs(pid);

        let stats = SessionStats {
            tasks_snapshot: vec![
                DigestTaskSnapshot {
                    subject: "Fix bug".into(),
                    status: "completed".into(),
                },
                DigestTaskSnapshot {
                    subject: "Add tests".into(),
                    status: "completed".into(),
                },
                DigestTaskSnapshot {
                    subject: "Deploy".into(),
                    status: "pending".into(),
                },
            ],
            commits_made: vec!["fix: auth flow".into(), "feat: add billing".into()],
            files_modified: vec!["src/lib.rs".into(), "src/main.rs".into()],
            duration_minutes: 25,
            outcome: SessionOutcome::Completed,
            ..Default::default()
        };

        write_prev_digest(pid, "test-sess", &stats, vec![], vec![]);

        let digest = read_prev_digest(pid).expect("should read prev_digest");
        assert_eq!(digest.session_id, "test-sess");
        assert_eq!(digest.outcome, "completed");
        assert_eq!(digest.duration_minutes, 25);
        assert_eq!(digest.completed_tasks, vec!["Fix bug", "Add tests"]);
        assert_eq!(digest.pending_tasks, vec!["Deploy"]);
        assert_eq!(digest.commits.len(), 2);
        assert_eq!(digest.files_modified_count, 2);

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn prev_digest_empty_tasks() {
        let pid = "test_prev_digest_empty";
        let _ = edda_store::ensure_dirs(pid);

        let stats = SessionStats {
            commits_made: vec!["chore: cleanup".into()],
            files_modified: vec!["README.md".into()],
            duration_minutes: 5,
            outcome: SessionOutcome::Interrupted,
            ..Default::default()
        };

        write_prev_digest(pid, "test-empty", &stats, vec![], vec![]);

        let digest = read_prev_digest(pid).expect("should read prev_digest");
        assert!(digest.completed_tasks.is_empty());
        assert!(digest.pending_tasks.is_empty());
        assert_eq!(digest.commits.len(), 1);
        assert_eq!(digest.outcome, "interrupted");

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn prev_digest_with_decisions_and_notes() {
        let pid = "test_prev_digest_dn";
        let _ = edda_store::ensure_dirs(pid);

        let stats = SessionStats {
            commits_made: vec!["feat: add auth".into()],
            files_modified: vec!["src/auth.rs".into()],
            failed_commands: vec!["cargo test".into()],
            duration_minutes: 20,
            outcome: SessionOutcome::Completed,
            ..Default::default()
        };
        write_prev_digest(
            pid,
            "test-dn",
            &stats,
            vec!["auth=jwt (stateless)".into(), "db=postgres".into()],
            vec!["OAuth deferred — needs client registration".into()],
        );

        let loaded = read_prev_digest(pid).expect("should read enriched prev_digest");
        assert_eq!(loaded.decisions.len(), 2);
        assert_eq!(loaded.decisions[0], "auth=jwt (stateless)");
        assert_eq!(loaded.notes.len(), 1);
        assert!(loaded.notes[0].contains("OAuth"));
        assert_eq!(loaded.failed_commands, vec!["cargo test"]);

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn prev_digest_backward_compat() {
        let pid = "test_prev_digest_compat";
        let _ = edda_store::ensure_dirs(pid);

        // Write old-format JSON without new fields
        let old_json = serde_json::json!({
            "session_id": "old-sess",
            "completed_at": "2026-02-17T10:00:00Z",
            "outcome": "completed",
            "duration_minutes": 10,
            "completed_tasks": ["Fix bug"],
            "pending_tasks": [],
            "commits": ["fix: bug"],
            "files_modified_count": 1,
            "total_edits": 5
        });
        let path = edda_store::project_dir(pid)
            .join("state")
            .join("prev_digest.json");
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        std::fs::write(&path, serde_json::to_string_pretty(&old_json).unwrap()).unwrap();

        let digest = read_prev_digest(pid).expect("old format should deserialize");
        assert_eq!(digest.session_id, "old-sess");
        assert!(
            digest.decisions.is_empty(),
            "decisions should default to empty"
        );
        assert!(digest.notes.is_empty(), "notes should default to empty");
        assert!(
            digest.failed_commands.is_empty(),
            "failed_commands should default to empty"
        );

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn collect_session_ledger_extras_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        let paths = edda_ledger::EddaPaths::discover(&workspace);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        let branch = ledger.head_branch().unwrap();

        // Write a decision event
        let tags_d = vec!["decision".to_string()];
        let mut evt =
            edda_core::event::new_note_event(&branch, None, "system", "auth: jwt", &tags_d)
                .unwrap();
        evt.payload["decision"] =
            serde_json::json!({"key": "auth", "value": "jwt", "reason": "stateless"});
        edda_core::event::finalize_event(&mut evt);
        let decision_ts = evt.ts.clone();
        ledger.append_event(&evt).unwrap();

        // Write a session note
        let tags_s = vec!["session".to_string()];
        let evt2 = edda_core::event::new_note_event(
            &branch,
            Some(&evt.hash),
            "user",
            "completed auth, next OAuth",
            &tags_s,
        )
        .unwrap();
        ledger.append_event(&evt2).unwrap();

        let (decisions, notes) =
            collect_session_ledger_extras(workspace.to_str().unwrap(), Some(&decision_ts));
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].contains("auth=jwt"), "got: {}", decisions[0]);
        assert!(decisions[0].contains("stateless"), "got: {}", decisions[0]);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("completed auth"), "got: {}", notes[0]);
    }

    #[test]
    fn collect_session_ledger_extras_excludes_digest_notes() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        let paths = edda_ledger::EddaPaths::discover(&workspace);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        let branch = ledger.head_branch().unwrap();

        // Write an auto-generated digest note (source: "bridge:session_digest")
        let tags = vec!["session_digest".to_string()];
        let mut evt = edda_core::event::new_note_event(
            &branch,
            None,
            "system",
            "Session abc: 10 tool calls",
            &tags,
        )
        .unwrap();
        evt.payload["source"] = serde_json::json!("bridge:session_digest");
        edda_core::event::finalize_event(&mut evt);
        let ts = evt.ts.clone();
        ledger.append_event(&evt).unwrap();

        let (decisions, notes) =
            collect_session_ledger_extras(workspace.to_str().unwrap(), Some(&ts));
        assert!(decisions.is_empty(), "auto-digest should be excluded");
        assert!(notes.is_empty(), "auto-digest should be excluded");
    }

    #[test]
    fn collect_session_ledger_extras_no_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        // No .edda/ directory
        let (decisions, notes) = collect_session_ledger_extras(
            tmp.path().to_str().unwrap(),
            Some("2026-02-17T10:00:00Z"),
        );
        assert!(decisions.is_empty());
        assert!(notes.is_empty());
    }

    #[test]
    fn digest_skips_empty_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (workspace, project_id) = setup_digest_workspace(tmp.path());

        // Write a session with only SessionStart (no tool calls, no user prompts)
        write_store_session_ledger(
            &project_id,
            "sess-empty-skip",
            &[make_envelope("SessionStart", "", serde_json::json!({}))],
        );

        let result =
            digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);

        // Should skip (NoPending), not write to workspace ledger
        assert!(matches!(result, DigestResult::NoPending), "got: {result:?}");

        // Workspace ledger should have 0 events
        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        assert_eq!(ledger.iter_events().unwrap().len(), 0);

        // Session ledger file should be cleaned up
        let session_path = edda_store::project_dir(&project_id)
            .join("ledger")
            .join("sess-empty-skip.jsonl");
        assert!(
            !session_path.exists(),
            "empty session ledger should be removed"
        );

        // State should mark as processed to avoid re-processing
        let state = load_digest_state(&project_id);
        assert_eq!(state.session_id, "sess-empty-skip");
    }

    // ── Recall Rate tests ──

    #[test]
    fn digest_payload_has_recall_fields() {
        let stats = SessionStats {
            tool_calls: 10,
            nudge_count: 3,
            decide_count: 1,
            ..Default::default()
        };
        let event = build_digest_event("sess-recall", &stats, "main", None, &[]).unwrap();
        assert_eq!(event.payload["session_stats"]["nudge_count"], 3);
        assert_eq!(event.payload["session_stats"]["decide_count"], 1);
    }

    #[test]
    fn digest_event_contains_notes() {
        let stats = SessionStats {
            tool_calls: 5,
            outcome: SessionOutcome::Completed,
            ..Default::default()
        };
        let notes = vec![
            "Switched to JWT auth approach".to_string(),
            "Need to revisit caching strategy".to_string(),
        ];
        let event = build_digest_event("sess-notes", &stats, "main", None, &notes).unwrap();

        let payload_notes = event.payload["session_stats"]["notes"]
            .as_array()
            .expect("notes should be an array");
        assert_eq!(payload_notes.len(), 2);
        assert_eq!(
            payload_notes[0].as_str().unwrap(),
            "Switched to JWT auth approach"
        );
        assert_eq!(
            payload_notes[1].as_str().unwrap(),
            "Need to revisit caching strategy"
        );
    }

    #[test]
    fn digest_event_empty_notes_backward_compat() {
        let stats = SessionStats::default();
        let event = build_digest_event("sess-no-notes", &stats, "main", None, &[]).unwrap();

        let payload_notes = event.payload["session_stats"]["notes"]
            .as_array()
            .expect("notes should be an array even when empty");
        assert!(payload_notes.is_empty());
    }

    #[test]
    fn prev_digest_has_recall_fields() {
        let pid = "test_prev_digest_recall";
        let _ = edda_store::ensure_dirs(pid);

        let stats = SessionStats {
            nudge_count: 5,
            decide_count: 2,
            duration_minutes: 15,
            outcome: SessionOutcome::Completed,
            ..Default::default()
        };
        write_prev_digest(pid, "test-recall", &stats, vec![], vec![]);

        let digest = read_prev_digest(pid).expect("should read prev_digest");
        assert_eq!(digest.nudge_count, 5);
        assert_eq!(digest.decide_count, 2);

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── signal_count + deps_added tests ──

    #[test]
    fn digest_payload_has_signal_count() {
        let stats = SessionStats {
            tool_calls: 10,
            nudge_count: 3,
            decide_count: 1,
            signal_count: 5,
            ..Default::default()
        };
        let event = build_digest_event("sess-signal", &stats, "main", None, &[]).unwrap();
        assert_eq!(event.payload["session_stats"]["signal_count"], 5);
    }

    #[test]
    fn digest_extracts_deps_added() {
        let dir = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope(
                "PostToolUse",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "cargo add serde" }
                }),
            ),
            make_envelope(
                "PostToolUse",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "npm install express" }
                }),
            ),
            make_envelope(
                "PostToolUse",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "pnpm add zod" }
                }),
            ),
            // Bare npm install (no package) → NOT captured
            make_envelope(
                "PostToolUse",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "npm install" }
                }),
            ),
        ];
        let path = write_session_ledger(dir.path(), &lines);
        let stats = extract_stats(&path).unwrap();
        assert_eq!(stats.deps_added, vec!["serde", "express", "zod"]);
    }

    #[test]
    fn digest_extracts_deps_added_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let lines = vec![
            make_envelope(
                "PostToolUse",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "cargo add serde" }
                }),
            ),
            make_envelope(
                "PostToolUse",
                "Bash",
                serde_json::json!({
                    "tool_input": { "command": "cargo add serde --features derive" }
                }),
            ),
        ];
        let path = write_session_ledger(dir.path(), &lines);
        let stats = extract_stats(&path).unwrap();
        assert_eq!(
            stats.deps_added,
            vec!["serde"],
            "duplicate deps should be deduped"
        );
    }

    // ── Passive harvest tests ──

    #[test]
    fn passive_harvest_writes_inferred_decision() {
        let dir = tempfile::tempdir().unwrap();
        let paths = edda_ledger::EddaPaths::discover(dir.path());
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        let ledger = edda_ledger::Ledger::open(dir.path()).unwrap();

        let stats = SessionStats {
            deps_added: vec!["jsonwebtoken".to_string()],
            commits_made: vec!["feat: add auth middleware".to_string()],
            tasks_snapshot: vec![DigestTaskSnapshot {
                subject: "Add JWT authentication".to_string(),
                status: "in_progress".to_string(),
            }],
            ..Default::default()
        };

        let ids = harvest_inferred_decisions(
            "sess-harvest",
            &stats,
            &[], // no decisions recorded
            &ledger,
            "main",
            None,
        );

        assert_eq!(ids.len(), 1, "should write one inferred decision");

        // Verify the event in the ledger
        let events = ledger.iter_events().unwrap();
        let last = events.iter().last().unwrap();
        assert_eq!(last.event_type, "note");
        assert_eq!(last.payload["source"], "bridge:passive_harvest");
        assert_eq!(last.payload["decision"]["key"], "dep.jsonwebtoken");
        assert_eq!(last.payload["decision"]["value"], "jsonwebtoken");

        let tags: Vec<&str> = last.payload["tags"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(tags.contains(&"decision"));
        assert!(tags.contains(&"inferred"));
    }

    #[test]
    fn passive_harvest_skips_already_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let paths = edda_ledger::EddaPaths::discover(dir.path());
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        let ledger = edda_ledger::Ledger::open(dir.path()).unwrap();

        let stats = SessionStats {
            deps_added: vec!["serde".to_string()],
            ..Default::default()
        };

        // Agent already recorded a decision mentioning "serde"
        let decisions = vec!["dep.serde=serde (serialization)".to_string()];

        let ids =
            harvest_inferred_decisions("sess-skip", &stats, &decisions, &ledger, "main", None);

        assert!(
            ids.is_empty(),
            "should NOT write inferred decision when already recorded"
        );
    }

    #[test]
    fn passive_harvest_includes_context_hint() {
        let stats = SessionStats {
            tasks_snapshot: vec![DigestTaskSnapshot {
                subject: "Add JWT authentication".to_string(),
                status: "in_progress".to_string(),
            }],
            commits_made: vec!["feat: add auth middleware".to_string()],
            ..Default::default()
        };

        let hint = build_context_hint(&stats);
        assert!(
            hint.contains("Add JWT authentication"),
            "should contain task subject"
        );
        assert!(
            hint.contains("feat: add auth middleware"),
            "should contain commit message"
        );
    }

    #[test]
    fn passive_harvest_context_hint_fallback() {
        let stats = SessionStats::default();
        let hint = build_context_hint(&stats);
        assert_eq!(hint, "(auto-inferred)");
    }

    #[test]
    fn passive_harvest_empty_deps_no_events() {
        let dir = tempfile::tempdir().unwrap();
        let paths = edda_ledger::EddaPaths::discover(dir.path());
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        let ledger = edda_ledger::Ledger::open(dir.path()).unwrap();

        let stats = SessionStats::default(); // no deps_added

        let ids = harvest_inferred_decisions("sess-empty", &stats, &[], &ledger, "main", None);

        assert!(ids.is_empty(), "empty deps_added should produce no events");
    }

    #[test]
    fn prev_digest_has_signal_count() {
        let pid = "test_prev_digest_signal";
        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);

        let stats = SessionStats {
            nudge_count: 3,
            decide_count: 1,
            signal_count: 5,
            duration_minutes: 15,
            outcome: SessionOutcome::Completed,
            ..Default::default()
        };
        write_prev_digest(pid, "test-signal", &stats, vec![], vec![]);

        let digest = read_prev_digest(pid).expect("should read prev_digest");
        assert_eq!(digest.signal_count, 5);

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }
}
