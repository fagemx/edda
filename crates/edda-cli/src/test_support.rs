//! Test-only helpers shared by the `cmd_*` test modules.

use edda_store::test_support::IsolatedStoreRoot;

/// Point the per-user store at a throwaway directory for this test.
///
/// Anything that writes to the store — `edda init` and `edda group` both call
/// `registry::register_project` — must be wrapped in this, or it writes into the
/// developer's real `registry.json` and stays there (GH-417). CI never notices,
/// because its containers start empty; only the developer's machine accumulates.
///
/// GH-757: the override is a thread-local installed by `edda-store`'s
/// test-support API. `edda_store::store_root()` on this thread resolves into
/// the private directory; every other thread keeps its own resolution, and the
/// process environment is never mutated — so a panicking test cannot strand
/// its siblings on a directory that is about to be deleted, and a concurrently
/// running test cannot resolve into this test's root. Spawned subprocesses
/// inherit only the real environment: a child that must use an isolated store
/// is passed `EDDA_STORE_ROOT` explicitly via `Command::env`.
///
/// Keep the returned value alive for the whole test:
///
/// ```ignore
/// let _store = test_support::isolated_store();
/// ```
pub(crate) fn isolated_store() -> IsolatedStoreRoot {
    edda_store::test_support::isolated_store_root().expect("isolated store")
}

pub(crate) fn write_aged_heartbeat(
    project_id: &str,
    session_id: &str,
    age_secs: u64,
    parent_session_id: Option<&str>,
) {
    let _ = edda_store::ensure_dirs(project_id);
    let state_dir = edda_store::project_dir(project_id).join("state");
    std::fs::create_dir_all(&state_dir).expect("state dir");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs()
        .saturating_sub(age_secs);
    let ts = time::OffsetDateTime::from_unix_timestamp(now as i64)
        .expect("unix timestamp")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339");
    let mut heartbeat = serde_json::json!({
        "session_id": session_id,
        "started_at": ts,
        "last_heartbeat": ts,
        "label": session_id,
        "focus_files": [],
        "active_tasks": [],
        "files_modified_count": 0,
        "total_edits": 0,
        "recent_commits": [],
    });
    if let Some(parent) = parent_session_id {
        heartbeat["parent_session_id"] = serde_json::json!(parent);
    }
    std::fs::write(
        state_dir.join(format!("session.{session_id}.json")),
        heartbeat.to_string(),
    )
    .expect("heartbeat file");
}

// Guard-restore semantics (RAII on drop, panic safety, thread locality) are
// tested at the source in `edda-store::test_support`; a duplicate here could
// only re-prove them through an extra indirection.
