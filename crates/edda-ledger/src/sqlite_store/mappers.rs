//! Row mapping functions, materialization helpers, and internal types.

use edda_core::types::{Digest, Event, Provenance, Refs};
use rusqlite::{params, Connection};

use super::types::*;

/// Intermediate row struct for deserialization.
pub(super) struct EventRow {
    pub event_id: String,
    pub ts: String,
    pub event_type: String,
    pub branch: String,
    pub parent_hash: Option<String>,
    pub hash: String,
    pub payload_str: String,
    pub refs_blobs_str: String,
    pub refs_events_str: String,
    pub refs_prov_str: String,
    pub schema_version: u32,
    pub digests_str: String,
    pub event_family: Option<String>,
    pub event_level: Option<String>,
}

pub(super) fn map_event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
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

pub(super) fn row_to_event(row: EventRow) -> anyhow::Result<Event> {
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

pub(super) fn map_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DecideSnapshotRow> {
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

pub(super) fn map_bundle_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BundleRow> {
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

pub(super) fn map_dep_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DepRow> {
    Ok(DepRow {
        source_key: row.get(0)?,
        target_key: row.get(1)?,
        dep_type: row.get(2)?,
        created_event: row.get(3)?,
        created_at: row.get(4)?,
    })
}

pub(super) fn map_decision_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DecisionRow> {
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
        village_id: row.get(18)?,
    })
}

pub(super) fn map_task_brief_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskBriefRow> {
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

/// Shared materialization logic for review bundles.
/// Accepts `&Connection` — works with both `Connection` and `Transaction` (via deref coercion).
pub(super) fn materialize_bundle_sql(
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

/// Materialize a task_intake event into the task_briefs table.
pub(super) fn materialize_task_brief_sql(
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
pub(super) fn update_task_brief_on_commit(conn: &Connection, event: &Event) -> anyhow::Result<()> {
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
pub(super) fn update_task_brief_on_note(conn: &Connection, event: &Event) -> anyhow::Result<()> {
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
pub(super) fn update_task_brief_on_merge(conn: &Connection, event: &Event) -> anyhow::Result<()> {
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
pub(super) fn extract_artifacts_from_payload(
    payload: &serde_json::Value,
    artifacts: &mut Vec<String>,
) {
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
pub(super) fn payload_tags(payload: &serde_json::Value) -> Vec<String> {
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
pub(super) fn extract_feedback_from_payload(payload: &serde_json::Value) -> Option<String> {
    if let Some(msg) = payload["message"].as_str() {
        return Some(msg.to_string());
    }
    if let Some(text) = payload["text"].as_str() {
        return Some(text.to_string());
    }
    None
}

pub(super) fn time_now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}
