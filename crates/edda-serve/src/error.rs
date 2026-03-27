use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

// ── Error Handling ──

#[derive(Debug, thiserror::Error)]
pub(crate) enum AppError {
    #[error("{0}")]
    Validation(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    #[error("{0}")]
    Unauthorized(String),

    #[error("{0}")]
    ServiceUnavailable(String),

    #[error("{0}")]
    NotImplemented(String),

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
            AppError::ServiceUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "SERVICE_UNAVAILABLE")
            }
            AppError::NotImplemented(_) => (StatusCode::NOT_IMPLEMENTED, "NOT_IMPLEMENTED"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        };
        let body = serde_json::json!({
            "error": self.to_string(),
            "code": code,
        });
        if matches!(&self, AppError::ServiceUnavailable(_)) {
            let mut resp = (status, Json(body)).into_response();
            resp.headers_mut()
                .insert("retry-after", "1".parse().expect("valid header value"));
            return resp;
        }
        (status, Json(body)).into_response()
    }
}

/// Classify a ledger `open()` error into the appropriate `AppError` variant.
///
/// - "not an edda workspace" → `NotFound` (project not initialized)
/// - "database is locked" → `ServiceUnavailable` (transient SQLite busy)
/// - Everything else → `Internal`
pub(crate) fn classify_open_error(err: anyhow::Error) -> AppError {
    let msg = format!("{err:#}");

    if msg.contains("not an edda workspace") {
        return AppError::NotFound(err.to_string());
    }

    if msg.contains("database is locked") {
        return AppError::ServiceUnavailable(
            "database is temporarily unavailable, please retry".into(),
        );
    }

    AppError::Internal(err)
}
