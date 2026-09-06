use edda_core::event::{new_cmd_event_with_git_context, CmdEventParams};
use edda_ledger::blob_store::blob_put;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::io::Write;
use std::path::Path;

pub fn execute(repo_root: &Path, argv: &[String]) -> anyhow::Result<()> {
    execute_in(repo_root, &std::env::current_dir()?, argv)
}

/// Keep the ledger root separate from the actual execution directory (which
/// can be a subdirectory or linked worktree with its own HEAD).
pub fn execute_in(repo_root: &Path, cwd: &Path, argv: &[String]) -> anyhow::Result<()> {
    if argv.is_empty() {
        anyhow::bail!("usage: edda run -- <command> [args...]");
    }

    let ledger = Ledger::open(repo_root)?;
    let before = git_context(cwd);
    let start = std::time::Instant::now();

    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to execute '{}': {e}", argv[0]))?;

    let duration_ms = start.elapsed().as_millis() as u64;
    let exit_code = output.status.code().unwrap_or(-1);
    let after = git_context(cwd);
    // A command that changes the checkout cannot certify the original clean
    // SHA. Git failures remain unknown; they never buy a clean receipt.
    let tree_dirty = if before.0 != after.0 || before.1 == Some(true) || after.1 == Some(true) {
        Some(true)
    } else if before.1 == Some(false) && after.1 == Some(false) {
        Some(false)
    } else {
        None
    };

    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let stdout_blob = blob_put(&ledger.paths, &output.stdout)?;
    let stderr_blob = blob_put(&ledger.paths, &output.stderr)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let cwd = cwd.to_string_lossy().to_string();

    let event = new_cmd_event_with_git_context(
        &CmdEventParams {
            branch: &branch,
            parent_hash: parent_hash.as_deref(),
            argv,
            cwd: &cwd,
            exit_code,
            duration_ms,
            stdout_blob: &stdout_blob,
            stderr_blob: &stderr_blob,
        },
        before.0.as_deref(),
        tree_dirty,
    )?;
    ledger.append_event(&event)?;

    // Replay output to terminal
    std::io::stdout().write_all(&output.stdout)?;
    std::io::stderr().write_all(&output.stderr)?;

    println!("Recorded CMD {} exit={exit_code}", event.event_id);
    Ok(())
}

fn git_context(cwd: &Path) -> (Option<String>, Option<bool>) {
    let output = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|o| o.status.success())
    };
    let sha = output(&["rev-parse", "--verify", "HEAD^{commit}"])
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let dirty = output(&[
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
        "--ignore-submodules=none",
    ])
    .map(|o| !o.stdout.is_empty());
    (sha, dirty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(path: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_owned()
    }

    #[test]
    fn git_context_distinguishes_clean_dirty_untracked_and_non_git() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(git_context(tmp.path()), (None, None));
        git(tmp.path(), &["init"]);
        git(tmp.path(), &["config", "user.name", "Test"]);
        git(tmp.path(), &["config", "user.email", "test@example.com"]);
        std::fs::write(tmp.path().join("tracked"), "one").expect("file");
        git(tmp.path(), &["add", "tracked"]);
        git(tmp.path(), &["commit", "-m", "initial"]);
        let sha = git(tmp.path(), &["rev-parse", "HEAD"]);
        assert_eq!(git_context(tmp.path()), (Some(sha.clone()), Some(false)));
        std::fs::write(tmp.path().join("tracked"), "two").expect("file");
        assert_eq!(git_context(tmp.path()), (Some(sha.clone()), Some(true)));
        git(tmp.path(), &["checkout", "--", "tracked"]);
        git(tmp.path(), &["config", "status.showUntrackedFiles", "no"]);
        std::fs::write(tmp.path().join("untracked"), "new").expect("file");
        assert_eq!(git_context(tmp.path()), (Some(sha), Some(true)));
    }

    #[test]
    fn receipt_uses_execution_directory_and_worktree_head() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        let worktree = tmp.path().join("linked");
        std::fs::create_dir(&repo).expect("repo");
        git(&repo, &["init"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        std::fs::write(repo.join(".gitignore"), ".edda/\n").expect("ignore");
        git(&repo, &["add", ".gitignore"]);
        git(&repo, &["commit", "-m", "initial"]);
        let ledger = Ledger::open_or_init(&repo).expect("ledger");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                worktree.to_str().expect("path"),
                "HEAD",
            ],
        );
        std::fs::write(worktree.join("tracked"), "content").expect("file");
        git(&worktree, &["add", "tracked"]);
        git(&worktree, &["commit", "-m", "worktree commit"]);
        let sha = git(&worktree, &["rev-parse", "HEAD"]);
        let nested = worktree.join("nested");
        std::fs::create_dir(&nested).expect("nested");
        execute_in(
            &repo,
            &nested,
            &["git".into(), "rev-parse".into(), "HEAD".into()],
        )
        .expect("run");
        let events = ledger.iter_events().expect("events");
        let event = events.last().expect("receipt");
        assert_eq!(event.payload["git_sha"], sha);
        assert_eq!(event.payload["tree_dirty"], false);
        assert_eq!(event.payload["cwd"], nested.to_string_lossy().as_ref());
        assert!(!worktree.join(".edda").exists());

        execute_in(
            &repo,
            &worktree,
            &["git".into(), "rm".into(), "tracked".into()],
        )
        .expect("mutating run");
        let events = ledger.iter_events().expect("events");
        let event = events.last().expect("receipt");
        assert_eq!(event.payload["git_sha"], sha);
        assert_eq!(event.payload["tree_dirty"], true);
    }
}
