use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use edda_core::policy::{self, ActorKind};

use crate::error::AppError;
use crate::state::AppState;

// ── GET /api/actors ──

#[derive(Serialize)]
struct ActorResponse {
    name: String,
    kind: ActorKind,
    roles: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<String>,
}

#[derive(Serialize)]
struct ActorsListResponse {
    actors: Vec<ActorResponse>,
}

async fn get_actors(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ActorsListResponse>, AppError> {
    let ledger = state.open_ledger()?;
    let cfg = policy::load_actors_from_dir(&ledger.paths.edda_dir)?;
    let actors = cfg
        .actors
        .into_iter()
        .map(|(name, def)| ActorResponse {
            name,
            kind: def.kind,
            roles: def.roles,
            email: def.email,
            display_name: def.display_name,
            runtime: def.runtime,
        })
        .collect();
    Ok(Json(ActorsListResponse { actors }))
}

// ── GET /api/actors/:name ──

async fn get_actor(
    State(state): State<Arc<AppState>>,
    AxumPath(name): AxumPath<String>,
) -> Result<Json<ActorResponse>, AppError> {
    let ledger = state.open_ledger()?;
    let cfg = policy::load_actors_from_dir(&ledger.paths.edda_dir)?;
    match cfg.actors.get(&name) {
        Some(def) => Ok(Json(ActorResponse {
            name,
            kind: def.kind.clone(),
            roles: def.roles.clone(),
            email: def.email.clone(),
            display_name: def.display_name.clone(),
            runtime: def.runtime.clone(),
        })),
        None => Err(AppError::NotFound(format!("Actor '{name}' not found"))),
    }
}
// ── GET /dashboard (HTML) ──

async fn serve_dashboard() -> impl IntoResponse {
    axum::response::Html(include_str!("../../static/dashboard.html"))
}

// ── GET /api/briefs ──

#[derive(Deserialize)]
struct BriefsQuery {
    status: Option<String>,
    intent: Option<String>,
}

async fn get_briefs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BriefsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ledger = state.open_ledger()?;
    let briefs = ledger.list_task_briefs(params.status.as_deref(), params.intent.as_deref())?;

    let items: Vec<serde_json::Value> = briefs
        .iter()
        .map(|b| {
            serde_json::json!({
                "task_id": b.task_id,
                "intake_event_id": b.intake_event_id,
                "title": b.title,
                "intent": b.intent.as_str(),
                "source_url": b.source_url,
                "status": b.status.as_str(),
                "branch": b.branch,
                "iterations": b.iterations,
                "artifacts": serde_json::from_str::<serde_json::Value>(&b.artifacts).unwrap_or_default(),
                "decisions": serde_json::from_str::<serde_json::Value>(&b.decisions).unwrap_or_default(),
                "last_feedback": b.last_feedback,
                "created_at": b.created_at,
                "updated_at": b.updated_at,
            })
        })
        .collect();

    Ok(Json(
        serde_json::json!({ "briefs": items, "count": items.len() }),
    ))
}

// ── GET /api/briefs/:task_id ──

async fn get_brief(
    State(state): State<Arc<AppState>>,
    AxumPath(task_id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ledger = state.open_ledger()?;
    let brief = ledger
        .get_task_brief(&task_id)?
        .ok_or_else(|| AppError::NotFound(format!("task brief not found: {task_id}")))?;

    Ok(Json(serde_json::json!({
        "task_id": brief.task_id,
        "intake_event_id": brief.intake_event_id,
        "title": brief.title,
        "intent": brief.intent.as_str(),
        "source_url": brief.source_url,
        "status": brief.status.as_str(),
        "branch": brief.branch,
        "iterations": brief.iterations,
        "artifacts": serde_json::from_str::<serde_json::Value>(&brief.artifacts).unwrap_or_default(),
        "decisions": serde_json::from_str::<serde_json::Value>(&brief.decisions).unwrap_or_default(),
        "last_feedback": brief.last_feedback,
        "created_at": brief.created_at,
        "updated_at": brief.updated_at,
    })))
}

/// Briefs, actors, and dashboard HTML routes.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/actors", get(get_actors))
        .route("/api/actors/{name}", get(get_actor))
        .route("/api/briefs", get(get_briefs))
        .route("/api/briefs/{task_id}", get(get_brief))
        .route("/dashboard", get(serve_dashboard))
}
