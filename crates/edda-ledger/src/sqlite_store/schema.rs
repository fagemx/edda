//! Schema constants, migrations, and version management.

use rusqlite::{params, OptionalExtension};
use tracing::warn;

use super::mappers::*;
use super::SqliteStore;

pub(super) const SCHEMA_SQL: &str = "
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

pub(super) const SCHEMA_V2_SQL: &str = "
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

pub(super) const SCHEMA_V4_SQL: &str = "
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

pub(super) const SCHEMA_V6_SQL: &str = "
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

pub(super) const SCHEMA_V7_SQL: &str = "
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

pub(super) const SCHEMA_V8_SQL: &str = "
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

pub(super) const SCHEMA_V3_SQL: &str = "
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

pub(super) const SCHEMA_V5_SQL: &str = "
ALTER TABLE decisions ADD COLUMN scope TEXT NOT NULL DEFAULT 'local';
ALTER TABLE decisions ADD COLUMN source_project_id TEXT;
ALTER TABLE decisions ADD COLUMN source_event_id TEXT;
CREATE INDEX IF NOT EXISTS idx_decisions_scope ON decisions(scope) WHERE scope != 'local';
CREATE INDEX IF NOT EXISTS idx_decisions_source ON decisions(source_project_id) WHERE source_project_id IS NOT NULL;
";

pub(super) const SCHEMA_V9_SQL: &str = "
CREATE INDEX IF NOT EXISTS idx_decisions_active_domain_branch
    ON decisions(is_active, domain, branch) WHERE is_active = TRUE;
CREATE INDEX IF NOT EXISTS idx_decisions_active_domain
    ON decisions(is_active, domain) WHERE is_active = TRUE;
";

pub(super) const SCHEMA_V10_SQL: &str = "
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

pub(super) const SCHEMA_V11_SQL: &str = "
ALTER TABLE decisions ADD COLUMN village_id TEXT;
CREATE INDEX IF NOT EXISTS idx_decisions_village
    ON decisions(village_id) WHERE village_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_village_status
    ON decisions(village_id, status) WHERE village_id IS NOT NULL;
";

pub(super) const SCHEMA_V12_SQL: &str = "
CREATE TABLE IF NOT EXISTS suggestions (
    id                TEXT PRIMARY KEY,
    event_type        TEXT NOT NULL,
    source_layer      TEXT NOT NULL,
    source_refs       TEXT NOT NULL DEFAULT '[]',
    summary           TEXT NOT NULL,
    suggested_because TEXT NOT NULL,
    detail            TEXT NOT NULL DEFAULT '{}',
    tags              TEXT NOT NULL DEFAULT '[]',
    status            TEXT NOT NULL DEFAULT 'pending',
    created_at        TEXT NOT NULL,
    reviewed_at       TEXT
);
CREATE INDEX IF NOT EXISTS idx_suggestions_status ON suggestions(status);
";

impl SqliteStore {
    pub(super) fn apply_schema(&self) -> anyhow::Result<()> {
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

        // Migrate to v11 if needed (village_id on decisions)
        let current = self.schema_version()?;
        if current < 11 {
            self.migrate_v10_to_v11()?;
        }

        // Migrate to v12 if needed (suggestions table for ingestion queue)
        let current = self.schema_version()?;
        if current < 12 {
            self.migrate_v11_to_v12()?;
        }

        // Post-migration verification: repair any columns that migrations
        // failed to add (e.g. version was bumped but ALTER TABLE didn't stick).
        self.verify_decisions_schema()?;

        Ok(())
    }

    pub(super) fn schema_version(&self) -> anyhow::Result<u32> {
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

    pub(super) fn set_schema_version(&self, version: u32) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('version', ?1)",
            params![version.to_string()],
        )?;
        Ok(())
    }

    /// Verify that the `decisions` table has all expected columns.
    ///
    /// If a migration partially failed (version bumped but ALTER TABLE didn't
    /// stick), this repairs the schema by re-adding missing columns with their
    /// correct defaults. Each ALTER TABLE ADD COLUMN is individually wrapped so
    /// that already-existing columns are silently skipped.
    pub(super) fn verify_decisions_schema(&self) -> anyhow::Result<()> {
        // Only run if the decisions table exists (schema >= v2).
        let has_decisions: bool = self
            .conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='decisions'")
            .and_then(|mut s| s.exists([]))
            .unwrap_or(false);
        if !has_decisions {
            return Ok(());
        }

        // Collect actual column names from PRAGMA table_info.
        let mut stmt = self.conn.prepare("PRAGMA table_info(decisions)")?;
        let actual_columns: std::collections::HashSet<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();

        // Expected columns and their ALTER TABLE definitions.
        // Base V2 columns (event_id, key, value, reason, domain, branch,
        // supersedes_id, is_active) are created via CREATE TABLE so they
        // should always be present. We verify the columns added by later
        // migrations (V5, V10).
        let expected_alters: &[(&str, &str)] = &[
            // V5 columns
            (
                "scope",
                "ALTER TABLE decisions ADD COLUMN scope TEXT NOT NULL DEFAULT 'local'",
            ),
            (
                "source_project_id",
                "ALTER TABLE decisions ADD COLUMN source_project_id TEXT",
            ),
            (
                "source_event_id",
                "ALTER TABLE decisions ADD COLUMN source_event_id TEXT",
            ),
            // V10 columns
            (
                "status",
                "ALTER TABLE decisions ADD COLUMN status TEXT NOT NULL DEFAULT 'active'",
            ),
            (
                "authority",
                "ALTER TABLE decisions ADD COLUMN authority TEXT NOT NULL DEFAULT 'human'",
            ),
            (
                "affected_paths",
                "ALTER TABLE decisions ADD COLUMN affected_paths TEXT NOT NULL DEFAULT '[]'",
            ),
            (
                "tags",
                "ALTER TABLE decisions ADD COLUMN tags TEXT NOT NULL DEFAULT '[]'",
            ),
            (
                "review_after",
                "ALTER TABLE decisions ADD COLUMN review_after TEXT",
            ),
            (
                "reversibility",
                "ALTER TABLE decisions ADD COLUMN reversibility TEXT NOT NULL DEFAULT 'medium'",
            ),
            // V11 column
            (
                "village_id",
                "ALTER TABLE decisions ADD COLUMN village_id TEXT",
            ),
        ];

        for (col_name, alter_sql) in expected_alters {
            if !actual_columns.contains(*col_name) {
                warn!(
                    column = col_name,
                    "decisions table missing column — repairing"
                );
                // SQLite ALTER TABLE ADD COLUMN will error if the column
                // already exists, but we checked above so this should succeed.
                // Wrap in a match so one failure doesn't abort the rest.
                if let Err(e) = self.conn.execute_batch(alter_sql) {
                    warn!(
                        column = col_name,
                        error = %e,
                        "failed to repair column — may already exist"
                    );
                }
            }
        }

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

            let provenance: Vec<edda_core::types::Provenance> =
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

    fn migrate_v10_to_v11(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA_V11_SQL)?;
        self.set_schema_version(11)?;
        Ok(())
    }

    fn migrate_v11_to_v12(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA_V12_SQL)?;
        // No backfill needed — suggestions is a new table with no existing data.
        self.set_schema_version(12)?;
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
}
