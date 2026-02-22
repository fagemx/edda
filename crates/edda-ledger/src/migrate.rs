//! JSONL-to-SQLite migration for legacy edda workspaces.
//!
//! Reads `events.jsonl`, `refs/HEAD`, and `refs/branches.json`,
//! then writes them into a new `ledger.db` using `SqliteStore`.

use crate::paths::EddaPaths;
use crate::sqlite_store::SqliteStore;
use edda_core::Event;
use std::io::BufRead;
use std::path::Path;

/// Options controlling migration behavior.
pub struct MigrateOptions {
    /// Run post-migration verification (default: true).
    pub verify: bool,
    /// Report what would be migrated without making changes.
    pub dry_run: bool,
}

impl Default for MigrateOptions {
    fn default() -> Self {
        Self {
            verify: true,
            dry_run: false,
        }
    }
}

/// Summary of a completed (or dry-run) migration.
#[derive(Debug)]
pub struct MigrationReport {
    pub events_migrated: usize,
    pub decisions_found: usize,
    pub head_branch: String,
    pub branches_count: usize,
}

/// Migrate a legacy JSONL workspace to SQLite.
///
/// Pre-conditions:
/// - `.edda/ledger.db` must NOT exist (refuses to overwrite).
/// - `.edda/ledger/events.jsonl` must exist.
/// - `.edda/refs/HEAD` and `.edda/refs/branches.json` must exist.
///
/// On error, any partially-created `ledger.db` is cleaned up.
pub fn migrate_jsonl_to_sqlite(
    paths: &EddaPaths,
    opts: &MigrateOptions,
) -> anyhow::Result<MigrationReport> {
    // Pre-checks
    if paths.ledger_db.exists() {
        anyhow::bail!("ledger.db already exists — workspace already uses SQLite");
    }
    if !paths.events_jsonl.exists() {
        anyhow::bail!("events.jsonl not found — nothing to migrate");
    }

    // Read source data
    let events = read_jsonl_events(&paths.events_jsonl)?;
    let head = read_head(&paths.head_file)?;
    let branches = read_branches_json(&paths.branches_json)?;

    if opts.dry_run {
        return Ok(MigrationReport {
            events_migrated: events.len(),
            decisions_found: count_decisions(&events),
            head_branch: head,
            branches_count: count_branches(&branches),
        });
    }

    // Perform migration, clean up on failure
    let result = do_migration(paths, &events, &head, &branches, opts);

    if result.is_err() && paths.ledger_db.exists() {
        let _ = std::fs::remove_file(&paths.ledger_db);
        let wal = paths.ledger_db.with_extension("db-wal");
        let shm = paths.ledger_db.with_extension("db-shm");
        let _ = std::fs::remove_file(&wal);
        let _ = std::fs::remove_file(&shm);
    }

    result
}

fn do_migration(
    paths: &EddaPaths,
    events: &[Event],
    head: &str,
    branches: &serde_json::Value,
    opts: &MigrateOptions,
) -> anyhow::Result<MigrationReport> {
    let store = SqliteStore::open_or_create(&paths.ledger_db)?;

    for event in events {
        store.append_event(event)?;
    }

    store.set_head_branch(head)?;
    store.set_branches_json(branches)?;

    if opts.verify {
        verify_migration(&store, events, head, branches)?;
    }

    let decision_count = store.active_decisions(None, None)?.len();

    Ok(MigrationReport {
        events_migrated: events.len(),
        decisions_found: decision_count,
        head_branch: head.to_string(),
        branches_count: count_branches(branches),
    })
}

fn verify_migration(
    store: &SqliteStore,
    original_events: &[Event],
    head: &str,
    branches: &serde_json::Value,
) -> anyhow::Result<()> {
    // Verify event count and identity
    let sqlite_events = store.iter_events()?;
    if sqlite_events.len() != original_events.len() {
        anyhow::bail!(
            "event count mismatch: JSONL={}, SQLite={}",
            original_events.len(),
            sqlite_events.len()
        );
    }
    for (i, (orig, migrated)) in original_events.iter().zip(sqlite_events.iter()).enumerate() {
        if orig.event_id != migrated.event_id {
            anyhow::bail!("event_id mismatch at index {i}");
        }
        if orig.hash != migrated.hash {
            anyhow::bail!("hash mismatch at index {i} (event {})", orig.event_id);
        }
    }

    // Verify hash chain integrity
    for i in 1..sqlite_events.len() {
        let expected_parent = &sqlite_events[i - 1].hash;
        match &sqlite_events[i].parent_hash {
            Some(ph) if ph == expected_parent => {}
            Some(ph) => anyhow::bail!(
                "hash chain broken at index {i}: expected parent={}, got={}",
                expected_parent,
                ph
            ),
            None => anyhow::bail!("hash chain broken at index {i}: parent_hash is null"),
        }
    }

    // Verify refs
    let migrated_head = store.head_branch()?;
    if migrated_head != head {
        anyhow::bail!("HEAD mismatch: expected={head}, got={migrated_head}");
    }
    let migrated_branches = store.branches_json()?;
    if migrated_branches != *branches {
        anyhow::bail!("branches.json content mismatch");
    }

    Ok(())
}

// ── Helper functions ────────────────────────────────────────────────

fn read_jsonl_events(path: &Path) -> anyhow::Result<Vec<Event>> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut events = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event: Event = serde_json::from_str(&line)
            .map_err(|e| anyhow::anyhow!("failed to parse event at line {}: {e}", i + 1))?;
        events.push(event);
    }
    Ok(events)
}

fn read_head(path: &Path) -> anyhow::Result<String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("cannot read HEAD: {e}"))?;
    Ok(content.trim().to_string())
}

fn read_branches_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read branches.json: {e}"))?;
    Ok(serde_json::from_str(&content)?)
}

fn count_decisions(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| {
            e.event_type == "note"
                && e.payload
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().any(|t| t.as_str() == Some("decision")))
                    .unwrap_or(false)
        })
        .count()
}

fn count_branches(branches: &serde_json::Value) -> usize {
    branches
        .get("branches")
        .and_then(|v| v.as_object())
        .map(|m| m.len())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::EddaPaths;
    use edda_core::event::{finalize_event, new_note_event};
    use edda_core::types::Provenance;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Create a legacy JSONL workspace for testing.
    fn setup_jsonl_workspace() -> (std::path::PathBuf, EddaPaths) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp =
            std::env::temp_dir().join(format!("edda_migrate_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = EddaPaths::discover(&tmp);
        paths.ensure_layout().unwrap();
        std::fs::create_dir_all(&paths.refs_dir).unwrap();

        // Write HEAD
        std::fs::write(&paths.head_file, "main\n").unwrap();

        // Write branches.json
        let branches = serde_json::json!({
            "branches": {
                "main": { "created_at": "2026-01-01T00:00:00Z" }
            }
        });
        std::fs::write(
            &paths.branches_json,
            serde_json::to_string_pretty(&branches).unwrap(),
        )
        .unwrap();

        (tmp, paths)
    }

    /// Write events to events.jsonl as JSONL lines.
    fn write_jsonl_events(paths: &EddaPaths, events: &[Event]) {
        let mut content = String::new();
        for e in events {
            content.push_str(&serde_json::to_string(e).unwrap());
            content.push('\n');
        }
        std::fs::write(&paths.events_jsonl, content).unwrap();
    }

    fn make_decision_event(key: &str, value: &str, reason: &str) -> Event {
        let tags = vec!["decision".to_string()];
        let text = format!("{key}: {value} — {reason}");
        let mut event = new_note_event("main", None, "system", &text, &tags).unwrap();
        event.payload["decision"] = serde_json::json!({
            "key": key,
            "value": value,
            "reason": reason
        });
        finalize_event(&mut event);
        event
    }

    #[test]
    fn migrate_single_event() {
        let (tmp, paths) = setup_jsonl_workspace();
        let e = new_note_event("main", None, "system", "hello", &[]).unwrap();
        write_jsonl_events(&paths, &[e.clone()]);

        let opts = MigrateOptions {
            verify: true,
            dry_run: false,
        };
        let report = migrate_jsonl_to_sqlite(&paths, &opts).unwrap();

        assert_eq!(report.events_migrated, 1);
        assert_eq!(report.decisions_found, 0);
        assert_eq!(report.head_branch, "main");
        assert_eq!(report.branches_count, 1);

        // Verify via Ledger
        let ledger = crate::ledger::Ledger::open(&tmp).unwrap();
        assert!(ledger.is_sqlite());
        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, e.event_id);
        assert_eq!(events[0].hash, e.hash);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_hash_chain() {
        let (tmp, paths) = setup_jsonl_workspace();
        let e1 = new_note_event("main", None, "system", "first", &[]).unwrap();
        let e2 = new_note_event("main", Some(&e1.hash), "user", "second", &[]).unwrap();
        let e3 = new_note_event("main", Some(&e2.hash), "user", "third", &[]).unwrap();
        write_jsonl_events(&paths, &[e1.clone(), e2.clone(), e3.clone()]);

        let report = migrate_jsonl_to_sqlite(&paths, &MigrateOptions::default()).unwrap();
        assert_eq!(report.events_migrated, 3);

        // Verify chain via SQLite
        let ledger = crate::ledger::Ledger::open(&tmp).unwrap();
        let events = ledger.iter_events().unwrap();
        assert_eq!(events[0].parent_hash, None);
        assert_eq!(events[1].parent_hash.as_deref(), Some(e1.hash.as_str()));
        assert_eq!(events[2].parent_hash.as_deref(), Some(e2.hash.as_str()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_with_decisions() {
        let (tmp, paths) = setup_jsonl_workspace();
        let e1 = new_note_event("main", None, "system", "init", &[]).unwrap();
        let mut d1 = make_decision_event("db.engine", "postgres", "JSONB support");
        d1.parent_hash = Some(e1.hash.clone());
        finalize_event(&mut d1);
        let mut d2 = make_decision_event("auth.method", "JWT", "stateless");
        d2.parent_hash = Some(d1.hash.clone());
        finalize_event(&mut d2);
        write_jsonl_events(&paths, &[e1, d1, d2]);

        let report = migrate_jsonl_to_sqlite(&paths, &MigrateOptions::default()).unwrap();
        assert_eq!(report.events_migrated, 3);
        assert_eq!(report.decisions_found, 2);

        // Verify decisions
        let ledger = crate::ledger::Ledger::open(&tmp).unwrap();
        let decisions = ledger.active_decisions(None, None).unwrap();
        assert_eq!(decisions.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_refs() {
        let (tmp, paths) = setup_jsonl_workspace();
        let e = new_note_event("main", None, "system", "init", &[]).unwrap();
        write_jsonl_events(&paths, &[e]);

        migrate_jsonl_to_sqlite(&paths, &MigrateOptions::default()).unwrap();

        let ledger = crate::ledger::Ledger::open(&tmp).unwrap();
        assert_eq!(ledger.head_branch().unwrap(), "main");
        let bj = ledger.branches_json().unwrap();
        assert!(bj["branches"]["main"].is_object());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_already_sqlite_errors() {
        let (tmp, paths) = setup_jsonl_workspace();
        let e = new_note_event("main", None, "system", "init", &[]).unwrap();
        write_jsonl_events(&paths, &[e]);

        // First migration succeeds
        migrate_jsonl_to_sqlite(&paths, &MigrateOptions::default()).unwrap();

        // Second migration should fail
        let result = migrate_jsonl_to_sqlite(&paths, &MigrateOptions::default());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("already uses SQLite"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_missing_jsonl_errors() {
        let (tmp, paths) = setup_jsonl_workspace();
        // Don't write events.jsonl

        let result = migrate_jsonl_to_sqlite(&paths, &MigrateOptions::default());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_dry_run() {
        let (tmp, paths) = setup_jsonl_workspace();
        let e1 = new_note_event("main", None, "system", "init", &[]).unwrap();
        let d1 = make_decision_event("db", "postgres", "JSONB");
        write_jsonl_events(&paths, &[e1, d1]);

        let opts = MigrateOptions {
            verify: false,
            dry_run: true,
        };
        let report = migrate_jsonl_to_sqlite(&paths, &opts).unwrap();

        assert_eq!(report.events_migrated, 2);
        assert_eq!(report.decisions_found, 1);
        assert_eq!(report.head_branch, "main");

        // ledger.db should NOT exist
        assert!(!paths.ledger_db.exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_empty_jsonl() {
        let (tmp, paths) = setup_jsonl_workspace();
        // Write empty events.jsonl
        std::fs::write(&paths.events_jsonl, "").unwrap();

        let opts = MigrateOptions {
            verify: false,
            dry_run: false,
        };
        let report = migrate_jsonl_to_sqlite(&paths, &opts).unwrap();

        assert_eq!(report.events_migrated, 0);
        assert_eq!(report.decisions_found, 0);
        assert!(paths.ledger_db.exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_old_format_events() {
        let (tmp, paths) = setup_jsonl_workspace();

        // Simulate old-format event (no digests, no schema_version, no event_family)
        let old_json = r#"{"event_id":"evt_old","ts":"2026-01-01T00:00:00Z","type":"note","branch":"main","parent_hash":null,"hash":"abc123","payload":{"role":"user","text":"old event","tags":[]}}"#;
        std::fs::write(&paths.events_jsonl, format!("{old_json}\n")).unwrap();

        let opts = MigrateOptions {
            verify: false, // skip chain verification since hash is fake
            dry_run: false,
        };
        let report = migrate_jsonl_to_sqlite(&paths, &opts).unwrap();

        assert_eq!(report.events_migrated, 1);

        let ledger = crate::ledger::Ledger::open(&tmp).unwrap();
        let events = ledger.iter_events().unwrap();
        assert_eq!(events[0].event_id, "evt_old");
        assert_eq!(events[0].schema_version, 0);
        assert!(events[0].digests.is_empty());
        assert!(events[0].event_family.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_events_with_refs() {
        let (tmp, paths) = setup_jsonl_workspace();

        let mut e =
            new_note_event("main", None, "system", "with refs", &["decision".into()]).unwrap();
        e.refs.blobs = vec!["blob:sha256:abc123".to_string()];
        e.refs.events = vec!["evt_prior".to_string()];
        e.refs.provenance = vec![Provenance {
            target: "evt_old".to_string(),
            rel: "supersedes".to_string(),
            note: Some("re-decided".to_string()),
        }];
        finalize_event(&mut e);
        write_jsonl_events(&paths, &[e.clone()]);

        let opts = MigrateOptions {
            verify: false, // single event, no chain to verify
            dry_run: false,
        };
        let report = migrate_jsonl_to_sqlite(&paths, &opts).unwrap();
        assert_eq!(report.events_migrated, 1);

        let ledger = crate::ledger::Ledger::open(&tmp).unwrap();
        let events = ledger.iter_events().unwrap();
        assert_eq!(events[0].refs.blobs, vec!["blob:sha256:abc123"]);
        assert_eq!(events[0].refs.events, vec!["evt_prior"]);
        assert_eq!(events[0].refs.provenance.len(), 1);
        assert_eq!(events[0].refs.provenance[0].rel, "supersedes");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
