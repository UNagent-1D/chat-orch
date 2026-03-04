// API key middleware for internal/ops endpoints (e.g., /metrics/pipeline).
//
// Uses constant-time comparison (subtle::ConstantTimeEq) to prevent timing
// attacks. The API key is loaded from the METRICS_API_KEY env var.
//
// Fail-closed: if the key is not configured, the endpoint returns 403.
// This ensures that forgetting to set the env var never leaves the endpoint open.
//
// SECURITY: Never log the provided or configured API key values.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use subtle::ConstantTimeEq;

use crate::state::AppState;

/// Axum middleware that validates API key from the `X-Api-Key` header.
///
/// Returns:
/// - 403 if `METRICS_API_KEY` is not configured (fail-closed)
/// - 401 if the `X-Api-Key` header is missing
/// - 403 if the key does not match (constant-time comparison)
/// - Passes through to the next handler on success
pub async fn api_key_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let configured_key = state.config.metrics_api_key.as_deref().ok_or_else(|| {
        tracing::warn!("metrics endpoint accessed but METRICS_API_KEY not configured — returning 403");
        StatusCode::FORBIDDEN
    })?;

    let provided_key = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Constant-time comparison to prevent timing attacks.
    // Both keys are compared byte-by-byte in constant time regardless of
    // where the first mismatch occurs.
    let keys_match: bool = configured_key
        .as_bytes()
        .ct_eq(provided_key.as_bytes())
        .into();

    if !keys_match {
        tracing::debug!("API key mismatch for metrics endpoint");
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(next.run(request).await)
}
