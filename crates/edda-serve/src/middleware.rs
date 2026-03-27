use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use edda_ledger::device_token::hash_token;

use crate::error::AppError;
use crate::state::AppState;

/// Check if a socket address is localhost.
pub(crate) fn is_localhost(addr: &SocketAddr) -> bool {
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
pub(crate) fn generate_pairing_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    hex::encode(bytes)
}

/// Auth middleware: localhost passes through, remote needs Bearer token.
pub(crate) async fn auth_middleware(
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
