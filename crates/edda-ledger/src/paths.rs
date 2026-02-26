use std::path::{Path, PathBuf};

/// All well-known paths under `.edda/`.
#[derive(Debug, Clone)]
pub struct EddaPaths {
    pub root: PathBuf,
    pub edda_dir: PathBuf,
    pub ledger_dir: PathBuf,
    pub ledger_db: PathBuf,
    pub blobs_dir: PathBuf,
    pub branches_dir: PathBuf,
    pub drafts_dir: PathBuf,
    pub lock_file: PathBuf,
    pub config_json: PathBuf,
    pub patterns_dir: PathBuf,
    pub blob_meta_json: PathBuf,
    pub tombstones_jsonl: PathBuf,
    pub archive_dir: PathBuf,
    pub archive_blobs_dir: PathBuf,
}

impl EddaPaths {
    /// Derive all paths from a repo root. Pure computation, no I/O.
    pub fn discover(repo_root: impl Into<PathBuf>) -> Self {
        let root = repo_root.into();
        let edda_dir = root.join(".edda");
        let ledger_dir = edda_dir.join("ledger");
        let archive_dir = edda_dir.join("archive");
        Self {
            ledger_db: edda_dir.join("ledger.db"),
            blobs_dir: ledger_dir.join("blobs"),
            blob_meta_json: ledger_dir.join("blob_meta.json"),
            tombstones_jsonl: ledger_dir.join("tombstones.jsonl"),
            branches_dir: edda_dir.join("branches"),
            drafts_dir: edda_dir.join("drafts"),
            lock_file: edda_dir.join("LOCK"),
            config_json: edda_dir.join("config.json"),
            patterns_dir: edda_dir.join("patterns"),
            archive_blobs_dir: archive_dir.join("blobs"),
            archive_dir,
            ledger_dir,
            edda_dir,
            root,
        }
    }

    /// Create all required directories. Idempotent.
    pub fn ensure_layout(&self) -> anyhow::Result<()> {
        for dir in [
            &self.ledger_dir,
            &self.blobs_dir,
            &self.branches_dir,
            &self.drafts_dir,
            &self.patterns_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Check whether `.edda/` exists.
    pub fn is_initialized(&self) -> bool {
        self.edda_dir.is_dir()
    }

    /// Resolve a branch directory under `.edda/branches/<name>/`.
    pub fn branch_dir(&self, name: &str) -> PathBuf {
        self.branches_dir.join(name)
    }
}

impl EddaPaths {
    /// Walk up from `start` looking for a directory containing `.edda/`.
    ///
    /// If the walk-up fails, falls back to git worktree resolution:
    /// reads the `.git` file to find the main repo root, then checks
    /// whether `.edda/` exists there.
    ///
    /// Returns `None` if not found by either method.
    pub fn find_root(start: &Path) -> Option<PathBuf> {
        // Phase 1: Walk up looking for .edda/ (fast path)
        let mut cur = start.to_path_buf();
        loop {
            if cur.join(".edda").is_dir() {
                return Some(cur);
            }
            if !cur.pop() {
                break;
            }
        }

        // Phase 2: Git worktree fallback — resolve to main repo, check .edda/ there
        resolve_git_repo_root(start).filter(|root| root.join(".edda").is_dir())
    }
}

/// Resolve git worktree to main repo root.
///
/// Walks up from `start` looking for `.git`:
/// - **Directory** → parent is the repo root (normal repo)
/// - **File** with `gitdir: .../worktrees/{name}` → resolve to main repo root
/// - **File** without `/worktrees/` (e.g. submodule) → use that directory
/// - **Not found** → `None`
fn resolve_git_repo_root(start: &Path) -> Option<PathBuf> {
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
    fn discover_builds_correct_paths() {
        let p = EddaPaths::discover("/tmp/repo");
        assert_eq!(p.edda_dir, PathBuf::from("/tmp/repo/.edda"));
        assert_eq!(p.blobs_dir, PathBuf::from("/tmp/repo/.edda/ledger/blobs"));
        assert_eq!(p.lock_file, PathBuf::from("/tmp/repo/.edda/LOCK"));
        assert_eq!(p.patterns_dir, PathBuf::from("/tmp/repo/.edda/patterns"));
        assert_eq!(
            p.blob_meta_json,
            PathBuf::from("/tmp/repo/.edda/ledger/blob_meta.json")
        );
        assert_eq!(
            p.tombstones_jsonl,
            PathBuf::from("/tmp/repo/.edda/ledger/tombstones.jsonl")
        );
        assert_eq!(p.archive_dir, PathBuf::from("/tmp/repo/.edda/archive"));
        assert_eq!(
            p.archive_blobs_dir,
            PathBuf::from("/tmp/repo/.edda/archive/blobs")
        );
    }

    #[test]
    fn ensure_layout_creates_dirs() {
        let tmp = std::env::temp_dir().join(format!("edda_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let p = EddaPaths::discover(&tmp);
        p.ensure_layout().unwrap();
        assert!(p.ledger_dir.is_dir());
        assert!(p.blobs_dir.is_dir());
        assert!(p.branches_dir.is_dir());
        assert!(p.drafts_dir.is_dir());
        assert!(p.patterns_dir.is_dir());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    use std::sync::atomic::{AtomicU64, Ordering};
    static PATH_TEST_CTR: AtomicU64 = AtomicU64::new(0);

    fn unique_tmp(label: &str) -> PathBuf {
        let n = PATH_TEST_CTR.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("edda_path_{label}_{}_{n}", std::process::id()))
    }

    #[test]
    fn find_root_walks_up_to_edda_dir() {
        let tmp = unique_tmp("walkup");
        let _ = std::fs::remove_dir_all(&tmp);
        // repo/.edda/ exists, start from repo/sub/deep/
        std::fs::create_dir_all(tmp.join(".edda")).unwrap();
        let deep = tmp.join("sub").join("deep");
        std::fs::create_dir_all(&deep).unwrap();

        let found = EddaPaths::find_root(&deep);
        assert_eq!(found.unwrap(), tmp);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_root_worktree_outside_repo() {
        // Simulate: main repo at repo/ with .edda/ and .git/
        // Worktree at wt/ with .git file pointing back
        let tmp = unique_tmp("wt_outside");
        let _ = std::fs::remove_dir_all(&tmp);
        let repo = tmp.join("repo");
        let wt = tmp.join("wt");

        // Main repo: .git/ directory + .edda/ workspace
        std::fs::create_dir_all(repo.join(".git").join("worktrees").join("feat-x")).unwrap();
        std::fs::create_dir_all(repo.join(".edda")).unwrap();

        // Worktree: .git file pointing to main repo's worktree gitdir
        std::fs::create_dir_all(&wt).unwrap();
        let gitdir = repo.join(".git").join("worktrees").join("feat-x");
        let gitdir_str = gitdir
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        std::fs::write(wt.join(".git"), format!("gitdir: {gitdir_str}")).unwrap();

        let found = EddaPaths::find_root(&wt);
        assert!(found.is_some(), "should resolve worktree to main repo");
        // Resolved root should contain .edda/
        assert!(found.unwrap().join(".edda").is_dir());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_root_non_git_no_edda_returns_none() {
        let tmp = unique_tmp("no_git");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        assert!(EddaPaths::find_root(&tmp).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_git_repo_root_normal_repo() {
        let tmp = unique_tmp("git_normal");
        let _ = std::fs::remove_dir_all(&tmp);
        let repo = tmp.join("my-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let result = resolve_git_repo_root(&repo);
        assert_eq!(result.unwrap(), repo.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_git_repo_root_worktree() {
        let tmp = unique_tmp("git_wt");
        let _ = std::fs::remove_dir_all(&tmp);
        let repo = tmp.join("repo");

        std::fs::create_dir_all(repo.join(".git").join("worktrees").join("feat-x")).unwrap();
        let wt = tmp.join("wt");
        std::fs::create_dir_all(&wt).unwrap();

        let gitdir = repo.join(".git").join("worktrees").join("feat-x");
        let gitdir_str = gitdir
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        std::fs::write(wt.join(".git"), format!("gitdir: {gitdir_str}")).unwrap();

        let resolved = resolve_git_repo_root(&wt).unwrap();
        let repo_canon = repo.canonicalize().unwrap();
        // Normalize for comparison
        let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
        assert_eq!(
            norm(&resolved),
            norm(&repo_canon),
            "worktree should resolve to main repo root"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_git_repo_root_submodule() {
        let tmp = unique_tmp("git_sub");
        let _ = std::fs::remove_dir_all(&tmp);
        let sub = tmp.join("parent").join("submodule");
        std::fs::create_dir_all(&sub).unwrap();
        // Submodule .git file has /modules/ not /worktrees/
        std::fs::write(sub.join(".git"), "gitdir: ../../.git/modules/submodule").unwrap();

        let resolved = resolve_git_repo_root(&sub).unwrap();
        assert_eq!(resolved, sub.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_git_repo_root_non_git() {
        let tmp = unique_tmp("git_none");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        assert!(resolve_git_repo_root(&tmp).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
