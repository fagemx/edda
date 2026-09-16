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

// ── Entrypoint ──

pub async fn serve(repo_root: &Path, config: ServeConfig) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("not an edda workspace (run `edda init` first)");
    }

    // Node bind guard (frozen contract §8): with the node transport enabled the
    // listener may only bind this machine's Tailscale 100.x IPv4 address. The
    // explicit test-only flag is the one way past it, and it says so on stderr.
    if let Some(node) = config.node.as_ref() {
        if !config.insecure_bind && !edda_ledger::node::is_tailnet_ipv4(&config.bind) {
            anyhow::bail!(
                "refusing to bind '{}': the node transport only binds this machine's Tailscale \
                 100.x IPv4 address; pass --insecure-bind only for a local test rig",
                config.bind
            );
        }
        // The same Tailscale policy applies to every configured peer host, so
        // a direct `serve()` caller cannot bypass the boundary by handing in a
        // LAN host while `bind` is a valid tailnet address (FU-1).
        edda_ledger::node::validate_config_with_bind_policy(node, config.insecure_bind)
            .map_err(|error| anyhow::anyhow!("invalid node config: {error:#}"))?;
        if config.insecure_bind {
            eprintln!(
                "warning: --insecure-bind is set: binding '{}', which is not a Tailscale 100.x \
                 address. This is for local test rigs only and must never be used for the real \
                 two-machine proof.",
                config.bind
            );
        }
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
        node: config.node.clone(),
        node_token: config.node_token.clone(),
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

    let mut app = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .layer(cors);

    // The node routes exist only when the node transport is running.
    if config.node.is_some() {
        app = app.merge(api::node::routes());
    }

    let app = app.with_state(state);

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
        node: None,
        node_token: None,
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

// ── Tests ──

#[cfg(test)]
#[allow(clippy::await_holding_lock, clippy::len_zero)]
mod tests;

#[cfg(test)]
mod node_tests;
