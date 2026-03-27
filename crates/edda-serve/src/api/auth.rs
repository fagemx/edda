use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::middleware::generate_pairing_token;
use crate::state::{AppState, PairingRequest};

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
    let device_token = edda_ledger::device_token::generate_device_token();
    let token_hash = edda_ledger::device_token::hash_token(&device_token);

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

/// Public auth routes (no auth required).
pub(crate) fn public_routes() -> Router<Arc<AppState>> {
    Router::new().route("/pair", get(complete_pairing))
}

/// Protected auth routes (auth middleware applied).
pub(crate) fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/pair/new", post(create_pairing))
        .route("/api/pair/list", get(list_paired_devices))
        .route("/api/pair/revoke", post(revoke_device))
        .route("/api/pair/revoke-all", post(revoke_all_devices))
}

/// All auth routes (for test router without auth middleware).
#[cfg(test)]
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/pair", get(complete_pairing))
        .route("/api/pair/new", post(create_pairing))
        .route("/api/pair/list", get(list_paired_devices))
        .route("/api/pair/revoke", post(revoke_device))
        .route("/api/pair/revoke-all", post(revoke_all_devices))
}
