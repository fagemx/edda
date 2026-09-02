use std::fs;
use std::io::Write;

use crate::parse::now_rfc3339;
use crate::signals::SessionSignals;

use super::board::{compute_board_state, partition_requests_for_session};
use super::helpers::{auto_label, parse_rfc3339_to_epoch, session_label_from_board};
use super::{
    coordination_path, detect_git_branch_in, env_label, heartbeat_path, read_heartbeat, stale_secs,
    BindingConflict, CoordEvent, CoordEventType, SessionHeartbeat,
};

// ── Heartbeat Write/Read ──

/// Write a full heartbeat (called from ingest_and_build_pack after signal extraction).
/// Read-modify-write via `edda_store::update_heartbeat`: the hook producer
/// owns the signal fields; lane fields (plan/phase/attempt/stage/pid) and
/// the parent link written by other producers on the same shared surface are
/// preserved, and concurrent writers are serialized by the sidecar lock.
pub(crate) fn write_heartbeat(
    project_id: &str,
    session_id: &str,
    signals: &SessionSignals,
    label: Option<&str>,
    cwd: &str,
) {
    let now = now_rfc3339();

    let branch = detect_git_branch_in(cwd);

    let derived_label = label
        .map(|s| s.to_string())
        .or_else(env_label)
        .unwrap_or_else(|| {
            let auto = auto_label(signals, Some(cwd));
            if auto.is_empty() {
                // Fresh session: no edits yet, so `auto_label` is empty. Fall back
                // to the git branch so the peer stays identifiable in `edda watch`
                // and can still receive `edda request` (#128). This is a presence
                // signal on the heartbeat — never a scope claim, which is what
                // used to block every peer under enforce_offlimits (#444).
                branch.clone().unwrap_or_default()
            } else {
                auto
            }
        });

    let _ = edda_store::update_heartbeat(project_id, session_id, |hb| {
        if hb.started_at.is_empty() {
            hb.started_at = now.clone();
        }
        hb.last_heartbeat = now.clone();
        hb.label = derived_label.clone();
        hb.focus_files = signals
            .files_modified
            .iter()
            .take(5)
            .map(|f| f.path.clone())
            .collect();
        hb.active_tasks = signals.tasks.clone();
        hb.files_modified_count = signals.files_modified.len();
        hb.total_edits = signals.files_modified.iter().map(|f| f.count).sum();
        hb.recent_commits = signals
            .commits
            .iter()
            .rev()
            .take(3)
            .map(|c| format!("{} {}", &c.hash[..7.min(c.hash.len())], c.message))
            .collect();
        hb.branch = branch.clone();
        hb.current_phase = crate::agent_phase::read_phase_state(project_id, session_id)
            .map(|ps| ps.phase.to_string());
        // parent_session_id and the conductor lane fields (plan, phase,
        // attempt, stage, pid) belong to other producers — preserved.
    });
}

/// Lightweight heartbeat touch: only update last_heartbeat timestamp.
/// Rides the shared locked update; a still-virgin record is not persisted.
pub fn touch_heartbeat(project_id: &str, session_id: &str) {
    let now = now_rfc3339();
    let _ = edda_store::update_heartbeat(project_id, session_id, |hb| {
        if hb.started_at.is_empty() && hb.last_heartbeat.is_empty() {
            // No existing heartbeat: skip touch (write_heartbeat creates it).
            return;
        }
        hb.last_heartbeat = now.clone();
    });
}

/// Update the branch field in an existing heartbeat.
/// Called when the agent intentionally switches branch (git checkout / git switch).
pub(crate) fn update_heartbeat_branch(project_id: &str, session_id: &str, branch: &str) {
    let _ = edda_store::update_heartbeat(project_id, session_id, |hb| {
        if hb.started_at.is_empty() && hb.last_heartbeat.is_empty() {
            // No existing heartbeat: nothing to update.
            return;
        }
        hb.branch = Some(branch.to_string());
    });
}

/// Ensure a heartbeat file exists for this session.
/// If one already exists (e.g. written by `ingest_and_build_pack`), it is preserved.
/// If none exists, writes a minimal heartbeat with empty signals so that other
/// sessions can discover this peer via `discover_active_peers` immediately.
///
/// This is needed because `ingest_and_build_pack` skips when the transcript file
/// doesn't exist yet — which is the normal case for brand-new SessionStart events
/// (Claude Code creates the transcript *after* the hook fires).
pub(crate) fn ensure_heartbeat_exists(project_id: &str, session_id: &str, cwd: &str) {
    if read_heartbeat(project_id, session_id).is_some() {
        return;
    }
    write_heartbeat(
        project_id,
        session_id,
        &SessionSignals::default(),
        None,
        cwd,
    );
}

/// Remove heartbeat on SessionEnd.
pub fn remove_heartbeat(project_id: &str, session_id: &str) {
    let _ = fs::remove_file(heartbeat_path(project_id, session_id));
}

/// Write a minimal heartbeat for CLI/external bridge use (no signal data).
///
/// Creates a heartbeat with the given label and empty signals, sufficient
/// for peer discovery. Use `write_heartbeat` for full signal-enriched heartbeats.
pub fn write_heartbeat_minimal(project_id: &str, session_id: &str, label: &str, cwd: &str) {
    let now = now_rfc3339();
    let branch = detect_git_branch_in(cwd);
    let _ = edda_store::update_heartbeat(project_id, session_id, |hb| {
        if hb.started_at.is_empty() {
            hb.started_at = now.clone();
        }
        hb.last_heartbeat = now.clone();
        hb.label = label.to_string();
        if branch.is_some() {
            hb.branch = branch.clone();
        }
        // Signal fields and the conductor lane fields belong to other
        // producers on the shared surface — preserved.
    });
}

/// Write a heartbeat for a sub-agent spawned via Claude Code's Task tool.
/// Uses agent_id as session identifier and records parent session for cleanup.
///
/// Rides the shared sidecar lock via `update_heartbeat` like every other
/// producer: a raw whole-record write here would clobber anything another
/// producer had already written for this id instead of refreshing it.
pub(crate) fn write_subagent_heartbeat(
    project_id: &str,
    agent_id: &str,
    parent_session_id: &str,
    label: &str,
    cwd: &str,
) {
    let now = now_rfc3339();
    let branch = detect_git_branch_in(cwd);
    let _ = edda_store::update_heartbeat(project_id, agent_id, |hb| {
        if hb.started_at.is_empty() {
            hb.started_at = now.clone();
        }
        hb.last_heartbeat = now;
        hb.label = label.to_string();
        hb.parent_session_id = Some(parent_session_id.to_string());
        if branch.is_some() {
            hb.branch = branch;
        }
        // Signal fields and lane fields belong to other producers — preserved.
    });
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

// read_heartbeat lives in edda_store (re-exported by super).

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

/// Write a cross-agent request event. Returns the request id.
///
/// Every request carries its own id so an ack can name the message it answers
/// rather than the peer that sent it (GH-442).
pub fn write_request(
    project_id: &str,
    session_id: &str,
    from_label: &str,
    to_label: &str,
    message: &str,
) -> String {
    let id = ulid::Ulid::new().to_string();
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::Request,
        payload: serde_json::json!({
            "id": id,
            "from_label": from_label,
            "to_label": to_label,
            "message": message,
        }),
    };
    append_coord_event(project_id, &event);
    id
}

/// Write a request acknowledgement event.
///
/// Resolves which of `from_label`'s messages are actually outstanding for this
/// session and records their ids, so the ack retires exactly those messages and
/// nothing that arrives afterwards.
pub fn write_request_ack(project_id: &str, session_id: &str, from_label: &str) {
    let board = compute_board_state(project_id);
    let my_label = session_label_from_board(&board, project_id, session_id);
    let (live, _expired) = partition_requests_for_session(&board, session_id, &my_label);
    let request_ids: Vec<String> = live
        .iter()
        .filter(|r| r.from_label == from_label)
        .map(|r| r.id.clone())
        .collect();

    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::RequestAck,
        // No resolvable pending message (unidentified session, or an ack for
        // something already retired): fall back to a label-scoped ack, which
        // board state bounds by timestamp so it cannot swallow future messages.
        payload: serde_json::json!({
            "from_label": from_label,
            "request_ids": request_ids,
        }),
    };
    append_coord_event(project_id, &event);
}

/// Resolve a request target label to the active sessions that would receive it.
///
/// Returns the session ids of every non-stale session whose claim label or
/// heartbeat label matches. Zero means the message is a dead letter — usually a
/// typo; more than one means the label is ambiguous and the message will land in
/// several inboxes (GH-443).
pub fn resolve_request_targets(project_id: &str, to_label: &str) -> Vec<String> {
    if to_label.is_empty() {
        return Vec::new();
    }
    let state_dir = edda_store::project_dir(project_id).join("state");
    let entries = match fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let board = compute_board_state(project_id);
    let stale_threshold = stale_secs();
    let now = parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0);

    let mut targets = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }
        let hb: SessionHeartbeat = match fs::read_to_string(entry.path())
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
        {
            Some(h) => h,
            None => continue,
        };

        // Parented sub-agent heartbeats match the extended threshold peer
        // discovery uses for them (they have no in-flight writer); conductor
        // lanes write real periodic heartbeats and use the standard one.
        let effective_threshold = if hb.parent_session_id.is_some() {
            stale_threshold * 15
        } else {
            stale_threshold
        };
        let age = now.saturating_sub(parse_rfc3339_to_epoch(&hb.last_heartbeat).unwrap_or(0));
        if age > effective_threshold {
            continue;
        }

        // Claim wins over heartbeat, matching how a session resolves the label
        // it answers to — otherwise a claimed session looks reachable under a
        // stale heartbeat label it no longer reads.
        let effective_label = board
            .claims
            .iter()
            .find(|c| c.session_id == hb.session_id)
            .map(|c| c.label.as_str())
            .unwrap_or(hb.label.as_str());
        if effective_label == to_label {
            targets.push(hb.session_id);
        }
    }
    targets
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

/// Write a task completion event to coordination.jsonl.
pub(crate) fn write_task_completed(
    project_id: &str,
    session_id: &str,
    task_id: &str,
    task_subject: &str,
    task_description: &str,
) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::TaskCompleted,
        payload: serde_json::json!({
            "task_id": task_id,
            "task_subject": task_subject,
            "task_description": task_description,
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

/// Resolve a teammate name to a session_id by scanning active heartbeats.
/// Returns `None` if no match found (teammate_name doesn't match any label or session_id).
pub(crate) fn resolve_teammate_session(project_id: &str, teammate_name: &str) -> Option<String> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let entries = fs::read_dir(&state_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }
        if let Ok(content) = fs::read_to_string(entry.path()) {
            if let Ok(hb) = serde_json::from_str::<SessionHeartbeat>(&content) {
                if hb.label == teammate_name || hb.session_id == teammate_name {
                    return Some(hb.session_id);
                }
            }
        }
    }
    None
}

/// Update a teammate's heartbeat phase to the given value.
/// Used to set phase to "idle" when a TeammateIdle event is received.
///
/// Rides the shared sidecar lock via `update_heartbeat`: an unlocked
/// read-modify-write here raced the runner's locked refresh (read a
/// pre-lane record, then atomically restored it after the runner wrote
/// lane fields, erasing them).
pub(crate) fn update_teammate_phase(project_id: &str, session_id: &str, phase: &str) {
    let _ = edda_store::update_heartbeat(project_id, session_id, |hb| {
        // `update_heartbeat` seeds a blank record when no file exists; a
        // TeammateIdle event must not create a heartbeat for a session that
        // never had one, so leave the still-blank record unwritten.
        if hb.started_at.is_empty() {
            return;
        }
        hb.current_phase = Some(phase.to_string());
        hb.last_heartbeat = now_rfc3339();
    });
}

/// Write a teammate idle event to coordination.jsonl.
pub(crate) fn write_teammate_idle(
    project_id: &str,
    notified_session_id: &str,
    teammate_name: &str,
    team_name: &str,
) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: notified_session_id.to_string(),
        event_type: CoordEventType::TeammateIdle,
        payload: serde_json::json!({
            "teammate_name": teammate_name,
            "team_name": team_name,
        }),
    };
    append_coord_event(project_id, &event);
}
