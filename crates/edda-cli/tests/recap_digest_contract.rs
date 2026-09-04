//! Contract test for `edda recap --digest` (GH-765).
//!
//! The digest is a deterministic, offline ledger projection: unratified
//! decisions, blocked/failed tasks, ready tasks, and cost samples from
//! session digests. Its output is embedded verbatim by
//! `scripts/fleet/daily-digest.sh`, so this test pins the exact rendered
//! lines against a real ledger.

use std::path::{Path, PathBuf};
use std::process::Command;

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
        // Initialize the ledger: open_or_create the DB and set HEAD so
        // `edda decide` / `edda task new` have a branch to write against.
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
            .output()
            .expect("spawn edda");
        (
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

/// Append a `note` event tagged `session_digest` whose
/// `session_stats.estimated_cost_usd` is `cost` when measured and absent
/// when `None` (never null-as-zero — GH-585 semantics).
fn add_session_digest_note(repo: &Path, cost: Option<f64>) {
    use edda_core::event::{finalize_event, new_note_event};
    use edda_ledger::Ledger;

    let ledger = Ledger::open(repo).expect("open ledger");
    let branch = ledger.head_branch().expect("head branch");
    let parent = ledger.last_event_hash().expect("last hash");
    let mut event = new_note_event(
        &branch,
        parent.as_deref(),
        "system",
        "session digest",
        &["session_digest".to_string()],
    )
    .expect("note event");
    let mut stats = serde_json::json!({});
    if let Some(c) = cost {
        stats["estimated_cost_usd"] = serde_json::json!(c);
    }
    event.payload["session_stats"] = stats;
    finalize_event(&mut event).expect("finalize");
    ledger.append_event(&event).expect("append");
}

#[test]
fn digest_lists_unratified_decision_ready_task_and_session_cost() {
    let env = TestEnv::new();

    // one unratified decision
    let (code, _, stderr) = env.run_edda(&["decide", "db.engine=PostgreSQL", "--reason", "test"]);
    assert_eq!(code, 0, "decide failed: {stderr}");

    // one ready task
    let (code, _, stderr) = env.run_edda(&["task", "new", "x"]);
    assert_eq!(code, 0, "task new failed: {stderr}");

    // one measured and one unmeasured session digest note
    add_session_digest_note(&env.repo, Some(1.25));
    add_session_digest_note(&env.repo, None);

    let (code, stdout, stderr) = env.run_edda(&["recap", "--digest"]);
    assert_eq!(code, 0, "recap --digest failed: {stderr}");

    assert!(
        stdout.contains("- decision `db.engine` — unratified"),
        "decision line missing, got:\n{stdout}"
    );
    assert!(
        stdout.contains("task #1 x"),
        "ready task missing, got:\n{stdout}"
    );
    assert!(
        stdout.contains("session_stats.estimated_cost_usd：$1.25（1 筆量測，1 筆未量測）"),
        "cost line missing, got:\n{stdout}"
    );
}

#[test]
fn digest_rejects_bad_since_with_exit_2() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.run_edda(&["recap", "--digest", "--since", "soon"]);
    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("invalid --since"),
        "error should name the flag, got: {combined:?}"
    );
}
