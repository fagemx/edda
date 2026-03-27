use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use edda_core::event::{finalize_event, new_decision_event, new_execution_event, new_note_event};
use edda_core::types::{rel, DecisionPayload, Provenance};
use edda_derive::{rebuild_branch, render_context, DeriveOptions};
use edda_ledger::lock::WorkspaceLock;

use crate::error::AppError;
use crate::state::AppState;

// ── Health ──

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

// ── GET /api/status ──

#[derive(Serialize)]
struct StatusResponse {
    branch: String,
    last_commit: Option<LastCommit>,
    uncommitted_events: usize,
}

#[derive(Serialize)]
struct LastCommit {
    ts: String,
    event_id: String,
    title: String,
}

async fn get_status(State(state): State<Arc<AppState>>) -> Result<Json<StatusResponse>, AppError> {
    let ledger = state.open_ledger()?;
    let head = ledger.head_branch()?;
    let snap = rebuild_branch(&ledger, &head)?;

    let last_commit = snap.last_commit.as_ref().map(|c| LastCommit {
        ts: c.ts.clone(),
        event_id: c.event_id.clone(),
        title: c.title.clone(),
    });

    Ok(Json(StatusResponse {
        branch: head,
        last_commit,
        uncommitted_events: snap.uncommitted_events,
    }))
}

// ── GET /api/context ──

#[derive(Deserialize)]
struct ContextQuery {
    depth: Option<usize>,
}

#[derive(Serialize)]
struct ContextResponse {
    context: String,
}

async fn get_context(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ContextQuery>,
) -> Result<Json<ContextResponse>, AppError> {
    let ledger = state.open_ledger()?;
    let head = ledger.head_branch()?;
    let depth = params.depth.unwrap_or(5);
    let text = render_context(&ledger, &head, DeriveOptions { depth })?;
    Ok(Json(ContextResponse { context: text }))
}

// ── GET /api/decisions ──

#[derive(Deserialize)]
struct DecisionsQuery {
    q: Option<String>,
    context_summary: Option<String>,
    limit: Option<usize>,
    all: Option<bool>,
    branch: Option<String>,
    /// ISO 8601 lower bound (inclusive) for temporal filtering.
    after: Option<String>,
    /// ISO 8601 upper bound (inclusive) for temporal filtering.
    before: Option<String>,
    /// Comma-separated tags to filter by (OR semantics).
    tags: Option<String>,
    /// Filter decisions belonging to a specific village.
    village_id: Option<String>,
}

async fn get_decisions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DecisionsQuery>,
) -> Result<Json<edda_ask::AskResult>, AppError> {
    if let Some(ref after) = params.after {
        crate::helpers::validate_iso8601(after).map_err(AppError::Validation)?;
    }
    if let Some(ref before) = params.before {
        crate::helpers::validate_iso8601(before).map_err(AppError::Validation)?;
    }

    let ledger = state.open_ledger()?;
    let q = params
        .q
        .as_deref()
        .or(params.context_summary.as_deref())
        .unwrap_or("");
    let tags: Vec<String> = params
        .tags
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
        .unwrap_or_default();
    let opts = edda_ask::AskOptions {
        limit: params.limit.unwrap_or(20),
        include_superseded: params.all.unwrap_or(false),
        branch: params.branch,
        impact: false,
        after: params.after,
        before: params.before,
        tags,
        village_id: params.village_id,
    };
    let result = edda_ask::ask(&ledger, q, &opts, None)?;
    Ok(Json(result))
}

// ── POST /api/decisions/batch ──

#[derive(Deserialize)]
struct BatchQuery {
    queries: Vec<BatchSubQuery>,
    #[serde(default)]
    slim: bool,
}

#[derive(Deserialize)]
struct BatchSubQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    context_summary: Option<String>,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    all: Option<bool>,
}

#[derive(Serialize)]
struct BatchResponse {
    results: Vec<BatchSubResult>,
}

#[derive(Serialize)]
struct BatchSubResult {
    query_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    decisions: Option<Vec<edda_ask::DecisionHit>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeline: Option<Vec<edda_ask::DecisionHit>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    related_commits: Option<Vec<edda_ask::CommitHit>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    related_notes: Option<Vec<edda_ask::NoteHit>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversations: Option<Vec<edda_ask::ConversationHit>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn post_decisions_batch(
    State(state): State<Arc<AppState>>,
    body: Result<Json<BatchQuery>, JsonRejection>,
) -> Result<Json<BatchResponse>, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.body_text()))?;

    if body.queries.is_empty() || body.queries.len() > 10 {
        return Err(AppError::Validation(
            "queries must contain 1\u{2013}10 items".into(),
        ));
    }

    let ledger = state.open_ledger()?;
    let mut results = Vec::with_capacity(body.queries.len());

    for (i, sub) in body.queries.iter().enumerate() {
        let q = sub
            .q
            .as_deref()
            .or(sub.context_summary.as_deref())
            .or(sub.domain.as_deref())
            .unwrap_or("");

        let opts = edda_ask::AskOptions {
            limit: sub.limit.unwrap_or(20).min(100),
            include_superseded: sub.all.unwrap_or(false),
            branch: sub.branch.clone(),
            impact: false,
            after: None,
            before: None,
            tags: vec![],
            village_id: None,
        };

        match edda_ask::ask(&ledger, q, &opts, None) {
            Ok(result) => {
                if body.slim {
                    results.push(BatchSubResult {
                        query_index: i,
                        decisions: Some(result.decisions),
                        timeline: None,
                        related_commits: None,
                        related_notes: None,
                        conversations: None,
                        error: None,
                    });
                } else {
                    results.push(BatchSubResult {
                        query_index: i,
                        decisions: Some(result.decisions),
                        timeline: Some(result.timeline),
                        related_commits: Some(result.related_commits),
                        related_notes: Some(result.related_notes),
                        conversations: Some(result.conversations),
                        error: None,
                    });
                }
            }
            Err(e) => {
                results.push(BatchSubResult {
                    query_index: i,
                    decisions: None,
                    timeline: None,
                    related_commits: None,
                    related_notes: None,
                    conversations: None,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    Ok(Json(BatchResponse { results }))
}

// ── GET /api/decisions/:event_id/outcomes ──

async fn get_decision_outcomes(
    State(state): State<Arc<AppState>>,
    AxumPath(event_id): AxumPath<String>,
) -> Result<Response, AppError> {
    let ledger = state.open_ledger()?;
    let outcomes = ledger.decision_outcomes(&event_id)?;

    match outcomes {
        Some(metrics) => {
            let json = serde_json::to_value(metrics)?;
            Ok(Json(json).into_response())
        }
        None => Err(AppError::NotFound(format!(
            "decision not found: {}",
            event_id
        ))),
    }
}

// ── GET /api/decisions/:event_id/chain ──

#[derive(Deserialize)]
struct ChainQuery {
    depth: Option<usize>,
}

#[derive(Serialize)]
struct ChainResponse {
    root: ChainNodeResponse,
    chain: Vec<ChainNodeResponse>,
    meta: ChainMeta,
}

#[derive(Serialize)]
struct ChainNodeResponse {
    event_id: String,
    key: String,
    value: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    relation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    depth: Option<usize>,
    ts: String,
    is_active: bool,
}

#[derive(Serialize)]
struct ChainMeta {
    max_depth: usize,
    total_nodes: usize,
}

async fn get_decision_chain(
    State(state): State<Arc<AppState>>,
    AxumPath(event_id): AxumPath<String>,
    Query(params): Query<ChainQuery>,
) -> Result<Json<ChainResponse>, AppError> {
    let depth = params.depth.unwrap_or(3).min(10);
    let ledger = state.open_ledger()?;

    let (root, chain) = ledger
        .causal_chain(&event_id, depth)?
        .ok_or_else(|| AppError::NotFound(format!("decision not found: {}", event_id)))?;

    let root_node = ChainNodeResponse {
        event_id: root.event_id.clone(),
        key: root.key,
        value: root.value,
        reason: root.reason,
        relation: None,
        depth: None,
        ts: root.ts.unwrap_or_default(),
        is_active: matches!(root.status.as_str(), "active" | "experimental"),
    };

    let chain_nodes: Vec<ChainNodeResponse> = chain
        .into_iter()
        .map(|entry| ChainNodeResponse {
            event_id: entry.decision.event_id.clone(),
            key: entry.decision.key,
            value: entry.decision.value,
            reason: entry.decision.reason,
            relation: Some(entry.relation),
            depth: Some(entry.depth),
            ts: entry.decision.ts.unwrap_or_default(),
            is_active: matches!(entry.decision.status.as_str(), "active" | "experimental"),
        })
        .collect();

    let total_nodes = 1 + chain_nodes.len();
    Ok(Json(ChainResponse {
        root: root_node,
        chain: chain_nodes,
        meta: ChainMeta {
            max_depth: depth,
            total_nodes,
        },
    }))
}

// ── GET /api/log ──

#[derive(Deserialize)]
struct LogQuery {
    r#type: Option<String>,
    keyword: Option<String>,
    after: Option<String>,
    before: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct LogEntry {
    ts: String,
    #[serde(rename = "type")]
    event_type: String,
    event_id: String,
    branch: String,
    #[serde(rename = "summary")]
    detail: String,
    tags: Vec<String>,
}

#[derive(Serialize)]
struct LogResponse {
    events: Vec<LogEntry>,
}

async fn get_log(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LogQuery>,
) -> Result<Json<LogResponse>, AppError> {
    let ledger = state.open_ledger()?;
    let head = ledger.head_branch()?;
    let limit = params.limit.unwrap_or(50);

    let events = ledger.iter_events_filtered(
        &head,
        params.r#type.as_deref(),
        params.keyword.as_deref(),
        params.after.as_deref(),
        params.before.as_deref(),
        limit,
    )?;

    let results: Vec<LogEntry> = events
        .iter()
        .map(|e| {
            let detail = e
                .payload
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| e.payload.get("title").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let tags: Vec<String> = e
                .payload
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            LogEntry {
                ts: e.ts.clone(),
                event_type: e.event_type.clone(),
                event_id: e.event_id.clone(),
                branch: e.branch.clone(),
                detail,
                tags,
            }
        })
        .collect();

    Ok(Json(LogResponse { events: results }))
}
// ── POST /api/note ──

#[derive(Deserialize)]
struct NoteBody {
    text: String,
    role: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Serialize)]
struct EventResponse {
    event_id: String,
}

async fn post_note(
    State(state): State<Arc<AppState>>,
    body: Result<Json<NoteBody>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.body_text()))?;

    let ledger = state.open_ledger()?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let role = body.role.as_deref().unwrap_or("user");
    let tags = body.tags.unwrap_or_default();

    let event = new_note_event(&branch, parent_hash.as_deref(), role, &body.text, &tags)?;
    ledger.append_event(&event)?;

    Ok((
        StatusCode::CREATED,
        Json(EventResponse {
            event_id: event.event_id,
        }),
    ))
}

// ── POST /api/decide ──

#[derive(Deserialize)]
struct DecideBody {
    decision: String,
    reason: Option<String>,
}

#[derive(Serialize)]
struct DecideResponse {
    event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    superseded: Option<String>,
}

async fn post_decide(
    State(state): State<Arc<AppState>>,
    body: Result<Json<DecideBody>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.body_text()))?;

    let (key, value) = body.decision.split_once('=').ok_or_else(|| {
        AppError::Validation(
            "decision must be in key=value format (e.g. \"db.engine=postgres\")".into(),
        )
    })?;
    let key = key.trim();
    let value = value.trim();

    let ledger = state.open_ledger()?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    let dp = DecisionPayload {
        key: key.to_string(),
        value: value.to_string(),
        reason: body.reason,
        scope: None,
        authority: None,
        affected_paths: None,
        tags: None,
        review_after: None,
        reversibility: None,
        village_id: None,
    };
    let mut event = new_decision_event(&branch, parent_hash.as_deref(), "system", &dp)?;

    // Auto-supersede: find prior decision with same key via SQL index
    let prior = ledger.find_active_decision(&branch, key)?;
    let mut superseded = None;
    if let Some(ref row) = prior {
        if row.value != value {
            superseded = Some(row.event_id.clone());
            event.refs.provenance.push(Provenance {
                target: row.event_id.clone(),
                rel: rel::SUPERSEDES.to_string(),
                note: Some(format!("key '{}' re-decided", key)),
            });
        }
    }

    finalize_event(&mut event)?;
    ledger.append_event(&event)?;

    Ok((
        StatusCode::CREATED,
        Json(DecideResponse {
            event_id: event.event_id,
            superseded,
        }),
    ))
}

// ── POST /api/events/karvi ──

#[derive(Deserialize)]
struct KarviEventBody {
    version: String,
    event_id: String,
    event_type: String,
    occurred_at: String,
    #[serde(default)]
    trace_id: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    step_id: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    actor: Option<serde_json::Value>,
    #[serde(default)]
    usage: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    decision_ref: Option<String>,
}

#[derive(Serialize)]
struct KarviEventResponse {
    event_id: String,
    status: String,
}

const VALID_KARVI_EVENT_TYPES: &[&str] = &["step_completed", "step_failed", "step_cancelled"];

async fn post_karvi_event(
    State(state): State<Arc<AppState>>,
    Json(body): Json<KarviEventBody>,
) -> Result<Response, AppError> {
    // Validate version
    if body.version != "karvi.event.v1" {
        let err = serde_json::json!({
            "error": format!("unsupported version: {}", body.version),
        });
        return Ok((StatusCode::BAD_REQUEST, Json(err)).into_response());
    }

    // Validate event_type
    if !VALID_KARVI_EVENT_TYPES.contains(&body.event_type.as_str()) {
        let err = serde_json::json!({
            "error": format!("unsupported event_type: {}", body.event_type),
        });
        return Ok((StatusCode::BAD_REQUEST, Json(err)).into_response());
    }

    // Serialize full body as payload
    let payload = serde_json::json!({
        "version": body.version,
        "event_id": body.event_id,
        "event_type": body.event_type,
        "occurred_at": body.occurred_at,
        "trace_id": body.trace_id,
        "task_id": body.task_id,
        "step_id": body.step_id,
        "project": body.project,
        "runtime": body.runtime,
        "model": body.model,
        "actor": body.actor,
        "usage": body.usage,
        "result": body.result,
        "decision_ref": body.decision_ref,
    });

    let ledger = state.open_ledger()?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    let event = new_execution_event(
        &branch,
        parent_hash.as_deref(),
        &body.event_id,
        &body.occurred_at,
        payload,
        body.decision_ref.as_deref(),
    )?;

    let inserted = ledger.append_event_idempotent(&event)?;

    let response = KarviEventResponse {
        event_id: event.event_id,
        status: if inserted {
            "created".to_string()
        } else {
            "duplicate".to_string()
        },
    };

    let status = if inserted {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };

    Ok((status, Json(response)).into_response())
}

/// Public event routes (no auth required).
pub(crate) fn public_routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/health", get(health))
}

/// Protected event routes (auth middleware applied).
pub(crate) fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/status", get(get_status))
        .route("/api/context", get(get_context))
        .route("/api/decisions", get(get_decisions))
        .route("/api/decisions/batch", post(post_decisions_batch))
        .route(
            "/api/decisions/{event_id}/outcomes",
            get(get_decision_outcomes),
        )
        .route("/api/decisions/{event_id}/chain", get(get_decision_chain))
        .route("/api/log", get(get_log))
        .route("/api/note", post(post_note))
        .route("/api/decide", post(post_decide))
        .route("/api/events/karvi", post(post_karvi_event))
}

/// All event routes (for test router without auth middleware).
#[cfg(test)]
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(get_status))
        .route("/api/context", get(get_context))
        .route("/api/decisions", get(get_decisions))
        .route("/api/decisions/batch", post(post_decisions_batch))
        .route(
            "/api/decisions/{event_id}/outcomes",
            get(get_decision_outcomes),
        )
        .route("/api/decisions/{event_id}/chain", get(get_decision_chain))
        .route("/api/log", get(get_log))
        .route("/api/note", post(post_note))
        .route("/api/decide", post(post_decide))
        .route("/api/events/karvi", post(post_karvi_event))
}
