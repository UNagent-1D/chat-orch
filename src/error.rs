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
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Downstream(_) => (
                StatusCode::BAD_GATEWAY,
                "downstream service unavailable".into(),
            ),
            AppError::MissingEnv(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".into(),
            ),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into()),
        };

        if status.is_client_error() {
            tracing::warn!(error = %self, status = %status, "request error");
        } else {
            tracing::error!(error = %self, status = %status, "request error");
        }

        let body = serde_json::json!({ "error": message });
        (status, axum::Json(body)).into_response()
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        AppError::Downstream(err.to_string())
    }
}
