use std::fs;

use crate::parse::now_rfc3339;

use super::board::compute_board_state;
use super::helpers::parse_rfc3339_to_epoch;
use super::liveness::{liveness_from_heartbeat, SessionLiveness};
use super::{stale_secs, PeerSummary, SessionHeartbeat};

// ── Peer Discovery ──

/// Discover active peer sessions (excluding current session and stale ones).
pub fn discover_active_peers(project_id: &str, current_session_id: &str) -> Vec<PeerSummary> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let now = parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0);

    let board = compute_board_state(project_id);

    let entries = match fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut peers = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }
        let sid = name
            .strip_prefix("session.")
            .and_then(|s| s.strip_suffix(".json"))
            .unwrap_or("");
        if sid.is_empty() || sid == current_session_id {
            continue;
        }

        let content = match fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let hb: SessionHeartbeat = match serde_json::from_str(&content) {
            Ok(h) => h,
            Err(_) => continue,
        };

        // The GH-617 shared liveness criterion — the same verdict `edda
        // claim check` applies, so the two verbs can never disagree about
        // who is dead. The sub-agent 15x multiplier lives inside the
        // criterion (see `liveness::liveness_from_heartbeat`).
        let age = match liveness_from_heartbeat(&hb, now) {
            SessionLiveness::Live { age_secs } => age_secs,
            _ => continue,
        };

        let claimed_paths = board
            .claims
            .iter()
            .find(|c| c.session_id == hb.session_id)
            .map(|c| c.paths.clone())
            .unwrap_or_default();

        let task_subjects: Vec<String> = hb
            .active_tasks
            .iter()
            .filter(|t| t.status == "in_progress")
            .take(2)
            .map(|t| t.subject.clone())
            .collect();

        peers.push(PeerSummary {
            session_id: hb.session_id,
            label: hb.label,
            age_secs: age,
            last_heartbeat: hb.last_heartbeat,
            focus_files: hb.focus_files,
            task_subjects,
            files_modified_count: hb.files_modified_count,
            recent_commits: hb.recent_commits,
            claimed_paths,
            branch: hb.branch,
            current_phase: hb.current_phase,
        });
    }

    // Sort by most recently active
    peers.sort_by_key(|p| p.age_secs);
    peers
}

/// Discover ALL sessions (including current one), for CLI display.
pub fn discover_all_sessions(project_id: &str) -> Vec<PeerSummary> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let now = parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0);
    let board = compute_board_state(project_id);

    let entries = match fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut peers = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }

        let content = match fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let hb: SessionHeartbeat = match serde_json::from_str(&content) {
            Ok(h) => h,
            Err(_) => continue,
        };

        let hb_epoch = parse_rfc3339_to_epoch(&hb.last_heartbeat).unwrap_or(0);
        let age = now.saturating_sub(hb_epoch);

        let claimed_paths = board
            .claims
            .iter()
            .find(|c| c.session_id == hb.session_id)
            .map(|c| c.paths.clone())
            .unwrap_or_default();

        let task_subjects: Vec<String> = hb
            .active_tasks
            .iter()
            .filter(|t| t.status == "in_progress")
            .take(2)
            .map(|t| t.subject.clone())
            .collect();

        peers.push(PeerSummary {
            session_id: hb.session_id,
            label: hb.label,
            age_secs: age,
            last_heartbeat: hb.last_heartbeat,
            focus_files: hb.focus_files,
            task_subjects,
            files_modified_count: hb.files_modified_count,
            recent_commits: hb.recent_commits,
            claimed_paths,
            branch: hb.branch,
            current_phase: hb.current_phase,
        });
    }

    peers.sort_by_key(|p| p.age_secs);
    peers
}

/// Infer the current session from heartbeat files.
///
/// If exactly one non-stale session exists for the project, returns
/// `Some((session_id, label))`. Otherwise returns `None` (ambiguous or no context).
///
/// This observes heartbeat cardinality; it does not establish caller identity.
/// A process has no way to prove a heartbeat is its own, so mutating CLI verbs
/// must use explicit `--session` or process-carried `EDDA_SESSION_ID` instead.
/// The environment value is attribution and a user override, not
/// authentication or authorization. This helper remains available only for
/// callers that need the observational "exactly one live heartbeat" fact
/// (GH-503).
pub fn infer_session_id(project_id: &str) -> Option<(String, String)> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let stale_threshold = stale_secs();
    let now = parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0);

    let entries = match fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return None,
    };

    let mut active: Vec<(String, String)> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }

        let content = match fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let hb: SessionHeartbeat = match serde_json::from_str(&content) {
            Ok(h) => h,
            Err(_) => continue,
        };

        let hb_epoch = parse_rfc3339_to_epoch(&hb.last_heartbeat).unwrap_or(0);
        let age = now.saturating_sub(hb_epoch);

        if age <= stale_threshold {
            active.push((hb.session_id, hb.label));
        }
    }

    if active.len() == 1 {
        Some(active.remove(0))
    } else {
        None
    }
}
