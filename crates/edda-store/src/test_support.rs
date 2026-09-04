//! Test-scoped store-root isolation (GH-757).
//!
//! `store_root()` resolves the per-user store root on every call. Tests that
//! redirect it must not do so through the process-global `EDDA_STORE_ROOT`
//! environment variable: cargo runs a crate's tests on parallel threads, and
//! every store-reading test in the process would resolve into whichever
//! sibling currently holds the redirect (and race its `TempDir` deletion).
//!
//! The mechanism here is a **thread-local override**: a test installs a root
//! on its own thread, and only that thread resolves it. It is compiled only
//! under the `test-support` feature (or when testing `edda-store` itself);
//! in production builds the override is always `None` and `store_root()`
//! keeps its ordinary env/default resolution.
//!
//! Guards are RAII: the override is restored on drop, including on panic, so
//! one failing test cannot leave state behind for its siblings.

use std::path::{Path, PathBuf};
use std::sync::Arc;

thread_local! {
    static THREAD_ROOT: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// The override installed on the current thread, if any.
pub(crate) fn current_override() -> Option<PathBuf> {
    THREAD_ROOT.with(|slot| slot.borrow().clone())
}

/// RAII: install `path` as the current thread's store-root override, and
/// restore whatever was installed before on drop (panic-safe).
pub struct ThreadOverrideGuard {
    prev: Option<PathBuf>,
}

impl ThreadOverrideGuard {
    fn install(path: &Path) -> Self {
        let prev = THREAD_ROOT.with(|slot| slot.borrow_mut().replace(path.to_path_buf()));
        ThreadOverrideGuard { prev }
    }
}

impl Drop for ThreadOverrideGuard {
    fn drop(&mut self) {
        THREAD_ROOT.with(|slot| *slot.borrow_mut() = self.prev.take());
    }
}

/// Owns the throwaway store directory for one test and installs it as the
/// current thread's store root.
///
/// The directory is deleted when the guard (or the last clone of its shared
/// handle) drops, after the override has been restored.
pub struct IsolatedStoreRoot {
    dir: Arc<tempfile::TempDir>,
    _guard: ThreadOverrideGuard,
}

impl IsolatedStoreRoot {
    fn new() -> Self {
        let dir = Arc::new(tempfile::tempdir().expect("tempdir for isolated store root"));
        let guard = ThreadOverrideGuard::install(dir.path());
        IsolatedStoreRoot { dir, _guard: guard }
    }

    /// The isolated store root this test resolves into.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Shared ownership of the directory, for propagating the root into a
    /// spawned thread: the directory outlives this guard if a thread still
    /// holds a clone. The thread installs the path with
    /// [`set_thread_override`] (or [`crate::install_captured_store_root`]).
    pub fn shared_dir(&self) -> Arc<tempfile::TempDir> {
        Arc::clone(&self.dir)
    }
}

/// Explicit handle: point the current thread's store resolver at `path` until
/// the returned guard drops. Use inside a spawned thread to reinstall a root
/// captured from the spawning test.
pub fn set_thread_override(path: &Path) -> ThreadOverrideGuard {
    ThreadOverrideGuard::install(path)
}

/// Create a fresh temp directory and make it the current thread's store root
/// for the lifetime of the returned guard.
///
/// ```ignore
/// let _store = edda_store::test_support::isolated_store_root();
/// // every edda_store::project_dir(...) call on this thread now resolves
/// // into the temp directory; other threads are unaffected.
/// ```
pub fn isolated_store_root() -> IsolatedStoreRoot {
    IsolatedStoreRoot::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_is_visible_through_store_root_and_restored_on_drop() {
        let before = current_override();
        let guard = isolated_store_root();
        assert_eq!(
            crate::store_root(),
            guard.path(),
            "store_root() must resolve the thread-local override while it is installed"
        );
        let path = guard.path().to_path_buf();
        drop(guard);
        assert_eq!(
            current_override(),
            before,
            "prior override must be restored"
        );
        assert_ne!(
            crate::store_root(),
            path,
            "store_root() must fall back once the guard is gone"
        );
    }

    /// Deterministic regression against a process-global redirect: with an
    /// `EDDA_STORE_ROOT`-style env redirect a spawned thread observes the
    /// installing test's root and races its deletion. The thread-local
    /// override must never leak across threads.
    #[test]
    fn override_never_leaks_to_spawned_threads() {
        let guard = isolated_store_root();
        let path = guard.path().to_path_buf();
        let seen = std::thread::spawn(move || (current_override(), crate::store_root()))
            .join()
            .unwrap();
        assert!(seen.0.is_none(), "override leaked to a spawned thread");
        assert_ne!(
            seen.1, path,
            "spawned thread resolved the test's private root"
        );
    }

    #[test]
    fn nested_overrides_restore_in_reverse_order() {
        let outer = isolated_store_root();
        let outer_path = outer.path().to_path_buf();
        let inner = set_thread_override(Path::new("C:/edda-test-nested-root"));
        assert_eq!(crate::store_root(), Path::new("C:/edda-test-nested-root"));
        drop(inner);
        assert_eq!(crate::store_root(), outer_path);
        drop(outer);
    }

    #[test]
    fn override_is_restored_when_the_test_panics_while_holding_it() {
        let before = current_override();
        let path = {
            let guard = isolated_store_root();
            let path = guard.path().to_path_buf();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _keep = guard;
                panic!("simulated failing test holding the guard");
            }));
            assert!(result.is_err());
            path
        };
        assert_eq!(
            current_override(),
            before,
            "panicking while holding the guard must still restore the prior override"
        );
        assert_ne!(crate::store_root(), path);
    }

    #[test]
    fn shared_dir_keeps_the_root_alive_after_the_guard_drops() {
        let guard = isolated_store_root();
        let dir = guard.shared_dir();
        let path = dir.path().to_path_buf();
        drop(guard);
        assert!(path.is_dir(), "shared handle must keep the directory alive");
    }
}
