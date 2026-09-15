//! Thread-scoped environment seam (GH-1126).
//!
//! Production behaviour is a plain `std::env::var`. In test builds a
//! thread-local override map installed by [`test_config_guard`] takes
//! precedence, so a test that asserts a variable is absent (or present) pins
//! it itself instead of inheriting it from the shell that launched
//! `cargo test`.
//!
//! This mirrors the GH-757 seam in `edda-bridge-claude` (`crate::env_var`).
//! Fleet lane wrappers export `EDDA_MACHINE` (GH-671 / PR #1120), which is
//! what made the claim-guard test's outcome a property of the launching shell
//! rather than its fixture.

/// Read a UTF-8 env var through the thread-scoped test configuration.
pub(crate) fn var(name: &str) -> Option<String> {
    #[cfg(test)]
    match test_config::lookup(name) {
        test_config::Entry::Value(v) => return Some(v),
        test_config::Entry::Masked => return None,
        test_config::Entry::Absent => {}
    }
    std::env::var(name).ok()
}

/// Install thread-scoped env overrides for the returned guard's lifetime.
#[cfg(test)]
pub(crate) fn test_config_guard(vars: &[(&str, Option<&str>)]) -> test_config::Guard {
    test_config::set(vars)
}

/// Thread-scoped test configuration (GH-1126, mirroring GH-757): overrides
/// live in a thread-local map, so one test's pin is invisible to every other
/// thread and the process environment is never mutated. Readers and writers
/// need no lock and cannot race; the guard restores on drop, including on
/// panic.
#[cfg(test)]
pub(crate) mod test_config {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static OVERRIDES: RefCell<HashMap<String, Option<String>>> =
            RefCell::new(HashMap::new());
    }

    /// What [`super::var`] should see for one var on this thread.
    pub(crate) enum Entry {
        /// No override on this thread: pass through to the real environment.
        Absent,
        /// The test masked the var: readers see it as unset even if the host
        /// process has it set.
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
