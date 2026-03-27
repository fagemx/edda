//! Decision queries: active decisions, timelines, domains, cross-project sync.

use rusqlite::params;
use std::time::Instant;
use tracing::debug;

use super::mappers::*;
use super::types::*;
use super::SqliteStore;

impl SqliteStore {
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
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility, d.village_id
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

    /// Return active/experimental decisions where `affected_paths` is non-empty.
    /// This pre-filters at the SQL level so glob matching runs on a small set.
    pub fn active_decisions_with_paths(
        &self,
        branch: Option<&str>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<DecisionRow>> {
        let mut sql = String::from(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags,
                    d.review_after, d.reversibility, d.village_id
             FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.is_active = TRUE
               AND d.affected_paths IS NOT NULL
               AND d.affected_paths != '[]'",
        );

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(b) = branch {
            sql.push_str(&format!(" AND d.branch = ?{idx}"));
            param_values.push(Box::new(b.to_string()));
            idx += 1;
        }

        sql.push_str(" ORDER BY e.ts DESC");

        if let Some(lim) = limit {
            sql.push_str(&format!(" LIMIT ?{idx}"));
            param_values.push(Box::new(lim as i64));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), map_decision_row)?;

        let result = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("active_decisions_with_paths query failed: {e}"))?;

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
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility, d.village_id
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
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility, d.village_id
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
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility, d.village_id
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

    /// Look up a single decision by its event_id.
    pub fn get_decision_by_event_id(&self, event_id: &str) -> anyhow::Result<Option<DecisionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility, d.village_id
             FROM decisions d
             JOIN events e ON d.event_id = e.event_id
             WHERE d.event_id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![event_id], map_decision_row)?;
        match rows.next() {
            Some(Ok(row)) => Ok(Some(row)),
            Some(Err(e)) => Err(anyhow::anyhow!("get_decision_by_event_id failed: {e}")),
            None => Ok(None),
        }
    }

    // ── Cross-Project Sync ─────────────────────────────────────────────

    /// Query active decisions with shared or global scope.
    /// Used by the sync engine to find decisions that should be shared.
    pub fn shared_decisions(&self) -> anyhow::Result<Vec<DecisionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility, d.village_id
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
}
