pub mod fleet;
pub mod heartbeat;
pub mod registry;
pub mod skill_registry;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod user_config;

pub use heartbeat::{
    heartbeat_path, read_heartbeat, update_heartbeat, write_heartbeat, SessionHeartbeat,
    TaskSnapshot,
};

use fs2::FileExt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Compute a deterministic project ID from a repo root or cwd path.
/// project_id = blake3(normalize_path(input)) → hex string (first 32 chars).
///
/// If `repo_root_or_cwd` is inside a git worktree, resolves to the main
/// repository root so that all worktrees share the same project ID.
pub fn project_id(repo_root_or_cwd: &Path) -> String {
    let resolved = edda_core::git::resolve_git_root(repo_root_or_cwd)
        .unwrap_or_else(|| repo_root_or_cwd.to_path_buf());
    project_id_for_root(&resolved)
}

/// Compute a deterministic project ID from an already-resolved authoritative root.
pub fn project_id_for_root(root: &Path) -> String {
    let normalized = normalize_path(root);
    let hash = blake3::hash(normalized.as_bytes());
    hash.to_hex()[..32].to_string()
}

/// Normalize a path: canonicalize, lowercase on Windows, forward slashes.
fn normalize_path(p: &Path) -> String {
    let abs = p
        .canonicalize()
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .to_string();
    // Lowercase on Windows for consistency
    #[cfg(windows)]
    let abs = abs.to_lowercase();
    // Normalize path separators to forward slashes
    abs.replace('\\', "/")
}

/// Return the per-user store root: `~/.edda/`
/// Windows: `%APPDATA%\edda\` (falls back to `%USERPROFILE%\.edda\`)
///
/// Override with `EDDA_STORE_ROOT` env var (useful for testing).
///
/// In test-support builds, a thread-local test override installed by
/// `test_support` takes precedence over the env var, so a test's private
/// store root is invisible to every other thread in the process (GH-757).
/// In production builds the override is always `None` and this function
/// behaves exactly as before.
pub fn store_root() -> PathBuf {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(root) = test_support::current_override() {
        return root;
    }
    if let Ok(custom) = std::env::var("EDDA_STORE_ROOT") {
        return PathBuf::from(custom);
    }
    if let Some(data_dir) = dirs::data_dir() {
        data_dir.join("edda")
    } else if let Some(home) = edda_core::paths::home_dir() {
        home.join(".edda")
    } else {
        PathBuf::from(".edda-store")
    }
}

/// Return the project directory: `store_root/projects/<project_id>/`
pub fn project_dir(project_id: &str) -> PathBuf {
    store_root().join("projects").join(project_id)
}

/// Ensure all subdirectories exist for a project.
pub fn ensure_dirs(project_id: &str) -> anyhow::Result<()> {
    let base = project_dir(project_id);
    let subdirs = ["ledger", "transcripts", "index", "packs", "state", "search"];
    for sub in &subdirs {
        fs::create_dir_all(base.join(sub))?;
    }
    Ok(())
}

/// Atomic write: write to temp file in same dir, then rename.
pub fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no parent dir for {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(data)?;
    tmp.flush()?;
    tmp.persist(path)?;
    Ok(())
}

/// File-based exclusive lock guard.
pub struct LockGuard {
    _file: fs::File,
}

/// Acquire an exclusive file lock. Creates the lock file if needed.
pub fn lock_file(path: &Path) -> anyhow::Result<LockGuard> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.lock_exclusive()?;
    Ok(LockGuard { _file: file })
}

/// Try to acquire an exclusive file lock without blocking.
/// Returns `Ok(Some(LockGuard))` if acquired, `Ok(None)` if contended.
pub fn try_lock_file(path: &Path) -> anyhow::Result<Option<LockGuard>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(LockGuard { _file: file })),
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.raw_os_error() == Some(33)
                || e.raw_os_error() == Some(32) =>
        {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// Capture the test-scoped store-root override installed on the current
/// thread, so a spawned thread can reinstall it with
/// [`install_captured_store_root`] (GH-757).
///
/// Background threads spawned by production code would otherwise resolve the
/// ordinary env/default root while their spawning test holds a private root.
/// In production builds no override can be installed, so this always returns
/// `None` and the spawned-thread scope is a no-op.
pub fn captured_store_root_for_spawn() -> Option<PathBuf> {
    #[cfg(any(test, feature = "test-support"))]
    let captured = test_support::current_override();
    #[cfg(not(any(test, feature = "test-support")))]
    let captured: Option<PathBuf> = None;
    captured
}

/// Scope guard reinstalling a captured test store root on the current thread.
/// No-op in production builds.
pub struct SpawnedThreadStoreRoot(
    #[cfg(any(test, feature = "test-support"))] Option<test_support::ThreadOverrideGuard>,
);

impl Drop for SpawnedThreadStoreRoot {
    fn drop(&mut self) {
        #[cfg(any(test, feature = "test-support"))]
        {
            // Restores the prior thread override, including on panic.
            drop(self.0.take());
        }
    }
}

/// Reinstall a captured store-root override ([`captured_store_root_for_spawn`])
/// on the current thread for the lifetime of the returned guard.
///
/// Call this at the top of a spawned background thread; it keeps the thread's
/// store resolution identical to the spawning test's. No-op in production
/// builds.
pub fn install_captured_store_root(captured: Option<&Path>) -> SpawnedThreadStoreRoot {
    #[cfg(any(test, feature = "test-support"))]
    let scope = SpawnedThreadStoreRoot(captured.map(test_support::set_thread_override));
    #[cfg(not(any(test, feature = "test-support")))]
    let scope = {
        let _ = captured;
        SpawnedThreadStoreRoot()
    };
    scope
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_id_is_deterministic() {
        let id1 = project_id(Path::new("/tmp/test-repo"));
        let id2 = project_id(Path::new("/tmp/test-repo"));
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 32);
        assert!(id1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn project_id_for_root_does_not_follow_an_unrelated_ancestor_git_root() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("parent");
        let nested = parent.join("nested-edda");
        fs::create_dir_all(parent.join(".git")).unwrap();
        fs::create_dir_all(&nested).unwrap();

        let nested_id = project_id_for_root(&nested);
        assert_eq!(nested_id, project_id_for_root(&nested));
        assert_ne!(nested_id, project_id(&nested));
        assert_eq!(project_id(&nested), project_id(&parent));
    }

    #[test]
    fn store_root_is_not_empty() {
        let root = store_root();
        assert!(!root.as_os_str().is_empty());
    }

    #[test]
    fn ensure_dirs_creates_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        // Override store root by using project_dir directly
        let base = tmp.path().join("projects").join("test_proj");
        let subdirs = ["ledger", "transcripts", "index", "packs", "state", "search"];
        for sub in &subdirs {
            fs::create_dir_all(base.join(sub)).unwrap();
        }
        for sub in &subdirs {
            assert!(base.join(sub).is_dir());
        }
    }

    #[test]
    fn write_atomic_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.txt");
        write_atomic(&path, b"hello world").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[test]
    fn lock_file_acquires_and_drops() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_path = tmp.path().join("test.lock");
        let guard = lock_file(&lock_path).unwrap();
        assert!(lock_path.exists());
        drop(guard);
    }

    #[test]
    fn worktree_and_main_produce_same_project_id() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".git").join("worktrees").join("feat-x")).unwrap();

        let wt = repo.join(".claude").join("worktrees").join("feat-x");
        fs::create_dir_all(&wt).unwrap();
        let gitdir = repo.join(".git").join("worktrees").join("feat-x");
        let gitdir_str = gitdir.to_string_lossy().replace('\\', "/");
        fs::write(wt.join(".git"), format!("gitdir: {gitdir_str}")).unwrap();

        let id_main = project_id(&repo);
        let id_wt = project_id(&wt);
        assert_eq!(
            id_main, id_wt,
            "worktree and main tree must have same project_id"
        );
    }
}
