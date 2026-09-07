//! Contract test for `edda review gate`'s three exit codes (GH-769).
//!
//! The exit code *is* the interface: `scripts/pr-review-watch.sh` maps 0/1/2
//! onto the `Independent Review` commit status (`success`/`failure`/`error`)
//! and decides nothing itself. The in-crate tests stop at `union()` and the
//! `Result` level, while `run` calls `std::process::exit` directly, so only a
//! spawned binary can assert the codes that mapping reads.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const SHA: &str = "8d943560d00053d5372745579b74b633da36a830";

struct TestEnv {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    store: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let store = dir.path().join("store");
        std::fs::create_dir_all(&repo).expect("create repo");
        std::fs::create_dir_all(&store).expect("create store");
        // Hermetic isolation (GH-646): .edda stops EddaPaths::find_root here
        // rather than letting it climb to the developer's home directory.
        std::fs::create_dir_all(repo.join(".edda")).expect("create .edda");
        // A real repository, because the `--base` case resolves a ref.
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "gate@test"][..],
            &["config", "user.name", "gate"][..],
            &["commit", "-q", "--allow-empty", "-m", "base"][..],
        ] {
            let out = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .expect("spawn git");
            assert!(out.status.success(), "git {args:?}");
        }
        Self {
            _dir: dir,
            repo,
            store,
        }
    }

    /// Run `edda review gate ...`, feeding `stdin` to it.
    fn gate(&self, args: &[&str], stdin: &str) -> (i32, String, String) {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_edda"));
        let mut child = Command::new(&bin)
            .arg("review")
            .arg("gate")
            .args(args)
            .current_dir(&self.repo)
            .env("EDDA_STORE_ROOT", &self.store)
            .env("EDDA_SESSION_ID", "review-gate-exit-probe")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn edda");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stdin.as_bytes())
            .expect("write verdicts");
        let out = child.wait_with_output().expect("wait for edda");
        (
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

#[test]
fn a_clean_lgtm_exits_0() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.gate(&[SHA, "--verdicts", "-"], "LGTM\t0\t0\n");
    assert_eq!(code, 0, "stdout: {stdout}stderr: {stderr}");
    assert_eq!(stdout.trim_end(), format!("PASS {SHA} verdicts=1"));
}

#[test]
fn a_standing_changes_requested_exits_1_naming_the_union() {
    let env = TestEnv::new();
    // GH-742: the later LGTM does not clear the earlier blocking verdict.
    let (code, stdout, stderr) = env.gate(
        &[SHA, "--verdicts", "-"],
        "Changes Requested\t1\t0\nLGTM\t0\t0\n",
    );
    assert_eq!(code, 1, "stdout: {stdout}stderr: {stderr}");
    assert_eq!(stdout.trim_end(), format!("FAIL {SHA} union"));
}

#[test]
fn no_verdict_at_all_exits_2_rather_than_judging() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.gate(&[SHA, "--verdicts", "-"], "\n   \n");
    assert_eq!(code, 2, "stdout: {stdout}stderr: {stderr}");
    assert_eq!(stdout.trim_end(), format!("NONE {SHA}"));
}

#[test]
fn a_value_that_is_not_a_full_sha_cannot_be_judged() {
    let env = TestEnv::new();
    // The watcher reads exit 2 as `error`, which is the honest answer to a
    // subject the gate cannot even name.
    let (code, _stdout, stderr) = env.gate(&["deadbeef", "--verdicts", "-"], "LGTM\t0\t0\n");
    assert_eq!(code, 2);
    assert!(stderr.contains("40-character SHA"), "{stderr}");
}

#[test]
fn a_base_shaped_like_a_flag_is_refused_not_handed_to_git() {
    let env = TestEnv::new();
    // The verdicts pass, so the window check runs and the base is resolved.
    // `commit` puts it behind `rev-parse --verify --end-of-options`, so this
    // is rejected as a ref rather than reaching git as an option.
    let (code, stdout, stderr) = env.gate(
        &[SHA, "--verdicts", "-", "--base", "--upload-pack=touched"],
        "LGTM\t0\t0\n",
    );
    assert_ne!(code, 0, "a flag-shaped base must not pass the gate");
    assert!(!stdout.contains("PASS"), "stdout: {stdout}stderr: {stderr}");
}
