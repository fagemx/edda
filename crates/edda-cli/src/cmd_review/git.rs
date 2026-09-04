use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const SUBJECT_MARKER: &str = ".edda-review-subject";

pub(crate) fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).current_dir(cwd).output()?;
    if !out.status.success() {
        bail!(
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?.trim_end().to_owned())
}

pub(crate) fn git_ok(cwd: &Path, args: &[&str]) -> Result<bool> {
    Ok(Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()?
        .status
        .success())
}

pub(crate) fn commit(cwd: &Path, value: &str) -> Result<String> {
    let sha = git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{value}^{{commit}}"),
        ],
    )?;
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("expected a full commit SHA: {value}");
    }
    Ok(sha)
}

pub(crate) fn repo_root_from(cwd: &Path) -> Result<PathBuf> {
    let common = git(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    Ok(Path::new(&common)
        .parent()
        .context("git common directory has no parent")?
        .canonicalize()?)
}

pub(crate) fn resolve_base(repo: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(value) = explicit {
        return commit(repo, value);
    }
    if let Ok(value) = git(repo, &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"]) {
        if let Ok(sha) = commit(repo, &value) {
            return Ok(sha);
        }
    }
    for value in ["origin/main", "origin/master", "main", "master"] {
        if let Ok(sha) = commit(repo, value) {
            return Ok(sha);
        }
    }
    bail!("cannot resolve comparison base; pass --base <ref>")
}

/// Owns only a freshly created, unique temporary checkout. Existing paths
/// are never reclaimed: they may belong to another review or to the user.
pub(crate) struct WorktreeGuard {
    repo: PathBuf,
    pub path: PathBuf,
    keep: bool,
    removed: bool,
    pristine: String,
    content_hash: String,
}

impl WorktreeGuard {
    pub(crate) fn create(repo: &Path, dest: &Path, sha: &str, keep: bool) -> Result<Self> {
        if dest.exists() {
            bail!("review scratch already exists: {}", dest.display());
        }
        let parent = dest.parent().context("scratch requires parent directory")?;
        std::fs::create_dir_all(parent)?;
        git(
            repo,
            &[
                "-c",
                "core.hooksPath=",
                "worktree",
                "add",
                "--detach",
                "--",
                &dest.to_string_lossy(),
                sha,
            ],
        )?;
        let mut guard = Self {
            repo: repo.into(),
            path: dest.into(),
            keep,
            removed: false,
            pristine: String::new(),
            content_hash: String::new(),
        };
        if std::fs::symlink_metadata(guard.path.join(SUBJECT_MARKER)).is_ok() {
            bail!("review subject reserves {SUBJECT_MARKER}; committed marker is not allowed");
        }
        std::fs::write(guard.path.join(SUBJECT_MARKER), sha)?;
        guard.pristine = git(
            &guard.path,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )?;
        guard.content_hash = content_hash(&guard.path)?;
        Ok(guard)
    }

    pub(crate) fn verify_unchanged(&self, sha: &str) -> Result<bool> {
        let head = git(&self.path, &["rev-parse", "HEAD"])?;
        let status = git(
            &self.path,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )?;
        let hash = content_hash(&self.path)?;
        Ok(head == sha && status == self.pristine && hash == self.content_hash)
    }

    pub(crate) fn remove(&mut self) -> Result<()> {
        if self.keep || self.removed {
            return Ok(());
        }
        git(
            &self.repo,
            &[
                "worktree",
                "remove",
                "--force",
                "--",
                &self.path.to_string_lossy(),
            ],
        )?;
        self.removed = true;
        Ok(())
    }
}

fn content_hash(root: &Path) -> Result<String> {
    // `ls-files -s` supplies the Git entry's mode and object id.  The index
    // alone is not enough for ordinary files (a reviewer can change the
    // working copy without staging it), so regular files and symlinks also
    // contribute their on-disk representation.  Gitlinks deliberately do
    // not: an uninitialized submodule is a valid checkout and recursing into
    // one would make its unrelated working state part of this review.
    let listed = git(root, &["ls-files", "-s", "-z"])?;
    let mut hasher = Sha256::new();
    for entry in listed.split('\0') {
        if entry.is_empty() {
            continue;
        }
        let (metadata, path) = entry
            .split_once('\t')
            .context("malformed git index entry")?;
        let mut fields = metadata.split_whitespace();
        let mode = fields.next().context("git index entry missing mode")?;
        let oid = fields.next().context("git index entry missing object id")?;
        let stage = fields.next().context("git index entry missing stage")?;
        if fields.next().is_some() {
            bail!("malformed git index entry");
        }
        hasher.update(mode.as_bytes());
        hasher.update([0]);
        hasher.update(oid.as_bytes());
        hasher.update([0]);
        hasher.update(stage.as_bytes());
        hasher.update([0]);
        hasher.update(path.as_bytes());
        hasher.update([0]);
        match mode {
            // A symlink's Git payload is its link target. `read` follows the
            // link and fails for a valid dangling link, so use `read_link`.
            "120000" => hasher.update(
                std::fs::read_link(root.join(path))?
                    .as_os_str()
                    .as_encoded_bytes(),
            ),
            // The index records the submodule commit. Do not traverse it.
            "160000" => {}
            _ => hasher.update(std::fs::read(root.join(path))?),
        }
        hasher.update([0]);
    }
    hasher.update(SUBJECT_MARKER.as_bytes());
    hasher.update([0]);
    hasher.update(std::fs::read(root.join(SUBJECT_MARKER))?);
    Ok(hex::encode(hasher.finalize()))
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!(
                "edda review: failed to remove {}: {error}",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod testrepo {
    use super::*;
    pub(crate) fn run(root: &Path, args: &[&str]) -> String {
        git(root, args).expect("git fixture")
    }
    pub(crate) fn init() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        run(&root, &["init", "-q", "-b", "main"]);
        run(&root, &["config", "user.email", "test@example.invalid"]);
        run(&root, &["config", "user.name", "test"]);
        run(&root, &["config", "core.hooksPath", ""]);
        commit_file(&root, "a.txt", "a\n", "initial");
        (temp, root)
    }
    pub(crate) fn commit_file(root: &Path, name: &str, body: &str, message: &str) -> String {
        std::fs::write(root.join(name), body).unwrap();
        run(root, &["add", "--", name]);
        run(root, &["commit", "-q", "-m", message]);
        commit(root, "HEAD").unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::{content_hash, testrepo, SUBJECT_MARKER};

    #[test]
    fn content_proof_changes_when_tracked_content_changes() {
        let (_temp, root) = testrepo::init();
        std::fs::write(root.join(SUBJECT_MARKER), "subject").unwrap();
        let before = content_hash(&root).unwrap();
        std::fs::write(root.join("a.txt"), "tampered\n").unwrap();
        assert_ne!(before, content_hash(&root).unwrap());
    }

    #[test]
    fn content_proof_accepts_uninitialized_gitlink_without_traversing_it() {
        let (_temp, root) = testrepo::init();
        let head = testrepo::run(&root, &["rev-parse", "HEAD"]);
        testrepo::run(
            &root,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{head},nested"),
            ],
        );
        testrepo::run(&root, &["commit", "-q", "-m", "gitlink"]);
        std::fs::write(root.join(SUBJECT_MARKER), "subject").unwrap();
        assert!(content_hash(&root).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn content_proof_accepts_dangling_symlink_by_its_link_target() {
        use std::os::unix::fs::symlink;

        let (_temp, root) = testrepo::init();
        symlink("missing-target", root.join("dangling")).unwrap();
        testrepo::run(&root, &["add", "--", "dangling"]);
        testrepo::run(&root, &["commit", "-q", "-m", "dangling link"]);
        std::fs::write(root.join(SUBJECT_MARKER), "subject").unwrap();
        assert!(content_hash(&root).is_ok());
    }
}
