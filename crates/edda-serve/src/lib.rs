use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path as AxumPath, Query, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive};
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use edda_ledger::device_token::{generate_device_token, hash_token};
use serde::{Deserialize, Serialize};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{debug, warn};

use edda_aggregate::aggregate::{
    aggregate_decisions, aggregate_overview, per_project_metrics, DateRange, ProjectMetrics,
};
use edda_aggregate::controls::evaluate_controls_rules;
use edda_aggregate::graph::build_dependency_graph;
use edda_aggregate::quality::{model_quality_from_events, QualityReport};
use edda_aggregate::risk::{compute_decision_risks, DecisionInput, DecisionRisk};
use edda_aggregate::rollup;
use edda_core::agent_phase::{mobile_context_summary, AgentPhaseState};
use edda_core::event::{
    finalize_event, new_approval_event, new_decision_event, new_execution_event, new_note_event,
    new_snapshot_event, ApprovalEventParams,
};
use edda_core::policy::{self, ActorKind};
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
    pending_pairings: Mutex<HashMap<String, PairingRequest>>,
    snapshot_cache: Mutex<HashMap<String, VillageSnapshotCacheEntry>>,
}

struct PairingRequest {
    device_name: String,
    expires_at: std::time::Instant,
}

struct VillageSnapshotCacheEntry {
    expires_at: std::time::Instant,
    snapshots: Vec<serde_json::Value>,
}

struct ChronicleContext {
    _store_root: PathBuf,
}

impl AppState {
    fn open_ledger(&self) -> anyhow::Result<Ledger> {
        Ledger::open(&self.repo_root)
    }
}

// ── Error Handling ──

#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("{0}")]
    Validation(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    #[error("{0}")]
    Unauthorized(String),

    #[error("{0}")]
    Internal(#[from] anyhow::Error),
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        Self::Internal(err.into())
    }
}

impl From<serde_yaml::Error> for AppError {
    fn from(err: serde_yaml::Error) -> Self {
        Self::Internal(err.into())
    }
}

impl From<globset::Error> for AppError {
    fn from(err: globset::Error) -> Self {
        Self::Internal(err.into())
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        Self::Internal(err.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::Validation(_) => (StatusCode::BAD_REQUEST, "VALIDATION_ERROR"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "CONFLICT"),
            AppError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        };
        let body = serde_json::json!({
            "error": self.to_string(),
            "code": code,
        });
        (status, Json(body)).into_response()
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
        Some(ChronicleContext {
            _store_root: store_root,
        })
    } else {
        None
    };

    let state = Arc::new(AppState {
        repo_root: repo_root.to_path_buf(),
        chronicle,
        pending_pairings: Mutex::new(HashMap::new()),
        snapshot_cache: Mutex::new(HashMap::new()),
    });

    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/api/health", get(health))
        .route("/pair", get(complete_pairing));

    // Protected routes (auth middleware applied)
    let protected_routes = Router::new()
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
        .route("/api/drafts", get(get_drafts))
        .route("/api/drafts/{id}/approve", post(post_draft_approve))
        .route("/api/drafts/{id}/deny", post(post_draft_deny))
        .route("/api/note", post(post_note))
        .route("/api/decide", post(post_decide))
        .route("/api/events/karvi", post(post_karvi_event))
        .route("/api/scope/check", post(post_scope_check))
        .route("/api/scope/whitelist", get(get_scope_whitelist))
        .route("/api/authz/check", post(post_authz_check))
        .route("/api/approval/check", post(post_approval_check))
        .route("/api/tool-tier/{tool_name}", get(get_tool_tier))
        .route("/api/recap", get(get_recap))
        .route("/api/recap/cached", get(get_recap_cached))
        .route("/api/overview", get(get_overview))
        .route("/api/projects", get(get_projects))
        .route("/api/metrics/quality", get(get_quality_metrics))
        .route("/api/metrics/overview", get(get_metrics_overview))
        .route("/api/metrics/trends", get(get_metrics_trends))
        .route("/api/dashboard", get(get_dashboard))
        .route("/dashboard", get(serve_dashboard))
        .route("/api/actors", get(get_actors))
        .route("/api/actors/{name}", get(get_actor))
        .route("/api/briefs", get(get_briefs))
        .route("/api/briefs/{task_id}", get(get_brief))
        .route("/api/events/stream", get(get_event_stream))
        .route("/api/controls/suggestions", get(get_controls_suggestions))
        .route("/api/controls/patches", get(get_controls_patches))
        .route(
            "/api/controls/patches/{patch_id}/approve",
            post(post_approve_controls_patch),
        )
        .route("/api/snapshot", post(post_snapshot))
        .route("/api/snapshots", get(get_snapshots))
        .route("/api/snapshots/{context_hash}", get(get_snapshots_by_hash))
        .route("/api/pair/new", post(create_pairing))
        .route("/api/pair/list", get(list_paired_devices))
        .route("/api/pair/revoke", post(revoke_device))
        .route("/api/pair/revoke-all", post(revoke_all_devices))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // SECURITY: restrict CORS to localhost origins only. edda is a local
    // development tool; if remote access is needed, consider adding an
    // explicit --cors-origin CLI flag.
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list([
            format!("http://127.0.0.1:{}", config.port)
                .parse()
                .expect("valid localhost origin"),
            format!("http://localhost:{}", config.port)
                .parse()
                .expect("valid localhost origin"),
            format!("http://[::1]:{}", config.port)
                .parse()
                .expect("valid localhost origin"),
        ]))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    let app = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .layer(cors)
        .with_state(state);

    let addr = format!("{}:{}", config.bind, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("edda HTTP server listening on http://{addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Build the router (for testing without binding to a port).
/// Note: no auth middleware is applied here — tests run as localhost.
#[cfg(test)]
fn router(repo_root: &Path) -> Router {
    let store_root = edda_store::store_root();
    let chronicle = if store_root.exists() {
        Some(ChronicleContext {
            _store_root: store_root,
        })
    } else {
        None
    };

    let state = Arc::new(AppState {
        repo_root: repo_root.to_path_buf(),
        chronicle,
        pending_pairings: Mutex::new(HashMap::new()),
        snapshot_cache: Mutex::new(HashMap::new()),
    });
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
        .route("/api/drafts", get(get_drafts))
        .route("/api/drafts/{id}/approve", post(post_draft_approve))
        .route("/api/drafts/{id}/deny", post(post_draft_deny))
        .route("/api/note", post(post_note))
        .route("/api/decide", post(post_decide))
        .route("/api/events/karvi", post(post_karvi_event))
        .route("/api/scope/check", post(post_scope_check))
        .route("/api/scope/whitelist", get(get_scope_whitelist))
        .route("/api/authz/check", post(post_authz_check))
        .route("/api/approval/check", post(post_approval_check))
        .route("/api/tool-tier/{tool_name}", get(get_tool_tier))
        .route("/api/recap", get(get_recap))
        .route("/api/recap/cached", get(get_recap_cached))
        .route("/api/overview", get(get_overview))
        .route("/api/projects", get(get_projects))
        .route("/api/actors", get(get_actors))
        .route("/api/actors/{name}", get(get_actor))
        .route("/api/briefs", get(get_briefs))
        .route("/api/briefs/{task_id}", get(get_brief))
        .route("/api/metrics/quality", get(get_quality_metrics))
        .route("/api/metrics/overview", get(get_metrics_overview))
        .route("/api/metrics/trends", get(get_metrics_trends))
        .route("/api/dashboard", get(get_dashboard))
        .route("/api/sync", post(post_sync))
        .route("/dashboard", get(serve_dashboard))
        .route("/api/events/stream", get(get_event_stream))
        .route("/api/controls/suggestions", get(get_controls_suggestions))
        .route("/api/controls/patches", get(get_controls_patches))
        .route(
            "/api/controls/patches/{patch_id}/approve",
            post(post_approve_controls_patch),
        )
        .route("/api/snapshot", post(post_snapshot))
        .route("/api/snapshots", get(get_snapshots))
        .route("/api/snapshots/{context_hash}", get(get_snapshots_by_hash))
        .route("/pair", get(complete_pairing))
        .route("/api/pair/new", post(create_pairing))
        .route("/api/pair/list", get(list_paired_devices))
        .route("/api/pair/revoke", post(revoke_device))
        .route("/api/pair/revoke-all", post(revoke_all_devices))
        .with_state(state)
}

// ── Auth Middleware ──

/// Check if a socket address is localhost.
fn is_localhost(addr: &SocketAddr) -> bool {
    let ip = addr.ip();
    ip.is_loopback()
        || match ip {
            std::net::IpAddr::V6(v6) => {
                // IPv4-mapped IPv6: ::ffff:127.0.0.1
                if let Some(v4) = v6.to_ipv4_mapped() {
                    v4.is_loopback()
                } else {
                    false
                }
            }
            _ => false,
        }
}

/// Generate a pairing token (random hex, shorter).
fn generate_pairing_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    hex::encode(bytes)
}

/// Auth middleware: localhost passes through, remote needs Bearer token.
async fn auth_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, AppError> {
    // Localhost: always allowed (backward compat)
    if is_localhost(&addr) {
        return Ok(next.run(req).await);
    }

    // Remote: check Authorization header
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    let raw_token = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => {
            return Err(AppError::Unauthorized(
                "missing or invalid Authorization header".to_string(),
            ));
        }
    };

    let token_hash = hash_token(raw_token);
    let ledger = state.open_ledger()?;
    let device = ledger.validate_device_token(&token_hash)?;

    match device {
        Some(_) => Ok(next.run(req).await),
        None => Err(AppError::Unauthorized(
            "invalid or revoked device token".to_string(),
        )),
    }
}

// ── Pairing Endpoints ──

#[derive(Deserialize)]
struct CreatePairingRequest {
    device_name: String,
}

#[derive(Serialize)]
struct CreatePairingResponse {
    pairing_url: String,
    pairing_token: String,
    expires_in_seconds: u64,
}

/// POST /api/pair/new — Create a pairing request (generates one-time pairing token).
async fn create_pairing(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<CreatePairingRequest>, JsonRejection>,
) -> Result<Json<CreatePairingResponse>, AppError> {
    let Json(req) = body.map_err(|e| AppError::Validation(e.to_string()))?;

    if req.device_name.is_empty() {
        return Err(AppError::Validation("device_name is required".to_string()));
    }

    let pairing_token = generate_pairing_token();
    let ttl = Duration::from_secs(600); // 10 minutes

    {
        let mut pairings = state
            .pending_pairings
            .lock()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;

        // Clean up expired pairings
        let now = std::time::Instant::now();
        pairings.retain(|_, v| v.expires_at > now);

        pairings.insert(
            pairing_token.clone(),
            PairingRequest {
                device_name: req.device_name,
                expires_at: now + ttl,
            },
        );
    }

    // Determine host from request headers for URL construction
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:7433");

    let pairing_url = format!("http://{host}/pair?token={pairing_token}");

    Ok(Json(CreatePairingResponse {
        pairing_url,
        pairing_token,
        expires_in_seconds: 600,
    }))
}

#[derive(Deserialize)]
struct CompletePairingQuery {
    token: String,
}

#[derive(Serialize)]
struct CompletePairingResponse {
    device_token: String,
    device_name: String,
}

/// GET /pair?token=<pairing_token> — Complete pairing (the URL the device visits).
async fn complete_pairing(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CompletePairingQuery>,
) -> Result<Json<CompletePairingResponse>, AppError> {
    // Extract and validate the pairing token
    let pairing_req = {
        let mut pairings = state
            .pending_pairings
            .lock()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;

        let now = std::time::Instant::now();
        pairings.retain(|_, v| v.expires_at > now);

        pairings.remove(&query.token)
    };

    let pairing_req = pairing_req
        .ok_or_else(|| AppError::Validation("invalid or expired pairing token".to_string()))?;

    // Generate the long-lived device token
    let device_token = generate_device_token();
    let token_hash = hash_token(&device_token);

    let now = time::OffsetDateTime::now_utc();
    let paired_at = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("time format error: {e}")))?;

    let from_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    let event_id = format!("evt_{}", ulid::Ulid::new());

    // Write device_pair event to ledger
    let ledger = state.open_ledger()?;
    let branch = ledger.head_branch()?;

    let payload = serde_json::json!({
        "device_name": pairing_req.device_name,
        "paired_from_ip": from_ip,
        "token_hash_prefix": &token_hash[..8],
    });

    let parent_hash = ledger.last_event_hash()?;
    let mut event = edda_core::types::Event {
        event_id: event_id.clone(),
        ts: paired_at.clone(),
        event_type: "device_pair".to_string(),
        branch: branch.clone(),
        parent_hash,
        hash: String::new(),
        payload,
        refs: Default::default(),
        schema_version: edda_core::types::SCHEMA_VERSION,
        digests: vec![],
        event_family: Some(edda_core::types::event_family::ADMIN.to_string()),
        event_level: Some(edda_core::types::event_level::INFO.to_string()),
    };

    edda_core::event::finalize_event(&mut event)?;
    ledger.append_event(&event)?;

    // Insert into device_tokens table
    ledger.insert_device_token(&edda_ledger::DeviceTokenRow {
        token_hash,
        device_name: pairing_req.device_name.clone(),
        paired_at,
        paired_from_ip: from_ip,
        revoked_at: None,
        pair_event_id: event_id,
        revoke_event_id: None,
    })?;

    Ok(Json(CompletePairingResponse {
        device_token,
        device_name: pairing_req.device_name,
    }))
}

#[derive(Serialize)]
struct DeviceInfo {
    device_name: String,
    paired_at: String,
    status: String,
    revoked_at: Option<String>,
}

/// GET /api/pair/list — List all paired devices.
async fn list_paired_devices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<DeviceInfo>>, AppError> {
    let ledger = state.open_ledger()?;
    let tokens = ledger.list_device_tokens()?;

    let devices: Vec<DeviceInfo> = tokens
        .into_iter()
        .map(|t| DeviceInfo {
            device_name: t.device_name,
            paired_at: t.paired_at,
            status: if t.revoked_at.is_some() {
                "revoked".to_string()
            } else {
                "active".to_string()
            },
            revoked_at: t.revoked_at,
        })
        .collect();

    Ok(Json(devices))
}

#[derive(Deserialize)]
struct RevokeDeviceRequest {
    device_name: String,
}

/// POST /api/pair/revoke — Revoke a specific device.
async fn revoke_device(
    State(state): State<Arc<AppState>>,
    body: Result<Json<RevokeDeviceRequest>, JsonRejection>,
) -> Result<Json<serde_json::Value>, AppError> {
    let Json(req) = body.map_err(|e| AppError::Validation(e.to_string()))?;

    let ledger = state.open_ledger()?;

    // Check the token exists *before* writing the ledger event
    let existing = ledger.list_device_tokens()?;
    let has_active = existing
        .iter()
        .any(|t| t.device_name == req.device_name && t.revoked_at.is_none());
    if !has_active {
        return Err(AppError::NotFound(format!(
            "no active device token found for '{}'",
            req.device_name
        )));
    }

    let event_id = format!("evt_{}", ulid::Ulid::new());
    let branch = ledger.head_branch()?;

    let now = time::OffsetDateTime::now_utc();
    let ts = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("time format error: {e}")))?;

    let payload = serde_json::json!({
        "device_name": req.device_name,
    });

    let parent_hash = ledger.last_event_hash()?;
    let mut event = edda_core::types::Event {
        event_id: event_id.clone(),
        ts,
        event_type: "device_revoke".to_string(),
        branch: branch.clone(),
        parent_hash,
        hash: String::new(),
        payload,
        refs: Default::default(),
        schema_version: edda_core::types::SCHEMA_VERSION,
        digests: vec![],
        event_family: Some(edda_core::types::event_family::ADMIN.to_string()),
        event_level: Some(edda_core::types::event_level::INFO.to_string()),
    };

    edda_core::event::finalize_event(&mut event)?;
    ledger.append_event(&event)?;
    ledger.revoke_device_token(&req.device_name, &event_id)?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "device_name": req.device_name,
        "event_id": event_id,
    })))
}

/// POST /api/pair/revoke-all — Revoke all active device tokens.
async fn revoke_all_devices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let event_id = format!("evt_{}", ulid::Ulid::new());
    let ledger = state.open_ledger()?;
    let branch = ledger.head_branch()?;

    let now = time::OffsetDateTime::now_utc();
    let ts = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("time format error: {e}")))?;

    let payload = serde_json::json!({ "revoke_all": true });

    let parent_hash = ledger.last_event_hash()?;
    let mut event = edda_core::types::Event {
        event_id: event_id.clone(),
        ts,
        event_type: "device_revoke".to_string(),
        branch: branch.clone(),
        parent_hash,
        hash: String::new(),
        payload,
        refs: Default::default(),
        schema_version: edda_core::types::SCHEMA_VERSION,
        digests: vec![],
        event_family: Some(edda_core::types::event_family::ADMIN.to_string()),
        event_level: Some(edda_core::types::event_level::INFO.to_string()),
    };

    edda_core::event::finalize_event(&mut event)?;
    ledger.append_event(&event)?;

    let count = ledger.revoke_all_device_tokens(&event_id)?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "revoked_count": count,
        "event_id": event_id,
    })))
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
    /// ISO 8601 lower bound (inclusive) for temporal filtering.
    after: Option<String>,
    /// ISO 8601 upper bound (inclusive) for temporal filtering.
    before: Option<String>,
}

/// Validate that a string looks like a valid ISO 8601 / RFC 3339 timestamp.
fn validate_iso8601(s: &str) -> Result<(), String> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .map(|_| ())
        .map_err(|_| format!("invalid ISO 8601 timestamp: {s}"))
}

async fn get_decisions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DecisionsQuery>,
) -> Result<Json<edda_ask::AskResult>, AppError> {
    if let Some(ref after) = params.after {
        validate_iso8601(after).map_err(AppError::Validation)?;
    }
    if let Some(ref before) = params.before {
        validate_iso8601(before).map_err(AppError::Validation)?;
    }

    let ledger = state.open_ledger()?;
    let q = params.q.as_deref().unwrap_or("");
    let opts = edda_ask::AskOptions {
        limit: params.limit.unwrap_or(20),
        include_superseded: params.all.unwrap_or(false),
        branch: params.branch,
        impact: false,
        after: params.after,
        before: params.before,
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
        let q = sub.q.as_deref().or(sub.domain.as_deref()).unwrap_or("");

        let opts = edda_ask::AskOptions {
            limit: sub.limit.unwrap_or(20).min(100),
            include_superseded: sub.all.unwrap_or(false),
            branch: sub.branch.clone(),
            impact: false,
            after: None,
            before: None,
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
        event_id: root.event_id,
        key: root.key,
        value: root.value,
        reason: root.reason,
        relation: None,
        depth: None,
        ts: root.ts.unwrap_or_default(),
        is_active: root.is_active,
    };

    let chain_nodes: Vec<ChainNodeResponse> = chain
        .into_iter()
        .map(|entry| ChainNodeResponse {
            event_id: entry.decision.event_id,
            key: entry.decision.key,
            value: entry.decision.value,
            reason: entry.decision.reason,
            relation: Some(entry.relation),
            depth: Some(entry.depth),
            ts: entry.decision.ts.unwrap_or_default(),
            is_active: entry.decision.is_active,
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

// ── GET /api/drafts ──

#[derive(Serialize)]
struct DraftItem {
    draft_id: String,
    title: String,
    stage_id: String,
    role: String,
    approved: usize,
    min_approvals: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    risk_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    labels: Vec<String>,
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
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    branch: String,
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

    // Load agent phase states for context enrichment
    let phase_states = load_agent_phase_states(&state.repo_root);

    // Load recent decisions/commits for context summary
    let head = ledger.head_branch().unwrap_or_default();
    let recent_decisions = recent_decision_summaries(&ledger, &head, 3);
    let recent_commits = recent_commit_summaries(&ledger, &head, 3);

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

        // Try to find a matching agent phase state (by branch or label)
        let matched_phase = phase_states.iter().find(|ps| {
            ps.branch.as_deref() == Some(&draft.branch) || ps.session_id == draft.draft_id
        });

        let (phase, agent, issue, context_summary) = if let Some(ps) = matched_phase {
            let summary = mobile_context_summary(ps, &recent_decisions, &recent_commits, 200);
            (
                Some(ps.phase.to_string()),
                Some(ps.session_id.clone()),
                ps.issue,
                Some(summary),
            )
        } else {
            (None, None, None, None)
        };

        // Derive risk_level from labels if present
        let risk_level = draft
            .labels
            .iter()
            .find(|l| l.starts_with("risk:") || l.contains("risk"))
            .map(|l| l.strip_prefix("risk:").unwrap_or(l).to_string())
            .or_else(|| {
                if draft.labels.iter().any(|l| l == "high-risk") {
                    Some("high".to_string())
                } else {
                    None
                }
            });

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
                risk_level: risk_level.clone(),
                phase: phase.clone(),
                agent: agent.clone(),
                issue,
                context_summary: context_summary.clone(),
                requested_at: draft.created_at.clone(),
                labels: draft.labels.clone(),
            });
        }
    }

    Ok(Json(DraftsResponse { drafts: items }))
}

/// Load agent phase state files from `.edda/agent-phases/`.
fn load_agent_phase_states(repo_root: &Path) -> Vec<AgentPhaseState> {
    let phases_dir = repo_root.join(".edda").join("agent-phases");
    if !phases_dir.exists() {
        return Vec::new();
    }
    let entries = match std::fs::read_dir(&phases_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut states = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(ps) = serde_json::from_str::<AgentPhaseState>(&content) {
                states.push(ps);
            }
        }
    }
    states
}

/// Fetch recent decision summaries from the ledger for context generation.
fn recent_decision_summaries(ledger: &Ledger, branch: &str, limit: usize) -> Vec<String> {
    let events = ledger
        .iter_events_filtered(branch, Some("decision"), None, None, None, limit)
        .unwrap_or_default();
    events
        .iter()
        .filter_map(|e| {
            let key = e.payload.get("key")?.as_str()?;
            let value = e.payload.get("value")?.as_str()?;
            Some(format!("{key}={value}"))
        })
        .collect()
}

/// Fetch recent commit summaries from the ledger for context generation.
fn recent_commit_summaries(ledger: &Ledger, branch: &str, limit: usize) -> Vec<String> {
    let events = ledger
        .iter_events_filtered(branch, Some("commit"), None, None, None, limit)
        .unwrap_or_default();
    events
        .iter()
        .filter_map(|e| {
            e.payload
                .get("title")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

// ── POST /api/drafts/:id/approve ──

#[derive(Deserialize)]
struct ApproveRequest {
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    stage: Option<String>,
}

#[derive(Serialize)]
struct ApprovalResponse {
    event_id: String,
    draft_status: String,
    stage_status: String,
}

async fn post_draft_approve(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(draft_id): AxumPath<String>,
    body: Result<Json<ApproveRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.body_text()))?;
    handle_draft_action(&state, &headers, &draft_id, "approve", &body).await
}

// ── POST /api/drafts/:id/deny ──

async fn post_draft_deny(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(draft_id): AxumPath<String>,
    body: Result<Json<ApproveRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.body_text()))?;
    handle_draft_action(&state, &headers, &draft_id, "deny", &body).await
}

/// Shared handler for approve/deny actions on drafts.
async fn handle_draft_action(
    state: &AppState,
    headers: &HeaderMap,
    draft_id: &str,
    action: &str,
    body: &ApproveRequest,
) -> Result<Response, AppError> {
    let ledger = state.open_ledger()?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    // Read the draft
    let draft_path = ledger.paths.drafts_dir.join(format!("{draft_id}.json"));
    if !draft_path.exists() {
        return Err(AppError::NotFound(format!("draft not found: {draft_id}")));
    }
    let content = std::fs::read_to_string(&draft_path)?;
    let mut draft: serde_json::Value = serde_json::from_str(&content)?;

    // Check draft status
    let draft_status = draft
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("proposed");
    if draft_status == "applied" || draft_status == "rejected" {
        return Err(AppError::Conflict(format!(
            "draft {draft_id} is already {draft_status}"
        )));
    }

    let actor = body.actor.as_deref().unwrap_or("human");
    let reason = body.reason.as_deref().unwrap_or("");
    let device_id = headers
        .get("x-edda-device-id")
        .and_then(|v| v.to_str().ok());

    let decision = if action == "approve" {
        "approve"
    } else {
        "reject"
    };

    let head = ledger.head_branch()?;

    // Compute draft SHA256
    let draft_sha256 = {
        use sha2::Digest;
        let bytes = std::fs::read(&draft_path)?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        hex::encode(hasher.finalize())
    };

    let parent_hash = ledger.last_event_hash()?;

    // Handle stage-aware drafts
    let stages = draft
        .get("stages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let (stage_id, stage_role, stage_status) = if !stages.is_empty() {
        // Determine which stage to act on
        let requested_stage = body.stage.as_deref();
        let target_stage = if let Some(sid) = requested_stage {
            stages
                .iter()
                .find(|s| s.get("stage_id").and_then(|v| v.as_str()) == Some(sid))
                .ok_or_else(|| AppError::NotFound(format!("stage not found: {sid}")))?
        } else {
            // Auto-select the first pending stage
            stages
                .iter()
                .find(|s| s.get("status").and_then(|v| v.as_str()) == Some("pending"))
                .ok_or_else(|| AppError::Conflict("no pending stages remain".to_string()))?
        };

        let sid = target_stage
            .get("stage_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let role = target_stage
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let st_status = target_stage
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending")
            .to_string();

        if st_status != "pending" {
            return Err(AppError::Conflict(format!(
                "stage '{sid}' is already {st_status}"
            )));
        }

        (sid, role, st_status)
    } else {
        (String::new(), String::new(), "pending".to_string())
    };

    // Replay protection: stage already acted on
    if stage_status != "pending" {
        return Err(AppError::Conflict(format!(
            "draft {draft_id} stage '{}' is already {stage_status}",
            stage_id
        )));
    }

    // Create approval event
    let event = new_approval_event(&ApprovalEventParams {
        branch: &head,
        parent_hash: parent_hash.as_deref(),
        draft_id,
        draft_sha256: &draft_sha256,
        decision,
        actor,
        note: reason,
        stage_id: &stage_id,
        role: &stage_role,
        device_id,
    })?;
    ledger.append_event(&event)?;

    // Update draft JSON
    let ts = event.ts.clone();
    let approval_record = serde_json::json!({
        "ts": ts,
        "actor": actor,
        "decision": decision,
        "note": reason,
        "approval_event_id": event.event_id,
        "stage_id": stage_id,
        "role": stage_role,
    });

    // Append to approvals array
    if let Some(approvals) = draft.get_mut("approvals") {
        if let Some(arr) = approvals.as_array_mut() {
            arr.push(approval_record);
        }
    } else {
        draft["approvals"] = serde_json::json!([approval_record]);
    }

    // Update stage status
    let mut new_stage_status = "pending".to_string();
    if let Some(stages_arr) = draft.get_mut("stages").and_then(|v| v.as_array_mut()) {
        for stage in stages_arr.iter_mut() {
            let sid = stage.get("stage_id").and_then(|v| v.as_str()).unwrap_or("");
            if sid == stage_id {
                if decision == "reject" {
                    stage["status"] = serde_json::Value::String("rejected".to_string());
                    new_stage_status = "rejected".to_string();
                } else {
                    // Read min_approvals first to avoid borrow conflict
                    let min = stage
                        .get("min_approvals")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1) as usize;
                    // Add actor to approved_by
                    if let Some(ab) = stage.get_mut("approved_by") {
                        if let Some(arr) = ab.as_array_mut() {
                            let actor_val = serde_json::Value::String(actor.to_string());
                            if !arr.contains(&actor_val) {
                                arr.push(actor_val);
                            }
                            if arr.len() >= min {
                                new_stage_status = "approved".to_string();
                            }
                        }
                    }
                    if new_stage_status == "approved" {
                        stage["status"] = serde_json::Value::String("approved".to_string());
                    }
                }
                break;
            }
        }

        // Update draft-level status
        let all_approved = stages_arr
            .iter()
            .all(|s| s.get("status").and_then(|v| v.as_str()) == Some("approved"));
        let any_rejected = stages_arr
            .iter()
            .any(|s| s.get("status").and_then(|v| v.as_str()) == Some("rejected"));

        if any_rejected {
            draft["status"] = serde_json::Value::String("rejected".to_string());
        } else if all_approved {
            draft["status"] = serde_json::Value::String("approved".to_string());
        }
    } else {
        // Flat (no stages) draft
        if decision == "reject" {
            draft["status"] = serde_json::Value::String("rejected".to_string());
            new_stage_status = "rejected".to_string();
        } else {
            let min = draft
                .get("policy_min_approvals")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as usize;
            let count = draft
                .get("approvals")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter(|r| r.get("decision").and_then(|v| v.as_str()) == Some("approve"))
                        .count()
                })
                .unwrap_or(0);
            if count >= min.max(1) {
                draft["status"] = serde_json::Value::String("approved".to_string());
                new_stage_status = "approved".to_string();
            }
        }
    }

    let final_draft_status = draft
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("proposed")
        .to_string();

    // Write updated draft
    std::fs::write(&draft_path, serde_json::to_string_pretty(&draft)?)?;

    // Rebuild derived state
    let snap_branch = ledger.head_branch()?;
    let _ = edda_derive::rebuild_branch(&ledger, &snap_branch);

    let resp = ApprovalResponse {
        event_id: event.event_id,
        draft_status: final_draft_status,
        stage_status: new_stage_status,
    };

    Ok((StatusCode::OK, Json(resp)).into_response())
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

// ── POST /api/snapshot ──

#[derive(Deserialize)]
struct SnapshotBody {
    context: serde_json::Value,
    result: serde_json::Value,
    engine_version: String,
    #[serde(default = "default_snapshot_decision_type")]
    decision_type: String,
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

fn default_snapshot_decision_type() -> String {
    "general".to_string()
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
        "decision_type": body.decision_type,
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
        decision_type: body.decision_type,
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
    decision_type: Option<String>,
    #[serde(default = "default_snapshot_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

fn default_snapshot_limit() -> usize {
    20
}

fn snapshots_cache_lookup(
    state: &Arc<AppState>,
    village_id: &str,
) -> Result<Option<Vec<serde_json::Value>>, AppError> {
    let mut cache = state
        .snapshot_cache
        .lock()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;
    let now = std::time::Instant::now();
    cache.retain(|_, v| v.expires_at > now);
    Ok(cache.get(village_id).map(|entry| entry.snapshots.clone()))
}

fn snapshots_cache_store(
    state: &Arc<AppState>,
    village_id: String,
    snapshots: Vec<serde_json::Value>,
) -> Result<(), AppError> {
    let mut cache = state
        .snapshot_cache
        .lock()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;
    let mut snapshots = snapshots;
    snapshots.truncate(100);
    cache.insert(
        village_id,
        VillageSnapshotCacheEntry {
            expires_at: std::time::Instant::now() + Duration::from_secs(300),
            snapshots,
        },
    );
    Ok(())
}

fn snapshots_page_from_cached(
    mut snapshots: Vec<serde_json::Value>,
    query: &SnapshotsQuery,
) -> Vec<serde_json::Value> {
    if let Some(ref engine) = query.engine_version {
        snapshots.retain(|s| {
            s.get("engine_version")
                .and_then(|v| v.as_str())
                .map(|v| v == engine)
                .unwrap_or(false)
        });
    }
    if let Some(ref decision_type) = query.decision_type {
        snapshots.retain(|s| {
            s.get("decision_type")
                .and_then(|v| v.as_str())
                .map(|v| v == decision_type)
                .unwrap_or(false)
        });
    }
    snapshots
        .into_iter()
        .skip(query.offset)
        .take(query.limit)
        .collect()
}

async fn get_snapshots(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SnapshotsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let start = std::time::Instant::now();
    let is_hot_path = query.village_id.is_some() && query.limit <= 100 && query.offset < 100;

    if is_hot_path {
        if let Some(village_id) = query.village_id.as_deref() {
            if let Some(cached_all) = snapshots_cache_lookup(&state, village_id)? {
                let cached = snapshots_page_from_cached(cached_all, &query);
                let elapsed = start.elapsed();
                let elapsed_ms = elapsed.as_millis() as u64;
                if elapsed_ms > 100 {
                    warn!(
                        village_id = query.village_id.as_deref(),
                        engine_version = query.engine_version.as_deref(),
                        decision_type = query.decision_type.as_deref(),
                        limit = query.limit,
                        offset = query.offset,
                        result_count = cached.len(),
                        elapsed_ms = elapsed_ms,
                        cache_hit = true,
                        hot_path = is_hot_path,
                        "get_snapshots request exceeded hot-path latency budget"
                    );
                }
                debug!(
                    village_id = query.village_id.as_deref(),
                    engine_version = query.engine_version.as_deref(),
                    decision_type = query.decision_type.as_deref(),
                    limit = query.limit,
                    offset = query.offset,
                    result_count = cached.len(),
                    elapsed_ms = elapsed_ms,
                    cache_hit = true,
                    hot_path = is_hot_path,
                    "get_snapshots request completed"
                );
                return Ok(Json(cached));
            }
        }
    }

    if is_hot_path {
        if let Some(village_id) = query.village_id.as_deref() {
            let ledger = state.open_ledger()?;
            let rows = ledger.query_snapshots(Some(village_id), None, None, 100, 0)?;

            let mut full_snapshots = Vec::new();
            for row in &rows {
                let snapshot = reconstruct_snapshot(&ledger, row)?;
                full_snapshots.push(snapshot);
            }
            snapshots_cache_store(&state, village_id.to_string(), full_snapshots.clone())?;

            let snapshots = snapshots_page_from_cached(full_snapshots, &query);
            let elapsed = start.elapsed();
            let elapsed_ms = elapsed.as_millis() as u64;
            if elapsed_ms > 100 {
                warn!(
                    village_id = query.village_id.as_deref(),
                    engine_version = query.engine_version.as_deref(),
                    decision_type = query.decision_type.as_deref(),
                    limit = query.limit,
                    offset = query.offset,
                    result_count = snapshots.len(),
                    elapsed_ms = elapsed_ms,
                    cache_hit = false,
                    hot_path = is_hot_path,
                    "get_snapshots request exceeded hot-path latency budget"
                );
            }
            debug!(
                village_id = query.village_id.as_deref(),
                engine_version = query.engine_version.as_deref(),
                decision_type = query.decision_type.as_deref(),
                limit = query.limit,
                offset = query.offset,
                result_count = snapshots.len(),
                elapsed_ms = elapsed_ms,
                cache_hit = false,
                hot_path = is_hot_path,
                "get_snapshots request completed"
            );
            return Ok(Json(snapshots));
        }
    }

    let ledger = state.open_ledger()?;
    let rows = ledger.query_snapshots(
        query.village_id.as_deref(),
        query.engine_version.as_deref(),
        query.decision_type.as_deref(),
        query.limit,
        query.offset,
    )?;

    let mut snapshots = Vec::new();
    for row in &rows {
        let snapshot = reconstruct_snapshot(&ledger, row)?;
        snapshots.push(snapshot);
    }

    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    if elapsed_ms > 100 {
        warn!(
            village_id = query.village_id.as_deref(),
            engine_version = query.engine_version.as_deref(),
            decision_type = query.decision_type.as_deref(),
            limit = query.limit,
            offset = query.offset,
            result_count = snapshots.len(),
            elapsed_ms = elapsed_ms,
            cache_hit = false,
            hot_path = is_hot_path,
            "get_snapshots request exceeded hot-path latency budget"
        );
    }
    debug!(
        village_id = query.village_id.as_deref(),
        engine_version = query.engine_version.as_deref(),
        decision_type = query.decision_type.as_deref(),
        limit = query.limit,
        offset = query.offset,
        result_count = snapshots.len(),
        elapsed_ms = elapsed_ms,
        cache_hit = false,
        hot_path = is_hot_path,
        "get_snapshots request completed"
    );

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

/// Reconstruct a full snapshot JSON from a materialized view row + event payload.
fn reconstruct_snapshot(
    ledger: &Ledger,
    row: &edda_ledger::DecideSnapshotRow,
) -> Result<serde_json::Value, AppError> {
    let event = ledger
        .get_event(&row.event_id)?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("event {} not found", row.event_id)))?;

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
        "decision_type": row.decision_type,
        "schema_version": row.schema_version,
        "redaction_level": row.redaction_level,
        "village_id": row.village_id,
        "cycle_id": row.cycle_id,
        "context": context,
        "result": result,
        "created_at": row.created_at,
    }))
}

// ── GET /api/recap ──

#[derive(Deserialize)]
struct RecapQuery {
    project: Option<String>,
    query: Option<String>,
    #[serde(rename = "since")]
    _since: Option<String>,
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
    let range = DateRange {
        after: Some({
            let now = time::OffsetDateTime::now_utc();
            let from = now - time::Duration::days(7);
            from.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()[..10]
                .to_string()
        }),
        before: None,
    };

    // Compute decisions + risks for attention routing
    let decisions = aggregate_decisions(&projects);
    let now_iso = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let decision_inputs: Vec<DecisionInput> = decisions
        .iter()
        .map(|d| DecisionInput {
            event_id: d.event_id.clone(),
            key: d.key.clone(),
            value: d.value.clone(),
            project: d.project_name.clone(),
            ts: d.ts.clone(),
        })
        .collect();

    // TODO: This event-loading block is duplicated in get_dashboard; extract into a shared helper in a follow-up.
    let mut all_events = Vec::new();
    for entry in &projects {
        let root = std::path::Path::new(&entry.path);
        if let Ok(ledger) = Ledger::open(root) {
            if let Ok(events) = ledger.iter_events() {
                all_events.extend(events);
            }
        }
    }

    let risks = compute_decision_risks(
        &decision_inputs,
        &all_events,
        &now_iso,
        &std::collections::HashSet::new(),
    );

    let response = compute_attention(&risks, &projects, &range, &[], 7);
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

// ── GET /api/controls/suggestions ──

#[derive(Deserialize)]
struct ControlsSuggestionsQuery {
    after: Option<String>,
    before: Option<String>,
    min_samples: Option<u64>,
}

#[derive(Serialize)]
struct ControlsSuggestionsResponse {
    suggestions: Vec<edda_aggregate::controls::ControlsSuggestion>,
    quality: QualityReport,
}

async fn get_controls_suggestions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ControlsSuggestionsQuery>,
) -> Result<Json<ControlsSuggestionsResponse>, AppError> {
    let range = DateRange {
        after: params.after,
        before: params.before,
    };
    let ledger = state.open_ledger()?;
    let events = ledger.iter_events_by_type("execution_event")?;
    let report = model_quality_from_events(&events, &range);

    let rules = edda_bridge_claude::controls_suggest::load_rules();
    let suggestions = evaluate_controls_rules(&rules, &report, params.min_samples);

    Ok(Json(ControlsSuggestionsResponse {
        suggestions,
        quality: report,
    }))
}

// ── GET /api/controls/patches ──

#[derive(Deserialize)]
struct ControlsPatchesQuery {
    status: Option<String>,
}

async fn get_controls_patches(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ControlsPatchesQuery>,
) -> Result<Json<Vec<edda_bridge_claude::controls_suggest::ControlsPatch>>, AppError> {
    let project_id = edda_store::project_id(&state.repo_root);

    let status_filter = match params.status.as_deref() {
        Some("pending") => Some(edda_bridge_claude::controls_suggest::PatchStatus::Pending),
        Some("approved") => Some(edda_bridge_claude::controls_suggest::PatchStatus::Approved),
        Some("dismissed") => Some(edda_bridge_claude::controls_suggest::PatchStatus::Dismissed),
        Some("applied") => Some(edda_bridge_claude::controls_suggest::PatchStatus::Applied),
        Some(s) => {
            return Err(AppError::Validation(format!(
                "Unknown status: {s} (expected: pending, approved, dismissed, applied)"
            )));
        }
        None => None,
    };

    let patches =
        edda_bridge_claude::controls_suggest::list_patches(&project_id, status_filter.as_ref())?;
    Ok(Json(patches))
}

// ── POST /api/controls/patches/{patch_id}/approve ──

#[derive(Deserialize)]
struct ApprovePatchBody {
    #[serde(default = "default_approve_actor")]
    by: String,
}

fn default_approve_actor() -> String {
    "api".to_string()
}

async fn post_approve_controls_patch(
    State(state): State<Arc<AppState>>,
    AxumPath(patch_id): AxumPath<String>,
    body: Result<Json<ApprovePatchBody>, JsonRejection>,
) -> Result<Json<edda_bridge_claude::controls_suggest::ControlsPatch>, AppError> {
    let project_id = edda_store::project_id(&state.repo_root);
    let by = match body {
        Ok(Json(b)) => b.by,
        Err(_) => "api".to_string(),
    };

    let patch = edda_bridge_claude::controls_suggest::approve_patch(&project_id, &patch_id, &by)?;
    Ok(Json(patch))
}

// ── GET /api/metrics/overview ──

fn default_overview_days() -> usize {
    30
}

#[derive(Deserialize)]
struct MetricsOverviewQuery {
    #[serde(default = "default_overview_days")]
    days: usize,
    group: Option<String>,
}

#[derive(Serialize)]
struct MetricsOverviewResponse {
    period: DashboardPeriod,
    projects: Vec<ProjectMetrics>,
    totals: MetricsTotals,
}

#[derive(Serialize)]
struct MetricsTotals {
    total_cost_usd: f64,
    total_events: usize,
    total_commits: usize,
    total_steps: u64,
    overall_success_rate: f64,
}

async fn get_metrics_overview(
    State(state): State<Arc<AppState>>,
    Query(params): Query<MetricsOverviewQuery>,
) -> Result<Json<MetricsOverviewResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(anyhow::anyhow!("chronicle feature not enabled").into());
    }

    let all_projects = list_projects();
    let projects: Vec<_> = if let Some(ref group) = params.group {
        all_projects
            .into_iter()
            .filter(|p| p.group.as_deref() == Some(group.as_str()))
            .collect()
    } else {
        all_projects
    };

    let now = time::OffsetDateTime::now_utc();
    let from_date = now - time::Duration::days(params.days as i64);
    let to_str = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let from_str = from_date
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    let range = DateRange {
        after: Some(from_str[..10].to_string()),
        before: None,
    };

    let metrics = per_project_metrics(&projects, &range, params.days);

    let total_cost: f64 = metrics.iter().map(|m| m.cost.total_usd).sum();
    let total_events: usize = metrics.iter().map(|m| m.activity.events).sum();
    let total_commits: usize = metrics.iter().map(|m| m.activity.commits).sum();
    let total_steps: u64 = metrics.iter().map(|m| m.quality.total_steps).sum();
    let total_success: u64 = metrics
        .iter()
        .map(|m| (m.quality.success_rate * m.quality.total_steps as f64) as u64)
        .sum();

    let period = DashboardPeriod {
        from: from_str[..10].to_string(),
        to: to_str[..10].to_string(),
        days: params.days,
    };

    Ok(Json(MetricsOverviewResponse {
        period,
        projects: metrics,
        totals: MetricsTotals {
            total_cost_usd: total_cost,
            total_events,
            total_commits,
            total_steps,
            overall_success_rate: if total_steps > 0 {
                total_success as f64 / total_steps as f64
            } else {
                0.0
            },
        },
    }))
}

// ── GET /api/metrics/trends ──

fn default_trend_granularity() -> String {
    "daily".to_string()
}

#[derive(Deserialize)]
struct TrendsQuery {
    #[serde(default = "default_overview_days")]
    days: usize,
    #[serde(default = "default_trend_granularity")]
    granularity: String,
    group: Option<String>,
}

#[derive(Serialize)]
struct TrendsResponse {
    granularity: String,
    data: Vec<TrendPoint>,
}

#[derive(Serialize)]
struct TrendPoint {
    date: String,
    events: usize,
    commits: usize,
    cost_usd: f64,
    execution_count: u64,
    success_count: u64,
    success_rate: f64,
}

async fn get_metrics_trends(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TrendsQuery>,
) -> Result<Json<TrendsResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(anyhow::anyhow!("chronicle feature not enabled").into());
    }

    let all_projects = list_projects();
    let projects: Vec<_> = if let Some(ref group) = params.group {
        all_projects
            .into_iter()
            .filter(|p| p.group.as_deref() == Some(group.as_str()))
            .collect()
    } else {
        all_projects
    };

    let now = time::OffsetDateTime::now_utc();
    let from_date = now - time::Duration::days(params.days as i64);
    let from_str = from_date
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    let range = DateRange {
        after: Some(from_str[..10].to_string()),
        before: None,
    };

    let r = rollup::compute_rollup(&projects, &range, "edda");

    let data: Vec<TrendPoint> = match params.granularity.as_str() {
        "weekly" => r
            .weekly
            .iter()
            .map(|w| TrendPoint {
                date: w.week_start.clone(),
                events: w.events,
                commits: w.commits,
                cost_usd: w.cost_usd,
                execution_count: w.execution_count,
                success_count: w.success_count,
                success_rate: if w.execution_count > 0 {
                    w.success_count as f64 / w.execution_count as f64
                } else {
                    0.0
                },
            })
            .collect(),
        "monthly" => r
            .monthly
            .iter()
            .map(|m| TrendPoint {
                date: m.month.clone(),
                events: m.events,
                commits: m.commits,
                cost_usd: m.cost_usd,
                execution_count: m.execution_count,
                success_count: m.success_count,
                success_rate: if m.execution_count > 0 {
                    m.success_count as f64 / m.execution_count as f64
                } else {
                    0.0
                },
            })
            .collect(),
        _ => r
            .daily
            .iter()
            .map(|d| TrendPoint {
                date: d.date.clone(),
                events: d.events,
                commits: d.commits,
                cost_usd: d.cost_usd,
                execution_count: d.execution_count,
                success_count: d.success_count,
                success_rate: if d.execution_count > 0 {
                    d.success_count as f64 / d.execution_count as f64
                } else {
                    0.0
                },
            })
            .collect(),
    };

    Ok(Json(TrendsResponse {
        granularity: params.granularity,
        data,
    }))
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

// SECURITY: `project_id` is caller-supplied and not validated against any
// ACL. Acceptable because edda is a single-user local tool; revisit if
// multi-tenant isolation is ever required.
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

// SECURITY: `project_id` is caller-supplied and not validated against any
// ACL. Acceptable because edda is a single-user local tool; revisit if
// multi-tenant isolation is ever required.
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

// ── GET /api/tool-tier/:tool_name ──

async fn get_tool_tier(
    State(state): State<Arc<AppState>>,
    AxumPath(tool_name): AxumPath<String>,
) -> Result<Json<edda_core::tool_tier::ToolTierResult>, AppError> {
    let edda_dir = state.repo_root.join(".edda");
    let config = edda_core::tool_tier::load_tool_tiers_from_dir(&edda_dir)?;
    let result = edda_core::tool_tier::resolve_tool_tier(&config, &tool_name);
    Ok(Json(result))
}

// ── POST /api/approval/check ──

#[derive(Deserialize)]
struct ApprovalCheckRequest {
    step: String,
    #[serde(default)]
    bundle_id: Option<String>,
    #[serde(default)]
    risk_level: Option<edda_core::bundle::RiskLevel>,
    #[serde(default)]
    files_changed: Option<u32>,
    #[serde(default)]
    tests_failed: Option<u32>,
    #[serde(default)]
    off_limits_touched: Option<bool>,
}

async fn post_approval_check(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ApprovalCheckRequest>,
) -> Result<Json<edda_core::approval::ApprovalDecision>, AppError> {
    let edda_dir = state.repo_root.join(".edda");
    let policy = edda_core::approval::load_approval_policy(&edda_dir)?;

    // Build ReviewBundle from request or from ledger
    let bundle = if let Some(bundle_id) = &body.bundle_id {
        let ledger = Ledger::open(&state.repo_root)?;
        let Some(row) = ledger.get_bundle(bundle_id)? else {
            return Err(AppError::NotFound(format!(
                "Bundle '{}' not found",
                bundle_id
            )));
        };
        let Some(event) = ledger.get_event(&row.event_id)? else {
            return Err(AppError::NotFound(format!(
                "Event for bundle '{}' not found",
                bundle_id
            )));
        };
        serde_json::from_value::<edda_core::bundle::ReviewBundle>(event.payload)?
    } else {
        // Build a synthetic bundle from inline fields
        let risk = body
            .risk_level
            .unwrap_or(edda_core::bundle::RiskLevel::Medium);
        let file_count = body.files_changed.unwrap_or(0) as usize;
        let failed = body.tests_failed.unwrap_or(0);
        let files: Vec<edda_core::bundle::FileChange> = (0..file_count)
            .map(|i| edda_core::bundle::FileChange {
                path: format!("file_{i}"),
                added: 1,
                deleted: 0,
            })
            .collect();
        edda_core::bundle::ReviewBundle {
            bundle_id: "inline".to_string(),
            change_summary: edda_core::bundle::ChangeSummary {
                files,
                total_added: file_count as u32,
                total_deleted: 0,
                diff_ref: "inline".to_string(),
            },
            test_results: edda_core::bundle::TestResults {
                passed: 0,
                failed,
                ignored: 0,
                total: failed,
                failures: vec![],
                command: "inline".to_string(),
            },
            risk_assessment: edda_core::bundle::RiskAssessment {
                level: risk,
                factors: vec![],
            },
            suggested_action: edda_core::bundle::SuggestedAction::Review,
            suggested_reason: "inline check".to_string(),
        }
    };

    let phase_state = edda_core::agent_phase::AgentPhaseState {
        phase: edda_core::agent_phase::AgentPhase::Implement,
        session_id: "api-check".to_string(),
        label: None,
        issue: None,
        pr: None,
        branch: None,
        confidence: 1.0,
        detected_at: String::new(),
        signals: vec![],
    };

    let ctx = edda_core::approval::EvalContext {
        bundle: &bundle,
        phase: &phase_state,
        off_limits_touched: body.off_limits_touched.unwrap_or(false),
        consecutive_failures: 0,
        current_time: Some(time::OffsetDateTime::now_utc()),
    };

    let decision = policy.evaluate(&body.step, &ctx);
    Ok(Json(decision))
}

// ── POST /api/sync ──
// NOTE: sync endpoint is wired only in the test router for now.

#[cfg(test)]
fn sources_from_group(repo_root: &Path) -> Vec<edda_ledger::sync::SyncSource> {
    edda_store::registry::list_group_members(repo_root)
        .into_iter()
        .map(|entry| edda_ledger::sync::SyncSource {
            project_id: entry.project_id,
            project_name: entry.name,
            ledger_path: PathBuf::from(&entry.path),
        })
        .collect()
}

#[cfg(test)]
fn sources_from_name(name: &str) -> Vec<edda_ledger::sync::SyncSource> {
    edda_store::registry::list_projects()
        .into_iter()
        .filter(|p| p.name == name)
        .map(|entry| edda_ledger::sync::SyncSource {
            project_id: entry.project_id,
            project_name: entry.name,
            ledger_path: PathBuf::from(&entry.path),
        })
        .collect()
}

#[cfg(test)]
#[derive(Deserialize)]
struct SyncRequest {
    /// Optional: sync from a specific project name
    from: Option<String>,
    /// Dry run mode
    #[serde(default)]
    dry_run: bool,
}

#[cfg(test)]
#[derive(Serialize)]
struct SyncResponse {
    imported: Vec<SyncImportedEntry>,
    skipped: usize,
    conflicts: Vec<SyncConflictEntry>,
}

#[cfg(test)]
#[derive(Serialize)]
struct SyncImportedEntry {
    key: String,
    value: String,
    source_project: String,
}

#[cfg(test)]
#[derive(Serialize)]
struct SyncConflictEntry {
    key: String,
    local_value: String,
    remote_value: String,
    source_project: String,
}

#[cfg(test)]
async fn post_sync(
    State(state): State<Arc<AppState>>,
    body: Result<Json<SyncRequest>, JsonRejection>,
) -> Result<Json<SyncResponse>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or(SyncRequest {
        from: None,
        dry_run: false,
    });

    let ledger = state.open_ledger()?;

    let sources = if let Some(name) = &body.from {
        sources_from_name(name)
    } else {
        sources_from_group(&state.repo_root)
    };

    let target_project_id = edda_store::project_id(&state.repo_root);
    let result =
        edda_ledger::sync::sync_from_sources(&ledger, &sources, &target_project_id, body.dry_run)?;

    Ok(Json(SyncResponse {
        imported: result
            .imported
            .into_iter()
            .map(|d| SyncImportedEntry {
                key: d.key,
                value: d.value,
                source_project: d.source_project,
            })
            .collect(),
        skipped: result.skipped,
        conflicts: result
            .conflicts
            .into_iter()
            .map(|c| SyncConflictEntry {
                key: c.key,
                local_value: c.local_value,
                remote_value: c.remote_value,
                source_project: c.source_project,
            })
            .collect(),
    }))
}

// ── GET /dashboard (HTML) ──

async fn serve_dashboard() -> impl IntoResponse {
    axum::response::Html(include_str!("../static/dashboard.html"))
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

// ── GET /api/dashboard ──

#[derive(Deserialize)]
struct DashboardQuery {
    #[serde(default = "default_days")]
    days: usize,
}

fn default_days() -> usize {
    7
}

#[derive(Serialize)]
struct DashboardResponse {
    period: DashboardPeriod,
    summary: DashboardSummary,
    attention: OverviewResponse,
    timeline: Vec<TimelineEntry>,
    graph: edda_aggregate::graph::DependencyGraph,
    risks: Vec<DecisionRisk>,
    project_metrics: Vec<ProjectMetrics>,
}

#[derive(Serialize)]
struct DashboardPeriod {
    from: String,
    to: String,
    days: usize,
}

#[derive(Serialize)]
struct DashboardSummary {
    total_projects: usize,
    total_decisions: usize,
    total_events: usize,
    total_commits: usize,
    total_cost_usd: f64,
    overall_success_rate: f64,
}

#[derive(Serialize)]
struct TimelineEntry {
    ts: String,
    event_type: String,
    key: String,
    value: String,
    reason: String,
    project: String,
    risk_level: String,
    supersedes: Option<String>,
}

async fn get_dashboard(
    State(_state): State<Arc<AppState>>,
    Query(params): Query<DashboardQuery>,
) -> Result<Json<DashboardResponse>, AppError> {
    let projects = list_projects();

    let now = time::OffsetDateTime::now_utc();
    let from_date = now - time::Duration::days(params.days as i64);
    let to_str = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let from_str = from_date
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    let range = DateRange {
        after: Some(from_str[..10].to_string()),
        before: None,
    };

    // Summary
    let agg = aggregate_overview(&projects, &range);

    // Decisions + risk scoring
    let decisions = aggregate_decisions(&projects);
    let now_iso = &to_str;

    let decision_inputs: Vec<DecisionInput> = decisions
        .iter()
        .map(|d| DecisionInput {
            event_id: d.event_id.clone(),
            key: d.key.clone(),
            value: d.value.clone(),
            project: d.project_name.clone(),
            ts: d.ts.clone(),
        })
        .collect();

    // Collect all events for risk computation
    // TODO: This event-loading block is duplicated in get_overview; extract into a shared helper in a follow-up.
    let mut all_events = Vec::new();
    for entry in &projects {
        let root = std::path::Path::new(&entry.path);
        if let Ok(ledger) = Ledger::open(root) {
            if let Ok(events) = ledger.iter_events() {
                all_events.extend(events);
            }
        }
    }

    // Cross-project: decision IDs that appear in provenance of events from OTHER projects
    let mut cross_project_ids = std::collections::HashSet::new();
    for entry in &projects {
        let root = std::path::Path::new(&entry.path);
        if let Ok(ledger) = Ledger::open(root) {
            if let Ok(events) = ledger.iter_events() {
                for event in &events {
                    for prov in &event.refs.provenance {
                        // If this event references a decision from another project
                        for d in &decisions {
                            if d.event_id == prov.target && d.project_name != entry.name {
                                cross_project_ids.insert(d.event_id.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    let risks = compute_decision_risks(&decision_inputs, &all_events, now_iso, &cross_project_ids);

    // Build risk lookup for timeline entries
    let risk_map: std::collections::HashMap<&str, &str> = risks
        .iter()
        .map(|r| (r.event_id.as_str(), r.risk_level.as_str()))
        .collect();

    // Timeline: decisions sorted by timestamp descending
    let mut timeline: Vec<TimelineEntry> = decisions
        .iter()
        .map(|d| {
            let risk_level = risk_map
                .get(d.event_id.as_str())
                .unwrap_or(&"low")
                .to_string();
            TimelineEntry {
                ts: d.ts.clone().unwrap_or_default(),
                event_type: "decision".to_string(),
                key: d.key.clone(),
                value: d.value.clone(),
                reason: d.reason.clone(),
                project: d.project_name.clone(),
                risk_level,
                supersedes: None, // Would need provenance walk
            }
        })
        .collect();
    timeline.sort_by(|a, b| b.ts.cmp(&a.ts));

    // Dependency graph
    let graph = build_dependency_graph(&projects);

    // Per-project metrics
    let project_metrics = per_project_metrics(&projects, &range, params.days);

    // Compute cost totals for summary
    let total_cost: f64 = project_metrics.iter().map(|m| m.cost.total_usd).sum();
    let total_steps: u64 = project_metrics.iter().map(|m| m.quality.total_steps).sum();
    let total_success: u64 = project_metrics
        .iter()
        .map(|m| (m.quality.success_rate * m.quality.total_steps as f64) as u64)
        .sum();
    let overall_success_rate = if total_steps > 0 {
        total_success as f64 / total_steps as f64
    } else {
        0.0
    };

    // Attention routing (with cost anomaly detection)
    let attention = compute_attention(&risks, &projects, &range, &project_metrics, params.days);

    let period = DashboardPeriod {
        from: from_str[..10].to_string(),
        to: to_str[..10].to_string(),
        days: params.days,
    };

    let summary = DashboardSummary {
        total_projects: agg.projects.len(),
        total_decisions: agg.total_decisions,
        total_events: agg.total_events,
        total_commits: agg.total_commits,
        total_cost_usd: total_cost,
        overall_success_rate,
    };

    Ok(Json(DashboardResponse {
        period,
        summary,
        attention,
        timeline,
        graph,
        risks,
        project_metrics,
    }))
}

/// Compute attention routing: red / yellow / green classification.
///
/// Includes cost anomaly detection when `project_metrics` is non-empty:
/// - Yellow: project daily cost > 2x period average
/// - Red: project daily cost > 5x period average
fn compute_attention(
    risks: &[DecisionRisk],
    projects: &[edda_store::registry::ProjectEntry],
    range: &DateRange,
    project_metrics: &[ProjectMetrics],
    days: usize,
) -> OverviewResponse {
    let mut red = Vec::new();
    let mut yellow = Vec::new();
    let mut green = Vec::new();

    let now = time::OffsetDateTime::now_utc();
    let updated_at = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string());

    // Red: high-risk decisions
    for r in risks {
        if r.risk_level == "high" {
            red.push(OverviewRedItem {
                project: r.project.clone(),
                summary: format!(
                    "{} = {} (risk {:.0}%)",
                    r.key,
                    r.value,
                    r.risk_score * 100.0
                ),
                action: "Review before overriding".to_string(),
                blocked_count: 0,
            });
        }
    }

    // Yellow: medium-risk decisions
    for r in risks {
        if r.risk_level == "medium" {
            yellow.push(OverviewYellowItem {
                project: r.project.clone(),
                summary: format!(
                    "{} = {} (risk {:.0}%)",
                    r.key,
                    r.value,
                    r.risk_score * 100.0
                ),
                eta: String::new(),
            });
        }
    }

    // Cost anomaly detection
    if days > 0 {
        for pm in project_metrics {
            let daily_avg = pm.cost.daily_avg_usd;
            if daily_avg > 0.0 && pm.cost.last_day_usd > 0.0 {
                // Use the actual most-recent-day cost from rollup data
                let last_day_cost = pm.cost.last_day_usd;
                if last_day_cost > daily_avg * 5.0 {
                    red.push(OverviewRedItem {
                        project: pm.name.clone(),
                        summary: format!(
                            "Cost spike: ${:.2}/day (5x above ${:.2} avg)",
                            last_day_cost, daily_avg
                        ),
                        action: "Investigate cost increase".to_string(),
                        blocked_count: 0,
                    });
                } else if last_day_cost > daily_avg * 2.0 {
                    yellow.push(OverviewYellowItem {
                        project: pm.name.clone(),
                        summary: format!(
                            "Elevated cost: ${:.2}/day (2x above ${:.2} avg)",
                            last_day_cost, daily_avg
                        ),
                        eta: String::new(),
                    });
                }
            }
        }
    }

    // Red: stale projects (no events in range)
    for entry in projects {
        let root = std::path::Path::new(&entry.path);
        let has_events = Ledger::open(root)
            .and_then(|l| l.iter_events())
            .map(|events| events.iter().any(|e| range.matches(&e.ts)))
            .unwrap_or(false);
        if !has_events {
            red.push(OverviewRedItem {
                project: entry.name.clone(),
                summary: "No activity in period".to_string(),
                action: "Check project status".to_string(),
                blocked_count: 0,
            });
        }
    }

    // Green: projects with normal activity
    for entry in projects {
        let root = std::path::Path::new(&entry.path);
        let has_events = Ledger::open(root)
            .and_then(|l| l.iter_events())
            .map(|events| events.iter().any(|e| range.matches(&e.ts)))
            .unwrap_or(false);
        if has_events {
            let high_risk = risks
                .iter()
                .any(|r| r.project == entry.name && r.risk_level == "high");
            if !high_risk {
                green.push(OverviewGreenItem {
                    project: entry.name.clone(),
                    summary: "Normal activity".to_string(),
                });
            }
        }
    }

    OverviewResponse {
        red,
        yellow,
        green,
        updated_at,
    }
}

// ── SSE Event Stream ──

/// Query parameters for the SSE event stream endpoint.
#[derive(Deserialize)]
struct StreamParams {
    /// Comma-separated event types to subscribe to (e.g. "decision,phase_change").
    /// If omitted, all event types are streamed.
    types: Option<String>,
    /// Resume from this event_id (alternative to `Last-Event-ID` header).
    since: Option<String>,
}

/// Map a ledger event to the SSE event name sent to clients.
///
/// Decisions are stored as `note` events with a `decision` key in the payload,
/// so we check the payload in addition to the `event_type` field.
fn sse_event_name(event: &edda_core::Event) -> &'static str {
    match event.event_type.as_str() {
        "agent_phase_change" => "phase_change",
        "approval_request" => "approval_pending",
        "note" if event.payload.get("decision").is_some() => "decision",
        _ => "new_event",
    }
}

/// `GET /api/events/stream` — Server-Sent Events endpoint.
///
/// Streams new ledger events in real time using a poll-based approach
/// (queries SQLite rowid cursor every 2 seconds).
///
/// Supports:
/// - `?types=decision,phase_change` — filter by SSE event type
/// - `?since=evt_xxx` or `Last-Event-ID` header — resume after disconnect
/// - 30-second keep-alive heartbeat
async fn get_event_stream(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StreamParams>,
    headers: HeaderMap,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>>, AppError> {
    // Determine the resume cursor: query param takes precedence over header.
    let since = params.since.or_else(|| {
        headers
            .get("Last-Event-ID")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    });

    // Parse type filter into a set for O(1) lookups.
    let type_filter: Option<Vec<String>> = params.types.map(|t| {
        t.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });

    // Resolve the initial cursor (rowid) from `since` event_id.
    let mut cursor: i64 = if let Some(ref event_id) = since {
        let ledger = state.open_ledger()?;
        ledger.rowid_for_event_id(event_id)?.unwrap_or(0)
    } else {
        0
    };

    let repo_root = state.repo_root.clone();

    let stream = async_stream::stream! {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;

            let ledger = match edda_ledger::Ledger::open(&repo_root) {
                Ok(l) => l,
                Err(_) => continue,
            };

            let new_events = match ledger.events_after_rowid(cursor) {
                Ok(evts) => evts,
                Err(_) => continue,
            };

            if new_events.is_empty() {
                continue;
            }

            // Update cursor to the latest rowid.
            if let Some((last_rowid, _)) = new_events.last() {
                cursor = *last_rowid;
            }

            for (_rowid, event) in new_events {
                let sse_name = sse_event_name(&event);

                // Apply type filter if specified.
                if let Some(ref filters) = type_filter {
                    if !filters.iter().any(|f| f == sse_name) {
                        continue;
                    }
                }

                let event_id = event.event_id.clone();
                let data = serde_json::json!({
                    "event_type": sse_name,
                    "data": serde_json::to_value(&event).unwrap_or_default(),
                    "ts": &event.ts,
                });

                let sse_event = SseEvent::default()
                    .event(sse_name)
                    .id(event_id)
                    .json_data(data)
                    .unwrap_or_else(|_| SseEvent::default().comment("serialization error"));

                yield Ok::<_, Infallible>(sse_event);
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("ping"),
    ))
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

        assert_eq!(resp.status(), StatusCode::CREATED);
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

        assert_eq!(resp.status(), StatusCode::CREATED);
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
        let event = new_note_event(
            "main",
            parent_hash.as_deref(),
            "user",
            "test note",
            &["session".into()],
        )
        .unwrap();
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
        assert_eq!(events[0]["type"], "note");
        let tags = events[0]["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0], "session");
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
    async fn overview_returns_structure() {
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
        assert_eq!(decide_resp.status(), StatusCode::CREATED);

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

    #[tokio::test]
    async fn dashboard_returns_all_sections() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/dashboard")
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
        assert!(json["period"].is_object());
        assert!(json["summary"].is_object());
        assert!(json["attention"].is_object());
        assert!(json["timeline"].is_array());
        assert!(json["graph"].is_object());
        assert!(json["risks"].is_array());
        assert!(json["project_metrics"].is_array());
        // New summary fields
        assert!(
            json["summary"]["total_cost_usd"].is_f64()
                || json["summary"]["total_cost_usd"].is_u64()
        );
        assert!(
            json["summary"]["overall_success_rate"].is_f64()
                || json["summary"]["overall_success_rate"].is_u64()
        );
    }

    #[tokio::test]
    async fn metrics_overview_returns_200() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/metrics/overview")
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
        assert!(json["period"].is_object());
        assert!(json["projects"].is_array());
        assert!(json["totals"].is_object());
    }

    #[tokio::test]
    async fn metrics_trends_returns_200() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/metrics/trends?granularity=daily")
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
        assert_eq!(json["granularity"], "daily");
        assert!(json["data"].is_array());
    }

    #[tokio::test]
    async fn metrics_trends_weekly_granularity() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/metrics/trends?granularity=weekly")
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
        assert_eq!(json["granularity"], "weekly");
    }

    #[tokio::test]
    async fn dashboard_respects_days_param() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/dashboard?days=1")
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
        assert_eq!(json["period"]["days"], 1);
    }

    #[tokio::test]
    async fn dashboard_html_returns_html() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Edda Dashboard"));
        assert!(html.contains("/api/dashboard"));
    }

    // ── Actor endpoint tests ──

    #[tokio::test]
    async fn test_get_actors_empty() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/actors")
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
        let actors = json["actors"].as_array().unwrap();
        assert!(actors.is_empty());
    }

    #[tokio::test]
    async fn test_get_actor_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/actors/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("not found"));
    }

    // ── SSE tests ──

    #[tokio::test]
    async fn test_sse_stream_content_type() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/events/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            ct.contains("text/event-stream"),
            "expected text/event-stream, got: {ct}"
        );
    }

    #[tokio::test]
    async fn test_sse_stream_new_events() {
        use http_body_util::BodyExt;

        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Append an event BEFORE connecting (so it's immediately available).
        let ledger = edda_ledger::Ledger::open(tmp.path()).unwrap();
        let note =
            edda_core::event::new_note_event("main", None, "system", "sse test note", &[]).unwrap();
        ledger.append_event(&note).unwrap();
        drop(ledger);

        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/events/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        // Read the first frame from the SSE stream (with timeout).
        let mut body = resp.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("timed out waiting for SSE frame")
            .expect("stream ended unexpectedly")
            .expect("frame error");

        let data = frame.into_data().expect("expected data frame");
        let text = String::from_utf8(data.to_vec()).unwrap();

        // SSE format: "event: new_event\ndata: ...\nid: evt_...\n\n"
        assert!(text.contains("event: new_event"), "got: {text}");
        assert!(text.contains(&note.event_id), "got: {text}");
    }

    #[tokio::test]
    async fn test_sse_stream_type_filter() {
        use http_body_util::BodyExt;

        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Append a decision event.
        let ledger = edda_ledger::Ledger::open(tmp.path()).unwrap();
        let dp = edda_core::types::DecisionPayload {
            key: "test.key".into(),
            value: "test_val".into(),
            reason: Some("testing".into()),
            scope: None,
        };
        let decision = edda_core::event::new_decision_event("main", None, "system", &dp).unwrap();
        ledger.append_event(&decision).unwrap();

        // Append a note event (should be filtered out).
        let note = edda_core::event::new_note_event(
            "main",
            Some(&decision.hash),
            "system",
            "filtered out",
            &[],
        )
        .unwrap();
        ledger.append_event(&note).unwrap();
        drop(ledger);

        let app = router(tmp.path());

        // Subscribe only to "decision" type.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/events/stream?types=decision")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let mut body = resp.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("timed out waiting for SSE frame")
            .expect("stream ended unexpectedly")
            .expect("frame error");

        let data = frame.into_data().expect("expected data frame");
        let text = String::from_utf8(data.to_vec()).unwrap();

        // Should contain the decision but NOT the note.
        assert!(text.contains("event: decision"), "got: {text}");
        assert!(text.contains(&decision.event_id), "got: {text}");
        assert!(
            !text.contains(&note.event_id),
            "note should be filtered out: {text}"
        );
    }

    #[tokio::test]
    async fn test_sse_stream_since_replay() {
        use http_body_util::BodyExt;

        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Append two events.
        let ledger = edda_ledger::Ledger::open(tmp.path()).unwrap();
        let e1 =
            edda_core::event::new_note_event("main", None, "system", "first event", &[]).unwrap();
        ledger.append_event(&e1).unwrap();

        let e2 =
            edda_core::event::new_note_event("main", Some(&e1.hash), "system", "second event", &[])
                .unwrap();
        ledger.append_event(&e2).unwrap();
        drop(ledger);

        let app = router(tmp.path());

        // Connect with ?since=<e1.event_id>, should only get e2.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/events/stream?since={}", e1.event_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let mut body = resp.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("timed out waiting for SSE frame")
            .expect("stream ended unexpectedly")
            .expect("frame error");

        let data = frame.into_data().expect("expected data frame");
        let text = String::from_utf8(data.to_vec()).unwrap();

        // Should contain e2 but NOT e1.
        assert!(text.contains(&e2.event_id), "expected e2: {text}");
        assert!(!text.contains(&e1.event_id), "e1 should be skipped: {text}");
    }

    #[tokio::test]
    async fn test_sse_stream_last_event_id_header() {
        use http_body_util::BodyExt;

        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let ledger = edda_ledger::Ledger::open(tmp.path()).unwrap();
        let e1 = edda_core::event::new_note_event("main", None, "system", "first", &[]).unwrap();
        ledger.append_event(&e1).unwrap();

        let e2 = edda_core::event::new_note_event("main", Some(&e1.hash), "system", "second", &[])
            .unwrap();
        ledger.append_event(&e2).unwrap();
        drop(ledger);

        let app = router(tmp.path());

        // Use Last-Event-ID header instead of query param.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/events/stream")
                    .header("Last-Event-ID", &e1.event_id)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let mut body = resp.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("timed out waiting for SSE frame")
            .expect("stream ended unexpectedly")
            .expect("frame error");

        let data = frame.into_data().expect("expected data frame");
        let text = String::from_utf8(data.to_vec()).unwrap();

        assert!(text.contains(&e2.event_id), "expected e2: {text}");
        assert!(!text.contains(&e1.event_id), "e1 should be skipped: {text}");
    }

    #[tokio::test]
    async fn post_note_rejects_bad_json() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/note")
                    .header("content-type", "application/json")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "VALIDATION_ERROR");
        assert!(json["error"].as_str().unwrap().len() > 0);
    }

    #[tokio::test]
    async fn post_decide_rejects_bad_json() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from("{invalid"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "VALIDATION_ERROR");
    }

    #[tokio::test]
    async fn post_decide_rejects_missing_equals() {
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
                        serde_json::json!({"decision": "no-equals-sign"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "VALIDATION_ERROR");
        assert!(json["error"].as_str().unwrap().contains("key=value format"));
    }

    #[tokio::test]
    async fn tool_tier_known_tool() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Write a tool_tiers.yaml with a known mapping
        let edda_dir = tmp.path().join(".edda");
        let mut config = edda_core::tool_tier::default_tool_tier_config();
        config
            .tools
            .insert("bash".to_string(), edda_core::tool_tier::ToolTier::T0);
        edda_core::tool_tier::save_tool_tiers_to_dir(&edda_dir, &config).unwrap();

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/tool-tier/bash")
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
        assert_eq!(json["tool"], "bash");
        assert_eq!(json["tier"], "T0");
        assert_eq!(json["approval"], "none");
    }

    #[tokio::test]
    async fn tool_tier_unknown_tool_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/tool-tier/nonexistent")
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
        assert_eq!(json["tool"], "nonexistent");
        assert_eq!(json["tier"], "T1"); // default tier
        assert_eq!(json["approval"], "none");
    }

    /// Helper: write a minimal draft JSON file into the workspace drafts dir.
    fn write_test_draft(dir: &Path, draft_id: &str, status: &str, with_stages: bool) {
        let paths = edda_ledger::EddaPaths::discover(dir);
        std::fs::create_dir_all(&paths.drafts_dir).unwrap();
        let draft = if with_stages {
            serde_json::json!({
                "version": 1,
                "draft_id": draft_id,
                "created_at": "2026-03-01T00:00:00Z",
                "branch": "main",
                "base_parent_hash": "",
                "title": "test draft",
                "purpose": "testing",
                "contribution": "test",
                "labels": ["auth", "risk:medium"],
                "evidence": [],
                "auto_preview_lines": [],
                "event_preview": {},
                "status": status,
                "approvals": [],
                "applied_commit_id": "",
                "policy_require_approval": true,
                "policy_min_approvals": 1,
                "stages": [{
                    "stage_id": "lead",
                    "role": "lead",
                    "min_approvals": 1,
                    "approved_by": [],
                    "status": "pending",
                    "assignees": []
                }],
                "route_rule_id": ""
            })
        } else {
            serde_json::json!({
                "version": 1,
                "draft_id": draft_id,
                "created_at": "2026-03-01T00:00:00Z",
                "branch": "main",
                "base_parent_hash": "",
                "title": "test draft flat",
                "purpose": "testing",
                "contribution": "test",
                "labels": [],
                "evidence": [],
                "auto_preview_lines": [],
                "event_preview": {},
                "status": status,
                "approvals": [],
                "applied_commit_id": "",
                "policy_require_approval": true,
                "policy_min_approvals": 1,
                "stages": [],
                "route_rule_id": ""
            })
        };
        let path = paths.drafts_dir.join(format!("{draft_id}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(&draft).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn get_drafts_enriched_fields() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        write_test_draft(tmp.path(), "drf_enrich", "proposed", true);

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
        let drafts = json["drafts"].as_array().unwrap();
        assert_eq!(drafts.len(), 1);
        let d = &drafts[0];
        assert_eq!(d["draft_id"], "drf_enrich");
        assert_eq!(d["stage_id"], "lead");
        assert_eq!(d["risk_level"], "medium");
        assert_eq!(d["requested_at"], "2026-03-01T00:00:00Z");
        // labels should include "auth" and "risk:medium"
        let labels = d["labels"].as_array().unwrap();
        assert!(labels.contains(&serde_json::json!("auth")));
    }

    #[tokio::test]
    async fn post_draft_approve_creates_event() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        write_test_draft(tmp.path(), "drf_app1", "proposed", true);

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/drafts/drf_app1/approve")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "reason": "LGTM",
                            "actor": "alice",
                            "stage": "lead"
                        })
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
        assert_eq!(json["draft_status"], "approved");
        assert_eq!(json["stage_status"], "approved");
    }

    #[tokio::test]
    async fn post_draft_approve_replay_protection() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        write_test_draft(tmp.path(), "drf_replay", "proposed", true);

        // First approval
        let app1 = router(tmp.path());
        let resp1 = app1
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/drafts/drf_replay/approve")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "reason": "ok",
                            "actor": "alice",
                            "stage": "lead"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);

        // Second approval on same stage should get 409 Conflict
        let app2 = router(tmp.path());
        let resp2 = app2
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/drafts/drf_replay/approve")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "reason": "again",
                            "actor": "bob",
                            "stage": "lead"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn post_draft_deny_creates_event() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        write_test_draft(tmp.path(), "drf_deny1", "proposed", true);

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/drafts/drf_deny1/deny")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "reason": "too risky",
                            "actor": "bob",
                            "stage": "lead"
                        })
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
        assert_eq!(json["draft_status"], "rejected");
        assert_eq!(json["stage_status"], "rejected");
    }

    #[tokio::test]
    async fn post_draft_approve_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/drafts/drf_nonexistent/approve")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "reason": "ok" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn post_draft_approve_with_device_id() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        write_test_draft(tmp.path(), "drf_devid", "proposed", true);

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/drafts/drf_devid/approve")
                    .header("content-type", "application/json")
                    .header("x-edda-device-id", "iphone-14-xyz")
                    .body(Body::from(
                        serde_json::json!({
                            "reason": "approved from phone",
                            "actor": "alice",
                            "stage": "lead"
                        })
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

        // Verify the event in the ledger has device_id
        let ledger = edda_ledger::Ledger::open(tmp.path()).unwrap();
        let events = ledger
            .iter_events_filtered("main", Some("approval"), None, None, None, 1)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["device_id"], "iphone-14-xyz");
    }

    #[test]
    fn cost_anomaly_detection_yellow_and_red() {
        use edda_aggregate::aggregate::{
            ActivityMetrics, CostMetrics, ProjectMetrics, QualityMetrics,
        };

        let range = DateRange {
            after: Some("2026-03-01".to_string()),
            before: Some("2026-03-08".to_string()),
        };

        // Project with 6x spike on last day → should be red
        let red_project = ProjectMetrics {
            project_id: "proj-red".to_string(),
            name: "red-spike".to_string(),
            group: None,
            activity: ActivityMetrics {
                events: 10,
                commits: 2,
                decisions: 0,
                sessions: 1,
            },
            cost: CostMetrics {
                total_usd: 0.70,
                daily_avg_usd: 0.10,
                last_day_usd: 0.60, // 6x the daily avg
                by_model: vec![],
            },
            quality: QualityMetrics {
                success_rate: 1.0,
                avg_latency_ms: 0.0,
                total_steps: 10,
            },
        };

        // Project with 3x spike on last day → should be yellow
        let yellow_project = ProjectMetrics {
            project_id: "proj-yellow".to_string(),
            name: "yellow-spike".to_string(),
            group: None,
            activity: ActivityMetrics {
                events: 10,
                commits: 2,
                decisions: 0,
                sessions: 1,
            },
            cost: CostMetrics {
                total_usd: 0.40,
                daily_avg_usd: 0.10,
                last_day_usd: 0.30, // 3x the daily avg
                by_model: vec![],
            },
            quality: QualityMetrics {
                success_rate: 1.0,
                avg_latency_ms: 0.0,
                total_steps: 10,
            },
        };

        // Project with normal cost → should not trigger
        let normal_project = ProjectMetrics {
            project_id: "proj-normal".to_string(),
            name: "normal".to_string(),
            group: None,
            activity: ActivityMetrics {
                events: 10,
                commits: 2,
                decisions: 0,
                sessions: 1,
            },
            cost: CostMetrics {
                total_usd: 0.70,
                daily_avg_usd: 0.10,
                last_day_usd: 0.10, // exactly average
                by_model: vec![],
            },
            quality: QualityMetrics {
                success_rate: 1.0,
                avg_latency_ms: 0.0,
                total_steps: 10,
            },
        };

        let metrics = vec![red_project, yellow_project, normal_project];
        let result = compute_attention(&[], &[], &range, &metrics, 7);

        // Red should contain the 6x spike project
        let red_names: Vec<&str> = result.red.iter().map(|r| r.project.as_str()).collect();
        assert!(
            red_names.contains(&"red-spike"),
            "Expected red-spike in red items, got: {red_names:?}"
        );

        // Yellow should contain the 3x spike project
        let yellow_names: Vec<&str> = result.yellow.iter().map(|y| y.project.as_str()).collect();
        assert!(
            yellow_names.contains(&"yellow-spike"),
            "Expected yellow-spike in yellow items, got: {yellow_names:?}"
        );

        // Normal project should not appear in red or yellow
        assert!(
            !red_names.contains(&"normal"),
            "Normal project should not be in red"
        );
        assert!(
            !yellow_names.contains(&"normal"),
            "Normal project should not be in yellow"
        );
    }

    // ── DecideSnapshot endpoint tests ──

    fn snapshot_json() -> serde_json::Value {
        serde_json::json!({
            "engine_version": "claude-3.5",
            "decision_type": "chief",
            "village_id": "blog-village",
            "context_hash": "abc123def456",
            "context": {"files": ["main.rs"], "prompt": "test"},
            "result": {"decisions": [{"key": "db.engine", "value": "sqlite"}]},
        })
    }

    #[tokio::test]
    async fn post_snapshot_returns_201() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/snapshot")
                    .header("content-type", "application/json")
                    .body(Body::from(snapshot_json().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["event_id"].as_str().unwrap().starts_with("evt_"));
        assert_eq!(json["context_hash"], "abc123def456");
    }

    #[tokio::test]
    async fn post_snapshot_rejects_empty_engine_version() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let body = serde_json::json!({
            "engine_version": "",
            "context_hash": "abc123",
            "context": {},
            "result": {},
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/snapshot")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_snapshot_rejects_empty_context_hash() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let body = serde_json::json!({
            "engine_version": "claude-3.5",
            "context_hash": "",
            "context": {},
            "result": {},
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/snapshot")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_snapshot_rejects_missing_fields() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let body = serde_json::json!({"engine_version": "claude-3.5"});

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/snapshot")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_snapshots_returns_posted() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // POST a snapshot
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/snapshot")
                    .header("content-type", "application/json")
                    .body(Body::from(snapshot_json().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // GET /api/snapshots
        let app2 = router(tmp.path());
        let resp2 = app2
            .oneshot(
                Request::builder()
                    .uri("/api/snapshots")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp2.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["context_hash"], "abc123def456");
        assert_eq!(json[0]["engine_version"], "claude-3.5");
    }

    #[tokio::test]
    async fn get_snapshots_by_context_hash() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // POST a snapshot
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/snapshot")
                    .header("content-type", "application/json")
                    .body(Body::from(snapshot_json().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // GET /api/snapshots/:context_hash
        let app2 = router(tmp.path());
        let resp2 = app2
            .oneshot(
                Request::builder()
                    .uri("/api/snapshots/abc123def456")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp2.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["context_hash"], "abc123def456");
    }

    #[tokio::test]
    async fn get_snapshots_by_hash_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/snapshots/nonexistent_hash")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_snapshots_filters_decision_type_and_offset() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Seed snapshots with mixed decision_type and same village
        let app = router(tmp.path());
        let body1 = serde_json::json!({
            "engine_version": "claude-3.5",
            "decision_type": "chief",
            "village_id": "blog-village",
            "context_hash": "ctx-chief-1",
            "context": {"files": ["a.rs"]},
            "result": {"ok": true}
        });
        let body2 = serde_json::json!({
            "engine_version": "claude-3.5",
            "decision_type": "chief",
            "village_id": "blog-village",
            "context_hash": "ctx-chief-2",
            "context": {"files": ["b.rs"]},
            "result": {"ok": true}
        });
        let body3 = serde_json::json!({
            "engine_version": "claude-3.5",
            "decision_type": "general",
            "village_id": "blog-village",
            "context_hash": "ctx-general-1",
            "context": {"files": ["c.rs"]},
            "result": {"ok": true}
        });

        for body in [body1, body2, body3] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/snapshot")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
        }

        // Filter by decision_type=chief, limit=1, offset=1
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/snapshots?village_id=blog-village&decision_type=chief&limit=1&offset=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["decision_type"], "chief");
    }

    #[tokio::test]
    async fn get_snapshots_hot_path_uses_village_cache_window() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let app = router(tmp.path());
        for i in 0..120 {
            let decision_type = if i % 2 == 0 { "chief" } else { "general" };
            let body = serde_json::json!({
                "engine_version": "claude-3.5",
                "decision_type": decision_type,
                "village_id": "blog-village",
                "context_hash": format!("ctx-{i}"),
                "context": {"files": ["a.rs"]},
                "result": {"ok": true}
            });
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/snapshot")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
        }

        // First query populates village cache with top-100 snapshot window.
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/snapshots?village_id=blog-village&decision_type=chief&limit=20&offset=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        // Second query should be served from the cached window and respect pagination.
        let second = app
            .oneshot(
                Request::builder()
                    .uri("/api/snapshots?village_id=blog-village&decision_type=chief&limit=20&offset=40")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json.len(), 10);
        assert!(json.iter().all(|v| v["decision_type"] == "chief"));
    }

    // ── POST /api/decisions/batch tests ──

    #[tokio::test]
    async fn batch_returns_multiple_results() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Seed a decision so queries can find something
        let ledger = Ledger::open(tmp.path()).unwrap();
        let parent_hash = ledger.last_event_hash().unwrap();
        let dp = DecisionPayload {
            key: "db.engine".into(),
            value: "sqlite".into(),
            reason: Some("embedded".into()),
            scope: None,
        };
        let event = new_decision_event("main", parent_hash.as_deref(), "user", &dp).unwrap();
        ledger.append_event(&event).unwrap();
        drop(ledger);

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decisions/batch")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "queries": [
                                { "q": "db.engine" },
                                { "q": "nonexistent_keyword_xyz", "limit": 5 }
                            ]
                        })
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
        let results = json["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["query_index"], 0);
        assert_eq!(results[1]["query_index"], 1);
        // First query should have decisions
        assert!(results[0]["decisions"].is_array());
        // Second query should also succeed (just empty)
        assert!(results[1]["decisions"].is_array());
    }

    #[tokio::test]
    async fn batch_slim_omits_extra_fields() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decisions/batch")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "queries": [{ "q": "anything" }],
                            "slim": true
                        })
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
        let result = &json["results"][0];
        assert!(result["decisions"].is_array());
        // slim mode should omit timeline, related_commits, related_notes, conversations
        assert!(result.get("timeline").is_none());
        assert!(result.get("related_commits").is_none());
        assert!(result.get("related_notes").is_none());
        assert!(result.get("conversations").is_none());
    }

    #[tokio::test]
    async fn batch_rejects_over_10_queries() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let queries: Vec<serde_json::Value> = (0..11)
            .map(|i| serde_json::json!({ "q": format!("q{i}") }))
            .collect();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decisions/batch")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "queries": queries }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn batch_rejects_empty_queries() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decisions/batch")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::json!({ "queries": [] }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn batch_domain_as_query() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Seed a decision with a domain
        let ledger = Ledger::open(tmp.path()).unwrap();
        let parent_hash = ledger.last_event_hash().unwrap();
        let dp = DecisionPayload {
            key: "blog-village.cache".into(),
            value: "redis".into(),
            reason: Some("fast".into()),
            scope: None,
        };
        let event = new_decision_event("main", parent_hash.as_deref(), "user", &dp).unwrap();
        ledger.append_event(&event).unwrap();
        drop(ledger);

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decisions/batch")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "queries": [
                                { "domain": "blog-village" }
                            ]
                        })
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
        let results = json["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["query_index"], 0);
        assert!(results[0]["decisions"].is_array());
    }

    // ── Causal chain endpoint tests ─────────────────────────────────

    #[tokio::test]
    async fn chain_endpoint_empty_chain() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = Router::new().merge(router(tmp.path()));

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
        assert_eq!(decide_resp.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(decide_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let event_id = json["event_id"].as_str().unwrap();

        let chain_resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/decisions/{}/chain", event_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(chain_resp.status(), StatusCode::OK);

        let chain_body = axum::body::to_bytes(chain_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let chain_json: serde_json::Value = serde_json::from_slice(&chain_body).unwrap();

        assert_eq!(chain_json["root"]["key"], "db.engine");
        assert_eq!(chain_json["root"]["value"], "postgres");
        assert!(chain_json["chain"].as_array().unwrap().is_empty());
        assert_eq!(chain_json["meta"]["total_nodes"], 1);
    }

    #[tokio::test]
    async fn chain_endpoint_404_for_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = Router::new().merge(router(tmp.path()));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/decisions/evt_nonexistent/chain")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn chain_endpoint_returns_chain_with_deps() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = Router::new().merge(router(tmp.path()));

        let resp_a = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"decision": "db.engine=postgres", "reason": "root"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_a = axum::body::to_bytes(resp_a.into_body(), usize::MAX)
            .await
            .unwrap();
        let json_a: serde_json::Value = serde_json::from_slice(&body_a).unwrap();
        let event_id_a = json_a["event_id"].as_str().unwrap().to_string();

        let _resp_b = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"decision": "db.pool=10", "reason": "pool config"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        {
            let ledger = Ledger::open(tmp.path()).unwrap();
            ledger
                .insert_dep("db.pool", "db.engine", "explicit", None)
                .unwrap();
        }

        let chain_resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/decisions/{}/chain?depth=3", event_id_a))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(chain_resp.status(), StatusCode::OK);

        let chain_body = axum::body::to_bytes(chain_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let chain_json: serde_json::Value = serde_json::from_slice(&chain_body).unwrap();

        assert_eq!(chain_json["root"]["key"], "db.engine");
        let chain_arr = chain_json["chain"].as_array().unwrap();
        assert_eq!(chain_arr.len(), 1);
        assert_eq!(chain_arr[0]["key"], "db.pool");
        assert_eq!(chain_arr[0]["relation"], "depends_on");
        assert_eq!(chain_arr[0]["depth"], 1);
        assert_eq!(chain_json["meta"]["total_nodes"], 2);
        assert_eq!(chain_json["meta"]["max_depth"], 3);
    }

    #[tokio::test]
    async fn chain_endpoint_depth_param() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = Router::new().merge(router(tmp.path()));

        let resp_a = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"decision": "db.engine=postgres", "reason": "root"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_a = axum::body::to_bytes(resp_a.into_body(), usize::MAX)
            .await
            .unwrap();
        let json_a: serde_json::Value = serde_json::from_slice(&body_a).unwrap();
        let event_id_a = json_a["event_id"].as_str().unwrap().to_string();

        let _resp_b = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"decision": "db.pool=10", "reason": "pool"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        let _resp_c = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"decision": "db.timeout=30", "reason": "timeout"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        {
            let ledger = Ledger::open(tmp.path()).unwrap();
            ledger
                .insert_dep("db.pool", "db.engine", "explicit", None)
                .unwrap();
            ledger
                .insert_dep("db.timeout", "db.pool", "explicit", None)
                .unwrap();
        }

        let chain_resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/decisions/{}/chain?depth=1", event_id_a))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(chain_resp.status(), StatusCode::OK);

        let chain_body = axum::body::to_bytes(chain_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let chain_json: serde_json::Value = serde_json::from_slice(&chain_body).unwrap();

        let chain_arr = chain_json["chain"].as_array().unwrap();
        assert_eq!(chain_arr.len(), 1);
        assert_eq!(chain_arr[0]["key"], "db.pool");
        assert_eq!(chain_json["meta"]["max_depth"], 1);
    }

    // ── Auth Middleware Tests ──────────────────────────────────────

    /// Build a production-like app with auth middleware for testing.
    fn app_with_auth(repo_root: &Path) -> Router {
        let store_root = edda_store::store_root();
        let chronicle = if store_root.exists() {
            Some(ChronicleContext {
                _store_root: store_root,
            })
        } else {
            None
        };

        let state = Arc::new(AppState {
            repo_root: repo_root.to_path_buf(),
            chronicle,
            pending_pairings: Mutex::new(HashMap::new()),
            snapshot_cache: Mutex::new(HashMap::new()),
        });

        let public_routes = Router::new().route("/api/health", get(health));

        let protected_routes = Router::new().route("/api/status", get(get_status)).layer(
            middleware::from_fn_with_state(state.clone(), auth_middleware),
        );

        Router::new()
            .merge(public_routes)
            .merge(protected_routes)
            .layer(CorsLayer::permissive())
            .with_state(state)
    }

    /// Insert a dummy event into the ledger and return its event_id.
    /// Needed because device_tokens.pair_event_id has a FK to events.
    fn seed_dummy_event(ledger: &Ledger) -> String {
        use edda_core::event::new_note_event;
        let event = new_note_event("main", None, "system", "seed event", &[]).unwrap();
        let event_id = event.event_id.clone();
        ledger.append_event(&event).unwrap();
        event_id
    }

    /// Build a request from a remote (non-localhost) IP.
    fn remote_request(uri: &str) -> Request<Body> {
        let mut req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 1], 12345))));
        req
    }

    /// Build a request from a remote IP with an Authorization header.
    fn remote_request_with_auth(uri: &str, token: &str) -> Request<Body> {
        let mut req = Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 1], 12345))));
        req
    }

    /// Build a request from a localhost IP.
    fn localhost_request(uri: &str) -> Request<Body> {
        let mut req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        req
    }

    #[tokio::test]
    async fn auth_localhost_bypasses_middleware() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = app_with_auth(tmp.path());

        let resp = app.oneshot(localhost_request("/api/status")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_missing_header_returns_401() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = app_with_auth(tmp.path());

        let resp = app.oneshot(remote_request("/api/status")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "UNAUTHORIZED");
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("missing or invalid"));
    }

    #[tokio::test]
    async fn auth_malformed_header_returns_401() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = app_with_auth(tmp.path());

        // "Token" instead of "Bearer"
        let mut req = Request::builder()
            .uri("/api/status")
            .header("authorization", "Token some_value")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 1], 12345))));

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("missing or invalid"));
    }

    #[tokio::test]
    async fn auth_invalid_token_returns_401() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = app_with_auth(tmp.path());

        let resp = app
            .oneshot(remote_request_with_auth("/api/status", "bad_token_value"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("invalid or revoked"));
    }

    #[tokio::test]
    async fn auth_revoked_token_returns_401() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Seed a device token that is already revoked
        let raw_token = generate_device_token();
        let token_hash = hash_token(&raw_token);
        let ledger = Ledger::open(tmp.path()).unwrap();
        let pair_evt = seed_dummy_event(&ledger);
        let revoke_evt = seed_dummy_event(&ledger);
        ledger
            .insert_device_token(&edda_ledger::sqlite_store::DeviceTokenRow {
                token_hash: token_hash.clone(),
                device_name: "test-device".to_string(),
                paired_at: "2026-01-01T00:00:00Z".to_string(),
                paired_from_ip: "203.0.113.1".to_string(),
                revoked_at: Some("2026-01-02T00:00:00Z".to_string()),
                pair_event_id: pair_evt,
                revoke_event_id: Some(revoke_evt),
            })
            .unwrap();
        drop(ledger);

        let app = app_with_auth(tmp.path());
        let resp = app
            .oneshot(remote_request_with_auth("/api/status", &raw_token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_valid_token_passes() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Seed a valid (non-revoked) device token
        let raw_token = generate_device_token();
        let token_hash = hash_token(&raw_token);
        let ledger = Ledger::open(tmp.path()).unwrap();
        let pair_evt = seed_dummy_event(&ledger);
        ledger
            .insert_device_token(&edda_ledger::sqlite_store::DeviceTokenRow {
                token_hash,
                device_name: "test-device".to_string(),
                paired_at: "2026-01-01T00:00:00Z".to_string(),
                paired_from_ip: "203.0.113.1".to_string(),
                revoked_at: None,
                pair_event_id: pair_evt,
                revoke_event_id: None,
            })
            .unwrap();
        drop(ledger);

        let app = app_with_auth(tmp.path());
        let resp = app
            .oneshot(remote_request_with_auth("/api/status", &raw_token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_public_route_no_auth_needed() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = app_with_auth(tmp.path());

        // /api/health is a public route — should work without auth from remote IP
        let resp = app.oneshot(remote_request("/api/health")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_ipv6_localhost_bypasses() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = app_with_auth(tmp.path());

        let mut req = Request::builder()
            .uri("/api/status")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(SocketAddr::from((
            [0, 0, 0, 0, 0, 0, 0, 1],
            12345,
        ))));

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
