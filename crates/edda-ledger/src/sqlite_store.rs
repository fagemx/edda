//! SQLite-backed storage for the edda ledger.
//!
//! Replaces the file-based storage (events.jsonl, refs/HEAD, refs/branches.json)
//! with a single `ledger.db` SQLite file using WAL mode.

use edda_core::types::{Digest, Event, Provenance, Refs};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// Schema version for migration tracking.
const SCHEMA_VERSION: &str = "1";

const SCHEMA_SQL: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS events (
    rowid INTEGER PRIMARY KEY,
    event_id TEXT UNIQUE NOT NULL,
    ts TEXT NOT NULL,
    event_type TEXT NOT NULL,
    branch TEXT NOT NULL,
    parent_hash TEXT,
    hash TEXT NOT NULL,
    payload TEXT NOT NULL,
    refs_blobs TEXT NOT NULL DEFAULT '[]',
    refs_events TEXT NOT NULL DEFAULT '[]',
    refs_provenance TEXT NOT NULL DEFAULT '[]',
    schema_version INTEGER NOT NULL DEFAULT 0,
    digests TEXT NOT NULL DEFAULT '[]',
    event_family TEXT,
    event_level TEXT
);

CREATE INDEX IF NOT EXISTS idx_events_branch ON events(branch);
CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type);
CREATE INDEX IF NOT EXISTS idx_events_branch_type ON events(branch, event_type);
CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
CREATE INDEX IF NOT EXISTS idx_events_branch_ts ON events(branch, ts DESC);

CREATE TABLE IF NOT EXISTS refs (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS schema_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// SQLite-backed storage engine.
pub struct SqliteStore {
    conn: Connection,
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

    fn apply_schema(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('version', ?1)",
            params![SCHEMA_VERSION],
        )?;
        Ok(())
    }

    // ── Events ──────────────────────────────────────────────────────

    /// Append an event. Append-only (CONTRACT LEDGER-02).
    pub fn append_event(&self, event: &Event) -> anyhow::Result<()> {
        let payload = serde_json::to_string(&event.payload)?;
        let refs_blobs = serde_json::to_string(&event.refs.blobs)?;
        let refs_events = serde_json::to_string(&event.refs.events)?;
        let refs_provenance = serde_json::to_string(&event.refs.provenance)?;
        let digests = serde_json::to_string(&event.digests)?;

        self.conn.execute(
            "INSERT INTO events (
                event_id, ts, event_type, branch, parent_hash, hash,
                payload, refs_blobs, refs_events, refs_provenance,
                schema_version, digests, event_family, event_level
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                event.event_id,
                event.ts,
                event.event_type,
                event.branch,
                event.parent_hash,
                event.hash,
                payload,
                refs_blobs,
                refs_events,
                refs_provenance,
                event.schema_version,
                digests,
                event.event_family,
                event.event_level,
            ],
        )?;
        Ok(())
    }

    /// Read all events in insertion order.
    pub fn iter_events(&self) -> anyhow::Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
             FROM events ORDER BY rowid",
        )?;

        let events = stmt
            .query_map([], |row| {
                let payload_str: String = row.get(6)?;
                let refs_blobs_str: String = row.get(7)?;
                let refs_events_str: String = row.get(8)?;
                let refs_prov_str: String = row.get(9)?;
                let digests_str: String = row.get(11)?;

                Ok(EventRow {
                    event_id: row.get(0)?,
                    ts: row.get(1)?,
                    event_type: row.get(2)?,
                    branch: row.get(3)?,
                    parent_hash: row.get(4)?,
                    hash: row.get(5)?,
                    payload_str,
                    refs_blobs_str,
                    refs_events_str,
                    refs_prov_str,
                    schema_version: row.get(10)?,
                    digests_str,
                    event_family: row.get(12)?,
                    event_level: row.get(13)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        events.into_iter().map(row_to_event).collect()
    }

    /// Get the hash of the last event.
    pub fn last_event_hash(&self) -> anyhow::Result<Option<String>> {
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT hash FROM events ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(result)
    }

    // ── Refs ────────────────────────────────────────────────────────

    /// Read the current HEAD branch name.
    pub fn head_branch(&self) -> anyhow::Result<String> {
        let value: String = self
            .conn
            .query_row(
                "SELECT value FROM refs WHERE key = 'HEAD'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| anyhow::anyhow!("HEAD not set in refs table"))?;
        Ok(value)
    }

    /// Write the HEAD branch name.
    pub fn set_head_branch(&self, name: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO refs (key, value) VALUES ('HEAD', ?1)",
            params![name],
        )?;
        Ok(())
    }

    /// Read branches.json equivalent from refs table.
    pub fn branches_json(&self) -> anyhow::Result<serde_json::Value> {
        let value: String = self
            .conn
            .query_row(
                "SELECT value FROM refs WHERE key = 'branches'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| anyhow::anyhow!("branches not set in refs table"))?;
        let json: serde_json::Value = serde_json::from_str(&value)?;
        Ok(json)
    }

    /// Write branches.json equivalent to refs table.
    pub fn set_branches_json(&self, value: &serde_json::Value) -> anyhow::Result<()> {
        let json_str = serde_json::to_string(value)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO refs (key, value) VALUES ('branches', ?1)",
            params![json_str],
        )?;
        Ok(())
    }
}

impl Drop for SqliteStore {
    fn drop(&mut self) {
        // Merge WAL back into main DB so users see a single file when idle.
        let _ = self
            .conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}

// ── Internal helpers ────────────────────────────────────────────────

/// Intermediate row struct for deserialization.
struct EventRow {
    event_id: String,
    ts: String,
    event_type: String,
    branch: String,
    parent_hash: Option<String>,
    hash: String,
    payload_str: String,
    refs_blobs_str: String,
    refs_events_str: String,
    refs_prov_str: String,
    schema_version: u32,
    digests_str: String,
    event_family: Option<String>,
    event_level: Option<String>,
}

fn row_to_event(row: EventRow) -> anyhow::Result<Event> {
    let payload: serde_json::Value = serde_json::from_str(&row.payload_str)?;
    let blobs: Vec<String> = serde_json::from_str(&row.refs_blobs_str)?;
    let events: Vec<String> = serde_json::from_str(&row.refs_events_str)?;
    let provenance: Vec<Provenance> = serde_json::from_str(&row.refs_prov_str)?;
    let digests: Vec<Digest> = serde_json::from_str(&row.digests_str)?;

    Ok(Event {
        event_id: row.event_id,
        ts: row.ts,
        event_type: row.event_type,
        branch: row.branch,
        parent_hash: row.parent_hash,
        hash: row.hash,
        payload,
        refs: Refs {
            blobs,
            events,
            provenance,
        },
        schema_version: row.schema_version,
        digests,
        event_family: row.event_family,
        event_level: row.event_level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::new_note_event;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_db() -> (std::path::PathBuf, SqliteStore) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("edda_sqlite_test_{}_{n}", std::process::id()));
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
        let e1 =
            new_note_event("main", None, "system", "first note", &["test".into()]).unwrap();
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
        let dir = std::env::temp_dir()
            .join(format!("edda_sqlite_wal_{}_{n}", std::process::id()));
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
        for i in 0..10 {
            assert_eq!(events[i].payload["text"], format!("event {i}"));
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
        let dir = std::env::temp_dir()
            .join(format!("edda_sqlite_idem_{}_{n}", std::process::id()));
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
}
