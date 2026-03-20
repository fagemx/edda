//! SQLite-backed storage for the edda ledger.
//!
//! Replaces the file-based storage (events.jsonl, refs/HEAD, refs/branches.json)
//! with a single `ledger.db` SQLite file using WAL mode.

use edda_core::types::{Digest, Event, Provenance, Refs};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::time::Instant;
use tracing::debug;

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

const SCHEMA_V4_SQL: &str = "
CREATE TABLE IF NOT EXISTS decision_deps (
    source_key TEXT NOT NULL,
    target_key TEXT NOT NULL,
    dep_type TEXT NOT NULL,
    created_event TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (source_key, target_key)
);
CREATE INDEX IF NOT EXISTS idx_deps_target ON decision_deps(target_key);
CREATE INDEX IF NOT EXISTS idx_deps_source ON decision_deps(source_key);
";

const SCHEMA_V6_SQL: &str = "
CREATE TABLE IF NOT EXISTS task_briefs (
    task_id         TEXT PRIMARY KEY,
    intake_event_id TEXT NOT NULL REFERENCES events(event_id),
    title           TEXT NOT NULL,
    intent          TEXT NOT NULL,
    source_url      TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'active',
    branch          TEXT NOT NULL,
    iterations      INTEGER NOT NULL DEFAULT 0,
    artifacts       TEXT NOT NULL DEFAULT '[]',
    decisions       TEXT NOT NULL DEFAULT '[]',
    last_feedback   TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_briefs_status ON task_briefs(status);
CREATE INDEX IF NOT EXISTS idx_task_briefs_branch ON task_briefs(branch);
CREATE INDEX IF NOT EXISTS idx_task_briefs_intent ON task_briefs(intent);
";

const SCHEMA_V7_SQL: &str = "
CREATE TABLE IF NOT EXISTS device_tokens (
    token_hash      TEXT PRIMARY KEY,
    device_name     TEXT NOT NULL,
    paired_at       TEXT NOT NULL,
    paired_from_ip  TEXT NOT NULL DEFAULT '',
    revoked_at      TEXT,
    pair_event_id   TEXT NOT NULL REFERENCES events(event_id),
    revoke_event_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_device_tokens_name ON device_tokens(device_name);
CREATE INDEX IF NOT EXISTS idx_device_tokens_active ON device_tokens(revoked_at) WHERE revoked_at IS NULL;
";

const SCHEMA_V8_SQL: &str = "
CREATE TABLE IF NOT EXISTS decide_snapshots (
    event_id        TEXT PRIMARY KEY REFERENCES events(event_id),
    context_hash    TEXT NOT NULL,
    engine_version  TEXT NOT NULL,
    schema_version  TEXT NOT NULL DEFAULT 'snapshot.v1',
    redaction_level TEXT NOT NULL DEFAULT 'full',
    village_id      TEXT,
    cycle_id        TEXT,
    has_blobs       BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snapshots_context_hash ON decide_snapshots(context_hash);
CREATE INDEX IF NOT EXISTS idx_snapshots_village ON decide_snapshots(village_id) WHERE village_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_snapshots_engine ON decide_snapshots(engine_version);
CREATE INDEX IF NOT EXISTS idx_snapshots_village_engine ON decide_snapshots(village_id, engine_version);
";

const SCHEMA_V3_SQL: &str = "
CREATE TABLE IF NOT EXISTS review_bundles (
    event_id TEXT PRIMARY KEY REFERENCES events(event_id),
    bundle_id TEXT UNIQUE NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    risk_level TEXT NOT NULL,
    total_added INTEGER NOT NULL DEFAULT 0,
    total_deleted INTEGER NOT NULL DEFAULT 0,
    files_changed INTEGER NOT NULL DEFAULT 0,
    tests_passed INTEGER NOT NULL DEFAULT 0,
    tests_failed INTEGER NOT NULL DEFAULT 0,
    suggested_action TEXT NOT NULL,
    branch TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_bundles_status ON review_bundles(status);
CREATE INDEX IF NOT EXISTS idx_bundles_bundle_id ON review_bundles(bundle_id);
";

const SCHEMA_V5_SQL: &str = "
ALTER TABLE decisions ADD COLUMN scope TEXT NOT NULL DEFAULT 'local';
ALTER TABLE decisions ADD COLUMN source_project_id TEXT;
ALTER TABLE decisions ADD COLUMN source_event_id TEXT;
CREATE INDEX IF NOT EXISTS idx_decisions_scope ON decisions(scope) WHERE scope != 'local';
CREATE INDEX IF NOT EXISTS idx_decisions_source ON decisions(source_project_id) WHERE source_project_id IS NOT NULL;
";

const SCHEMA_V9_SQL: &str = "
CREATE INDEX IF NOT EXISTS idx_decisions_active_domain_branch
    ON decisions(is_active, domain, branch) WHERE is_active = TRUE;
CREATE INDEX IF NOT EXISTS idx_decisions_active_domain
    ON decisions(is_active, domain) WHERE is_active = TRUE;
";

const SCHEMA_V10_SQL: &str = "
ALTER TABLE decisions ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE decisions ADD COLUMN authority TEXT NOT NULL DEFAULT 'human';
ALTER TABLE decisions ADD COLUMN affected_paths TEXT NOT NULL DEFAULT '[]';
ALTER TABLE decisions ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE decisions ADD COLUMN review_after TEXT;
ALTER TABLE decisions ADD COLUMN reversibility TEXT NOT NULL DEFAULT 'medium';

-- Backfill: sync status from existing is_active boolean
UPDATE decisions SET status = CASE WHEN is_active = 1 THEN 'active' ELSE 'superseded' END;

-- Indexes for status-based queries
CREATE INDEX IF NOT EXISTS idx_decisions_status
    ON decisions(status);
CREATE INDEX IF NOT EXISTS idx_decisions_status_domain
    ON decisions(status, domain);
CREATE INDEX IF NOT EXISTS idx_decisions_affected_paths
    ON decisions(affected_paths) WHERE affected_paths != '[]';
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
    /// Decision propagation scope: "local", "shared", or "global".
    pub scope: String,
    /// Source project ID if this decision was imported from another project.
    pub source_project_id: Option<String>,
    /// Source event ID if this decision was imported from another project.
    pub source_event_id: Option<String>,
    /// Lifecycle status: "proposed", "active", "experimental", "deprecated", "superseded"
    pub status: String,
    /// Decision authority: "human", "agent", "system"
    pub authority: String,
    /// JSON array of glob patterns for guarded file paths
    pub affected_paths: String,
    /// JSON array of tag strings
    pub tags: String,
    /// Optional ISO-8601 date for scheduled re-evaluation
    pub review_after: Option<String>,
    /// Reversibility level: "easy", "medium", "hard"
    pub reversibility: String,
}

/// An entry in a causal chain traversal result.
#[derive(Debug, Clone)]
pub struct ChainEntry {
    pub decision: DecisionRow,
    pub relation: String,
    pub depth: usize,
}

/// A row from the `review_bundles` table.
#[derive(Debug, Clone)]
pub struct BundleRow {
    pub event_id: String,
    pub bundle_id: String,
    pub status: String,
    pub risk_level: String,
    pub total_added: i64,
    pub total_deleted: i64,
    pub files_changed: i64,
    pub tests_passed: i64,
    pub tests_failed: i64,
    pub suggested_action: String,
    pub branch: String,
    pub created_at: String,
}

/// A row from the `decision_deps` table.
#[derive(Debug, Clone)]
pub struct DepRow {
    pub source_key: String,
    pub target_key: String,
    pub dep_type: String,
    pub created_event: Option<String>,
    pub created_at: String,
}

/// Parameters for inserting an imported decision from another project.
pub struct ImportParams<'a> {
    pub event: &'a edda_core::types::Event,
    pub key: &'a str,
    pub value: &'a str,
    pub reason: &'a str,
    pub domain: &'a str,
    pub scope: &'a str,
    pub source_project_id: &'a str,
    pub source_event_id: &'a str,
    pub is_active: bool,
}

/// A row from the `task_briefs` table.
#[derive(Debug, Clone)]
pub struct TaskBriefRow {
    pub task_id: String,
    pub intake_event_id: String,
    pub title: String,
    pub intent: edda_core::types::TaskBriefIntent,
    pub source_url: String,
    pub status: edda_core::types::TaskBriefStatus,
    pub branch: String,
    pub iterations: i64,
    pub artifacts: String,
    pub decisions: String,
    pub last_feedback: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A row from the `device_tokens` table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceTokenRow {
    pub token_hash: String,
    pub device_name: String,
    pub paired_at: String,
    pub paired_from_ip: String,
    pub revoked_at: Option<String>,
    pub pair_event_id: String,
    pub revoke_event_id: Option<String>,
}

/// A row from the `decide_snapshots` table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DecideSnapshotRow {
    pub event_id: String,
    pub context_hash: String,
    pub engine_version: String,
    pub schema_version: String,
    pub redaction_level: String,
    pub village_id: Option<String>,
    pub cycle_id: Option<String>,
    pub has_blobs: bool,
    pub created_at: String,
}

/// Aggregated outcome metrics for a decision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutcomeMetrics {
    pub decision_event_id: String,
    pub decision_key: String,
    pub decision_value: String,
    pub decision_ts: String,
    pub total_executions: u64,
    pub success_count: u64,
    pub failed_count: u64,
    pub cancelled_count: u64,
    pub success_rate: f64,
    pub total_cost_usd: f64,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub avg_latency_ms: f64,
    pub first_execution_ts: Option<String>,
    pub last_execution_ts: Option<String>,
}

/// An execution event linked to a decision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecutionLinked {
    pub event_id: String,
    pub ts: String,
    pub status: String,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub cost_usd: Option<f64>,
    pub token_in: Option<u64>,
    pub token_out: Option<u64>,
    pub latency_ms: Option<u64>,
}

/// Map a decision status string to the legacy is_active boolean.
///
/// `is_active = true` iff status is "active" or "experimental".
/// This enforces CONTRACT COMPAT-01.
fn status_to_is_active(status: &str) -> bool {
    matches!(status, "active" | "experimental")
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

        // Migrate to v3 if needed
        let current = self.schema_version()?;
        if current < 3 {
            self.migrate_v2_to_v3()?;
        }

        // Migrate to v4 if needed
        let current = self.schema_version()?;
        if current < 4 {
            self.migrate_v3_to_v4()?;
        }

        // Migrate to v5 if needed (cross-project sync fields)
        let current = self.schema_version()?;
        if current < 5 {
            self.migrate_v4_to_v5()?;
        }

        // Migrate to v6 if needed (task_briefs materialized view)
        let current = self.schema_version()?;
        if current < 6 {
            self.migrate_v5_to_v6()?;
        }

        // Migrate to v7 if needed (device_tokens table)
        let current = self.schema_version()?;
        if current < 7 {
            self.migrate_v6_to_v7()?;
        }

        // Migrate to v8 if needed (decide_snapshots table)
        let current = self.schema_version()?;
        if current < 8 {
            self.migrate_v7_to_v8()?;
        }

        // Migrate to v9 if needed (hot path query optimization indexes)
        let current = self.schema_version()?;
        if current < 9 {
            self.migrate_v8_to_v9()?;
        }

        // Migrate to v10 if needed (decision deepening columns)
        let current = self.schema_version()?;
        if current < 10 {
            self.migrate_v9_to_v10()?;
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

            if !edda_core::decision::is_decision(&payload) {
                continue;
            }

            let dp = match edda_core::decision::extract_decision(&payload) {
                Some(dp) => dp,
                None => continue,
            };
            let key = &dp.key;
            let value = &dp.value;
            let reason = dp.reason.as_deref().unwrap_or("");
            let domain = edda_core::decision::extract_domain(key);

            let provenance: Vec<Provenance> = serde_json::from_str(prov_str).unwrap_or_default();
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

    fn migrate_v2_to_v3(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA_V3_SQL)?;

        // Backfill: scan existing review_bundle events
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, branch, payload FROM events
             WHERE event_type = 'review_bundle' ORDER BY rowid",
        )?;
        let rows: Vec<(String, String, String, String)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        for (event_id, ts, branch, payload_str) in &rows {
            let payload: serde_json::Value = match serde_json::from_str(payload_str) {
                Ok(v) => v,
                Err(_) => continue,
            };
            materialize_bundle_sql(&self.conn, event_id, ts, branch, &payload)?;
        }

        self.set_schema_version(3)?;
        Ok(())
    }

    fn migrate_v3_to_v4(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA_V4_SQL)?;

        // Backfill: create star-shaped auto_domain edges for existing active decisions
        let mut stmt = self
            .conn
            .prepare("SELECT key, domain FROM decisions WHERE is_active = TRUE ORDER BY rowid")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        // Group by domain
        let mut domain_keys: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (key, domain) in &rows {
            domain_keys
                .entry(domain.clone())
                .or_default()
                .push(key.clone());
        }

        let now = time_now_rfc3339();
        for keys in domain_keys.values() {
            if keys.len() < 2 {
                continue;
            }
            // Star-shaped: each key after the first depends_on all prior keys
            for i in 1..keys.len() {
                for j in 0..i {
                    self.conn.execute(
                        "INSERT OR IGNORE INTO decision_deps
                         (source_key, target_key, dep_type, created_event, created_at)
                         VALUES (?1, ?2, 'auto_domain', NULL, ?3)",
                        params![keys[i], keys[j], now],
                    )?;
                }
            }
        }

        self.set_schema_version(4)?;
        Ok(())
    }

    fn migrate_v4_to_v5(&self) -> anyhow::Result<()> {
        // Add cross-project sync columns to decisions table.
        // SQLite ALTER TABLE ADD COLUMN must be done one at a time.
        // Use a check to see if column already exists (idempotent).
        let has_scope: bool = self
            .conn
            .prepare("SELECT scope FROM decisions LIMIT 0")
            .is_ok();
        if !has_scope {
            self.conn.execute_batch(SCHEMA_V5_SQL)?;
        }
        self.set_schema_version(5)?;
        Ok(())
    }

    fn migrate_v5_to_v6(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA_V6_SQL)?;

        // Backfill: scan existing task_intake events
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, branch, payload FROM events
             WHERE event_type = 'task_intake' ORDER BY rowid",
        )?;
        let rows: Vec<(String, String, String, String)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        for (event_id, ts, branch, payload_str) in &rows {
            let payload: serde_json::Value = match serde_json::from_str(payload_str) {
                Ok(v) => v,
                Err(_) => continue,
            };
            materialize_task_brief_sql(&self.conn, event_id, ts, branch, &payload)?;
        }

        // Backfill updates: scan commits, notes, and merges on brief branches
        self.backfill_task_brief_updates()?;

        self.set_schema_version(6)?;
        Ok(())
    }

    fn migrate_v6_to_v7(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA_V7_SQL)?;
        // No backfill needed — device_tokens is a new feature with no existing data.
        self.set_schema_version(7)?;
        Ok(())
    }

    fn migrate_v7_to_v8(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA_V8_SQL)?;

        // Backfill: scan existing decide_snapshot events
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, payload FROM events
             WHERE event_type = 'decide_snapshot' ORDER BY rowid",
        )?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        for (event_id, ts, payload_str) in &rows {
            let payload: serde_json::Value = match serde_json::from_str(payload_str) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let context_hash = payload["context_hash"].as_str().unwrap_or("");
            let engine_version = payload["engine_version"].as_str().unwrap_or("");
            let schema_version = payload["schema_version"].as_str().unwrap_or("snapshot.v1");
            let redaction_level = payload["redaction_level"].as_str().unwrap_or("full");
            let village_id = payload["village_id"].as_str();
            let cycle_id = payload["cycle_id"].as_str();
            let has_blobs =
                payload.get("context_blob").is_some() || payload.get("result_blob").is_some();

            self.conn.execute(
                "INSERT OR IGNORE INTO decide_snapshots
                 (event_id, context_hash, engine_version, schema_version,
                  redaction_level, village_id, cycle_id, has_blobs, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    event_id,
                    context_hash,
                    engine_version,
                    schema_version,
                    redaction_level,
                    village_id,
                    cycle_id,
                    has_blobs,
                    ts
                ],
            )?;
        }

        self.set_schema_version(8)?;
        Ok(())
    }

    fn migrate_v8_to_v9(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA_V9_SQL)?;
        self.set_schema_version(9)?;
        Ok(())
    }

    fn migrate_v9_to_v10(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA_V10_SQL)?;
        self.set_schema_version(10)?;
        Ok(())
    }

    /// Backfill task brief updates from existing commit/note/merge events.
    fn backfill_task_brief_updates(&self) -> anyhow::Result<()> {
        let mut brief_stmt = self
            .conn
            .prepare("SELECT task_id, branch, created_at FROM task_briefs")?;
        let briefs: Vec<(String, String, String)> = brief_stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        for (_task_id, branch, created_at) in &briefs {
            // Count commits on this branch after the intake
            let commit_count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM events
                 WHERE branch = ?1 AND event_type = 'commit' AND ts >= ?2",
                params![branch, created_at],
                |row| row.get(0),
            )?;

            // Collect artifacts from commit payloads
            let mut artifacts: Vec<String> = Vec::new();
            let mut art_stmt = self.conn.prepare(
                "SELECT payload FROM events
                 WHERE branch = ?1 AND event_type = 'commit' AND ts >= ?2
                 ORDER BY rowid",
            )?;
            let payloads: Vec<String> = art_stmt
                .query_map(params![branch, created_at], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for p_str in &payloads {
                if let Ok(p) = serde_json::from_str::<serde_json::Value>(p_str) {
                    extract_artifacts_from_payload(&p, &mut artifacts);
                }
            }

            // Scan all notes on this branch to find feedback and decision tags.
            // We parse JSON in Rust instead of using SQL LIKE to avoid false matches.
            let mut note_stmt = self.conn.prepare(
                "SELECT payload FROM events
                 WHERE branch = ?1 AND event_type = 'note' AND ts >= ?2
                 ORDER BY rowid",
            )?;
            let note_payloads: Vec<String> = note_stmt
                .query_map(params![branch, created_at], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;

            let mut last_feedback: Option<String> = None;
            let mut decision_keys: Vec<String> = Vec::new();

            for p_str in &note_payloads {
                let p: serde_json::Value = match serde_json::from_str(p_str) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let tags = payload_tags(&p);
                if tags.iter().any(|t| t == "review" || t == "feedback") {
                    if let Some(fb) = extract_feedback_from_payload(&p) {
                        last_feedback = Some(fb);
                    }
                }
                if tags.iter().any(|t| t == "decision") {
                    if let Some(key) = p["decision"]["key"].as_str() {
                        if !decision_keys.contains(&key.to_string()) {
                            decision_keys.push(key.to_string());
                        }
                    }
                }
            }

            // Check for merge (completion)
            let has_merge: bool = self.conn.query_row(
                "SELECT COUNT(*) > 0 FROM events
                 WHERE branch = ?1 AND event_type = 'merge' AND ts >= ?2",
                params![branch, created_at],
                |row| row.get(0),
            )?;

            // Get updated_at from the latest event on this branch
            let latest_ts: Option<String> = self
                .conn
                .query_row(
                    "SELECT ts FROM events
                     WHERE branch = ?1 AND ts >= ?2
                     ORDER BY rowid DESC LIMIT 1",
                    params![branch, created_at],
                    |row| row.get(0),
                )
                .optional()?;

            let artifacts_json =
                serde_json::to_string(&artifacts).unwrap_or_else(|_| "[]".to_string());
            let decisions_json =
                serde_json::to_string(&decision_keys).unwrap_or_else(|_| "[]".to_string());
            let status = if has_merge {
                edda_core::types::TaskBriefStatus::Completed
            } else {
                edda_core::types::TaskBriefStatus::Active
            };

            self.conn.execute(
                "UPDATE task_briefs SET
                    iterations = ?1,
                    artifacts = ?2,
                    decisions = ?3,
                    last_feedback = ?4,
                    status = ?5,
                    updated_at = COALESCE(?6, updated_at)
                 WHERE branch = ?7 AND created_at = ?8",
                params![
                    commit_count,
                    artifacts_json,
                    decisions_json,
                    last_feedback,
                    status.as_str(),
                    latest_ts,
                    branch,
                    created_at,
                ],
            )?;
        }

        Ok(())
    }

    // ── Device Tokens ──────────────────────────────────────────────

    /// Insert a new device token row.
    pub fn insert_device_token(&self, row: &DeviceTokenRow) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO device_tokens
             (token_hash, device_name, paired_at, paired_from_ip, revoked_at, pair_event_id, revoke_event_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                row.token_hash,
                row.device_name,
                row.paired_at,
                row.paired_from_ip,
                row.revoked_at,
                row.pair_event_id,
                row.revoke_event_id,
            ],
        )?;
        Ok(())
    }

    /// Validate a device token by its SHA-256 hash. Returns the row if active (not revoked).
    pub fn validate_device_token(
        &self,
        token_hash: &str,
    ) -> anyhow::Result<Option<DeviceTokenRow>> {
        let row = self
            .conn
            .query_row(
                "SELECT token_hash, device_name, paired_at, paired_from_ip,
                        revoked_at, pair_event_id, revoke_event_id
                 FROM device_tokens
                 WHERE token_hash = ?1 AND revoked_at IS NULL",
                params![token_hash],
                |row| {
                    Ok(DeviceTokenRow {
                        token_hash: row.get(0)?,
                        device_name: row.get(1)?,
                        paired_at: row.get(2)?,
                        paired_from_ip: row.get(3)?,
                        revoked_at: row.get(4)?,
                        pair_event_id: row.get(5)?,
                        revoke_event_id: row.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// List all device tokens (active and revoked).
    pub fn list_device_tokens(&self) -> anyhow::Result<Vec<DeviceTokenRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT token_hash, device_name, paired_at, paired_from_ip,
                    revoked_at, pair_event_id, revoke_event_id
             FROM device_tokens
             ORDER BY paired_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DeviceTokenRow {
                    token_hash: row.get(0)?,
                    device_name: row.get(1)?,
                    paired_at: row.get(2)?,
                    paired_from_ip: row.get(3)?,
                    revoked_at: row.get(4)?,
                    pair_event_id: row.get(5)?,
                    revoke_event_id: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Revoke a device token by name. Returns true if a token was revoked.
    pub fn revoke_device_token(
        &self,
        device_name: &str,
        revoke_event_id: &str,
    ) -> anyhow::Result<bool> {
        let now = time_now_rfc3339();
        let count = self.conn.execute(
            "UPDATE device_tokens
             SET revoked_at = ?1, revoke_event_id = ?2
             WHERE device_name = ?3 AND revoked_at IS NULL",
            params![now, revoke_event_id, device_name],
        )?;
        Ok(count > 0)
    }

    /// Revoke all active device tokens. Returns the count of revoked tokens.
    pub fn revoke_all_device_tokens(&self, revoke_event_id: &str) -> anyhow::Result<u64> {
        let now = time_now_rfc3339();
        let count = self.conn.execute(
            "UPDATE device_tokens
             SET revoked_at = ?1, revoke_event_id = ?2
             WHERE revoked_at IS NULL",
            params![now, revoke_event_id],
        )?;
        Ok(count as u64)
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
        if event.event_type == "note" && edda_core::decision::is_decision(&event.payload) {
            if let Some(dp) = edda_core::decision::extract_decision(&event.payload) {
                let domain = edda_core::decision::extract_domain(&dp.key);
                let reason = dp.reason.as_deref().unwrap_or("");
                let key = &dp.key;
                let value = &dp.value;
                let supersedes_id = event
                    .refs
                    .provenance
                    .iter()
                    .find(|p| p.rel == "supersedes")
                    .map(|p| p.target.as_str());

                // Deactivate prior decision with same key on same branch
                tx.execute(
                    "UPDATE decisions SET is_active = FALSE, status = 'superseded'
                     WHERE key = ?1 AND branch = ?2 AND is_active = TRUE",
                    params![key, event.branch],
                )?;

                let scope_str = dp
                    .scope
                    .unwrap_or(edda_core::types::DecisionScope::Local)
                    .to_string();

                // Read new V10 fields from payload, with safe defaults
                let status = "active";
                let is_active = status_to_is_active(status);
                let authority = dp.authority.as_deref().unwrap_or("human");
                let affected_paths = dp
                    .affected_paths
                    .as_ref()
                    .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()))
                    .unwrap_or_else(|| "[]".to_string());
                let tags = dp
                    .tags
                    .as_ref()
                    .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()))
                    .unwrap_or_else(|| "[]".to_string());
                let review_after = dp.review_after.as_deref();
                let reversibility = dp.reversibility.as_deref().unwrap_or("medium");

                tx.execute(
                    "INSERT INTO decisions
                     (event_id, key, value, reason, domain, branch, supersedes_id,
                      is_active, scope, status, authority, affected_paths, tags,
                      review_after, reversibility)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                             ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        event.event_id,
                        key,
                        value,
                        reason,
                        domain,
                        event.branch,
                        supersedes_id,
                        is_active,
                        scope_str,
                        status,
                        authority,
                        affected_paths,
                        tags,
                        review_after,
                        reversibility,
                    ],
                )?;
            }
        }

        // Materialize review bundle if applicable
        if event.event_type == "review_bundle" {
            materialize_bundle_sql(
                &tx,
                &event.event_id,
                &event.ts,
                &event.branch,
                &event.payload,
            )?;
        }

        // Materialize task brief on intake
        if event.event_type == "task_intake" {
            materialize_task_brief_sql(
                &tx,
                &event.event_id,
                &event.ts,
                &event.branch,
                &event.payload,
            )?;
        }

        // Update task brief on commit (same branch, increment iterations)
        if event.event_type == "commit" {
            update_task_brief_on_commit(&tx, event)?;
        }

        // Update task brief on note with review/feedback tag
        if event.event_type == "note" {
            update_task_brief_on_note(&tx, event)?;
        }

        // Update task brief on merge (mark completed)
        if event.event_type == "merge" {
            update_task_brief_on_merge(&tx, event)?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Append an event idempotently. Returns `true` if inserted, `false` if duplicate.
    ///
    /// Uses `INSERT OR IGNORE` so that a duplicate `event_id` is silently skipped
    /// without returning an error. This is used by the Karvi event consumer to
    /// handle webhook retries gracefully.
    pub fn append_event_idempotent(&self, event: &Event) -> anyhow::Result<bool> {
        let payload = serde_json::to_string(&event.payload)?;
        let refs_blobs = serde_json::to_string(&event.refs.blobs)?;
        let refs_events = serde_json::to_string(&event.refs.events)?;
        let refs_provenance = serde_json::to_string(&event.refs.provenance)?;
        let digests = serde_json::to_string(&event.digests)?;

        self.conn.execute(
            "INSERT OR IGNORE INTO events (
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

        Ok(self.conn.changes() > 0)
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
            .query_map([], map_event_row)?
            .collect::<Result<Vec<_>, _>>()?;

        events.into_iter().map(row_to_event).collect()
    }

    /// Get all events of a given type, filtered at the SQL level using `idx_events_type`.
    pub fn iter_events_by_type(&self, event_type: &str) -> anyhow::Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
             FROM events WHERE event_type = ?1 ORDER BY rowid",
        )?;

        let events = stmt
            .query_map(params![event_type], map_event_row)?
            .collect::<Result<Vec<_>, _>>()?;

        events.into_iter().map(row_to_event).collect()
    }

    /// Get all events for a specific branch, filtered at the SQL level using `idx_events_branch`.
    pub fn iter_branch_events(&self, branch: &str) -> anyhow::Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
             FROM events WHERE branch = ?1 ORDER BY rowid",
        )?;

        let events = stmt
            .query_map(params![branch], map_event_row)?
            .collect::<Result<Vec<_>, _>>()?;

        events.into_iter().map(row_to_event).collect()
    }

    /// Get events filtered by branch and optional type/keyword/date range/limit,
    /// all pushed down to SQL for index-backed retrieval.
    ///
    /// Results are returned in reverse insertion order (newest first), capped at `limit`.
    pub fn iter_events_filtered(
        &self,
        branch: &str,
        event_type: Option<&str>,
        keyword: Option<&str>,
        after: Option<&str>,
        before: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<Event>> {
        let mut sql = String::from(
            "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
             FROM events WHERE branch = ?",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        param_values.push(Box::new(branch.to_string()));

        if let Some(et) = event_type {
            sql.push_str(" AND event_type = ?");
            param_values.push(Box::new(et.to_string()));
        }
        if let Some(kw) = keyword {
            sql.push_str(" AND LOWER(payload) LIKE ?");
            let pattern = format!("%{}%", kw.to_lowercase());
            param_values.push(Box::new(pattern));
        }
        if let Some(a) = after {
            sql.push_str(" AND ts >= ?");
            param_values.push(Box::new(a.to_string()));
        }
        if let Some(b) = before {
            sql.push_str(" AND ts <= ?");
            param_values.push(Box::new(b.to_string()));
        }
        sql.push_str(" ORDER BY rowid DESC LIMIT ?");
        param_values.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;

        let events = stmt
            .query_map(param_refs.as_slice(), map_event_row)?
            .collect::<Result<Vec<_>, _>>()?;

        events.into_iter().map(row_to_event).collect()
    }

    /// Find commit events related to a query by evidence chain or keyword match.
    ///
    /// Uses `idx_events_type` for `event_type = 'commit'` filtering.
    pub fn find_related_commits(
        &self,
        branch: Option<&str>,
        keyword: &str,
        decision_event_ids: &[&str],
        limit: usize,
    ) -> anyhow::Result<Vec<Event>> {
        let mut sql = String::from(
            "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
             FROM events WHERE event_type = 'commit'",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(b) = branch {
            sql.push_str(" AND branch = ?");
            param_values.push(Box::new(b.to_string()));
        }

        // Filter: keyword in payload OR evidence chain match
        if !keyword.is_empty() || !decision_event_ids.is_empty() {
            let mut conditions = Vec::new();
            if !keyword.is_empty() {
                conditions.push("LOWER(payload) LIKE ?".to_string());
                param_values.push(Box::new(format!("%{}%", keyword.to_lowercase())));
            }
            for eid in decision_event_ids {
                conditions.push("(refs_events LIKE ? OR refs_provenance LIKE ?)".to_string());
                let pattern = format!("%{}%", eid);
                param_values.push(Box::new(pattern.clone()));
                param_values.push(Box::new(pattern));
            }
            if !conditions.is_empty() {
                sql.push_str(" AND (");
                sql.push_str(&conditions.join(" OR "));
                sql.push(')');
            }
        }

        sql.push_str(" ORDER BY rowid DESC LIMIT ?");
        param_values.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;

        let events = stmt
            .query_map(param_refs.as_slice(), map_event_row)?
            .collect::<Result<Vec<_>, _>>()?;

        events.into_iter().map(row_to_event).collect()
    }

    /// Find note events matching a keyword, excluding decision notes and session digests.
    ///
    /// Uses `idx_events_type` for `event_type = 'note'` filtering.
    pub fn find_related_notes(
        &self,
        branch: Option<&str>,
        keyword: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<Event>> {
        if keyword.is_empty() {
            return Ok(vec![]);
        }

        let mut sql = String::from(
            "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
             FROM events WHERE event_type = 'note'",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(b) = branch {
            sql.push_str(" AND branch = ?");
            param_values.push(Box::new(b.to_string()));
        }

        // Keyword match on payload text
        sql.push_str(" AND LOWER(payload) LIKE ?");
        param_values.push(Box::new(format!("%{}%", keyword.to_lowercase())));

        // Exclude decision notes and session digests at SQL level
        sql.push_str(" AND payload NOT LIKE '%\"decision\"%'");
        sql.push_str(" AND payload NOT LIKE '%\"session_digest\"%'");

        sql.push_str(" ORDER BY rowid DESC LIMIT ?");
        param_values.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;

        let events = stmt
            .query_map(param_refs.as_slice(), map_event_row)?
            .collect::<Result<Vec<_>, _>>()?;

        events.into_iter().map(row_to_event).collect()
    }

    /// Get a single event by event_id.
    pub fn get_event(&self, event_id: &str) -> anyhow::Result<Option<Event>> {
        let row = self
            .conn
            .query_row(
                "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                        payload, refs_blobs, refs_events, refs_provenance,
                        schema_version, digests, event_family, event_level
                 FROM events WHERE event_id = ?1",
                params![event_id],
                map_event_row,
            )
            .optional()?;

        match row {
            Some(r) => Ok(Some(row_to_event(r)?)),
            None => Ok(None),
        }
    }

    /// Get all events with rowid strictly greater than `after_rowid`.
    ///
    /// Returns `(rowid, Event)` pairs ordered by rowid, useful for cursor-based
    /// polling (e.g. SSE streaming).
    pub fn events_after_rowid(&self, after_rowid: i64) -> anyhow::Result<Vec<(i64, Event)>> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid, event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
             FROM events WHERE rowid > ?1 ORDER BY rowid",
        )?;

        let rows = stmt
            .query_map(params![after_rowid], |row| {
                let rowid: i64 = row.get(0)?;
                let payload_str: String = row.get(7)?;
                let refs_blobs_str: String = row.get(8)?;
                let refs_events_str: String = row.get(9)?;
                let refs_prov_str: String = row.get(10)?;
                let digests_str: String = row.get(12)?;

                Ok((
                    rowid,
                    EventRow {
                        event_id: row.get(1)?,
                        ts: row.get(2)?,
                        event_type: row.get(3)?,
                        branch: row.get(4)?,
                        parent_hash: row.get(5)?,
                        hash: row.get(6)?,
                        payload_str,
                        refs_blobs_str,
                        refs_events_str,
                        refs_prov_str,
                        schema_version: row.get(11)?,
                        digests_str,
                        event_family: row.get(13)?,
                        event_level: row.get(14)?,
                    },
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        rows.into_iter()
            .map(|(rid, er)| Ok((rid, row_to_event(er)?)))
            .collect()
    }

    /// Look up the rowid for a given `event_id`.
    ///
    /// Returns `None` if the event does not exist.
    pub fn rowid_for_event_id(&self, event_id: &str) -> anyhow::Result<Option<i64>> {
        let result: Option<i64> = self
            .conn
            .query_row(
                "SELECT rowid FROM events WHERE event_id = ?1",
                params![event_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(result)
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
            .query_row("SELECT value FROM refs WHERE key = 'HEAD'", [], |row| {
                row.get(0)
            })
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
            .query_row("SELECT value FROM refs WHERE key = 'branches'", [], |row| {
                row.get(0)
            })
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
    /// `after`/`before` are optional ISO 8601 bounds for temporal filtering.
    /// `limit` caps the result set to prevent full table scans on hot path.
    pub fn active_decisions(
        &self,
        domain: Option<&str>,
        key_pattern: Option<&str>,
        after: Option<&str>,
        before: Option<&str>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<DecisionRow>> {
        let start = Instant::now();
        let has_temporal = after.is_some() || before.is_some();

        let mut sql = String::from(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility
             FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.is_active = TRUE",
        );

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(d) = domain {
            sql.push_str(&format!(" AND d.domain = ?{idx}"));
            param_values.push(Box::new(d.to_string()));
            idx += 1;
        } else if let Some(k) = key_pattern {
            let like = format!("%{k}%");
            sql.push_str(&format!(" AND (d.key LIKE ?{idx} OR d.value LIKE ?{idx})"));
            param_values.push(Box::new(like));
            idx += 1;
        }

        if let Some(a) = after {
            sql.push_str(&format!(" AND e.ts >= ?{idx}"));
            param_values.push(Box::new(a.to_string()));
            idx += 1;
        }
        if let Some(b) = before {
            sql.push_str(&format!(" AND e.ts <= ?{idx}"));
            param_values.push(Box::new(b.to_string()));
            let _ = idx + 1;
        }

        if has_temporal {
            sql.push_str(" ORDER BY e.ts DESC");
        } else {
            sql.push_str(" ORDER BY d.domain, d.key");
        }

        if let Some(lim) = limit {
            sql.push_str(&format!(" LIMIT {lim}"));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), map_decision_row)?;

        let result = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("decision query failed: {e}"))?;

        let elapsed = start.elapsed();
        debug!(
            domain = domain,
            key_pattern = key_pattern,
            limit = limit,
            result_count = result.len(),
            elapsed_ms = elapsed.as_millis() as u64,
            "active_decisions query completed"
        );

        Ok(result)
    }

    /// All decisions for a key (active + superseded), ordered by time.
    /// `after`/`before` are optional ISO 8601 bounds for temporal filtering.
    pub fn decision_timeline(
        &self,
        key: &str,
        after: Option<&str>,
        before: Option<&str>,
    ) -> anyhow::Result<Vec<DecisionRow>> {
        let has_temporal = after.is_some() || before.is_some();

        let mut sql = String::from(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility
             FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.key = ?1",
        );

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        param_values.push(Box::new(key.to_string()));
        let mut idx = 2;

        if let Some(a) = after {
            sql.push_str(&format!(" AND e.ts >= ?{idx}"));
            param_values.push(Box::new(a.to_string()));
            idx += 1;
        }
        if let Some(b) = before {
            sql.push_str(&format!(" AND e.ts <= ?{idx}"));
            param_values.push(Box::new(b.to_string()));
            let _ = idx + 1;
        }

        if has_temporal {
            sql.push_str(" ORDER BY e.ts DESC");
        } else {
            sql.push_str(" ORDER BY e.ts");
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), map_decision_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("decision timeline query failed: {e}"))
    }

    /// All decisions for a domain (active + superseded), ordered by time.
    /// `after`/`before` are optional ISO 8601 bounds for temporal filtering.
    pub fn domain_timeline(
        &self,
        domain: &str,
        after: Option<&str>,
        before: Option<&str>,
    ) -> anyhow::Result<Vec<DecisionRow>> {
        let has_temporal = after.is_some() || before.is_some();

        let mut sql = String::from(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility
             FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.domain = ?1",
        );

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        param_values.push(Box::new(domain.to_string()));
        let mut idx = 2;

        if let Some(a) = after {
            sql.push_str(&format!(" AND e.ts >= ?{idx}"));
            param_values.push(Box::new(a.to_string()));
            idx += 1;
        }
        if let Some(b) = before {
            sql.push_str(&format!(" AND e.ts <= ?{idx}"));
            param_values.push(Box::new(b.to_string()));
            let _ = idx + 1;
        }

        if has_temporal {
            sql.push_str(" ORDER BY e.ts DESC");
        } else {
            sql.push_str(" ORDER BY e.ts");
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), map_decision_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("domain timeline query failed: {e}"))
    }

    /// Distinct domain values from active decisions.
    pub fn list_domains(&self) -> anyhow::Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT domain FROM decisions WHERE is_active = TRUE ORDER BY domain",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("list domains query failed: {e}"))
    }

    // ── Cross-Project Sync ─────────────────────────────────────────────

    /// Query active decisions with shared or global scope.
    /// Used by the sync engine to find decisions that should be shared.
    pub fn shared_decisions(&self) -> anyhow::Result<Vec<DecisionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility
             FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.is_active = TRUE AND d.scope IN ('shared', 'global')
               AND d.source_project_id IS NULL
             ORDER BY d.domain, d.key",
        )?;
        let rows = stmt.query_map([], map_decision_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("shared decisions query failed: {e}"))
    }

    /// Check if a decision from a source project/event has already been imported.
    pub fn is_already_imported(
        &self,
        source_project_id: &str,
        source_event_id: &str,
    ) -> anyhow::Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM decisions
             WHERE source_project_id = ?1 AND source_event_id = ?2",
            params![source_project_id, source_event_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Insert an imported decision from another project.
    /// This writes both the event and the decisions table entry.
    pub fn insert_imported_decision(&self, p: ImportParams<'_>) -> anyhow::Result<()> {
        let payload = serde_json::to_string(&p.event.payload)?;
        let refs_blobs = serde_json::to_string(&p.event.refs.blobs)?;
        let refs_events = serde_json::to_string(&p.event.refs.events)?;
        let refs_provenance = serde_json::to_string(&p.event.refs.provenance)?;
        let digests = serde_json::to_string(&p.event.digests)?;

        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "INSERT INTO events (
                event_id, ts, event_type, branch, parent_hash, hash,
                payload, refs_blobs, refs_events, refs_provenance,
                schema_version, digests, event_family, event_level
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                p.event.event_id,
                p.event.ts,
                p.event.event_type,
                p.event.branch,
                p.event.parent_hash,
                p.event.hash,
                payload,
                refs_blobs,
                refs_events,
                refs_provenance,
                p.event.schema_version,
                digests,
                p.event.event_family,
                p.event.event_level,
            ],
        )?;

        // If active, deactivate prior local decision with same key
        if p.is_active {
            tx.execute(
                "UPDATE decisions SET is_active = FALSE, status = 'superseded'
                 WHERE key = ?1 AND branch = ?2 AND is_active = TRUE
                   AND source_project_id IS NULL",
                params![p.key, p.event.branch],
            )?;
        }

        let status = if p.is_active { "active" } else { "superseded" };
        tx.execute(
            "INSERT INTO decisions
             (event_id, key, value, reason, domain, branch, supersedes_id, is_active,
              scope, source_project_id, source_event_id, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?9, ?10, ?11)",
            params![
                p.event.event_id,
                p.key,
                p.value,
                p.reason,
                p.domain,
                p.event.branch,
                p.is_active,
                p.scope,
                p.source_project_id,
                p.source_event_id,
                status,
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    // ── Review Bundles ────────────────────────────────────────────────

    /// Get a review bundle by bundle_id.
    pub fn get_bundle(&self, bundle_id: &str) -> anyhow::Result<Option<BundleRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, bundle_id, status, risk_level, total_added, total_deleted,
                    files_changed, tests_passed, tests_failed, suggested_action, branch, created_at
             FROM review_bundles WHERE bundle_id = ?1",
        )?;
        let result = stmt.query_map(params![bundle_id], map_bundle_row)?.next();
        match result {
            Some(Ok(row)) => Ok(Some(row)),
            Some(Err(e)) => Err(anyhow::anyhow!("bundle query failed: {e}")),
            None => Ok(None),
        }
    }

    /// List review bundles, optionally filtered by status.
    pub fn list_bundles(&self, status: Option<&str>) -> anyhow::Result<Vec<BundleRow>> {
        let (sql, param) = match status {
            Some(s) => (
                "SELECT event_id, bundle_id, status, risk_level, total_added, total_deleted,
                        files_changed, tests_passed, tests_failed, suggested_action, branch, created_at
                 FROM review_bundles WHERE status = ?1
                 ORDER BY created_at DESC",
                Some(s.to_string()),
            ),
            None => (
                "SELECT event_id, bundle_id, status, risk_level, total_added, total_deleted,
                        files_changed, tests_passed, tests_failed, suggested_action, branch, created_at
                 FROM review_bundles ORDER BY created_at DESC",
                None,
            ),
        };

        let mut stmt = self.conn.prepare(sql)?;
        let rows = if let Some(p) = &param {
            stmt.query_map(params![p], map_bundle_row)?
        } else {
            stmt.query_map([], map_bundle_row)?
        };

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("bundle list query failed: {e}"))
    }

    /// Find the active decision for a specific key on a branch.
    pub fn find_active_decision(
        &self,
        branch: &str,
        key: &str,
    ) -> anyhow::Result<Option<DecisionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility
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
    // ── Decision Dependencies ────────────────────────────────────────

    /// Insert a dependency edge.
    pub fn insert_dep(
        &self,
        source_key: &str,
        target_key: &str,
        dep_type: &str,
        created_event: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = time_now_rfc3339();
        self.conn.execute(
            "INSERT OR IGNORE INTO decision_deps
             (source_key, target_key, dep_type, created_event, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![source_key, target_key, dep_type, created_event, now],
        )?;
        Ok(())
    }

    /// What does `key` depend on?
    pub fn deps_of(&self, key: &str) -> anyhow::Result<Vec<DepRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_key, target_key, dep_type, created_event, created_at
             FROM decision_deps WHERE source_key = ?1",
        )?;
        let rows = stmt.query_map(params![key], map_dep_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("deps_of query failed: {e}"))
    }

    /// Who depends on `key`?
    pub fn dependents_of(&self, key: &str) -> anyhow::Result<Vec<DepRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_key, target_key, dep_type, created_event, created_at
             FROM decision_deps WHERE target_key = ?1",
        )?;
        let rows = stmt.query_map(params![key], map_dep_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("dependents_of query failed: {e}"))
    }

    /// Transitive dependents of `key` via BFS, up to `max_depth` hops.
    /// Returns `(DepRow, DecisionRow, depth)` tuples, deduplicated by key
    /// (shortest path wins). Only active decisions are included.
    pub fn transitive_dependents_of(
        &self,
        key: &str,
        max_depth: usize,
    ) -> anyhow::Result<Vec<(DepRow, DecisionRow, usize)>> {
        use std::collections::{HashSet, VecDeque};

        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(key.to_string());
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((key.to_string(), 0));

        let mut results: Vec<(DepRow, DecisionRow, usize)> = Vec::new();

        while let Some((current_key, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let deps = self.active_dependents_of(&current_key)?;
            for (dep, decision) in deps {
                if visited.insert(decision.key.clone()) {
                    let next_depth = depth + 1;
                    queue.push_back((decision.key.clone(), next_depth));
                    results.push((dep, decision, next_depth));
                }
            }
        }

        Ok(results)
    }

    /// Who depends on `key`, joined with active decisions only.
    pub fn active_dependents_of(&self, key: &str) -> anyhow::Result<Vec<(DepRow, DecisionRow)>> {
        let mut stmt = self.conn.prepare(
            "SELECT dd.source_key, dd.target_key, dd.dep_type, dd.created_event, dd.created_at,
                    d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility
             FROM decision_deps dd
             JOIN decisions d ON d.key = dd.source_key AND d.is_active = TRUE
             JOIN events e ON d.event_id = e.event_id
             WHERE dd.target_key = ?1",
        )?;
        let rows = stmt.query_map(params![key], |row| {
            let dep = DepRow {
                source_key: row.get(0)?,
                target_key: row.get(1)?,
                dep_type: row.get(2)?,
                created_event: row.get(3)?,
                created_at: row.get(4)?,
            };
            let decision = DecisionRow {
                event_id: row.get(5)?,
                key: row.get(6)?,
                value: row.get(7)?,
                reason: row.get(8)?,
                domain: row.get(9)?,
                branch: row.get(10)?,
                supersedes_id: row.get(11)?,
                is_active: row.get(12)?,
                ts: row.get(13)?,
                scope: row.get(14)?,
                source_project_id: row.get(15)?,
                source_event_id: row.get(16)?,
                status: row.get(17)?,
                authority: row.get(18)?,
                affected_paths: row.get(19)?,
                tags: row.get(20)?,
                review_after: row.get(21)?,
                reversibility: row.get(22)?,
            };
            Ok((dep, decision))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("active_dependents_of query failed: {e}"))
    }

    // ── Causal Chain ─────────────────────────────────────────────────

    /// Look up a single decision by its event_id.
    pub fn get_decision_by_event_id(&self, event_id: &str) -> anyhow::Result<Option<DecisionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility
             FROM decisions d
             JOIN events e ON d.event_id = e.event_id
             WHERE d.event_id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![event_id], |row| {
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
                scope: row.get(9)?,
                source_project_id: row.get(10)?,
                source_event_id: row.get(11)?,
                status: row.get(12)?,
                authority: row.get(13)?,
                affected_paths: row.get(14)?,
                tags: row.get(15)?,
                review_after: row.get(16)?,
                reversibility: row.get(17)?,
            })
        })?;
        match rows.next() {
            Some(Ok(row)) => Ok(Some(row)),
            Some(Err(e)) => Err(anyhow::anyhow!("get_decision_by_event_id failed: {e}")),
            None => Ok(None),
        }
    }

    /// Traverse the causal chain from a root decision via unified BFS.
    pub fn causal_chain(
        &self,
        event_id: &str,
        max_depth: usize,
    ) -> anyhow::Result<Option<(DecisionRow, Vec<ChainEntry>)>> {
        use std::collections::{HashSet, VecDeque};

        let root = match self.get_decision_by_event_id(event_id)? {
            Some(d) => d,
            None => return Ok(None),
        };

        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(root.event_id.clone());

        let mut queue: VecDeque<(String, String, Option<String>, usize)> = VecDeque::new();
        queue.push_back((
            root.key.clone(),
            root.event_id.clone(),
            root.supersedes_id.clone(),
            0,
        ));

        let mut results: Vec<ChainEntry> = Vec::new();

        while let Some((current_key, current_event_id, current_supersedes_id, depth)) =
            queue.pop_front()
        {
            if depth >= max_depth {
                continue;
            }
            let next_depth = depth + 1;

            // 1) Dependency edges: who depends on current_key
            let dep_stmt_sql = "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                        d.supersedes_id, d.is_active, e.ts,
                        d.scope, d.source_project_id, d.source_event_id,
                        d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility,
                        dd.dep_type
                 FROM decision_deps dd
                 JOIN decisions d ON d.key = dd.source_key
                 JOIN events e ON d.event_id = e.event_id
                 WHERE dd.target_key = ?1";
            let mut dep_stmt = self.conn.prepare(dep_stmt_sql)?;
            let dep_rows = dep_stmt.query_map(params![current_key], |row| {
                let decision = DecisionRow {
                    event_id: row.get(0)?,
                    key: row.get(1)?,
                    value: row.get(2)?,
                    reason: row.get(3)?,
                    domain: row.get(4)?,
                    branch: row.get(5)?,
                    supersedes_id: row.get(6)?,
                    is_active: row.get(7)?,
                    ts: row.get(8)?,
                    scope: row.get(9)?,
                    source_project_id: row.get(10)?,
                    source_event_id: row.get(11)?,
                    status: row.get(12)?,
                    authority: row.get(13)?,
                    affected_paths: row.get(14)?,
                    tags: row.get(15)?,
                    review_after: row.get(16)?,
                    reversibility: row.get(17)?,
                };
                let dep_type: String = row.get(18)?;
                Ok((decision, dep_type))
            })?;
            for row in dep_rows {
                let (decision, dep_type) =
                    row.map_err(|e| anyhow::anyhow!("causal_chain dep query failed: {e}"))?;
                if visited.insert(decision.event_id.clone()) {
                    let relation = match dep_type.as_str() {
                        "explicit" => "depends_on".to_string(),
                        "auto_domain" => "domain_related".to_string(),
                        other => other.to_string(),
                    };
                    queue.push_back((
                        decision.key.clone(),
                        decision.event_id.clone(),
                        decision.supersedes_id.clone(),
                        next_depth,
                    ));
                    results.push(ChainEntry {
                        decision,
                        relation,
                        depth: next_depth,
                    });
                }
            }

            // 2) Superseded-by: decisions whose supersedes_id = current_event_id
            let mut sup_by_stmt = self.conn.prepare(
                "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                        d.supersedes_id, d.is_active, e.ts,
                        d.scope, d.source_project_id, d.source_event_id,
                        d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility
                 FROM decisions d
                 JOIN events e ON d.event_id = e.event_id
                 WHERE d.supersedes_id = ?1",
            )?;
            let sup_by_rows = sup_by_stmt.query_map(params![current_event_id], |row| {
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
                    scope: row.get(9)?,
                    source_project_id: row.get(10)?,
                    source_event_id: row.get(11)?,
                    status: row.get(12)?,
                    authority: row.get(13)?,
                    affected_paths: row.get(14)?,
                    tags: row.get(15)?,
                    review_after: row.get(16)?,
                    reversibility: row.get(17)?,
                })
            })?;
            for row in sup_by_rows {
                let decision = row
                    .map_err(|e| anyhow::anyhow!("causal_chain superseded_by query failed: {e}"))?;
                if visited.insert(decision.event_id.clone()) {
                    queue.push_back((
                        decision.key.clone(),
                        decision.event_id.clone(),
                        decision.supersedes_id.clone(),
                        next_depth,
                    ));
                    results.push(ChainEntry {
                        decision,
                        relation: "superseded_by".to_string(),
                        depth: next_depth,
                    });
                }
            }

            // 3) Supersedes (reverse): if current has supersedes_id, look up that decision
            if let Some(ref sup_id) = current_supersedes_id {
                if let Some(decision) = self.get_decision_by_event_id(sup_id)? {
                    if visited.insert(decision.event_id.clone()) {
                        queue.push_back((
                            decision.key.clone(),
                            decision.event_id.clone(),
                            decision.supersedes_id.clone(),
                            next_depth,
                        ));
                        results.push(ChainEntry {
                            decision,
                            relation: "supersedes".to_string(),
                            depth: next_depth,
                        });
                    }
                }
            }
        }

        Ok(Some((root, results)))
    }

    // ── Decision Outcomes ─────────────────────────────────────────────

    /// Get aggregated outcome metrics for a decision.
    ///
    /// Queries execution_events that have a `based_on` provenance link to the
    /// given decision event_id, then aggregates success rate, cost, and latency.
    pub fn decision_outcomes(
        &self,
        decision_event_id: &str,
    ) -> anyhow::Result<Option<OutcomeMetrics>> {
        let decision = self.get_event(decision_event_id)?;
        let decision = match decision {
            Some(d) => d,
            None => return Ok(None),
        };

        let dp = edda_core::decision::extract_decision(&decision.payload);
        let (decision_key, decision_value) = match dp {
            Some(d) => (d.key, d.value),
            None => return Ok(None),
        };

        let executions = self.executions_for_decision(decision_event_id)?;

        if executions.is_empty() {
            return Ok(Some(OutcomeMetrics {
                decision_event_id: decision_event_id.to_string(),
                decision_key: decision_key.clone(),
                decision_value: decision_value.clone(),
                decision_ts: decision.ts.clone(),
                total_executions: 0,
                success_count: 0,
                failed_count: 0,
                cancelled_count: 0,
                success_rate: 0.0,
                total_cost_usd: 0.0,
                total_tokens_in: 0,
                total_tokens_out: 0,
                avg_latency_ms: 0.0,
                first_execution_ts: None,
                last_execution_ts: None,
            }));
        }

        let total_executions = executions.len() as u64;
        let success_count = executions.iter().filter(|e| e.status == "success").count() as u64;
        let failed_count = executions.iter().filter(|e| e.status == "failed").count() as u64;
        let cancelled_count = executions
            .iter()
            .filter(|e| e.status == "cancelled")
            .count() as u64;

        let success_rate = if total_executions > 0 {
            (success_count as f64 / total_executions as f64) * 100.0
        } else {
            0.0
        };

        let total_cost_usd: f64 = executions.iter().filter_map(|e| e.cost_usd).sum();
        let total_tokens_in: u64 = executions.iter().filter_map(|e| e.token_in).sum();
        let total_tokens_out: u64 = executions.iter().filter_map(|e| e.token_out).sum();

        let latencies: Vec<u64> = executions.iter().filter_map(|e| e.latency_ms).collect();
        let avg_latency_ms = if !latencies.is_empty() {
            latencies.iter().sum::<u64>() as f64 / latencies.len() as f64
        } else {
            0.0
        };

        let first_execution_ts = executions
            .iter()
            .map(|e| e.ts.as_str())
            .min()
            .map(|s| s.to_string());
        let last_execution_ts = executions
            .iter()
            .map(|e| e.ts.as_str())
            .max()
            .map(|s| s.to_string());

        Ok(Some(OutcomeMetrics {
            decision_event_id: decision_event_id.to_string(),
            decision_key,
            decision_value,
            decision_ts: decision.ts,
            total_executions,
            success_count,
            failed_count,
            cancelled_count,
            success_rate,
            total_cost_usd,
            total_tokens_in,
            total_tokens_out,
            avg_latency_ms,
            first_execution_ts,
            last_execution_ts,
        }))
    }

    /// Get all execution events linked to a decision via `based_on` provenance.
    pub fn executions_for_decision(
        &self,
        decision_event_id: &str,
    ) -> anyhow::Result<Vec<ExecutionLinked>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, payload, refs_provenance FROM events
             WHERE event_type = 'execution_event'
             ORDER BY ts",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        let mut results = Vec::new();
        for row_result in rows {
            let (event_id, ts, payload_str, refs_prov_str) = row_result?;
            let provenance: Vec<Provenance> = match serde_json::from_str(&refs_prov_str) {
                Ok(p) => p,
                Err(_) => continue,
            };

            let has_link = provenance
                .iter()
                .any(|p| p.rel == "based_on" && p.target == decision_event_id);

            if !has_link {
                continue;
            }

            let payload: serde_json::Value = match serde_json::from_str(&payload_str) {
                Ok(p) => p,
                Err(_) => continue,
            };

            let status = payload["result"]["status"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            let runtime = payload["runtime"].as_str().map(|s| s.to_string());
            let model = payload["model"].as_str().map(|s| s.to_string());
            let cost_usd = payload["usage"]["cost_usd"].as_f64();
            let token_in = payload["usage"]["token_in"].as_u64();
            let token_out = payload["usage"]["token_out"].as_u64();
            let latency_ms = payload["usage"]["latency_ms"].as_u64();

            results.push(ExecutionLinked {
                event_id,
                ts,
                status,
                runtime,
                model,
                cost_usd,
                token_in,
                token_out,
                latency_ms,
            });
        }

        Ok(results)
    }

    /// Verify the hash chain integrity of all events in insertion order.
    ///
    /// Returns `Ok(())` if the chain is valid: the first event has
    /// `parent_hash == None`, and each subsequent event's `parent_hash`
    /// matches the previous event's `hash`.
    ///
    /// Returns `Err` describing the first break found.
    pub fn verify_chain(&self) -> anyhow::Result<()> {
        let events = self.iter_events()?;
        if events.is_empty() {
            return Ok(());
        }

        // First event must have no parent
        if events[0].parent_hash.is_some() {
            anyhow::bail!(
                "chain break at first event {}: expected parent_hash=None, got {:?}",
                events[0].event_id,
                events[0].parent_hash,
            );
        }

        for i in 1..events.len() {
            let expected = Some(events[i - 1].hash.as_str());
            let actual = events[i].parent_hash.as_deref();
            if actual != expected {
                anyhow::bail!(
                    "chain break at event {} (index {}): expected parent_hash={:?}, got {:?}",
                    events[i].event_id,
                    i,
                    expected,
                    actual,
                );
            }
        }

        Ok(())
    }

    // ── Task Briefs ─────────────────────────────────────────────────

    /// Get a task brief by task_id.
    pub fn get_task_brief(&self, task_id: &str) -> anyhow::Result<Option<TaskBriefRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, intake_event_id, title, intent, source_url,
                    status, branch, iterations, artifacts, decisions,
                    last_feedback, created_at, updated_at
             FROM task_briefs WHERE task_id = ?1",
        )?;
        let result = stmt.query_map(params![task_id], map_task_brief_row)?.next();
        match result {
            Some(Ok(row)) => Ok(Some(row)),
            Some(Err(e)) => Err(anyhow::anyhow!("task brief query failed: {e}")),
            None => Ok(None),
        }
    }

    /// List task briefs, optionally filtered by status and/or intent.
    pub fn list_task_briefs(
        &self,
        status: Option<&str>,
        intent: Option<&str>,
    ) -> anyhow::Result<Vec<TaskBriefRow>> {
        let base = "SELECT task_id, intake_event_id, title, intent, source_url,
                           status, branch, iterations, artifacts, decisions,
                           last_feedback, created_at, updated_at
                    FROM task_briefs";

        let mut conditions: Vec<&str> = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(s) = status {
            conditions.push("status = ?");
            param_values.push(Box::new(s.to_string()));
        }
        if let Some(i) = intent {
            conditions.push("intent = ?");
            param_values.push(Box::new(i.to_string()));
        }

        let sql = if conditions.is_empty() {
            format!("{base} ORDER BY updated_at DESC")
        } else {
            format!(
                "{base} WHERE {} ORDER BY updated_at DESC",
                conditions.join(" AND ")
            )
        };

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), map_task_brief_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("task brief list query failed: {e}"))
    }

    // ── Decide Snapshots ─────────────────────────────────────────────

    /// Insert a row into the `decide_snapshots` materialized view.
    pub fn insert_snapshot(&self, row: &DecideSnapshotRow) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO decide_snapshots
             (event_id, context_hash, engine_version, schema_version,
              redaction_level, village_id, cycle_id, has_blobs, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                row.event_id,
                row.context_hash,
                row.engine_version,
                row.schema_version,
                row.redaction_level,
                row.village_id,
                row.cycle_id,
                row.has_blobs,
                row.created_at,
            ],
        )?;
        Ok(())
    }

    /// Query snapshots with optional filtering by village_id and engine_version.
    pub fn query_snapshots(
        &self,
        village_id: Option<&str>,
        engine_version: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<DecideSnapshotRow>> {
        let base = "SELECT event_id, context_hash, engine_version, schema_version,
                           redaction_level, village_id, cycle_id, has_blobs, created_at
                    FROM decide_snapshots";

        let mut conditions: Vec<&str> = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(v) = village_id {
            conditions.push("village_id = ?");
            param_values.push(Box::new(v.to_string()));
        }
        if let Some(e) = engine_version {
            conditions.push("engine_version = ?");
            param_values.push(Box::new(e.to_string()));
        }

        let sql = if conditions.is_empty() {
            format!("{base} ORDER BY created_at DESC LIMIT ?")
        } else {
            format!(
                "{base} WHERE {} ORDER BY created_at DESC LIMIT ?",
                conditions.join(" AND ")
            )
        };
        param_values.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), map_snapshot_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("snapshot query failed: {e}"))
    }

    /// Find all snapshots with a given context_hash (for version comparison).
    pub fn snapshots_by_context_hash(
        &self,
        context_hash: &str,
    ) -> anyhow::Result<Vec<DecideSnapshotRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, context_hash, engine_version, schema_version,
                    redaction_level, village_id, cycle_id, has_blobs, created_at
             FROM decide_snapshots
             WHERE context_hash = ?1
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![context_hash], map_snapshot_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("snapshot context_hash query failed: {e}"))
    }
}

fn map_event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
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
}

fn map_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DecideSnapshotRow> {
    Ok(DecideSnapshotRow {
        event_id: row.get(0)?,
        context_hash: row.get(1)?,
        engine_version: row.get(2)?,
        schema_version: row.get(3)?,
        redaction_level: row.get(4)?,
        village_id: row.get(5)?,
        cycle_id: row.get(6)?,
        has_blobs: row.get(7)?,
        created_at: row.get(8)?,
    })
}

impl Drop for SqliteStore {
    fn drop(&mut self) {
        // Merge WAL back into main DB so users see a single file when idle.
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}

// ── Decision helpers ────────────────────────────────────────────────
// Centralized in edda_core::decision — detection, extraction, domain parsing.

/// Shared materialization logic for review bundles.
/// Accepts `&Connection` — works with both `Connection` and `Transaction` (via deref coercion).
fn materialize_bundle_sql(
    conn: &Connection,
    event_id: &str,
    ts: &str,
    branch: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let bundle_id = payload["bundle_id"].as_str().unwrap_or("");
    let risk_level = payload["risk_assessment"]["level"]
        .as_str()
        .unwrap_or("low");
    let total_added = payload["change_summary"]["total_added"]
        .as_i64()
        .unwrap_or(0);
    let total_deleted = payload["change_summary"]["total_deleted"]
        .as_i64()
        .unwrap_or(0);
    let files_changed = payload["change_summary"]["files"]
        .as_array()
        .map(|a| a.len() as i64)
        .unwrap_or(0);
    let tests_passed = payload["test_results"]["passed"].as_i64().unwrap_or(0);
    let tests_failed = payload["test_results"]["failed"].as_i64().unwrap_or(0);
    let suggested_action = payload["suggested_action"].as_str().unwrap_or("review");

    conn.execute(
        "INSERT OR IGNORE INTO review_bundles
         (event_id, bundle_id, status, risk_level, total_added, total_deleted,
          files_changed, tests_passed, tests_failed, suggested_action, branch, created_at)
         VALUES (?1, ?2, 'pending', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            event_id,
            bundle_id,
            risk_level,
            total_added,
            total_deleted,
            files_changed,
            tests_passed,
            tests_failed,
            suggested_action,
            branch,
            ts,
        ],
    )?;

    Ok(())
}

fn map_bundle_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BundleRow> {
    Ok(BundleRow {
        event_id: row.get(0)?,
        bundle_id: row.get(1)?,
        status: row.get(2)?,
        risk_level: row.get(3)?,
        total_added: row.get(4)?,
        total_deleted: row.get(5)?,
        files_changed: row.get(6)?,
        tests_passed: row.get(7)?,
        tests_failed: row.get(8)?,
        suggested_action: row.get(9)?,
        branch: row.get(10)?,
        created_at: row.get(11)?,
    })
}

fn map_dep_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DepRow> {
    Ok(DepRow {
        source_key: row.get(0)?,
        target_key: row.get(1)?,
        dep_type: row.get(2)?,
        created_event: row.get(3)?,
        created_at: row.get(4)?,
    })
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
        scope: row.get(9)?,
        source_project_id: row.get(10)?,
        source_event_id: row.get(11)?,
        status: row.get(12)?,
        authority: row.get(13)?,
        affected_paths: row.get(14)?,
        tags: row.get(15)?,
        review_after: row.get(16)?,
        reversibility: row.get(17)?,
    })
}

fn map_task_brief_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskBriefRow> {
    let intent_str: String = row.get(3)?;
    let status_str: String = row.get(5)?;
    let intent = intent_str
        .parse::<edda_core::types::TaskBriefIntent>()
        .unwrap_or(edda_core::types::TaskBriefIntent::Implement);
    let status = status_str
        .parse::<edda_core::types::TaskBriefStatus>()
        .unwrap_or(edda_core::types::TaskBriefStatus::Active);
    Ok(TaskBriefRow {
        task_id: row.get(0)?,
        intake_event_id: row.get(1)?,
        title: row.get(2)?,
        intent,
        source_url: row.get(4)?,
        status,
        branch: row.get(6)?,
        iterations: row.get(7)?,
        artifacts: row.get(8)?,
        decisions: row.get(9)?,
        last_feedback: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

/// Materialize a task_intake event into the task_briefs table.
fn materialize_task_brief_sql(
    conn: &Connection,
    event_id: &str,
    ts: &str,
    branch: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let source = payload["source"].as_str().unwrap_or("unknown");
    let source_id = payload["source_id"].as_str().unwrap_or("");
    let task_id = format!("{source}#{source_id}");
    let title = payload["title"].as_str().unwrap_or("");
    let intent_str = payload["intent"].as_str().unwrap_or("implement");
    // Validate intent; fall back to "implement" if unrecognised
    let intent = intent_str
        .parse::<edda_core::types::TaskBriefIntent>()
        .unwrap_or(edda_core::types::TaskBriefIntent::Implement);
    let source_url = payload["source_url"].as_str().unwrap_or("");

    conn.execute(
        "INSERT OR IGNORE INTO task_briefs
         (task_id, intake_event_id, title, intent, source_url, status,
          branch, iterations, artifacts, decisions, last_feedback,
          created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, 0, '[]', '[]', NULL, ?7, ?7)",
        params![
            task_id,
            event_id,
            title,
            intent.as_str(),
            source_url,
            branch,
            ts
        ],
    )?;
    Ok(())
}

/// Update task brief when a commit event occurs on the same branch.
fn update_task_brief_on_commit(conn: &Connection, event: &Event) -> anyhow::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT task_id, artifacts FROM task_briefs
         WHERE branch = ?1 AND status = ?2",
    )?;
    let briefs: Vec<(String, String)> = stmt
        .query_map(
            params![
                event.branch,
                edda_core::types::TaskBriefStatus::Active.as_str()
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;

    for (task_id, artifacts_str) in &briefs {
        let mut artifacts: Vec<String> = serde_json::from_str(artifacts_str).unwrap_or_default();
        extract_artifacts_from_payload(&event.payload, &mut artifacts);
        let artifacts_json = serde_json::to_string(&artifacts).unwrap_or_else(|_| "[]".to_string());

        conn.execute(
            "UPDATE task_briefs SET
                iterations = iterations + 1,
                artifacts = ?1,
                updated_at = ?2
             WHERE task_id = ?3",
            params![artifacts_json, event.ts, task_id],
        )?;
    }
    Ok(())
}

/// Update task brief when a note with review/feedback tag occurs.
fn update_task_brief_on_note(conn: &Connection, event: &Event) -> anyhow::Result<()> {
    let tags = event.payload["tags"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    let has_feedback_tag = tags.contains(&"review") || tags.contains(&"feedback");
    let has_decision_tag = tags.contains(&"decision");

    if !has_feedback_tag && !has_decision_tag {
        return Ok(());
    }

    let mut stmt = conn.prepare(
        "SELECT task_id, decisions FROM task_briefs
         WHERE branch = ?1 AND status = ?2",
    )?;
    let briefs: Vec<(String, String)> = stmt
        .query_map(
            params![
                event.branch,
                edda_core::types::TaskBriefStatus::Active.as_str()
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;

    for (task_id, decisions_str) in &briefs {
        if has_feedback_tag {
            let feedback = extract_feedback_from_payload(&event.payload);
            if let Some(fb) = &feedback {
                conn.execute(
                    "UPDATE task_briefs SET last_feedback = ?1, updated_at = ?2
                     WHERE task_id = ?3",
                    params![fb, event.ts, task_id],
                )?;
            }
        }

        if has_decision_tag {
            if let Some(key) = event.payload["decision"]["key"].as_str() {
                let mut decisions: Vec<String> =
                    serde_json::from_str(decisions_str).unwrap_or_default();
                if !decisions.contains(&key.to_string()) {
                    decisions.push(key.to_string());
                    let decisions_json =
                        serde_json::to_string(&decisions).unwrap_or_else(|_| "[]".to_string());
                    conn.execute(
                        "UPDATE task_briefs SET decisions = ?1, updated_at = ?2
                         WHERE task_id = ?3",
                        params![decisions_json, event.ts, task_id],
                    )?;
                }
            }
        }
    }

    Ok(())
}

/// Update task brief when a merge event occurs (mark completed).
fn update_task_brief_on_merge(conn: &Connection, event: &Event) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE task_briefs SET status = ?1, updated_at = ?2
         WHERE branch = ?3 AND status = ?4",
        params![
            edda_core::types::TaskBriefStatus::Completed.as_str(),
            event.ts,
            event.branch,
            edda_core::types::TaskBriefStatus::Active.as_str(),
        ],
    )?;
    Ok(())
}

/// Extract file paths from a commit payload into the artifacts list.
fn extract_artifacts_from_payload(payload: &serde_json::Value, artifacts: &mut Vec<String>) {
    if let Some(files) = payload["files"].as_array() {
        for f in files {
            if let Some(path) = f.as_str() {
                if !artifacts.contains(&path.to_string()) {
                    artifacts.push(path.to_string());
                }
            }
            if let Some(path) = f["path"].as_str() {
                if !artifacts.contains(&path.to_string()) {
                    artifacts.push(path.to_string());
                }
            }
        }
    }
}

/// Extract the `tags` array from a JSON payload, returning an empty vec on any error.
fn payload_tags(payload: &serde_json::Value) -> Vec<String> {
    payload["tags"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Extract feedback text from a note payload.
fn extract_feedback_from_payload(payload: &serde_json::Value) -> Option<String> {
    if let Some(msg) = payload["message"].as_str() {
        return Some(msg.to_string());
    }
    if let Some(text) = payload["text"].as_str() {
        return Some(text.to_string());
    }
    None
}

// ── Internal helpers ────────────────────────────────────────────────

fn time_now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

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
        assert_eq!(store.schema_version().unwrap(), 10);
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

        // Version should be 10
        assert_eq!(store.schema_version().unwrap(), 10);

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

        // Phase 2: Reopen — should auto-migrate to V10
        let store = SqliteStore::open_or_create(&db_path).unwrap();
        assert_eq!(store.schema_version().unwrap(), 10);

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
}
