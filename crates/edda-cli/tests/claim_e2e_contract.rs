//! End-to-end exit-code and surface contracts for `edda claim` and `edda claim check` (GH-562, GH-617, GH-705, GH-789).
//!
//! Spawns the compiled `edda` binary (`CARGO_BIN_EXE_edda`) against temporary
//! coordination boards and repositories.

use std::path::{Path, PathBuf};

fn edda_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_edda"))
}

fn e2e_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().expect("repo tempdir");
    std::fs::create_dir_all(repo.path().join(".edda")).expect("anchor .edda workspace");
    std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
    repo
}

fn run_edda(args: &[&str], repo: &Path, store: &Path) -> (i32, String, String) {
    let out = std::process::Command::new(edda_bin())
        .args(args)
        .current_dir(repo)
        .env("EDDA_STORE_ROOT", store)
        .output()
        .expect("spawn edda");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn run_edda_bare(args: &[&str], repo: &Path, store: &Path) -> (i32, String, String) {
    let out = std::process::Command::new(edda_bin())
        .args(args)
        .current_dir(repo)
        .env("EDDA_STORE_ROOT", store)
        .env_remove("EDDA_SESSION_ID")
        .env_remove("EDDA_SESSION_LABEL")
        .output()
        .expect("spawn edda");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn board_file(store: &Path, project_id: &str) -> PathBuf {
    store
        .join("projects")
        .join(project_id)
        .join("state")
        .join("coordination.jsonl")
}

fn write_board(store_root: &Path, project_id: &str, lines: &[String]) {
    let dir = store_root.join("projects").join(project_id).join("state");
    std::fs::create_dir_all(&dir).expect("state dir");
    std::fs::write(dir.join("coordination.jsonl"), lines.join("\n") + "\n")
        .expect("coordination.jsonl");
}

fn coord_event(session: &str, label: &str, paths: &[&str]) -> String {
    serde_json::json!({
        "ts": "2026-01-01T00:00:00Z",
        "session_id": session,
        "event_type": "claim",
        "payload": { "label": label, "paths": paths }
    })
    .to_string()
}

fn rfc3339_now_minus(secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs()
        .saturating_sub(secs);
    time::OffsetDateTime::from_unix_timestamp(now as i64)
        .expect("unix timestamp")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339")
}

fn write_heartbeat(store_root: &Path, project_id: &str, session: &str, age_secs: u64) {
    let dir = store_root.join("projects").join(project_id).join("state");
    std::fs::create_dir_all(&dir).expect("state dir");
    let hb = serde_json::json!({
        "session_id": session,
        "started_at": rfc3339_now_minus(age_secs),
        "last_heartbeat": rfc3339_now_minus(age_secs),
        "label": session,
        "focus_files": [],
        "active_tasks": [],
        "files_modified_count": 0,
        "total_edits": 0,
        "recent_commits": [],
    });
    std::fs::write(dir.join(format!("session.{session}.json")), hb.to_string())
        .expect("heartbeat file");
}

#[cfg(windows)]
#[test]
fn e2e_unicode_case_pair_is_error_not_clear() {
    // Claim `src/Ä.rs`, query `src/ä.rs`: the pre-fix engine returned
    // exit 0 with {"conflicts":[]} although both spellings resolve to
    // the same NTFS file. The check must refuse (exit 2) instead.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[coord_event("sess-9", "peer-c", &["src/Ä.rs"])],
    );
    // GH-617: the claim must come from a LIVE session for the engine's
    // refusal path to be reachable — a dead session's claim is filtered
    // before any surface comparison.
    write_heartbeat(store.path(), &project_id, "sess-9", 0);
    let bin = edda_bin();
    assert!(bin.exists(), "edda binary not found at {}", bin.display());
    let out = std::process::Command::new(&bin)
        .args(["claim", "check", "src/ä.rs", "--json"])
        .current_dir(repo.path())
        .env("EDDA_STORE_ROOT", store.path())
        .output()
        .expect("spawn edda");
    let code = out.status.code().expect("exit code");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    assert!(stderr.contains("cannot decide"), "stderr={stderr:?}");
}

#[test]
fn e2e_unreadable_board_is_error_not_clear() {
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    // A directory at the board path makes every read fail.
    std::fs::create_dir_all(board_file(store.path(), &project_id)).expect("block board");
    let (code, stdout, stderr) = run_edda(
        &["claim", "check", "src/main.rs", "--json"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 2,
        "unreadable board must exit 2, got stdout={stdout:?} stderr={stderr:?}"
    );
}

#[test]
fn e2e_malformed_board_line_is_error_not_clear() {
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &["{".to_string(), coord_event("s1", "peer-a", &["src/*"])],
    );
    let (code, stdout, stderr) = run_edda(
        &["claim", "check", "src/main.rs", "--json"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 2,
        "malformed board line must exit 2, got stdout={stdout:?} stderr={stderr:?}"
    );
}

#[test]
fn e2e_missing_board_is_clear() {
    // A missing board file legitimately means an empty board: exit 0.
    let repo = e2e_repo();
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, stdout, stderr) = run_edda(
        &["claim", "check", "src/main.rs", "--json"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "missing board must stay clear, got stdout={stdout:?} stderr={stderr:?}"
    );
}

#[test]
fn e2e_non_check_label_rejects_trailing_positional() {
    // Pre-GH-562 this was a clap usage error (exit 2); the shortcut must
    // not silently record a pathless claim from a typo.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, stdout, stderr) = run_edda(
        &["claim", "auth", "extra", "--session", "probe"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 2,
        "expected usage error, got stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        !board_file(store.path(), &project_id).exists(),
        "a pathless claim must not be recorded"
    );
}

#[test]
fn e2e_non_check_label_rejects_json_flag() {
    let repo = e2e_repo();
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, stdout, stderr) = run_edda(&["claim", "auth", "--json"], repo.path(), store.path());
    assert_eq!(
        code, 2,
        "--json is check-only, got stdout={stdout:?} stderr={stderr:?}"
    );
}

#[test]
fn e2e_non_check_claim_still_records_paths() {
    // The plain claim path must keep working byte-identically.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, stdout, stderr) = run_edda(
        &[
            "claim",
            "auth",
            "--paths",
            "src/auth/*",
            "--session",
            "probe",
        ],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "plain claim must still work, got stdout={stdout:?} stderr={stderr:?}"
    );
    let board = std::fs::read_to_string(board_file(store.path(), &project_id)).expect("board file");
    assert!(
        board.contains("\"paths\":[\"src/auth/*\"]"),
        "claim event must carry the paths, got: {board}"
    );
}

#[test]
fn e2e_bare_cli_claim_is_visible_to_claim_check() {
    // GH-705 defect A: a bare CLI claim (`cli-<label>`, minted by
    // resolve_session_id tier 4) comes from a one-shot process that
    // leaves no heartbeat whose age could later prove anything about
    // liveness. The machine gate must see the claim anyway: `claim
    // check` on the claimed surface conflicts immediately after the
    // claim — no freshness window, no heartbeat file required.
    let repo = e2e_repo();
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "bare-cli-lane", "--paths", "src/*"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "bare claim must succeed, got stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("cli-bare-cli-lane"),
        "claim must mint the tier-4 session id, got {stdout:?}"
    );
    // Reader side: the bare-CLI claim must conflict.
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "check", "src/main.rs"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 1,
        "a bare-CLI claim on the queried surface must conflict; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("bare-cli-lane"),
        "the bare-CLI claim must be reported as a conflict, got {stdout:?}"
    );
}

#[test]
fn e2e_aged_bare_cli_claim_still_conflicts() {
    // GH-705 round-1 P0: the claimant is a one-shot process, gone the
    // moment the command returns, so a heartbeat written at claim time
    // ages with nothing to refresh it. Backdating it past stale_secs
    // (120s by default — the round-1 probe) must NOT return the surface
    // to clear: the claim stands on the board until it is unclaimed, so
    // the gate keeps counting it (fail-closed) instead of dismissing it
    // as a dead session's claim.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[coord_event("cli-aged", "ghost-cli", &["src/*"])],
    );
    write_heartbeat(store.path(), &project_id, "cli-aged", 600);
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "check", "src/main.rs"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 1,
        "an aged bare-CLI claim must still conflict, not read as clear; \
         stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("cli-aged"),
        "the aged bare-CLI claim must be named, got {stdout:?}"
    );
}

#[test]
fn e2e_non_intersecting_bare_cli_claim_is_still_named() {
    let repo = e2e_repo();
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "other-lane", "--paths", "src/auth/*"],
        repo.path(),
        store.path(),
    );
    assert_eq!(code, 0, "claim failed: {stdout:?} {stderr:?}");

    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "check", "docs/guide.md"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "non-intersection is not a conflict: {stdout:?} {stderr:?}"
    );
    assert!(stdout.contains("other-lane") || stderr.contains("other-lane"));
}

#[test]
fn e2e_non_intersecting_bare_cli_claim_is_listed_in_json() {
    let repo = e2e_repo();
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "other-lane", "--paths", "src/auth/*"],
        repo.path(),
        store.path(),
    );
    assert_eq!(code, 0, "claim failed: {stdout:?} {stderr:?}");

    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "check", "docs/guide.md", "--json"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "non-intersection is not a conflict: {stdout:?} {stderr:?}"
    );
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON report");
    assert_eq!(
        report["standing_bare_claims"][0]["session_id"],
        "cli-other-lane"
    );
    assert!(report["unjudgeable_claims"]
        .as_array()
        .is_some_and(Vec::is_empty));
}

#[test]
fn e2e_never_heartbeated_bare_cli_claim_conflicts() {
    // The same fail-closed verdict when the one-shot claim left no
    // heartbeat at all: for a session id this binary itself mints,
    // "no heartbeat" cannot mean "never existed" — the board entry is
    // the evidence that the occupation is real.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[coord_event("cli-lost", "ghost-cli", &["src/*"])],
    );
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "check", "src/main.rs"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 1,
        "a bare-CLI claim without any heartbeat must still conflict; \
         stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("cli-lost"),
        "the bare-CLI claim must be named, got {stdout:?}"
    );
}

#[test]
fn e2e_aged_bare_cli_claim_is_classified_in_json() {
    // Machine-readable classification (GH-705 doneWhen): an overlapping
    // bare-CLI claim appears in its own `unjudgeable_claims` bucket — an
    // occupation whose liveness cannot be judged, distinct from a live
    // peer's conflict and from a dead session's stale claim, and counted
    // toward the exit code.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[coord_event("cli-aged", "ghost-cli", &["src/*"])],
    );
    write_heartbeat(store.path(), &project_id, "cli-aged", 600);
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "check", "src/main.rs", "--json"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 1,
        "must exit 1 on unjudgeable claim conflict; stdout={stdout:?} stderr={stderr:?}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON report");
    let unjudgeable = parsed["unjudgeable_claims"]
        .as_array()
        .expect("unjudgeable_claims array");
    assert_eq!(unjudgeable.len(), 1, "got {parsed:?}");
    assert_eq!(unjudgeable[0]["session_id"], "cli-aged");
    assert!(
        parsed["conflicts"].as_array().unwrap().is_empty(),
        "an unjudgeable claim must not be reported as a live conflict, got {parsed:?}"
    );
    assert!(
        parsed["stale_claims"].as_array().unwrap().is_empty(),
        "an unjudgeable claim must not be dismissed as stale, got {parsed:?}"
    );
}

#[test]
fn e2e_bare_cli_reclaim_is_idempotent() {
    // GH-705 round-1 P1: a one-shot heartbeat written at claim time made
    // the NEXT bare claim in the project hit resolve_session_id's
    // live-session ambiguity refusal — an identical re-claim, the most
    // ordinary bare-shell restart, exited 1. Tier 4's mint is
    // deterministic (`cli-<label>`), so a bare re-claim must go through.
    let repo = e2e_repo();
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "auth", "--paths", "src/auth/*"],
        repo.path(),
        store.path(),
    );
    assert_eq!(code, 0, "first claim must succeed: {stdout:?} {stderr:?}");
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "auth", "--paths", "src/auth/*"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "an idempotent bare re-claim must succeed; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("Re-claimed scope: auth (unchanged)"),
        "re-claim must be disclosed as a replacement, got {stdout:?}"
    );
}

#[test]
fn e2e_bare_cli_claim_can_narrow_scope() {
    // Same refusal shape, different command: narrowing a scope re-claims
    // the same minted session with fewer paths (cmd_bridge.rs: the
    // replacement-is-right contract).
    let repo = e2e_repo();
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, _stdout, stderr) = run_edda_bare(
        &["claim", "auth", "--paths", "src/auth/*"],
        repo.path(),
        store.path(),
    );
    assert_eq!(code, 0, "first claim must succeed: {stderr:?}");
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "auth", "--paths", "src/auth/login.rs"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "narrowing a bare claim must succeed; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("previous paths replaced"),
        "narrowing must be disclosed as replacement, got {stdout:?}"
    );
}

#[test]
fn e2e_second_agent_claims_disjoint_scope() {
    // The multi-agent instruction every agent is given is "claim your
    // scope at session start". A second bare-CLI agent claiming a
    // disjoint scope must not be refused because the first agent's claim
    // left a heartbeat behind.
    let repo = e2e_repo();
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, _stdout, stderr) = run_edda_bare(
        &["claim", "auth", "--paths", "src/auth/*"],
        repo.path(),
        store.path(),
    );
    assert_eq!(code, 0, "first claim must succeed: {stderr:?}");
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "api", "--paths", "src/api/*"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "a second bare-CLI agent claiming a disjoint scope must succeed; \
         stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("Claimed scope: api") && stdout.contains("session: cli-api"),
        "the second claim must mint its own session, got {stdout:?}"
    );
}

#[test]
fn e2e_lost_board_write_is_not_reported_as_success() {
    // GH-705 round-1 §5.5: the board append the machine gate reads is
    // best-effort on the hook paths it serves, but `edda claim`'s board
    // entry is its entire machine visibility. A directory at the board
    // path makes every append fail; the claim verb must verify the fold
    // actually sees the claim and refuse, rather than print `Claimed
    // scope:` and exit 0 over a claim nothing will ever count.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    std::fs::create_dir_all(board_file(store.path(), &project_id)).expect("block board");
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "auth", "--paths", "src/auth/*"],
        repo.path(),
        store.path(),
    );
    assert_ne!(
        code, 0,
        "a claim that was not recorded must not exit 0; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        !stdout.contains("Claimed scope"),
        "must not report a claim that did not land, got {stdout:?}"
    );
    assert!(
        stderr.contains("not recorded") || stderr.contains("coordination board"),
        "the failure must name the lost write, got {stderr:?}"
    );
}

#[test]
#[allow(clippy::permissions_set_readonly_false)]
fn e2e_lost_board_write_on_reclaim_is_not_reported_as_success() {
    // GH-705 Round 2 P1-1: a lost write during re-claiming (narrowing or
    // widening scope) must not be satisfied by re-reading the PREVIOUS
    // claim entry. If the new event fails to append to the board, the verb
    // must fail loudly rather than printing `Re-claimed scope:` and
    // returning 0.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "auth", "--paths", "src/auth/*"],
        repo.path(),
        store.path(),
    );
    assert_eq!(code, 0, "first claim must succeed: {stdout:?} {stderr:?}");

    let b_file = board_file(store.path(), &project_id);
    let mut perms = std::fs::metadata(&b_file).expect("metadata").permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&b_file, perms.clone()).expect("set readonly");

    let (code, stdout, stderr) = run_edda_bare(
        &["claim", "auth", "--paths", "src/auth/login.rs"],
        repo.path(),
        store.path(),
    );

    // Restore permissions so tempdir cleanup doesn't fail on Windows.
    perms.set_readonly(false);
    let _ = std::fs::set_permissions(&b_file, perms);

    assert_ne!(
        code, 0,
        "a lost write on re-claim must not exit 0; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        !stdout.contains("Re-claimed scope"),
        "must not report re-claim success when write was lost; got {stdout:?}"
    );
    assert!(
        stderr.contains("not recorded") || stderr.contains("coordination board"),
        "the failure must name the lost write, got {stderr:?}"
    );
}

#[test]
fn e2e_stale_session_claim_is_not_a_conflict() {
    // GH-617 consumption proof (write → read): one claim from a session
    // whose heartbeat expired plus one from a live session, both claiming
    // the same path. Querying that path must conflict with ONLY the live
    // session's claim; the stale one must neither appear as a conflict
    // nor flip the exit code.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[
            coord_event("dead0001", "ghost-lane", &["src/*"]),
            coord_event("livetest", "peer-live", &["src/*"]),
        ],
    );
    write_heartbeat(store.path(), &project_id, "dead0001", 3600);
    write_heartbeat(store.path(), &project_id, "livetest", 0);
    let (code, stdout, stderr) = run_edda(
        &["claim", "check", "src/main.rs"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 1,
        "the live claim must still conflict; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("peer-live") && stdout.contains("livetest"),
        "the live claim must be reported as a conflict, got {stdout:?}"
    );
    assert!(
        !stdout.contains("ghost-lane"),
        "a stale session's claim must not be reported as a conflict, got {stdout:?}"
    );
    assert!(
        !stdout.contains("dead0001"),
        "a dead session must not appear in the conflict list, got {stdout:?}"
    );
}

#[test]
fn e2e_stale_claims_are_visible_and_do_not_flip_exit_code() {
    // GH-617 death visibility: a board holding only a dead session's
    // claim must exit 0 AND say what was filtered, so "surface is clear"
    // stays distinguishable from "the liveness judgement broke".
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[coord_event("dead0002", "ghost-lane", &["src/*"])],
    );
    write_heartbeat(store.path(), &project_id, "dead0002", 3600);
    let (code, stdout, stderr) = run_edda(
        &["claim", "check", "src/main.rs"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "stale claims must not flip the exit code; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("stale"),
        "output must reveal that stale claims were filtered, got {stdout:?}"
    );
}

#[test]
fn e2e_claim_from_session_without_heartbeat_is_not_a_conflict() {
    // A session with no heartbeat file at all was never heard from — the
    // same verdict `edda peers` reaches — so its claim must not conflict.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[coord_event("lost0003", "ghost-lane", &["src/*"])],
    );
    let (code, stdout, stderr) = run_edda(
        &["claim", "check", "src/main.rs"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "a never-heartbeated session must not conflict; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("stale"),
        "output must reveal that stale claims were filtered, got {stdout:?}"
    );
}

#[test]
fn e2e_stale_claims_appear_in_json_report() {
    // Machine-readable death visibility: the JSON report must carry the
    // filtered stale claims alongside the (live-only) conflict list.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[
            coord_event("dead0004", "ghost-lane", &["src/*"]),
            coord_event("livetest", "peer-live", &["src/*"]),
        ],
    );
    write_heartbeat(store.path(), &project_id, "dead0004", 3600);
    write_heartbeat(store.path(), &project_id, "livetest", 0);
    let (code, stdout, stderr) = run_edda(
        &["claim", "check", "src/main.rs", "--json"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 1,
        "the live claim must still conflict; stdout={stdout:?} stderr={stderr:?}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON report");
    let conflicts = parsed["conflicts"].as_array().expect("conflicts array");
    assert_eq!(
        conflicts.len(),
        1,
        "only the live claim may conflict, got {conflicts:?}"
    );
    assert_eq!(conflicts[0]["session_id"], "livetest");
    let stale = parsed["stale_claims"]
        .as_array()
        .expect("stale_claims array in JSON report");
    assert_eq!(
        stale.len(),
        1,
        "the filtered stale claim must be named in the report, got {parsed:?}"
    );
    assert_eq!(stale[0]["session_id"], "dead0004");
}

#[test]
fn e2e_never_heartbeated_stale_claim_is_explicit_in_json() {
    // GH-705 defect A (machine report): "never heard from" must be an
    // explicit signal in the machine report (`"age_secs": null`), not a
    // silently missing field — a consumer must be able to tell "expired
    // heartbeat" from "no heartbeat file at all" without guessing.
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[coord_event("lost0005", "ghost-lane", &["src/*"])],
    );
    let (code, stdout, stderr) = run_edda(
        &["claim", "check", "src/main.rs", "--json"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        code, 0,
        "a never-heartbeated session must not conflict; stdout={stdout:?} stderr={stderr:?}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON report");
    let stale = parsed["stale_claims"]
        .as_array()
        .expect("stale_claims array in JSON report");
    assert_eq!(stale.len(), 1, "got {parsed:?}");
    assert_eq!(stale[0]["session_id"], "lost0005");
    assert!(
        stale[0].get("age_secs").is_some(),
        "age_secs must be present (null) for a never-heartbeated session, got {stale:?}"
    );
    assert!(
        stale[0]["age_secs"].is_null(),
        "a never-heartbeated session has no age, got {stale:?}"
    );
}

#[test]
fn e2e_exit_codes_conflict_and_disjoint() {
    let bin = edda_bin();
    if !bin.exists() {
        panic!("edda binary not found at {}", bin.display());
    }
    let repo = e2e_repo();
    let project_id = edda_store::project_id(repo.path());

    // Conflict case: an active claim overlaps the query surface.
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[coord_event("sess-1", "peer-a", &["crates/edda-cli/src/*"])],
    );
    // GH-617: only a LIVE session's claim conflicts — give sess-1 a
    // fresh heartbeat.
    write_heartbeat(store.path(), &project_id, "sess-1", 0);
    let out = std::process::Command::new(&bin)
        .args(["claim", "check", "crates/edda-cli/src/main.rs"])
        .current_dir(repo.path())
        .env("EDDA_STORE_ROOT", store.path())
        .output()
        .expect("spawn edda");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("peer-a"),
        "conflict must name the claim label"
    );
    assert!(stdout.contains("sess-1"), "conflict must name the session");

    // Disjoint case: active claim exists but covers nothing overlapping.
    let store = tempfile::tempdir().expect("store tempdir");
    write_board(
        store.path(),
        &project_id,
        &[coord_event(
            "sess-2",
            "peer-b",
            &["crates/edda-conductor/src/plan/**"],
        )],
    );
    // GH-617: a live, disjoint claim must keep the surface clear.
    write_heartbeat(store.path(), &project_id, "sess-2", 0);
    let out = std::process::Command::new(&bin)
        .args(["claim", "check", "crates/edda-cli/src/main.rs"])
        .current_dir(repo.path())
        .env("EDDA_STORE_ROOT", store.path())
        .output()
        .expect("spawn edda");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    // JSON case: machine-readable conflict list.
    let out = std::process::Command::new(&bin)
        .args(["claim", "check", "crates/edda-cli/src/main.rs", "--json"])
        .current_dir(repo.path())
        .env("EDDA_STORE_ROOT", store.path())
        .output()
        .expect("spawn edda");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON report");
    assert!(parsed["conflicts"]
        .as_array()
        .expect("conflicts array")
        .is_empty());
}
