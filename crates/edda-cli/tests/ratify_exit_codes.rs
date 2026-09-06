//! Contract test for `edda ratify`'s three exit codes (GH-764, GH-761).
//!
//! `claim-check.exit-codes=0/1/2` is what a post-merge hook reads: it must
//! tell "that key does not exist" (1) apart from "you mistyped the evidence"
//! (2) apart from "already binding, nothing to do" (0). The in-crate tests
//! stop at the `Result` level and `usage_exit` calls `std::process::exit(2)`
//! directly, so only a spawned binary can assert the code itself.

use std::path::PathBuf;
use std::process::Command;

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
        // Hermetic isolation (GH-646): create .edda so EddaPaths::find_root
        // stops here rather than climbing to the home directory.
        std::fs::create_dir_all(repo.join(".edda")).expect("create .edda");
        std::fs::create_dir_all(repo.join(".git")).expect("create .git");
        std::fs::create_dir_all(&store).expect("create store");
        {
            use edda_ledger::Ledger;
            let ledger = Ledger::open(&repo).expect("open ledger");
            ledger.set_head_branch("main").expect("set HEAD");
        }
        Self {
            _dir: dir,
            repo,
            store,
        }
    }

    fn run_edda(&self, args: &[&str]) -> (i32, String, String) {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_edda"));
        let out = Command::new(&bin)
            .args(args)
            .current_dir(&self.repo)
            .env("EDDA_STORE_ROOT", &self.store)
            .env("EDDA_SESSION_ID", "ratify-exit-probe")
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
fn malformed_evidence_exits_2() {
    let env = TestEnv::new();
    // Abbreviated SHA: refused before the ledger is even opened, so the code
    // is 2 (usage) and not 1 (no such key) even though the key is absent.
    let (code, stdout, stderr) =
        env.run_edda(&["ratify", "db.engine", "--evidence", "pr#1@8d9435"]);

    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stderr.contains("error:"),
        "stderr should carry a usage diagnostic, got: {stderr:?}"
    );
}

#[test]
fn unknown_rule_exits_2() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.run_edda(&["ratify", "--by-rule", "no-such-rule"]);

    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stderr.contains("unknown rule"),
        "stderr should name the unknown rule, got: {stderr:?}"
    );
}

#[test]
fn key_together_with_by_rule_exits_2() {
    let env = TestEnv::new();
    let (code, stdout, stderr) =
        env.run_edda(&["ratify", "db.engine", "--by-rule", "cited-authority"]);

    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
}

#[test]
fn absent_key_exits_1() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.run_edda(&["ratify", "nope.key", "--by", "operator"]);

    assert_eq!(code, 1, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stderr.contains("no active decision"),
        "stderr should say the key has no active decision, got: {stderr:?}"
    );
}

#[test]
fn already_binding_exits_0_and_writes_nothing_more() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.run_edda(&[
        "decide",
        "db.engine=sqlite",
        "--reason",
        "embedded, zero-config",
    ]);
    assert_eq!(code, 0, "decide: stdout={stdout:?} stderr={stderr:?}");

    let evidence = format!("pr#1016@{SHA}");
    let first = env.run_edda(&["ratify", "db.engine", "--evidence", &evidence]);
    assert_eq!(first.0, 0, "first ratify: {first:?}");
    assert!(first.1.contains("now binding"), "first ratify: {first:?}");

    // A hook that fires twice on the same PR is a no-op the second time —
    // exit 0, not an error, and no second assertion in the ledger.
    let second = env.run_edda(&["ratify", "db.engine", "--evidence", &evidence]);
    assert_eq!(second.0, 0, "second ratify: {second:?}");
    assert!(
        second.1.contains("already binding"),
        "second ratify: {second:?}"
    );

    let ledger = edda_ledger::Ledger::open(&env.repo).expect("open ledger");
    let events = ledger
        .iter_events_by_type("decision_ratify")
        .expect("ratify events");
    assert_eq!(events.len(), 1, "second ratify must not append");
    assert_eq!(
        events[0].payload["ratified_by"],
        format!("evidence:{evidence}")
    );
}
