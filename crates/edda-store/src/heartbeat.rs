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

/// Sanitize a session id for safe interpolation into a filesystem path.
///
/// Every heartbeat reader/writer funnels through `heartbeat_path`, so
/// encoding here (rather than rejecting at individual CLI entry points)
/// cannot be bypassed by a second caller, keeps the IO surface infallible,
/// and defuses separators, `..`, absolute/drive-letter prefixes, control
/// bytes and reserved Windows device names in one move: the result is
/// always the single filename `session.<encoded>.json`, and a reserved
/// device name only matches as a whole filename (`session.con.json` is
/// not reserved). Chosen over rejecting: a hostile id then cannot fail a
/// phase at an observation-plane trust boundary, and read/write stay
/// consistent because both sides sanitize identically.
fn sanitize_session_id(session_id: &str) -> String {
    let mut out = String::with_capacity(session_id.len());
    for b in session_id.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'_' | b'-' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Path of one session's heartbeat file under the project state dir.
/// The session id is sanitized, so the result always stays inside the
/// project state directory regardless of what the caller passes.
pub fn heartbeat_path(project_id: &str, session_id: &str) -> PathBuf {
    crate::project_dir(project_id)
        .join("state")
        .join(format!("session.{}.json", sanitize_session_id(session_id)))
}

/// Sidecar lock guarding read-modify-write updates of one session's
/// heartbeat file. Derived from the (sanitized) heartbeat path, so it stays
/// inside the state dir too; the `.json.lock` suffix keeps discovery's
/// `session.*.json` enumeration from ever seeing it.
fn heartbeat_lock_path(project_id: &str, session_id: &str) -> PathBuf {
    let mut p = heartbeat_path(project_id, session_id).into_os_string();
    p.push(".lock");
    PathBuf::from(p)
}

impl SessionHeartbeat {
    /// A blank record for `update_heartbeat` to seed when no file exists yet.
    /// `started_at`/`last_heartbeat` stay empty until the producing writer
    /// (which owns the clock) stamps them; `update_heartbeat` never persists
    /// a still-blank record, so a "touch" on a nonexistent session creates
    /// no file.
    pub fn blank(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            started_at: String::new(),
            last_heartbeat: String::new(),
            label: String::new(),
            focus_files: Vec::new(),
            active_tasks: Vec::new(),
            files_modified_count: 0,
            total_edits: 0,
            recent_commits: Vec::new(),
            branch: None,
            current_phase: None,
            parent_session_id: None,
            plan: None,
            phase: None,
            attempt: None,
            stage: None,
            pid: None,
        }
    }

    fn is_blank(&self) -> bool {
        self.started_at.is_empty() && self.last_heartbeat.is_empty()
    }
}

/// Read-modify-write one session's heartbeat under an exclusive lock.
///
/// Multiple producers share one file/format (the Claude hook path, the
/// conductor runner, external bridges). A refresh must update only the
/// fields its producer owns and preserve everything else — reconstructing
/// the whole record races and destroys the other producer's data. This
/// serializes read-modify-write cycles between cooperating writers via a
/// sidecar lock file; `mutate` receives the existing record, or a blank one
/// (see [`SessionHeartbeat::blank`]) when no file exists yet. A still-blank
/// record is not written, so no-op touches create nothing.
pub fn update_heartbeat<F>(project_id: &str, session_id: &str, mutate: F) -> anyhow::Result<()>
where
    F: FnOnce(&mut SessionHeartbeat),
{
    let _guard = crate::lock_file(&heartbeat_lock_path(project_id, session_id))?;
    let mut hb = read_heartbeat(project_id, session_id)
        .unwrap_or_else(|| SessionHeartbeat::blank(session_id));
    mutate(&mut hb);
    hb.session_id = session_id.to_string();
    if hb.is_blank() {
        return Ok(());
    }
    write_heartbeat(project_id, &hb)
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

    /// P0 regression (review round 1): a user-controlled `--session-id` must
    /// never escape the project state directory through `heartbeat_path` —
    /// this is the single funnel every heartbeat reader/writer passes through.
    /// Covers the exact reviewed reproduction plus absolute-path and
    /// drive-letter forms, on both separator styles.
    #[test]
    fn heartbeat_path_confines_a_hostile_session_id_to_the_state_dir() {
        // Serialize with the other heartbeat tests: they mutate
        // EDDA_STORE_ROOT, and this test compares paths against it.
        let _lock = crate::ENV_STORE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let state_dir = heartbeat_path("proj", "innocent")
            .parent()
            .unwrap()
            .to_path_buf();
        let hostile = [
            "x\\..\\..\\..\\escaped", // the reviewed reproduction
            "x/../../../escaped",     // same shape, forward slashes
            "..",                     // bare dot-dot
            "..\\..\\evil",           // pure dot-dot escape
            "C:\\evil\\x",            // drive-letter absolute
            "C:/evil",                // drive letter, forward slashes
            "/abs/evil",              // Unix-style absolute
            "a:b",                    // NTFS alternate-data-stream separator
            "con",                    // reserved Windows device name
        ];
        for sid in hostile {
            let p = heartbeat_path("proj", sid);
            assert_eq!(
                p.parent(),
                Some(state_dir.as_path()),
                "session id {sid:?} escaped the state dir: {}",
                p.display()
            );
        }
    }

    /// P0 regression: the shared writer/reader must both contain a hostile
    /// session id — the file lands (encoded) inside the state dir, nothing
    /// named `escaped.json` appears outside it, and the record round-trips.
    #[test]
    fn hostile_session_id_round_trips_without_leaving_the_state_dir() {
        let _lock = crate::ENV_STORE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("EDDA_STORE_ROOT");
        std::env::set_var("EDDA_STORE_ROOT", tmp.path());
        let sid = "x\\..\\..\\..\\escaped";
        write_heartbeat("proj", &sample(sid)).expect("write");
        let project = crate::project_dir("proj");
        let state = project.join("state");
        let session_files = std::fs::read_dir(&state)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("session."))
            .count();
        assert_eq!(session_files, 1, "exactly one bounded session file");
        // Nothing escaped to the project dir, the projects/ level or the
        // store root (the reviewed repro wrote `store/projects/escaped.json`).
        assert!(!project.join("escaped.json").exists());
        assert!(!state.join("escaped.json").exists());
        if let Some(projects) = project.parent() {
            assert!(!projects.join("escaped.json").exists());
            if let Some(root) = projects.parent() {
                assert!(!root.join("escaped.json").exists());
            }
        }
        assert!(
            read_heartbeat("proj", sid).is_some(),
            "heartbeat round-trips through the sanitized path"
        );
        match previous {
            Some(v) => std::env::set_var("EDDA_STORE_ROOT", v),
            None => std::env::remove_var("EDDA_STORE_ROOT"),
        }
        let _ = std::fs::remove_dir_all(tmp.path());
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

#[cfg(test)]
mod scratch_tests {
    use super::*;

    #[test]
    fn scratch_update_heartbeat_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("EDDA_STORE_ROOT");
        std::env::set_var("EDDA_STORE_ROOT", tmp.path());
        let r = update_heartbeat("proj", "s1", |hb| {
            eprintln!("DEBUG mutate: started_at={:?} last={:?}", hb.started_at, hb.last_heartbeat);
            if hb.started_at.is_empty() {
                hb.started_at = "T0".into();
            }
            hb.last_heartbeat = "T1".into();
            hb.plan = Some("p".into());
        });
        eprintln!("DEBUG update result: {r:?}");
        let p = heartbeat_path("proj", "s1");
        eprintln!("DEBUG path {} exists={}", p.display(), p.exists());
        match previous {
            Some(v) => std::env::set_var("EDDA_STORE_ROOT", v),
            None => std::env::remove_var("EDDA_STORE_ROOT"),
        }
        assert!(p.exists());
    }
}
