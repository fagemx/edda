use edda_core::event::{new_cmd_event_with_git_context_and_dirty_paths, CmdEventParams};
use edda_ledger::blob_store::blob_put;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    let before = git_snapshot(cwd);
    let start = std::time::Instant::now();

    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to execute '{}': {e}", argv[0]))?;

    let duration_ms = start.elapsed().as_millis() as u64;
    let exit_code = output.status.code().unwrap_or(-1);
    let after = git_snapshot(cwd);
    let git_context = combine_git_snapshots(before, after);

    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let stdout_blob = blob_put(&ledger.paths, &output.stdout)?;
    let stderr_blob = blob_put(&ledger.paths, &output.stderr)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let cwd = cwd.to_string_lossy().to_string();

    let dirty_paths = git_context
        .dirty_paths
        .as_ref()
        .map(|paths| (paths.tracked.as_slice(), paths.untracked.as_slice()));
    let event = new_cmd_event_with_git_context_and_dirty_paths(
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
        git_context.sha.as_deref(),
        git_context.tree_dirty,
        dirty_paths,
    )?;
    ledger.append_event(&event)?;

    // Replay output to terminal
    std::io::stdout().write_all(&output.stdout)?;
    std::io::stderr().write_all(&output.stderr)?;

    println!("Recorded CMD {} exit={exit_code}", event.event_id);
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DirtyPaths {
    tracked: Vec<String>,
    untracked: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct GitSnapshot {
    sha: Option<String>,
    dirty_paths: Option<DirtyPaths>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct GitContext {
    sha: Option<String>,
    tree_dirty: Option<bool>,
    dirty_paths: Option<DirtyPaths>,
}

fn git_snapshot(cwd: &Path) -> GitSnapshot {
    let sha = git_output(cwd, &["rev-parse", "--verify", "HEAD^{commit}"])
        .and_then(|bytes| one_git_line(&bytes));
    let dirty_paths = git_output(cwd, &["rev-parse", "--show-toplevel"])
        .and_then(|bytes| one_git_line(&bytes))
        .map(PathBuf::from)
        .and_then(|root| {
            git_output(
                &root,
                &[
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--untracked-files=all",
                    "--ignore-submodules=none",
                ],
            )
        })
        .and_then(|bytes| parse_porcelain_v1_z(&bytes));
    GitSnapshot { sha, dirty_paths }
}

fn git_output(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
}

fn one_git_line(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    (!text.is_empty() && !text.chars().any(char::is_control)).then(|| text.to_owned())
}

fn parse_porcelain_v1_z(bytes: &[u8]) -> Option<DirtyPaths> {
    let mut tracked = BTreeSet::new();
    let mut untracked = BTreeSet::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let record = take_nul_record(bytes, &mut offset)?;
        if record.len() < 4 || record[2] != b' ' || !valid_status(record[0], record[1]) {
            return None;
        }
        let path = parse_status_path(&record[3..])?;
        let destination = if record[0] == b'?' && record[1] == b'?' {
            &mut untracked
        } else {
            &mut tracked
        };
        destination.insert(path);
        if matches!(record[0], b'R' | b'C') || matches!(record[1], b'R' | b'C') {
            destination.insert(parse_status_path(take_nul_record(bytes, &mut offset)?)?);
        }
    }
    Some(DirtyPaths {
        tracked: tracked.into_iter().collect(),
        untracked: untracked.into_iter().collect(),
    })
}

fn valid_status(index: u8, worktree: u8) -> bool {
    if (index, worktree) == (b'?', b'?') {
        return true;
    }
    let ordinary = |status| {
        matches!(
            status,
            b' ' | b'M' | b'T' | b'A' | b'D' | b'R' | b'C' | b'U'
        )
    };
    ordinary(index) && ordinary(worktree) && (index, worktree) != (b' ', b' ')
}

fn take_nul_record<'a>(bytes: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
    let length = bytes.get(*offset..)?.iter().position(|byte| *byte == 0)?;
    let end = offset.checked_add(length)?;
    let record = bytes.get(*offset..end)?;
    *offset = end.checked_add(1)?;
    Some(record)
}

fn parse_status_path(bytes: &[u8]) -> Option<String> {
    (!bytes.is_empty())
        .then_some(bytes)
        .and_then(|path| std::str::from_utf8(path).ok())
        .map(str::to_owned)
}

fn combine_git_snapshots(before: GitSnapshot, after: GitSnapshot) -> GitContext {
    let sha = before.sha.clone();
    match (before.sha.as_deref(), after.sha.as_deref()) {
        (Some(before_sha), Some(after_sha)) if before_sha != after_sha => GitContext {
            sha,
            tree_dirty: Some(true),
            dirty_paths: None,
        },
        (Some(_), Some(_)) => match (before.dirty_paths, after.dirty_paths) {
            (Some(before_paths), Some(after_paths)) => {
                let dirty_paths = union_dirty_paths(before_paths, after_paths);
                GitContext {
                    sha,
                    tree_dirty: Some(
                        !dirty_paths.tracked.is_empty() || !dirty_paths.untracked.is_empty(),
                    ),
                    dirty_paths: Some(dirty_paths),
                }
            }
            _ => GitContext {
                sha,
                tree_dirty: None,
                dirty_paths: None,
            },
        },
        _ => GitContext {
            sha,
            tree_dirty: None,
            dirty_paths: None,
        },
    }
}

fn union_dirty_paths(before: DirtyPaths, after: DirtyPaths) -> DirtyPaths {
    DirtyPaths {
        tracked: before
            .tracked
            .into_iter()
            .chain(after.tracked)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        untracked: before
            .untracked
            .into_iter()
            .chain(after.untracked)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
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
    fn strict_porcelain_parser_covers_paths_and_rejects_bad_records() {
        assert_eq!(parse_porcelain_v1_z(b""), Some(DirtyPaths::default()));
        assert_eq!(
            parse_porcelain_v1_z(
                " M z tracked.rs\0A  staged file.rs\0?? untracked space.txt\0?? 文档.txt\0"
                    .as_bytes()
            ),
            Some(DirtyPaths {
                tracked: vec!["staged file.rs".into(), "z tracked.rs".into()],
                untracked: vec!["untracked space.txt".into(), "文档.txt".into()],
            })
        );
        assert_eq!(
            parse_porcelain_v1_z(b"R  new name\0old name\0C  copy\0source\0"),
            Some(DirtyPaths {
                tracked: vec![
                    "copy".into(),
                    "new name".into(),
                    "old name".into(),
                    "source".into()
                ],
                untracked: vec![],
            })
        );
        for malformed in [
            b" M missing-nul".as_slice(),
            b"M bad-status-width\0",
            b"?? \0",
            b"?M mixed-question\0",
            b"R  missing-second\0",
            b" M valid\0\0",
            b"!! ignored\0",
            b" M non-utf8-\xff\0",
        ] {
            assert!(parse_porcelain_v1_z(malformed).is_none(), "{malformed:?}");
        }
    }

    #[test]
    fn git_snapshot_distinguishes_clean_tracked_untracked_and_non_git() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(git_snapshot(tmp.path()), GitSnapshot::default());
        git(tmp.path(), &["init"]);
        git(tmp.path(), &["config", "user.name", "Test"]);
        git(tmp.path(), &["config", "user.email", "test@example.com"]);
        std::fs::write(tmp.path().join("tracked"), "one").expect("file");
        git(tmp.path(), &["add", "tracked"]);
        git(tmp.path(), &["commit", "-m", "initial"]);
        let sha = git(tmp.path(), &["rev-parse", "HEAD"]);
        assert_eq!(
            git_snapshot(tmp.path()),
            GitSnapshot {
                sha: Some(sha.clone()),
                dirty_paths: Some(DirtyPaths::default()),
            }
        );
        std::fs::write(tmp.path().join("tracked"), "two").expect("file");
        assert_eq!(
            git_snapshot(tmp.path())
                .dirty_paths
                .expect("known status")
                .tracked,
            ["tracked"]
        );
        git(tmp.path(), &["checkout", "--", "tracked"]);
        git(tmp.path(), &["config", "status.showUntrackedFiles", "no"]);
        std::fs::write(tmp.path().join("untracked"), "new").expect("file");
        let snapshot = git_snapshot(tmp.path());
        assert_eq!(snapshot.sha.as_deref(), Some(sha.as_str()));
        assert_eq!(
            snapshot.dirty_paths.expect("known status").untracked,
            ["untracked"]
        );
    }

    #[test]
    fn before_after_paths_are_a_sorted_deduplicated_union() {
        let context = combine_git_snapshots(
            GitSnapshot {
                sha: Some("head".into()),
                dirty_paths: Some(DirtyPaths {
                    tracked: vec!["z".into(), "same".into()],
                    untracked: vec!["old".into()],
                }),
            },
            GitSnapshot {
                sha: Some("head".into()),
                dirty_paths: Some(DirtyPaths {
                    tracked: vec!["a".into(), "same".into()],
                    untracked: vec!["new".into()],
                }),
            },
        );
        assert_eq!(context.tree_dirty, Some(true));
        assert_eq!(
            context.dirty_paths,
            Some(DirtyPaths {
                tracked: vec!["a".into(), "same".into(), "z".into()],
                untracked: vec!["new".into(), "old".into()],
            })
        );
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
        assert_eq!(
            event.payload["tree_dirty_paths"],
            serde_json::json!({"tracked": [], "untracked": []})
        );
        assert_eq!(event.payload["cwd"], nested.to_string_lossy().as_ref());
        assert!(!worktree.join(".edda").exists());

        std::fs::write(worktree.join("root-only"), "untracked").expect("root file");
        assert_eq!(
            git_snapshot(&nested)
                .dirty_paths
                .expect("root-relative status")
                .untracked,
            ["root-only"]
        );
        std::fs::remove_file(worktree.join("root-only")).expect("remove root file");

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
        assert_eq!(event.payload["tree_dirty_paths"]["tracked"][0], "tracked");

        execute_in(
            &repo,
            &worktree,
            &[
                "git".into(),
                "-c".into(),
                "user.name=Test".into(),
                "-c".into(),
                "user.email=test@example.com".into(),
                "commit".into(),
                "-m".into(),
                "move head".into(),
            ],
        )
        .expect("head-mutating run");
        let events = ledger.iter_events().expect("events");
        let event = events.last().expect("receipt");
        assert_eq!(event.payload["git_sha"], sha);
        assert_eq!(event.payload["tree_dirty"], true);
        assert!(event.payload["tree_dirty_paths"].is_null());
    }
}
