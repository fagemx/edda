use anyhow::{bail, Context, Result};
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
        let guard = Self {
            repo: repo.into(),
            path: dest.into(),
            keep,
            removed: false,
        };
        if std::fs::symlink_metadata(guard.path.join(SUBJECT_MARKER)).is_ok() {
            bail!("review subject reserves {SUBJECT_MARKER}; committed marker is not allowed");
        }
        std::fs::write(guard.path.join(SUBJECT_MARKER), sha)?;
        Ok(guard)
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
