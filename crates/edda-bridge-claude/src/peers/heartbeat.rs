use std::fs;
use std::io::Write;

use crate::parse::now_rfc3339;
use crate::signals::SessionSignals;

use super::board::compute_board_state;
use super::helpers::auto_label;
use super::{
    coordination_path, detect_git_branch, detect_git_branch_in, env_label, heartbeat_path,
    BindingConflict, CoordEvent, CoordEventType, SessionHeartbeat,
};

// ── Heartbeat Write/Read ──

/// Write a full heartbeat (called from ingest_and_build_pack after signal extraction).
pub(crate) fn write_heartbeat(
    project_id: &str,
    session_id: &str,
    signals: &SessionSignals,
    label: Option<&str>,
) {
    let now = now_rfc3339();
    let path = heartbeat_path(project_id, session_id);

    // Preserve started_at from existing heartbeat, or use now
    let started_at = read_heartbeat(project_id, session_id)
        .map(|h| h.started_at)
        .unwrap_or_else(|| now.clone());

    let derived_label = label
        .map(|s| s.to_string())
        .or_else(env_label)
        .unwrap_or_else(|| auto_label(signals));

    let heartbeat = SessionHeartbeat {
        session_id: session_id.to_string(),
        started_at,
        last_heartbeat: now,
        label: derived_label,
        focus_files: signals
            .files_modified
            .iter()
            .take(5)
            .map(|f| f.path.clone())
            .collect(),
        active_tasks: signals.tasks.clone(),
        files_modified_count: signals.files_modified.len(),
        total_edits: signals.files_modified.iter().map(|f| f.count).sum(),
        recent_commits: signals
            .commits
            .iter()
            .rev()
            .take(3)
            .map(|c| format!("{} {}", &c.hash[..7.min(c.hash.len())], c.message))
            .collect(),
        branch: detect_git_branch(),
        current_phase: crate::agent_phase::read_phase_state(project_id, session_id)
            .map(|ps| ps.phase.to_string()),
        parent_session_id: None,
    };

    let data = match serde_json::to_string_pretty(&heartbeat) {
        Ok(d) => d,
        Err(_) => return,
    };
    let _ = edda_store::write_atomic(&path, data.as_bytes());
}

/// Lightweight heartbeat touch: only update last_heartbeat timestamp.
pub fn touch_heartbeat(project_id: &str, session_id: &str) {
    let path = heartbeat_path(project_id, session_id);
    if let Some(mut hb) = read_heartbeat(project_id, session_id) {
        hb.last_heartbeat = now_rfc3339();
        if let Ok(data) = serde_json::to_string_pretty(&hb) {
            let _ = edda_store::write_atomic(&path, data.as_bytes());
        }
    }
    // If no existing heartbeat, skip touch (write_heartbeat will create it)
}

/// Update the branch field in an existing heartbeat.
/// Called when the agent intentionally switches branch (git checkout / git switch).
pub(crate) fn update_heartbeat_branch(project_id: &str, session_id: &str, branch: &str) {
    let path = heartbeat_path(project_id, session_id);
    if let Some(mut hb) = read_heartbeat(project_id, session_id) {
        hb.branch = Some(branch.to_string());
        if let Ok(data) = serde_json::to_string_pretty(&hb) {
            let _ = edda_store::write_atomic(&path, data.as_bytes());
        }
    }
}

/// Ensure a heartbeat file exists for this session.
/// If one already exists (e.g. written by `ingest_and_build_pack`), it is preserved.
/// If none exists, writes a minimal heartbeat with empty signals so that other
/// sessions can discover this peer via `discover_active_peers` immediately.
///
/// This is needed because `ingest_and_build_pack` skips when the transcript file
/// doesn't exist yet — which is the normal case for brand-new SessionStart events
/// (Claude Code creates the transcript *after* the hook fires).
pub(crate) fn ensure_heartbeat_exists(project_id: &str, session_id: &str) {
    if read_heartbeat(project_id, session_id).is_some() {
        return;
    }
    write_heartbeat(project_id, session_id, &SessionSignals::default(), None);
}

/// Remove heartbeat on SessionEnd.
pub fn remove_heartbeat(project_id: &str, session_id: &str) {
    let _ = fs::remove_file(heartbeat_path(project_id, session_id));
}

/// Write a minimal heartbeat for CLI/external bridge use (no signal data).
///
/// Creates a heartbeat with the given label and empty signals, sufficient
/// for peer discovery. Use `write_heartbeat` for full signal-enriched heartbeats.
pub fn write_heartbeat_minimal(project_id: &str, session_id: &str, label: &str) {
    let now = now_rfc3339();
    let path = heartbeat_path(project_id, session_id);

    let started_at = read_heartbeat(project_id, session_id)
        .map(|h| h.started_at)
        .unwrap_or_else(|| now.clone());

    let heartbeat = SessionHeartbeat {
        session_id: session_id.to_string(),
        started_at,
        last_heartbeat: now,
        label: label.to_string(),
        focus_files: Vec::new(),
        active_tasks: Vec::new(),
        files_modified_count: 0,
        total_edits: 0,
        recent_commits: Vec::new(),
        branch: detect_git_branch(),
        current_phase: None,
        parent_session_id: None,
    };

    let data = match serde_json::to_string_pretty(&heartbeat) {
        Ok(d) => d,
        Err(_) => return,
    };
    let _ = edda_store::write_atomic(&path, data.as_bytes());
}

/// Write a heartbeat for a sub-agent spawned via Claude Code's Task tool.
/// Uses agent_id as session identifier and records parent session for cleanup.
pub(crate) fn write_subagent_heartbeat(
    project_id: &str,
    agent_id: &str,
    parent_session_id: &str,
    label: &str,
    cwd: &str,
) {
    let now = now_rfc3339();
    let path = heartbeat_path(project_id, agent_id);
    let heartbeat = SessionHeartbeat {
        session_id: agent_id.to_string(),
        started_at: now.clone(),
        last_heartbeat: now,
        label: label.to_string(),
        focus_files: Vec::new(),
        active_tasks: Vec::new(),
        files_modified_count: 0,
        total_edits: 0,
        recent_commits: Vec::new(),
        branch: detect_git_branch_in(cwd),
        current_phase: None,
        parent_session_id: Some(parent_session_id.to_string()),
    };
    let data = match serde_json::to_string_pretty(&heartbeat) {
        Ok(d) => d,
        Err(_) => return,
    };
    let _ = edda_store::write_atomic(&path, data.as_bytes());
}

/// Remove all sub-agent heartbeats belonging to a parent session.
/// Called during parent's SessionEnd cleanup to prevent orphans.
pub(crate) fn cleanup_subagent_heartbeats(project_id: &str, parent_session_id: &str) {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let entries = match fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }
        if let Ok(content) = fs::read_to_string(entry.path()) {
            if let Ok(hb) = serde_json::from_str::<SessionHeartbeat>(&content) {
                if hb.parent_session_id.as_deref() == Some(parent_session_id) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// Read a single session's heartbeat file.
pub(crate) fn read_heartbeat(project_id: &str, session_id: &str) -> Option<SessionHeartbeat> {
    let path = heartbeat_path(project_id, session_id);
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

// ── Coordination Events (append-only log) ──

/// Append a coordination event to coordination.jsonl.
pub(crate) fn append_coord_event(project_id: &str, event: &CoordEvent) {
    let path = coordination_path(project_id);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let line = match serde_json::to_string(event) {
        Ok(l) => l,
        Err(_) => return,
    };
    let mut file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let _ = writeln!(file, "{line}");
}

/// Write a claim event.
pub fn write_claim(project_id: &str, session_id: &str, label: &str, paths: &[String]) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::Claim,
        payload: serde_json::json!({
            "label": label,
            "paths": paths,
        }),
    };
    append_coord_event(project_id, &event);
}

/// Write an unclaim event (on session end).
pub fn write_unclaim(project_id: &str, session_id: &str) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::Unclaim,
        payload: serde_json::json!({}),
    };
    append_coord_event(project_id, &event);
}

/// Write a binding event to the coordination log.
pub fn write_binding(project_id: &str, session_id: &str, label: &str, key: &str, value: &str) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::Binding,
        payload: serde_json::json!({
            "key": key,
            "value": value,
            "by_label": label,
        }),
    };
    append_coord_event(project_id, &event);
}

/// Write a cross-agent request event.
pub fn write_request(
    project_id: &str,
    session_id: &str,
    from_label: &str,
    to_label: &str,
    message: &str,
) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::Request,
        payload: serde_json::json!({
            "from_label": from_label,
            "to_label": to_label,
            "message": message,
        }),
    };
    append_coord_event(project_id, &event);
}

/// Write a request acknowledgement event.
pub fn write_request_ack(project_id: &str, session_id: &str, from_label: &str) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::RequestAck,
        payload: serde_json::json!({ "from_label": from_label }),
    };
    append_coord_event(project_id, &event);
}

/// Data describing a completed sub-agent's work output.
pub(crate) struct SubagentReport<'a> {
    pub agent_id: &'a str,
    pub agent_type: &'a str,
    pub summary: &'a str,
    pub files_touched: &'a [String],
    pub decisions: &'a [String],
    pub commits: &'a [String],
}

/// Write a sub-agent completion summary event.
pub(crate) fn write_subagent_completed(
    project_id: &str,
    parent_session_id: &str,
    report: &SubagentReport<'_>,
) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: parent_session_id.to_string(),
        event_type: CoordEventType::SubagentCompleted,
        payload: serde_json::json!({
            "kind": "subagent_completed",
            "parent_session_id": parent_session_id,
            "agent_id": report.agent_id,
            "agent_type": report.agent_type,
            "summary": report.summary,
            "files_touched": report.files_touched,
            "decisions": report.decisions,
            "commits": report.commits,
        }),
    };
    append_coord_event(project_id, &event);
}

/// Check if a binding conflict exists for the given key in coordination.jsonl.
///
/// Returns `Some(BindingConflict)` if a binding with the same key but a
/// different value already exists. Returns `None` if no existing binding
/// or the value is identical (idempotent re-decide).
pub fn find_binding_conflict(
    project_id: &str,
    key: &str,
    new_value: &str,
) -> Option<BindingConflict> {
    let board = compute_board_state(project_id);
    let existing = board.bindings.iter().find(|b| b.key == key)?;
    if existing.value == new_value {
        return None; // idempotent — same value, no conflict
    }
    Some(BindingConflict {
        existing_value: existing.value.clone(),
        by_session: existing.by_session.clone(),
        by_label: existing.by_label.clone(),
        ts: existing.ts.clone(),
    })
}
