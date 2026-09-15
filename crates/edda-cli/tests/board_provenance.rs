//! Board provenance for `unclaim` and `peers` (GH-1048).
//!
//! The literal diagnosis — that a lane worktree and the main checkout are
//! different boards — is false: `edda_store::project_id` resolves a linked
//! worktree's `.git` file to the main checkout root, so both share one board.
//! The real failure class is that an exit-0 no-op named no board at all, so a
//! `unclaim --if-claimed` run from a directory that resolves to a *different*
//! board reads as a release that happened while the original claim survives.
//!
//! These tests drive the compiled binary (`CARGO_BIN_EXE_edda`) against two
//! throwaway git boards in one throwaway store. They never touch the
//! operator's real store.

use std::path::{Path, PathBuf};
use std::process::Command;

const EDDA: &str = env!("CARGO_BIN_EXE_edda");

/// One isolated world: a private store root and two git boards.
struct Fixture {
    _dir: tempfile::TempDir,
    store: PathBuf,
    board_a: PathBuf,
    board_b: PathBuf,
}

impl Fixture {
    fn store(&self) -> &Path {
        &self.store
    }
}

/// Run edda in `dir` against `store`, with no ambient session identity.
fn run_in(dir: &Path, store: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(EDDA)
        .args(args)
        .current_dir(dir)
        .env("EDDA_STORE_ROOT", store)
        .env_remove("EDDA_SESSION_ID")
        .env_remove("EDDA_SESSION_LABEL")
        .output()
        .unwrap_or_else(|e| panic!("could not run edda {args:?} in {}: {e}", dir.display()));
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn git_init(dir: &Path) {
    let out = Command::new("git")
        .args(["init", "-q", "."])
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git init in {}: {e}", dir.display()));
    assert!(out.status.success(), "git init failed in {}", dir.display());
}

/// A private store plus two boards, each a real git repo initialized with edda.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("store");
    let board_a = dir.path().join("board-a");
    let board_b = dir.path().join("board-b");
    std::fs::create_dir_all(&store).expect("store dir");
    for board in [&board_a, &board_b] {
        std::fs::create_dir_all(board).expect("board dir");
        git_init(board);
        let (code, stdout, stderr) = run_in(board, &store, &["init", "--no-hooks"]);
        assert_eq!(
            code,
            0,
            "edda init failed in {}:\n{stdout}\n{stderr}",
            board.display()
        );
    }
    Fixture {
        _dir: dir,
        store,
        board_a,
        board_b,
    }
}

/// The board a directory resolves to, as the machine surface reports it.
fn board_of(dir: &Path, store: &Path) -> (String, String) {
    let (code, stdout, stderr) = run_in(dir, store, &["bridge", "claude", "peers", "--json"]);
    assert_eq!(
        code,
        0,
        "peers --json failed in {}:\n{stdout}\n{stderr}",
        dir.display()
    );
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("peers --json not JSON: {e}\n{stdout}"));
    let id = json["board"]["project_id"]
        .as_str()
        .unwrap_or_else(|| panic!("peers --json carries no board.project_id:\n{stdout}"))
        .to_string();
    let root = json["board"]["root"]
        .as_str()
        .unwrap_or_else(|| panic!("peers --json carries no board.root:\n{stdout}"))
        .to_string();
    (id, root)
}

fn claim_scope(dir: &Path, store: &Path, label: &str, session: &str) {
    let (code, stdout, stderr) = run_in(
        dir,
        store,
        &[
            "bridge",
            "claude",
            "claim",
            label,
            "--session",
            session,
            "--paths",
            "src/*",
        ],
    );
    assert_eq!(
        code,
        0,
        "claim failed in {}:\n{stdout}\n{stderr}",
        dir.display()
    );
}

fn board_claims(dir: &Path, store: &Path) -> Vec<serde_json::Value> {
    let (code, stdout, stderr) = run_in(dir, store, &["bridge", "claude", "peers", "--json"]);
    assert_eq!(code, 0, "peers --json failed:\n{stdout}\n{stderr}");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    json["claims"].as_array().expect("claims array").clone()
}

#[test]
fn cross_board_if_claimed_no_op_carries_the_board_it_read() {
    // The class of failure GH-1048 names: a teardown runs `unclaim
    // --if-claimed` from a directory that resolves to another board, gets
    // exit 0 with a reassuring "Nothing to unclaim", and leaves the original
    // claim standing. The no-op must say which board it actually read.
    let fx = fixture();
    let (board_a_id, _) = board_of(&fx.board_a, fx.store());
    claim_scope(&fx.board_a, fx.store(), "zombie", "cli-zombie");

    let (board_b_id, board_b_root) = board_of(&fx.board_b, fx.store());

    let (code, stdout, stderr) = run_in(
        &fx.board_b,
        fx.store(),
        &[
            "bridge",
            "claude",
            "unclaim",
            "--session",
            "cli-zombie",
            "--if-claimed",
        ],
    );
    assert_eq!(
        code, 0,
        "a no-op teardown must stay exit 0:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("Nothing to unclaim for session cli-zombie"),
        "the release-that-did-not-happen must still say so:\n{stdout}"
    );
    assert!(
        stdout.contains(&board_b_id),
        "the no-op must name the board it consulted ({board_b_id}):\n{stdout}"
    );
    assert!(
        stdout.contains(&board_b_root),
        "the no-op must name the board root it consulted ({board_b_root}):\n{stdout}"
    );
    assert!(
        !stdout.contains(&board_a_id),
        "the no-op must not claim to have read board A ({board_a_id}):\n{stdout}"
    );

    // The release did not happen anywhere: board A still holds the claim.
    let claims = board_claims(&fx.board_a, fx.store());
    assert!(
        claims.iter().any(|c| c["session_id"] == "cli-zombie"),
        "board A must still hold cli-zombie after the cross-board no-op: {claims:?}"
    );
}

#[test]
fn peers_text_names_the_board_it_read() {
    // Every text path of `peers` names the board, including the empty-board
    // early return, so "no peers" and "wrong board" stop looking identical.
    let fx = fixture();
    let (id, root) = board_of(&fx.board_a, fx.store());

    let (code, stdout, stderr) = run_in(&fx.board_a, fx.store(), &["bridge", "claude", "peers"]);
    assert_eq!(code, 0, "peers failed:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("Board:"),
        "peers text names the board:\n{stdout}"
    );
    assert!(
        stdout.contains(&id),
        "peers text carries the project id ({id}):\n{stdout}"
    );
    assert!(
        stdout.contains(&root),
        "peers text carries the project root ({root}):\n{stdout}"
    );
}

#[test]
fn peers_json_reports_the_board_it_read() {
    // The machine surface carries a `board` object naming the project id and
    // root it read, so a consumer can tell which board produced these sessions
    // and claims (GH-1048). The unit-level twin of this assertion was moved
    // here: `cmd_bridge/tests.rs` sits exactly at its file-length ceiling
    // (1162) and only its two `peers_json` call sites changed, so the ratchet
    // is honoured rather than bypassed.
    let fx = fixture();
    let (code, stdout, stderr) = run_in(
        &fx.board_a,
        fx.store(),
        &["bridge", "claude", "peers", "--json"],
    );
    assert_eq!(code, 0, "peers --json failed:\n{stdout}\n{stderr}");
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("peers --json not JSON: {e}\n{stdout}"));

    let id = json["board"]["project_id"]
        .as_str()
        .unwrap_or_else(|| panic!("board.project_id must be a string:\n{stdout}"));
    assert_eq!(id.len(), 32, "project id is 32 hex chars, got {id:?}");
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit()),
        "project id is hex, got {id:?}"
    );

    let root = json["board"]["root"]
        .as_str()
        .unwrap_or_else(|| panic!("board.root must be a string:\n{stdout}"));
    assert!(!root.is_empty(), "board.root must not be empty");
    assert!(
        root.contains("board-a"),
        "board.root must name the board directory, got {root:?}"
    );
}

#[test]
fn unclaim_success_names_the_board() {
    let fx = fixture();
    let (id, root) = board_of(&fx.board_a, fx.store());
    claim_scope(&fx.board_a, fx.store(), "zombie", "cli-zombie");

    let (code, stdout, stderr) = run_in(
        &fx.board_a,
        fx.store(),
        &["bridge", "claude", "unclaim", "--session", "cli-zombie"],
    );
    assert_eq!(code, 0, "unclaim failed:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("Unclaimed scope for session: cli-zombie"),
        "the success line survives:\n{stdout}"
    );
    assert!(
        stdout.contains(&id),
        "the release names the board it wrote ({id}):\n{stdout}"
    );
    assert!(
        stdout.contains(&root),
        "the release names the board root ({root}):\n{stdout}"
    );

    let claims = board_claims(&fx.board_a, fx.store());
    assert!(
        claims.is_empty(),
        "board A must have released cli-zombie: {claims:?}"
    );
}

#[test]
fn worktree_shares_the_main_checkout_board() {
    // Pins the refutation: a linked worktree's `.git` file resolves to the
    // main checkout, so a lane worktree and its main checkout are the same
    // board. The `.edda` marker inside the worktree keeps `EddaPaths::find_root`
    // from short-circuiting to board A on its own, so `project_root` in the
    // store is what performs the worktree resolution under test.
    let fx = fixture();
    let (board_a_id, board_a_root) = board_of(&fx.board_a, fx.store());

    let worktree = fx._dir.path().join("lane-wt");
    std::fs::create_dir_all(worktree.join(".edda")).expect("worktree .edda");
    let gitdir = fx.board_a.join(".git").join("worktrees").join("lane-wt");
    std::fs::create_dir_all(&gitdir).expect("gitdir for the fake worktree");
    let gitdir = gitdir.to_string_lossy().replace('\\', "/");
    std::fs::write(worktree.join(".git"), format!("gitdir: {gitdir}")).expect("worktree .git file");

    let (wt_id, wt_root) = board_of(&worktree, fx.store());
    assert_eq!(
        wt_id, board_a_id,
        "a worktree must share the main checkout board id"
    );
    let canonical = |p: &str| {
        Path::new(p)
            .canonicalize()
            .unwrap_or_else(|e| panic!("canonicalize {p}: {e}"))
    };
    assert_eq!(
        canonical(&wt_root),
        canonical(&board_a_root),
        "a worktree's board root must be the main checkout root"
    );
}
