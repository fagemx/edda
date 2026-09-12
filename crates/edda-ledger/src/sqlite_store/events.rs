//! Event persistence: append, iterate, get, find, refs, chain verification.

use crate::guided_execution::execution_brief_data;
use crate::task_actions::CONTROLLED_TASK_LEASE_PREFIX;
use crate::tasks::{self, TaskStatus};
use edda_core::event::finalize_event;
use edda_core::guided_execution::{parse_work_receipt, ReceiptExpectationV1, ResultClassV1};
use edda_core::types::Event;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Deserialize;

use super::mappers::*;
use super::status_to_is_active;
use super::SqliteStore;

pub(super) fn validate_event_hash(event: &Event) -> anyhow::Result<()> {
    let mut canonical = event.clone();
    finalize_event(&mut canonical)?;
    if event.event_family != canonical.event_family || event.event_level != canonical.event_level {
        anyhow::bail!("event {} has invalid taxonomy", event.event_id);
    }
    if event.hash != canonical.hash || event.digests != canonical.digests {
        anyhow::bail!("event {} has invalid hash or digest", event.event_id);
    }
    Ok(())
}

pub(super) fn validate_event_for_append(conn: &Connection, event: &Event) -> anyhow::Result<()> {
    let event_value = serde_json::to_value(event)?;
    edda_core::event::validate_readable_payload(&event_value)?;

    let current_tail: Option<String> = conn
        .query_row(
            "SELECT hash FROM events ORDER BY rowid DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if event.parent_hash != current_tail {
        anyhow::bail!(
            "event {} has stale parent_hash: expected {:?}, got {:?}",
            event.event_id,
            current_tail,
            event.parent_hash,
        );
    }

    validate_event_hash(event)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlledCompletion {
    attempt: u32,
    lease_owner: String,
    session_id: String,
    agent_kind: String,
    brief_event_id: String,
    brief_digest: String,
    outcome_code: String,
}

fn task_events_on(conn: &Connection) -> anyhow::Result<Vec<Event>> {
    let mut stmt = conn.prepare(
        "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                payload, refs_blobs, refs_events, refs_provenance,
                schema_version, digests, event_family, event_level
         FROM events WHERE event_type LIKE 'task.%' ORDER BY rowid",
    )?;
    let rows = stmt
        .query_map([], map_event_row)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(row_to_event).collect()
}

fn event_on(conn: &Connection, event_id: &str) -> anyhow::Result<Option<Event>> {
    let row = conn
        .query_row(
            "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
             FROM events WHERE event_id = ?1",
            params![event_id],
            map_event_row,
        )
        .optional()?;
    row.map(row_to_event).transpose()
}

fn consume_controlled_lease(
    tx: &Transaction<'_>,
    task_id: u64,
    attempt: u32,
    lease_owner: &str,
) -> anyhow::Result<()> {
    let deleted = tx.execute(
        "DELETE FROM task_leases
         WHERE task_id = ?1 AND attempt = ?2 AND owner = ?3
           AND julianday(expires_at) > julianday('now')",
        params![task_id, attempt, lease_owner],
    )?;
    anyhow::ensure!(
        deleted == 1,
        "controlled task.done lost its exact unexpired lease; completion rolled back"
    );
    Ok(())
}

fn validate_controlled_task_done(
    tx: &Transaction<'_>,
    event: &Event,
    test_authority_sealed: bool,
) -> anyhow::Result<()> {
    let task_id = event
        .payload
        .get("task_id")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("task.done omits task_id"))?;
    let views = tasks::project_tasks(&task_events_on(tx)?);
    let Some(task) = views.iter().find(|view| view.task_id == task_id) else {
        anyhow::ensure!(
            event.payload.get("controlled_completion").is_none(),
            "controlled task.done metadata requires a known controlled task"
        );
        return Ok(());
    };
    let lease: Option<crate::TaskLease> = tx
        .query_row(
            "SELECT task_id, attempt, owner, expires_at, heartbeat_at
             FROM task_leases WHERE task_id = ?1",
            params![task_id],
            |row| {
                Ok(crate::TaskLease {
                    task_id: row.get(0)?,
                    attempt: row.get(1)?,
                    owner: row.get(2)?,
                    expires_at: row.get(3)?,
                    heartbeat_at: row.get(4)?,
                })
            },
        )
        .optional()?;
    let controlled_state = task.session_brief_event_id.is_some()
        || task.session_brief_digest.is_some()
        || task.session_lease_owner.is_some()
        || lease
            .as_ref()
            .is_some_and(|value| value.owner.starts_with(CONTROLLED_TASK_LEASE_PREFIX));
    let supplied = event.payload.get("controlled_completion");
    if !controlled_state {
        anyhow::ensure!(
            supplied.is_none(),
            "controlled task.done metadata requires a current controlled task state"
        );
        return Ok(());
    }

    anyhow::ensure!(
        task.status == TaskStatus::Running,
        "controlled task is not running; stale completion refused"
    );
    let controlled: ControlledCompletion = serde_json::from_value(
        supplied
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("controlled task.done requires completion metadata"))?,
    )
    .map_err(|_| anyhow::anyhow!("controlled task.done has invalid completion metadata"))?;
    let (brief_event_id, brief_digest, session_id, agent_kind, lease_owner, attempt) = match (
        task.session_brief_event_id.as_deref(),
        task.session_brief_digest.as_deref(),
        task.session_id.as_deref(),
        task.session_agent_kind.as_deref(),
        task.session_lease_owner.as_deref(),
        task.session_attempt,
    ) {
        (Some(event_id), Some(digest), Some(session), Some(agent), Some(owner), Some(attempt)) => {
            (event_id, digest, session, agent, owner, attempt)
        }
        _ => anyhow::bail!("controlled task has an incomplete session authority binding"),
    };
    anyhow::ensure!(
        attempt == task.attempts
            && controlled.attempt == attempt
            && controlled.lease_owner == lease_owner
            && controlled.session_id == session_id
            && controlled.agent_kind == agent_kind
            && controlled.brief_event_id == brief_event_id
            && controlled.brief_digest == brief_digest,
        "controlled task.done correlation does not match current task state"
    );
    let lease = lease.ok_or_else(|| anyhow::anyhow!("controlled task lease is missing"))?;
    anyhow::ensure!(
        lease.attempt == attempt && lease.owner == lease_owner,
        "controlled task lease changed before completion"
    );
    let expires = time::OffsetDateTime::parse(
        &lease.expires_at,
        &time::format_description::well_known::Rfc3339,
    )?;
    anyhow::ensure!(
        expires > time::OffsetDateTime::now_utc(),
        "controlled task lease expired before completion"
    );
    let prefix = format!("{CONTROLLED_TASK_LEASE_PREFIX}{brief_event_id}:{brief_digest}:");
    anyhow::ensure!(
        lease_owner.starts_with(&prefix),
        "controlled task lease is bound to a different execution brief"
    );
    let brief_event = event_on(tx, brief_event_id)?
        .ok_or_else(|| anyhow::anyhow!("controlled execution brief event is missing"))?;
    let accepted_data = execution_brief_data(&brief_event, brief_digest)?;
    anyhow::ensure!(
        accepted_data.brief.task_ref == Some(task_id),
        "execution brief does not identify the completed task"
    );
    let task_scope: std::collections::BTreeSet<_> = task.scope_paths.iter().collect();
    let brief_scope: std::collections::BTreeSet<_> =
        accepted_data.brief.scope.allowed_paths.iter().collect();
    anyhow::ensure!(
        task_scope == brief_scope,
        "execution brief scope does not match the completed task"
    );
    let receipt_text = event
        .payload
        .get("receipt")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("controlled task.done omits WorkReceiptV1"))?;
    let receipt = parse_work_receipt(
        receipt_text.as_bytes(),
        &accepted_data.brief,
        ReceiptExpectationV1 {
            task_id: Some(task_id),
            attempt: Some(attempt),
            lease_owner: Some(lease_owner),
            agent_kind: Some(agent_kind),
            session_id: Some(session_id),
            ..ReceiptExpectationV1::default()
        },
    )?;
    anyhow::ensure!(
        receipt.result_class == ResultClassV1::Success
            && receipt.outcome_code == controlled.outcome_code,
        "controlled task.done requires its correlated success outcome"
    );
    anyhow::ensure!(
        event
            .payload
            .get("evidence_paths")
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty),
        "controlled task.done evidence must come from WorkReceiptV1"
    );
    anyhow::ensure!(
        test_authority_sealed,
        "controlled task.done requires an S6 product-verifiable execution authority seal"
    );

    consume_controlled_lease(tx, task_id, attempt, lease_owner)
}

fn materialize_snapshot(conn: &Connection, event: &Event) -> anyhow::Result<()> {
    let context_hash = event.payload["context_hash"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("snapshot event is missing context_hash"))?;
    let engine_version = event.payload["engine_version"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("snapshot event is missing engine_version"))?;
    let schema_version = event.payload["schema_version"]
        .as_str()
        .unwrap_or("snapshot.v1");
    let redaction_level = event.payload["redaction_level"].as_str().unwrap_or("full");
    let village_id = event.payload["village_id"].as_str();
    let cycle_id = event.payload["cycle_id"].as_str();
    let has_blobs =
        event.payload.get("context_blob").is_some() || event.payload.get("result_blob").is_some();

    conn.execute(
        "INSERT INTO decide_snapshots
         (event_id, context_hash, engine_version, schema_version,
          redaction_level, village_id, cycle_id, has_blobs, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            event.event_id,
            context_hash,
            engine_version,
            schema_version,
            redaction_level,
            village_id,
            cycle_id,
            has_blobs,
            event.ts,
        ],
    )?;
    Ok(())
}

impl SqliteStore {
    /// Append an event. Append-only (CONTRACT LEDGER-02).
    ///
    /// If the event is a decision (note with `"decision"` tag), the `decisions`
    /// table is also updated atomically within the same transaction.
    pub fn append_event(&self, event: &Event) -> anyhow::Result<()> {
        self.append_event_inner(event, false)
    }

    /// Test-only stand-in for the future S6 authority verifier. This method is
    /// not compiled into production artifacts and cannot be reached through
    /// [`crate::Ledger::append_event`].
    #[cfg(test)]
    pub(crate) fn append_event_with_test_execution_authority(
        &self,
        event: &Event,
    ) -> anyhow::Result<()> {
        self.append_event_inner(event, true)
    }

    #[allow(clippy::too_many_lines)] // append and materialization are one transaction
    fn append_event_inner(&self, event: &Event, test_authority_sealed: bool) -> anyhow::Result<()> {
        let payload = serde_json::to_string(&event.payload)?;
        let refs_blobs = serde_json::to_string(&event.refs.blobs)?;
        let refs_events = serde_json::to_string(&event.refs.events)?;
        let refs_provenance = serde_json::to_string(&event.refs.provenance)?;
        let digests = serde_json::to_string(&event.digests)?;

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        validate_event_for_append(&tx, event)?;

        // Classification comes from current controlled state, never from the
        // caller-selected constructor/path. Full receipt correlation and exact
        // lease consumption share this BEGIN IMMEDIATE transaction.
        if event.event_type == "task.done" {
            validate_controlled_task_done(&tx, event, test_authority_sealed)?;
        }

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
                // GH-401: absence of provenance is not operator authority. A
                // decision written without an explicit authority (pre-401
                // events, or any write path that omits it) projects as
                // "unknown", never "human" — the projection must not mint
                // operator authorship. Explicit tags (agent/system/operator)
                // pass through unchanged.
                let authority = dp
                    .authority
                    .as_deref()
                    .unwrap_or(edda_core::types::authority::UNKNOWN);
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

        if event.event_type == "decide_snapshot" {
            materialize_snapshot(&tx, event)?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Append an event idempotently. Returns `true` if inserted, `false` if duplicate.
    ///
    /// Duplicate `event_id` values are skipped without returning an error. New
    /// events still pass the same tail and canonical-hash validation as normal
    /// appends.
    pub fn append_event_idempotent(&self, event: &Event) -> anyhow::Result<bool> {
        let payload = serde_json::to_string(&event.payload)?;
        let refs_blobs = serde_json::to_string(&event.refs.blobs)?;
        let refs_events = serde_json::to_string(&event.refs.events)?;
        let refs_provenance = serde_json::to_string(&event.refs.provenance)?;
        let digests = serde_json::to_string(&event.digests)?;

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let exists = tx
            .query_row(
                "SELECT 1 FROM events WHERE event_id = ?1",
                params![event.event_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            return Ok(false);
        }

        validate_event_for_append(&tx, event)?;
        if event.event_type == "task.done" {
            // Idempotent append has no authority-bearing path. A new
            // controlled completion therefore fails closed after the same
            // current-state and full-receipt validation.
            validate_controlled_task_done(&tx, event, false)?;
        }

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
        if event.event_type == "decide_snapshot" {
            materialize_snapshot(&tx, event)?;
        }
        tx.commit()?;

        Ok(true)
    }

    /// Return the total count of events in the events table.
    pub fn count_events(&self) -> anyhow::Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        Ok(count as u64)
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

    /// Read all events in insertion order, keeping per-row deserialization
    /// failures attached to the failing row's `event_id`.
    ///
    /// [`Self::iter_events`] aborts the whole iteration when any row cannot
    /// be deserialized, so a single tampered payload hides *which* event is
    /// broken. Verification (`edda verify`, GH-647) needs the failing row's
    /// identity, so this variant returns `Err((event_id, reason))` for
    /// unreadable rows instead of failing the whole scan.
    pub fn iter_events_reporting(&self) -> anyhow::Result<Vec<Result<Event, (String, String)>>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
             FROM events ORDER BY rowid",
        )?;

        let rows = stmt
            .query_map([], map_event_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let event_id = row.event_id.clone();
                row_to_event(row).map_err(|err| (event_id, format!("{err:#}")))
            })
            .collect())
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

    /// Read all `task.*` events in insertion order — the task rail's fold input.
    /// The LIKE dot is literal (`_` is the LIKE single-char wildcard), so
    /// legacy `task_intake` events are not matched.
    pub fn iter_task_events(&self) -> anyhow::Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
             FROM events WHERE event_type LIKE 'task.%' ORDER BY rowid",
        )?;

        let events = stmt
            .query_map([], map_event_row)?
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

        for event in &events {
            validate_event_hash(event)?;
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
