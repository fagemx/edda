use std::path::{Path, PathBuf};

/// Resolve the git repository root, handling worktrees.
///
/// Walks up from `start` looking for `.git`:
/// - **Directory** → parent is the repo root (normal repo).
/// - **File** with `gitdir: .../worktrees/{name}` → strip to find the common
///   `.git` directory, then return its parent (the main working tree root).
/// - **File** without `/worktrees/` (e.g. submodule) → return that directory.
/// - **Not found** → returns `None` (non-git directory; caller uses original path).
pub fn resolve_git_root(start: &Path) -> Option<PathBuf> {
    let abs = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut cur = abs.as_path();
    loop {
        let dot_git = cur.join(".git");
        if dot_git.is_dir() {
            return Some(cur.to_path_buf());
        }
        if dot_git.is_file() {
            if let Ok(content) = std::fs::read_to_string(&dot_git) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_git_root_normal_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("my-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let result = resolve_git_root(&repo);
        assert_eq!(result.unwrap(), repo.canonicalize().unwrap());
    }

    #[test]
    fn resolve_git_root_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git").join("worktrees").join("feat-x")).unwrap();

        let wt = repo.join(".claude").join("worktrees").join("feat-x");
        std::fs::create_dir_all(&wt).unwrap();

        let gitdir = repo.join(".git").join("worktrees").join("feat-x");
        let gitdir_str = gitdir
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        std::fs::write(wt.join(".git"), format!("gitdir: {gitdir_str}")).unwrap();

        let resolved = resolve_git_root(&wt).unwrap();
        let repo_canon = repo.canonicalize().unwrap();
        let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
        assert_eq!(
            norm(&resolved),
            norm(&repo_canon),
            "worktree should resolve to repo root"
        );
    }

    #[test]
    fn resolve_git_root_submodule() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("parent").join("submodule");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(".git"), "gitdir: ../../.git/modules/submodule").unwrap();

        let resolved = resolve_git_root(&sub).unwrap();
        assert_eq!(resolved, sub.canonicalize().unwrap());
    }

    #[test]
    fn resolve_git_root_non_git_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("not-a-repo");
        std::fs::create_dir_all(&dir).unwrap();

        assert!(resolve_git_root(&dir).is_none());
    }
}
