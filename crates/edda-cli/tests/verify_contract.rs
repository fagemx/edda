//! Integration tests for `edda verify` (GH-646, GH-647, GH-651, GH-789).
//!
//! Spawns the compiled `edda` binary (`CARGO_BIN_EXE_edda`) against temporary
//! ledgers and repositories.

use edda_core::event::new_note_event;
use edda_ledger::Ledger;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Path to the `edda` binary cargo just built for this test run
/// (`current_exe` = `target/debug/deps/<test>-<hash>.exe`).
fn edda_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_edda"))
}

/// Run `edda verify` in `repo` and return (exit code, stdout, stderr).
fn run_edda(args: &[&str], repo: &Path) -> (i32, String, String) {
    assert!(edda_bin().exists(), "edda binary not found");
    let out = std::process::Command::new(edda_bin())
        .args(args)
        .current_dir(repo)
        .output()
        .expect("spawn edda");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A real SQLite ledger with a two-event hash chain (tempfile, no mocks).
fn seeded_ledger(repo: &Path) {
    let ledger = Ledger::open_or_init(repo).expect("open_or_init");
    let e1 = new_note_event("main", None, "system", "first event", &[]).expect("e1");
    ledger.append_event(&e1).expect("append e1");
    let e2 = new_note_event("main", Some(&e1.hash), "system", "second event", &[]).expect("e2");
    ledger.append_event(&e2).expect("append e2");
    drop(ledger);
}

/// Tamper with a real ledger row via raw SQL, bypassing the append path.
fn tamper(repo: &Path, event_id: &str, sql: &str) {
    let conn = rusqlite::Connection::open(repo.join(".edda").join("ledger.db"))
        .expect("open ledger.db for tampering");
    conn.execute(sql, rusqlite::params![event_id])
        .expect("tamper update");
}

/// Event ids of the seeded chain, in insertion order.
fn seeded_event_ids(repo: &Path) -> Vec<String> {
    let ledger = Ledger::open(repo).expect("reopen");
    ledger
        .iter_events()
        .expect("events")
        .into_iter()
        .map(|e| e.event_id)
        .collect()
}

/// GH-651 golden fixture for the `edda verify --json` stable contract
/// (ledger decision `compat.stable-json-surfaces`; policy page:
/// COMPATIBILITY.md § "Stable `--json` contracts"). Within 0.x, keys may
/// be added, never deleted, renamed, or retyped. Pins the exact key set
/// and per-key types on both the clean (`first_bad_event: null`) and the
/// broken (`first_bad_event: string`) side, through the real binary.
#[test]
fn compat_golden_fixture_verify_json_keys_and_types() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    seeded_ledger(repo.path());

    let (code, stdout, stderr) = run_edda(&["verify", "--json"], repo.path());
    assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
    let v: Value = serde_json::from_str(&stdout).expect("valid JSON");

    let mut keys: Vec<&str> = v
        .as_object()
        .expect("one JSON object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["events", "first_bad_event", "ok"],
        "verify --json key set changed — this is a stable contract; \
         see COMPATIBILITY.md"
    );
    assert_eq!(v["ok"], Value::Bool(true));
    assert!(v["events"].is_u64(), "events must be an integer: {v}");
    assert_eq!(v["events"], Value::from(2));
    assert_eq!(v["first_bad_event"], Value::Null);

    // Broken side: same key set, `ok` flips to false and
    // `first_bad_event` names the first broken event as a string.
    let e2 = &seeded_event_ids(repo.path())[1];
    tamper(
        repo.path(),
        e2,
        "UPDATE events SET payload = replace(payload, 'second event', 'tampered') \
         WHERE event_id = ?1",
    );
    let (code, stdout, stderr) = run_edda(&["verify", "--json"], repo.path());
    assert_eq!(code, 1, "stdout={stdout:?} stderr={stderr:?}");
    let v: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let mut keys: Vec<&str> = v
        .as_object()
        .expect("one JSON object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["events", "first_bad_event", "ok"],
        "broken-side key set must match the clean side"
    );
    assert_eq!(v["ok"], Value::Bool(false));
    assert_eq!(v["first_bad_event"], Value::String(e2.clone()));
    assert!(v["events"].is_u64(), "events must be an integer: {v}");
}

#[test]
fn verify_clean_ledger_is_ok_and_exits_0() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    seeded_ledger(repo.path());
    let (code, stdout, stderr) = run_edda(&["verify"], repo.path());
    assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
    assert!(stdout.contains("OK"), "stdout={stdout:?}");
    assert!(stdout.contains('2'), "must report event count: {stdout:?}");
}

#[test]
fn verify_empty_ledger_is_ok_not_an_error() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    let ledger = Ledger::open_or_init(repo.path()).expect("open_or_init");
    drop(ledger);
    let (code, stdout, stderr) = run_edda(&["verify"], repo.path());
    assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
    assert!(stdout.contains("OK"), "stdout={stdout:?}");
}

/// P0 (GH-647 round 1): a vanished ledger must be reported (exit 2),
/// never silently rebuilt as an empty one that "verifies OK".
#[test]
fn verify_deleted_ledger_db_exits_2_and_is_not_recreated() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    seeded_ledger(repo.path());
    std::fs::remove_file(repo.path().join(".edda").join("ledger.db"))
        .expect("delete ledger.db, keeping .edda/");

    let (code, stdout, stderr) = run_edda(&["verify"], repo.path());
    assert_eq!(
        code, 2,
        "missing ledger must exit 2, not rebuild an empty one: {stdout:?} {stderr:?}"
    );
    let report = format!("{stdout}{stderr}");
    assert!(
        report.contains("ledger"),
        "must explain the ledger problem: {report:?}"
    );
    assert!(
        !report.contains("OK"),
        "must not report success for a vanished ledger: {report:?}"
    );
    assert!(
        !repo.path().join(".edda").join("ledger.db").exists(),
        "verify must never recreate the ledger database"
    );
}

#[test]
fn verify_tampered_payload_fails_with_first_bad_event_and_exit_1() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    seeded_ledger(repo.path());
    let e2 = &seeded_event_ids(repo.path())[1];

    tamper(
        repo.path(),
        e2,
        "UPDATE events SET payload = replace(payload, 'second event', 'tampered') \
         WHERE event_id = ?1",
    );

    let (code, stdout, stderr) = run_edda(&["verify"], repo.path());
    assert_eq!(
        code, 1,
        "broken chain must not exit 0: {stdout:?} {stderr:?}"
    );
    let report = format!("{stdout}{stderr}");
    assert!(
        report.contains(e2),
        "must name first broken event {e2}: {report:?}"
    );
}

/// P1 (GH-647 round 1): a payload that is not valid JSON at all must
/// still be reported as the first broken event, by id.
#[test]
fn verify_malformed_payload_fails_with_first_bad_event_and_exit_1() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    seeded_ledger(repo.path());
    let e2 = &seeded_event_ids(repo.path())[1];

    tamper(
        repo.path(),
        e2,
        "UPDATE events SET payload = 'not-json' WHERE event_id = ?1",
    );

    let (code, stdout, stderr) = run_edda(&["verify"], repo.path());
    assert_eq!(
        code, 1,
        "malformed payload must not exit 0: {stdout:?} {stderr:?}"
    );
    let report = format!("{stdout}{stderr}");
    assert!(
        report.contains(e2),
        "must name first broken event {e2} even when the payload is not JSON: {report:?}"
    );
}

#[test]
fn verify_broken_parent_link_fails_with_first_bad_event_and_exit_1() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    seeded_ledger(repo.path());
    let e2 = &seeded_event_ids(repo.path())[1];

    tamper(
        repo.path(),
        e2,
        "UPDATE events SET parent_hash = 'sha256:bogus' WHERE event_id = ?1",
    );

    let (code, stdout, stderr) = run_edda(&["verify"], repo.path());
    assert_eq!(
        code, 1,
        "broken chain must not exit 0: {stdout:?} {stderr:?}"
    );
    let report = format!("{stdout}{stderr}");
    assert!(
        report.contains(e2),
        "must name first broken event {e2}: {report:?}"
    );
}

#[test]
fn verify_json_output_has_stable_shape_on_clean_ledger() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    seeded_ledger(repo.path());
    let (code, stdout, stderr) = run_edda(&["verify", "--json"], repo.path());
    assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
    let v: Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["ok"], Value::Bool(true), "json={v}");
    assert_eq!(v["events"], Value::from(2), "json={v}");
    assert_eq!(v["first_bad_event"], Value::Null, "json={v}");
}

#[test]
fn verify_json_output_names_first_bad_event_when_broken() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    seeded_ledger(repo.path());
    let e2 = &seeded_event_ids(repo.path())[1];

    tamper(
        repo.path(),
        e2,
        "UPDATE events SET payload = replace(payload, 'second event', 'tampered') \
         WHERE event_id = ?1",
    );

    let (code, stdout, stderr) = run_edda(&["verify", "--json"], repo.path());
    assert_eq!(code, 1, "stdout={stdout:?} stderr={stderr:?}");
    let v: Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["ok"], Value::Bool(false), "json={v}");
    assert_eq!(v["first_bad_event"], Value::String(e2.clone()), "json={v}");
}

#[test]
fn verify_anchored_workspace_with_unopenable_ledger_exits_2_and_never_reports_ok() {
    // GH-646 hermetic isolation: `find_root` climbs parents, so a bare
    // tempdir would silently resolve to whatever workspace exists above
    // %TEMP% (the fleet coordination workspace in $HOME) and `edda
    // verify` would judge THAT ledger. The "no `.edda/` anywhere above"
    // premise cannot be established by a spawned binary, so anchor the
    // climb at this test's own directory and make the ledger
    // deterministically unopenable: `.edda/` exists (find_root stops
    // here) but `ledger.db` is a directory (SQLite cannot open it).
    // Verify must refuse with exit 2 — never fabricate OK.
    let repo = tempfile::tempdir().expect("repo tempdir");
    std::fs::create_dir_all(repo.path().join(".edda").join("ledger.db"))
        .expect("anchored workspace with directory-shaped ledger.db");
    let (code, stdout, stderr) = run_edda(&["verify"], repo.path());
    assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
    let report = format!("{stdout}{stderr}");
    assert!(
        !report.contains("OK"),
        "must not report success on an unopenable ledger: {report:?}"
    );
    assert!(
        report.contains("cannot open ledger"),
        "must explain the ledger cannot be opened: {report:?}"
    );
}
