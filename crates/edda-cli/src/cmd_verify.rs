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
            if let Some(e) = err.downcast_ref::<edda_ledger::UnsupportedSchemaVersionError>() {
                eprintln!("error: {e}");
                std::process::exit(2);
            }
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
