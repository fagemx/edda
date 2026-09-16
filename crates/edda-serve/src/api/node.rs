//! Node transport endpoints: `POST /api/sync` and `GET /api/node/status`.
//!
//! These routes exist **only** when `edda node start` wired a node config
//! (`ServeConfig::node`). `edda serve` alone never exposes them.
//!
//! Auth is a shared bearer token. The check runs **before** the body is read:
//! a missing, empty or wrong token is a 401 and writes nothing. A token absent
//! from both the request and the server config is a 401, not an open door.
//! Tokens are compared in constant time and never logged.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::error::AppError;
use crate::AppState;

/// Whole request body bound (frozen contract §3).
const SYNC_MAX_BODY: usize = 4 * 1024 * 1024;
/// Events per request bound (frozen contract §3).
const SYNC_MAX_EVENTS: usize = 512;
const SYNC_KIND: &str = "edda.node.sync";
const SYNC_RESULT_KIND: &str = "edda.node.sync.result";

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/sync", post(post_sync))
        .route("/api/node/status", get(get_status))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncRequest {
    version: u32,
    kind: String,
    origin_machine: String,
    events: Vec<serde_json::Value>,
}

/// Constant-time byte comparison. Length inequality returns early (a length is
/// not the secret); equal lengths are compared without an early exit.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

/// Authorize the request against the configured shared token. Runs before the
/// body is read.
fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let expected = state
        .node_token
        .as_deref()
        .filter(|value| !value.is_empty());
    match (provided, expected) {
        (Some(provided), Some(expected))
            if constant_time_eq(provided.as_bytes(), expected.as_bytes()) =>
        {
            Ok(())
        }
        _ => Err(AppError::Unauthorized(
            "missing or invalid node token".into(),
        )),
    }
}

fn node_config(state: &AppState) -> Result<&edda_ledger::node::NodeConfig, AppError> {
    state
        .node
        .as_ref()
        .ok_or_else(|| AppError::NotFound("node transport is not running".into()))
}

async fn post_sync(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    // The token is checked before anything else, including reading the body.
    authorize(&state, request.headers())?;

    let bytes = axum::body::to_bytes(request.into_body(), SYNC_MAX_BODY)
        .await
        .map_err(|error| {
            AppError::Validation(format!("request body too large or unreadable: {error}"))
        })?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::Validation(format!("invalid JSON body: {error}")))?;
    let body: SyncRequest = serde_json::from_value(value)
        .map_err(|error| AppError::Validation(format!("invalid sync request: {error}")))?;

    if body.version != 1 {
        return Err(AppError::Validation(format!(
            "unsupported sync version {}: expected 1",
            body.version
        )));
    }
    if body.kind != SYNC_KIND {
        return Err(AppError::Validation(format!(
            "unexpected sync kind '{}': expected {SYNC_KIND}",
            body.kind
        )));
    }
    edda_ledger::node::validate_machine_label(&body.origin_machine)
        .map_err(|error| AppError::Validation(format!("originMachine: {error}")))?;
    if body.events.is_empty() || body.events.len() > SYNC_MAX_EVENTS {
        return Err(AppError::Validation(format!(
            "events must contain 1..={SYNC_MAX_EVENTS} entries"
        )));
    }

    let machine = node_config(&state)?.node.alias.clone();

    // Parse every event; a refused event is reported per-index and writes
    // nothing. Accepted events are imported by the idempotent engine.
    let mut refused: Vec<serde_json::Value> = Vec::new();
    let mut valid = Vec::new();
    for (index, raw) in body.events.iter().enumerate() {
        match edda_ledger::node::parse_event(raw) {
            Ok(event) => valid.push((index, event)),
            Err(refusal) => refused.push(serde_json::json!({
                "index": index,
                "reason": refusal.reason,
            })),
        }
    }

    let events: Vec<edda_ledger::node::NodeEvent> =
        valid.iter().map(|(_, event)| event.clone()).collect();
    let mut outcome =
        edda_ledger::node::import_batch(&state.repo_root, &events).map_err(AppError::Internal)?;

    // Re-anchor import refusals to their original request index.
    for refusal in outcome.refused {
        let index = valid
            .get(refusal.index)
            .map(|(index, _)| *index)
            .unwrap_or(refusal.index);
        refused.push(serde_json::json!({
            "index": index,
            "reason": refusal.reason,
        }));
    }

    // Land the `lane_request`s `import_batch` handed back (GH-685): write the
    // local `request` coord event plus the `request_delivered` marker into this
    // node's own coordination log, then record the event ids as imported.
    //
    // A `lane_request` whose origin is this machine's own alias is a loop and is
    // refused rather than landed (the local path never needs the wire). A crash
    // between the receipt and the landing leaves the event un-imported, so the
    // sender resends; `write_remote_request` refuses an already-landed id and
    // `request_delivered_exists` keeps the marker from being doubled.
    let project_id = edda_store::project_id(&state.repo_root);
    let node_session = format!("node-{machine}");
    let mut landed_ids: Vec<String> = Vec::new();
    for (request, event_id) in outcome
        .lane_requests
        .iter()
        .zip(outcome.lane_request_ids.iter())
    {
        let index = valid
            .iter()
            .find(|(_, event)| &event.event_id == event_id)
            .map(|(index, _)| *index)
            .unwrap_or(0);
        if request.origin_machine == machine {
            refused.push(serde_json::json!({
                "index": index,
                "reason": format!(
                    "lane_request '{}' carries this machine's own alias '{machine}'; refused as a loop",
                    request.request_id
                ),
            }));
            continue;
        }
        if !edda_bridge_claude::peers::request_exists(&project_id, &request.request_id) {
            if let Err(error) = edda_bridge_claude::peers::write_remote_request(
                &project_id,
                &node_session,
                &request.request_id,
                &request.from_label,
                &request.to_label,
                &request.message,
                &request.origin_machine,
            ) {
                refused.push(serde_json::json!({
                    "index": index,
                    "reason": format!(
                        "lane_request '{}' could not be landed: {error}",
                        request.request_id
                    ),
                }));
                continue;
            }
        }
        if !edda_bridge_claude::peers::request_delivered_exists(&project_id, &request.request_id) {
            if let Err(error) = edda_bridge_claude::peers::write_request_delivered(
                &project_id,
                &request.request_id,
                &request.to_label,
                &request.origin_machine,
                event_id,
            ) {
                refused.push(serde_json::json!({
                    "index": index,
                    "reason": format!(
                        "lane_request '{}' could not record delivery: {error}",
                        request.request_id
                    ),
                }));
                continue;
            }
        }
        landed_ids.push(event_id.clone());
    }
    edda_ledger::node::mark_imported(&state.repo_root, &landed_ids).map_err(AppError::Internal)?;
    outcome.accepted.extend(landed_ids);

    Ok(Json(serde_json::json!({
        "version": 1,
        "kind": SYNC_RESULT_KIND,
        "machine": machine,
        "accepted": outcome.accepted,
        "duplicates": outcome.duplicates,
        "refused": refused,
    })))
}

async fn get_status(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
) -> Result<Response, AppError> {
    authorize(&state, request.headers())?;
    let config = node_config(&state)?.clone();

    let queue: Vec<serde_json::Value> = config
        .peers
        .iter()
        .map(|peer| {
            let status = edda_ledger::node::peer_queue_status(&peer.alias).ok();
            match status {
                Some(status) => serde_json::json!({
                    "peer": peer.alias,
                    "pending": status.pending,
                    "sent": status.sent,
                    "delivered": status.delivered,
                    "acked": status.acked,
                    "lastSuccessAt": status.last_success_at,
                }),
                None => serde_json::json!({
                    "peer": peer.alias,
                    "pending": null,
                    "sent": null,
                    "delivered": null,
                    "acked": null,
                    "lastSuccessAt": null,
                    "error": "queue unreadable",
                }),
            }
        })
        .collect();

    let observations = edda_ledger::node::peer_observations().unwrap_or_default();
    let peers: Vec<serde_json::Value> = config
        .peers
        .iter()
        .map(|peer| {
            let observed = observations.get(&peer.alias);
            // Absence is never healthy-zero: an unobserved peer reports
            // reachable:false with an explicit reason.
            let reachable = observed
                .and_then(|value| value.get("reachable"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let reason = observed
                .and_then(|value| value.get("reason"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no observation recorded");
            let last_seen_at = observed
                .and_then(|value| value.get("lastSeenAt"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "alias": peer.alias,
                "host": peer.host,
                "port": peer.port,
                "reachable": reachable,
                "reason": reason,
                "lastSeenAt": last_seen_at,
            })
        })
        .collect();

    // The revision is the **edda** revision this process can observe, with its
    // origin named. `none` pairs with `revision: null`; a different artifact's
    // identifier (a Pi package release id, a session id, a path) is never
    // substituted for it (contract §7).
    let (revision, revision_origin) = edda_ledger::node::local_revision_origin(&state.repo_root);
    let body = serde_json::json!({
        "machine": config.node.alias,
        "bind": config.node.bind,
        "port": config.node.port,
        "revision": revision,
        "revisionOrigin": revision_origin.as_str(),
        "queue": queue,
        "peers": peers,
    });
    Ok(Json(body).into_response())
}
