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
mod tests;
