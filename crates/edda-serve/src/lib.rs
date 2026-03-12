use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

use edda_aggregate::aggregate::{aggregate_overview, DateRange};
use edda_aggregate::quality::{model_quality_from_events, QualityReport};
use edda_core::event::{finalize_event, new_decision_event, new_execution_event, new_note_event};
use edda_core::policy;
use edda_core::types::{rel, DecisionPayload, Provenance};
use edda_derive::{rebuild_branch, render_context, DeriveOptions};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use edda_store::registry::list_projects;

// ── Config ──

pub struct ServeConfig {
    pub bind: String,
    pub port: u16,
}

// ── App State ──

struct AppState {
    repo_root: PathBuf,
    chronicle: Option<ChronicleContext>,
}

#[allow(dead_code)]
struct ChronicleContext {
    store_root: PathBuf,
}

impl AppState {
    fn open_ledger(&self) -> anyhow::Result<Ledger> {
        Ledger::open(&self.repo_root)
    }
}

// ── Error Handling ──

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.0.to_string() });
        (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

// ── Entrypoint ──

pub async fn serve(repo_root: &Path, config: ServeConfig) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("not a edda workspace (run `edda init` first)");
    }

    let store_root = edda_store::store_root();
    let chronicle = if store_root.exists() {
        Some(ChronicleContext { store_root })
    } else {
        None
    };

    let state = Arc::new(AppState {
        repo_root: repo_root.to_path_buf(),
        chronicle,
    });

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(get_status))
        .route("/api/context", get(get_context))
        .route("/api/decisions", get(get_decisions))
        .route(
            "/api/decisions/{event_id}/outcomes",
            get(get_decision_outcomes),
        )
        .route("/api/log", get(get_log))
        .route("/api/drafts", get(get_drafts))
        .route("/api/note", post(post_note))
        .route("/api/decide", post(post_decide))
        .route("/api/events/karvi", post(post_karvi_event))
        .route("/api/scope/check", post(post_scope_check))
        .route("/api/scope/whitelist", get(get_scope_whitelist))
        .route("/api/authz/check", post(post_authz_check))
        .route("/api/recap", get(get_recap))
        .route("/api/recap/cached", get(get_recap_cached))
        .route("/api/overview", get(get_overview))
        .route("/api/projects", get(get_projects))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", config.bind, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "HTTP server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the router (for testing without binding to a port).
pub fn router(repo_root: &Path) -> Router {
    let store_root = edda_store::store_root();
    let chronicle = if store_root.exists() {
        Some(ChronicleContext { store_root })
    } else {
        None
    };

    let state = Arc::new(AppState {
        repo_root: repo_root.to_path_buf(),
        chronicle,
    });
    Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(get_status))
        .route("/api/context", get(get_context))
        .route("/api/decisions", get(get_decisions))
        .route(
            "/api/decisions/{event_id}/outcomes",
            get(get_decision_outcomes),
        )
        .route("/api/log", get(get_log))
        .route("/api/drafts", get(get_drafts))
        .route("/api/note", post(post_note))
        .route("/api/decide", post(post_decide))
        .route("/api/events/karvi", post(post_karvi_event))
        .route("/api/scope/check", post(post_scope_check))
        .route("/api/scope/whitelist", get(get_scope_whitelist))
        .route("/api/authz/check", post(post_authz_check))
        .route("/api/recap", get(get_recap))
        .route("/api/recap/cached", get(get_recap_cached))
        .route("/api/overview", get(get_overview))
        .route("/api/projects", get(get_projects))
        .route("/api/metrics/quality", get(get_quality_metrics))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

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
    limit: Option<usize>,
    all: Option<bool>,
    branch: Option<String>,
}

async fn get_decisions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DecisionsQuery>,
) -> Result<Json<edda_ask::AskResult>, AppError> {
    let ledger = state.open_ledger()?;
    let q = params.q.as_deref().unwrap_or("");
    let opts = edda_ask::AskOptions {
        limit: params.limit.unwrap_or(20),
        include_superseded: params.all.unwrap_or(false),
        branch: params.branch,
        impact: false,
    };
    let result = edda_ask::ask(&ledger, q, &opts, None)?;
    Ok(Json(result))
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
        None => {
            let body = serde_json::json!({ "error": format!("decision not found: {}", event_id) });
            Ok((StatusCode::NOT_FOUND, Json(body)).into_response())
        }
    }
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
    event_type: String,
    event_id: String,
    branch: String,
    detail: String,
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
    let events = ledger.iter_events()?;
    let limit = params.limit.unwrap_or(50);

    let results: Vec<LogEntry> = events
        .iter()
        .rev()
        .filter(|e| e.branch == head)
        .filter(|e| {
            if let Some(ref et) = params.r#type {
                if e.event_type != *et {
                    return false;
                }
            }
            if let Some(ref kw) = params.keyword {
                let payload_str = e.payload.to_string().to_lowercase();
                if !payload_str.contains(&kw.to_lowercase()) {
                    return false;
                }
            }
            if let Some(ref after) = params.after {
                if e.ts.as_str() < after.as_str() {
                    return false;
                }
            }
            if let Some(ref before) = params.before {
                if e.ts.as_str() > before.as_str() {
                    return false;
                }
            }
            true
        })
        .take(limit)
        .map(|e| {
            let detail = e
                .payload
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| e.payload.get("title").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            LogEntry {
                ts: e.ts.clone(),
                event_type: e.event_type.clone(),
                event_id: e.event_id.clone(),
                branch: e.branch.clone(),
                detail,
            }
        })
        .collect();

    Ok(Json(LogResponse { events: results }))
}

// ── GET /api/drafts ──

#[derive(Serialize)]
struct DraftItem {
    draft_id: String,
    title: String,
    stage_id: String,
    role: String,
    approved: usize,
    min_approvals: usize,
}

#[derive(Serialize)]
struct DraftsResponse {
    drafts: Vec<DraftItem>,
}

#[derive(Deserialize)]
struct MinimalDraft {
    #[serde(default)]
    draft_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    stages: Vec<MinimalStage>,
}

#[derive(Deserialize)]
struct MinimalStage {
    #[serde(default)]
    stage_id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    min_approvals: usize,
    #[serde(default)]
    approved_by: Vec<String>,
    #[serde(default)]
    status: String,
}

async fn get_drafts(State(state): State<Arc<AppState>>) -> Result<Json<DraftsResponse>, AppError> {
    let ledger = state.open_ledger()?;
    let drafts_dir = &ledger.paths.drafts_dir;

    if !drafts_dir.exists() {
        return Ok(Json(DraftsResponse { drafts: vec![] }));
    }

    let mut items = Vec::new();
    for entry in std::fs::read_dir(drafts_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path.file_stem().and_then(|s| s.to_str()) == Some("latest") {
            continue;
        }

        let content = std::fs::read_to_string(&path)?;
        let draft: MinimalDraft = match serde_json::from_str(&content) {
            Ok(d) => d,
            Err(_) => continue,
        };

        if draft.status == "applied" {
            continue;
        }

        for stage in &draft.stages {
            if stage.status != "pending" {
                continue;
            }
            items.push(DraftItem {
                draft_id: draft.draft_id.clone(),
                title: draft.title.clone(),
                stage_id: stage.stage_id.clone(),
                role: stage.role.clone(),
                approved: stage.approved_by.len(),
                min_approvals: stage.min_approvals,
            });
        }
    }

    Ok(Json(DraftsResponse { drafts: items }))
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
    Json(body): Json<NoteBody>,
) -> Result<Json<EventResponse>, AppError> {
    let ledger = state.open_ledger()?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let role = body.role.as_deref().unwrap_or("user");
    let tags = body.tags.unwrap_or_default();

    let event = new_note_event(&branch, parent_hash.as_deref(), role, &body.text, &tags)?;
    ledger.append_event(&event)?;

    Ok(Json(EventResponse {
        event_id: event.event_id,
    }))
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
    Json(body): Json<DecideBody>,
) -> Result<Json<DecideResponse>, AppError> {
    let (key, value) = body.decision.split_once('=').ok_or_else(|| {
        anyhow::anyhow!("decision must be in key=value format (e.g. \"db.engine=postgres\")")
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
    };
    let mut event = new_decision_event(&branch, parent_hash.as_deref(), "system", &dp)?;

    // Auto-supersede: find prior decision with same key
    let prior = find_prior_decision(&ledger, &branch, key);
    let mut superseded = None;
    if let Some((prior_id, prior_value)) = &prior {
        if prior_value.as_deref() != Some(value) {
            superseded = Some(prior_id.clone());
            event.refs.provenance.push(Provenance {
                target: prior_id.clone(),
                rel: rel::SUPERSEDES.to_string(),
                note: Some(format!("key '{}' re-decided", key)),
            });
        }
    }

    finalize_event(&mut event);
    ledger.append_event(&event)?;

    Ok(Json(DecideResponse {
        event_id: event.event_id,
        superseded,
    }))
}

/// Find the most recent decision event with the same key on the given branch.
fn find_prior_decision(
    ledger: &Ledger,
    branch: &str,
    key: &str,
) -> Option<(String, Option<String>)> {
    let events = ledger.iter_events().ok()?;
    events
        .iter()
        .rev()
        .filter(|e| e.branch == branch && e.event_type == "note")
        .filter(|e| edda_core::decision::is_decision(&e.payload))
        .find_map(|e| {
            let dp = edda_core::decision::extract_decision(&e.payload)?;
            if dp.key == key {
                Some((e.event_id.clone(), Some(dp.value)))
            } else {
                None
            }
        })
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

// ── GET /api/recap ──

#[derive(Deserialize)]
#[allow(dead_code)]
struct RecapQuery {
    project: Option<String>,
    query: Option<String>,
    since: Option<String>,
    week: Option<bool>,
    scope: Option<String>,
}

#[derive(Serialize)]
struct RecapAnchor {
    #[serde(rename = "type")]
    anchor_type: String,
    value: String,
}

#[derive(Serialize)]
struct NeedsYouItem {
    severity: String,
    summary: String,
    action: String,
}

#[derive(Serialize)]
struct DecisionItem {
    key: String,
    value: String,
    reason: String,
}

#[derive(Serialize)]
struct RelatedItem {
    summary: String,
    relevance: String,
}

#[derive(Serialize)]
struct RecapLayers {
    net_result: String,
    needs_you: Vec<NeedsYouItem>,
    decisions: Vec<DecisionItem>,
    related: Vec<RelatedItem>,
}

#[derive(Serialize)]
struct RecapMeta {
    sessions_analyzed: usize,
    llm_used: bool,
    cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback: Option<String>,
}

#[derive(Serialize)]
struct RecapResponse {
    anchor: RecapAnchor,
    layers: RecapLayers,
    meta: RecapMeta,
}

async fn get_recap(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RecapQuery>,
) -> Result<Json<RecapResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(anyhow::anyhow!("chronicle feature not enabled").into());
    }

    let anchor = if let Some(ref project) = params.project {
        RecapAnchor {
            anchor_type: "project".to_string(),
            value: project.clone(),
        }
    } else if let Some(ref query) = params.query {
        RecapAnchor {
            anchor_type: "query".to_string(),
            value: query.clone(),
        }
    } else if params.week.unwrap_or(false) {
        RecapAnchor {
            anchor_type: "time".to_string(),
            value: "week".to_string(),
        }
    } else if params.scope.as_deref() == Some("all") {
        RecapAnchor {
            anchor_type: "scope".to_string(),
            value: "all".to_string(),
        }
    } else {
        RecapAnchor {
            anchor_type: "default".to_string(),
            value: "current".to_string(),
        }
    };

    // TODO: Replace with actual edda-chronicle integration when #173 is complete
    // For now, return a stub response
    let response = RecapResponse {
        anchor,
        layers: RecapLayers {
            net_result: "Recap engine not yet integrated (depends on #173)".to_string(),
            needs_you: vec![],
            decisions: vec![],
            related: vec![],
        },
        meta: RecapMeta {
            sessions_analyzed: 0,
            llm_used: false,
            cached: false,
            cost_usd: None,
            fallback: Some("stub".to_string()),
        },
    };

    Ok(Json(response))
}

// ── GET /api/recap/cached ──

#[derive(Deserialize)]
struct RecapCachedQuery {
    project: Option<String>,
}

async fn get_recap_cached(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RecapCachedQuery>,
) -> Result<Json<RecapResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(anyhow::anyhow!("chronicle feature not enabled").into());
    }

    let anchor = if let Some(ref project) = params.project {
        RecapAnchor {
            anchor_type: "project".to_string(),
            value: project.clone(),
        }
    } else {
        RecapAnchor {
            anchor_type: "default".to_string(),
            value: "current".to_string(),
        }
    };

    // TODO: Replace with actual cache lookup when #176 is complete
    // For now, return a 404-like response
    let response = RecapResponse {
        anchor,
        layers: RecapLayers {
            net_result: "No cached recap available".to_string(),
            needs_you: vec![],
            decisions: vec![],
            related: vec![],
        },
        meta: RecapMeta {
            sessions_analyzed: 0,
            llm_used: false,
            cached: true,
            cost_usd: None,
            fallback: Some("cache_miss".to_string()),
        },
    };

    Ok(Json(response))
}

// ── GET /api/overview ──

#[derive(Serialize)]
struct OverviewRedItem {
    project: String,
    summary: String,
    action: String,
    blocked_count: usize,
}

#[derive(Serialize)]
struct OverviewYellowItem {
    project: String,
    summary: String,
    eta: String,
}

#[derive(Serialize)]
struct OverviewGreenItem {
    project: String,
    summary: String,
}

#[derive(Serialize)]
struct OverviewResponse {
    red: Vec<OverviewRedItem>,
    yellow: Vec<OverviewYellowItem>,
    green: Vec<OverviewGreenItem>,
    updated_at: String,
}

async fn get_overview(
    State(state): State<Arc<AppState>>,
) -> Result<Json<OverviewResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(anyhow::anyhow!("chronicle feature not enabled").into());
    }

    let projects = list_projects();
    let range = DateRange::default();
    let _aggregate = aggregate_overview(&projects, &range);

    // TODO: Implement actual attention routing logic
    // For now, return empty lists
    let now = time::OffsetDateTime::now_utc();
    let updated_at = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string());

    let response = OverviewResponse {
        red: vec![],
        yellow: vec![],
        green: vec![],
        updated_at,
    };

    Ok(Json(response))
}

// ── GET /api/projects ──

#[derive(Serialize)]
struct ProjectStatus {
    name: String,
    id: String,
    last_activity: String,
    status: String,
}

#[derive(Serialize)]
struct ProjectsResponse {
    projects: Vec<ProjectStatus>,
}

async fn get_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ProjectsResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(anyhow::anyhow!("chronicle feature not enabled").into());
    }

    let projects = list_projects();
    let project_statuses: Vec<ProjectStatus> = projects
        .into_iter()
        .map(|p| ProjectStatus {
            name: p.name,
            id: p.project_id,
            last_activity: p.last_seen,
            status: "unknown".to_string(), // TODO: Calculate from overview
        })
        .collect();

    Ok(Json(ProjectsResponse {
        projects: project_statuses,
    }))
}

// ── GET /api/metrics/quality ──

#[derive(Deserialize)]
struct QualityQuery {
    after: Option<String>,
    before: Option<String>,
}

async fn get_quality_metrics(
    State(state): State<Arc<AppState>>,
    Query(params): Query<QualityQuery>,
) -> Result<Json<QualityReport>, AppError> {
    let range = DateRange {
        after: params.after,
        before: params.before,
    };
    let ledger = state.open_ledger()?;
    let events = ledger.iter_events_by_type("execution_event")?;
    let report = model_quality_from_events(&events, &range);
    Ok(Json(report))
}

// ── POST /api/scope/check ──

#[derive(Deserialize)]
struct ScopeCheckBody {
    project_id: String,
    session_id: String,
    files: Vec<String>,
}

#[derive(Serialize)]
struct ScopeCheckResult {
    path: String,
    allowed: bool,
}

#[derive(Serialize)]
struct ScopeCheckResponse {
    session_id: String,
    label: String,
    scope: Vec<String>,
    no_claim: bool,
    all_allowed: bool,
    results: Vec<ScopeCheckResult>,
}

async fn post_scope_check(
    Json(body): Json<ScopeCheckBody>,
) -> Result<Json<ScopeCheckResponse>, AppError> {
    let board = edda_bridge_claude::peers::compute_board_state(&body.project_id);
    let claim = board
        .claims
        .iter()
        .find(|c| c.session_id == body.session_id);

    match claim {
        None => {
            // Permissive default: no claim means all files allowed
            let results = body
                .files
                .iter()
                .map(|f| ScopeCheckResult {
                    path: f.clone(),
                    allowed: true,
                })
                .collect();
            Ok(Json(ScopeCheckResponse {
                session_id: body.session_id,
                label: String::new(),
                scope: vec![],
                no_claim: true,
                all_allowed: true,
                results,
            }))
        }
        Some(claim) => {
            // Build glob set from claim patterns
            let mut builder = globset::GlobSetBuilder::new();
            for pattern in &claim.paths {
                if let Ok(glob) = globset::GlobBuilder::new(pattern)
                    .literal_separator(false)
                    .build()
                {
                    builder.add(glob);
                }
            }
            let glob_set = builder
                .build()
                .map_err(|e| anyhow::anyhow!("invalid glob patterns: {}", e))?;

            let results: Vec<ScopeCheckResult> = body
                .files
                .iter()
                .map(|f| ScopeCheckResult {
                    path: f.clone(),
                    allowed: glob_set.is_match(f),
                })
                .collect();

            let all_allowed = results.iter().all(|r| r.allowed);

            Ok(Json(ScopeCheckResponse {
                session_id: body.session_id,
                label: claim.label.clone(),
                scope: claim.paths.clone(),
                no_claim: false,
                all_allowed,
                results,
            }))
        }
    }
}

// ── GET /api/scope/whitelist ──

#[derive(Deserialize)]
struct WhitelistQuery {
    project_id: String,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Serialize)]
struct WhitelistClaim {
    session_id: String,
    label: String,
    patterns: Vec<String>,
    ts: String,
}

#[derive(Serialize)]
struct WhitelistResponse {
    claims: Vec<WhitelistClaim>,
}

async fn get_scope_whitelist(
    Query(query): Query<WhitelistQuery>,
) -> Result<Json<WhitelistResponse>, AppError> {
    let board = edda_bridge_claude::peers::compute_board_state(&query.project_id);

    let claims: Vec<WhitelistClaim> = board
        .claims
        .iter()
        .filter(|c| {
            query
                .session_id
                .as_ref()
                .is_none_or(|sid| &c.session_id == sid)
        })
        .map(|c| WhitelistClaim {
            session_id: c.session_id.clone(),
            label: c.label.clone(),
            patterns: c.paths.clone(),
            ts: c.ts.clone(),
        })
        .collect();

    Ok(Json(WhitelistResponse { claims }))
}

// ── POST /api/authz/check ──

async fn post_authz_check(
    State(state): State<Arc<AppState>>,
    Json(body): Json<policy::AuthzRequest>,
) -> Result<Json<policy::AuthzResult>, AppError> {
    let edda_dir = state.repo_root.join(".edda");
    let pol = policy::load_policy_from_dir(&edda_dir)?;
    let actors = policy::load_actors_from_dir(&edda_dir)?;
    let result = policy::evaluate_authz(&body, &pol, &actors);
    Ok(Json(result))
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Serialize tests that set EDDA_STORE_ROOT env var.
    static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn setup_workspace(dir: &Path) {
        let paths = edda_ledger::EddaPaths::discover(dir);
        paths.ensure_layout().unwrap();
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
    }

    #[tokio::test]
    async fn status_returns_branch() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["branch"], "main");
    }

    #[tokio::test]
    async fn context_returns_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/context")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["context"].as_str().unwrap().contains("main"));
    }

    #[tokio::test]
    async fn post_note_creates_event() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/note")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"text": "hello from HTTP"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["event_id"].as_str().unwrap().starts_with("evt_"));
    }

    #[tokio::test]
    async fn post_decide_creates_event() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"decision": "db.engine=sqlite", "reason": "embedded"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["event_id"].as_str().unwrap().starts_with("evt_"));
        assert!(json.get("superseded").is_none() || json["superseded"].is_null());
    }

    #[tokio::test]
    async fn log_returns_events() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Seed an event
        let ledger = Ledger::open(tmp.path()).unwrap();
        let parent_hash = ledger.last_event_hash().unwrap();
        let event =
            new_note_event("main", parent_hash.as_deref(), "user", "test note", &[]).unwrap();
        ledger.append_event(&event).unwrap();
        drop(ledger);

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/log")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let events = json["events"].as_array().unwrap();
        assert!(!events.is_empty());
        assert_eq!(events[0]["event_type"], "note");
    }

    #[tokio::test]
    async fn decisions_returns_results() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/decisions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn drafts_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/drafts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["drafts"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn projects_returns_list() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/projects")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["projects"].is_array());
    }

    #[tokio::test]
    async fn overview_returns_empty_structure() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/overview")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["red"].as_array().unwrap().is_empty());
        assert!(json["yellow"].as_array().unwrap().is_empty());
        assert!(json["green"].as_array().unwrap().is_empty());
        assert!(json["updated_at"].is_string());
    }

    #[tokio::test]
    async fn recap_returns_stub_response() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/recap")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["anchor"].is_object());
        assert!(json["layers"].is_object());
        assert!(json["meta"].is_object());
    }

    #[tokio::test]
    async fn recap_cached_returns_stub_response() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/recap/cached")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["anchor"].is_object());
        assert!(json["meta"]["cached"].is_boolean());
    }

    fn karvi_event_json(event_id: &str, event_type: &str) -> serde_json::Value {
        serde_json::json!({
            "version": "karvi.event.v1",
            "event_id": event_id,
            "event_type": event_type,
            "occurred_at": "2026-03-11T00:00:00Z",
            "trace_id": "trace-1",
            "task_id": "task-1",
            "step_id": "step-1",
            "project": "owner/repo",
            "runtime": "claude",
            "model": "claude-3-opus",
            "actor": { "kind": "agent", "id": "agent-1" },
            "usage": { "token_in": 100, "token_out": 50, "cost_usd": 0.01, "latency_ms": 500 },
            "result": { "status": "success", "error_code": null, "retryable": false }
        })
    }

    #[tokio::test]
    async fn karvi_event_creates_execution_event() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let body = karvi_event_json("evt_karvi_1", "step_completed");
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/events/karvi")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["event_id"], "evt_karvi_1");
        assert_eq!(json["status"], "created");

        // Verify event is in ledger
        let ledger = Ledger::open(tmp.path()).unwrap();
        let event = ledger.get_event("evt_karvi_1").unwrap().unwrap();
        assert_eq!(event.event_type, "execution_event");
        assert_eq!(event.payload["runtime"], "claude");
        assert_eq!(event.payload["usage"]["cost_usd"], 0.01);
    }

    #[tokio::test]
    async fn karvi_event_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let body = karvi_event_json("evt_karvi_dup", "step_completed").to_string();

        // First request
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/events/karvi")
                    .header("content-type", "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Second (duplicate) request
        let app2 = router(tmp.path());
        let resp2 = app2
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/events/karvi")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let json2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["status"], "duplicate");

        // Only one event in ledger
        let ledger = Ledger::open(tmp.path()).unwrap();
        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn karvi_event_rejects_bad_version() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let body = serde_json::json!({
            "version": "karvi.event.v99",
            "event_id": "evt_bad",
            "event_type": "step_completed",
            "occurred_at": "2026-03-11T00:00:00Z"
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/events/karvi")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("unsupported version"));
    }

    #[tokio::test]
    async fn karvi_event_rejects_bad_event_type() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let body = serde_json::json!({
            "version": "karvi.event.v1",
            "event_id": "evt_bad_type",
            "event_type": "step_exploded",
            "occurred_at": "2026-03-11T00:00:00Z"
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/events/karvi")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("unsupported event_type"));
    }

    #[tokio::test]
    async fn karvi_event_with_decision_ref() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let mut body = karvi_event_json("evt_karvi_ref", "step_completed");
        body["decision_ref"] = serde_json::json!("evt_decision_xyz");

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/events/karvi")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CREATED);

        // Verify provenance link
        let ledger = Ledger::open(tmp.path()).unwrap();
        let event = ledger.get_event("evt_karvi_ref").unwrap().unwrap();
        assert_eq!(event.refs.provenance.len(), 1);
        assert_eq!(event.refs.provenance[0].target, "evt_decision_xyz");
        assert_eq!(event.refs.provenance[0].rel, "based_on");
    }

    #[tokio::test]
    async fn get_decision_outcomes_returns_metrics() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = Router::new().merge(router(tmp.path()));

        // Create a decision
        let decide_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"decision": "db.engine=postgres", "reason": "test"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(decide_resp.status(), StatusCode::OK);

        let decide_body = axum::body::to_bytes(decide_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let decide_json: serde_json::Value = serde_json::from_slice(&decide_body).unwrap();
        let decision_event_id = decide_json["event_id"].as_str().unwrap();

        // Add execution events linked to the decision
        let exec_body = serde_json::json!({
            "version": "karvi.event.v1",
            "event_id": "evt_exec_1",
            "event_type": "step_completed",
            "occurred_at": "2026-03-01T10:00:00Z",
            "trace_id": "trace_1",
            "task_id": "task_1",
            "step_id": "step_1",
            "project": "test/repo",
            "runtime": "opencode",
            "model": "gpt-4",
            "actor": { "kind": "agent", "id": "test" },
            "usage": { "token_in": 100, "token_out": 50, "cost_usd": 0.01, "latency_ms": 500 },
            "result": { "status": "success" },
            "decision_ref": decision_event_id
        });

        let _exec_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/events/karvi")
                    .header("content-type", "application/json")
                    .body(Body::from(exec_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Query outcomes
        let outcomes_resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/decisions/{}/outcomes", decision_event_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(outcomes_resp.status(), StatusCode::OK);
        let outcomes_body = axum::body::to_bytes(outcomes_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let outcomes_json: serde_json::Value = serde_json::from_slice(&outcomes_body).unwrap();

        assert_eq!(outcomes_json["decision_key"], "db.engine");
        assert_eq!(outcomes_json["decision_value"], "postgres");
        assert_eq!(outcomes_json["total_executions"], 1);
        assert_eq!(outcomes_json["success_count"], 1);
        assert_eq!(outcomes_json["failed_count"], 0);
        assert!((outcomes_json["success_rate"].as_f64().unwrap() - 100.0).abs() < 0.01);
        assert!((outcomes_json["total_cost_usd"].as_f64().unwrap() - 0.01).abs() < 0.0001);
        assert_eq!(outcomes_json["total_tokens_in"], 100);
        assert_eq!(outcomes_json["total_tokens_out"], 50);
    }

    #[tokio::test]
    async fn get_decision_outcomes_404_for_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = Router::new().merge(router(tmp.path()));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/decisions/evt_nonexistent/outcomes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn quality_endpoint_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/metrics/quality")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total_steps"], 0);
        assert_eq!(json["overall_success_rate"], 0.0);
        assert!(json["models"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn quality_endpoint_with_events() {
        use edda_core::event::new_execution_event;

        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let ledger = Ledger::open(tmp.path()).unwrap();
        let branch = ledger.head_branch().unwrap();

        let payload1 = serde_json::json!({
            "runtime": "claude", "model": "claude-3-opus",
            "usage": { "token_in": 100, "token_out": 50, "cost_usd": 0.01, "latency_ms": 500 },
            "result": { "status": "success" },
            "event_type": "step_completed",
        });
        let e1 = new_execution_event(
            &branch,
            None,
            "evt_q1",
            "2026-03-11T00:00:00Z",
            payload1,
            None,
        )
        .unwrap();
        ledger.append_event(&e1).unwrap();

        let payload2 = serde_json::json!({
            "runtime": "claude", "model": "claude-3-opus",
            "usage": { "token_in": 200, "token_out": 80, "cost_usd": 0.02, "latency_ms": 700 },
            "result": { "status": "failed" },
            "event_type": "step_failed",
        });
        let e2 = new_execution_event(
            &branch,
            Some(&e1.hash),
            "evt_q2",
            "2026-03-11T01:00:00Z",
            payload2,
            None,
        )
        .unwrap();
        ledger.append_event(&e2).unwrap();
        drop(ledger);

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/metrics/quality")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let report: QualityReport = serde_json::from_slice(&body).unwrap();
        assert_eq!(report.total_steps, 2);
        assert_eq!(report.models.len(), 1);
        assert_eq!(report.models[0].model, "claude-3-opus");
        assert_eq!(report.models[0].success_count, 1);
        assert_eq!(report.models[0].failed_count, 1);
        assert!((report.overall_success_rate - 0.5).abs() < 1e-9);
    }

    #[tokio::test]
    async fn quality_endpoint_with_date_filter() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let app1 = router(tmp.path());
        let body1 = karvi_event_json("evt_qf1", "step_completed");
        app1.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/events/karvi")
                .header("content-type", "application/json")
                .body(Body::from(body1.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

        let app2 = router(tmp.path());
        let resp = app2
            .oneshot(
                Request::builder()
                    .uri("/api/metrics/quality?after=2099-01-01T00:00:00Z")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let report: QualityReport = serde_json::from_slice(&body).unwrap();
        assert_eq!(report.total_steps, 0);
    }

    // ── Scope check tests ──

    /// Helper: set up EDDA_STORE_ROOT and write a claim, returning the project_id.
    fn setup_claim(store_dir: &Path, session_id: &str, label: &str, paths: &[String]) -> String {
        let project_id = "test-project-abc";
        std::env::set_var("EDDA_STORE_ROOT", store_dir);
        edda_store::ensure_dirs(project_id).unwrap();
        edda_bridge_claude::peers::write_claim(project_id, session_id, label, paths);
        project_id.to_string()
    }

    #[tokio::test]
    async fn scope_check_with_matching_claim() {
        let _lock = STORE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store_dir = tmp.path().join("store");
        setup_workspace(tmp.path());

        let pid = setup_claim(
            &store_dir,
            "sess-1",
            "edda-serve",
            &["crates/edda-serve/*".to_string()],
        );

        let app = router(tmp.path());
        let body = serde_json::json!({
            "project_id": pid,
            "session_id": "sess-1",
            "files": ["crates/edda-serve/src/lib.rs"]
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/scope/check")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["no_claim"], false);
        assert_eq!(json["all_allowed"], true);
        assert_eq!(json["results"][0]["allowed"], true);
        assert_eq!(json["label"], "edda-serve");
    }

    #[tokio::test]
    async fn scope_check_with_out_of_scope_files() {
        let _lock = STORE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store_dir = tmp.path().join("store");
        setup_workspace(tmp.path());

        let pid = setup_claim(
            &store_dir,
            "sess-2",
            "edda-serve",
            &["crates/edda-serve/*".to_string()],
        );

        let app = router(tmp.path());
        let body = serde_json::json!({
            "project_id": pid,
            "session_id": "sess-2",
            "files": [
                "crates/edda-serve/src/lib.rs",
                "crates/edda-cli/src/main.rs"
            ]
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/scope/check")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["no_claim"], false);
        assert_eq!(json["all_allowed"], false);
        assert_eq!(json["results"][0]["path"], "crates/edda-serve/src/lib.rs");
        assert_eq!(json["results"][0]["allowed"], true);
        assert_eq!(json["results"][1]["path"], "crates/edda-cli/src/main.rs");
        assert_eq!(json["results"][1]["allowed"], false);
    }

    #[tokio::test]
    async fn scope_check_no_claim_permissive() {
        let _lock = STORE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store_dir = tmp.path().join("store");
        std::env::set_var("EDDA_STORE_ROOT", &store_dir);
        setup_workspace(tmp.path());

        let app = router(tmp.path());
        let body = serde_json::json!({
            "project_id": "no-such-project",
            "session_id": "sess-no-claim",
            "files": ["anything.rs"]
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/scope/check")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["no_claim"], true);
        assert_eq!(json["all_allowed"], true);
        assert_eq!(json["results"][0]["allowed"], true);
    }

    #[tokio::test]
    async fn scope_check_wildcard_claim() {
        let _lock = STORE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store_dir = tmp.path().join("store");
        setup_workspace(tmp.path());

        let pid = setup_claim(&store_dir, "sess-wild", "everything", &["**/*".to_string()]);

        let app = router(tmp.path());
        let body = serde_json::json!({
            "project_id": pid,
            "session_id": "sess-wild",
            "files": [
                "crates/edda-serve/src/lib.rs",
                "crates/edda-cli/src/main.rs",
                "README.md"
            ]
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/scope/check")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["no_claim"], false);
        assert_eq!(json["all_allowed"], true);
    }

    #[tokio::test]
    async fn scope_check_empty_files_list() {
        let _lock = STORE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store_dir = tmp.path().join("store");
        setup_workspace(tmp.path());

        let pid = setup_claim(&store_dir, "sess-empty", "test", &["src/*".to_string()]);

        let app = router(tmp.path());
        let body = serde_json::json!({
            "project_id": pid,
            "session_id": "sess-empty",
            "files": []
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/scope/check")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["all_allowed"], true);
        assert_eq!(json["results"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn scope_whitelist_returns_all_claims() {
        let _lock = STORE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store_dir = tmp.path().join("store");
        setup_workspace(tmp.path());

        let pid = setup_claim(&store_dir, "sess-a", "crate-a", &["crates/a/*".to_string()]);
        // Add a second claim for a different session
        edda_bridge_claude::peers::write_claim(
            &pid,
            "sess-b",
            "crate-b",
            &["crates/b/*".to_string()],
        );

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/scope/whitelist?project_id={}", pid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let claims = json["claims"].as_array().unwrap();
        assert_eq!(claims.len(), 2);
    }

    #[tokio::test]
    async fn scope_whitelist_filters_by_session() {
        let _lock = STORE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store_dir = tmp.path().join("store");
        setup_workspace(tmp.path());

        let pid = setup_claim(&store_dir, "sess-x", "crate-x", &["crates/x/*".to_string()]);
        edda_bridge_claude::peers::write_claim(
            &pid,
            "sess-y",
            "crate-y",
            &["crates/y/*".to_string()],
        );

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/scope/whitelist?project_id={}&session_id=sess-x",
                        pid
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let claims = json["claims"].as_array().unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0]["session_id"], "sess-x");
        assert_eq!(claims[0]["label"], "crate-x");
        assert_eq!(claims[0]["patterns"][0], "crates/x/*");
    }

    // ── Authz check tests ──

    fn write_policy_and_actors(dir: &Path, policy_yaml: &str, actors_yaml: &str) {
        let edda_dir = dir.join(".edda");
        std::fs::write(edda_dir.join("policy.yaml"), policy_yaml).unwrap();
        std::fs::write(edda_dir.join("actors.yaml"), actors_yaml).unwrap();
    }

    const TEST_POLICY: &str = "\
version: 2
roles:
  - lead
  - reviewer
  - operator
rules: []
permissions:
  default: deny
  grants:
    - actions: [deploy, rollback]
      roles: [lead, operator]
    - actions: [merge, approve]
      roles: [lead, reviewer]
    - actions: [read]
      roles: [\"*\"]
";

    const TEST_ACTORS: &str = "\
version: 1
actors:
  alice:
    roles: [lead]
  bob:
    roles: [reviewer]
  charlie:
    roles: [operator]
";

    #[tokio::test]
    async fn authz_check_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        write_policy_and_actors(tmp.path(), TEST_POLICY, TEST_ACTORS);
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/authz/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"actor": "alice", "action": "deploy"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["allowed"], true);
        assert_eq!(json["actor_roles"], serde_json::json!(["lead"]));
        assert!(json["matched_grant"].is_object());
    }

    #[tokio::test]
    async fn authz_check_denied() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        write_policy_and_actors(tmp.path(), TEST_POLICY, TEST_ACTORS);
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/authz/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"actor": "bob", "action": "deploy"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["allowed"], false);
        assert!(json["reason"].as_str().unwrap().contains("no grant"));
    }

    #[tokio::test]
    async fn authz_check_unknown_actor() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        write_policy_and_actors(tmp.path(), TEST_POLICY, TEST_ACTORS);
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/authz/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"actor": "nobody", "action": "deploy"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["allowed"], false);
        assert_eq!(json["actor_roles"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn authz_check_wildcard_role() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        write_policy_and_actors(tmp.path(), TEST_POLICY, TEST_ACTORS);
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/authz/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"actor": "bob", "action": "read"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["allowed"], true);
    }

    #[tokio::test]
    async fn authz_check_no_permissions_section() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        // Default policy.yaml from setup_workspace has no permissions section
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/authz/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"actor": "anyone", "action": "deploy"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["allowed"], false);
        assert_eq!(json["policy_default"], "deny");
    }

    #[tokio::test]
    async fn authz_full_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        write_policy_and_actors(tmp.path(), TEST_POLICY, TEST_ACTORS);
        let app = Router::new().merge(router(tmp.path()));

        // 1. Allowed: alice (lead) can deploy
        let r1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/authz/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"actor": "alice", "action": "deploy"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        let b1 = axum::body::to_bytes(r1.into_body(), usize::MAX)
            .await
            .unwrap();
        let j1: serde_json::Value = serde_json::from_slice(&b1).unwrap();
        assert_eq!(j1["allowed"], true);

        // 2. Denied: bob (reviewer) cannot deploy
        let r2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/authz/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"actor": "bob", "action": "deploy"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::OK);
        let b2 = axum::body::to_bytes(r2.into_body(), usize::MAX)
            .await
            .unwrap();
        let j2: serde_json::Value = serde_json::from_slice(&b2).unwrap();
        assert_eq!(j2["allowed"], false);

        // 3. Unknown actor denied
        let r3 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/authz/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"actor": "unknown", "action": "deploy"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r3.status(), StatusCode::OK);
        let b3 = axum::body::to_bytes(r3.into_body(), usize::MAX)
            .await
            .unwrap();
        let j3: serde_json::Value = serde_json::from_slice(&b3).unwrap();
        assert_eq!(j3["allowed"], false);
        assert!(j3["actor_roles"].as_array().unwrap().is_empty());
    }
}
