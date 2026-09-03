//! Golden contract tests for refusing newer schema versions on ledger open (GH-729).
//!
//! Operator ruling (compat.schema-version-policy=read-older-refuse-newer-minor-bump-announced):
//! A ledger whose store version is newer than the binary must be refused to open
//! with exit code 2 and an error message naming both the stored version and the
//! binary's maximum supported version.

use rusqlite::Connection;
use std::path::PathBuf;
use std::process::Command;

struct TestEnv {
    _dir: tempfile::TempDir,
    repo: PathBuf,
}

impl TestEnv {
    fn new(future_version: u32) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let edda_dir = repo.join(".edda");
        std::fs::create_dir_all(&edda_dir).expect("create .edda");
        std::fs::create_dir_all(repo.join(".git")).expect("create .git");

        // Create a ledger.db with future schema version in schema_meta
        let db_path = edda_dir.join("ledger.db");
        let conn = Connection::open(&db_path).expect("open sqlite db");
        conn.execute_batch(&format!(
            "CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO schema_meta (key, value) VALUES ('version', '{future_version}');"
        ))
        .expect("write schema_meta");

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
}

#[test]
fn status_refuses_newer_schema_with_exit_2_and_both_versions() {
    let env = TestEnv::new(99);
    let (code, stdout, stderr) = env.run_edda(&["status"]);

    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stderr.contains("refusing to open ledger: stored schema version 99 is newer than maximum supported version 13"),
        "stderr should name both version numbers (stored 99, max 13), got: {stderr:?}"
    );
}

#[test]
fn verify_refuses_newer_schema_with_exit_2_and_both_versions() {
    let env = TestEnv::new(99);
    let (code, stdout, stderr) = env.run_edda(&["verify"]);

    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stderr.contains("refusing to open ledger: stored schema version 99 is newer than maximum supported version 13"),
        "stderr should name both version numbers (stored 99, max 13), got: {stderr:?}"
    );
}

#[test]
fn ask_refuses_newer_schema_with_exit_2_and_both_versions() {
    let env = TestEnv::new(99);
    let (code, stdout, stderr) = env.run_edda(&["ask", "test query"]);

    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stderr.contains("refusing to open ledger: stored schema version 99 is newer than maximum supported version 13"),
        "stderr should name both version numbers (stored 99, max 13), got: {stderr:?}"
    );
}
