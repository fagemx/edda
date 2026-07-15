//! Test-only helpers shared by the `cmd_*` test modules.

use std::sync::{Mutex, MutexGuard};

/// Serialize tests that redirect `EDDA_STORE_ROOT`.
///
/// The store root is process-wide, and cargo runs a crate's tests in parallel
/// threads, so two tests redirecting it at once would each see the other's root.
static ENV_STORE_LOCK: Mutex<()> = Mutex::new(());

/// Holds the redirect open for the duration of a test, and — importantly —
/// takes it back down on drop, so a panicking test cannot leave
/// `EDDA_STORE_ROOT` pointing at a `TempDir` that is about to be deleted.
pub(crate) struct IsolatedStore {
    _dir: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
}

impl Drop for IsolatedStore {
    fn drop(&mut self) {
        std::env::remove_var("EDDA_STORE_ROOT");
    }
}

/// Point the per-user store at a throwaway directory for this test.
///
/// Anything that writes to the store — `edda init` and `edda group` both call
/// `registry::register_project` — must be wrapped in this, or it writes into the
/// developer's real `registry.json` and stays there (GH-417). CI never notices,
/// because its containers start empty; only the developer's machine accumulates.
///
/// Keep the returned value alive for the whole test:
///
/// ```ignore
/// let _store = test_support::isolated_store();
/// ```
pub(crate) fn isolated_store() -> IsolatedStore {
    // A test that panics while holding the lock poisons it. That is the failure
    // of one test, and must not cascade into every later one, so the poison is
    // taken rather than unwrapped.
    let guard = ENV_STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir for isolated store");
    std::env::set_var("EDDA_STORE_ROOT", dir.path());
    IsolatedStore {
        _dir: dir,
        _guard: guard,
    }
}
