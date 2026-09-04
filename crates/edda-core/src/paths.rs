//! Home-anchored path resolution.
//!
//! Every home-anchored path in the workspace resolves here, so all of them
//! agree. `dirs::home_dir` is banned in `clippy.toml`; this module holds the
//! one sanctioned call.

use std::path::PathBuf;

/// The current user's home directory.
///
/// # Platform behaviour, and why this wrapper exists
///
/// On Windows this is **not** `%HOME%` and **not** `%USERPROFILE%`. It is a
/// shell known-folder lookup — `dirs` 6.0.0 `src/win.rs:5` calls
/// `dirs_sys::known_folder_profile()`, which is `dirs-sys` 0.5.0
/// `src/lib.rs:172` calling `known_folder(FOLDERID_Profile)`. Neither
/// environment variable is read.
///
/// That matters here for two reasons:
///
/// 1. `fleet.lane-launcher` requires the generated lane wrapper to set `HOME`
///    explicitly, because the Task Scheduler environment has none. Nothing
///    reached through this function will observe that `HOME`.
/// 2. Several callers locate *another tool's* configuration — `~/.codex`,
///    `~/.hermes`, `~/.claude`. A Node-based tool's `os.homedir()` reads
///    `USERPROFILE`, so if a profile is redirected the two disagree and edda
///    writes its hook config where that tool will not look for it.
///
/// This function deliberately preserves the existing behaviour rather than
/// switching to an environment-first lookup: changing it would move paths for
/// users who already have state on disk. It exists so that the behaviour is
/// stated once, in one place, instead of being rediscovered at ten call sites.
pub fn home_dir() -> Option<PathBuf> {
    // The one sanctioned call. See the module docs and clippy.toml.
    #[allow(clippy::disallowed_methods)]
    dirs::home_dir()
}
