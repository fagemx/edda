//! `edda verify` — verify the ledger hash chain (GH-647).
//!
//! Connects the existing `Ledger::verify_chain()` capability (which previously
//! had no user-facing entrypoint) to a top-level CLI verb. Exit codes follow
//! the `claim-check.exit-codes=0/1/2` convention:
//!
//! - `0` — chain intact (including the empty ledger, which is OK, not an error)
//! - `1` — chain broken; the report names the first broken event
//! - `2` — the ledger could not be opened or read (not an edda workspace, or
//!   `.edda/ledger.db` missing/unreadable)
//!
//! The verification is read-only: this module opens the ledger via
//! `Ledger::open_existing()`, which never creates the database file, never
//! applies schema or migrations, and opens the connection `query_only` — a
//! missing or damaged ledger is reported (exit 2), never silently rebuilt
//! into an empty one that "verifies OK".

use anyhow::Result;
use edda_ledger::Ledger;
use std::path::Path;

pub fn execute(repo_root: &Path, json: bool) -> Result<()> {
    // `main` falls back to the cwd when no `.edda/` exists; refuse loudly
    // instead of opening a ledger that is not there (exit 2, per the
    // `claim-check.exit-codes=0/1/2` convention).
    if !repo_root.join(".edda").is_dir() {
        eprintln!(
            "error: not an edda workspace ({}): no .edda/ directory found. \
             Run `edda init` first, or cd into the workspace root.",
            repo_root.display()
        );
        std::process::exit(2);
    }

    // Existing/read-only open: `Ledger::open()` would create an empty ledger
    // when the DB file is missing and report it as a healthy empty chain.
    let ledger = match Ledger::open_existing(repo_root) {
        Ok(ledger) => ledger,
        Err(err) => {
            eprintln!(
                "error: cannot open ledger at {}: {:#}",
                repo_root.display(),
                err
            );
            std::process::exit(2);
        }
    };

    // A ledger that cannot be read at all (corrupt DB, missing schema) is
    // also a "cannot open" condition, not a broken chain: exit 2, never 0.
    let report = match ledger.verify_chain_report() {
        Ok(report) => report,
        Err(err) => {
            eprintln!(
                "error: cannot read ledger at {}: {:#}",
                repo_root.display(),
                err
            );
            std::process::exit(2);
        }
    };

    if json {
        let payload = serde_json::json!({
            "ok": report.first_bad_event.is_none(),
            "events": report.events,
            "first_bad_event": report.first_bad_event,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).expect("serialize verify report")
        );
    } else if let Some(bad) = &report.first_bad_event {
        println!(
            "ledger chain BROKEN at event {} ({} event(s) scanned): {}",
            bad,
            report.events,
            report.reason.as_deref().unwrap_or("integrity check failed")
        );
    } else if report.events == 0 {
        println!("ledger chain OK: empty ledger (0 event(s))");
    } else {
        println!(
            "ledger chain OK: {} event(s), last event {}",
            report.events,
            report.last_event_id.as_deref().unwrap_or("?")
        );
    }

    // An intact chain exits 0; a broken one must never masquerade as success.
    if report.first_bad_event.is_some() {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::new_note_event;
    use serde_json::Value;
    use std::path::PathBuf;

    /// Path to the `edda` binary cargo just built for this test run
    /// (`current_exe` = `target/debug/deps/<test>-<hash>.exe`).
    fn edda_bin() -> PathBuf {
        let exe = std::env::current_exe().expect("current_exe");
        let dir = exe
            .parent()
            .and_then(|d| d.parent())
            .expect("deps/.. = target/debug")
            .to_path_buf();
        dir.join(format!("edda{}", std::env::consts::EXE_SUFFIX))
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
    fn verify_outside_edda_repo_exits_2_with_explanation() {
        let repo = tempfile::tempdir().expect("repo tempdir");
        // No `.edda/` anywhere — `edda verify` must refuse, not fabricate OK.
        let (code, stdout, stderr) = run_edda(&["verify"], repo.path());
        assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
        let report = format!("{stdout}{stderr}");
        assert!(
            !report.contains("OK"),
            "must not report success outside a repo: {report:?}"
        );
        assert!(
            report.contains("workspace"),
            "must explain it is not an edda workspace: {report:?}"
        );
    }
}
