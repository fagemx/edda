//! SQLite-backed storage for the edda ledger.
//!
//! Replaces the file-based storage (events.jsonl, refs/HEAD, refs/branches.json)
//! with a single `ledger.db` SQLite file using WAL mode.

mod decisions;
mod dependencies;
mod entities;
mod events;
mod mappers;
mod schema;
pub mod types;
mod village;

pub use types::*;
pub use village::detect_trend_direction;

use rusqlite::Connection;
use std::path::Path;

/// Map a decision status string to the legacy is_active boolean.
///
/// `is_active = true` iff status is "active" or "experimental".
/// This enforces CONTRACT COMPAT-01.
fn status_to_is_active(status: &str) -> bool {
    matches!(status, "active" | "experimental")
}

/// SQLite-backed storage engine.
pub struct SqliteStore {
    pub(crate) conn: Connection,
}

impl SqliteStore {
    /// Open an existing ledger.db.
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(db_path)?;
        let store = Self { conn };
        store.apply_pragmas()?;
        Ok(store)
    }

    /// Open or create ledger.db with full schema.
    pub fn open_or_create(db_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        let store = Self { conn };
        store.apply_pragmas()?;
        store.apply_schema()?;
        Ok(store)
    }

    fn apply_pragmas(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(())
    }
}

impl Drop for SqliteStore {
    fn drop(&mut self) {
        // Merge WAL back into main DB so users see a single file when idle.
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}

#[cfg(test)]
mod tests {
    use super::mappers::time_now_rfc3339;
    use super::schema::*;
    use super::*;
    use edda_core::event::new_note_event;
    use edda_core::types::{Event, Provenance, Refs};
    use rusqlite::params;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_db() -> (std::path::PathBuf, SqliteStore) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("edda_sqlite_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("ledger.db");
        let store = SqliteStore::open_or_create(&db_path).unwrap();
        (dir, store)
    }
    #[test]
    fn schema_creation() {
        let (dir, store) = tmp_db();
        // Verify tables exist
        let tables: Vec<String> = store
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(tables.contains(&"events".to_string()));
        assert!(tables.contains(&"refs".to_string()));
        assert!(tables.contains(&"schema_meta".to_string()));
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn event_round_trip() {
        let (dir, store) = tmp_db();
        let e1 = new_note_event("main", None, "system", "first note", &["test".into()]).unwrap();
        store.append_event(&e1).unwrap();

        let e2 = new_note_event(
            "main",
            Some(&e1.hash),
            "user",
            "second note",
            &["test".into()],
        )
        .unwrap();
        store.append_event(&e2).unwrap();

        let events = store.iter_events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_id, e1.event_id);
        assert_eq!(events[0].hash, e1.hash);
        assert_eq!(events[0].event_type, "note");
        assert_eq!(events[0].branch, "main");
        assert_eq!(events[1].event_id, e2.event_id);
        assert_eq!(events[1].parent_hash, Some(e1.hash.clone()));

        // Payload preserved
        assert_eq!(events[0].payload["text"], "first note");
        assert_eq!(events[1].payload["text"], "second note");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_event_hash_empty() {
        let (dir, store) = tmp_db();
        assert_eq!(store.last_event_hash().unwrap(), None);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_event_hash_returns_latest() {
        let (dir, store) = tmp_db();
        let e1 = new_note_event("main", None, "system", "init", &[]).unwrap();
        store.append_event(&e1).unwrap();
        assert_eq!(store.last_event_hash().unwrap(), Some(e1.hash.clone()));

        let e2 = new_note_event("main", Some(&e1.hash), "user", "hello", &[]).unwrap();
        store.append_event(&e2).unwrap();
        assert_eq!(store.last_event_hash().unwrap(), Some(e2.hash.clone()));

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refs_head_branch() {
        let (dir, store) = tmp_db();
        // HEAD not set yet
        assert!(store.head_branch().is_err());

        store.set_head_branch("main").unwrap();
        assert_eq!(store.head_branch().unwrap(), "main");

        store.set_head_branch("feat/x").unwrap();
        assert_eq!(store.head_branch().unwrap(), "feat/x");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refs_branches_json() {
        let (dir, store) = tmp_db();
        let json = serde_json::json!({
            "branches": {
                "main": { "created_at": "2026-01-01T00:00:00Z" }
            }
        });
        store.set_branches_json(&json).unwrap();
        let loaded = store.branches_json().unwrap();
        assert_eq!(loaded, json);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn event_with_refs() {
        let (dir, store) = tmp_db();
        let mut e =
            new_note_event("main", None, "system", "with refs", &["decision".into()]).unwrap();
        e.refs.blobs = vec!["blob:sha256:abc123".to_string()];
        e.refs.events = vec!["evt_prior".to_string()];
        e.refs.provenance = vec![Provenance {
            target: "evt_old".to_string(),
            rel: "supersedes".to_string(),
            note: Some("re-decided".to_string()),
        }];

        store.append_event(&e).unwrap();
        let events = store.iter_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].refs.blobs, vec!["blob:sha256:abc123"]);
        assert_eq!(events[0].refs.events, vec!["evt_prior"]);
        assert_eq!(events[0].refs.provenance.len(), 1);
        assert_eq!(events[0].refs.provenance[0].rel, "supersedes");
        assert_eq!(
            events[0].refs.provenance[0].note.as_deref(),
            Some("re-decided")
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wal_checkpoint_on_drop() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("edda_sqlite_wal_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("ledger.db");

        {
            let store = SqliteStore::open_or_create(&db_path).unwrap();
            let e = new_note_event("main", None, "system", "wal test", &[]).unwrap();
            store.append_event(&e).unwrap();
            // Drop triggers checkpoint
        }

        // After drop, WAL should be checkpointed (file may still exist but be empty)
        assert!(db_path.exists());
        let wal_path = dir.join("ledger.db-wal");
        if wal_path.exists() {
            // WAL file exists but should be 0 bytes after TRUNCATE checkpoint
            let size = std::fs::metadata(&wal_path).unwrap().len();
            assert_eq!(size, 0, "WAL file should be empty after checkpoint");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn event_ordering_preserved() {
        let (dir, store) = tmp_db();
        let mut prev_hash: Option<String> = None;
        for i in 0..10 {
            let e = new_note_event(
                "main",
                prev_hash.as_deref(),
                "system",
                &format!("event {i}"),
                &[],
            )
            .unwrap();
            prev_hash = Some(e.hash.clone());
            store.append_event(&e).unwrap();
        }

        let events = store.iter_events().unwrap();
        assert_eq!(events.len(), 10);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.payload["text"], format!("event {i}"));
        }
        // Verify hash chain
        for i in 1..10 {
            assert_eq!(
                events[i].parent_hash.as_deref(),
                Some(events[i - 1].hash.as_str())
            );
        }

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_event_id_errors() {
        let (dir, store) = tmp_db();
        let e = new_note_event("main", None, "system", "first", &[]).unwrap();
        store.append_event(&e).unwrap();
        // Same event_id should fail (UNIQUE constraint)
        assert!(store.append_event(&e).is_err());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn idempotent_schema_apply() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("edda_sqlite_idem_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("ledger.db");

        // Create twice — should not error
        let store1 = SqliteStore::open_or_create(&db_path).unwrap();
        store1.set_head_branch("main").unwrap();
        drop(store1);

        let store2 = SqliteStore::open_or_create(&db_path).unwrap();
        assert_eq!(store2.head_branch().unwrap(), "main");
        drop(store2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Decision tests ──────────────────────────────────────────────

    fn make_decision_event(
        branch: &str,
        key: &str,
        value: &str,
        reason: Option<&str>,
        supersedes: Option<&str>,
    ) -> Event {
        use edda_core::event::finalize_event;

        let text = match reason {
            Some(r) => format!("{key}: {value} — {r}"),
            None => format!("{key}: {value}"),
        };
        let tags = vec!["decision".to_string()];
        let mut event = new_note_event(branch, None, "system", &text, &tags).unwrap();

        // Inject structured decision object
        let decision_obj = match reason {
            Some(r) => serde_json::json!({"key": key, "value": value, "reason": r}),
            None => serde_json::json!({"key": key, "value": value}),
        };
        event.payload["decision"] = decision_obj;

        if let Some(target) = supersedes {
            event.refs.provenance.push(Provenance {
                target: target.to_string(),
                rel: "supersedes".to_string(),
                note: Some(format!("key '{key}' re-decided")),
            });
        }

        finalize_event(&mut event).unwrap();
        event
    }

    #[test]
    fn decision_materialized_on_append() {
        let (dir, store) = tmp_db();
        let e = make_decision_event("main", "db.engine", "postgres", Some("JSONB support"), None);
        store.append_event(&e).unwrap();

        let active = store
            .active_decisions(None, None, None, None, None)
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].key, "db.engine");
        assert_eq!(active[0].value, "postgres");
        assert_eq!(active[0].reason, "JSONB support");
        assert_eq!(active[0].domain, "db");
        assert_eq!(active[0].branch, "main");
        assert!(active[0].is_active);
        assert!(active[0].supersedes_id.is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn supersede_deactivates_prior() {
        let (dir, store) = tmp_db();
        let d1 = make_decision_event("main", "db.engine", "mysql", None, None);
        let d1_id = d1.event_id.clone();
        store.append_event(&d1).unwrap();

        let d2 = make_decision_event("main", "db.engine", "postgres", Some("JSONB"), Some(&d1_id));
        store.append_event(&d2).unwrap();

        let active = store
            .active_decisions(None, None, None, None, None)
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].value, "postgres");
        assert_eq!(active[0].supersedes_id.as_deref(), Some(d1_id.as_str()));

        // Timeline should show both
        let timeline = store.decision_timeline("db.engine", None, None).unwrap();
        assert_eq!(timeline.len(), 2);
        assert!(!timeline[0].is_active); // mysql deactivated
        assert!(timeline[1].is_active); // postgres active

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn domain_auto_extracted() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event(
                "main",
                "db.engine",
                "postgres",
                None,
                None,
            ))
            .unwrap();
        store
            .append_event(&make_decision_event(
                "main",
                "db.pool_size",
                "10",
                None,
                None,
            ))
            .unwrap();
        store
            .append_event(&make_decision_event(
                "main",
                "auth.method",
                "JWT",
                None,
                None,
            ))
            .unwrap();

        let db_decisions = store
            .active_decisions(Some("db"), None, None, None, None)
            .unwrap();
        assert_eq!(db_decisions.len(), 2);

        let auth_decisions = store
            .active_decisions(Some("auth"), None, None, None, None)
            .unwrap();
        assert_eq!(auth_decisions.len(), 1);
        assert_eq!(auth_decisions[0].key, "auth.method");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_text_only_decision() {
        let (dir, store) = tmp_db();
        // Old-format event: no payload.decision field, only text
        use edda_core::event::finalize_event;

        let tags = vec!["decision".to_string()];
        let mut event = new_note_event(
            "main",
            None,
            "system",
            "orm: sqlx — compile-time checks",
            &tags,
        )
        .unwrap();
        // Do NOT add payload.decision — simulate legacy format
        // Remove it if new_note_event somehow adds it (it doesn't)
        event.payload.as_object_mut().unwrap().remove("decision");
        finalize_event(&mut event).unwrap();
        store.append_event(&event).unwrap();

        let active = store
            .active_decisions(None, None, None, None, None)
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].key, "orm");
        assert_eq!(active[0].value, "sqlx");
        assert_eq!(active[0].reason, "compile-time checks");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn active_decisions_key_pattern_search() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event(
                "main",
                "db.engine",
                "postgres",
                None,
                None,
            ))
            .unwrap();
        store
            .append_event(&make_decision_event(
                "main",
                "auth.method",
                "JWT",
                None,
                None,
            ))
            .unwrap();
        store
            .append_event(&make_decision_event(
                "main",
                "cache.driver",
                "redis",
                None,
                None,
            ))
            .unwrap();

        // Search by key/value pattern
        let results = store
            .active_decisions(None, Some("postgres"), None, None, None)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "db.engine");

        let results = store
            .active_decisions(None, Some("auth"), None, None, None)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "auth.method");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_active_decision_by_branch_key() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event(
                "main",
                "db.engine",
                "postgres",
                None,
                None,
            ))
            .unwrap();

        let found = store.find_active_decision("main", "db.engine").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().value, "postgres");

        let not_found = store.find_active_decision("main", "db.pool_size").unwrap();
        assert!(not_found.is_none());

        let wrong_branch = store.find_active_decision("dev", "db.engine").unwrap();
        assert!(wrong_branch.is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn branch_scoped_supersession() {
        let (dir, store) = tmp_db();
        // Same key on different branches — both should stay active
        store
            .append_event(&make_decision_event(
                "main",
                "db.engine",
                "postgres",
                None,
                None,
            ))
            .unwrap();
        store
            .append_event(&make_decision_event(
                "dev",
                "db.engine",
                "sqlite",
                None,
                None,
            ))
            .unwrap();

        let all = store
            .active_decisions(None, None, None, None, None)
            .unwrap();
        assert_eq!(all.len(), 2);

        let main = store
            .find_active_decision("main", "db.engine")
            .unwrap()
            .unwrap();
        assert_eq!(main.value, "postgres");

        let dev = store
            .find_active_decision("dev", "db.engine")
            .unwrap()
            .unwrap();
        assert_eq!(dev.value, "sqlite");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_migration_v1_to_v2() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("edda_sqlite_migrate_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("ledger.db");

        // Create a v1 database manually (only base schema)
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(SCHEMA_SQL).unwrap();
            conn.execute(
                "INSERT INTO schema_meta (key, value) VALUES ('version', '1')",
                [],
            )
            .unwrap();

            // Insert a decision event directly into v1 events table
            conn.execute(
                "INSERT INTO events (event_id, ts, event_type, branch, hash, payload, schema_version)
                 VALUES ('evt_v1', '2026-01-01T00:00:00Z', 'note', 'main', 'abc', ?1, 1)",
                params![serde_json::to_string(&serde_json::json!({
                    "role": "system",
                    "text": "db.engine: postgres — need JSONB",
                    "tags": ["decision"],
                    "decision": {"key": "db.engine", "value": "postgres", "reason": "need JSONB"}
                })).unwrap()],
            ).unwrap();
        }

        // Open with open_or_create — should trigger v1→v2 migration
        let store = SqliteStore::open_or_create(&db_path).unwrap();
        assert!(store.schema_version().unwrap() >= 2);

        // Verify decisions table was populated by backfill
        let active = store
            .active_decisions(None, None, None, None, None)
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].key, "db.engine");
        assert_eq!(active[0].value, "postgres");
        assert_eq!(active[0].domain, "db");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_decision_event_not_materialized() {
        let (dir, store) = tmp_db();
        // Regular note (no decision tag)
        let e = new_note_event("main", None, "system", "just a note", &["todo".into()]).unwrap();
        store.append_event(&e).unwrap();

        let active = store
            .active_decisions(None, None, None, None, None)
            .unwrap();
        assert!(active.is_empty());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn domain_timeline_returns_active_and_superseded() {
        let (dir, store) = tmp_db();
        let d1 = make_decision_event("main", "db.engine", "sqlite", Some("MVP"), None);
        let d1_id = d1.event_id.clone();
        store.append_event(&d1).unwrap();

        let d2 = make_decision_event("main", "db.engine", "postgres", Some("JSONB"), Some(&d1_id));
        store.append_event(&d2).unwrap();

        // Also add a decision in a different domain
        store
            .append_event(&make_decision_event(
                "main",
                "auth.method",
                "JWT",
                None,
                None,
            ))
            .unwrap();

        let timeline = store.domain_timeline("db", None, None).unwrap();
        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0].value, "sqlite");
        assert!(!timeline[0].is_active);
        assert_eq!(timeline[1].value, "postgres");
        assert!(timeline[1].is_active);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn domain_timeline_empty_for_unknown_domain() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event(
                "main",
                "db.engine",
                "postgres",
                None,
                None,
            ))
            .unwrap();

        let timeline = store.domain_timeline("nonexistent", None, None).unwrap();
        assert!(timeline.is_empty());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_domains_returns_sorted_unique() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event(
                "main",
                "db.engine",
                "postgres",
                None,
                None,
            ))
            .unwrap();
        store
            .append_event(&make_decision_event(
                "main",
                "db.pool_size",
                "10",
                None,
                None,
            ))
            .unwrap();
        store
            .append_event(&make_decision_event(
                "main",
                "auth.method",
                "JWT",
                None,
                None,
            ))
            .unwrap();

        let domains = store.list_domains().unwrap();
        assert_eq!(domains, vec!["auth", "db"]);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_domains_excludes_superseded_only() {
        let (dir, store) = tmp_db();
        let d1 = make_decision_event("main", "cache.strategy", "redis", None, None);
        let d1_id = d1.event_id.clone();
        store.append_event(&d1).unwrap();

        // Supersede with a different domain key
        let d2 = make_decision_event("main", "cache.strategy", "memcached", None, Some(&d1_id));
        store.append_event(&d2).unwrap();

        // "cache" should still appear (d2 is active)
        let domains = store.list_domains().unwrap();
        assert!(domains.contains(&"cache".to_string()));

        // Now supersede d2 but with NO replacement active
        // We can't easily do this with current API, so instead test
        // that a domain with only superseded decisions is excluded:
        // Create a new domain, supersede its only decision, check it disappears
        let d3 = make_decision_event("main", "temp.flag", "on", None, None);
        let d3_id = d3.event_id.clone();
        store.append_event(&d3).unwrap();
        assert!(store.list_domains().unwrap().contains(&"temp".to_string()));

        let d4 = make_decision_event("main", "temp.flag", "off", None, Some(&d3_id));
        store.append_event(&d4).unwrap();
        // "temp" still has active decision (d4)
        assert!(store.list_domains().unwrap().contains(&"temp".to_string()));

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decisions_table_in_schema() {
        let (dir, store) = tmp_db();
        let tables: Vec<String> = store
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(tables.contains(&"decisions".to_string()));
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Review bundle tests ────────────────────────────────────────

    fn make_bundle_event(branch: &str, bundle_id: &str, risk: &str, failed: u32) -> Event {
        use edda_core::bundle::*;
        use edda_core::event::{new_review_bundle_event, ReviewBundleParams};

        let bundle = ReviewBundle {
            bundle_id: bundle_id.into(),
            change_summary: ChangeSummary {
                files: vec![FileChange {
                    path: "src/main.rs".into(),
                    added: 10,
                    deleted: 3,
                }],
                total_added: 10,
                total_deleted: 3,
                diff_ref: "HEAD~1".into(),
            },
            test_results: TestResults {
                passed: 50,
                failed,
                ignored: 0,
                total: 50 + failed,
                failures: if failed > 0 {
                    vec!["some::test".into()]
                } else {
                    vec![]
                },
                command: "cargo test".into(),
            },
            risk_assessment: RiskAssessment {
                level: match risk {
                    "low" => RiskLevel::Low,
                    "medium" => RiskLevel::Medium,
                    "high" => RiskLevel::High,
                    _ => RiskLevel::Critical,
                },
                factors: vec![],
            },
            suggested_action: if failed > 0 {
                SuggestedAction::Reject
            } else {
                SuggestedAction::Approve
            },
            suggested_reason: "test".into(),
        };

        new_review_bundle_event(&ReviewBundleParams {
            branch: branch.into(),
            parent_hash: None,
            bundle,
        })
        .unwrap()
    }

    #[test]
    fn review_bundle_materialized_on_append() {
        let (dir, store) = tmp_db();
        let e = make_bundle_event("main", "bun_test1", "low", 0);
        store.append_event(&e).unwrap();

        let bundles = store.list_bundles(None).unwrap();
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].bundle_id, "bun_test1");
        assert_eq!(bundles[0].status, "pending");
        assert_eq!(bundles[0].risk_level, "low");
        assert_eq!(bundles[0].total_added, 10);
        assert_eq!(bundles[0].tests_passed, 50);
        assert_eq!(bundles[0].tests_failed, 0);
        assert_eq!(bundles[0].suggested_action, "approve");
        assert_eq!(bundles[0].branch, "main");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_bundle_by_id() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_bundle_event("main", "bun_abc", "high", 1))
            .unwrap();

        let found = store.get_bundle("bun_abc").unwrap();
        assert!(found.is_some());
        let b = found.unwrap();
        assert_eq!(b.risk_level, "high");
        assert_eq!(b.tests_failed, 1);
        assert_eq!(b.suggested_action, "reject");

        let not_found = store.get_bundle("bun_nonexistent").unwrap();
        assert!(not_found.is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_bundles_filter_by_status() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_bundle_event("main", "bun_1", "low", 0))
            .unwrap();
        store
            .append_event(&make_bundle_event("main", "bun_2", "medium", 0))
            .unwrap();

        // All are "pending" initially
        let all = store.list_bundles(None).unwrap();
        assert_eq!(all.len(), 2);

        let pending = store.list_bundles(Some("pending")).unwrap();
        assert_eq!(pending.len(), 2);

        let approved = store.list_bundles(Some("approved")).unwrap();
        assert!(approved.is_empty());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review_bundles_table_in_schema() {
        let (dir, store) = tmp_db();
        let tables: Vec<String> = store
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(tables.contains(&"review_bundles".to_string()));
        assert!(tables.contains(&"decision_deps".to_string()));
        assert!(tables.contains(&"task_briefs".to_string()));
        assert!(tables.contains(&"device_tokens".to_string()));
        assert!(tables.contains(&"decide_snapshots".to_string()));
        assert!(tables.contains(&"suggestions".to_string()));
        assert_eq!(store.schema_version().unwrap(), 12);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Decision dependency tests ───────────────────────────────────

    #[test]
    fn test_insert_and_query_deps() {
        let (dir, store) = tmp_db();
        store
            .insert_dep("db.schema", "db.engine", "explicit", Some("evt_1"))
            .unwrap();
        store
            .insert_dep("db.pool", "db.engine", "auto_domain", Some("evt_2"))
            .unwrap();

        let deps = store.deps_of("db.schema").unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].target_key, "db.engine");
        assert_eq!(deps[0].dep_type, "explicit");

        let dependents = store.dependents_of("db.engine").unwrap();
        assert_eq!(dependents.len(), 2);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_active_dependents_join() {
        let (dir, store) = tmp_db();
        // Create two decisions in same domain
        let d1 = make_decision_event("main", "db.engine", "postgres", Some("JSONB"), None);
        store.append_event(&d1).unwrap();
        let d2 = make_decision_event("main", "db.schema", "JSONB", Some("postgres feature"), None);
        store.append_event(&d2).unwrap();

        // Add explicit dep
        store
            .insert_dep("db.schema", "db.engine", "explicit", Some(&d2.event_id))
            .unwrap();

        let active = store.active_dependents_of("db.engine").unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].1.key, "db.schema");
        assert_eq!(active[0].1.value, "JSONB");
        assert_eq!(active[0].0.dep_type, "explicit");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_schema_v4_migration_backfill() {
        let (dir, store) = tmp_db();
        // After fresh creation, schema is v4 and decision_deps exists
        let d1 = make_decision_event("main", "db.engine", "postgres", None, None);
        store.append_event(&d1).unwrap();
        let d2 = make_decision_event("main", "db.pool", "10", None, None);
        store.append_event(&d2).unwrap();

        // Insert dep manually (simulating what cmd_bridge.rs does)
        store
            .insert_dep("db.pool", "db.engine", "auto_domain", Some(&d2.event_id))
            .unwrap();

        let deps = store.deps_of("db.pool").unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].target_key, "db.engine");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Decision outcome tests ───────────────────────────────────────

    struct ExecEventParams<'a> {
        branch: &'a str,
        event_id: &'a str,
        ts: &'a str,
        decision_ref: Option<&'a str>,
        status: &'a str,
        cost_usd: f64,
        token_in: u64,
        token_out: u64,
        latency_ms: u64,
    }

    #[allow(clippy::too_many_arguments)]
    fn make_execution_event_with_decision_ref(
        branch: &str,
        event_id: &str,
        ts: &str,
        decision_ref: Option<&str>,
        status: &str,
        cost_usd: f64,
        token_in: u64,
        token_out: u64,
        latency_ms: u64,
    ) -> Event {
        make_exec_event_from_params(&ExecEventParams {
            branch,
            event_id,
            ts,
            decision_ref,
            status,
            cost_usd,
            token_in,
            token_out,
            latency_ms,
        })
    }

    fn make_exec_event_from_params(p: &ExecEventParams<'_>) -> Event {
        use edda_core::types::{Provenance, Refs};
        use edda_core::SCHEMA_VERSION;

        let refs = if let Some(dr) = p.decision_ref {
            Refs {
                provenance: vec![Provenance {
                    target: dr.to_string(),
                    rel: "based_on".to_string(),
                    note: Some("karvi decision_ref".to_string()),
                }],
                ..Default::default()
            }
        } else {
            Refs::default()
        };

        let payload = serde_json::json!({
            "version": "karvi.event.v1",
            "event_id": p.event_id,
            "event_type": "step_completed",
            "occurred_at": p.ts,
            "trace_id": format!("trace_{}", p.event_id),
            "task_id": format!("task_{}", p.event_id),
            "step_id": format!("step_{}", p.event_id),
            "project": "test/repo",
            "runtime": "opencode",
            "model": "gpt-4",
            "actor": { "kind": "agent", "id": "test-agent" },
            "usage": { "token_in": p.token_in, "token_out": p.token_out, "cost_usd": p.cost_usd, "latency_ms": p.latency_ms },
            "result": { "status": p.status, "error_code": null, "retryable": false },
            "decision_ref": p.decision_ref,
        });

        let mut event = Event {
            event_id: p.event_id.to_string(),
            ts: p.ts.to_string(),
            event_type: "execution_event".to_string(),
            branch: p.branch.to_string(),
            parent_hash: None,
            hash: String::new(),
            payload,
            refs,
            schema_version: SCHEMA_VERSION,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };
        edda_core::event::finalize_event(&mut event).unwrap();
        event
    }

    #[test]
    fn decision_outcomes_empty_when_no_executions() {
        let (dir, store) = tmp_db();
        let d1 = make_decision_event("main", "db.engine", "postgres", Some("JSONB"), None);
        let d1_id = d1.event_id.clone();
        store.append_event(&d1).unwrap();

        let outcomes = store.decision_outcomes(&d1_id).unwrap();
        assert!(outcomes.is_some());
        let m = outcomes.unwrap();
        assert_eq!(m.decision_key, "db.engine");
        assert_eq!(m.decision_value, "postgres");
        assert_eq!(m.total_executions, 0);
        assert_eq!(m.success_rate, 0.0);
        assert_eq!(m.total_cost_usd, 0.0);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decision_outcomes_aggregates_metrics() {
        let (dir, store) = tmp_db();
        let d1 = make_decision_event("main", "db.engine", "postgres", Some("JSONB"), None);
        let d1_id = d1.event_id.clone();
        store.append_event(&d1).unwrap();

        // Add 3 execution events linked to the decision
        let e1 = make_execution_event_with_decision_ref(
            "main",
            "evt_exec_1",
            "2026-03-01T10:00:00Z",
            Some(&d1_id),
            "success",
            0.01,
            100,
            50,
            500,
        );
        store.append_event(&e1).unwrap();

        let e2 = make_execution_event_with_decision_ref(
            "main",
            "evt_exec_2",
            "2026-03-01T11:00:00Z",
            Some(&d1_id),
            "success",
            0.02,
            200,
            100,
            600,
        );
        store.append_event(&e2).unwrap();

        let e3 = make_execution_event_with_decision_ref(
            "main",
            "evt_exec_3",
            "2026-03-01T12:00:00Z",
            Some(&d1_id),
            "failed",
            0.015,
            150,
            75,
            400,
        );
        store.append_event(&e3).unwrap();

        // Add an execution NOT linked to this decision (should be ignored)
        let e4 = make_execution_event_with_decision_ref(
            "main",
            "evt_exec_4",
            "2026-03-01T13:00:00Z",
            None,
            "success",
            0.5,
            1000,
            500,
            1000,
        );
        store.append_event(&e4).unwrap();

        let outcomes = store.decision_outcomes(&d1_id).unwrap();
        assert!(outcomes.is_some());
        let m = outcomes.unwrap();

        assert_eq!(m.decision_key, "db.engine");
        assert_eq!(m.decision_value, "postgres");
        assert_eq!(m.total_executions, 3);
        assert_eq!(m.success_count, 2);
        assert_eq!(m.failed_count, 1);
        assert_eq!(m.cancelled_count, 0);
        assert!((m.success_rate - 66.66666666666666).abs() < 0.01);
        assert!((m.total_cost_usd - 0.045).abs() < 0.0001);
        assert_eq!(m.total_tokens_in, 450);
        assert_eq!(m.total_tokens_out, 225);
        assert!((m.avg_latency_ms - 500.0).abs() < 0.01);
        assert_eq!(
            m.first_execution_ts,
            Some("2026-03-01T10:00:00Z".to_string())
        );
        assert_eq!(
            m.last_execution_ts,
            Some("2026-03-01T12:00:00Z".to_string())
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decision_outcomes_returns_none_for_nonexistent() {
        let (dir, store) = tmp_db();
        let outcomes = store.decision_outcomes("evt_nonexistent").unwrap();
        assert!(outcomes.is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn executions_for_decision_filters_correctly() {
        let (dir, store) = tmp_db();
        let d1 = make_decision_event("main", "db.engine", "postgres", None, None);
        let d1_id = d1.event_id.clone();
        store.append_event(&d1).unwrap();

        let d2 = make_decision_event("main", "cache.strategy", "redis", None, None);
        let d2_id = d2.event_id.clone();
        store.append_event(&d2).unwrap();

        // Execution linked to d1
        let e1 = make_execution_event_with_decision_ref(
            "main",
            "evt_exec_d1",
            "2026-03-01T10:00:00Z",
            Some(&d1_id),
            "success",
            0.01,
            100,
            50,
            500,
        );
        store.append_event(&e1).unwrap();

        // Execution linked to d2
        let e2 = make_execution_event_with_decision_ref(
            "main",
            "evt_exec_d2",
            "2026-03-01T11:00:00Z",
            Some(&d2_id),
            "failed",
            0.02,
            200,
            100,
            600,
        );
        store.append_event(&e2).unwrap();

        // Execution with no decision_ref
        let e3 = make_execution_event_with_decision_ref(
            "main",
            "evt_exec_none",
            "2026-03-01T12:00:00Z",
            None,
            "success",
            0.03,
            300,
            150,
            700,
        );
        store.append_event(&e3).unwrap();

        let d1_execs = store.executions_for_decision(&d1_id).unwrap();
        assert_eq!(d1_execs.len(), 1);
        assert_eq!(d1_execs[0].event_id, "evt_exec_d1");
        assert_eq!(d1_execs[0].status, "success");
        assert_eq!(d1_execs[0].runtime, Some("opencode".to_string()));
        assert_eq!(d1_execs[0].model, Some("gpt-4".to_string()));
        assert!((d1_execs[0].cost_usd.unwrap() - 0.01).abs() < 0.0001);

        let d2_execs = store.executions_for_decision(&d2_id).unwrap();
        assert_eq!(d2_execs.len(), 1);
        assert_eq!(d2_execs[0].event_id, "evt_exec_d2");
        assert_eq!(d2_execs[0].status, "failed");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Concurrent writer tests ────────────────────────────────────

    #[test]
    fn concurrent_writers_no_data_loss() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("edda_sqlite_conc_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("ledger.db");

        // Create DB with schema first
        {
            let _init = SqliteStore::open_or_create(&db_path).unwrap();
        }

        let num_threads: usize = 10;
        let events_per_thread: usize = 5;
        let db_path_shared = db_path.clone();

        let handles: Vec<_> = (0..num_threads)
            .map(|t| {
                let path = db_path_shared.clone();
                std::thread::spawn(move || {
                    let store = SqliteStore::open(&path).unwrap();
                    for i in 0..events_per_thread {
                        // Each event gets a unique UUID via new_note_event, no parent_hash
                        // linkage needed — we're testing concurrent *writes*, not chain integrity.
                        let e = new_note_event(
                            "main",
                            None,
                            "system",
                            &format!("thread {t} event {i}"),
                            &[],
                        )
                        .unwrap();
                        store.append_event(&e).unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread panicked");
        }

        // Verify: all events persisted, no data loss
        let store = SqliteStore::open(&db_path).unwrap();
        let events = store.iter_events().unwrap();
        assert_eq!(
            events.len(),
            num_threads * events_per_thread,
            "expected {} events, got {}",
            num_threads * events_per_thread,
            events.len()
        );

        // Verify no duplicate event_ids
        let mut ids: Vec<&str> = events.iter().map(|e| e.event_id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), num_threads * events_per_thread);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Hash chain verification tests ──────────────────────────────

    #[test]
    fn verify_chain_empty_store() {
        let (dir, store) = tmp_db();
        assert!(store.verify_chain().is_ok());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_chain_single_event() {
        let (dir, store) = tmp_db();
        let e = new_note_event("main", None, "system", "only event", &[]).unwrap();
        store.append_event(&e).unwrap();
        assert!(store.verify_chain().is_ok());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_chain_valid_sequence() {
        let (dir, store) = tmp_db();
        let e1 = new_note_event("main", None, "system", "first", &[]).unwrap();
        store.append_event(&e1).unwrap();
        let e2 = new_note_event("main", Some(&e1.hash), "system", "second", &[]).unwrap();
        store.append_event(&e2).unwrap();
        let e3 = new_note_event("main", Some(&e2.hash), "system", "third", &[]).unwrap();
        store.append_event(&e3).unwrap();

        assert!(store.verify_chain().is_ok());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_chain_detects_broken_parent_hash() {
        let (dir, store) = tmp_db();
        let e1 = new_note_event("main", None, "system", "first", &[]).unwrap();
        store.append_event(&e1).unwrap();

        // Intentionally use wrong parent_hash
        let e2 =
            new_note_event("main", Some("sha256:bogus_hash"), "system", "broken", &[]).unwrap();
        store.append_event(&e2).unwrap();

        let result = store.verify_chain();
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("chain break"),
            "error should mention 'chain break', got: {msg}"
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_chain_detects_first_event_with_parent() {
        let (dir, store) = tmp_db();
        // First event should have parent_hash=None, but we insert one with a parent
        let e = new_note_event(
            "main",
            Some("sha256:unexpected"),
            "system",
            "bad first",
            &[],
        )
        .unwrap();
        store.append_event(&e).unwrap();

        let result = store.verify_chain();
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("first event"));

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Task brief tests ─────────────────────────────────────────────

    fn make_task_intake_event(branch: &str, source_id: &str, title: &str, intent: &str) -> Event {
        use edda_core::event::{new_task_intake_event, TaskIntakeParams};
        new_task_intake_event(&TaskIntakeParams {
            branch: branch.to_string(),
            parent_hash: None,
            source: "github_issue".to_string(),
            source_id: source_id.to_string(),
            source_url: format!("https://github.com/test/repo/issues/{source_id}"),
            title: title.to_string(),
            intent: intent.to_string(),
            labels: vec!["enhancement".to_string()],
            priority: "medium".to_string(),
            constraints: vec![],
        })
        .unwrap()
    }

    fn make_commit_event(branch: &str, files: &[&str]) -> Event {
        use edda_core::event::finalize_event;
        let file_vals: Vec<serde_json::Value> = files
            .iter()
            .map(|f| serde_json::json!({"path": f}))
            .collect();
        let payload = serde_json::json!({
            "sha": "abc123",
            "message": "test commit",
            "files": file_vals,
        });
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut event = Event {
            event_id: format!("evt_commit_{n}"),
            ts: time_now_rfc3339(),
            event_type: "commit".to_string(),
            branch: branch.to_string(),
            parent_hash: None,
            hash: String::new(),
            payload,
            refs: Refs::default(),
            schema_version: 1,
            digests: Vec::new(),
            event_family: Some("milestone".to_string()),
            event_level: Some("milestone".to_string()),
        };
        finalize_event(&mut event).unwrap();
        event
    }

    fn make_merge_event(branch: &str) -> Event {
        use edda_core::event::finalize_event;
        let payload = serde_json::json!({
            "target_branch": "main",
            "merge_sha": "def456",
        });
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut event = Event {
            event_id: format!("evt_merge_{n}"),
            ts: time_now_rfc3339(),
            event_type: "merge".to_string(),
            branch: branch.to_string(),
            parent_hash: None,
            hash: String::new(),
            payload,
            refs: Refs::default(),
            schema_version: 1,
            digests: Vec::new(),
            event_family: Some("milestone".to_string()),
            event_level: Some("milestone".to_string()),
        };
        finalize_event(&mut event).unwrap();
        event
    }

    #[test]
    fn task_brief_materialized_on_intake() {
        let (dir, store) = tmp_db();
        let e = make_task_intake_event("feat/x", "42", "Add auth", "implement");
        store.append_event(&e).unwrap();

        let brief = store.get_task_brief("github_issue#42").unwrap();
        assert!(brief.is_some());
        let b = brief.unwrap();
        assert_eq!(b.task_id, "github_issue#42");
        assert_eq!(b.title, "Add auth");
        assert_eq!(b.intent, edda_core::types::TaskBriefIntent::Implement);
        assert_eq!(b.status, edda_core::types::TaskBriefStatus::Active);
        assert_eq!(b.branch, "feat/x");
        assert_eq!(b.iterations, 0);
        assert_eq!(b.intake_event_id, e.event_id);
        assert!(b.source_url.contains("42"));
        assert!(b.last_feedback.is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_not_found_returns_none() {
        let (dir, store) = tmp_db();
        let result = store.get_task_brief("nonexistent#99").unwrap();
        assert!(result.is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_commit_increments_iterations() {
        let (dir, store) = tmp_db();
        let intake = make_task_intake_event("feat/x", "10", "Fix bug", "fix");
        store.append_event(&intake).unwrap();

        // First commit
        let c1 = make_commit_event("feat/x", &["src/main.rs"]);
        store.append_event(&c1).unwrap();

        let b = store.get_task_brief("github_issue#10").unwrap().unwrap();
        assert_eq!(b.iterations, 1);
        assert_eq!(b.intent, edda_core::types::TaskBriefIntent::Fix);

        // Second commit adds new artifact
        let c2 = make_commit_event("feat/x", &["src/lib.rs"]);
        store.append_event(&c2).unwrap();

        let b = store.get_task_brief("github_issue#10").unwrap().unwrap();
        assert_eq!(b.iterations, 2);

        let artifacts: Vec<String> = serde_json::from_str(&b.artifacts).unwrap();
        assert!(artifacts.contains(&"src/main.rs".to_string()));
        assert!(artifacts.contains(&"src/lib.rs".to_string()));

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_commit_on_different_branch_ignored() {
        let (dir, store) = tmp_db();
        let intake = make_task_intake_event("feat/a", "20", "Task A", "implement");
        store.append_event(&intake).unwrap();

        // Commit on a different branch should not affect this brief
        let c = make_commit_event("feat/b", &["other.rs"]);
        store.append_event(&c).unwrap();

        let b = store.get_task_brief("github_issue#20").unwrap().unwrap();
        assert_eq!(b.iterations, 0);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_merge_marks_completed() {
        let (dir, store) = tmp_db();
        let intake = make_task_intake_event("feat/x", "30", "Impl feature", "implement");
        store.append_event(&intake).unwrap();

        let m = make_merge_event("feat/x");
        store.append_event(&m).unwrap();

        let b = store.get_task_brief("github_issue#30").unwrap().unwrap();
        assert_eq!(b.status, edda_core::types::TaskBriefStatus::Completed);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_merge_does_not_affect_other_branch() {
        let (dir, store) = tmp_db();
        let intake = make_task_intake_event("feat/a", "31", "Task A", "implement");
        store.append_event(&intake).unwrap();

        // Merge on a different branch
        let m = make_merge_event("feat/b");
        store.append_event(&m).unwrap();

        let b = store.get_task_brief("github_issue#31").unwrap().unwrap();
        assert_eq!(b.status, edda_core::types::TaskBriefStatus::Active);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_note_with_feedback_tag() {
        let (dir, store) = tmp_db();
        let intake = make_task_intake_event("feat/x", "40", "Review task", "implement");
        store.append_event(&intake).unwrap();

        // Note with review tag
        let note = new_note_event(
            "feat/x",
            None,
            "reviewer",
            "Looks good but needs tests",
            &["review".to_string()],
        )
        .unwrap();
        store.append_event(&note).unwrap();

        let b = store.get_task_brief("github_issue#40").unwrap().unwrap();
        assert_eq!(
            b.last_feedback.as_deref(),
            Some("Looks good but needs tests")
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_note_with_decision_tag() {
        let (dir, store) = tmp_db();
        let intake = make_task_intake_event("feat/x", "50", "Decide arch", "implement");
        store.append_event(&intake).unwrap();

        // Note with decision tag
        let mut note = new_note_event(
            "feat/x",
            None,
            "system",
            "db.engine: sqlite",
            &["decision".to_string()],
        )
        .unwrap();
        note.payload["decision"] = serde_json::json!({"key": "db.engine", "value": "sqlite"});
        store.append_event(&note).unwrap();

        let b = store.get_task_brief("github_issue#50").unwrap().unwrap();
        let decisions: Vec<String> = serde_json::from_str(&b.decisions).unwrap();
        assert!(decisions.contains(&"db.engine".to_string()));

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_note_without_relevant_tag_ignored() {
        let (dir, store) = tmp_db();
        let intake = make_task_intake_event("feat/x", "51", "Some task", "implement");
        store.append_event(&intake).unwrap();

        // Note with unrelated tag
        let note = new_note_event(
            "feat/x",
            None,
            "user",
            "random note",
            &["session".to_string()],
        )
        .unwrap();
        store.append_event(&note).unwrap();

        let b = store.get_task_brief("github_issue#51").unwrap().unwrap();
        assert!(b.last_feedback.is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_list_all() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_task_intake_event(
                "feat/a",
                "60",
                "Task A",
                "implement",
            ))
            .unwrap();
        store
            .append_event(&make_task_intake_event("feat/b", "61", "Task B", "fix"))
            .unwrap();

        let all = store.list_task_briefs(None, None).unwrap();
        assert_eq!(all.len(), 2);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_list_filter_by_status() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_task_intake_event(
                "feat/a",
                "70",
                "Active task",
                "implement",
            ))
            .unwrap();
        store
            .append_event(&make_task_intake_event("feat/b", "71", "Done task", "fix"))
            .unwrap();

        // Complete the second one
        store.append_event(&make_merge_event("feat/b")).unwrap();

        let active = store.list_task_briefs(Some("active"), None).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].task_id, "github_issue#70");

        let completed = store.list_task_briefs(Some("completed"), None).unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].task_id, "github_issue#71");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_list_filter_by_intent() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_task_intake_event(
                "feat/a",
                "80",
                "Impl task",
                "implement",
            ))
            .unwrap();
        store
            .append_event(&make_task_intake_event("feat/b", "81", "Fix task", "fix"))
            .unwrap();

        let impls = store.list_task_briefs(None, Some("implement")).unwrap();
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].task_id, "github_issue#80");

        let fixes = store.list_task_briefs(None, Some("fix")).unwrap();
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].task_id, "github_issue#81");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_list_filter_combined() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_task_intake_event("feat/a", "90", "A", "implement"))
            .unwrap();
        store
            .append_event(&make_task_intake_event("feat/b", "91", "B", "implement"))
            .unwrap();
        store
            .append_event(&make_task_intake_event("feat/c", "92", "C", "fix"))
            .unwrap();

        // Complete B
        store.append_event(&make_merge_event("feat/b")).unwrap();

        let result = store
            .list_task_briefs(Some("active"), Some("implement"))
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].task_id, "github_issue#90");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_duplicate_intake_ignored() {
        let (dir, store) = tmp_db();
        let e1 = make_task_intake_event("feat/x", "100", "Task", "implement");
        store.append_event(&e1).unwrap();

        // Another intake with same source/source_id but different event
        let e2 = make_task_intake_event("feat/x", "100", "Task v2", "fix");
        store.append_event(&e2).unwrap();

        // Should still have only one brief (INSERT OR IGNORE)
        let briefs = store.list_task_briefs(None, None).unwrap();
        assert_eq!(briefs.len(), 1);
        assert_eq!(briefs[0].title, "Task"); // original title kept

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_unknown_intent_falls_back() {
        let (dir, store) = tmp_db();
        // Manually create an event with an unknown intent
        let e = make_task_intake_event("feat/x", "110", "Unknown intent task", "foobar");
        store.append_event(&e).unwrap();

        let b = store.get_task_brief("github_issue#110").unwrap().unwrap();
        // Should fall back to Implement
        assert_eq!(b.intent, edda_core::types::TaskBriefIntent::Implement);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_completed_ignores_further_commits() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_task_intake_event(
                "feat/x",
                "120",
                "Done task",
                "implement",
            ))
            .unwrap();
        store
            .append_event(&make_commit_event("feat/x", &["a.rs"]))
            .unwrap();
        store.append_event(&make_merge_event("feat/x")).unwrap();

        let b = store.get_task_brief("github_issue#120").unwrap().unwrap();
        assert_eq!(b.status, edda_core::types::TaskBriefStatus::Completed);
        assert_eq!(b.iterations, 1);

        // Commit after merge should not increment (status is completed, not active)
        store
            .append_event(&make_commit_event("feat/x", &["b.rs"]))
            .unwrap();

        let b = store.get_task_brief("github_issue#120").unwrap().unwrap();
        assert_eq!(b.iterations, 1); // unchanged

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_backfill_parses_json_not_like() {
        let (dir, store) = tmp_db();

        // Create a task_intake directly in events table (simulating pre-v5 data)
        let intake = make_task_intake_event("feat/x", "130", "Backfill test", "implement");
        store.append_event(&intake).unwrap();

        // Create a note that mentions "review" in the message text but NOT in tags.
        // This should NOT be picked up as feedback (unlike the old LIKE approach).
        let note = new_note_event(
            "feat/x",
            None,
            "user",
            "I did a code review and found issues",
            &["session".to_string()],
        )
        .unwrap();
        store.append_event(&note).unwrap();

        let b = store.get_task_brief("github_issue#130").unwrap().unwrap();
        // No feedback should be set because the tag is "session", not "review"
        assert!(b.last_feedback.is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_brief_all_intent_variants() {
        let (dir, store) = tmp_db();
        let intents = [
            "implement",
            "fix",
            "maintain",
            "investigate",
            "refactor",
            "document",
            "test",
        ];
        for (i, intent) in intents.iter().enumerate() {
            let id = format!("{}", 200 + i);
            store
                .append_event(&make_task_intake_event("feat/x", &id, "task", intent))
                .unwrap();
        }

        let all = store.list_task_briefs(None, None).unwrap();
        assert_eq!(all.len(), intents.len());

        // Verify each intent can be filtered
        for intent in &intents {
            let found = store.list_task_briefs(None, Some(intent)).unwrap();
            assert_eq!(
                found.len(),
                1,
                "should find exactly one task with intent {intent}"
            );
        }

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Temporal filter tests ────────────────────────────────────────

    /// Helper: create a decision event with a specific timestamp.
    fn make_decision_event_at(branch: &str, key: &str, value: &str, ts: &str) -> Event {
        use edda_core::event::finalize_event;
        let text = format!("{key}: {value}");
        let tags = vec!["decision".to_string()];
        let mut event = new_note_event(branch, None, "system", &text, &tags).unwrap();
        event.ts = ts.to_string();
        event.payload["decision"] = serde_json::json!({"key": key, "value": value});
        finalize_event(&mut event).unwrap();
        event
    }

    #[test]
    fn temporal_filter_after_only() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.engine",
                "postgres",
                "2026-03-10T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.pool",
                "10",
                "2026-03-12T00:00:00Z",
            ))
            .unwrap();

        let results = store
            .active_decisions(None, None, Some("2026-03-11T00:00:00Z"), None, None)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "db.pool");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporal_filter_before_only() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.engine",
                "postgres",
                "2026-03-10T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.pool",
                "10",
                "2026-03-12T00:00:00Z",
            ))
            .unwrap();

        let results = store
            .active_decisions(None, None, None, Some("2026-03-11T00:00:00Z"), None)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "db.engine");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporal_filter_range() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event_at(
                "main",
                "a.early",
                "v1",
                "2026-03-08T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "b.mid",
                "v2",
                "2026-03-10T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "c.late",
                "v3",
                "2026-03-14T00:00:00Z",
            ))
            .unwrap();

        let results = store
            .active_decisions(
                None,
                None,
                Some("2026-03-09T00:00:00Z"),
                Some("2026-03-12T00:00:00Z"),
                None,
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "b.mid");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporal_filter_with_domain() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.engine",
                "postgres",
                "2026-03-10T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.pool",
                "10",
                "2026-03-12T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "auth.method",
                "JWT",
                "2026-03-12T00:00:00Z",
            ))
            .unwrap();

        let results = store
            .active_decisions(Some("db"), None, Some("2026-03-11T00:00:00Z"), None, None)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "db.pool");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporal_filter_with_keyword() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.engine",
                "postgres",
                "2026-03-10T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "cache.engine",
                "redis",
                "2026-03-12T00:00:00Z",
            ))
            .unwrap();

        let results = store
            .active_decisions(
                None,
                Some("engine"),
                Some("2026-03-11T00:00:00Z"),
                None,
                None,
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "cache.engine");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporal_filter_empty_range() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.engine",
                "postgres",
                "2026-03-10T00:00:00Z",
            ))
            .unwrap();

        let results = store
            .active_decisions(
                None,
                None,
                Some("2026-03-15T00:00:00Z"),
                Some("2026-03-05T00:00:00Z"),
                None,
            )
            .unwrap();
        assert!(results.is_empty());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporal_filter_no_params_unchanged() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.engine",
                "postgres",
                "2026-03-10T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "auth.method",
                "JWT",
                "2026-03-12T00:00:00Z",
            ))
            .unwrap();

        let results = store
            .active_decisions(None, None, None, None, None)
            .unwrap();
        assert_eq!(results.len(), 2);
        // Original sort order: domain, key (auth before db)
        assert_eq!(results[0].key, "auth.method");
        assert_eq!(results[1].key, "db.engine");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporal_filter_sort_order_desc() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event_at(
                "main",
                "a.first",
                "v1",
                "2026-03-08T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "b.second",
                "v2",
                "2026-03-10T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "c.third",
                "v3",
                "2026-03-12T00:00:00Z",
            ))
            .unwrap();

        let results = store
            .active_decisions(None, None, Some("2026-03-07T00:00:00Z"), None, None)
            .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].key, "c.third");
        assert_eq!(results[1].key, "b.second");
        assert_eq!(results[2].key, "a.first");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporal_filter_decision_timeline() {
        let (dir, store) = tmp_db();

        let e1 = make_decision_event_at("main", "db.engine", "mysql", "2026-03-08T00:00:00Z");
        store.append_event(&e1).unwrap();
        let mut e2 =
            make_decision_event_at("main", "db.engine", "postgres", "2026-03-12T00:00:00Z");
        e2.refs.provenance.push(Provenance {
            target: e1.event_id.clone(),
            rel: "supersedes".to_string(),
            note: Some("upgrade".to_string()),
        });
        edda_core::event::finalize_event(&mut e2).unwrap();
        store.append_event(&e2).unwrap();

        let results = store
            .decision_timeline("db.engine", Some("2026-03-10T00:00:00Z"), None)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "postgres");

        let results = store.decision_timeline("db.engine", None, None).unwrap();
        assert_eq!(results.len(), 2);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporal_filter_domain_timeline() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.engine",
                "postgres",
                "2026-03-08T00:00:00Z",
            ))
            .unwrap();
        store
            .append_event(&make_decision_event_at(
                "main",
                "db.pool",
                "10",
                "2026-03-12T00:00:00Z",
            ))
            .unwrap();

        let results = store
            .domain_timeline("db", None, Some("2026-03-10T00:00:00Z"))
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "db.engine");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Causal chain tests ──────────────────────────────────────────

    #[test]
    fn get_decision_by_event_id_returns_decision() {
        let (dir, store) = tmp_db();
        let e = make_decision_event("main", "db.engine", "postgres", Some("JSONB support"), None);
        let eid = e.event_id.clone();
        store.append_event(&e).unwrap();

        let found = store.get_decision_by_event_id(&eid).unwrap();
        assert!(found.is_some());
        let d = found.unwrap();
        assert_eq!(d.key, "db.engine");
        assert_eq!(d.value, "postgres");
        assert_eq!(d.reason, "JSONB support");

        // Non-existent returns None
        let missing = store.get_decision_by_event_id("evt_nonexistent").unwrap();
        assert!(missing.is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn causal_chain_returns_none_for_nonexistent() {
        let (dir, store) = tmp_db();
        let result = store.causal_chain("evt_nonexistent", 3).unwrap();
        assert!(result.is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn causal_chain_empty_when_no_deps() {
        let (dir, store) = tmp_db();
        let e = make_decision_event("main", "db.engine", "postgres", Some("test"), None);
        let eid = e.event_id.clone();
        store.append_event(&e).unwrap();

        let (root, chain) = store.causal_chain(&eid, 3).unwrap().unwrap();
        assert_eq!(root.key, "db.engine");
        assert!(chain.is_empty());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn causal_chain_follows_dependency_edges() {
        let (dir, store) = tmp_db();

        let e_a = make_decision_event("main", "db.engine", "postgres", Some("root"), None);
        let eid_a = e_a.event_id.clone();
        store.append_event(&e_a).unwrap();

        let e_b = make_decision_event("main", "db.pool", "10", Some("pool config"), None);
        store.append_event(&e_b).unwrap();

        let e_c = make_decision_event("main", "db.timeout", "30", Some("timeout"), None);
        store.append_event(&e_c).unwrap();

        // db.pool depends on db.engine, db.timeout depends on db.pool
        store
            .insert_dep("db.pool", "db.engine", "explicit", None)
            .unwrap();
        store
            .insert_dep("db.timeout", "db.pool", "explicit", None)
            .unwrap();

        let (root, chain) = store.causal_chain(&eid_a, 5).unwrap().unwrap();
        assert_eq!(root.key, "db.engine");
        assert_eq!(chain.len(), 2);

        // depth 1: db.pool depends on db.engine
        let pool_entry = chain.iter().find(|e| e.decision.key == "db.pool").unwrap();
        assert_eq!(pool_entry.relation, "depends_on");
        assert_eq!(pool_entry.depth, 1);

        // depth 2: db.timeout depends on db.pool
        let timeout_entry = chain
            .iter()
            .find(|e| e.decision.key == "db.timeout")
            .unwrap();
        assert_eq!(timeout_entry.relation, "depends_on");
        assert_eq!(timeout_entry.depth, 2);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn causal_chain_follows_supersession() {
        let (dir, store) = tmp_db();

        let e_a = make_decision_event("main", "db.engine", "postgres", Some("original"), None);
        let eid_a = e_a.event_id.clone();
        store.append_event(&e_a).unwrap();

        let e_b = make_decision_event(
            "main",
            "db.engine",
            "mysql",
            Some("changed mind"),
            Some(&eid_a),
        );
        store.append_event(&e_b).unwrap();

        let (root, chain) = store.causal_chain(&eid_a, 3).unwrap().unwrap();
        assert_eq!(root.key, "db.engine");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].relation, "superseded_by");
        assert_eq!(chain[0].decision.value, "mysql");
        assert_eq!(chain[0].depth, 1);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn causal_chain_mixed_dep_and_supersession() {
        let (dir, store) = tmp_db();

        let e_a = make_decision_event("main", "db.engine", "postgres", Some("original"), None);
        let eid_a = e_a.event_id.clone();
        store.append_event(&e_a).unwrap();

        // B supersedes A
        let e_b = make_decision_event("main", "db.engine", "mysql", Some("changed"), Some(&eid_a));
        store.append_event(&e_b).unwrap();

        // C depends on A's key
        let e_c = make_decision_event("main", "db.pool", "10", Some("pool"), None);
        store.append_event(&e_c).unwrap();
        store
            .insert_dep("db.pool", "db.engine", "explicit", None)
            .unwrap();

        let (root, chain) = store.causal_chain(&eid_a, 3).unwrap().unwrap();
        assert_eq!(root.key, "db.engine");
        assert_eq!(chain.len(), 2);

        let has_superseded_by = chain.iter().any(|e| e.relation == "superseded_by");
        let has_depends_on = chain.iter().any(|e| e.relation == "depends_on");
        assert!(has_superseded_by);
        assert!(has_depends_on);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn causal_chain_respects_depth_limit() {
        let (dir, store) = tmp_db();

        let e_a = make_decision_event("main", "db.engine", "postgres", Some("root"), None);
        let eid_a = e_a.event_id.clone();
        store.append_event(&e_a).unwrap();

        let e_b = make_decision_event("main", "db.pool", "10", Some("pool"), None);
        store.append_event(&e_b).unwrap();

        let e_c = make_decision_event("main", "db.timeout", "30", Some("timeout"), None);
        store.append_event(&e_c).unwrap();

        store
            .insert_dep("db.pool", "db.engine", "explicit", None)
            .unwrap();
        store
            .insert_dep("db.timeout", "db.pool", "explicit", None)
            .unwrap();

        // depth=1 should only reach db.pool, not db.timeout
        let (root, chain) = store.causal_chain(&eid_a, 1).unwrap().unwrap();
        assert_eq!(root.key, "db.engine");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].decision.key, "db.pool");
        assert_eq!(chain[0].depth, 1);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hot_path_query_performance() {
        use std::time::Instant;

        let (dir, store) = tmp_db();

        // Seed with 500 decisions across 5 domains (typical workload)
        let domains = ["db", "auth", "api", "cache", "infra"];
        for (i, dom) in domains.iter().enumerate() {
            for j in 0..100 {
                let key = format!("{}.key{}", dom, j);
                let value = format!("value{}_{}", i, j);
                let reason = format!("reason for {} decision", key);
                let e = make_decision_event("main", &key, &value, Some(&reason), None);
                store.append_event(&e).unwrap();
            }
        }

        // Warm up
        let _ = store
            .active_decisions(None, None, None, None, Some(20))
            .unwrap();

        // Benchmark: query all active decisions with limit (hot path)
        let start = Instant::now();
        for _ in 0..100 {
            let results = store
                .active_decisions(None, None, None, None, Some(20))
                .unwrap();
            assert_eq!(results.len(), 20);
        }
        let elapsed_all = start.elapsed();
        let avg_ms_all = elapsed_all.as_millis() as f64 / 100.0;

        // Benchmark: query by domain (hot path)
        let start = Instant::now();
        for _ in 0..100 {
            let results = store
                .active_decisions(Some("db"), None, None, None, Some(20))
                .unwrap();
            assert_eq!(results.len(), 20);
        }
        let elapsed_domain = start.elapsed();
        let avg_ms_domain = elapsed_domain.as_millis() as f64 / 100.0;

        // Verify hot path queries are under 100ms (requirement from GH-319)
        // With index optimization, should be well under 10ms
        assert!(
            avg_ms_all < 100.0,
            "hot path query (all) avg {}ms exceeds 100ms threshold",
            avg_ms_all
        );
        assert!(
            avg_ms_domain < 100.0,
            "hot path query (domain) avg {}ms exceeds 100ms threshold",
            avg_ms_domain
        );

        // Log results for visibility
        eprintln!(
            "hot_path_query_performance: avg_all={:.2}ms, avg_domain={:.2}ms",
            avg_ms_all, avg_ms_domain
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_schema_v10_fresh_db() {
        let (dir, store) = tmp_db();

        // Version should be 12 (V11 village_id, V12 suggestions)
        assert_eq!(store.schema_version().unwrap(), 12);

        // Verify new columns exist by inserting a test row
        store
            .conn
            .execute(
                "INSERT INTO events (event_id, ts, event_type, branch, hash, payload)
                 VALUES ('evt_test', '2026-01-01T00:00:00Z', 'note', 'main', 'h1', '{}')",
                [],
            )
            .unwrap();

        store
            .conn
            .execute(
                "INSERT INTO decisions
                 (event_id, key, value, reason, domain, branch, is_active, scope,
                  status, authority, affected_paths, tags, reversibility)
                 VALUES ('evt_test', 'test.key', 'val', 'reason', 'test', 'main', TRUE, 'local',
                         'active', 'human', '[\"src/**\"]', '[\"arch\"]', 'medium')",
                [],
            )
            .unwrap();

        // Read back and verify
        let status: String = store
            .conn
            .query_row(
                "SELECT status FROM decisions WHERE key = 'test.key'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "active");

        let paths: String = store
            .conn
            .query_row(
                "SELECT affected_paths FROM decisions WHERE key = 'test.key'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(paths, "[\"src/**\"]");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_schema_v9_to_v10_migration() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("edda_sqlite_v10_mig_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("ledger.db");

        // Phase 1: Create a V9 database manually
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(SCHEMA_SQL).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('version', '1')",
                [],
            )
            .unwrap();
            conn.execute_batch(SCHEMA_V2_SQL).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '2')",
                [],
            )
            .unwrap();
            conn.execute_batch(SCHEMA_V3_SQL).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '3')",
                [],
            )
            .unwrap();
            conn.execute_batch(SCHEMA_V4_SQL).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '4')",
                [],
            )
            .unwrap();
            conn.execute_batch(SCHEMA_V5_SQL).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '5')",
                [],
            )
            .unwrap();
            conn.execute_batch(SCHEMA_V6_SQL).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '6')",
                [],
            )
            .unwrap();
            conn.execute_batch(SCHEMA_V7_SQL).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '7')",
                [],
            )
            .unwrap();
            conn.execute_batch(SCHEMA_V8_SQL).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '8')",
                [],
            )
            .unwrap();
            conn.execute_batch(SCHEMA_V9_SQL).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', '9')",
                [],
            )
            .unwrap();

            // Insert test events + decisions (V9 schema — no status/authority columns)
            conn.execute(
                "INSERT INTO events (event_id, ts, event_type, branch, hash, payload)
                 VALUES ('evt_a', '2026-01-01T00:00:00Z', 'note', 'main', 'h1', '{}')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO events (event_id, ts, event_type, branch, hash, payload)
                 VALUES ('evt_b', '2026-01-02T00:00:00Z', 'note', 'main', 'h2', '{}')",
                [],
            )
            .unwrap();

            conn.execute(
                "INSERT INTO decisions (event_id, key, value, reason, domain, branch, is_active, scope)
                 VALUES ('evt_a', 'db.engine', 'sqlite', 'embedded', 'db', 'main', TRUE, 'local')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO decisions (event_id, key, value, reason, domain, branch, is_active, scope)
                 VALUES ('evt_b', 'old.key', 'old_val', 'deprecated', 'old', 'main', FALSE, 'local')",
                [],
            )
            .unwrap();
        }

        // Phase 2: Reopen — should auto-migrate to V12
        let store = SqliteStore::open_or_create(&db_path).unwrap();
        assert_eq!(store.schema_version().unwrap(), 12);

        // Active decision should have status='active'
        let status: String = store
            .conn
            .query_row(
                "SELECT status FROM decisions WHERE key = 'db.engine'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "active");

        // Inactive decision should have status='superseded'
        let status: String = store
            .conn
            .query_row(
                "SELECT status FROM decisions WHERE key = 'old.key'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "superseded");

        // COMPAT-01 invariant check
        let violations: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM decisions
                 WHERE (is_active = 1 AND status NOT IN ('active', 'experimental'))
                    OR (is_active = 0 AND status IN ('active', 'experimental'))",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(violations, 0, "COMPAT-01 violated after migration");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_v10_backward_compat_is_active_queries() {
        let (dir, store) = tmp_db();

        // Insert events + decisions directly for unit test isolation
        store
            .conn
            .execute(
                "INSERT INTO events (event_id, ts, event_type, branch, hash, payload)
                 VALUES ('evt_c1', '2026-01-01T00:00:00Z', 'note', 'main', 'h1', '{}')",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO decisions
                 (event_id, key, value, reason, domain, branch, is_active, scope, status)
                 VALUES ('evt_c1', 'compat.test', 'yes', 'test', 'compat', 'main', TRUE, 'local', 'active')",
                [],
            )
            .unwrap();

        // Existing query pattern: WHERE is_active = TRUE
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM decisions WHERE is_active = TRUE AND domain = 'compat'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Existing partial index query pattern
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM decisions WHERE is_active = TRUE AND domain = 'compat' AND branch = 'main'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── B1: Status Sync tests ──────────────────────────────────────

    #[test]
    fn test_status_to_is_active() {
        assert!(status_to_is_active("active"));
        assert!(status_to_is_active("experimental"));
        assert!(!status_to_is_active("proposed"));
        assert!(!status_to_is_active("deprecated"));
        assert!(!status_to_is_active("superseded"));
    }

    #[test]
    fn test_status_is_active_sync_on_insert() {
        let (dir, store) = tmp_db();

        let dp = edda_core::types::DecisionPayload {
            key: "sync.test".to_string(),
            value: "v1".to_string(),
            reason: Some("testing sync".to_string()),
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let event = edda_core::event::new_decision_event("main", None, "system", &dp).unwrap();
        store.append_event(&event).unwrap();

        // Verify status and is_active agree
        let (status, is_active): (String, bool) = store
            .conn
            .query_row(
                "SELECT status, is_active FROM decisions WHERE key = 'sync.test'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "active");
        assert!(is_active);

        // Supersede with a new value
        let dp2 = edda_core::types::DecisionPayload {
            key: "sync.test".to_string(),
            value: "v2".to_string(),
            reason: Some("supersede".to_string()),
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let mut event2 =
            edda_core::event::new_decision_event("main", Some(&event.hash), "system", &dp2)
                .unwrap();
        event2.refs.provenance.push(edda_core::types::Provenance {
            target: event.event_id.clone(),
            rel: "supersedes".to_string(),
            note: None,
        });
        edda_core::event::finalize_event(&mut event2).unwrap();
        store.append_event(&event2).unwrap();

        // Old decision: is_active=false, status=superseded
        let (old_status, old_active): (String, bool) = store
            .conn
            .query_row(
                "SELECT status, is_active FROM decisions WHERE key = 'sync.test' AND value = 'v1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(old_status, "superseded");
        assert!(!old_active);

        // New decision: is_active=true, status=active
        let (new_status, new_active): (String, bool) = store
            .conn
            .query_row(
                "SELECT status, is_active FROM decisions WHERE key = 'sync.test' AND value = 'v2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(new_status, "active");
        assert!(new_active);

        // COMPAT-01 full table check
        let violations: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM decisions
                 WHERE (is_active = 1 AND status NOT IN ('active', 'experimental'))
                    OR (is_active = 0 AND status IN ('active', 'experimental'))",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(violations, 0, "COMPAT-01 violated");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── B2: DecisionPayload new fields tests ───────────────────────

    #[test]
    fn test_decision_payload_new_fields_roundtrip() {
        let (dir, store) = tmp_db();

        let dp = edda_core::types::DecisionPayload {
            key: "db.engine".to_string(),
            value: "sqlite".to_string(),
            reason: Some("embedded".to_string()),
            scope: None,
            authority: Some("human".to_string()),
            affected_paths: Some(vec![
                "crates/edda-ledger/**".to_string(),
                "crates/edda-store/**".to_string(),
            ]),
            tags: Some(vec!["architecture".to_string(), "storage".to_string()]),
            review_after: Some("2026-06-01".to_string()),
            reversibility: Some("hard".to_string()),
            village_id: None,
        };
        let event = edda_core::event::new_decision_event("main", None, "system", &dp).unwrap();
        store.append_event(&event).unwrap();

        let row = store
            .conn
            .query_row(
                "SELECT authority, affected_paths, tags, review_after, reversibility
                 FROM decisions WHERE key = 'db.engine'",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0, "human");
        assert_eq!(row.1, r#"["crates/edda-ledger/**","crates/edda-store/**"]"#);
        assert_eq!(row.2, r#"["architecture","storage"]"#);
        assert_eq!(row.3.as_deref(), Some("2026-06-01"));
        assert_eq!(row.4, "hard");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_decision_payload_defaults_when_none() {
        let (dir, store) = tmp_db();

        let dp = edda_core::types::DecisionPayload {
            key: "default.test".to_string(),
            value: "val".to_string(),
            reason: None,
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let event = edda_core::event::new_decision_event("main", None, "system", &dp).unwrap();
        store.append_event(&event).unwrap();

        let row = store
            .conn
            .query_row(
                "SELECT authority, affected_paths, tags, review_after, reversibility
                 FROM decisions WHERE key = 'default.test'",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0, "human"); // authority default
        assert_eq!(row.1, "[]"); // affected_paths default
        assert_eq!(row.2, "[]"); // tags default
        assert_eq!(row.3, None); // review_after default
        assert_eq!(row.4, "medium"); // reversibility default

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_decisions_schema_repairs_missing_columns() {
        // Simulate the bug: create a DB at schema V11 but with the V5 `scope`
        // column missing from the decisions table.
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "edda_sqlite_verify_schema_{}_{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("ledger.db");

        // Manually create a DB with an incomplete decisions table
        // (missing scope, source_project_id, source_event_id from V5
        //  and all V10/V11 columns).
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(SCHEMA_SQL).unwrap();
            conn.execute_batch(
                "INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('version', '11');",
            )
            .unwrap();
            // Create decisions with only base V2 columns — skip V5 + V10 + V11
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS decisions (
                    event_id TEXT PRIMARY KEY REFERENCES events(event_id),
                    key TEXT NOT NULL,
                    value TEXT NOT NULL,
                    reason TEXT NOT NULL DEFAULT '',
                    domain TEXT NOT NULL DEFAULT '',
                    branch TEXT NOT NULL,
                    supersedes_id TEXT,
                    is_active BOOLEAN NOT NULL DEFAULT TRUE
                );",
            )
            .unwrap();
        }

        // Now open via the normal path — apply_schema sees version=11, so
        // no migrations run, but verify_decisions_schema should repair.
        let store = SqliteStore::open_or_create(&db_path).unwrap();

        // Check that all expected columns now exist.
        let columns: std::collections::HashSet<String> = {
            let mut stmt = store.conn.prepare("PRAGMA table_info(decisions)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };

        let expected = [
            "event_id",
            "key",
            "value",
            "reason",
            "domain",
            "branch",
            "supersedes_id",
            "is_active",
            "scope",
            "source_project_id",
            "source_event_id",
            "status",
            "authority",
            "affected_paths",
            "tags",
            "review_after",
            "reversibility",
            "village_id",
        ];

        for col in &expected {
            assert!(
                columns.contains(*col),
                "expected column `{col}` to exist after verify_decisions_schema, but it was missing. actual: {columns:?}"
            );
        }

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_village_stats_basic() {
        let (dir, store) = tmp_db();

        // Insert two decisions with village_id "village-abc"
        let dp1 = edda_core::types::DecisionPayload {
            key: "db.engine".to_string(),
            value: "sqlite".to_string(),
            reason: Some("embedded".to_string()),
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: Some("village-abc".to_string()),
        };
        let event1 = edda_core::event::new_decision_event("main", None, "system", &dp1).unwrap();
        store.append_event(&event1).unwrap();

        let dp2 = edda_core::types::DecisionPayload {
            key: "auth.strategy".to_string(),
            value: "jwt".to_string(),
            reason: Some("stateless".to_string()),
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: Some("village-abc".to_string()),
        };
        let event2 =
            edda_core::event::new_decision_event("main", Some(&event1.hash), "system", &dp2)
                .unwrap();
        store.append_event(&event2).unwrap();

        // Insert one decision with a different village
        let dp3 = edda_core::types::DecisionPayload {
            key: "log.level".to_string(),
            value: "debug".to_string(),
            reason: None,
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: Some("village-other".to_string()),
        };
        let event3 =
            edda_core::event::new_decision_event("main", Some(&event2.hash), "system", &dp3)
                .unwrap();
        store.append_event(&event3).unwrap();

        // Village stats for "village-abc" should see exactly 2 decisions
        let stats = store.village_stats("village-abc", None, None).unwrap();
        assert_eq!(stats.village_id, "village-abc");
        assert_eq!(stats.total_decisions, 2);
        assert!(stats.period.is_none(), "no temporal filter => no period");
        assert!(stats.decisions_per_day > 0.0);

        // by_status should have "active" entries
        assert!(
            stats.by_status.get("active").copied().unwrap_or(0) >= 1,
            "should have at least one active decision"
        );

        // top_domains should be populated
        assert!(!stats.top_domains.is_empty());

        // Village stats for "village-other" should see exactly 1 decision
        let stats_other = store.village_stats("village-other", None, None).unwrap();
        assert_eq!(stats_other.total_decisions, 1);

        // Non-existent village should return 0 decisions
        let stats_empty = store.village_stats("no-such-village", None, None).unwrap();
        assert_eq!(stats_empty.total_decisions, 0);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_village_stats_with_temporal_filter() {
        let (dir, store) = tmp_db();

        let dp = edda_core::types::DecisionPayload {
            key: "cache.ttl".to_string(),
            value: "3600".to_string(),
            reason: None,
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: Some("village-t".to_string()),
        };
        let event = edda_core::event::new_decision_event("main", None, "system", &dp).unwrap();
        store.append_event(&event).unwrap();

        // With a future "after" filter, should still include (ts is now)
        let stats = store
            .village_stats("village-t", Some("2020-01-01"), None)
            .unwrap();
        assert_eq!(stats.total_decisions, 1);
        assert!(stats.period.is_some());

        // With a past "before" filter, should exclude
        let stats_empty = store
            .village_stats("village-t", None, Some("2020-01-01"))
            .unwrap();
        assert_eq!(stats_empty.total_decisions, 0);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_village_id_stored_and_queried() {
        let (dir, store) = tmp_db();

        let dp = edda_core::types::DecisionPayload {
            key: "village.test".to_string(),
            value: "v1".to_string(),
            reason: None,
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: Some("my-village".to_string()),
        };
        let event = edda_core::event::new_decision_event("main", None, "system", &dp).unwrap();
        store.append_event(&event).unwrap();

        // Verify village_id is persisted in the DB
        let stored: Option<String> = store
            .conn
            .query_row(
                "SELECT village_id FROM decisions WHERE key = 'village.test'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored.as_deref(), Some("my-village"));

        // Query back via active decisions and check village_id
        let rows = store
            .active_decisions(None, None, None, None, None)
            .unwrap();
        let found = rows.iter().find(|r| r.key == "village.test").unwrap();
        assert_eq!(found.village_id.as_deref(), Some("my-village"));

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Pattern Detection Tests ──

    /// Helper to create a decision payload with specific authority and village.
    fn make_dp(
        key: &str,
        value: &str,
        authority: &str,
        village: &str,
    ) -> edda_core::types::DecisionPayload {
        edda_core::types::DecisionPayload {
            key: key.to_string(),
            value: value.to_string(),
            reason: Some("test".to_string()),
            scope: None,
            authority: Some(authority.to_string()),
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: Some(village.to_string()),
        }
    }

    #[test]
    fn test_detect_village_patterns_recurring() {
        let (dir, store) = tmp_db();

        // Insert 5 decisions with the same key in village "v1"
        let mut prev_hash: Option<String> = None;
        for i in 0..5 {
            let dp = make_dp(
                "rewards.daily_limit",
                &format!("{}", 100 + i),
                "event_chief",
                "v1",
            );
            let event =
                edda_core::event::new_decision_event("main", prev_hash.as_deref(), "system", &dp)
                    .unwrap();
            prev_hash = Some(event.hash.clone());
            store.append_event(&event).unwrap();
        }

        let patterns = store
            .detect_village_patterns("v1", "2020-01-01", 3)
            .unwrap();

        // Should detect at least one recurring_decision pattern
        let recurring: Vec<_> = patterns
            .iter()
            .filter(|p| {
                matches!(p.pattern_type, PatternType::RecurringDecision)
                    && p.key == "rewards.daily_limit"
            })
            .collect();
        assert!(
            !recurring.is_empty(),
            "should detect recurring decision pattern"
        );
        assert_eq!(recurring[0].occurrences, 5);
        assert!(recurring[0].description.contains("rewards.daily_limit"));

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_village_patterns_chief_repeated() {
        let (dir, store) = tmp_db();

        // Insert 3 decisions by "safety_chief" on the same key
        let mut prev_hash: Option<String> = None;
        for i in 0..3 {
            let dp = make_dp(
                "economy.reward_cap",
                &format!("{}", 50 + i),
                "safety_chief",
                "v2",
            );
            let event =
                edda_core::event::new_decision_event("main", prev_hash.as_deref(), "system", &dp)
                    .unwrap();
            prev_hash = Some(event.hash.clone());
            store.append_event(&event).unwrap();
        }

        let patterns = store
            .detect_village_patterns("v2", "2020-01-01", 3)
            .unwrap();

        let chief: Vec<_> = patterns
            .iter()
            .filter(|p| {
                matches!(p.pattern_type, PatternType::ChiefRepeatedAction)
                    && p.authority.as_deref() == Some("safety_chief")
            })
            .collect();
        assert!(
            !chief.is_empty(),
            "should detect chief repeated action pattern"
        );
        assert_eq!(chief[0].occurrences, 3);
        assert!(chief[0].description.contains("safety_chief"));

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_village_patterns_rollback() {
        let (dir, store) = tmp_db();

        // Create a supersession chain: d1 -> d2 supersedes d1 -> d3 supersedes d2
        let dp1 = make_dp("activity.bonus", "100", "event_chief", "v3");
        let e1 = edda_core::event::new_decision_event("main", None, "system", &dp1).unwrap();
        store.append_event(&e1).unwrap();

        let dp2 = make_dp("activity.bonus", "50", "safety_chief", "v3");
        let e2 =
            edda_core::event::new_decision_event("main", Some(&e1.hash), "system", &dp2).unwrap();
        // Manually set supersedes_id via direct SQL update
        store.append_event(&e2).unwrap();
        store
            .conn
            .execute(
                "UPDATE decisions SET supersedes_id = ?1 WHERE event_id = ?2",
                params![e1.event_id, e2.event_id],
            )
            .unwrap();

        let dp3 = make_dp("activity.bonus", "30", "safety_chief", "v3");
        let e3 =
            edda_core::event::new_decision_event("main", Some(&e2.hash), "system", &dp3).unwrap();
        store.append_event(&e3).unwrap();
        store
            .conn
            .execute(
                "UPDATE decisions SET supersedes_id = ?1 WHERE event_id = ?2",
                params![e2.event_id, e3.event_id],
            )
            .unwrap();

        let patterns = store
            .detect_village_patterns("v3", "2020-01-01", 3)
            .unwrap();

        let rollback: Vec<_> = patterns
            .iter()
            .filter(|p| {
                matches!(p.pattern_type, PatternType::RollbackTrend) && p.key == "activity.bonus"
            })
            .collect();
        assert!(!rollback.is_empty(), "should detect rollback trend pattern");
        assert_eq!(rollback[0].occurrences, 2);
        assert!(rollback[0].trending_up.is_some());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_village_patterns_below_threshold() {
        let (dir, store) = tmp_db();

        // Only 2 decisions — threshold is 3, should not be detected
        let mut prev_hash: Option<String> = None;
        for i in 0..2 {
            let dp = make_dp("db.pool_size", &format!("{}", 10 + i), "human", "v4");
            let event =
                edda_core::event::new_decision_event("main", prev_hash.as_deref(), "system", &dp)
                    .unwrap();
            prev_hash = Some(event.hash.clone());
            store.append_event(&event).unwrap();
        }

        let patterns = store
            .detect_village_patterns("v4", "2020-01-01", 3)
            .unwrap();

        let recurring: Vec<_> = patterns
            .iter()
            .filter(|p| {
                matches!(p.pattern_type, PatternType::RecurringDecision) && p.key == "db.pool_size"
            })
            .collect();
        assert!(
            recurring.is_empty(),
            "2 decisions should not reach threshold of 3"
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_village_patterns_empty_village() {
        let (dir, store) = tmp_db();

        let patterns = store
            .detect_village_patterns("nonexistent", "2020-01-01", 3)
            .unwrap();
        assert!(
            patterns.is_empty(),
            "non-existent village should return empty patterns"
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
