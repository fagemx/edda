use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use edda_ledger::lock::WorkspaceLock;

use crate::error::AppError;
use crate::state::AppState;

// ── Ingestion types ──

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvaluateBody {
    event_type: String,
    source_layer: String,
    #[serde(default)]
    source_refs: Vec<edda_ingestion::SourceRef>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    detail: Option<serde_json::Value>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EvaluateResponse {
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    record_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestion_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManualIngestBody {
    event_type: String,
    source_layer: String,
    #[serde(default)]
    source_refs: Vec<edda_ingestion::SourceRef>,
    summary: String,
    detail: serde_json::Value,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IngestionRecordsQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    source_layer: Option<String>,
    #[serde(default)]
    trigger_type: Option<String>,
}

// ── Ingestion handlers ──

// POST /api/ingestion/evaluate
async fn post_ingestion_evaluate(
    State(state): State<Arc<AppState>>,
    body: Result<Json<EvaluateBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.body_text()))?;

    let layer: edda_ingestion::SourceLayer = body
        .source_layer
        .parse()
        .map_err(|e: String| AppError::Validation(e))?;

    let result = edda_ingestion::evaluate_trigger(&body.event_type, &body.source_layer);

    match result {
        edda_ingestion::TriggerResult::AutoIngest => {
            let ledger = state.open_ledger()?;
            let _lock = WorkspaceLock::acquire(&ledger.paths)?;

            let summary = body
                .summary
                .unwrap_or_else(|| format!("{} from {}", body.event_type, body.source_layer));
            let record = edda_ingestion::IngestionRecord {
                id: edda_ingestion::IngestionRecord::new_id("prec"),
                trigger_type: edda_ingestion::TriggerType::Auto,
                event_type: body.event_type,
                source_layer: layer,
                source_refs: body.source_refs,
                summary,
                detail: body.detail.unwrap_or(serde_json::json!({})),
                tags: body.tags,
                created_at: crate::helpers::time_now_rfc3339(),
            };

            edda_ingestion::write_ingestion_record(&ledger, &record)?;

            Ok((
                StatusCode::CREATED,
                Json(EvaluateResponse {
                    action: "ingested".to_string(),
                    record_id: Some(record.id),
                    suggestion_id: None,
                    reason: None,
                }),
            )
                .into_response())
        }
        edda_ingestion::TriggerResult::SuggestIngest { reason } => {
            let ledger = state.open_ledger()?;
            let _lock = WorkspaceLock::acquire(&ledger.paths)?;

            let summary = body
                .summary
                .unwrap_or_else(|| format!("{} from {}", body.event_type, body.source_layer));
            let suggestion = edda_ingestion::Suggestion {
                id: edda_ingestion::Suggestion::new_id(),
                event_type: body.event_type,
                source_layer: layer,
                source_refs: body.source_refs,
                summary,
                suggested_because: reason.clone(),
                detail: body.detail.unwrap_or(serde_json::json!({})),
                tags: body.tags,
                status: edda_ingestion::SuggestionStatus::Pending,
                created_at: crate::helpers::time_now_rfc3339(),
                reviewed_at: None,
            };

            let queue = edda_ingestion::SuggestionQueue::new(&ledger);
            let id = queue.enqueue(&suggestion)?;

            Ok((
                StatusCode::OK,
                Json(EvaluateResponse {
                    action: "queued".to_string(),
                    record_id: None,
                    suggestion_id: Some(id),
                    reason: Some(reason),
                }),
            )
                .into_response())
        }
        edda_ingestion::TriggerResult::Skip => Ok((
            StatusCode::OK,
            Json(EvaluateResponse {
                action: "skipped".to_string(),
                record_id: None,
                suggestion_id: None,
                reason: None,
            }),
        )
            .into_response()),
    }
}

// POST /api/ingestion/records
async fn post_ingestion_record(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ManualIngestBody>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.body_text()))?;

    let layer: edda_ingestion::SourceLayer = body
        .source_layer
        .parse()
        .map_err(|e: String| AppError::Validation(e))?;

    let ledger = state.open_ledger()?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let record = edda_ingestion::IngestionRecord {
        id: edda_ingestion::IngestionRecord::new_id("prec"),
        trigger_type: edda_ingestion::TriggerType::Manual,
        event_type: body.event_type,
        source_layer: layer,
        source_refs: body.source_refs,
        summary: body.summary,
        detail: body.detail,
        tags: body.tags,
        created_at: crate::helpers::time_now_rfc3339(),
    };

    edda_ingestion::write_ingestion_record(&ledger, &record)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "recordId": record.id })),
    ))
}

// GET /api/ingestion/records
async fn get_ingestion_records(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IngestionRecordsQuery>,
) -> Result<Json<Vec<edda_ingestion::IngestionRecord>>, AppError> {
    let ledger = state.open_ledger()?;
    let events = ledger.iter_events_by_type("ingestion")?;

    let mut records: Vec<edda_ingestion::IngestionRecord> = events
        .into_iter()
        .filter_map(|e| serde_json::from_value(e.payload).ok())
        .collect();

    if let Some(ref layer) = params.source_layer {
        records.retain(|r| r.source_layer.to_string() == *layer);
    }
    if let Some(ref tt) = params.trigger_type {
        records.retain(|r| {
            let label = match r.trigger_type {
                edda_ingestion::TriggerType::Auto => "auto",
                edda_ingestion::TriggerType::Suggested => "suggested",
                edda_ingestion::TriggerType::Manual => "manual",
            };
            label == tt.as_str()
        });
    }

    let limit = params.limit.unwrap_or(50);
    records.truncate(limit);

    Ok(Json(records))
}

// GET /api/ingestion/suggestions
async fn get_ingestion_suggestions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<edda_ingestion::Suggestion>>, AppError> {
    let ledger = state.open_ledger()?;
    let queue = edda_ingestion::SuggestionQueue::new(&ledger);
    let pending = queue.list_pending()?;
    Ok(Json(pending))
}

// POST /api/ingestion/suggestions/{id}/accept
async fn post_suggestion_accept(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<edda_ingestion::IngestionRecord>, AppError> {
    let ledger = state.open_ledger()?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    // Pre-check for proper HTTP error codes
    let row = ledger
        .get_suggestion(&id)?
        .ok_or_else(|| AppError::NotFound(format!("suggestion not found: {id}")))?;
    if row.status != "pending" {
        return Err(AppError::Conflict(format!(
            "suggestion {id} has status '{}', expected 'pending'",
            row.status
        )));
    }

    let queue = edda_ingestion::SuggestionQueue::new(&ledger);
    let record = queue.accept(&id)?;
    Ok(Json(record))
}

// POST /api/ingestion/suggestions/{id}/reject
async fn post_suggestion_reject(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ledger = state.open_ledger()?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    // Pre-check for proper HTTP error codes
    let row = ledger
        .get_suggestion(&id)?
        .ok_or_else(|| AppError::NotFound(format!("suggestion not found: {id}")))?;
    if row.status != "pending" {
        return Err(AppError::Conflict(format!(
            "suggestion {id} has status '{}', expected 'pending'",
            row.status
        )));
    }

    let queue = edda_ingestion::SuggestionQueue::new(&ledger);
    queue.reject(&id)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Ingestion routes.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/ingestion/evaluate", post(post_ingestion_evaluate))
        .route(
            "/api/ingestion/records",
            post(post_ingestion_record).get(get_ingestion_records),
        )
        .route("/api/ingestion/suggestions", get(get_ingestion_suggestions))
        .route(
            "/api/ingestion/suggestions/{id}/accept",
            post(post_suggestion_accept),
        )
        .route(
            "/api/ingestion/suggestions/{id}/reject",
            post(post_suggestion_reject),
        )
}
