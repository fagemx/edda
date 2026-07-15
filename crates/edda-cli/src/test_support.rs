//! Test-only helpers shared by the `cmd_*` test modules.

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

/// Serialize tests that redirect `EDDA_STORE_ROOT`.
///
/// The store root is process-wide, and cargo runs a crate's tests in parallel
/// threads, so two tests redirecting it at once would each see the other's root.
static ENV_STORE_LOCK: Mutex<()> = Mutex::new(());

/// Holds the redirect open for the duration of a test, and — importantly —
/// puts back whatever was there on drop, so a panicking test cannot leave
/// `EDDA_STORE_ROOT` pointing at a `TempDir` that is about to be deleted.
pub(crate) struct IsolatedStore {
    /// What `EDDA_STORE_ROOT` was before we took it over, if anything.
    prev: Option<OsString>,
    _dir: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
}

impl Drop for IsolatedStore {
    fn drop(&mut self) {
        // Restore rather than remove. Someone who already knows about this
        // pollution may well run `EDDA_STORE_ROOT=/somewhere cargo test` to keep
        // the suite off their real store; removing the variable here would drop
        // every later test back onto the real one, writing exactly the entries
        // they redirected to avoid — and silently, since they believe they are
        // covered.
        match self.prev.take() {
            Some(prev) => std::env::set_var("EDDA_STORE_ROOT", prev),
            None => std::env::remove_var("EDDA_STORE_ROOT"),
        }
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
    let prev = std::env::var_os("EDDA_STORE_ROOT");
    let dir = tempfile::tempdir().expect("tempdir for isolated store");
    std::env::set_var("EDDA_STORE_ROOT", dir.path());
    IsolatedStore {
        prev,
        _dir: dir,
        _guard: guard,
    }
}

// No test for the restore path, deliberately.
//
// It was attempted and it is racy at every available seam. Any assertion has to
// compare against the process-wide `EDDA_STORE_ROOT`, and the only window in
// which that value is stable is while the lock is held — but acquiring it means
// calling `isolated_store()`, which takes the same non-reentrant lock, so a
// nested read deadlocks and an unnested one is a coin flip against whichever
// sibling test is mid-redirect. The first version of this test duly failed that
// way.
//
// A flaky test here would be worse than none: it would train the next person to
// re-run until green, which is the habit that lets real failures through.
