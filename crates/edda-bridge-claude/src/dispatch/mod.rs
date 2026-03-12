use std::fs;

use crate::parse::*;
use crate::render;
use crate::signals::*;
use crate::state;

mod events;
mod helpers;
mod session;
mod tools;

// Re-export sub-module items that were pub(crate) in original dispatch.rs
pub(crate) use helpers::render_active_plan;

// Sub-module function imports used in hook_entrypoint_from_stdin
use events::try_write_subagent_completed_note_event;
use helpers::run_auto_digest;
use session::{
    dispatch_session_end, dispatch_session_start, dispatch_subagent_context,
    dispatch_user_prompt_submit, ingest_and_build_pack,
};
use tools::{dispatch_post_tool_use, dispatch_pre_tool_use};

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
        "PreToolUse" => dispatch_pre_tool_use(&raw, &cwd, &project_id, &session_id),
        "PostToolUse" => dispatch_post_tool_use(&raw, &project_id, &session_id, &cwd),
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
        "SessionEnd" => {
            // Solo gate: only used to skip coordination log writes (write_unclaim).
            // Computed here (not at top) so non-SessionEnd hooks avoid the dir scan (#83).
            let peers_active = !session_id.is_empty() && has_active_peers(&project_id, &session_id);
            dispatch_session_end(
                &project_id,
                &session_id,
                &transcript_path,
                &cwd,
                peers_active,
            )
        }
        "SubagentStart" => {
            // Inject peer context BEFORE writing heartbeat so the sub-agent
            // doesn't see itself in the peer list.
            let result = dispatch_subagent_context(&project_id, &session_id);
            let agent_id = get_str(&raw, "agent_id");
            let agent_type = get_str(&raw, "agent_type");
            if !agent_id.is_empty() {
                let label = format!("sub:{agent_type}");
                crate::peers::write_subagent_heartbeat(
                    &project_id,
                    &agent_id,
                    &session_id,
                    &label,
                    &cwd,
                );
            }
            result
        }
        "SubagentStop" => {
            let agent_id = get_str(&raw, "agent_id");
            let agent_type = get_str(&raw, "agent_type");
            let agent_transcript_path = get_str(&raw, "agent_transcript_path");
            let last_assistant_message = get_str(&raw, "last_assistant_message");
            if !agent_id.is_empty() {
                let summary = extract_subagent_summary(
                    &agent_transcript_path,
                    &last_assistant_message,
                    &agent_id,
                );

                crate::peers::write_subagent_completed(
                    &project_id,
                    &session_id,
                    &agent_id,
                    &agent_type,
                    &summary.summary,
                    &summary.files_touched,
                    &summary.decisions,
                    &summary.commits,
                );

                try_write_subagent_completed_note_event(&cwd, &agent_id, &agent_type, &summary);
                crate::peers::remove_heartbeat(&project_id, &agent_id);
            }
            Ok(HookResult::empty())
        }
        _ => Ok(HookResult::empty()),
    }
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
// ── Config Helpers (delegates to render module) ──

fn read_workspace_config_bool(cwd: &str, key: &str) -> Option<bool> {
    render::config_bool(cwd, key)
}

#[allow(dead_code)]
fn read_workspace_config_usize(cwd: &str, key: &str) -> Option<usize> {
    render::config_usize(cwd, key)
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
#[path = "tests.rs"]
mod tests;
