//! Contract test for `edda notify send` (GH-765).
//!
//! `notify send` is the delivery verb for the daily digest:
//! - no channels configured → one log line, exit 0 (never fails delivery)
//! - a channel that cannot be reached → `ERR <channel>: …` line, exit 0
//!   (the error line is the log; delivery is best-effort)

use std::path::{Path, PathBuf};
use std::process::Command;

struct TestEnv {
    _dir: tempfile::TempDir,
    repo: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".edda")).expect("create .edda");
        std::fs::create_dir_all(repo.join(".git")).expect("create .git");
        Self { _dir: dir, repo }
    }

    fn run_edda(&self, args: &[&str]) -> (i32, String, String) {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_edda"));
        let out = Command::new(&bin)
            .args(args)
            .current_dir(&self.repo)
            .output()
            .expect("spawn edda");
        (
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn write_config(&self, json: &str) {
        std::fs::write(self.repo.join(".edda/config.json"), json).expect("write config");
    }
}

fn write_body(dir: &Path, content: &str) -> PathBuf {
    let path = dir.join("body.md");
    std::fs::write(&path, content).expect("write body");
    path
}

#[test]
fn send_without_channels_logs_one_line_and_exits_zero() {
    let env = TestEnv::new();
    let body = write_body(env._dir.path(), "digest body");
    let body = body.to_str().unwrap();

    let (code, stdout, stderr) = env.run_edda(&["notify", "send", "--title", "t", "--file", body]);
    assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stdout.contains("No notification channels configured; digest not sent."),
        "stdout should carry the log line, got: {stdout:?}"
    );
}

#[test]
fn send_to_dead_webhook_reports_err_and_exits_zero() {
    let env = TestEnv::new();
    env.write_config(
        r#"{"notify_channels":[{"type":"webhook","url":"http://127.0.0.1:9","events":["*"]}]}"#,
    );
    let body = write_body(env._dir.path(), "digest body");
    let body = body.to_str().unwrap();

    let (code, stdout, stderr) = env.run_edda(&["notify", "send", "--title", "t", "--file", body]);
    assert_eq!(
        code, 0,
        "a failing channel must not fail the verb: {stderr:?}"
    );
    assert!(
        stdout.contains("ERR webhook("),
        "stdout should carry the per-channel ERR line, got: {stdout:?}"
    );
}
