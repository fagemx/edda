//! Event persistence: append, iterate, get, find, refs, chain verification.

use edda_core::types::Event;
use rusqlite::{params, OptionalExtension};

use super::mappers::*;
use super::status_to_is_active;
use super::SqliteStore;

impl SqliteStore {
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
                let village_id = dp.village_id.as_deref();

                tx.execute(
                    "INSERT INTO decisions
                     (event_id, key, value, reason, domain, branch, supersedes_id,
                      is_active, scope, status, authority, affected_paths, tags,
                      review_after, reversibility, village_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                             ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
                        village_id,
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
}
