//! GH-1093 integration matrix for `edda fleet reclaim`.
//!
//! These tests drive the compiled binary (`CARGO_BIN_EXE_edda`) against
//! isolated fixture repositories built with the real `git`. `gh` and `edda`
//! are stubbed on a prepended `PATH`; `git`, `sh` and the filesystem stay
//! real, because worktree registration, `git worktree remove`'s own refusals
//! and `git push --delete` are the behaviour under test.
//!
//! The matrix mirrors `scripts/fleet/test-reclaim-merged.sh` cases 1-7 and
//! 2a-2d: the exclusion boundary first (a reclaimer that is merely usually
//! right is a data-loss tool), then the fail-closed reads.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EDDA_BIN: &str = env!("CARGO_BIN_EXE_edda");
const SQUASH: &str = "1111111111111111111111111111111111111111";

/// Cross-platform stub writer, matching `dispatch_claim_guard.rs`.
fn stub(dir: &Path, name: &str, windows: &str, unix: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = unix;
        let path = dir.join(format!("{name}.bat"));
        std::fs::write(&path, windows.replace('\n', "\r\n")).unwrap();
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = windows;
        let path = dir.join(name);
        std::fs::write(&path, unix).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("git {args:?} in {}: {err}", dir.display()))
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn toplevel(dir: &Path) -> String {
    git_ok(dir, &["rev-parse", "--show-toplevel"])
        .trim()
        .to_string()
}

fn has_local(repo: &Path, name: &str) -> bool {
    git(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .status
    .success()
}

fn has_remote(repo: &Path, name: &str) -> bool {
    let out = git(repo, &["ls-remote", "--heads", "origin", name]);
    out.status.success() && !out.stdout.is_empty()
}

#[cfg(windows)]
fn real_git() -> String {
    let out = Command::new("where").arg("git").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find(|line| line.to_lowercase().ends_with(".exe"))
        .or_else(|| text.lines().next())
        .unwrap()
        .trim()
        .to_string()
}

#[cfg(not(windows))]
fn real_git() -> String {
    let out = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    repo: PathBuf,
    gh_stub: PathBuf,
    edda_stub: PathBuf,
    prs: PathBuf,
    empty_peers: PathBuf,
    lsremote_calls: PathBuf,
    base_sha: String,
}

impl Fixture {
    #[allow(clippy::too_many_lines)] // one fixture builder; splitting it hides the sequence
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_path_buf();
        let origin = base.join("origin.git");
        let repo = base.join("repo");
        git_ok(
            &base,
            &["init", "--quiet", "--bare", origin.to_str().unwrap()],
        );
        git_ok(&base, &["init", "--quiet", repo.to_str().unwrap()]);
        git_ok(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git_ok(&repo, &["config", "user.email", "test@example.com"]);
        git_ok(&repo, &["config", "user.name", "reclaim test"]);
        git_ok(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
        git_ok(&repo, &["add", "seed.txt"]);
        git_ok(&repo, &["commit", "--quiet", "-m", "seed"]);
        let base_sha = git_ok(&repo, &["rev-parse", "HEAD"]).trim().to_string();
        git_ok(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );

        for branch in [
            "merged-clean",
            "merged-dirty",
            "open-clean",
            "nested-merged",
            "moved",
            "no-pr",
        ] {
            git_ok(&repo, &["branch", branch, "main"]);
        }
        // `moved` outran its own merged PR: one commit no PR ever saw.
        git_ok(&repo, &["checkout", "--quiet", "moved"]);
        std::fs::write(repo.join("later.txt"), "later\n").unwrap();
        git_ok(&repo, &["add", "later.txt"]);
        git_ok(&repo, &["commit", "--quiet", "-m", "work the PR never saw"]);
        git_ok(&repo, &["checkout", "--quiet", "main"]);

        git_ok(
            &repo,
            &[
                "push",
                "--quiet",
                "origin",
                "main",
                "merged-clean",
                "merged-dirty",
                "open-clean",
                "moved",
            ],
        );

        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                base.join("wt-merged-clean").to_str().unwrap(),
                "merged-clean",
            ],
        );
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                base.join("wt-merged-dirty").to_str().unwrap(),
                "merged-dirty",
            ],
        );
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                base.join("wt-open").to_str().unwrap(),
                "open-clean",
            ],
        );
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                repo.join("nested").to_str().unwrap(),
                "nested-merged",
            ],
        );
        std::fs::write(
            base.join("wt-merged-dirty").join("uncommitted.txt"),
            "scratch\n",
        )
        .unwrap();
        git_ok(
            &repo,
            &["push", "--quiet", "origin", "merged-clean:remote-only"],
        );

        let prs = base.join("prs.tsv");
        std::fs::write(&prs, base_pr_table(&base_sha)).unwrap();
        let empty_peers = base.join("peers-empty.json");
        std::fs::write(&empty_peers, "{\"sessions\":[]}\n").unwrap();
        let lsremote_calls = base.join("lsremote-calls");
        std::fs::write(&lsremote_calls, "0\n").unwrap();

        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let gh_stub = stub(
            &bin,
            "gh",
            r#"@echo off
setlocal enabledelayedexpansion
if not "%GH_CALLS%"=="" (echo gh %*>>"%GH_CALLS%")
if "%1"=="pr" if "%2"=="list" (
  type "%STUB_PRS%"
  exit /b !ERRORLEVEL!
)
echo gh stub: unexpected invocation: %* 1>&2
exit /b 1
"#,
            r#"#!/bin/sh
[ -z "$GH_CALLS" ] || printf 'gh %s\n' "$*" >> "$GH_CALLS"
case "$1 $2" in
  'pr list') cat "$STUB_PRS" ;;
  *) echo "gh stub: unexpected invocation: $*" >&2; exit 1 ;;
esac
"#,
        );
        stub(
            &bin,
            "edda",
            r#"@echo off
if "%1"=="peers" if "%2"=="--json" (
  type "%EDDA_PEERS_JSON%"
  exit /b %EDDA_PEERS_RC%
)
echo edda stub: unexpected invocation: %* 1>&2
exit /b 1
"#,
            r#"#!/bin/sh
case "$1 $2" in
  'peers --json') cat "$EDDA_PEERS_JSON"; exit "${EDDA_PEERS_RC:-0}" ;;
  *) echo "edda stub: unexpected invocation: $*" >&2; exit 1 ;;
esac
"#,
        );
        let edda_stub = bin.join(if cfg!(windows) { "edda.bat" } else { "edda" });

        Self {
            _dir: dir,
            base,
            repo,
            gh_stub,
            edda_stub,
            prs,
            empty_peers,
            lsremote_calls,
            base_sha,
        }
    }

    fn wpath(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }

    fn write_file(&self, name: &str, content: &str) -> PathBuf {
        let path = self.base.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn prs_with_extra(&self, extra: &[(&str, &str, &str)], name: &str) -> PathBuf {
        // (branch, number, state)
        let mut text = String::new();
        for (branch, number, state) in extra {
            text.push_str(&format!(
                "{branch}\t{number}\t{state}\t{}\t{SQUASH}\n",
                self.base_sha
            ));
        }
        let mut combined = std::fs::read_to_string(&self.prs).unwrap();
        combined.push_str(&text);
        self.write_file(name, &combined)
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_cfg(args, &self.prs, &self.empty_peers, "0", None)
    }

    fn run_cfg(
        &self,
        args: &[&str],
        prs: &Path,
        peers: &Path,
        peers_rc: &str,
        git_bin: Option<&Path>,
    ) -> Output {
        let mut command = Command::new(EDDA_BIN);
        command
            .args(["fleet", "reclaim"])
            .args(args)
            .current_dir(&self.repo)
            .env("EDDA_GH_BIN", &self.gh_stub)
            .env("EDDA_PEERS_BIN", &self.edda_stub)
            .env("EDDA_GIT_BIN", git_bin.unwrap_or_else(|| Path::new("git")))
            .env("STUB_PRS", prs)
            .env("EDDA_PEERS_JSON", peers)
            .env("EDDA_PEERS_RC", peers_rc)
            .env("GH_CALLS", self.base.join("gh-calls"))
            .env("REAL_GIT", real_git())
            .env("RECLAIM_LSREMOTE_CALLS", &self.lsremote_calls);
        command.output().unwrap()
    }
}

fn base_pr_table(base: &str) -> String {
    format!(
        "merged-clean\t101\tMERGED\t{base}\t{SQUASH}\n\
         merged-dirty\t102\tMERGED\t{base}\t{SQUASH}\n\
         open-clean\t103\tOPEN\t{base}\t-\n\
         nested-merged\t104\tMERGED\t{base}\t{SQUASH}\n\
         moved\t105\tMERGED\t{base}\t{SQUASH}\n\
         remote-only\t106\tMERGED\t{base}\t{SQUASH}\n"
    )
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code_of(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn find_row<'a>(text: &'a str, kind: &str, item: &str) -> Option<Vec<&'a str>> {
    text.lines()
        .filter(|line| line.contains('\t'))
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .find(|fields| fields.len() >= 8 && fields[1] == kind && fields[2] == item)
}

#[track_caller]
fn expect_row(text: &str, kind: &str, item: &str, verdict: &str, reason: &str) {
    let fields = find_row(text, kind, item)
        .unwrap_or_else(|| panic!("no row for {kind} {item} in:\n{text}"));
    assert_eq!(fields[0], verdict, "row {kind} {item}: {fields:?}");
    assert!(
        fields[7].contains(reason),
        "row {kind} {item} reason {:?} lacks {reason:?}",
        fields[7]
    );
}

#[track_caller]
fn expect_no_row(text: &str, kind: &str, item: &str) {
    assert!(
        find_row(text, kind, item).is_none(),
        "unexpected row for {kind} {item} in:\n{text}"
    );
}

#[test]
fn dry_run_classifies_and_removes_nothing() {
    let f = Fixture::new();
    let out = f.run(&[]);
    let text = stdout_of(&out);
    assert_eq!(code_of(&out), 0, "stderr: {}", stderr_of(&out));

    let g_repo = toplevel(&f.repo);
    let g_clean = toplevel(&f.wpath("wt-merged-clean"));
    let g_dirty = toplevel(&f.wpath("wt-merged-dirty"));
    let g_open = toplevel(&f.wpath("wt-open"));
    let g_nested = toplevel(&f.repo.join("nested"));

    let calls = std::fs::read_to_string(f.base.join("gh-calls")).unwrap_or_default();
    assert!(
        calls.contains("pr list --state all"),
        "PR table not read: {calls}"
    );

    expect_row(&text, "worktree", &g_clean, "RECLAIM", "pr-merged");
    expect_row(&text, "worktree", &g_dirty, "KEEP", "tree-dirty");
    expect_row(&text, "worktree", &g_open, "KEEP", "pr-OPEN");
    expect_row(
        &text,
        "worktree",
        &g_nested,
        "KEEP",
        "nested-in-main-checkout",
    );
    expect_row(&text, "worktree", &g_repo, "KEEP", "main-checkout");
    expect_row(&text, "local-branch", "moved", "KEEP", "local-ahead-of-pr");
    expect_row(&text, "local-branch", "no-pr", "KEEP", "no-pr");
    expect_row(&text, "local-branch", "merged-dirty", "KEEP", "checked-out");
    expect_row(
        &text,
        "local-branch",
        "nested-merged",
        "KEEP",
        "checked-out",
    );
    expect_row(&text, "local-branch", "main", "KEEP", "default-branch");
    // The dry run has to predict --apply: merged-clean is checked out only in
    // the worktree this same run listed RECLAIM.
    expect_row(
        &text,
        "local-branch",
        "merged-clean",
        "RECLAIM",
        "pr-merged",
    );
    expect_row(
        &text,
        "remote-branch",
        "origin/moved",
        "KEEP",
        "remote-moved-since-merge",
    );
    expect_row(
        &text,
        "remote-branch",
        "origin/open-clean",
        "KEEP",
        "pr-OPEN",
    );
    expect_row(
        &text,
        "remote-branch",
        "origin/merged-dirty",
        "KEEP",
        "local-kept",
    );
    expect_row(
        &text,
        "remote-branch",
        "origin/merged-clean",
        "RECLAIM",
        "pr-merged",
    );
    expect_row(
        &text,
        "remote-branch",
        "origin/remote-only",
        "RECLAIM",
        "pr-merged",
    );
    expect_no_row(&text, "local-branch", "remote-only");

    // The dirty worktree's tree state is READ, not inferred from the verdict.
    let fields = find_row(&text, "worktree", &g_dirty).unwrap();
    assert_eq!(fields[6], "dirty");
    assert!(
        f.wpath("wt-merged-clean").is_dir(),
        "dry run removed a worktree"
    );
}

#[test]
fn protect_keeps_an_otherwise_qualifying_item() {
    let f = Fixture::new();
    let out = f.run(&["--protect", "wt-merged-clean"]);
    let text = stdout_of(&out);
    assert_eq!(code_of(&out), 0, "stderr: {}", stderr_of(&out));
    let g_clean = toplevel(&f.wpath("wt-merged-clean"));
    expect_row(&text, "worktree", &g_clean, "KEEP", "protected");
    assert!(f.wpath("wt-merged-clean").is_dir());
    // Protect is exact, not a prefix.
    let out = f.run(&["--protect", "wt-merged"]);
    let text = stdout_of(&out);
    expect_row(&text, "worktree", &g_clean, "RECLAIM", "pr-merged");
}

#[test]
fn live_peer_keeps_worktree_local_and_remote() {
    let f = Fixture::new();
    let live = f.write_file(
        "peers-live.json",
        "{\"sessions\":[{\"stale\":false,\"branch\":\"merged-clean\",\"label\":\"peer-lane\",\"session_id\":\"deadbeef\"}]}\n",
    );
    let out = f.run_cfg(&[], &f.prs, &live, "0", None);
    let text = stdout_of(&out);
    assert_eq!(code_of(&out), 0, "stderr: {}", stderr_of(&out));
    let g_clean = toplevel(&f.wpath("wt-merged-clean"));
    expect_row(&text, "worktree", &g_clean, "KEEP", "live-peer peer-lane");
    expect_row(
        &text,
        "local-branch",
        "merged-clean",
        "KEEP",
        "live-peer peer-lane",
    );
    expect_row(
        &text,
        "remote-branch",
        "origin/merged-clean",
        "KEEP",
        "live-peer peer-lane",
    );
    // Protection is specific to the peer's branch.
    expect_row(
        &text,
        "remote-branch",
        "origin/remote-only",
        "RECLAIM",
        "pr-merged",
    );

    let applied = f.run_cfg(&["--apply"], &f.prs, &live, "0", None);
    assert_eq!(code_of(&applied), 0, "stderr: {}", stderr_of(&applied));
    assert!(
        f.wpath("wt-merged-clean").is_dir(),
        "removed a live peer worktree"
    );
    assert!(
        has_local(&f.repo, "merged-clean"),
        "deleted a live peer local branch"
    );
    assert!(
        has_remote(&f.repo, "merged-clean"),
        "deleted a live peer remote branch"
    );
}

#[test]
fn empty_live_set_reclaims_the_same_item_again() {
    let f = Fixture::new();
    let out = f.run(&[]);
    let text = stdout_of(&out);
    let g_clean = toplevel(&f.wpath("wt-merged-clean"));
    expect_row(&text, "worktree", &g_clean, "RECLAIM", "pr-merged");
}

#[test]
fn unreadable_liveness_fails_closed() {
    let f = Fixture::new();
    let out = f.run_cfg(&[], &f.prs, &f.empty_peers, "1", None);
    let text = stdout_of(&out);
    let err = stderr_of(&out);
    assert_eq!(code_of(&out), 0, "stderr: {err}");
    let g_clean = toplevel(&f.wpath("wt-merged-clean"));
    expect_row(&text, "worktree", &g_clean, "KEEP", "liveness-unreadable");
    // The local ref is not an otherwise-RECLAIM candidate: its worktree is
    // present, so it keeps the more specific already-KEEP reason.
    expect_row(&text, "local-branch", "merged-clean", "KEEP", "checked-out");
    expect_row(
        &text,
        "remote-branch",
        "origin/merged-clean",
        "KEEP",
        "liveness-unreadable",
    );
    assert!(
        err.contains("live peer state unavailable"),
        "missing warning: {err}"
    );
    assert!(!text.contains("\tRECLAIM\t"), "a RECLAIM survived: {text}");

    let applied = f.run_cfg(&["--apply"], &f.prs, &f.empty_peers, "1", None);
    assert_eq!(code_of(&applied), 0, "stderr: {}", stderr_of(&applied));
    assert!(f.wpath("wt-merged-clean").is_dir());
    assert!(has_local(&f.repo, "merged-clean"));
    assert!(has_remote(&f.repo, "merged-clean"));
}

#[test]
fn unparseable_liveness_fails_closed() {
    let f = Fixture::new();
    let garbage = f.write_file("peers-garbage.json", "not json at all\n");
    let out = f.run_cfg(&[], &f.prs, &garbage, "0", None);
    let text = stdout_of(&out);
    assert_eq!(code_of(&out), 0, "stderr: {}", stderr_of(&out));
    let g_clean = toplevel(&f.wpath("wt-merged-clean"));
    expect_row(&text, "worktree", &g_clean, "KEEP", "liveness-unreadable");
    expect_row(
        &text,
        "remote-branch",
        "origin/merged-clean",
        "KEEP",
        "liveness-unreadable",
    );
    assert!(!text.contains("\tRECLAIM\t"), "a RECLAIM survived: {text}");

    let applied = f.run_cfg(&["--apply"], &f.prs, &garbage, "0", None);
    assert_eq!(code_of(&applied), 0);
    assert!(f.wpath("wt-merged-clean").is_dir());
}

#[test]
fn apply_removes_exactly_the_reclaim_set_then_is_a_noop() {
    let f = Fixture::new();
    let g_clean = toplevel(&f.wpath("wt-merged-clean"));
    let out = f.run(&["--apply"]);
    let text = stdout_of(&out);
    assert_eq!(code_of(&out), 0, "stderr: {}", stderr_of(&out));

    assert!(
        !f.wpath("wt-merged-clean").exists(),
        "merged clean worktree survived"
    );
    assert!(
        text.contains(&format!(
            "reclaimed worktree\t{g_clean}\tbranch=merged-clean\tpr=#101\tsquash={SQUASH}"
        )),
        "no worktree receipt:\n{text}"
    );

    assert!(
        f.wpath("wt-merged-dirty").is_dir(),
        "dirty worktree removed"
    );
    assert!(f.wpath("wt-merged-dirty").join("uncommitted.txt").exists());
    assert!(f.wpath("wt-open").is_dir(), "open-PR worktree removed");
    assert!(f.repo.join("nested").is_dir(), "nested worktree removed");

    assert!(
        !has_local(&f.repo, "merged-clean"),
        "merged branch survived"
    );
    assert!(
        text.contains("reclaimed local branch\tmerged-clean\tpr=#101"),
        "no local receipt:\n{text}"
    );
    assert!(
        has_local(&f.repo, "moved"),
        "branch ahead of its PR deleted"
    );
    assert!(has_local(&f.repo, "no-pr"), "no-PR branch deleted");
    assert!(
        has_local(&f.repo, "merged-dirty"),
        "checked-out branch deleted"
    );

    assert!(
        !has_remote(&f.repo, "merged-clean"),
        "merged remote survived"
    );
    assert!(
        text.contains("reclaimed remote branch\torigin/merged-clean\tpr=#101"),
        "no remote receipt:\n{text}"
    );
    assert!(
        !has_remote(&f.repo, "remote-only"),
        "remote-only branch survived"
    );
    assert!(has_remote(&f.repo, "moved"), "moved remote deleted");
    assert!(has_remote(&f.repo, "open-clean"), "open-PR remote deleted");
    assert!(
        has_remote(&f.repo, "merged-dirty"),
        "kept dirty lane's remote deleted"
    );
    assert!(
        text.contains("reclaim-merged: after"),
        "no after snapshot:\n{text}"
    );

    // A second pass finds nothing: every exclusion survives.
    let second = f.run(&["--apply"]);
    let second_text = stdout_of(&second);
    assert_eq!(code_of(&second), 0, "stderr: {}", stderr_of(&second));
    assert!(
        !second_text.contains("\tRECLAIM\t"),
        "second pass found candidates:\n{second_text}"
    );
    assert!(f.wpath("wt-merged-dirty").is_dir() && f.wpath("wt-open").is_dir());
}

#[test]
fn unreadable_pr_table_exits_three_and_reclaims_nothing() {
    let f = Fixture::new();
    let missing = f.base.join("does-not-exist.tsv");
    let out = f.run_cfg(&["--apply"], &missing, &f.empty_peers, "0", None);
    assert_eq!(code_of(&out), 3, "stderr: {}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(
        !text.contains("reclaimed"),
        "something removed without PR state:\n{text}"
    );
    assert!(f.wpath("wt-merged-clean").is_dir());
}

#[test]
fn partially_rejected_remote_batch_is_verified_not_trusted() {
    let f = Fixture::new();
    git_ok(&f.repo, &["branch", "partial-a", "main"]);
    git_ok(&f.repo, &["branch", "partial-b", "main"]);
    git_ok(
        &f.repo,
        &["push", "--quiet", "origin", "partial-a", "partial-b"],
    );
    git_ok(&f.repo, &["branch", "-D", "partial-a", "partial-b"]);
    let prs = f.prs_with_extra(
        &[
            ("partial-a", "201", "MERGED"),
            ("partial-b", "202", "MERGED"),
        ],
        "prs-case6.tsv",
    );
    let hook = f.base.join("origin.git").join("hooks").join("update");
    std::fs::write(
        &hook,
        "#!/bin/sh\ncase \"$1\" in\n  refs/heads/partial-b) echo \"rejected by hook: $1\" >&2; exit 1 ;;\nesac\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let out = f.run_cfg(&["--apply"], &prs, &f.empty_peers, "0", None);
    let text = stdout_of(&out);
    let err = stderr_of(&out);
    assert_eq!(code_of(&out), 0, "stderr: {err}");
    assert!(!has_remote(&f.repo, "partial-a"), "accepted ref survived");
    assert!(
        text.contains("reclaimed remote branch\torigin/partial-a\tpr=#201"),
        "no receipt for accepted ref:\n{text}"
    );
    assert!(
        has_remote(&f.repo, "partial-b"),
        "rejected ref deleted anyway"
    );
    assert!(
        err.contains("KEPT remote branch\torigin/partial-b\tstill present after delete"),
        "rejected ref not reported KEPT: {err}"
    );
    assert!(
        !text.contains("reclaimed remote branch\torigin/partial-b"),
        "rejected ref receipted as reclaimed:\n{text}"
    );
}

#[test]
fn failed_remote_reread_does_not_fabricate_a_receipt() {
    let f = Fixture::new();
    git_ok(&f.repo, &["branch", "verify-fail", "main"]);
    git_ok(&f.repo, &["push", "--quiet", "origin", "verify-fail"]);
    let prs = f.prs_with_extra(&[("verify-fail", "301", "MERGED")], "prs-case7.tsv");

    let shim_dir = f.base.join("git-shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let shim_git = stub(
        &shim_dir,
        "git",
        r#"@echo off
setlocal enabledelayedexpansion
if not "%~1"=="ls-remote" goto passthrough
if not "%~2"=="--heads" goto passthrough
if not "%~3"=="origin" goto passthrough
if not "%~4"=="" goto passthrough
set /p count=<"%RECLAIM_LSREMOTE_CALLS%"
set /a count=!count!+1
echo !count!>"%RECLAIM_LSREMOTE_CALLS%"
if !count! GTR 1 (
  echo git shim: origin unreachable 1>&2
  exit /b 128
)
:passthrough
"%REAL_GIT%" %*
exit /b %ERRORLEVEL%
"#,
        r#"#!/bin/sh
if [ "$1" = ls-remote ] && [ "$2" = --heads ] && [ "$3" = origin ] && [ $# -eq 3 ]; then
  n=$(cat "$RECLAIM_LSREMOTE_CALLS")
  n=$((n + 1))
  echo "$n" > "$RECLAIM_LSREMOTE_CALLS"
  if [ "$n" -gt 1 ]; then
    echo 'git shim: origin unreachable' >&2
    exit 128
  fi
fi
exec "$REAL_GIT" "$@"
"#,
    );

    let out = f.run_cfg(&["--apply"], &prs, &f.empty_peers, "0", Some(&shim_git));
    let text = stdout_of(&out);
    let err = stderr_of(&out);
    assert_eq!(code_of(&out), 4, "stdout: {text}\nstderr: {err}");
    assert!(
        text.contains("reclaimed local branch\tverify-fail\tpr=#301"),
        "local arm should still receipt:\n{text}"
    );
    assert!(
        !text.contains("reclaimed remote branch\torigin/verify-fail"),
        "fabricated a receipt for an unverified batch:\n{text}"
    );
    assert!(
        err.contains("KEPT remote branch\torigin/verify-fail\tunverified"),
        "no unverified/KEPT line: {err}"
    );
}
