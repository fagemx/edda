mod api;
mod error;
mod helpers;
mod middleware;
mod state;

pub use state::ServeConfig;
pub(crate) use state::{AppState, ChronicleContext};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::middleware as axum_mw;
use axum::Router;
use tower_http::cors::{AllowOrigin, CorsLayer};

#[cfg(test)]
use crate::error::AppError;

#[cfg(test)]
use axum::extract::rejection::JsonRejection;
#[cfg(test)]
use axum::extract::State;
#[cfg(test)]
use axum::Json;
#[cfg(test)]
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::path::PathBuf;

// ── Entrypoint ──

pub async fn serve(repo_root: &Path, config: ServeConfig) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("not an edda workspace (run `edda init` first)");
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
    });

    // Public routes (no auth required)
    let public_routes = api::auth::public_routes().merge(api::events::public_routes());

    // Protected routes (auth middleware applied)
    let protected_routes = api::events::protected_routes()
        .merge(api::drafts::routes())
        .merge(api::telemetry::routes())
        .merge(api::snapshots::routes())
        .merge(api::analytics::routes())
        .merge(api::metrics::routes())
        .merge(api::dashboard::routes())
        .merge(api::policy::routes())
        .merge(api::briefs::routes())
        .merge(api::stream::routes())
        .merge(api::ingestion::routes())
        .merge(api::auth::protected_routes())
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            middleware::auth_middleware,
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
    });
    api::events::routes()
        .merge(api::drafts::routes())
        .merge(api::telemetry::routes())
        .merge(api::snapshots::routes())
        .merge(api::analytics::routes())
        .merge(api::metrics::routes())
        .merge(api::dashboard::routes())
        .merge(api::policy::routes())
        .merge(api::briefs::routes())
        .merge(api::stream::routes())
        .merge(api::ingestion::routes())
        .merge(api::auth::routes())
        .merge(sync_routes())
        .with_state(state)
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

#[cfg(test)]
fn sync_routes() -> Router<Arc<AppState>> {
    use axum::routing::post;
    Router::new().route("/api/sync", post(post_sync))
}

// ── Tests ──

#[cfg(test)]
#[allow(clippy::await_holding_lock, clippy::len_zero)]
mod tests {
    use super::*;
    use crate::api::dashboard::compute_attention;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use edda_aggregate::aggregate::DateRange;
    use edda_aggregate::quality::QualityReport;
    use edda_core::event::{new_decision_event, new_note_event};
    use edda_core::types::DecisionPayload;
    use edda_ledger::device_token::{generate_device_token, hash_token};
    use edda_ledger::Ledger;
    use std::time::Duration;
    use tower::ServiceExt;

    /// Serialize tests that set EDDA_STORE_ROOT env var.
    static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that removes EDDA_STORE_ROOT on drop (panic-safe cleanup).
    struct StoreRootGuard;
    impl Drop for StoreRootGuard {
        fn drop(&mut self) {
            std::env::remove_var("EDDA_STORE_ROOT");
        }
    }

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
        let _lock = STORE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store_dir = tmp.path().join("store");
        std::fs::create_dir_all(&store_dir).unwrap();
        std::env::set_var("EDDA_STORE_ROOT", &store_dir);
        let _guard = StoreRootGuard;
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

    // ── Karvi harvest integration tests (GH-342) ──

    #[tokio::test]
    async fn post_note_with_karvi_tags() {
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
                        serde_json::json!({
                            "text": "[GH-598] spawn fix pattern",
                            "role": "system",
                            "tags": ["auto-harvest", "lesson"]
                        })
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
        let event_id = json["event_id"].as_str().unwrap();
        assert!(event_id.starts_with("evt_"));

        // Verify tags and role persisted in ledger
        let ledger = Ledger::open(tmp.path()).unwrap();
        let event = ledger.get_event(event_id).unwrap().unwrap();
        assert_eq!(event.payload["role"], "system");
        let tags = event.payload["tags"].as_array().unwrap();
        assert!(tags.contains(&serde_json::json!("auto-harvest")));
        assert!(tags.contains(&serde_json::json!("lesson")));
    }

    #[tokio::test]
    async fn post_decide_auto_supersedes_same_key() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = Router::new().merge(router(tmp.path()));

        // First decide
        let resp1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "decision": "runtime.spawn=sh",
                            "reason": "initial"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::CREATED);
        let body1 = axum::body::to_bytes(resp1.into_body(), usize::MAX)
            .await
            .unwrap();
        let json1: serde_json::Value = serde_json::from_slice(&body1).unwrap();
        let first_id = json1["event_id"].as_str().unwrap().to_string();
        assert!(json1.get("superseded").is_none() || json1["superseded"].is_null());

        // Second decide with same key, different value
        let resp2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "decision": "runtime.spawn=cmd.exe",
                            "reason": "verified in #598"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::CREATED);
        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let json2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["superseded"].as_str().unwrap(), first_id);

        // Verify only the second decision is active
        let ledger = Ledger::open(tmp.path()).unwrap();
        let active = ledger
            .find_active_decision("main", "runtime.spawn")
            .unwrap()
            .unwrap();
        assert_eq!(active.value, "cmd.exe");
    }

    #[tokio::test]
    async fn karvi_harvest_full_smoke() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = Router::new().merge(router(tmp.path()));

        // 1. POST decide
        let decide_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "decision": "test.harvest=works",
                            "reason": "integration test"
                        })
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
        let decide_id = decide_json["event_id"].as_str().unwrap().to_string();

        // 2. POST note
        let note_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/note")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "text": "test harvest note",
                            "role": "system",
                            "tags": ["auto-harvest", "test"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(note_resp.status(), StatusCode::CREATED);
        let note_body = axum::body::to_bytes(note_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let note_json: serde_json::Value = serde_json::from_slice(&note_body).unwrap();
        let note_id = note_json["event_id"].as_str().unwrap().to_string();

        // 3. POST karvi event
        let karvi_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/events/karvi")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        karvi_event_json("karvi-smoke-001", "step_completed").to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(karvi_resp.status(), StatusCode::CREATED);

        // Verify all three events exist in ledger
        let ledger = Ledger::open(tmp.path()).unwrap();
        assert!(ledger.get_event(&decide_id).unwrap().is_some());
        assert!(ledger.get_event(&note_id).unwrap().is_some());
        assert!(ledger.get_event("karvi-smoke-001").unwrap().is_some());
    }

    #[tokio::test]
    async fn karvi_harvest_decide_queryback() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = Router::new().merge(router(tmp.path()));

        // POST decide
        let decide_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "decision": "runtime.spawn=cmd.exe",
                            "reason": "verified in #598"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(decide_resp.status(), StatusCode::CREATED);

        // GET /api/decisions?q=runtime
        let query_resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/decisions?q=runtime")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(query_resp.status(), StatusCode::OK);
        let query_body = axum::body::to_bytes(query_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let query_json: serde_json::Value = serde_json::from_slice(&query_body).unwrap();

        let decisions = query_json["decisions"].as_array().unwrap();
        assert!(
            decisions
                .iter()
                .any(|d| d["key"] == "runtime.spawn" && d["value"] == "cmd.exe"),
            "expected runtime.spawn=cmd.exe in decisions: {decisions:?}"
        );
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
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
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
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
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
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
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

    #[tokio::test]
    async fn decisions_supports_context_summary_param() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let ledger = Ledger::open(tmp.path()).unwrap();
        let parent_hash = ledger.last_event_hash().unwrap();
        let dp = DecisionPayload {
            key: "pricing.discount_policy".into(),
            value: "daytime_revenue_shield".into(),
            reason: Some("avoid aggressive daytime markdowns".into()),
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let event = new_decision_event("main", parent_hash.as_deref(), "user", &dp).unwrap();
        ledger.append_event(&event).unwrap();
        drop(ledger);

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/decisions?context_summary=daytime%20discount%20outcome")
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
        assert!(json["decisions"].is_array());
        assert_eq!(json["decisions"][0]["key"], "pricing.discount_policy");
    }

    #[tokio::test]
    async fn batch_supports_context_summary_query() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let ledger = Ledger::open(tmp.path()).unwrap();
        let parent_hash = ledger.last_event_hash().unwrap();
        let dp = DecisionPayload {
            key: "pricing.discount_policy".into(),
            value: "daytime_revenue_shield".into(),
            reason: Some("avoid aggressive daytime markdowns".into()),
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
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
                                { "context_summary": "daytime discount outcome" }
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
        assert!(results[0]["decisions"].is_array());
        assert_eq!(results[0]["decisions"][0]["key"], "pricing.discount_policy");
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
        });

        let public_routes = api::events::public_routes();

        let protected_routes = api::events::protected_routes().layer(axum_mw::from_fn_with_state(
            state.clone(),
            middleware::auth_middleware,
        ));

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
            .insert_device_token(&edda_ledger::DeviceTokenRow {
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
            .insert_device_token(&edda_ledger::DeviceTokenRow {
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

    // ── Telemetry endpoint tests ──

    fn sample_telemetry_body(cycle_id: &str) -> serde_json::Value {
        serde_json::json!({
            "cycle_id": cycle_id,
            "source": "thyra",
            "started_at": "2026-03-27T10:00:00Z",
            "total_duration_ms": 5000,
            "operations": [
                {
                    "name": "probe",
                    "duration_ms": 2000,
                    "token_usage": { "input_tokens": 1000, "output_tokens": 500 },
                    "status": "ok"
                },
                {
                    "name": "evaluate",
                    "duration_ms": 3000,
                    "status": "ok"
                }
            ],
            "cost": {
                "total_usd": 0.05
            },
            "tags": ["governance"]
        })
    }

    #[tokio::test]
    async fn post_telemetry_creates_event() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/telemetry")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_telemetry_body("cycle_001").to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["event_id"].as_str().unwrap(), "cycle_001");
        assert_eq!(json["status"].as_str().unwrap(), "created");
    }

    #[tokio::test]
    async fn post_telemetry_deduplicates() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // First POST
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/telemetry")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_telemetry_body("cycle_dup").to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Second POST with same cycle_id
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/telemetry")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_telemetry_body("cycle_dup").to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"].as_str().unwrap(), "duplicate");
    }

    #[tokio::test]
    async fn post_telemetry_rejects_bad_json() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/telemetry")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"not": "valid telemetry"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_telemetry_returns_events() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // POST two telemetry events
        let app = router(tmp.path());
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/telemetry")
                .header("content-type", "application/json")
                .body(Body::from(sample_telemetry_body("cycle_get_1").to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

        let app = router(tmp.path());
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/telemetry")
                .header("content-type", "application/json")
                .body(Body::from(sample_telemetry_body("cycle_get_2").to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

        // GET telemetry
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/telemetry")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 2);
    }

    #[tokio::test]
    async fn get_telemetry_stats_computes_averages() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // POST 3 telemetry events with different durations
        for (id, dur) in [("cyc_s1", 1000u64), ("cyc_s2", 2000), ("cyc_s3", 3000)] {
            let body = serde_json::json!({
                "cycle_id": id,
                "source": "thyra",
                "started_at": "2026-03-27T10:00:00Z",
                "total_duration_ms": dur,
                "operations": [
                    { "name": "probe", "duration_ms": dur / 2, "status": "ok" },
                    { "name": "evaluate", "duration_ms": dur / 2, "status": "ok" }
                ],
                "cost": { "total_usd": 0.01 }
            });
            let app = router(tmp.path());
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/telemetry")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        }

        // GET stats
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/telemetry/stats?days=30")
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

        assert_eq!(json["cycle_count"].as_u64().unwrap(), 3);
        assert!((json["avg_duration_ms"].as_f64().unwrap() - 2000.0).abs() < 0.1);
        assert_eq!(json["p95_duration_ms"].as_f64().unwrap(), 3000.0);
        assert!((json["total_cost_usd"].as_f64().unwrap() - 0.03).abs() < 0.001);
        assert_eq!(json["error_rate"].as_f64().unwrap(), 0.0);

        let ops = json["slowest_operations"].as_array().unwrap();
        assert_eq!(ops.len(), 2);
    }

    // ── Pattern Detection Endpoint Tests ──

    #[tokio::test]
    async fn get_patterns_returns_recurring() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Seed 4 decisions with same key in village "v-test"
        let ledger = Ledger::open(tmp.path()).unwrap();
        let mut prev_hash = ledger.last_event_hash().unwrap();
        for i in 0..4 {
            let dp = edda_core::types::DecisionPayload {
                key: "rewards.cap".to_string(),
                value: format!("{}", 100 + i),
                reason: Some("adjusting".to_string()),
                scope: None,
                authority: Some("event_chief".to_string()),
                affected_paths: None,
                tags: None,
                review_after: None,
                reversibility: None,
                village_id: Some("v-test".to_string()),
            };
            let event =
                edda_core::event::new_decision_event("main", prev_hash.as_deref(), "system", &dp)
                    .unwrap();
            prev_hash = Some(event.hash.clone());
            ledger.append_event(&event).unwrap();
        }
        drop(ledger);

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/patterns?village_id=v-test&lookback_days=30&min_occurrences=3")
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

        assert_eq!(json["village_id"].as_str().unwrap(), "v-test");
        assert_eq!(json["lookback_days"].as_u64().unwrap(), 30);
        assert!(json["after"].as_str().is_some());
        assert!(json["total_patterns"].as_u64().unwrap() >= 1);

        let patterns = json["patterns"].as_array().unwrap();
        let recurring: Vec<_> = patterns
            .iter()
            .filter(|p| p["pattern_type"].as_str() == Some("recurring_decision"))
            .collect();
        assert!(!recurring.is_empty());
        assert_eq!(recurring[0]["key"].as_str().unwrap(), "rewards.cap");
        assert_eq!(recurring[0]["occurrences"].as_u64().unwrap(), 4);
    }

    #[tokio::test]
    async fn get_patterns_missing_village_id_returns_400() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/patterns")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── Ingestion integration tests ──

    #[tokio::test]
    async fn ingestion_auto_ingest_via_evaluate() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // POST /api/ingestion/evaluate with auto-ingest trigger
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ingestion/evaluate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "eventType": "decision.commit",
                            "sourceLayer": "L1",
                            "summary": "Formal decision committed",
                            "detail": {"session": "ds_test"}
                        })
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
        assert_eq!(json["action"], "ingested");
        assert!(json["recordId"].as_str().unwrap().starts_with("prec_"));

        // GET /api/ingestion/records — verify it was written
        let app2 = router(tmp.path());
        let resp2 = app2
            .oneshot(
                Request::builder()
                    .uri("/api/ingestion/records")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp2.status(), StatusCode::OK);
        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let records: Vec<serde_json::Value> = serde_json::from_slice(&body2).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["eventType"], "decision.commit");
        assert_eq!(records[0]["triggerType"], "auto");
    }

    #[tokio::test]
    async fn ingestion_suggest_then_accept() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // POST /api/ingestion/evaluate with suggest trigger
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ingestion/evaluate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "eventType": "route.changed",
                            "sourceLayer": "L1",
                            "summary": "Route changed in session",
                            "detail": {"path": "/api/foo"}
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
        assert_eq!(json["action"], "queued");
        let sug_id = json["suggestionId"].as_str().unwrap().to_string();
        assert!(sug_id.starts_with("sug_"));
        assert!(json["reason"].as_str().is_some());

        // GET /api/ingestion/suggestions — 1 pending
        let app2 = router(tmp.path());
        let resp2 = app2
            .oneshot(
                Request::builder()
                    .uri("/api/ingestion/suggestions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp2.status(), StatusCode::OK);
        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let suggestions: Vec<serde_json::Value> = serde_json::from_slice(&body2).unwrap();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0]["id"], sug_id);

        // POST /api/ingestion/suggestions/{id}/accept
        let app3 = router(tmp.path());
        let resp3 = app3
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/ingestion/suggestions/{sug_id}/accept"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp3.status(), StatusCode::OK);
        let body3 = axum::body::to_bytes(resp3.into_body(), usize::MAX)
            .await
            .unwrap();
        let record: serde_json::Value = serde_json::from_slice(&body3).unwrap();
        assert!(record["id"].as_str().unwrap().starts_with("prec_"));
        assert_eq!(record["triggerType"], "suggested");

        // GET /api/ingestion/suggestions — empty after accept
        let app4 = router(tmp.path());
        let resp4 = app4
            .oneshot(
                Request::builder()
                    .uri("/api/ingestion/suggestions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body4 = axum::body::to_bytes(resp4.into_body(), usize::MAX)
            .await
            .unwrap();
        let suggestions4: Vec<serde_json::Value> = serde_json::from_slice(&body4).unwrap();
        assert!(suggestions4.is_empty());

        // GET /api/ingestion/records — 1 record with trigger_type=suggested
        let app5 = router(tmp.path());
        let resp5 = app5
            .oneshot(
                Request::builder()
                    .uri("/api/ingestion/records")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body5 = axum::body::to_bytes(resp5.into_body(), usize::MAX)
            .await
            .unwrap();
        let records: Vec<serde_json::Value> = serde_json::from_slice(&body5).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["triggerType"], "suggested");
    }

    #[tokio::test]
    async fn ingestion_never_ingest_skips() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // POST /api/ingestion/evaluate with never-ingest trigger
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ingestion/evaluate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "eventType": "followup.draft",
                            "sourceLayer": "L1"
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
        assert_eq!(json["action"], "skipped");

        // GET /api/ingestion/records — empty
        let app2 = router(tmp.path());
        let resp2 = app2
            .oneshot(
                Request::builder()
                    .uri("/api/ingestion/records")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let records: Vec<serde_json::Value> = serde_json::from_slice(&body2).unwrap();
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn ingestion_manual_record() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // POST /api/ingestion/records (manual)
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ingestion/records")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "eventType": "custom.event",
                            "sourceLayer": "L1",
                            "summary": "Manual ingestion test",
                            "detail": {"key": "value"}
                        })
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
        assert!(json["recordId"].as_str().unwrap().starts_with("prec_"));

        // GET /api/ingestion/records — 1 record with trigger_type=manual
        let app2 = router(tmp.path());
        let resp2 = app2
            .oneshot(
                Request::builder()
                    .uri("/api/ingestion/records")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let records: Vec<serde_json::Value> = serde_json::from_slice(&body2).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["triggerType"], "manual");
    }

    #[tokio::test]
    async fn ingestion_accept_nonexistent_404() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ingestion/suggestions/sug_fake/accept")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn ingestion_reject_nonexistent_404() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ingestion/suggestions/sug_fake/reject")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn ingestion_suggest_then_reject() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Queue a suggestion
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ingestion/evaluate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "eventType": "route.changed",
                            "sourceLayer": "L1",
                            "summary": "Route change to reject",
                            "detail": {}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let sug_id = json["suggestionId"].as_str().unwrap().to_string();

        // Reject it
        let app2 = router(tmp.path());
        let resp2 = app2
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/ingestion/suggestions/{sug_id}/reject"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp2.status(), StatusCode::OK);
        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let json2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["ok"], true);

        // GET /api/ingestion/records — empty (rejected, not written)
        let app3 = router(tmp.path());
        let resp3 = app3
            .oneshot(
                Request::builder()
                    .uri("/api/ingestion/records")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body3 = axum::body::to_bytes(resp3.into_body(), usize::MAX)
            .await
            .unwrap();
        let records: Vec<serde_json::Value> = serde_json::from_slice(&body3).unwrap();
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn ingestion_evaluate_invalid_layer_400() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ingestion/evaluate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "eventType": "decision.commit",
                            "sourceLayer": "L99"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn ingestion_accept_already_accepted_409() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Queue a suggestion
        let app = router(tmp.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ingestion/evaluate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "eventType": "route.changed",
                            "sourceLayer": "L1",
                            "summary": "Route change for double-accept test",
                            "detail": {}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let sug_id = json["suggestionId"].as_str().unwrap().to_string();

        // Accept it (first time — should succeed)
        let app2 = router(tmp.path());
        let resp2 = app2
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/ingestion/suggestions/{sug_id}/accept"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);

        // Accept again — should get 409 Conflict
        let app3 = router(tmp.path());
        let resp3 = app3
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/ingestion/suggestions/{sug_id}/accept"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp3.status(), StatusCode::CONFLICT);
    }

    // ── Error differentiation tests (GH-379) ──

    /// Build a router without chronicle context (for testing 501 responses).
    fn router_no_chronicle(repo_root: &Path) -> Router {
        let state = Arc::new(AppState {
            repo_root: repo_root.to_path_buf(),
            chronicle: None,
            pending_pairings: Mutex::new(HashMap::new()),
        });
        api::events::routes()
            .merge(api::drafts::routes())
            .merge(api::telemetry::routes())
            .merge(api::snapshots::routes())
            .merge(api::analytics::routes())
            .merge(api::metrics::routes())
            .merge(api::dashboard::routes())
            .merge(api::policy::routes())
            .merge(api::briefs::routes())
            .merge(api::stream::routes())
            .merge(api::ingestion::routes())
            .merge(api::auth::routes())
            .with_state(state)
    }

    #[test]
    fn classify_open_error_not_edda_workspace() {
        use crate::error::classify_open_error;
        let err = anyhow::anyhow!("not an edda workspace (run `edda init` first)");
        match classify_open_error(err) {
            crate::error::AppError::NotFound(_) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn classify_open_error_database_locked() {
        use crate::error::classify_open_error;
        let err = anyhow::anyhow!("database is locked");
        match classify_open_error(err) {
            crate::error::AppError::ServiceUnavailable(_) => {}
            other => panic!("expected ServiceUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn classify_open_error_unknown_becomes_internal() {
        use crate::error::classify_open_error;
        let err = anyhow::anyhow!("some random error");
        match classify_open_error(err) {
            crate::error::AppError::Internal(_) => {}
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn status_returns_404_for_non_edda_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        // Do NOT call setup_workspace — leave it as a bare directory
        let app = router_no_chronicle(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
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
        assert_eq!(json["code"], "NOT_FOUND");
    }

    #[tokio::test]
    async fn recap_returns_501_without_chronicle() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let app = router_no_chronicle(tmp.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/recap")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "NOT_IMPLEMENTED");
    }

    #[tokio::test]
    async fn service_unavailable_includes_retry_after_header() {
        use crate::error::AppError;
        use axum::response::IntoResponse;

        let err = AppError::ServiceUnavailable("database is temporarily unavailable".into());
        let resp = err.into_response();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            resp.headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok()),
            Some("1")
        );
    }
}
