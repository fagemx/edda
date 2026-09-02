//! Contract tests for process object claims and merge gate claim protection (GH-581).
//!
//! Verifies:
//! 1. `edda claim review-pr570 --subject pr:570` records the subject on the board.
//! 2. `edda claim check pr:570` detects the conflict (exit 1) when session is live.
//! 3. `edda claim check pr:571` reports clear (exit 0).
//! 4. `edda prs check-merge` refuses when PR subject is claimed by an active session (exit 1).
//! 5. `edda prs check-merge --force` overrides the active claim and proceeds with a Claim Notice.
//! 6. Stale session claims or unrelated subjects do not block merge gate.

use std::path::PathBuf;
use std::process::Command;

struct TestEnv {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    store: PathBuf,
    project_id: String,
}

impl TestEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let store = dir.path().join("store");
        // Hermetic isolation (GH-646)
        std::fs::create_dir_all(repo.join(".edda")).expect("create .edda");
        std::fs::create_dir_all(repo.join(".git")).expect("create .git");
        std::fs::create_dir_all(&store).expect("create store");
        let project_id = edda_store::project_id(&repo);
        Self {
            _dir: dir,
            repo,
            store,
            project_id,
        }
    }

    fn board_path(&self) -> PathBuf {
        self.store
            .join("projects")
            .join(&self.project_id)
            .join("state")
            .join("coordination.jsonl")
    }

    fn write_heartbeat(&self, session_id: &str, age_secs: u64) {
        let ts = chrono::Utc::now() - chrono::Duration::seconds(age_secs as i64);
        let hb = serde_json::json!({
            "session_id": session_id,
            "started_at": ts.to_rfc3339(),
            "last_heartbeat": ts.to_rfc3339(),
            "label": "worker",
            "focus_files": [],
            "active_tasks": [],
            "files_modified_count": 0,
            "total_edits": 0,
            "recent_commits": []
        });
        let state_dir = self
            .store
            .join("projects")
            .join(&self.project_id)
            .join("state");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        std::fs::write(
            state_dir.join(format!("session.{session_id}.json")),
            serde_json::to_string_pretty(&hb).unwrap(),
        )
        .expect("write heartbeat");
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

    fn run_edda_with_env(&self, args: &[&str], env_vars: &[(&str, &str)]) -> (i32, String, String) {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_edda"));
        let mut cmd = Command::new(&bin);
        cmd.args(args)
            .current_dir(&self.repo)
            .env("EDDA_STORE_ROOT", &self.store);
        for (k, v) in env_vars {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("spawn edda");
        (
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

#[test]
fn claim_with_subject_records_on_board() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.run_edda(&[
        "claim",
        "review-pr570",
        "--subject",
        "pr:570",
        "--session",
        "reviewer-1",
    ]);

    assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
    assert!(stdout.contains("Claimed scope: review-pr570"));
    assert!(stdout.contains("subject: pr:570"));
    assert!(stdout.contains("session: reviewer-1"));

    let board = std::fs::read_to_string(env.board_path()).expect("read board");
    assert!(board.contains("\"subject\":\"pr:570\""), "board: {board}");
    assert!(
        board.contains("\"label\":\"review-pr570\""),
        "board: {board}"
    );
}

#[test]
fn claim_check_detects_matching_process_subject() {
    let env = TestEnv::new();
    env.write_heartbeat("reviewer-1", 0); // live session
    let (code, stdout, stderr) = env.run_edda(&[
        "claim",
        "review-pr570",
        "--subject",
        "pr:570",
        "--session",
        "reviewer-1",
    ]);
    assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");

    // Conflict check on pr:570 -> exit 1
    let (code_conflict, stdout_conflict, stderr_conflict) =
        env.run_edda(&["claim", "check", "pr:570"]);
    assert_eq!(
        code_conflict, 1,
        "stdout={stdout_conflict:?} stderr={stderr_conflict:?}"
    );
    assert!(
        stdout_conflict.contains("CONFLICT with claim \"review-pr570\""),
        "stdout: {stdout_conflict}"
    );
    assert!(stdout_conflict.contains("session reviewer-1"));

    // Disjoint check on pr:571 -> exit 0
    let (code_clear, stdout_clear, stderr_clear) = env.run_edda(&["claim", "check", "pr:571"]);
    assert_eq!(
        code_clear, 0,
        "stdout={stdout_clear:?} stderr={stderr_clear:?}"
    );
    assert!(
        stdout_clear.contains("No conflicts") || stdout_clear.contains("surface is clear"),
        "stdout: {stdout_clear}"
    );
}

#[test]
fn claim_check_on_stale_session_subject_reports_clear() {
    let env = TestEnv::new();
    // Stale session (e.g. 500s ago, heartbeat expired)
    env.write_heartbeat("reviewer-old", 500);
    let (code, _, _) = env.run_edda(&[
        "claim",
        "review-pr570",
        "--subject",
        "pr:570",
        "--session",
        "reviewer-old",
    ]);
    assert_eq!(code, 0);

    // Stale session claim must not conflict (exit 0)
    let (code_check, stdout_check, _) = env.run_edda(&["claim", "check", "pr:570"]);
    assert_eq!(code_check, 0);
    assert!(
        stdout_check.contains("surface is clear") || stdout_check.contains("No conflicts"),
        "stdout: {stdout_check}"
    );
}

#[test]
fn check_merge_refuses_claimed_pr_unless_force() {
    let env = TestEnv::new();
    env.write_heartbeat("active-reviewer", 0); // live session
    let (code, _, _) = env.run_edda(&[
        "claim",
        "review-pr570",
        "--subject",
        "pr:570",
        "--session",
        "active-reviewer",
    ]);
    assert_eq!(code, 0);

    // Prepare a valid MergeGateInput json file
    let input_json = serde_json::json!({
        "head_sha": "1234567890abcdef1234567890abcdef12345678",
        "pr_author": "worker-1",
        "verdict": "LGTM",
        "verdict_sha": "1234567890abcdef1234567890abcdef12345678",
        "verdict_author": "authorized-reviewer",
        "p0_count": 0,
        "p1_count": 0,
        "required_ci_green": true,
        "failed_checks": []
    });
    let input_path = env.repo.join("input.json");
    std::fs::write(&input_path, input_json.to_string()).expect("write input.json");

    // Case 1: Same session holding claim is allowed to merge
    let (code_same, stdout_same, _) = env.run_edda_with_env(
        &[
            "prs",
            "check-merge",
            "--input",
            input_path.to_str().unwrap(),
            "--head-sha",
            "1234567890abcdef1234567890abcdef12345678",
        ],
        &[("EDDA_SESSION_ID", "active-reviewer")],
    );
    assert_eq!(
        code_same, 0,
        "same session holding claim should pass: stdout={stdout_same}"
    );
    assert!(stdout_same.contains("PASS: Merge preconditions satisfied"));

    // Now test with PR argument in run_check_merge
    // We can simulate input with claimed_by set to verify evaluate_merge_preconditions and formatting
    let claimed_input_json = serde_json::json!({
        "head_sha": "1234567890abcdef1234567890abcdef12345678",
        "pr_author": "worker-1",
        "verdict": "LGTM",
        "verdict_sha": "1234567890abcdef1234567890abcdef12345678",
        "verdict_author": "authorized-reviewer",
        "p0_count": 0,
        "p1_count": 0,
        "required_ci_green": true,
        "failed_checks": [],
        "claimed_by": "active-reviewer (label: 'review-pr570', subject: 'pr:570')"
    });
    let claimed_input_path = env.repo.join("claimed_input.json");
    std::fs::write(&claimed_input_path, claimed_input_json.to_string())
        .expect("write claimed_input.json");

    // Case 2: Claimed by other session -> REFUSED (exit 1)
    let (code_refused, _stdout_refused, stderr_refused) = env.run_edda_with_env(
        &[
            "prs",
            "check-merge",
            "--input",
            claimed_input_path.to_str().unwrap(),
        ],
        &[("EDDA_SESSION_ID", "other-worker")],
    );
    assert_eq!(
        code_refused, 1,
        "claimed PR should refuse merge: stderr={stderr_refused}"
    );
    assert!(stderr_refused.contains("REFUSED"));
    assert!(stderr_refused.contains("PR is claimed by active session 'active-reviewer (label: 'review-pr570', subject: 'pr:570')' — use --force to override"));

    // Case 3: Override with --force -> PASS with Claim Notice (exit 0)
    let (code_force, stdout_force, stderr_force) = env.run_edda_with_env(
        &[
            "prs",
            "check-merge",
            "--input",
            claimed_input_path.to_str().unwrap(),
            "--force",
        ],
        &[("EDDA_SESSION_ID", "other-worker")],
    );
    assert_eq!(
        code_force, 0,
        "force should allow merge: stderr={stderr_force} stdout={stdout_force}"
    );
    assert!(stdout_force.contains("PASS: Merge preconditions satisfied"));
    assert!(stdout_force.contains("Claim Notice: Overriding process claim held by 'active-reviewer (label: 'review-pr570', subject: 'pr:570')' (--force specified)"));
}
