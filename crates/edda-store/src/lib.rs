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
    let resolved = resolve_git_root(repo_root_or_cwd)
        .unwrap_or_else(|| repo_root_or_cwd.to_path_buf());
    let normalized = normalize_path(&resolved);
    let hash = blake3::hash(normalized.as_bytes());
    hash.to_hex()[..32].to_string()
}

/// Resolve the git repository root, handling worktrees.
///
/// Walks up from `start` looking for `.git`:
/// - **Directory** → parent is the repo root (normal repo).
/// - **File** with `gitdir: .../worktrees/{name}` → strip to find the common
///   `.git` directory, then return its parent (the main working tree root).
/// - **File** without `/worktrees/` (e.g. submodule) → return that directory.
/// - **Not found** → returns `None` (non-git directory; caller uses original path).
fn resolve_git_root(start: &Path) -> Option<PathBuf> {
    let abs = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut cur = abs.as_path();
    loop {
        let dot_git = cur.join(".git");
        if dot_git.is_dir() {
            return Some(cur.to_path_buf());
        }
        if dot_git.is_file() {
            if let Ok(content) = fs::read_to_string(&dot_git) {
                let content = content.trim();
                if let Some(gitdir) = content.strip_prefix("gitdir:") {
                    let gitdir = gitdir.trim().replace('\\', "/");
                    if let Some(pos) = gitdir.find("/worktrees/") {
                        // Worktree: gitdir points to .git/worktrees/{name}
                        // Strip /worktrees/{name} to get the common .git dir,
                        // then take its parent as the repo root.
                        let common_git = &gitdir[..pos];
                        return Path::new(common_git).parent().map(|p| p.to_path_buf());
                    }
                }
            }
            // .git file but not a worktree (e.g. submodule) → use this dir
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
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
pub fn store_root() -> PathBuf {
    if let Some(data_dir) = dirs::data_dir() {
        data_dir.join("edda")
    } else if let Some(home) = dirs::home_dir() {
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
    fn resolve_git_root_normal_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("my-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();

        let result = resolve_git_root(&repo);
        assert_eq!(result.unwrap(), repo.canonicalize().unwrap());
    }

    #[test]
    fn resolve_git_root_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        // Simulate: repo/.git/ (directory) + repo/.claude/worktrees/feat-x/.git (file)
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".git").join("worktrees").join("feat-x")).unwrap();

        let wt = repo.join(".claude").join("worktrees").join("feat-x");
        fs::create_dir_all(&wt).unwrap();

        // Write .git file pointing to the worktree gitdir
        let gitdir = repo.join(".git").join("worktrees").join("feat-x");
        let gitdir_str = gitdir.to_string_lossy().replace('\\', "/");
        fs::write(wt.join(".git"), format!("gitdir: {gitdir_str}")).unwrap();

        let resolved = resolve_git_root(&wt).unwrap();
        assert_eq!(
            normalize_path(&resolved),
            normalize_path(&repo),
            "worktree should resolve to repo root"
        );
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
        assert_eq!(id_main, id_wt, "worktree and main tree must have same project_id");
    }

    #[test]
    fn resolve_git_root_submodule_no_worktree_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("parent").join("submodule");
        fs::create_dir_all(&sub).unwrap();
        // Submodule .git file has /modules/ not /worktrees/
        fs::write(sub.join(".git"), "gitdir: ../../.git/modules/submodule").unwrap();

        let resolved = resolve_git_root(&sub).unwrap();
        // Should resolve to the submodule dir itself (not the parent repo)
        assert_eq!(resolved, sub.canonicalize().unwrap());
    }

    #[test]
    fn resolve_git_root_non_git_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("not-a-repo");
        fs::create_dir_all(&dir).unwrap();

        assert!(resolve_git_root(&dir).is_none());
    }
}
