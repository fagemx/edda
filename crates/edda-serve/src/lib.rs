use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

use edda_core::event::{finalize_event, new_decision_event, new_note_event};
use edda_core::types::{rel, DecisionPayload, Provenance};
use edda_derive::{rebuild_branch, render_context, DeriveOptions};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;

// ── Config ──

pub struct ServeConfig {
    pub bind: String,
    pub port: u16,
}

// ── App State ──

struct AppState {
    repo_root: PathBuf,
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

    let state = Arc::new(AppState {
        repo_root: repo_root.to_path_buf(),
    });

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(get_status))
        .route("/api/context", get(get_context))
        .route("/api/decisions", get(get_decisions))
        .route("/api/log", get(get_log))
        .route("/api/drafts", get(get_drafts))
        .route("/api/note", post(post_note))
        .route("/api/decide", post(post_decide))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", config.bind, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("edda HTTP server listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the router (for testing without binding to a port).
pub fn router(repo_root: &Path) -> Router {
    let state = Arc::new(AppState {
        repo_root: repo_root.to_path_buf(),
    });
    Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(get_status))
        .route("/api/context", get(get_context))
        .route("/api/decisions", get(get_decisions))
        .route("/api/log", get(get_log))
        .route("/api/drafts", get(get_drafts))
        .route("/api/note", post(post_note))
        .route("/api/decide", post(post_decide))
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
    };
    let result = edda_ask::ask(&ledger, q, &opts, None)?;
    Ok(Json(result))
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

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

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
}
