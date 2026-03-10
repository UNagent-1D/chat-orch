use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Application-wide error type.
///
/// Every variant maps to an HTTP status code and a safe client-facing message.
/// Internal details are logged server-side via `tracing` but never exposed to callers.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("tenant not found for channel key")]
    TenantNotFound,

    #[error("channel is inactive")]
    ChannelInactive,

    #[error("session not found or expired")]
    SessionNotFound,

    #[error("session creation failed: {0}")]
    SessionCreation(String),

    #[error("service overloaded — try again later")]
    Overloaded,

    #[error("webhook signature verification failed")]
    SignatureInvalid,

    #[error("authentication required")]
    Unauthorized,

    #[error("insufficient permissions")]
    Forbidden,

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("downstream service error: {0}")]
    Downstream(String),

    #[error("LLM provider error: {0}")]
    LlmError(String),

    #[error("redis error: {0}")]
    Redis(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::TenantNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::ChannelInactive => (StatusCode::FORBIDDEN, self.to_string()),
            AppError::SessionNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::SessionCreation(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "session creation failed".into(),
            ),
            AppError::Overloaded => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
            AppError::SignatureInvalid => (StatusCode::FORBIDDEN, self.to_string()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, self.to_string()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Downstream(_) => (
                StatusCode::BAD_GATEWAY,
                "downstream service unavailable".into(),
            ),
            AppError::LlmError(_) => (StatusCode::BAD_GATEWAY, "LLM provider error".into()),
            AppError::Redis(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal storage error".into(),
            ),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into()),
        };

        // Log the full error server-side
        tracing::error!(error = %self, status = %status, "request error");

        let body = serde_json::json!({ "error": message });
        (status, axum::Json(body)).into_response()
    }
}

// Convenience conversions
impl From<redis::RedisError> for AppError {
    fn from(err: redis::RedisError) -> Self {
        AppError::Redis(err.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        AppError::Downstream(err.to_string())
    }
}
