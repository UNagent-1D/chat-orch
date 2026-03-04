use std::time::Duration;

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use tower_http::{
    catch_panic::CatchPanicLayer,
    limit::RequestBodyLimitLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::auth::rest;
use crate::ingest::{telegram, whatsapp};
use crate::state::AppState;

/// Build the complete Axum router with all routes and middleware layers.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Health / readiness probes (no auth, no state needed for health)
        .route("/health", get(health))
        .route("/ready", get(ready))
        // Pipeline metrics
        .route("/metrics/pipeline", get(pipeline_metrics))
        // Channel webhook handlers
        .merge(telegram::routes())
        .merge(whatsapp::routes())
        // REST API with JWT authentication
        .merge(rest::routes(state.clone()))
        //
        // Middleware layers (applied bottom-up, so outermost first):
        .layer(CatchPanicLayer::new())
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(RequestBodyLimitLayer::new(1024 * 1024)) // 1MB max body
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Liveness probe — always returns 200 if the process is running.
async fn health() -> &'static str {
    "ok"
}

/// Readiness probe — checks Redis connectivity.
///
/// Returns 200 if all dependencies are reachable, 503 otherwise.
/// Kubernetes uses this to decide whether to route traffic to this pod.
async fn ready(State(state): State<AppState>) -> Result<&'static str, StatusCode> {
    state
        .session_store
        .ping()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "readiness check failed — Redis unreachable");
            StatusCode::SERVICE_UNAVAILABLE
        })?;

    Ok("ok")
}

/// Pipeline metrics endpoint — useful for monitoring/autoscaling.
async fn pipeline_metrics(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "pipeline_available_permits": state.pipeline.available_permits(),
        "channel_cache_entries": state.channel_cache.entry_count(),
        "config_cache_entries": state.config_cache.entry_count(),
    }))
}
