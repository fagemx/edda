use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use edda_core::event::new_snapshot_event;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;

use crate::error::AppError;
use crate::state::AppState;

// ── POST /api/snapshot ──

#[derive(Deserialize)]
struct SnapshotBody {
    context: serde_json::Value,
    result: serde_json::Value,
    engine_version: String,
    #[serde(default = "default_snapshot_schema")]
    schema_version: String,
    context_hash: String,
    #[serde(default = "default_redaction_level")]
    redaction_level: String,
    village_id: Option<String>,
    cycle_id: Option<String>,
}

fn default_snapshot_schema() -> String {
    "snapshot.v1".to_string()
}

fn default_redaction_level() -> String {
    "full".to_string()
}

#[derive(Serialize)]
struct SnapshotResponse {
    event_id: String,
    context_hash: String,
}

async fn post_snapshot(
    State(state): State<Arc<AppState>>,
    body: Result<Json<SnapshotBody>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.body_text()))?;

    if body.engine_version.is_empty() {
        return Err(AppError::Validation(
            "engine_version must not be empty".into(),
        ));
    }
    if body.context_hash.is_empty() {
        return Err(AppError::Validation(
            "context_hash must not be empty".into(),
        ));
    }

    let ledger = state.open_ledger()?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    // Attempt blob offload for large payloads
    let context_bytes = serde_json::to_vec(&body.context)?;
    let result_bytes = serde_json::to_vec(&body.result)?;

    let threshold = edda_ledger::SNAPSHOT_BLOB_THRESHOLD;
    let context_blob = edda_ledger::blob_put_if_large(
        &ledger.paths,
        &context_bytes,
        edda_ledger::BlobClass::DecisionEvidence,
        threshold,
    )
    .map_err(|e| anyhow::anyhow!("writing context blob: {e}"))?;
    let result_blob = edda_ledger::blob_put_if_large(
        &ledger.paths,
        &result_bytes,
        edda_ledger::BlobClass::DecisionEvidence,
        threshold,
    )
    .map_err(|e| anyhow::anyhow!("writing result blob: {e}"))?;

    let has_blobs = context_blob.is_some() || result_blob.is_some();

    // Build event payload: metadata + inline or blob refs
    let mut payload = serde_json::json!({
        "engine_version": body.engine_version,
        "schema_version": body.schema_version,
        "context_hash": body.context_hash,
        "redaction_level": body.redaction_level,
    });
    if let Some(ref vid) = body.village_id {
        payload["village_id"] = serde_json::Value::String(vid.clone());
    }
    if let Some(ref cid) = body.cycle_id {
        payload["cycle_id"] = serde_json::Value::String(cid.clone());
    }

    let mut blob_refs = Vec::new();
    if let Some(ref br) = context_blob {
        payload["context_blob"] = serde_json::Value::String(br.clone());
        blob_refs.push(br.clone());
    } else {
        payload["context_inline"] = body.context;
    }
    if let Some(ref br) = result_blob {
        payload["result_blob"] = serde_json::Value::String(br.clone());
        blob_refs.push(br.clone());
    } else {
        payload["result_inline"] = body.result;
    }

    let event = new_snapshot_event(&branch, parent_hash.as_deref(), payload, blob_refs)?;
    let event_id = event.event_id.clone();
    let created_at = event.ts.clone();

    ledger.append_event(&event)?;

    // Insert into materialized view
    ledger.insert_snapshot(&edda_ledger::DecideSnapshotRow {
        event_id: event_id.clone(),
        context_hash: body.context_hash.clone(),
        engine_version: body.engine_version,
        schema_version: body.schema_version,
        redaction_level: body.redaction_level,
        village_id: body.village_id,
        cycle_id: body.cycle_id,
        has_blobs,
        created_at,
    })?;

    Ok((
        StatusCode::CREATED,
        Json(SnapshotResponse {
            event_id,
            context_hash: body.context_hash,
        }),
    ))
}

// ── GET /api/snapshots ──

#[derive(Deserialize)]
struct SnapshotsQuery {
    village_id: Option<String>,
    engine_version: Option<String>,
    #[serde(default = "default_snapshot_limit")]
    limit: usize,
}

fn default_snapshot_limit() -> usize {
    20
}

async fn get_snapshots(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SnapshotsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let ledger = state.open_ledger()?;
    let rows = ledger.query_snapshots(
        query.village_id.as_deref(),
        query.engine_version.as_deref(),
        query.limit,
    )?;

    let mut snapshots = Vec::new();
    for row in &rows {
        let snapshot = reconstruct_snapshot(&ledger, row)?;
        snapshots.push(snapshot);
    }

    Ok(Json(snapshots))
}

// ── GET /api/snapshots/:context_hash ──

async fn get_snapshots_by_hash(
    State(state): State<Arc<AppState>>,
    AxumPath(context_hash): AxumPath<String>,
) -> Result<impl IntoResponse, AppError> {
    let ledger = state.open_ledger()?;
    let rows = ledger.snapshots_by_context_hash(&context_hash)?;

    if rows.is_empty() {
        return Err(AppError::NotFound(format!(
            "no snapshots found for context_hash: {context_hash}"
        )));
    }

    let mut snapshots = Vec::new();
    for row in &rows {
        let snapshot = reconstruct_snapshot(&ledger, row)?;
        snapshots.push(snapshot);
    }

    Ok(Json(snapshots))
}

// ── GET /api/villages/{village_id}/stats ──

#[derive(Deserialize)]
struct VillageStatsQuery {
    /// ISO 8601 lower bound (inclusive).
    after: Option<String>,
    /// ISO 8601 upper bound (inclusive).
    before: Option<String>,
}

async fn get_village_stats(
    State(state): State<Arc<AppState>>,
    AxumPath(village_id): AxumPath<String>,
    Query(params): Query<VillageStatsQuery>,
) -> Result<Json<edda_ledger::sqlite_store::VillageStats>, AppError> {
    if let Some(ref after) = params.after {
        crate::helpers::validate_iso8601(after).map_err(AppError::Validation)?;
    }
    if let Some(ref before) = params.before {
        crate::helpers::validate_iso8601(before).map_err(AppError::Validation)?;
    }

    let ledger = state.open_ledger()?;
    let stats = ledger.village_stats(
        &village_id,
        params.after.as_deref(),
        params.before.as_deref(),
    )?;
    Ok(Json(stats))
}

// ── GET /api/patterns ──

#[derive(Deserialize)]
struct PatternsQuery {
    village_id: Option<String>,
    /// Number of days to look back (default 7, max 90).
    #[serde(default)]
    lookback_days: Option<u32>,
    /// Minimum occurrences to qualify as a pattern (default 3).
    #[serde(default)]
    min_occurrences: Option<usize>,
}

async fn get_patterns(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PatternsQuery>,
) -> Result<Json<edda_ledger::sqlite_store::PatternDetectionResult>, AppError> {
    let village_id = params
        .village_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Validation("village_id query parameter is required".into()))?;

    let lookback_days = params.lookback_days.unwrap_or(7).min(90);
    let min_occurrences = params.min_occurrences.unwrap_or(3).max(2);

    let now = time::OffsetDateTime::now_utc();
    let after_date = now - time::Duration::days(i64::from(lookback_days));
    let after_str = after_date
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    let ledger = state.open_ledger()?;
    let patterns = ledger.detect_village_patterns(village_id, &after_str, min_occurrences)?;
    let total = patterns.len();

    Ok(Json(edda_ledger::sqlite_store::PatternDetectionResult {
        village_id: village_id.to_string(),
        lookback_days,
        after: after_str,
        total_patterns: total,
        patterns,
    }))
}

/// Reconstruct a full snapshot JSON from a materialized view row + event payload.
fn reconstruct_snapshot(
    ledger: &Ledger,
    row: &edda_ledger::DecideSnapshotRow,
) -> Result<serde_json::Value, AppError> {
    let event = ledger
        .get_event(&row.event_id)?
        .ok_or_else(|| AppError::NotFound(format!("event {} not found", row.event_id)))?;

    let payload = &event.payload;

    // Resolve context: inline or blob
    let context = if let Some(inline) = payload.get("context_inline") {
        inline.clone()
    } else if let Some(blob_ref) = payload.get("context_blob").and_then(|v| v.as_str()) {
        let path =
            edda_ledger::blob_get_path(&ledger.paths, blob_ref).map_err(AppError::Internal)?;
        let bytes = std::fs::read(&path)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("read context blob: {e}")))?;
        serde_json::from_slice(&bytes)?
    } else {
        serde_json::Value::Null
    };

    // Resolve result: inline or blob
    let result = if let Some(inline) = payload.get("result_inline") {
        inline.clone()
    } else if let Some(blob_ref) = payload.get("result_blob").and_then(|v| v.as_str()) {
        let path =
            edda_ledger::blob_get_path(&ledger.paths, blob_ref).map_err(AppError::Internal)?;
        let bytes = std::fs::read(&path)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("read result blob: {e}")))?;
        serde_json::from_slice(&bytes)?
    } else {
        serde_json::Value::Null
    };

    Ok(serde_json::json!({
        "event_id": row.event_id,
        "context_hash": row.context_hash,
        "engine_version": row.engine_version,
        "schema_version": row.schema_version,
        "redaction_level": row.redaction_level,
        "village_id": row.village_id,
        "cycle_id": row.cycle_id,
        "context": context,
        "result": result,
        "created_at": row.created_at,
    }))
}

/// Snapshot and village-related routes.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/snapshot", post(post_snapshot))
        .route("/api/snapshots", get(get_snapshots))
        .route("/api/snapshots/{context_hash}", get(get_snapshots_by_hash))
        .route("/api/villages/{village_id}/stats", get(get_village_stats))
        .route("/api/patterns", get(get_patterns))
}
