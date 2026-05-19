use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("downstream service unavailable: {0}")]
    Downstream(String),

    #[error("required env var {0} is not set")]
    MissingEnv(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("rate limit exceeded for {scope}, retry in {retry_after_secs}s")]
    TooManyRequests {
        retry_after_secs: u64,
        scope: &'static str,
    },
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message, retry_after) = match &self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone(), None),
            AppError::Downstream(_) => (
                StatusCode::BAD_GATEWAY,
                "downstream service unavailable".into(),
                None,
            ),
            AppError::MissingEnv(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".into(),
                None,
            ),
            AppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".into(),
                None,
            ),
            AppError::TooManyRequests {
                retry_after_secs, ..
            } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate limit exceeded".into(),
                Some(*retry_after_secs),
            ),
        };

        if status.is_client_error() {
            tracing::warn!(error = %self, status = %status, "request error");
        } else {
            tracing::error!(error = %self, status = %status, "request error");
        }

        let body = serde_json::json!({ "error": message });
        let mut response = (status, axum::Json(body)).into_response();
        if let Some(secs) = retry_after {
            if let Ok(value) = axum::http::HeaderValue::from_str(&secs.to_string()) {
                response.headers_mut().insert("Retry-After", value);
            }
        }
        response
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        AppError::Downstream(err.to_string())
    }
}
