//! CRUD operations for device tokens, suggestions, bundles, task briefs, and snapshots.

use rusqlite::{params, OptionalExtension};

use super::mappers::*;
use super::types::*;
use super::SqliteStore;

impl SqliteStore {
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

    // ── Suggestions ──────────────────────────────────────────────────

    /// Insert a new suggestion row.
    pub fn insert_suggestion(&self, row: &SuggestionRow) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO suggestions
             (id, event_type, source_layer, source_refs, summary,
              suggested_because, detail, tags, status, created_at, reviewed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                row.id,
                row.event_type,
                row.source_layer,
                row.source_refs,
                row.summary,
                row.suggested_because,
                row.detail,
                row.tags,
                row.status,
                row.created_at,
                row.reviewed_at,
            ],
        )?;
        Ok(())
    }

    /// List suggestions filtered by status.
    pub fn list_suggestions_by_status(&self, status: &str) -> anyhow::Result<Vec<SuggestionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, event_type, source_layer, source_refs, summary,
                    suggested_because, detail, tags, status, created_at, reviewed_at
             FROM suggestions
             WHERE status = ?1
             ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map(params![status], |row| {
                Ok(SuggestionRow {
                    id: row.get(0)?,
                    event_type: row.get(1)?,
                    source_layer: row.get(2)?,
                    source_refs: row.get(3)?,
                    summary: row.get(4)?,
                    suggested_because: row.get(5)?,
                    detail: row.get(6)?,
                    tags: row.get(7)?,
                    status: row.get(8)?,
                    created_at: row.get(9)?,
                    reviewed_at: row.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Get a single suggestion by id.
    pub fn get_suggestion(&self, id: &str) -> anyhow::Result<Option<SuggestionRow>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, event_type, source_layer, source_refs, summary,
                        suggested_because, detail, tags, status, created_at, reviewed_at
                 FROM suggestions
                 WHERE id = ?1",
                params![id],
                |row| {
                    Ok(SuggestionRow {
                        id: row.get(0)?,
                        event_type: row.get(1)?,
                        source_layer: row.get(2)?,
                        source_refs: row.get(3)?,
                        summary: row.get(4)?,
                        suggested_because: row.get(5)?,
                        detail: row.get(6)?,
                        tags: row.get(7)?,
                        status: row.get(8)?,
                        created_at: row.get(9)?,
                        reviewed_at: row.get(10)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Update a suggestion's status and reviewed_at timestamp.
    /// Returns true if a row was updated.
    pub fn update_suggestion_status(
        &self,
        id: &str,
        status: &str,
        reviewed_at: &str,
    ) -> anyhow::Result<bool> {
        let count = self.conn.execute(
            "UPDATE suggestions SET status = ?1, reviewed_at = ?2 WHERE id = ?3",
            params![status, reviewed_at, id],
        )?;
        Ok(count > 0)
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
