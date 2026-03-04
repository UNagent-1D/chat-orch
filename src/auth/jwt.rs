// JWT validation middleware for REST API endpoints.
//
// The JWT is issued by the Tenant Service (Go). It contains:
// - sub: user ID
// - tenant_id: UUID
// - role: "app_admin" | "tenant_admin" | "tenant_operator"
// - iss: "tenant-service" (configurable)
// - exp: expiration timestamp

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::AppState;

/// Claims extracted from the JWT token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Subject — the user ID.
    pub sub: String,
    /// The tenant this user belongs to.
    pub tenant_id: Uuid,
    /// User role: app_admin, tenant_admin, tenant_operator
    pub role: String,
    /// Issuer
    pub iss: Option<String>,
    /// Expiration (Unix timestamp)
    pub exp: usize,
}

/// Axum middleware that validates JWT tokens.
///
/// Extracts the `Authorization: Bearer <token>` header, validates the JWT,
/// and injects the claims into request extensions for downstream handlers.
///
/// Usage in router:
/// ```rust,ignore
/// Router::new()
///     .route("/api/foo", get(handler))
///     .layer(middleware::from_fn_with_state(state.clone(), jwt_middleware))
/// ```
pub async fn jwt_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let mut validation = Validation::default();
    validation.set_issuer(&[&state.config.jwt_issuer]);

    let key = DecodingKey::from_secret(state.config.jwt_secret.as_bytes());

    let token_data = decode::<JwtClaims>(token, &key, &validation)
        .map_err(|e| {
            tracing::debug!(error = %e, "JWT validation failed");
            StatusCode::UNAUTHORIZED
        })?;

    // Insert claims into request extensions for handlers to access
    request.extensions_mut().insert(token_data.claims);

    Ok(next.run(request).await)
}
