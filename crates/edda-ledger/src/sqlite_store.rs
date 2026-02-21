//! SQLite-backed storage for the edda ledger.
//!
//! Replaces the file-based storage (events.jsonl, refs/HEAD, refs/branches.json)
//! with a single `ledger.db` SQLite file using WAL mode.

use edda_core::types::{Digest, Event, Provenance, Refs};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

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

const SCHEMA_V2_SQL: &str = "
CREATE TABLE IF NOT EXISTS decisions (
    event_id TEXT PRIMARY KEY REFERENCES events(event_id),
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    domain TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL,
    supersedes_id TEXT,
    is_active BOOLEAN NOT NULL DEFAULT TRUE
);
CREATE INDEX IF NOT EXISTS idx_decisions_key ON decisions(key);
CREATE INDEX IF NOT EXISTS idx_decisions_domain ON decisions(domain);
CREATE INDEX IF NOT EXISTS idx_decisions_active ON decisions(is_active) WHERE is_active = TRUE;
CREATE INDEX IF NOT EXISTS idx_decisions_branch_key ON decisions(branch, key);
";

/// A row from the `decisions` table.
#[derive(Debug, Clone)]
pub struct DecisionRow {
    pub event_id: String,
    pub key: String,
    pub value: String,
    pub reason: String,
    pub domain: String,
    pub branch: String,
    pub supersedes_id: Option<String>,
    pub is_active: bool,
    pub ts: Option<String>,
}

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
        // Always apply v1 base schema (idempotent via IF NOT EXISTS)
        self.conn.execute_batch(SCHEMA_SQL)?;

        // Bootstrap version if not set
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('version', '1')",
            [],
        )?;

        // Migrate to v2 if needed
        let current = self.schema_version()?;
        if current < 2 {
            self.migrate_v1_to_v2()?;
        }

        Ok(())
    }

    fn schema_version(&self) -> anyhow::Result<u32> {
        let version_str: String = self
            .conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'version'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "1".to_string());
        Ok(version_str.parse().unwrap_or(1))
    }

    fn set_schema_version(&self, version: u32) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', ?1)",
            params![version.to_string()],
        )?;
        Ok(())
    }

    fn migrate_v1_to_v2(&self) -> anyhow::Result<()> {
        // Create decisions table + indexes
        self.conn.execute_batch(SCHEMA_V2_SQL)?;

        // Backfill: scan existing events for decisions
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, branch, payload, refs_provenance FROM events
             WHERE event_type = 'note' ORDER BY rowid",
        )?;
        let rows: Vec<(String, String, String, String, String)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        for (event_id, _ts, branch, payload_str, prov_str) in &rows {
            let payload: serde_json::Value = match serde_json::from_str(payload_str) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if !is_decision_payload(&payload) {
                continue;
            }

            let (key, value, reason) = extract_decision_from_payload(&payload);
            if key.is_empty() && value.is_empty() {
                continue;
            }
            let domain = extract_domain(&key);

            let provenance: Vec<Provenance> =
                serde_json::from_str(prov_str).unwrap_or_default();
            let supersedes_id = provenance
                .iter()
                .find(|p| p.rel == "supersedes")
                .map(|p| p.target.as_str());

            self.conn.execute(
                "INSERT OR IGNORE INTO decisions
                 (event_id, key, value, reason, domain, branch, supersedes_id, is_active)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, TRUE)",
                params![event_id, key, value, reason, domain, branch, supersedes_id],
            )?;
        }

        // Fix is_active: deactivate decisions that have been superseded
        self.conn.execute(
            "UPDATE decisions SET is_active = FALSE
             WHERE event_id IN (
                 SELECT d_old.event_id FROM decisions d_old
                 JOIN decisions d_new ON d_new.supersedes_id = d_old.event_id
             )",
            [],
        )?;

        // Also deactivate by key+branch: for each (key, branch), only the latest is active
        // This handles cases where supersedes_id wasn't set (legacy events)
        self.conn.execute_batch(
            "UPDATE decisions SET is_active = FALSE
             WHERE rowid NOT IN (
                 SELECT MAX(d.rowid) FROM decisions d
                 GROUP BY d.key, d.branch
             ) AND is_active = TRUE",
        )?;

        self.set_schema_version(2)?;
        Ok(())
    }

    // ── Events ──────────────────────────────────────────────────────

    /// Append an event. Append-only (CONTRACT LEDGER-02).
    ///
    /// If the event is a decision (note with `"decision"` tag), the `decisions`
    /// table is also updated atomically within the same transaction.
    pub fn append_event(&self, event: &Event) -> anyhow::Result<()> {
        let payload = serde_json::to_string(&event.payload)?;
        let refs_blobs = serde_json::to_string(&event.refs.blobs)?;
        let refs_events = serde_json::to_string(&event.refs.events)?;
        let refs_provenance = serde_json::to_string(&event.refs.provenance)?;
        let digests = serde_json::to_string(&event.digests)?;

        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
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

        // Materialize decision if applicable
        if is_decision_event(event) {
            let (key, value, reason) = extract_decision_from_payload(&event.payload);
            if !key.is_empty() || !value.is_empty() {
                let domain = extract_domain(&key);
                let supersedes_id = event
                    .refs
                    .provenance
                    .iter()
                    .find(|p| p.rel == "supersedes")
                    .map(|p| p.target.as_str());

                // Deactivate prior decision with same key on same branch
                tx.execute(
                    "UPDATE decisions SET is_active = FALSE
                     WHERE key = ?1 AND branch = ?2 AND is_active = TRUE",
                    params![key, event.branch],
                )?;

                tx.execute(
                    "INSERT INTO decisions
                     (event_id, key, value, reason, domain, branch, supersedes_id, is_active)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, TRUE)",
                    params![
                        event.event_id,
                        key,
                        value,
                        reason,
                        domain,
                        event.branch,
                        supersedes_id
                    ],
                )?;
            }
        }

        tx.commit()?;
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

    // ── Decisions ───────────────────────────────────────────────────

    /// Query active decisions, optionally filtered by domain or key prefix.
    pub fn active_decisions(
        &self,
        domain: Option<&str>,
        key_pattern: Option<&str>,
    ) -> anyhow::Result<Vec<DecisionRow>> {
        let sql = match (domain, key_pattern) {
            (Some(_), _) => {
                "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                        d.supersedes_id, d.is_active, e.ts
                 FROM decisions d JOIN events e ON d.event_id = e.event_id
                 WHERE d.is_active = TRUE AND d.domain = ?1
                 ORDER BY d.domain, d.key"
            }
            (_, Some(_)) => {
                "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                        d.supersedes_id, d.is_active, e.ts
                 FROM decisions d JOIN events e ON d.event_id = e.event_id
                 WHERE d.is_active = TRUE AND (d.key LIKE ?1 OR d.value LIKE ?1)
                 ORDER BY d.domain, d.key"
            }
            (None, None) => {
                "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                        d.supersedes_id, d.is_active, e.ts
                 FROM decisions d JOIN events e ON d.event_id = e.event_id
                 WHERE d.is_active = TRUE
                 ORDER BY d.domain, d.key"
            }
        };

        let param: String = match (domain, key_pattern) {
            (Some(d), _) => d.to_string(),
            (_, Some(k)) => format!("%{k}%"),
            _ => String::new(),
        };

        let mut stmt = self.conn.prepare(sql)?;
        let rows = if domain.is_some() || key_pattern.is_some() {
            stmt.query_map(params![param], map_decision_row)?
        } else {
            stmt.query_map([], map_decision_row)?
        };

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("decision query failed: {e}"))
    }

    /// All decisions for a key (active + superseded), ordered by time.
    pub fn decision_timeline(&self, key: &str) -> anyhow::Result<Vec<DecisionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts
             FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.key = ?1
             ORDER BY e.ts",
        )?;
        let rows = stmt.query_map(params![key], map_decision_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("decision timeline query failed: {e}"))
    }

    /// Find the active decision for a specific key on a branch.
    pub fn find_active_decision(
        &self,
        branch: &str,
        key: &str,
    ) -> anyhow::Result<Option<DecisionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts
             FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.key = ?1 AND d.branch = ?2 AND d.is_active = TRUE
             LIMIT 1",
        )?;
        let result = stmt
            .query_map(params![key, branch], map_decision_row)?
            .next();
        match result {
            Some(Ok(row)) => Ok(Some(row)),
            Some(Err(e)) => Err(anyhow::anyhow!("decision query failed: {e}")),
            None => Ok(None),
        }
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

// ── Decision helpers ────────────────────────────────────────────────

/// Check if an event is a decision (note with "decision" tag).
fn is_decision_event(event: &Event) -> bool {
    event.event_type == "note"
        && event
            .payload
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|t| t.as_str() == Some("decision")))
            .unwrap_or(false)
}

/// Check if a payload JSON contains a "decision" tag.
fn is_decision_payload(payload: &serde_json::Value) -> bool {
    payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|t| t.as_str() == Some("decision")))
        .unwrap_or(false)
}

/// Extract (key, value, reason) from a payload.
/// Prefers structured `payload.decision`, falls back to text parse.
fn extract_decision_from_payload(payload: &serde_json::Value) -> (String, String, String) {
    if let Some(d) = payload.get("decision") {
        let key = d
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let value = d
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let reason = d
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        return (key, value, reason);
    }
    // Fallback: parse text "key: value — reason"
    let text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let (key, rest) = match text.split_once(": ") {
        Some((k, r)) => (k.to_string(), r),
        None => return (String::new(), text.to_string(), String::new()),
    };
    let (value, reason) = match rest.split_once(" — ") {
        Some((v, r)) => (v.to_string(), r.to_string()),
        None => (rest.to_string(), String::new()),
    };
    (key, value, reason)
}

/// Extract domain from a decision key: "db.engine" → "db".
fn extract_domain(key: &str) -> String {
    key.split('.').next().unwrap_or(key).to_string()
}

fn map_decision_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DecisionRow> {
    Ok(DecisionRow {
        event_id: row.get(0)?,
        key: row.get(1)?,
        value: row.get(2)?,
        reason: row.get(3)?,
        domain: row.get(4)?,
        branch: row.get(5)?,
        supersedes_id: row.get(6)?,
        is_active: row.get(7)?,
        ts: row.get(8)?,
    })
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

        finalize_event(&mut event);
        event
    }

    #[test]
    fn decision_materialized_on_append() {
        let (dir, store) = tmp_db();
        let e = make_decision_event("main", "db.engine", "postgres", Some("JSONB support"), None);
        store.append_event(&e).unwrap();

        let active = store.active_decisions(None, None).unwrap();
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

        let active = store.active_decisions(None, None).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].value, "postgres");
        assert_eq!(active[0].supersedes_id.as_deref(), Some(d1_id.as_str()));

        // Timeline should show both
        let timeline = store.decision_timeline("db.engine").unwrap();
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
            .append_event(&make_decision_event("main", "db.engine", "postgres", None, None))
            .unwrap();
        store
            .append_event(&make_decision_event("main", "db.pool_size", "10", None, None))
            .unwrap();
        store
            .append_event(&make_decision_event("main", "auth.method", "JWT", None, None))
            .unwrap();

        let db_decisions = store.active_decisions(Some("db"), None).unwrap();
        assert_eq!(db_decisions.len(), 2);

        let auth_decisions = store.active_decisions(Some("auth"), None).unwrap();
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
        let mut event =
            new_note_event("main", None, "system", "orm: sqlx — compile-time checks", &tags)
                .unwrap();
        // Do NOT add payload.decision — simulate legacy format
        // Remove it if new_note_event somehow adds it (it doesn't)
        event.payload.as_object_mut().unwrap().remove("decision");
        finalize_event(&mut event);
        store.append_event(&event).unwrap();

        let active = store.active_decisions(None, None).unwrap();
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
            .append_event(&make_decision_event("main", "db.engine", "postgres", None, None))
            .unwrap();
        store
            .append_event(&make_decision_event("main", "auth.method", "JWT", None, None))
            .unwrap();
        store
            .append_event(&make_decision_event("main", "cache.driver", "redis", None, None))
            .unwrap();

        // Search by key/value pattern
        let results = store.active_decisions(None, Some("postgres")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "db.engine");

        let results = store.active_decisions(None, Some("auth")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "auth.method");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_active_decision_by_branch_key() {
        let (dir, store) = tmp_db();
        store
            .append_event(&make_decision_event("main", "db.engine", "postgres", None, None))
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
            .append_event(&make_decision_event("main", "db.engine", "postgres", None, None))
            .unwrap();
        store
            .append_event(&make_decision_event("dev", "db.engine", "sqlite", None, None))
            .unwrap();

        let all = store.active_decisions(None, None).unwrap();
        assert_eq!(all.len(), 2);

        let main = store.find_active_decision("main", "db.engine").unwrap().unwrap();
        assert_eq!(main.value, "postgres");

        let dev = store.find_active_decision("dev", "db.engine").unwrap().unwrap();
        assert_eq!(dev.value, "sqlite");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_migration_v1_to_v2() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir()
            .join(format!("edda_sqlite_migrate_{}_{n}", std::process::id()));
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
        assert_eq!(store.schema_version().unwrap(), 2);

        // Verify decisions table was populated by backfill
        let active = store.active_decisions(None, None).unwrap();
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

        let active = store.active_decisions(None, None).unwrap();
        assert!(active.is_empty());

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
}
