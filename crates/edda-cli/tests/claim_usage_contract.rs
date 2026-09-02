//! Regression tests for the non-check dda claim usage diagnostics contract (GH-589).
//!
//! GH-562 introduced dda claim check as a subcommand while preserving
//! dda claim <LABEL> for scope claims. The diagnostics contract for
//! non-check usage errors is:
//!
//! 1. Missing label and no subcommand (dda claim):
//!    - Exit code 2.
//!    - Stderr specifies that dda claim requires a label or the check subcommand.
//!    - No claim is written to the coordination board.
//!
//! 2. Extra positional arguments on plain claim (dda claim auth extra --session probe):
//!    - Exit code 2.
//!    - Stderr reports subcommand conflict (cannot be used with).
//!    - No claim is written to the coordination board.
//!
//! 3. Check-only flag on plain claim (dda claim auth --json):
//!    - Exit code 2.
//!    - Stderr reports unexpected argument --json.
//!    - No claim is written to the coordination board.
//!
//! 4. Valid plain claim (dda claim auth --paths src/auth/* --session probe):
//!    - Exit code 0.
//!    - Claim is written to the coordination board with paths and session.

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
        // Hermetic isolation (GH-646): create .edda so EddaPaths::find_root
        // stops here rather than climbing to the home directory.
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

#[test]
fn missing_label_and_no_subcommand_exits_2_with_contract_diagnostics() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.run_edda(&["claim"]);

    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    assert!(stdout.is_empty(), "stdout should be empty, got: {stdout:?}");
    assert!(
        stderr.contains("error: 'edda claim' requires a label (e.g. 'edda claim auth --paths src/auth/*') or the 'check' subcommand"),
        "stderr should contain contract diagnostics, got: {stderr:?}"
    );
    assert!(
        !env.board_path().exists(),
        "coordination board must not be created on missing label"
    );
}

#[test]
fn plain_claim_rejects_extra_positional_arguments() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.run_edda(&["claim", "auth", "extra", "--session", "probe"]);

    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("unexpected argument"),
        "stderr should report argument conflict, got: {stderr:?}"
    );
    assert!(
        !env.board_path().exists(),
        "coordination board must not be created on invalid usage"
    );
}

#[test]
fn plain_claim_rejects_check_only_json_flag() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.run_edda(&["claim", "auth", "--json"]);

    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stderr.contains("unexpected argument '--json'"),
        "stderr should report unexpected argument, got: {stderr:?}"
    );
    assert!(
        !env.board_path().exists(),
        "coordination board must not be created on invalid usage"
    );
}

#[test]
fn valid_plain_claim_succeeds_and_records_to_board() {
    let env = TestEnv::new();
    let (code, stdout, stderr) = env.run_edda(&[
        "claim",
        "auth",
        "--paths",
        "src/auth/*",
        "--session",
        "probe",
    ]);

    assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stdout.contains("Claimed scope: auth"),
        "stdout should confirm claim, got: {stdout:?}"
    );
    assert!(
        env.board_path().exists(),
        "coordination board must be created on valid claim"
    );
    let content = std::fs::read_to_string(env.board_path()).expect("read board");
    assert!(
        content.contains("\"paths\":[\"src/auth/*\"]"),
        "board content should carry claimed paths, got: {content:?}"
    );
    assert!(
        content.contains("\"session_id\":\"probe\""),
        "board content should carry session_id, got: {content:?}"
    );
}
