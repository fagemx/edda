pub mod agent_phase;
pub mod bg_detect;
pub mod bg_digest;
pub mod bg_extract;
pub mod bg_scan;
pub mod controls_suggest;
pub mod digest;
pub mod issue_proposal;
pub mod pattern;
pub mod peers;
pub mod redact;
pub mod render;
pub mod state;
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

/// Serialize tests that mutate env vars to avoid races.
/// Same pattern as edda-store's `ENV_STORE_LOCK`.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run a closure with env vars set, then clean up.
/// Acquires `ENV_LOCK` to prevent concurrent env var mutation.
#[cfg(test)]
pub(crate) fn with_env_guard(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
    let _guard = ENV_LOCK.lock().unwrap();
    for (key, val) in vars {
        match val {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
    f();
    for (key, _) in vars {
        std::env::remove_var(key);
    }
}
