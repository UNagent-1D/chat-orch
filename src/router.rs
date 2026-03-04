// Task 13: Full implementation pending.
// Builds the Axum router with all routes and middleware.

use crate::state::AppState;
use axum::{routing::get, Router};

/// Build the complete Axum router with all routes and middleware layers.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Health / readiness
        .route("/health", get(health))
        .route("/ready", get(ready))
        // TODO(task-14): .merge(ingest::telegram::routes())
        // TODO(task-15): .merge(ingest::whatsapp::routes())
        // TODO(task-16): .merge(rest API routes with JWT middleware)
        //
        // Middleware layers (applied bottom-up):
        // TODO(task-13): TraceLayer, TimeoutLayer, DefaultBodyLimit, CatchPanicLayer
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn ready() -> &'static str {
    // TODO(task-13): Check Redis connectivity + downstream service health
    "ok"
}
