use std::fs;

use crate::parse::now_rfc3339;

use super::board::compute_board_state;
use super::helpers::parse_rfc3339_to_epoch;
use super::{stale_secs, PeerSummary, SessionHeartbeat};

// ── Peer Discovery ──

/// Discover active peer sessions (excluding current session and stale ones).
pub fn discover_active_peers(project_id: &str, current_session_id: &str) -> Vec<PeerSummary> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let stale_threshold = stale_secs();
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

        let hb_epoch = parse_rfc3339_to_epoch(&hb.last_heartbeat).unwrap_or(0);
        let age = now.saturating_sub(hb_epoch);

        // Sub-agents can't touch_heartbeat (no hook events fire during execution),
        // so use a much longer stale threshold (15x = ~30min at default 120s).
        let effective_threshold = if hb.parent_session_id.is_some() {
            stale_threshold * 15
        } else {
            stale_threshold
        };
        if age > effective_threshold {
            continue;
        }

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

/// Live sessions for a project, as `(session_id, label, branch)`.
fn active_sessions(project_id: &str) -> Vec<(String, String, Option<String>)> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let stale_threshold = stale_secs();
    let now = parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0);

    let entries = match fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut active = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }
        let Ok(content) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(hb) = serde_json::from_str::<SessionHeartbeat>(&content) else {
            continue;
        };
        let hb_epoch = parse_rfc3339_to_epoch(&hb.last_heartbeat).unwrap_or(0);
        if now.saturating_sub(hb_epoch) <= stale_threshold {
            active.push((hb.session_id, hb.label, hb.branch));
        }
    }
    active
}

/// Identify the caller's own session by the branch it is working on.
///
/// `infer_session_id` can only answer when exactly one session is live, which
/// is precisely when identity matters least. In a fleet several are live, it
/// gives up, and the caller falls back to a synthetic `cli-<label>` id — or,
/// worse for a bare shell standing next to one live peer, adopts that peer's
/// identity (GH-488).
///
/// A branch is the one thing a CLI process can compare against the board:
/// each session owns a worktree and therefore a branch. Matching on it is
/// strictly narrower than "the only session", so it can remove a
/// misattribution but never create one — and it answers in the fleet case the
/// old inference could not.
///
/// Returns `None` when no live session claims this branch, or when more than
/// one does; both mean the caller cannot be told apart from a peer.
pub fn infer_session_id_on_branch(project_id: &str, branch: &str) -> Option<(String, String)> {
    if branch.is_empty() {
        return None;
    }
    let mut matches = active_sessions(project_id)
        .into_iter()
        .filter(|(_, _, b)| b.as_deref() == Some(branch));
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some((first.0, first.1))
}

/// Infer the current session from heartbeat files.
///
/// If exactly one non-stale session exists for the project, returns
/// `Some((session_id, label))`. Otherwise returns `None` (ambiguous or no context).
///
/// Used by CLI commands (`edda decide`, etc.) to resolve session identity
/// when `EDDA_SESSION_ID` env var is not set. Prefer
/// [`infer_session_id_on_branch`] where the caller knows its branch: this one
/// cannot tell the caller apart from a lone peer.
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
