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

    /// Resolve a validated branch directory under `.edda/branches/<name>/`.
    pub fn branch_dir(&self, name: &str) -> anyhow::Result<PathBuf> {
        validate_branch_name(name)?;
        let candidate = self.branches_dir.join(name);
        if candidate.strip_prefix(&self.branches_dir).is_err() {
            anyhow::bail!("invalid branch name: resolved path escapes branch root");
        }

        if self.branches_dir.exists() {
            let canonical_root = self.branches_dir.canonicalize()?;
            let mut existing = candidate.as_path();
            while !existing.exists() {
                existing = existing.parent().ok_or_else(|| {
                    anyhow::anyhow!("invalid branch name: no contained path ancestor")
                })?;
            }
            let canonical_existing = existing.canonicalize()?;
            if !canonical_existing.starts_with(&canonical_root) {
                anyhow::bail!("invalid branch name: resolved path escapes branch root");
            }
        }

        Ok(candidate)
    }
}

/// Validate a branch name before using it in an event or filesystem path.
///
/// Hierarchical names such as `feature/auth` are supported. Empty path
/// components, `.` and `..`, absolute paths, platform prefixes, and path
/// separators other than `/` are rejected.
pub fn validate_branch_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("invalid branch name: must be 1-64 characters");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
    {
        anyhow::bail!("invalid branch name: only [A-Za-z0-9._-/] allowed");
    }
    if name
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        anyhow::bail!("invalid branch name: path components must not be empty, '.' or '..'");
    }
    if Path::new(name).is_absolute()
        || !Path::new(name)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        anyhow::bail!("invalid branch name: absolute or prefixed paths are not allowed");
    }
    Ok(())
}

fn clean_unc_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        p.to_path_buf()
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
    ///
    /// Note: the walk has no upper bound — it can climb past the user's
    /// home directory to the drive root. Tests that need the walk confined
    /// to a directory they control must use [`EddaPaths::find_root_bounded`]
    /// instead (GH-646): an unbounded walk inherits whatever workspace the
    /// environment happens to have above the start path.
    pub fn find_root(start: &Path) -> Option<PathBuf> {
        Self::find_root_walk(start, None)
    }

    /// Test-facing bounded variant of [`EddaPaths::find_root`] (GH-646).
    ///
    /// Phase 1 climbs from `start` up to and including `ceiling` and never
    /// above it, so "there is / is not a workspace here" is a premise the
    /// caller establishes in its own tree instead of inheriting from the
    /// environment (e.g. the fleet coordination workspace in `$HOME`).
    /// `ceiling` must be `start` itself or an ancestor of it.
    ///
    /// Phase 2 (the git worktree fallback) is unchanged: it only fires when
    /// the caller's own fixture contains a git repository, so it resolves
    /// to a root the test created, never to an environment workspace.
    ///
    /// `pub` only because tests in other crates call it; it is not part of
    /// the crate's production surface. Production code resolves a workspace
    /// with [`EddaPaths::find_root`].
    #[doc(hidden)]
    pub fn find_root_bounded(start: &Path, ceiling: &Path) -> Option<PathBuf> {
        // Both sides of the bound check must be spelled the same way or the
        // bound is inert: `Path::starts_with` compares path *components*, and
        // on Windows `canonicalize` returns a `\\?\`-verbatim path whose
        // `Prefix::VerbatimDisk('C')` never equals a raw path's
        // `Prefix::Disk('C')`. A caller that canonicalized only `start` would
        // therefore leave the ceiling region on its first `pop()` and stop
        // before climbing anywhere. Normalising both sides here takes that
        // failure out of the caller's hands.
        // Canonicalize both or neither: a per-side fallback would reintroduce
        // the very mismatch this guards against whenever one path exists and
        // the other does not.
        let (clean_start, clean_ceiling) = match (start.canonicalize(), ceiling.canonicalize()) {
            (Ok(s), Ok(c)) => (clean_unc_path(&s), clean_unc_path(&c)),
            _ => (start.to_path_buf(), ceiling.to_path_buf()),
        };
        debug_assert!(
            clean_start.starts_with(&clean_ceiling),
            "start must be within ceiling: start={:?}, ceiling={:?}",
            clean_start,
            clean_ceiling
        );
        Self::find_root_walk(&clean_start, Some(&clean_ceiling))
    }

    fn find_root_walk(start: &Path, ceiling: Option<&Path>) -> Option<PathBuf> {
        let home = dirs::home_dir();

        // Normalisation belongs to `find_root_bounded`, not here: the
        // unbounded `find_root` is production API and this PR does not
        // change what it returns for a given `start`.
        //
        // Walk up looking for `.edda/` or `.git`.
        let mut cur = start.to_path_buf();
        loop {
            // Case 1: Check for .edda/ directory (nested or direct workspace).
            // Do NOT treat the user's home directory as a workspace root (its ~/.edda
            // is the global user state directory or fleet scratch space).
            let is_home = home.as_deref().is_some_and(|h| {
                h == cur.as_path()
                    || h.canonicalize().ok().as_deref() == cur.canonicalize().ok().as_deref()
            });

            if !is_home && cur.join(".edda").is_dir() {
                return Some(cur);
            }

            // Case 2: Git boundary.
            // If we encounter a `.git` file or directory, we have reached the root of
            // this git worktree or repository.
            let git_marker = cur.join(".git");
            if git_marker.is_file() {
                // This is a git worktree root (its .git is a file pointing to gitdir).
                // If the worktree root itself does not have .edda/, resolve to the main
                // repository and check if .edda/ exists there.
                // Do NOT continue walking up above this worktree (which could escape into ~/.edda).
                return edda_core::git::resolve_git_root(&cur).and_then(|main_repo| {
                    let cleaned = clean_unc_path(&main_repo);
                    if cleaned.join(".edda").is_dir() {
                        Some(cleaned)
                    } else {
                        None
                    }
                });
            } else if git_marker.is_dir() {
                // This is a main git repository root. Since it has no .edda/ (checked above),
                // do NOT escape above this git repository into parent directories (e.g. ~/.edda).
                return None;
            }

            if is_home || !cur.pop() {
                break;
            }
            if ceiling.is_some_and(|c| !cur.starts_with(c)) {
                // Left the ceiling region (the ceiling itself was just
                // checked on the previous iteration) — stop climbing.
                break;
            }
        }

        None
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
    fn branch_dir_accepts_hierarchical_branch_name() {
        let paths = EddaPaths::discover("/tmp/repo");

        let branch = paths.branch_dir("feature/auth").unwrap();

        assert_eq!(branch, paths.branches_dir.join("feature").join("auth"));
    }

    #[test]
    fn branch_dir_rejects_path_traversal_and_absolute_names() {
        let paths = EddaPaths::discover("/tmp/repo");

        for name in [
            "",
            ".",
            "..",
            "../escape",
            "feature/../escape",
            "feature//escape",
            "/absolute",
            r"C:\escape",
            r"..\escape",
            r"\\server\share",
        ] {
            assert!(
                paths.branch_dir(name).is_err(),
                "accepted unsafe name: {name}"
            );
        }
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

    /// GH-646 regression guard.
    ///
    /// The unbounded [`EddaPaths::find_root`] climbs to the drive root, so
    /// the premise "no `.edda/` anywhere above" can only be made true by
    /// the environment — and the fleet coordination workspace in `$HOME`
    /// makes it false (that is the non-hermeticity this issue fixes). A
    /// test that asserts "no workspace here" must therefore anchor the
    /// climb with [`EddaPaths::find_root_bounded`] and create that premise
    /// inside its own tree. The ceiling here is the test's own directory;
    /// a workspace placed ABOVE it simulates the environment and must stay
    /// invisible to the bounded walk.
    #[test]
    fn find_root_bounded_stops_at_ceiling_and_ignores_workspace_above_it() {
        let tmp = unique_tmp("bounded");
        let _ = std::fs::remove_dir_all(&tmp);
        let ceiling = tmp.join("ceiling");
        let leaf = ceiling.join("leaf");
        std::fs::create_dir_all(&leaf).unwrap();
        // Hostile environment: a workspace above the test's own region.
        std::fs::create_dir_all(tmp.join(".edda")).unwrap();

        // Nothing within [leaf..=ceiling] is a workspace, and the climb may
        // not leave the ceiling — so the environment workspace is invisible.
        assert!(EddaPaths::find_root_bounded(&leaf, &ceiling).is_none());

        // A workspace AT the ceiling is still found (the anchor is inclusive).
        //
        // Compare canonically on both sides: `find_root_bounded` normalises its
        // inputs, so it returns a canonical path, and on macOS the temp dir is
        // reached through a symlink (`/var/...` -> `/private/var/...`) that
        // makes the canonical and raw spellings of the same directory differ.
        std::fs::create_dir_all(ceiling.join(".edda")).unwrap();
        let found =
            EddaPaths::find_root_bounded(&leaf, &ceiling).expect("workspace at the ceiling");
        assert_eq!(
            clean_unc_path(&found.canonicalize().unwrap()),
            clean_unc_path(&ceiling.canonicalize().unwrap())
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_root_bounded_handles_canonical_start_with_raw_ceiling() {
        let tmp = unique_tmp("bounded_canonical");
        let _ = std::fs::remove_dir_all(&tmp);
        let ceiling = tmp.join("ceiling");
        let sub = ceiling.join("sub");
        let leaf = sub.join("leaf");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::create_dir_all(sub.join(".edda")).unwrap();

        // start canonicalized (which produces \\?\ verbatim prefix on Windows),
        // ceiling uncanonicalized.
        let canonical_leaf = leaf.canonicalize().unwrap();
        let found = EddaPaths::find_root_bounded(&canonical_leaf, &ceiling);
        assert!(
            found.is_some(),
            "must find .edda when start is canonicalized"
        );
        assert_eq!(
            clean_unc_path(&found.unwrap().canonicalize().unwrap()),
            clean_unc_path(&sub.canonicalize().unwrap())
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_root_worktree_inside_parent_with_dot_edda() {
        // GH-701: A detached review worktree is created at ~/.edda/fleet/wt-review-pr<N>.
        // Its parent hierarchy has ~/.edda (the global user state directory).
        // find_root must resolve to the main repo, NOT ~/.edda!
        let tmp = unique_tmp("wt_in_fake_home");
        let _ = std::fs::remove_dir_all(&tmp);
        let repo = tmp.join("main_repo");
        let fake_home = tmp.join("fake_home");
        let wt = fake_home
            .join(".edda")
            .join("fleet")
            .join("wt-review-pr100");

        // Main repo with .edda
        std::fs::create_dir_all(repo.join(".git").join("worktrees").join("pr100")).unwrap();
        std::fs::create_dir_all(repo.join(".edda")).unwrap();

        // Fake home containing an unrelated .edda directory (user store root)
        std::fs::create_dir_all(fake_home.join(".edda")).unwrap();

        // Worktree under fake_home/.edda/fleet
        std::fs::create_dir_all(&wt).unwrap();
        let gitdir = repo.join(".git").join("worktrees").join("pr100");
        let gitdir_str = gitdir
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        std::fs::write(wt.join(".git"), format!("gitdir: {gitdir_str}")).unwrap();

        let found = EddaPaths::find_root(&wt);
        assert!(found.is_some(), "must resolve worktree to main repo");
        let resolved = found.unwrap();
        assert_eq!(
            resolved.canonicalize().unwrap(),
            repo.canonicalize().unwrap(),
            "must resolve to main_repo, NOT fake_home!"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_root_nested_workspace_inside_git_repo() {
        // P0-1: An edda workspace nested inside a larger git repo must be discovered
        let tmp = unique_tmp("nested_git");
        let _ = std::fs::remove_dir_all(&tmp);
        let repo = tmp.join("outer_git");
        let nested = repo.join("sub").join("nested_workspace");
        let deep = nested.join("deep").join("dir");

        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(nested.join(".edda")).unwrap();
        std::fs::create_dir_all(&deep).unwrap();

        let found = EddaPaths::find_root(&deep);
        assert!(found.is_some(), "must discover nested edda workspace");
        assert_eq!(
            found.unwrap().canonicalize().unwrap(),
            nested.canonicalize().unwrap(),
            "must resolve to nested workspace, not outer git repo"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
