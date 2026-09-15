//! `edda peers --json` vs `edda claim check` on the same board (GH-1069).
//!
//! `stale` reports heartbeat-window freshness; `blocks` must report whether the
//! claim still refuses a writer, which is bounded by a different window
//! (`EDDA_CLAIM_TTL_SECS`). A claim between the two windows is the only fixture
//! that tells them apart, so this drives both verbs against one such board and
//! checks they agree, then repeats with an expired claim.

use std::path::{Path, PathBuf};

fn e2e_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().expect("repo tempdir");
    std::fs::create_dir_all(repo.path().join(".edda")).expect("anchor .edda workspace");
    std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
    repo
}

fn run_edda(args: &[&str], repo: &Path, store: &Path) -> (i32, String, String) {
    let out = std::process::Command::new(PathBuf::from(env!("CARGO_BIN_EXE_edda")))
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

/// Write a one-claim board whose claim was recorded `age_secs` ago.
fn write_aged_claim(store: &Path, project_id: &str, session: &str, age_secs: u64) {
    let dir = store.join("projects").join(project_id).join("state");
    std::fs::create_dir_all(&dir).expect("state dir");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs()
        .saturating_sub(age_secs);
    let ts = time::OffsetDateTime::from_unix_timestamp(now as i64)
        .expect("unix timestamp")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339");
    let event = serde_json::json!({
        "ts": ts,
        "session_id": session,
        "event_type": "claim",
        "payload": { "label": session, "paths": ["src/*"] },
    });
    std::fs::write(dir.join("coordination.jsonl"), format!("{event}\n"))
        .expect("coordination board");
}

/// The one claim `edda peers --json` reported, looked up by label.
fn reported_claim(peers_json: &str, label: &str) -> serde_json::Value {
    let parsed: serde_json::Value = serde_json::from_str(peers_json).expect("valid peers JSON");
    parsed["claims"]
        .as_array()
        .expect("claims array")
        .iter()
        .find(|c| c["label"] == label)
        .unwrap_or_else(|| panic!("claim {label} missing: {parsed}"))
        .clone()
}

#[test]
fn peers_json_blocks_matches_claim_check_inside_the_ttl() {
    // One hour is past `stale_secs` (120s) and inside `claim_ttl_secs` (24h):
    // not fresh, but still refusing a writer. The two answers must differ.
    let repo = e2e_repo();
    let store = tempfile::tempdir().expect("store tempdir");
    let project_id = edda_store::project_id(repo.path());
    write_aged_claim(store.path(), &project_id, "cli-an-hour", 3600);

    let (check_code, check_out, check_err) = run_edda(
        &["claim", "check", "src/main.rs"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        check_code, 1,
        "an hour-old bare-CLI claim must still refuse a writer: {check_out} {check_err}"
    );

    let (peers_code, peers_out, peers_err) =
        run_edda(&["peers", "--json"], repo.path(), store.path());
    assert_eq!(
        peers_code, 0,
        "peers --json failed: {peers_out} {peers_err}"
    );
    let claim = reported_claim(&peers_out, "cli-an-hour");
    assert_eq!(
        claim["stale"], true,
        "an hour is past the heartbeat window: {claim}"
    );
    assert_eq!(
        claim["blocks"], true,
        "inside the TTL the claim still blocks a writer: {claim}"
    );
    assert_eq!(
        check_code == 1,
        claim["blocks"].as_bool().expect("blocks is a bool"),
        "`blocks` must agree with `claim check` on the same claim"
    );
}

#[test]
fn peers_json_blocks_matches_claim_check_past_the_ttl() {
    // Past `claim_ttl_secs` the claim no longer refuses a writer, even though
    // it is still not fresh.
    let repo = e2e_repo();
    let store = tempfile::tempdir().expect("store tempdir");
    let project_id = edda_store::project_id(repo.path());
    write_aged_claim(
        store.path(),
        &project_id,
        "cli-two-months",
        60 * 60 * 24 * 60,
    );

    let (check_code, check_out, check_err) = run_edda(
        &["claim", "check", "src/main.rs"],
        repo.path(),
        store.path(),
    );
    assert_eq!(
        check_code, 0,
        "a two-month-old bare-CLI claim must not refuse a writer: {check_out} {check_err}"
    );

    let (peers_code, peers_out, peers_err) =
        run_edda(&["peers", "--json"], repo.path(), store.path());
    assert_eq!(
        peers_code, 0,
        "peers --json failed: {peers_out} {peers_err}"
    );
    let claim = reported_claim(&peers_out, "cli-two-months");
    assert_eq!(claim["stale"], true, "{claim}");
    assert_eq!(
        claim["blocks"], false,
        "past the TTL nothing keeps it standing: {claim}"
    );
    assert_eq!(
        check_code == 1,
        claim["blocks"].as_bool().expect("blocks is a bool"),
        "`blocks` must agree with `claim check` on the same claim"
    );
}
