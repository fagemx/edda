use std::fs;
use std::path::{Path, PathBuf};

use crate::parse::*;
use crate::render;
use crate::signals::*;
use crate::state;

// ── Hook Result ──

/// Result from a hook dispatch.
///
/// - `stdout`: JSON string to print to stdout (consumed by Claude Code)
/// - `stderr`: warning message to print to stderr (shown to user, exit 1)
#[derive(Debug, Default, Clone)]
pub struct HookResult {
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

impl HookResult {
    /// Construct a result with stdout only (normal output, exit 0).
    pub fn output(stdout: String) -> Self {
        Self {
            stdout: Some(stdout),
            stderr: None,
        }
    }

    /// Construct a result with stderr warning (exit 1).
    pub fn warning(msg: String) -> Self {
        Self {
            stdout: None,
            stderr: Some(msg),
        }
    }

    /// Construct an empty result (no output, exit 0).
    pub fn empty() -> Self {
        Self::default()
    }
}

impl From<Option<String>> for HookResult {
    fn from(stdout: Option<String>) -> Self {
        Self {
            stdout,
            stderr: None,
        }
    }
}

// ── Context Boundary (delegates to render module) ──

#[cfg(test)]
const EDDA_BOUNDARY_START: &str = render::BOUNDARY_START;
#[cfg(test)]
const EDDA_BOUNDARY_END: &str = render::BOUNDARY_END;

fn wrap_context_boundary(content: &str) -> String {
    render::wrap_boundary(content)
}

fn context_budget(cwd: &str) -> usize {
    render::context_budget(cwd)
}

fn apply_context_budget(content: &str, budget: usize) -> String {
    render::apply_budget(content, budget)
}

// ── L1 Protocol Rendering ──

/// Render write-back protocol — always fires when hook is active.
/// Teaches the agent to record decisions via `edda decide`.
/// No `.edda/` gate: if the bridge hook is running, the user has edda installed,
/// so the agent should always learn about `edda decide`.
pub(crate) fn render_write_back_protocol(_cwd: &str) -> Option<String> {
    Some(render::writeback())
}

// ── L2 Solo Gate ──

/// Check if any non-stale peer session heartbeats exist (excluding current session).
/// Used to skip all L2 I/O when running solo.
fn has_active_peers(project_id: &str, session_id: &str) -> bool {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let stale_threshold: u64 = std::env::var("EDDA_PEER_STALE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let entries = match fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }
        let sid = name
            .strip_prefix("session.")
            .and_then(|s| s.strip_suffix(".json"))
            .unwrap_or("");
        if sid.is_empty() || sid == session_id {
            continue;
        }
        // Check file modification time as a lightweight staleness check
        // (avoids parsing JSON — just stat the file)
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                let age = now.duration_since(modified).unwrap_or_default();
                if age.as_secs() <= stale_threshold {
                    return true;
                }
            }
        }
    }
    false
}

// ── Hook dispatch ──

/// Main hook entrypoint: parse stdin, dispatch by hook_event_name.
/// Returns `HookResult` with optional stdout JSON and/or stderr warnings.
pub fn hook_entrypoint_from_stdin(stdin: &str) -> anyhow::Result<HookResult> {
    if stdin.trim().is_empty() {
        return Ok(HookResult::empty());
    }
    let raw = parse_hook_stdin(stdin)?;

    let hook_event_name = get_str(&raw, "hook_event_name");
    let session_id = get_str(&raw, "session_id");
    let transcript_path = get_str(&raw, "transcript_path");
    let cwd = get_str(&raw, "cwd");
    let permission_mode = get_str(&raw, "permission_mode");
    let tool_name = get_str(&raw, "tool_name");
    let tool_use_id = get_str(&raw, "tool_use_id");

    let project_id = resolve_project_id(&cwd);

    // Ensure project dirs exist
    let _ = edda_store::ensure_dirs(&project_id);

    // Redact secrets from raw payload before storing in append-only ledger
    let sanitized_raw = crate::redact::redact_hook_payload(&raw);

    let envelope = EventEnvelope {
        ts: now_rfc3339(),
        project_id: project_id.clone(),
        session_id: session_id.clone(),
        hook_event_name: hook_event_name.clone(),
        transcript_path: transcript_path.clone(),
        cwd: cwd.clone(),
        permission_mode,
        tool_name,
        tool_use_id,
        raw: sanitized_raw,
    };

    // Append to session ledger
    let _ = append_to_session_ledger(&envelope);

    // Solo gate: only used to skip coordination log writes (write_unclaim).
    // Heartbeat writes remain unconditional — they ARE the peer discovery mechanism.
    let peers_active = !session_id.is_empty() && has_active_peers(&project_id, &session_id);

    // Update peer heartbeat timestamp (lightweight touch for liveness)
    if !session_id.is_empty() {
        crate::peers::touch_heartbeat(&project_id, &session_id);
    }

    // Dispatch — injection strategy:
    //   SessionStart     → ingest + full pack (turns + workspace) — cold start needs full context
    //   UserPromptSubmit → workspace-only (~2K) — re-ingest only on post-compact (no output)
    //   PreCompact       → ingest + rebuild pack (side-effect only, NO output — Claude Code
    //                      schema does not allow hookSpecificOutput for PreCompact events;
    //                      the rebuilt pack is consumed by the subsequent SessionStart:compact)
    match hook_event_name.as_str() {
        "SessionStart" => {
            // Auto-digest previous sessions FIRST so workspace section reflects latest digests
            let digest_warning = run_auto_digest(&project_id, &session_id, &cwd);
            ingest_and_build_pack(&project_id, &session_id, &transcript_path, &cwd);
            // Ensure heartbeat exists for peer discovery. ingest_and_build_pack
            // writes heartbeat as a side-effect, but skips when the transcript
            // file doesn't exist yet — the normal case for brand-new sessions
            // where Claude Code creates the file AFTER SessionStart fires.
            crate::peers::ensure_heartbeat_exists(&project_id, &session_id);
            dispatch_session_start(&project_id, &session_id, &cwd, digest_warning.as_deref())
        }
        "UserPromptSubmit" => {
            dispatch_user_prompt_submit(&project_id, &session_id, &transcript_path, &cwd)
        }
        "PreToolUse" => dispatch_pre_tool_use(&raw, &cwd),
        "PostToolUse" => dispatch_post_tool_use(&raw, &project_id, &session_id),
        "PostToolUseFailure" => Ok(HookResult::empty()),
        "PreCompact" => {
            // PreCompact hooks cannot inject context via hookSpecificOutput —
            // Claude Code's schema only allows: SessionStart, UserPromptSubmit,
            // PreToolUse, PostToolUse.  But the side-effect matters: rebuild the
            // pack so the *subsequent* SessionStart:compact can inject it.
            // Also set compact_pending flag so the next UserPromptSubmit
            // re-ingests (keeping state fresh) instead of lightweight workspace-only.
            ingest_and_build_pack(&project_id, &session_id, &transcript_path, &cwd);
            set_compact_pending(&project_id);
            Ok(HookResult::empty())
        }
        "SessionEnd" => dispatch_session_end(
            &project_id,
            &session_id,
            &transcript_path,
            &cwd,
            peers_active,
        ),
        _ => Ok(HookResult::empty()),
    }
}

/// Ingest transcript + build pack. Errors are silently ignored to not break the host agent.
fn ingest_and_build_pack(project_id: &str, session_id: &str, transcript_path: &str, cwd: &str) {
    if transcript_path.is_empty() || session_id.is_empty() {
        return;
    }
    let transcript = std::path::Path::new(transcript_path);
    if !transcript.exists() {
        return;
    }

    let project_dir = edda_store::project_dir(project_id);
    let _ = edda_store::ensure_dirs(project_id);

    // Ingest transcript delta with index generation
    let index_path = project_dir
        .join("index")
        .join(format!("{session_id}.jsonl"));
    let sid = session_id.to_string();
    let idx_path = index_path.clone();
    let index_writer = move |_raw: &str,
                             offset: u64,
                             len: u64,
                             parsed: &serde_json::Value|
          -> anyhow::Result<()> {
        let record = edda_index::build_index_record(&sid, offset, len, parsed);
        edda_index::append_index(&idx_path, &record)
    };

    let _ = edda_transcript::ingest_transcript_delta(
        &project_dir,
        session_id,
        transcript,
        Some(&index_writer),
    );

    // Build turns and render pack
    let max_turns: usize = std::env::var("EDDA_PACK_TURNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    let budget: usize = std::env::var("EDDA_PACK_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6000);

    if let Ok(turns) = edda_pack::build_turns(&project_dir, session_id, max_turns) {
        // Compute workspace section from .edda/ ledger
        let workspace_budget: usize = std::env::var("EDDA_WORKSPACE_BUDGET_CHARS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2500);
        let workspace_section = render_workspace_section(cwd, workspace_budget);
        let ws_len = workspace_section.as_ref().map(|s| s.len()).unwrap_or(0);
        let turns_budget = budget.saturating_sub(ws_len);

        let git_branch = {
            let cwd_path = std::path::Path::new(cwd);
            edda_ledger::EddaPaths::find_root(cwd_path)
                .and_then(|root| edda_ledger::Ledger::open(&root).ok())
                .and_then(|l| l.head_branch().ok())
                .unwrap_or_default()
        };
        let meta = edda_pack::PackMetadata {
            project_id: project_id.to_string(),
            session_id: session_id.to_string(),
            git_branch,
            turn_count: turns.len(),
            budget_chars: budget,
        };

        let mut pack_md = edda_pack::render_pack(&turns, &meta, turns_budget);

        // Insert workspace section between header and "## Recent Turns"
        if let Some(ws) = workspace_section {
            if let Some(pos) = pack_md.find("## Recent Turns") {
                pack_md.insert_str(pos, &format!("{ws}\n"));
            }
        }

        let _ = edda_pack::write_pack(&project_dir, &pack_md, &meta);
    }

    // Extract session signals (tasks, files, commits) from stored transcript
    let store_path = project_dir
        .join("transcripts")
        .join(format!("{session_id}.jsonl"));
    let signals = extract_session_signals(&store_path);
    save_session_signals(project_id, session_id, &signals);

    // Write full peer heartbeat with signals snapshot (unconditional — peer discovery depends on it)
    crate::peers::write_heartbeat(project_id, session_id, &signals, None);

    // Auto-claim scope from edited files (L1 auto-detection, #24)
    crate::peers::maybe_auto_claim(project_id, session_id, &signals);
}

/// Lightweight injection: workspace context only (~2K chars), no turns.
/// Supports session-scoped dedup: if workspace context is identical to the
/// last injection for this session, skip re-injecting.
fn dispatch_with_workspace_only(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    event_name: &str,
) -> anyhow::Result<HookResult> {
    let workspace_budget: usize = std::env::var("EDDA_WORKSPACE_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2500);
    let mut ws = render_workspace_section(cwd, workspace_budget);

    // Detect solo → multi-session transition for late peer detection (#11).
    // On 0→N transition, inject the full coordination protocol instead of
    // lightweight peer updates so the agent learns L2 commands and sees
    // peer scope claims.
    let peers = crate::peers::discover_active_peers(project_id, session_id);
    let prev_count = read_peer_count(project_id, session_id);
    let first_peers = prev_count == 0 && !peers.is_empty();
    write_peer_count(project_id, session_id, peers.len());

    if first_peers {
        // First time seeing peers — inject full coordination protocol
        if let Some(coord) = crate::peers::render_coordination_protocol(project_id, session_id, cwd)
        {
            ws = Some(match ws {
                Some(w) => format!("{w}\n\n{coord}"),
                None => coord,
            });
        }
    } else {
        // Normal: lightweight peer updates
        if let Some(updates) = crate::peers::render_peer_updates(project_id, session_id) {
            ws = Some(match ws {
                Some(w) => format!("{w}\n{updates}"),
                None => updates,
            });
        }
    }

    if let Some(ws) = ws {
        let wrapped = wrap_context_boundary(&ws);
        // Dedup: skip if identical to last injection
        if !session_id.is_empty() && is_same_as_last_inject(project_id, session_id, &wrapped) {
            return Ok(HookResult::empty());
        }
        if !session_id.is_empty() {
            write_inject_hash(project_id, session_id, &wrapped);
        }
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": event_name,
                "additionalContext": wrapped
            }
        });
        Ok(HookResult::output(serde_json::to_string(&output)?))
    } else {
        Ok(HookResult::empty())
    }
}

// ── Active Plan ──

/// Default maximum chars for the plan excerpt.
const PLAN_EXCERPT_MAX_CHARS: usize = 700;
/// Default maximum lines to read from the plan file.
const PLAN_EXCERPT_MAX_LINES: usize = 30;

/// Render an "Active Plan" section from the user's Claude plans directory.
/// Uses `EDDA_PLANS_DIR` env var if set, otherwise `~/.claude/plans/`.
/// Returns `None` if no plan file exists.
///
/// When `project_id` is provided, attempts structured rendering with progress
/// tracking (cross-referencing plan steps against tasks/commits). Falls back
/// to simple truncation if the plan has no recognizable step structure.
pub(crate) fn render_active_plan(project_id: Option<&str>) -> Option<String> {
    let plans_dir = match std::env::var("EDDA_PLANS_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => dirs::home_dir()?.join(".claude").join("plans"),
    };
    render_active_plan_from_dir(&plans_dir, project_id)
}

/// Render an "Active Plan" section from a given directory.
/// Returns `None` if no plan file exists.
fn render_active_plan_from_dir(plans_dir: &Path, project_id: Option<&str>) -> Option<String> {
    if !plans_dir.is_dir() {
        return None;
    }

    // Find most recently modified .md file
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries = fs::read_dir(plans_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                    best = Some((mtime, path));
                }
            }
        }
    }

    let (mtime, path) = best?;
    let content = fs::read_to_string(&path).ok()?;
    if content.trim().is_empty() {
        return None;
    }

    // Format mtime as UTC (local offset unavailable in sandboxed time crate)
    let mtime_str = {
        let duration = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let dt = time::OffsetDateTime::from_unix_timestamp(duration.as_secs() as i64)
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            dt.year(),
            dt.month() as u8,
            dt.day(),
            dt.hour(),
            dt.minute()
        )
    };

    let filename = path.file_name()?.to_str()?;

    // Try structured rendering with progress tracking
    if let Some(pid) = project_id {
        if let Some(structured) =
            crate::plan::render_plan_with_progress(&content, pid, filename, &mtime_str)
        {
            return Some(structured);
        }
    }

    // Fallback: excerpt (first N lines, up to MAX_CHARS)
    let mut excerpt = String::new();
    let mut line_count = 0;
    for line in content.lines() {
        if line_count >= PLAN_EXCERPT_MAX_LINES {
            break;
        }
        if excerpt.len() + line.len() + 1 > PLAN_EXCERPT_MAX_CHARS {
            break;
        }
        excerpt.push_str(line);
        excerpt.push('\n');
        line_count += 1;
    }

    if line_count < content.lines().count() {
        excerpt.push_str("...(truncated)\n");
    }

    Some(format!(
        "## Active Plan\n> {filename} ({mtime_str})\n\n{excerpt}"
    ))
}

// ── State Management (delegates to state module) ──

fn set_compact_pending(project_id: &str) {
    state::set_compact_pending(project_id);
}

fn take_compact_pending(project_id: &str) -> bool {
    state::take_compact_pending(project_id)
}

fn should_nudge(project_id: &str, session_id: &str) -> bool {
    state::should_nudge(project_id, session_id)
}

fn mark_nudge_sent(project_id: &str, session_id: &str) {
    state::mark_nudge_sent(project_id, session_id);
}

fn increment_counter(project_id: &str, session_id: &str, name: &str) {
    state::increment_counter(project_id, session_id, name);
}

fn read_counter(project_id: &str, session_id: &str, name: &str) -> u64 {
    state::read_counter(project_id, session_id, name)
}

fn is_same_as_last_inject(project_id: &str, session_id: &str, content: &str) -> bool {
    state::is_same_as_last_inject(project_id, session_id, content)
}

fn write_inject_hash(project_id: &str, session_id: &str, content: &str) {
    state::write_inject_hash(project_id, session_id, content);
}

fn read_peer_count(project_id: &str, session_id: &str) -> usize {
    state::read_peer_count(project_id, session_id)
}

fn write_peer_count(project_id: &str, session_id: &str, count: usize) {
    state::write_peer_count(project_id, session_id, count);
}

/// Dispatch UserPromptSubmit — compact-aware.
///
/// Normal case: inject lightweight workspace context (~2K).
/// Post-compact: inject full hot pack + workspace context to compensate for
/// context loss during compaction.
fn dispatch_user_prompt_submit(
    project_id: &str,
    session_id: &str,
    transcript_path: &str,
    cwd: &str,
) -> anyhow::Result<HookResult> {
    let post_compact = take_compact_pending(project_id);

    if post_compact {
        // Re-ingest so state files are fresh for future hooks.
        // The actual context injection already happened in the preceding
        // SessionStart:compact hook (hot pack + L1 narrative), so we
        // skip duplicate injection here.
        ingest_and_build_pack(project_id, session_id, transcript_path, cwd);
        Ok(HookResult::empty())
    } else {
        // Normal: lightweight workspace-only injection (with dedup).
        dispatch_with_workspace_only(project_id, session_id, cwd, "UserPromptSubmit")
    }
}

// ── SessionEnd ──

/// Dispatch SessionEnd — auto-digest, cleanup state, warn about pending tasks.
fn dispatch_session_end(
    project_id: &str,
    session_id: &str,
    transcript_path: &str,
    cwd: &str,
    peers_active: bool,
) -> anyhow::Result<HookResult> {
    // 1. Final ingest so signals are up-to-date
    ingest_and_build_pack(project_id, session_id, transcript_path, cwd);

    // 2. Auto-digest this session eagerly (don't wait for next SessionStart)
    let _ = run_auto_digest(project_id, session_id, cwd);

    // 2b. Read recall rate counters before cleanup
    let nudge_count = read_counter(project_id, session_id, "nudge_count");
    let decide_count = read_counter(project_id, session_id, "decide_count");
    let signal_count = read_counter(project_id, session_id, "signal_count");

    // 2c. Snapshot session digest for next session's "## Previous Session"
    crate::digest::write_prev_digest_from_store(
        project_id,
        session_id,
        cwd,
        nudge_count,
        decide_count,
        signal_count,
    );

    // 3. Clean up session-scoped state files
    cleanup_session_state(project_id, session_id, peers_active);

    // 4. Collect warnings (pending tasks)
    if let Some(warning) = collect_session_end_warnings(project_id) {
        Ok(HookResult::warning(warning))
    } else {
        Ok(HookResult::empty())
    }
}

/// Remove session-scoped state files that are no longer needed.
fn cleanup_session_state(project_id: &str, session_id: &str, peers_active: bool) {
    let state_dir = edda_store::project_dir(project_id).join("state");
    // Dedup hash (Step 2)
    let _ = fs::remove_file(state_dir.join(format!("inject_hash.{session_id}")));
    // Compact pending flag
    let _ = fs::remove_file(state_dir.join("compact_pending"));
    // Note: prev_digest.json is NOT deleted here — it persists for the next
    // session's SessionStart injection. It gets overwritten when this next
    // session ends and writes its own prev_digest.json.
    // Nudge state
    let _ = fs::remove_file(state_dir.join(format!("nudge_ts.{session_id}")));
    // Recall rate counters
    let _ = fs::remove_file(state_dir.join(format!("nudge_count.{session_id}")));
    let _ = fs::remove_file(state_dir.join(format!("decide_count.{session_id}")));
    let _ = fs::remove_file(state_dir.join(format!("signal_count.{session_id}")));
    // Late peer detection counter (#11)
    let _ = fs::remove_file(state_dir.join(format!("peer_count.{session_id}")));
    // Auto-claim state file (#24)
    crate::peers::remove_autoclaim_state(project_id, session_id);
    // Peer heartbeat + unclaim (L2 — keep remove_heartbeat unconditional as idempotent cleanup)
    crate::peers::remove_heartbeat(project_id, session_id);
    if peers_active {
        crate::peers::write_unclaim(project_id, session_id);
    }
    // NOTE: The following state files are intentionally NOT deleted here — they
    // persist across sessions to provide continuity context at the next SessionStart:
    //   - active_tasks.json  → L1 narrative shows previous session's final task state
    //   - files_modified.json → activity summary for carry-over context
    //   - recent_commits.json → commit history carry-over
    //   - failed_commands.json → recurring failure detection
    // These files are overwritten (not appended) by save_session_signals() when
    // the next session produces new signal data, so stale data self-heals.
}

/// Check for pending tasks and produce a warning message if any remain.
fn collect_session_end_warnings(project_id: &str) -> Option<String> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let tasks_path = state_dir.join("active_tasks.json");
    let content = fs::read_to_string(&tasks_path).ok()?;
    let tasks: Vec<TaskSnapshot> = serde_json::from_str(&content).ok()?;
    let pending: Vec<&TaskSnapshot> = tasks.iter().filter(|t| t.status != "completed").collect();
    if pending.is_empty() {
        return None;
    }
    let list: String = pending
        .iter()
        .take(5)
        .map(|t| format!("  - [ ] {}", t.subject))
        .collect::<Vec<_>>()
        .join("\n");
    let suffix = if pending.len() > 5 {
        format!("\n  ... and {} more", pending.len() - 5)
    } else {
        String::new()
    };
    Some(format!(
        "edda: {} task(s) still pending\n{list}{suffix}",
        pending.len()
    ))
}

// ── Skill Catalog ──

/// Render a skill guide directive for guide mode.
/// Does NOT duplicate the skill list (Claude Code system-reminder already provides it).
/// Only injects behavioral instruction to proactively recommend skills.
fn render_skill_guide_directive() -> String {
    [
        "## Skill Guide Mode",
        "",
        "The available skills/commands are listed in the system-reminder above.",
        "When the user's current task or question matches a skill, **proactively suggest it**:",
        "- Name the skill with `/<name>` so the user can invoke it directly",
        "- Briefly explain what it does and why it fits their situation",
        "- If a workflow applies (e.g. `/deep-research` → `/deep-innovate` → `/deep-plan`), mention the sequence",
        "",
        "Goal: help users discover and learn available tools over time.",
    ]
    .join("\n")
}

/// Run auto-digest: digest pending previous sessions into workspace ledger.
/// Returns an optional warning string to inject into context.
fn run_auto_digest(project_id: &str, current_session_id: &str, cwd: &str) -> Option<String> {
    // Check if auto_digest is enabled (default: true)
    let enabled = match std::env::var("EDDA_BRIDGE_AUTO_DIGEST") {
        Ok(val) => val != "0",
        Err(_) => read_workspace_config_bool(cwd, "bridge.auto_digest").unwrap_or(true),
    };
    if !enabled {
        return None;
    }

    let lock_timeout_ms: u64 = std::env::var("EDDA_BRIDGE_LOCK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);

    let digest_failed_cmds = match std::env::var("EDDA_BRIDGE_DIGEST_FAILED_CMDS") {
        Ok(val) => val != "0",
        Err(_) => read_workspace_config_bool(cwd, "bridge.digest_failed_cmds").unwrap_or(true),
    };

    match crate::digest::digest_previous_sessions_with_opts(
        project_id,
        current_session_id,
        cwd,
        lock_timeout_ms,
        digest_failed_cmds,
    ) {
        crate::digest::DigestResult::Written { event_id } => {
            eprintln!("[edda] digested previous session → {event_id}");
            None
        }
        crate::digest::DigestResult::PermanentFailure(warning) => Some(warning),
        crate::digest::DigestResult::NoPending
        | crate::digest::DigestResult::Disabled
        | crate::digest::DigestResult::LockTimeout
        | crate::digest::DigestResult::Error(_) => None,
    }
}

// ── Last Assistant Message ──

/// Default max chars for the last assistant message excerpt.
const LAST_ASSISTANT_MAX_CHARS: usize = 500;

/// Extract the last assistant message from the most recent prior session's transcript.
/// Returns None if no prior session exists or no assistant text found.
fn extract_prior_session_last_message(
    project_id: &str,
    current_session_id: &str,
) -> Option<String> {
    let transcripts_dir = edda_store::project_dir(project_id).join("transcripts");
    if !transcripts_dir.is_dir() {
        return None;
    }

    // Find the most recently modified transcript that isn't the current session
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries = fs::read_dir(&transcripts_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let stem = path.file_stem()?.to_str()?;
        if stem == current_session_id {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                    best = Some((mtime, path));
                }
            }
        }
    }

    let (_, transcript_path) = best?;
    let max_chars: usize = std::env::var("EDDA_LAST_ASSISTANT_MAX_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(LAST_ASSISTANT_MAX_CHARS);
    edda_transcript::extract_last_assistant_text(&transcript_path, max_chars)
}

/// Dispatch SessionStart with pack + skills + optional digest warning.
fn dispatch_session_start(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    digest_warning: Option<&str>,
) -> anyhow::Result<HookResult> {
    // Conductor mode: skip sections that overlap with conductor's --append-system-prompt.
    // See CONDUCTOR-SPEC.md §10.2.
    let conductor_mode = std::env::var("EDDA_CONDUCTOR_MODE").is_ok();

    let pack = read_hot_pack(project_id);
    let guide_mode = match std::env::var("EDDA_SKILL_GUIDE") {
        Ok(val) => val == "1",
        Err(_) => read_workspace_config_bool(cwd, "skill_guide").unwrap_or(false),
    };
    let mut content = if guide_mode {
        let directive = render_skill_guide_directive();
        match pack {
            Some(p) => Some(format!("{p}\n{directive}")),
            None => Some(directive),
        }
    } else {
        pack
    };

    // Append active plan file excerpt for cross-session continuity.
    // Conductor mode: skip — conductor provides plan context via --append-system-prompt.
    if !conductor_mode {
        if let Some(plan) = render_active_plan(Some(project_id)) {
            content = Some(match content {
                Some(c) => format!("{c}\n\n{plan}"),
                None => plan,
            });
        }
    }

    // Append L1 narrative (composed signals: focus + blocking + tasks + activity).
    // Conductor mode: minimal — only activity summary (1-2 lines).
    if conductor_mode {
        if let Some(activity) = crate::narrative::compose_narrative_minimal(project_id) {
            content = Some(match content {
                Some(c) => format!("{c}\n\n{activity}"),
                None => activity,
            });
        }
    } else if let Some(narrative) = crate::narrative::compose_narrative(project_id) {
        content = Some(match content {
            Some(c) => format!("{c}\n\n{narrative}"),
            None => narrative,
        });
    }

    // Previous session context is now rendered within the workspace section's
    // "## Session History" (tiered rendering). No separate injection needed.

    // Build tail (reserved sections — always included, never truncated).
    // These are critical for agent functionality and must survive budget cuts.
    let mut tail = String::new();

    // Write-back protocol (L1 — always, solo or multi-session).
    if let Some(wb) = render_write_back_protocol(cwd) {
        tail.push_str(&format!("\n\n{wb}"));
    }

    // Coordination protocol for multi-session awareness.
    if let Some(coord) = crate::peers::render_coordination_protocol(project_id, session_id, cwd) {
        tail.push_str(&format!("\n\n{coord}"));
    }

    // Seed peer count so UserPromptSubmit knows the baseline and doesn't
    // re-inject the full protocol on the first prompt after SessionStart (#11).
    let peers = crate::peers::discover_active_peers(project_id, session_id);
    write_peer_count(project_id, session_id, peers.len());

    // Remaining body sections (truncatable — nice-to-have context).

    // Append last assistant message from prior session for continuity.
    // Conductor mode: skip — phases are independent.
    if !conductor_mode {
        if let Some(msg) = extract_prior_session_last_message(project_id, session_id) {
            let section = format!("## Previously (last response)\n> {msg}\n");
            content = Some(match content {
                Some(c) => format!("{c}\n\n{section}"),
                None => section,
            });
        }
    }

    // Append digest warning if present
    if let Some(warning) = digest_warning {
        content = Some(match content {
            Some(c) => format!("{c}\n\n{warning}"),
            None => warning.to_string(),
        });
    }

    // Apply budget: body gets (total - tail.len()), tail appended unconditionally.
    let total_budget = context_budget(cwd);
    let body_budget = total_budget.saturating_sub(tail.len());

    if let Some(ctx) = content {
        let budgeted_body = apply_context_budget(&ctx, body_budget);
        let final_content = if tail.is_empty() {
            budgeted_body
        } else {
            format!("{budgeted_body}{tail}")
        };
        let wrapped = wrap_context_boundary(&final_content);
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": wrapped
            }
        });
        Ok(HookResult::output(serde_json::to_string(&output)?))
    } else if !tail.is_empty() {
        let trimmed = tail.trim_start().to_string();
        let wrapped = wrap_context_boundary(&trimmed);
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": wrapped
            }
        });
        Ok(HookResult::output(serde_json::to_string(&output)?))
    } else {
        Ok(HookResult::empty())
    }
}

// ── Config Helpers (delegates to render module) ──

fn read_workspace_config_bool(cwd: &str, key: &str) -> Option<bool> {
    render::config_bool(cwd, key)
}

#[allow(dead_code)]
fn read_workspace_config_usize(cwd: &str, key: &str) -> Option<usize> {
    render::config_usize(cwd, key)
}

fn dispatch_pre_tool_use(raw: &serde_json::Value, cwd: &str) -> anyhow::Result<HookResult> {
    let auto_approve = std::env::var("EDDA_CLAUDE_AUTO_APPROVE").unwrap_or_else(|_| "1".into());

    // Pattern matching (only for Edit/Write)
    let pattern_ctx = match_tool_patterns(raw, cwd);

    if auto_approve == "1" {
        let mut hook_output = serde_json::json!({
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "edda auto-approved (M8)"
        });
        if let Some(ctx) = pattern_ctx {
            hook_output["additionalContext"] =
                serde_json::Value::String(wrap_context_boundary(&ctx));
        }
        let output = serde_json::json!({ "hookSpecificOutput": hook_output });
        Ok(HookResult::output(serde_json::to_string(&output)?))
    } else if let Some(ctx) = pattern_ctx {
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": wrap_context_boundary(&ctx)
            }
        });
        Ok(HookResult::output(serde_json::to_string(&output)?))
    } else {
        Ok(HookResult::empty())
    }
}

/// Dispatch PostToolUse — detect decision signals and nudge.
fn dispatch_post_tool_use(
    raw: &serde_json::Value,
    project_id: &str,
    session_id: &str,
) -> anyhow::Result<HookResult> {
    let signal = match crate::nudge::detect_signal(raw) {
        Some(s) => s,
        None => return Ok(HookResult::empty()),
    };

    // Count every detected signal (including SelfRecord and cooldown-suppressed ones).
    increment_counter(project_id, session_id, "signal_count");

    // Agent is recording a decision → increment counter, but do NOT suppress future nudges.
    // This allows the agent to receive nudges for subsequent signals after cooldown.
    if signal == crate::nudge::NudgeSignal::SelfRecord {
        increment_counter(project_id, session_id, "decide_count");
        return Ok(HookResult::empty());
    }

    // Check cooldown
    if !should_nudge(project_id, session_id) {
        return Ok(HookResult::empty());
    }

    let decide_count = read_counter(project_id, session_id, "decide_count");
    let nudge_text = crate::nudge::format_nudge(&signal, decide_count);
    mark_nudge_sent(project_id, session_id);
    increment_counter(project_id, session_id, "nudge_count");

    let wrapped = wrap_context_boundary(&nudge_text);
    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": wrapped
        }
    });
    Ok(HookResult::output(serde_json::to_string(&output)?))
}

/// Check if patterns are enabled and match tool input against Pattern Store.
fn match_tool_patterns(raw: &serde_json::Value, cwd: &str) -> Option<String> {
    // Check if patterns feature is enabled
    let enabled = match std::env::var("EDDA_PATTERNS_ENABLED") {
        Ok(val) => val == "1",
        Err(_) => read_workspace_config_bool(cwd, "patterns_enabled").unwrap_or(false),
    };
    if !enabled {
        return None;
    }

    // Only match on Edit and Write tools
    let tool_name = raw.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    if tool_name != "Edit" && tool_name != "Write" {
        return None;
    }

    // Extract file_path from tool_input
    let file_path = raw
        .get("tool_input")
        .and_then(|ti| ti.get("file_path"))
        .and_then(|fp| fp.as_str())
        .unwrap_or("");
    if file_path.is_empty() {
        return None;
    }

    // Find patterns dir
    let root = edda_ledger::EddaPaths::find_root(Path::new(cwd))?;
    let patterns_dir = root.join(".edda").join("patterns");

    // Load and match
    let patterns = crate::pattern::load_patterns(&patterns_dir);
    if patterns.is_empty() {
        return None;
    }

    let matched = crate::pattern::match_patterns(&patterns, file_path);
    if matched.is_empty() {
        return None;
    }

    let budget: usize = std::env::var("EDDA_PATTERN_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);

    crate::pattern::render_pattern_context(&matched, file_path, budget)
}

pub(crate) fn read_hot_pack(project_id: &str) -> Option<String> {
    let pack_path = edda_store::project_dir(project_id)
        .join("packs")
        .join("hot.md");
    fs::read_to_string(&pack_path).ok()
}

// ── Workspace Context (delegate to render module) ──

pub(crate) fn render_workspace_section(cwd: &str, workspace_budget: usize) -> Option<String> {
    render::workspace(cwd, workspace_budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_entrypoint_session_start() {
        // Disable auto-digest to avoid interacting with real project state
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        // Disable plan injection to avoid picking up real plan files
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent/plans/dir");
        let stdin = r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":".","transcript_path":"","permission_mode":"default"}"#;
        let result = hook_entrypoint_from_stdin(stdin).unwrap();
        // Write-back protocol always fires, so there should be output
        assert!(
            result.stdout.is_some(),
            "write-back protocol should always inject"
        );
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            ctx.contains("Write-Back Protocol"),
            "should contain write-back protocol"
        );
        assert!(result.stderr.is_none());
        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
        std::env::remove_var("EDDA_PLANS_DIR");
    }

    #[test]
    fn hook_entrypoint_camel_case_input() {
        // Claude Code sends camelCase JSON
        std::env::set_var("EDDA_CLAUDE_AUTO_APPROVE", "1");
        let stdin = r#"{"sessionId":"s-camel","hookEventName":"PreToolUse","cwd":".","toolName":"Bash","toolUseId":"tu1"}"#;
        let result = hook_entrypoint_from_stdin(stdin).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "allow");
    }

    #[test]
    fn hook_entrypoint_pre_tool_use_auto_approve() {
        std::env::set_var("EDDA_CLAUDE_AUTO_APPROVE", "1");
        let stdin = r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":".","tool_name":"Bash","tool_use_id":"tu1"}"#;
        let result = hook_entrypoint_from_stdin(stdin).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "allow");
    }

    #[test]
    fn hook_entrypoint_post_tool_use_no_output() {
        let stdin = r#"{"session_id":"s1","hook_event_name":"PostToolUse","cwd":".","tool_name":"Bash","tool_use_id":"tu1"}"#;
        let result = hook_entrypoint_from_stdin(stdin).unwrap();
        assert!(result.stdout.is_none());
        assert!(result.stderr.is_none());
    }

    // transform_context_strips_header_and_cite → moved to render::tests

    #[test]
    fn render_workspace_section_no_edda_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result = render_workspace_section(tmp.path().to_str().unwrap(), 2000);
        assert!(result.is_none());
    }

    #[test]
    fn pre_tool_use_with_patterns() {
        // Setup: create temp dir with .edda/patterns/
        let tmp = tempfile::tempdir().unwrap();
        let edda_dir = tmp.path().join(".edda");
        let patterns_dir = edda_dir.join("patterns");
        std::fs::create_dir_all(&patterns_dir).unwrap();

        // Write a test pattern
        let pat = serde_json::json!({
            "id": "test-no-db",
            "trigger": { "file_glob": ["**/*.test.*"], "keywords": [] },
            "rule": "Tests should use API, not direct DB",
            "source": "PR #2587",
            "metadata": { "status": "active", "hit_count": 0 }
        });
        std::fs::write(
            patterns_dir.join("test-no-db.json"),
            serde_json::to_string_pretty(&pat).unwrap(),
        )
        .unwrap();

        // Enable patterns
        std::env::set_var("EDDA_PATTERNS_ENABLED", "1");
        std::env::set_var("EDDA_CLAUDE_AUTO_APPROVE", "1");

        let stdin = serde_json::json!({
            "session_id": "s1",
            "hook_event_name": "PreToolUse",
            "cwd": tmp.path().to_str().unwrap(),
            "tool_name": "Edit",
            "tool_use_id": "tu1",
            "tool_input": {
                "file_path": "src/foo.test.ts",
                "old_string": "old",
                "new_string": "new"
            }
        });

        let result = hook_entrypoint_from_stdin(&serde_json::to_string(&stdin).unwrap()).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "allow");
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.contains("test-no-db"));
        assert!(ctx.contains("API"));

        // Cleanup
        std::env::remove_var("EDDA_PATTERNS_ENABLED");
    }

    #[test]
    fn compact_pending_flag_lifecycle() {
        // Use a unique fake project id to avoid collisions with real state
        let pid = "test_compact_pending_00";
        let _ = edda_store::ensure_dirs(pid);

        // Initially no flag
        assert!(!take_compact_pending(pid));

        // Set flag
        set_compact_pending(pid);
        let cp_path = edda_store::project_dir(pid).join("state").join("compact_pending");
        assert!(cp_path.exists());

        // Take clears it and returns true once
        assert!(take_compact_pending(pid));
        assert!(!cp_path.exists());

        // Second take returns false
        assert!(!take_compact_pending(pid));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Active Plan tests ──

    #[test]
    fn active_plan_missing_dir_returns_none() {
        let result = render_active_plan_from_dir(Path::new("/nonexistent/plans/dir"), None);
        assert!(result.is_none());
    }

    #[test]
    fn active_plan_selects_latest_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let plans = tmp.path().join("plans");
        fs::create_dir_all(&plans).unwrap();

        let old_plan = plans.join("old-plan.md");
        fs::write(&old_plan, "# Old Plan\nThis is old").unwrap();

        // Small sleep to ensure different mtime
        std::thread::sleep(std::time::Duration::from_millis(50));

        let new_plan = plans.join("new-plan.md");
        fs::write(&new_plan, "# New Plan\nThis is new").unwrap();

        let section = render_active_plan_from_dir(&plans, None).unwrap();
        assert!(section.contains("new-plan.md"), "Should select newest plan");
        assert!(section.contains("# New Plan"));
        assert!(!section.contains("# Old Plan"));
    }

    #[test]
    fn active_plan_truncates_to_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let plans = tmp.path().join("plans");
        fs::create_dir_all(&plans).unwrap();

        let mut content = String::new();
        for i in 0..100 {
            content.push_str(&format!("## Step {i}: do something important\n"));
        }
        fs::write(plans.join("big-plan.md"), &content).unwrap();

        let section = render_active_plan_from_dir(&plans, None).unwrap();
        assert!(section.contains("...(truncated)"));
        assert!(!section.contains("Step 99"));
        // Excerpt should stay under budget (700 chars) + header overhead
        assert!(section.len() < 1000);
    }

    #[test]
    fn session_start_includes_signals() {
        let pid = "test_session_start_signals";
        let _ = edda_store::ensure_dirs(pid);
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent/plans/dir");

        // Save session signals (tasks, files, commits)
        let signals = SessionSignals {
            tasks: vec![TaskSnapshot {
                id: "1".into(),
                subject: "Fix bug".into(),
                status: "in_progress".into(),
            }],
            files_modified: vec![FileEditCount {
                path: "/repo/crates/foo/src/lib.rs".into(),
                count: 3,
            }],
            commits: vec![CommitInfo {
                hash: "abc1234".into(),
                message: "fix: the bug".into(),
            }],
            failed_commands: vec![],
        };
        save_session_signals(pid, "test-session", &signals);

        // Write a minimal hot pack so dispatch_session_start has something to read
        let pack_dir = edda_store::project_dir(pid).join("packs");
        let _ = fs::create_dir_all(&pack_dir);
        let _ = fs::write(pack_dir.join("hot.md"), "# edda memory pack (hot)\n");

        let result = dispatch_session_start(pid, "test-session", "", None).unwrap();
        assert!(result.stdout.is_some(), "should return output");

        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();

        // L1 narrative sections
        assert!(
            ctx.contains("## Tasks"),
            "should contain Tasks section:\n{ctx}"
        );
        assert!(
            ctx.contains("Fix bug"),
            "should contain task subject:\n{ctx}"
        );
        assert!(
            ctx.contains("Session Activity"),
            "should contain Session Activity section:\n{ctx}"
        );
        assert!(
            ctx.contains("1 files modified"),
            "should contain file count:\n{ctx}"
        );
        assert!(
            ctx.contains("abc1234"),
            "should contain commit hash:\n{ctx}"
        );

        std::env::remove_var("EDDA_PLANS_DIR");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn session_start_no_signals_no_extra_sections() {
        let pid = "test_session_start_no_signals";
        let _ = edda_store::ensure_dirs(pid);
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent/plans/dir");

        // Write a minimal hot pack, no signals
        let pack_dir = edda_store::project_dir(pid).join("packs");
        let _ = fs::create_dir_all(&pack_dir);
        let _ = fs::write(pack_dir.join("hot.md"), "# edda memory pack (hot)\n");

        let result = dispatch_session_start(pid, "test-session", "", None).unwrap();
        assert!(result.stdout.is_some());

        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();

        // No narrative sections should appear when empty
        assert!(
            !ctx.contains("## Tasks"),
            "should not contain Tasks when empty"
        );
        assert!(
            !ctx.contains("Session Activity"),
            "should not contain Session Activity when empty"
        );
        assert!(
            !ctx.contains("Current Focus"),
            "should not contain Focus when empty"
        );

        std::env::remove_var("EDDA_PLANS_DIR");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn active_plan_renders_from_custom_dir() {
        // Test render_active_plan_from_dir directly (avoids env var race in parallel tests)
        let tmp = tempfile::tempdir().unwrap();
        let plans_dir = tmp.path().join("plans");
        fs::create_dir_all(&plans_dir).unwrap();
        fs::write(
            plans_dir.join("test-plan.md"),
            "# My Plan\n\n## Step 1\nDo something\n",
        )
        .unwrap();

        let section = render_active_plan_from_dir(&plans_dir, None).unwrap();
        assert!(section.contains("## Active Plan"));
        assert!(section.contains("test-plan.md"));
        assert!(section.contains("# My Plan"));
        assert!(section.contains("## Step 1"));
    }

    // ── HookResult tests ──

    #[test]
    fn hook_result_output_has_stdout_only() {
        let r = HookResult::output("hello".into());
        assert_eq!(r.stdout.as_deref(), Some("hello"));
        assert!(r.stderr.is_none());
    }

    #[test]
    fn hook_result_warning_has_stderr_only() {
        let r = HookResult::warning("oops".into());
        assert!(r.stdout.is_none());
        assert_eq!(r.stderr.as_deref(), Some("oops"));
    }

    #[test]
    fn hook_result_empty_has_nothing() {
        let r = HookResult::empty();
        assert!(r.stdout.is_none());
        assert!(r.stderr.is_none());
    }

    #[test]
    fn hook_result_from_option_some() {
        let r: HookResult = Some("data".to_string()).into();
        assert_eq!(r.stdout.as_deref(), Some("data"));
        assert!(r.stderr.is_none());
    }

    #[test]
    fn hook_result_from_option_none() {
        let r: HookResult = None.into();
        assert!(r.stdout.is_none());
        assert!(r.stderr.is_none());
    }

    // ── Injection Dedup tests ──

    #[test]
    fn dedup_skips_identical_context() {
        let pid = "test_dedup_skip";
        let sid = "sess-dedup-1";
        let _ = edda_store::ensure_dirs(pid);

        // First write sets the hash
        write_inject_hash(pid, sid, "hello workspace");
        // Same content → should be detected as identical
        assert!(is_same_as_last_inject(pid, sid, "hello workspace"));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn dedup_injects_changed_context() {
        let pid = "test_dedup_changed";
        let sid = "sess-dedup-2";
        let _ = edda_store::ensure_dirs(pid);

        write_inject_hash(pid, sid, "version 1");
        // Different content → should NOT be identical
        assert!(!is_same_as_last_inject(pid, sid, "version 2"));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn dedup_first_call_always_injects() {
        let pid = "test_dedup_first";
        let sid = "sess-dedup-3";
        let _ = edda_store::ensure_dirs(pid);

        // No prior hash → should return false (inject)
        assert!(!is_same_as_last_inject(pid, sid, "anything"));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── SessionEnd tests ──

    #[test]
    fn session_end_cleans_state() {
        let pid = "test_session_end_clean";
        let sid = "sess-end-1";
        let _ = edda_store::ensure_dirs(pid);

        // Create state files that should be cleaned
        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::create_dir_all(&state_dir);
        fs::write(state_dir.join(format!("inject_hash.{sid}")), "abcd").unwrap();
        fs::write(state_dir.join("compact_pending"), "1").unwrap();

        cleanup_session_state(pid, sid, false);

        assert!(
            !state_dir.join(format!("inject_hash.{sid}")).exists(),
            "inject_hash should be cleaned"
        );
        assert!(
            !state_dir.join("compact_pending").exists(),
            "compact_pending should be cleaned"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn session_end_warns_pending_tasks() {
        let pid = "test_session_end_warn";
        let _ = edda_store::ensure_dirs(pid);

        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::create_dir_all(&state_dir);

        // Write active_tasks with some pending
        let tasks = serde_json::json!([
            {"id": "1", "subject": "Fix bug", "status": "in_progress"},
            {"id": "2", "subject": "Add tests", "status": "pending"},
            {"id": "3", "subject": "Done task", "status": "completed"}
        ]);
        fs::write(
            state_dir.join("active_tasks.json"),
            serde_json::to_string(&tasks).unwrap(),
        )
        .unwrap();

        let warning = collect_session_end_warnings(pid);
        assert!(warning.is_some());
        let w = warning.unwrap();
        assert!(w.contains("2 task(s) still pending"));
        assert!(w.contains("Fix bug"));
        assert!(w.contains("Add tests"));
        assert!(!w.contains("Done task"));

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn session_end_no_warning_when_all_completed() {
        let pid = "test_session_end_no_warn";
        let _ = edda_store::ensure_dirs(pid);

        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::create_dir_all(&state_dir);

        let tasks = serde_json::json!([
            {"id": "1", "subject": "Done", "status": "completed"}
        ]);
        fs::write(
            state_dir.join("active_tasks.json"),
            serde_json::to_string(&tasks).unwrap(),
        )
        .unwrap();

        let warning = collect_session_end_warnings(pid);
        assert!(warning.is_none());

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Boundary marker tests ──

    #[test]
    fn wrap_context_boundary_adds_markers() {
        let content = "hello world";
        let wrapped = wrap_context_boundary(content);
        assert!(wrapped.starts_with(EDDA_BOUNDARY_START));
        assert!(wrapped.ends_with(EDDA_BOUNDARY_END));
        assert!(wrapped.contains("hello world"));
    }

    #[test]
    fn session_start_output_has_boundary_markers() {
        let pid = "test_boundary_session_start";
        let _ = edda_store::ensure_dirs(pid);
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent/plans/dir");

        let pack_dir = edda_store::project_dir(pid).join("packs");
        let _ = fs::create_dir_all(&pack_dir);
        let _ = fs::write(pack_dir.join("hot.md"), "# edda memory pack (hot)\n");

        let result = dispatch_session_start(pid, "test-session", "", None).unwrap();
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();

        assert!(
            ctx.contains(EDDA_BOUNDARY_START),
            "SessionStart should have edda:start marker"
        );
        assert!(
            ctx.contains(EDDA_BOUNDARY_END),
            "SessionStart should have edda:end marker"
        );

        std::env::remove_var("EDDA_PLANS_DIR");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Token budget tests ──

    #[test]
    fn apply_context_budget_no_truncation() {
        let content = "short content";
        let result = apply_context_budget(content, 8000);
        assert_eq!(result, content);
    }

    #[test]
    fn apply_context_budget_truncates_long_content() {
        let content = "x".repeat(10000);
        let result = apply_context_budget(&content, 500);
        assert!(result.len() <= 550); // budget + truncation notice
        assert!(result.contains("truncated"));
        assert!(result.contains("500 char budget"));
    }

    #[test]
    fn context_budget_uses_env_var() {
        std::env::set_var("EDDA_MAX_CONTEXT_CHARS", "1234");
        let budget = context_budget("");
        assert_eq!(budget, 1234);
        std::env::remove_var("EDDA_MAX_CONTEXT_CHARS");
    }

    #[test]
    fn context_budget_default_without_config() {
        std::env::remove_var("EDDA_MAX_CONTEXT_CHARS");
        let budget = context_budget("/nonexistent/dir");
        assert_eq!(budget, render::DEFAULT_MAX_CONTEXT_CHARS);
    }

    // ── Body/Tail Budget Split tests ──

    #[test]
    fn tail_sections_survive_budget_truncation() {
        // Simulate: large body (10K) + tail (write-back + coord), budget = 8000
        let body = "x".repeat(10000);
        let tail_wb = "\n\n## Write-Back Protocol\nRecord decisions with: `edda decide`";
        let tail_coord = "\n\n## Coordination Protocol\nYou are one of 3 agents.";
        let tail = format!("{tail_wb}{tail_coord}");

        let total_budget: usize = 8000;
        let body_budget = total_budget.saturating_sub(tail.len());
        let budgeted_body = apply_context_budget(&body, body_budget);
        let final_content = format!("{budgeted_body}{tail}");

        assert!(
            final_content.contains("Write-Back Protocol"),
            "write-back protocol must survive: {}",
            &final_content[final_content.len().saturating_sub(200)..]
        );
        assert!(
            final_content.contains("Coordination Protocol"),
            "coordination protocol must survive: {}",
            &final_content[final_content.len().saturating_sub(200)..]
        );
    }

    #[test]
    fn body_truncated_when_tail_present() {
        let body = "y".repeat(10000);
        let tail = "\n\n## Reserved Section\nThis must appear.";
        let total_budget: usize = 8000;
        let body_budget = total_budget.saturating_sub(tail.len());
        let budgeted_body = apply_context_budget(&body, body_budget);
        let final_content = format!("{budgeted_body}{tail}");

        // Body portion should be truncated
        assert!(
            budgeted_body.contains("truncated"),
            "body should be truncated"
        );
        // Body portion should fit within body_budget (+ truncation notice overhead)
        assert!(
            budgeted_body.len() <= body_budget + 60,
            "body len {} should be near body_budget {}",
            budgeted_body.len(),
            body_budget
        );
        // Tail must be present and complete
        assert!(
            final_content.ends_with("This must appear."),
            "tail must be at the end: {}",
            &final_content[final_content.len().saturating_sub(100)..]
        );
    }

    #[test]
    fn empty_tail_preserves_existing_behavior() {
        let body = "z".repeat(5000);
        let tail = "";
        let total_budget: usize = 8000;
        let body_budget = total_budget.saturating_sub(tail.len());
        let budgeted_body = apply_context_budget(&body, body_budget);

        // With empty tail, body should not be truncated (5000 < 8000)
        assert!(
            !budgeted_body.contains("truncated"),
            "body should NOT be truncated when under budget"
        );
        assert_eq!(budgeted_body.len(), 5000);
    }

    // ── Decision Nudge tests ──

    #[test]
    fn post_tool_use_commit_triggers_nudge() {
        let pid = "test_nudge_commit";
        let sid = "sess-nudge-1";
        let _ = edda_store::ensure_dirs(pid);

        let raw = serde_json::json!({
            "session_id": sid,
            "hook_event_name": "PostToolUse",
            "cwd": ".",
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m \"feat: switch to postgres\"" }
        });
        let result = dispatch_post_tool_use(&raw, pid, sid).unwrap();
        assert!(result.stdout.is_some(), "should produce nudge output");
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            ctx.contains("edda decide"),
            "nudge should mention edda decide"
        );
        assert!(
            ctx.contains("switch to postgres"),
            "nudge should quote commit msg"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn post_tool_use_after_decide_cooldown_still_applies() {
        let pid = "test_nudge_suppressed";
        let sid = "sess-nudge-2";
        let _ = edda_store::ensure_dirs(pid);

        // Agent calls edda decide (SelfRecord) — no longer suppresses all future nudges.
        let decide_raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "edda decide \"db=postgres\"" }
        });
        dispatch_post_tool_use(&decide_raw, pid, sid).unwrap();

        // SelfRecord does NOT set cooldown timestamp, so the first real signal fires.
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m \"feat: add redis cache\"" }
        });
        let result = dispatch_post_tool_use(&raw, pid, sid).unwrap();
        assert!(
            result.stdout.is_some(),
            "should nudge after decide (no global suppression)"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn post_tool_use_cooldown_suppresses() {
        let pid = "test_nudge_cooldown";
        let sid = "sess-nudge-3";
        let _ = edda_store::ensure_dirs(pid);

        // First commit → nudge
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m \"feat: first commit\"" }
        });
        let result = dispatch_post_tool_use(&raw, pid, sid).unwrap();
        assert!(result.stdout.is_some(), "first commit should nudge");

        // Second commit immediately → no nudge (cooldown)
        let raw2 = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m \"feat: second commit\"" }
        });
        let result2 = dispatch_post_tool_use(&raw2, pid, sid).unwrap();
        assert!(
            result2.stdout.is_none(),
            "second commit should be suppressed by cooldown"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn post_tool_use_self_record_increments_decide_count() {
        let pid = "test_nudge_selfrecord";
        let sid = "sess-nudge-4";
        let _ = edda_store::ensure_dirs(pid);

        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "edda decide \"db=postgres\" --reason \"need JSONB\"" }
        });
        let result = dispatch_post_tool_use(&raw, pid, sid).unwrap();
        assert!(
            result.stdout.is_none(),
            "self-record should not produce output"
        );
        assert_eq!(
            read_counter(pid, sid, "decide_count"),
            1,
            "decide_count should be incremented"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn session_end_cleans_nudge_state() {
        let pid = "test_nudge_cleanup";
        let sid = "sess-nudge-5";
        let _ = edda_store::ensure_dirs(pid);

        mark_nudge_sent(pid, sid);

        let state_dir = edda_store::project_dir(pid).join("state");
        assert!(state_dir.join(format!("nudge_ts.{sid}")).exists());

        cleanup_session_state(pid, sid, false);

        assert!(!state_dir.join(format!("nudge_ts.{sid}")).exists());

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn write_back_protocol_always_fires() {
        let dir = tempfile::tempdir().unwrap();
        // No .edda/ → still fires (gate removed)
        let result = render_write_back_protocol(dir.path().to_str().unwrap());
        assert!(result.is_some(), "should fire without .edda/");
        let text = result.unwrap();
        assert!(text.contains("Write-Back Protocol"), "header: {text}");
        assert!(text.contains("edda decide"), "decide: {text}");
        assert!(text.contains("edda note"), "note: {text}");
        assert!(text.contains("--tag session"), "tag: {text}");
    }

    // ── Write-Back Protocol text tests ──

    #[test]
    fn write_back_protocol_contains_examples() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".edda")).unwrap();
        let text = render_write_back_protocol(dir.path().to_str().unwrap()).unwrap();
        assert!(text.contains("db.engine=postgres"), "example 1: {text}");
        assert!(text.contains("auth.method=JWT"), "example 2: {text}");
        assert!(text.contains("api.style=REST"), "example 3: {text}");
        assert!(text.contains("Do NOT record"), "anti-examples: {text}");
        assert!(text.contains("edda note"), "note command: {text}");
    }

    // ── Recall Rate Counter tests ──

    #[test]
    fn counter_increment_and_read() {
        let pid = "test_counter_ops";
        let sid = "sess-counter-1";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);

        // Read non-existent counter returns 0
        assert_eq!(read_counter(pid, sid, "nudge_count"), 0);

        // Increment 3 times, read, assert 3
        increment_counter(pid, sid, "nudge_count");
        increment_counter(pid, sid, "nudge_count");
        increment_counter(pid, sid, "nudge_count");
        assert_eq!(read_counter(pid, sid, "nudge_count"), 3);

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn post_tool_use_increments_nudge_counter() {
        let pid = "test_nudge_counter";
        let sid = "sess-nudge-cnt-1";
        let _ = edda_store::ensure_dirs(pid);

        // First commit → nudge emitted → counter = 1
        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m \"feat: first\"" }
        });
        let result = dispatch_post_tool_use(&raw, pid, sid).unwrap();
        assert!(result.stdout.is_some(), "should produce nudge");
        assert_eq!(read_counter(pid, sid, "nudge_count"), 1);

        // Reset cooldown by removing nudge_ts
        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::remove_file(state_dir.join(format!("nudge_ts.{sid}")));

        // Second commit → nudge emitted → counter = 2
        let raw2 = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m \"feat: second\"" }
        });
        let result2 = dispatch_post_tool_use(&raw2, pid, sid).unwrap();
        assert!(
            result2.stdout.is_some(),
            "should produce nudge after cooldown reset"
        );
        assert_eq!(read_counter(pid, sid, "nudge_count"), 2);

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn post_tool_use_increments_decide_counter() {
        let pid = "test_decide_counter";
        let sid = "sess-decide-cnt-1";
        let _ = edda_store::ensure_dirs(pid);

        let raw = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "edda decide \"db=postgres\" --reason \"need JSONB\"" }
        });
        dispatch_post_tool_use(&raw, pid, sid).unwrap();
        assert_eq!(read_counter(pid, sid, "decide_count"), 1);

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn session_end_cleans_recall_counters() {
        let pid = "test_counter_cleanup";
        let sid = "sess-counter-clean";
        let _ = edda_store::ensure_dirs(pid);

        // Create counter files
        increment_counter(pid, sid, "nudge_count");
        increment_counter(pid, sid, "decide_count");

        let state_dir = edda_store::project_dir(pid).join("state");
        assert!(state_dir.join(format!("nudge_count.{sid}")).exists());
        assert!(state_dir.join(format!("decide_count.{sid}")).exists());

        cleanup_session_state(pid, sid, false);

        assert!(!state_dir.join(format!("nudge_count.{sid}")).exists());
        assert!(!state_dir.join(format!("decide_count.{sid}")).exists());

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn signal_count_incremented_for_all_signals() {
        let pid = "test_signal_count_all";
        let sid = "sess-sig-cnt";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);

        // Commit signal → signal_count +1
        let raw1 = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m \"feat: add auth\"" }
        });
        dispatch_post_tool_use(&raw1, pid, sid).unwrap();
        assert_eq!(read_counter(pid, sid, "signal_count"), 1);

        // SelfRecord signal → signal_count +1 (even though no nudge sent)
        let raw2 = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "edda decide \"db=postgres\"" }
        });
        dispatch_post_tool_use(&raw2, pid, sid).unwrap();
        assert_eq!(read_counter(pid, sid, "signal_count"), 2);

        // DependencyAdd signal → signal_count +1 (suppressed by cooldown)
        let raw3 = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "cargo add serde" }
        });
        dispatch_post_tool_use(&raw3, pid, sid).unwrap();
        assert_eq!(read_counter(pid, sid, "signal_count"), 3);

        // signal_count >= nudge_count always
        assert!(read_counter(pid, sid, "signal_count") >= read_counter(pid, sid, "nudge_count"));

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // nudge_cooldown_env_var_override → moved to state::tests

    #[test]
    fn session_end_cleans_signal_count() {
        let pid = "test_signal_count_cleanup";
        let sid = "sess-sig-clean";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);

        increment_counter(pid, sid, "signal_count");

        let state_dir = edda_store::project_dir(pid).join("state");
        assert!(state_dir.join(format!("signal_count.{sid}")).exists());

        cleanup_session_state(pid, sid, false);

        assert!(!state_dir.join(format!("signal_count.{sid}")).exists());

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn has_active_peers_false_when_solo() {
        let pid = "test_dispatch_solo_gate";
        let _ = edda_store::ensure_dirs(pid);
        // No heartbeat files → no peers
        assert!(!has_active_peers(pid, "my-session"));
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn has_active_peers_true_when_peer_exists() {
        let pid = "test_dispatch_peer_gate";
        let _ = edda_store::ensure_dirs(pid);
        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::create_dir_all(&state_dir);

        // Create a fresh peer heartbeat file
        let peer_path = state_dir.join("session.peer-session.json");
        fs::write(&peer_path, r#"{"session_id":"peer-session"}"#).unwrap();

        assert!(has_active_peers(pid, "my-session"));
        // Own session should be excluded
        assert!(!has_active_peers(pid, "peer-session"));

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Issue #148 Gap 3: Cross-session binding visibility via dispatch ──

    #[test]
    fn cross_session_binding_visible_via_user_prompt_submit() {
        let pid = "test_xsess_bind_vis";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

        // Create temp cwd (no .edda/ — workspace section will be None)
        let cwd = std::env::temp_dir().join("edda_xsess_vis_cwd");
        let _ = fs::create_dir_all(&cwd);

        // Multi-session: write heartbeats for s1 and s2
        let signals = crate::signals::SessionSignals::default();
        crate::peers::write_heartbeat(pid, "s1", &signals, Some("auth"));
        crate::peers::write_heartbeat(pid, "s2", &signals, Some("billing"));

        // Session A (s1) writes a binding
        crate::peers::write_binding(pid, "s1", "auth", "db.engine", "postgres");

        // Session B (s2) dispatches UserPromptSubmit — should see the binding
        let result = dispatch_user_prompt_submit(pid, "s2", "", cwd.to_str().unwrap()).unwrap();
        assert!(
            result.stdout.is_some(),
            "should return output (not dedup-skipped)"
        );

        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or("");
        assert!(
            ctx.contains("db.engine"),
            "should contain binding key, got:\n{ctx}"
        );
        assert!(
            ctx.contains("postgres"),
            "should contain binding value, got:\n{ctx}"
        );

        crate::peers::remove_heartbeat(pid, "s1");
        crate::peers::remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = fs::remove_dir_all(&cwd);
    }

    #[test]
    fn user_prompt_submit_dedup_skips_identical_state() {
        let pid = "test_ups_dedup";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

        let cwd = std::env::temp_dir().join("edda_dedup_cwd");
        let _ = fs::create_dir_all(&cwd);

        // Write a binding so there's something to inject
        crate::peers::write_binding(pid, "s1", "auth", "cache.backend", "redis");

        // First call — should produce output
        let r1 = dispatch_user_prompt_submit(pid, "dedup-sess", "", cwd.to_str().unwrap()).unwrap();
        assert!(r1.stdout.is_some(), "first call should return output");

        // Second call with identical state — should be dedup-skipped
        let r2 = dispatch_user_prompt_submit(pid, "dedup-sess", "", cwd.to_str().unwrap()).unwrap();
        assert!(
            r2.stdout.is_none(),
            "second call should be dedup-skipped (empty)"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = fs::remove_dir_all(&cwd);
    }

    // ── Issue #148 Gap 5: Solo session binding visibility ──

    #[test]
    fn solo_session_still_sees_bindings_via_prompt_submit() {
        let pid = "test_solo_bind_vis";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

        let cwd = std::env::temp_dir().join("edda_solo_vis_cwd");
        let _ = fs::create_dir_all(&cwd);

        // Write binding — no heartbeats (solo mode)
        crate::peers::write_binding(pid, "solo-s", "solo", "api.style", "GraphQL");

        let result = dispatch_user_prompt_submit(pid, "solo-s", "", cwd.to_str().unwrap()).unwrap();
        assert!(result.stdout.is_some(), "solo session should see bindings");
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or("");
        assert!(
            ctx.contains("GraphQL"),
            "solo session should see binding value, got:\n{ctx}"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = fs::remove_dir_all(&cwd);
    }

    // ── Issue #148 Gap 5: Solo → multi-session transition ──

    #[test]
    fn solo_to_multi_session_transition() {
        let pid = "test_solo_multi_trans";
        // Clean slate to avoid interference from other tests
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);
        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::create_dir_all(&state_dir);

        // Phase 1: Only own heartbeat → solo (no active peers)
        let own_hb = state_dir.join("session.s1.json");
        fs::write(&own_hb, r#"{"session_id":"s1"}"#).unwrap();
        assert!(
            !has_active_peers(pid, "s1"),
            "should be solo with only own heartbeat"
        );

        // Phase 2: Peer appears → multi-session
        let peer_hb = state_dir.join("session.s2.json");
        fs::write(&peer_hb, r#"{"session_id":"s2"}"#).unwrap();
        assert!(
            has_active_peers(pid, "s1"),
            "should detect peer after heartbeat written"
        );

        // Phase 3: Peer goes stale → back to solo
        // Sleep to ensure file mtime is in the past, then set threshold to 0
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::env::set_var("EDDA_PEER_STALE_SECS", "0");
        assert!(
            !has_active_peers(pid, "s1"),
            "peer should be stale after threshold=0 with old mtime"
        );
        std::env::remove_var("EDDA_PEER_STALE_SECS");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Issue #148 Gap 7: SessionEnd unclaim gating ──

    #[test]
    fn session_end_unclaim_only_with_active_peers() {
        let pid = "test_se_unclaim_gate";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

        let cwd = std::env::temp_dir().join("edda_se_unclaim_cwd");
        let _ = fs::create_dir_all(&cwd);

        // Write claims for two sessions
        crate::peers::write_claim(pid, "s1", "auth", &["src/auth.rs".into()]);
        crate::peers::write_claim(pid, "s2", "billing", &["src/bill.rs".into()]);

        // SessionEnd with peers_active=false — should NOT write unclaim
        let _ = dispatch_session_end(pid, "s1", "", cwd.to_str().unwrap(), false);

        // Read coordination.jsonl and check no unclaim for s1
        let coord_path = edda_store::project_dir(pid)
            .join("state")
            .join("coordination.jsonl");
        let content = fs::read_to_string(&coord_path).unwrap_or_default();
        let unclaim_count = content
            .lines()
            .filter(|l| l.contains("\"unclaim\"") && l.contains("s1"))
            .count();
        assert_eq!(unclaim_count, 0, "no unclaim when peers_active=false");

        // Write fresh claim for s3 and end with peers_active=true — SHOULD write unclaim
        crate::peers::write_claim(pid, "s3", "infra", &["infra/main.tf".into()]);
        let _ = dispatch_session_end(pid, "s3", "", cwd.to_str().unwrap(), true);

        let content2 = fs::read_to_string(&coord_path).unwrap_or_default();
        let unclaim_s3 = content2
            .lines()
            .filter(|l| l.contains("\"unclaim\"") && l.contains("s3"))
            .count();
        assert!(unclaim_s3 > 0, "should have unclaim when peers_active=true");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = fs::remove_dir_all(&cwd);
    }

    #[test]
    fn session_end_reads_counters_before_cleanup() {
        let pid = "test_se_counters";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);
        let sid = "counter-sess";
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

        let cwd = std::env::temp_dir().join("edda_se_counter_cwd");
        let _ = fs::create_dir_all(&cwd);

        // Set up counters
        increment_counter(pid, sid, "decide_count");
        increment_counter(pid, sid, "decide_count");
        increment_counter(pid, sid, "decide_count");
        increment_counter(pid, sid, "nudge_count");
        increment_counter(pid, sid, "nudge_count");
        increment_counter(pid, sid, "signal_count");

        // Verify counters exist before SessionEnd
        let state_dir = edda_store::project_dir(pid).join("state");
        assert!(state_dir.join(format!("decide_count.{sid}")).exists());
        assert!(state_dir.join(format!("nudge_count.{sid}")).exists());
        assert!(state_dir.join(format!("signal_count.{sid}")).exists());

        // SessionEnd should read counters then clean them up
        let result = dispatch_session_end(pid, sid, "", cwd.to_str().unwrap(), false);
        assert!(result.is_ok(), "session_end should not error");

        // Counter files should be cleaned up
        assert!(
            !state_dir.join(format!("decide_count.{sid}")).exists(),
            "decide_count should be cleaned"
        );
        assert!(
            !state_dir.join(format!("nudge_count.{sid}")).exists(),
            "nudge_count should be cleaned"
        );
        assert!(
            !state_dir.join(format!("signal_count.{sid}")).exists(),
            "signal_count should be cleaned"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = fs::remove_dir_all(&cwd);
    }

    // ── Issue #11: Late Peer Detection ──

    #[test]
    fn late_peer_detection_injects_full_protocol() {
        let pid = "test_late_peer_full";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

        let cwd = std::env::temp_dir().join("edda_late_peer_full_cwd");
        let _ = fs::create_dir_all(&cwd);

        let sid = "solo-sess";
        // No peer_count file yet (virgin session) → prev_count = 0
        // Create a peer heartbeat to simulate a second agent joining
        let signals = crate::signals::SessionSignals::default();
        crate::peers::write_heartbeat(pid, "peer-a", &signals, Some("billing"));

        let result =
            dispatch_with_workspace_only(pid, sid, cwd.to_str().unwrap(), "UserPromptSubmit")
                .unwrap();
        assert!(result.stdout.is_some(), "should return output");

        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or("");
        assert!(
            ctx.contains("Coordination Protocol") || ctx.contains("edda claim"),
            "should contain full coordination protocol on first peer detection, got:\n{ctx}"
        );

        // Verify peer_count state file was written
        let state_dir = edda_store::project_dir(pid).join("state");
        let count_file = state_dir.join(format!("peer_count.{sid}"));
        assert!(count_file.exists(), "peer_count file should be created");
        let count: usize = fs::read_to_string(&count_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(count, 1, "peer_count should be 1");

        crate::peers::remove_heartbeat(pid, "peer-a");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = fs::remove_dir_all(&cwd);
    }

    #[test]
    fn subsequent_prompts_use_lightweight_updates() {
        let pid = "test_late_peer_subsequent";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

        let cwd = std::env::temp_dir().join("edda_late_peer_subseq_cwd");
        let _ = fs::create_dir_all(&cwd);

        let sid = "known-peers-sess";
        // Pre-set peer_count to 1 (peer already known from previous prompt)
        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::create_dir_all(&state_dir);
        fs::write(state_dir.join(format!("peer_count.{sid}")), "1").unwrap();

        // Peer heartbeat still active
        let signals = crate::signals::SessionSignals::default();
        crate::peers::write_heartbeat(pid, "peer-b", &signals, Some("auth"));

        let result =
            dispatch_with_workspace_only(pid, sid, cwd.to_str().unwrap(), "UserPromptSubmit")
                .unwrap();
        assert!(result.stdout.is_some(), "should return output");

        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or("");
        // Should have lightweight peer updates, not full protocol
        assert!(
            ctx.contains("## Peers"),
            "should contain lightweight peer header, got:\n{ctx}"
        );

        crate::peers::remove_heartbeat(pid, "peer-b");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = fs::remove_dir_all(&cwd);
    }

    #[test]
    fn solo_session_writes_zero_peer_count() {
        let pid = "test_late_peer_solo";
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = edda_store::ensure_dirs(pid);
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        std::env::set_var("EDDA_PLANS_DIR", "/nonexistent");

        let cwd = std::env::temp_dir().join("edda_late_peer_solo_cwd");
        let _ = fs::create_dir_all(&cwd);

        let sid = "solo-only";
        // No peers — dispatch should still work, writing peer_count = 0
        let _ = dispatch_with_workspace_only(pid, sid, cwd.to_str().unwrap(), "UserPromptSubmit");

        let state_dir = edda_store::project_dir(pid).join("state");
        let count_file = state_dir.join(format!("peer_count.{sid}"));
        assert!(
            count_file.exists(),
            "peer_count file should be created even for solo"
        );
        let count: usize = fs::read_to_string(&count_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(count, 0, "peer_count should be 0 for solo session");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        let _ = fs::remove_dir_all(&cwd);
    }

    #[test]
    fn peer_count_cleaned_on_session_end() {
        let pid = "test_peer_count_clean";
        let _ = edda_store::ensure_dirs(pid);
        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::create_dir_all(&state_dir);

        let sid = "clean-sess";
        fs::write(state_dir.join(format!("peer_count.{sid}")), "2").unwrap();
        assert!(state_dir.join(format!("peer_count.{sid}")).exists());

        cleanup_session_state(pid, sid, false);

        assert!(
            !state_dir.join(format!("peer_count.{sid}")).exists(),
            "peer_count should be cleaned on session end"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }
}
