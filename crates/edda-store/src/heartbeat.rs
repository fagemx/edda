//! Shared session heartbeat surface (GH-569).
//!
//! The heartbeat file (`~/.edda/projects/<pid>/state/session.<sid>.json`) is
//! THE one liveness surface consumed by peer discovery. It used to live in
//! `edda-bridge-claude`, which made the Claude hook path the only production
//! writer: lanes launched by `edda dispatch --agent pi|codex` fire no hooks,
//! wrote no heartbeat, and could never appear in `edda peers` (GH-569).
//!
//! The type and its writers now live here so any crate that can depend on
//! `edda-store` — the conductor runner, any bridge — can produce heartbeats.
//! `edda-bridge-claude` remains *a* producer (the hook path enriches the
//! heartbeat with transcript signals); it is no longer the only one.
//!
//! One surface, one format: do not add a parallel liveness file.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A snapshot of one task the session is working on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: String,
    pub subject: String,
    pub status: String,
}

/// Per-session heartbeat file.
/// Location: `~/.edda/projects/{pid}/state/session.{sid}.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeartbeat {
    pub session_id: String,
    pub started_at: String,
    pub last_heartbeat: String,
    pub label: String,
    pub focus_files: Vec<String>,
    pub active_tasks: Vec<TaskSnapshot>,
    pub files_modified_count: usize,
    pub total_edits: usize,
    pub recent_commits: Vec<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_phase: Option<String>,
    /// Set for sub-agent heartbeats to link back to the parent session.
    /// Used for orphan cleanup and extended stale threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    // ── Conductor lane fields (GH-566) — additive, old readers ignore them ──
    /// Plan name the lane is running (`edda conduct` / `edda dispatch`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Phase id within the plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Attempt number of the running phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// What the lane is doing right now (running / awaiting_verdict / ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// OS pid of the process writing the heartbeat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Path of one session's heartbeat file under the project state dir.
pub fn heartbeat_path(project_id: &str, session_id: &str) -> PathBuf {
    crate::project_dir(project_id)
        .join("state")
        .join(format!("session.{session_id}.json"))
}

/// Read one session's heartbeat file, if present and well-formed.
pub fn read_heartbeat(project_id: &str, session_id: &str) -> Option<SessionHeartbeat> {
    let content = std::fs::read_to_string(heartbeat_path(project_id, session_id)).ok()?;
    serde_json::from_str(&content).ok()
}

/// Atomically write a session heartbeat (temp + rename, bounded single file —
/// never an append log). Callers own error policy: the observation plane must
/// not be able to fail the work plane, so treat `Err` as a warning upstream.
pub fn write_heartbeat(project_id: &str, hb: &SessionHeartbeat) -> anyhow::Result<()> {
    let data = serde_json::to_string_pretty(hb)?;
    crate::write_atomic(&heartbeat_path(project_id, &hb.session_id), data.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(sid: &str) -> SessionHeartbeat {
        SessionHeartbeat {
            session_id: sid.into(),
            started_at: "2026-09-01T00:00:00Z".into(),
            last_heartbeat: "2026-09-01T00:00:30Z".into(),
            label: "a".into(),
            focus_files: vec![],
            active_tasks: vec![],
            files_modified_count: 0,
            total_edits: 0,
            recent_commits: vec![],
            branch: None,
            current_phase: Some("running".into()),
            parent_session_id: None,
            plan: Some("p".into()),
            phase: Some("a".into()),
            attempt: Some(1),
            stage: Some("running".into()),
            pid: Some(42),
        }
    }

    #[test]
    fn heartbeat_roundtrips_through_the_shared_writer() {
        let _lock = crate::ENV_STORE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("EDDA_STORE_ROOT");
        std::env::set_var("EDDA_STORE_ROOT", tmp.path());
        let pid = "test_store_hb_roundtrip";
        write_heartbeat(pid, &sample("s1")).expect("write");
        let hb = read_heartbeat(pid, "s1").expect("read");
        assert_eq!(hb.plan.as_deref(), Some("p"));
        assert_eq!(hb.attempt, Some(1));
        assert_eq!(hb.pid, Some(42));
        assert!(heartbeat_path(pid, "s1").exists());
        match previous {
            Some(v) => std::env::set_var("EDDA_STORE_ROOT", v),
            None => std::env::remove_var("EDDA_STORE_ROOT"),
        }
        let _ = std::fs::remove_dir_all(tmp.path());
    }
}
