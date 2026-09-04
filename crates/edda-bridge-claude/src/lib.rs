pub mod agent_phase;
pub mod bg_detect;
pub mod bg_digest;
pub mod bg_extract;
pub mod bg_index;
pub mod bg_scan;
pub mod controls_suggest;
pub mod digest;
pub mod issue_proposal;
pub mod pattern;
pub mod peers;
pub mod redact;
pub mod render;
pub mod state;
pub mod task_nudge;
pub mod watch;

mod admin;
pub(crate) mod decision_warning;
mod dispatch;
mod narrative;
pub mod nudge;
mod parse;
mod plan;
mod signals;

// Re-export public API (CLI consumers unchanged)
pub use admin::{doctor, install, uninstall};
pub use dispatch::{hook_entrypoint_from_stdin, HookResult};

/// Read an env var through the thread-scoped test configuration (GH-757).
///
/// This is the single production read path for every env var any test in this
/// crate mutates. Production behavior is a plain `std::env::var(...).ok()`.
/// In test builds, a thread-local override map installed by [`with_env_guard`]
/// takes precedence, so one test's mutations are invisible to every other
/// thread in the process and the process environment itself is never mutated
/// by a test. Readers and writers therefore need no lock and cannot race.
pub(crate) fn env_var(name: &str) -> Option<String> {
    #[cfg(test)]
    match test_config::lookup(name) {
        test_config::Entry::Value(v) => return Some(v),
        test_config::Entry::Masked => return None,
        test_config::Entry::Absent => {}
    }
    std::env::var(name).ok()
}

/// `OsString` variant of [`env_var`] for vars whose values may be non-UTF-8
/// paths. Same test-configuration passthrough.
pub(crate) fn env_var_os(name: &str) -> Option<std::ffi::OsString> {
    #[cfg(test)]
    match test_config::lookup(name) {
        test_config::Entry::Value(v) => return Some(v.into()),
        test_config::Entry::Masked => return None,
        test_config::Entry::Absent => {}
    }
    std::env::var_os(name)
}

/// Thread-scoped test configuration (GH-757).
///
/// Replaces process-global `EDDA_STORE_ROOT`/`EDDA_*` env mutation in tests:
/// `with_env_guard`/`test_config_guard` write into a thread-local map that
/// [`env_var`] consults first, so a mutation is scoped to the mutating test's
/// thread and restored on drop (panic-safe). No locks; no cross-thread races.
#[cfg(test)]
pub(crate) mod test_config {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static OVERRIDES: RefCell<HashMap<String, Option<String>>> =
            RefCell::new(HashMap::new());
    }

    /// What [`crate::env_var`] should see for one var on this thread.
    pub(crate) enum Entry {
        /// No override on this thread: pass through to the real environment.
        Absent,
        /// The test masked the var: readers must see it as unset even if the
        /// host process has it set (the old `remove_var` semantics).
        Masked,
        /// The test set the var to this value.
        Value(String),
    }

    pub(crate) fn lookup(name: &str) -> Entry {
        OVERRIDES.with(|m| match m.borrow().get(name) {
            None => Entry::Absent,
            Some(None) => Entry::Masked,
            Some(Some(v)) => Entry::Value(v.clone()),
        })
    }

    /// RAII handle restoring the previous override values on drop.
    pub(crate) struct Guard {
        prev: Vec<(String, Option<Option<String>>)>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            OVERRIDES.with(|m| {
                let mut m = m.borrow_mut();
                for (k, v) in self.prev.drain(..) {
                    match v {
                        Some(entry) => {
                            m.insert(k, entry);
                        }
                        None => {
                            m.remove(&k);
                        }
                    }
                }
            });
        }
    }

    pub(crate) fn set(vars: &[(&str, Option<&str>)]) -> Guard {
        let mut prev = Vec::with_capacity(vars.len());
        OVERRIDES.with(|m| {
            let mut m = m.borrow_mut();
            for (k, v) in vars {
                prev.push(((*k).to_string(), m.get(*k).cloned()));
                m.insert((*k).to_string(), v.map(|v| v.to_string()));
            }
        });
        Guard { prev }
    }
}

/// RAII variant of [`with_env_guard`] for tests that set vars inline instead
/// of wrapping the whole body in a closure.
#[cfg(test)]
pub(crate) fn test_config_guard(vars: &[(&str, Option<&str>)]) -> test_config::Guard {
    test_config::set(vars)
}

/// Run a closure with thread-scoped env-var overrides, then clean up.
///
/// Unlike the pre-GH-757 implementation this never touches the process
/// environment: the overrides live in a thread-local map read by [`env_var`],
/// so concurrent tests neither observe each other's values nor race a
/// restoration. Restores prior values on drop, including on panic.
#[cfg(test)]
pub(crate) fn with_env_guard(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
    let _guard = test_config::set(vars);
    f();
}

/// Point this test's store resolution at a fresh throwaway directory for the
/// lifetime of the returned guard (GH-757).
///
/// Every store-reading test fixture must hold one: the override is
/// thread-local, so `edda_store::store_root()` resolves into the test's
/// private directory on this thread while every other thread keeps its own
/// resolution. Without it, store writes land in the operator's real store.
/// Restores on drop, including on panic.
#[cfg(test)]
pub(crate) fn isolated_store() -> edda_store::test_support::IsolatedStoreRoot {
    edda_store::test_support::isolated_store_root()
}

#[cfg(test)]
mod store_isolation_regression {
    use super::isolated_store;

    /// Deterministic regression against the pre-GH-757 mechanism, which
    /// redirected the process-global `EDDA_STORE_ROOT`: a store-reading
    /// sibling thread observed whichever test currently held the redirect
    /// and raced its deletion. The thread-local override must be invisible
    /// to every other thread, and the installing thread's own writes must
    /// land inside its private root.
    #[test]
    fn isolated_store_is_invisible_to_concurrent_reader_threads() {
        let store = isolated_store();
        let root = edda_store::store_root();
        assert_eq!(
            root,
            store.path(),
            "the installing thread must resolve its private root"
        );

        // A concurrent reader that holds no guard resolves the ordinary
        // env/default root — never this test's private root.
        let seen = std::thread::spawn(edda_store::store_root).join().unwrap();
        assert_ne!(
            seen, root,
            "a guard-less sibling thread resolved this test's private root"
        );

        // Writes on the guarded thread land inside the private root.
        let project = edda_store::project_dir("test_gh757_isolation");
        assert!(project.starts_with(store.path()));
        edda_store::ensure_dirs("test_gh757_isolation").expect("dirs inside private root");
        assert!(project.join("state").is_dir());
    }
}
