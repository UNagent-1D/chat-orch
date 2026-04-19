use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;

use crate::error::AppError;
use crate::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/chat", post(chat_forward))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    tenant_id: String,
    #[serde(default)]
    session_id: Option<String>,
    message: String,
}

#[derive(Debug, Serialize)]
struct ChatResponse {
    session_id: String,
    #[serde(flatten)]
    downstream: serde_json::Value,
}

async fn chat_forward(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, AppError> {
    if req.tenant_id.trim().is_empty() {
        return Err(AppError::BadRequest("tenant_id is required".into()));
    }
    if req.message.trim().is_empty() {
        return Err(AppError::BadRequest("message is required".into()));
    }

    // Record every incoming turn in Metricas up-front, so KPIs reflect usage
    // even when conversation-chat itself is mid-refactor and errors out.
    // resolution is derived from the downstream response below when we get one.
    if let Some(metricas) = &state.metricas {
        metricas.record_turn(req.tenant_id.clone(), req.message.clone(), false);
    }

    let sid = match req.session_id {
        Some(id) if !id.trim().is_empty() => id,
        _ => state.conversation_chat.create_session(&req.tenant_id).await?,
    };

    let downstream = state.conversation_chat.post_turn(&sid, &req.message).await?;

    if let Some(metricas) = &state.metricas {
        if downstream
            .get("action")
            .and_then(|v| v.as_str())
            .map(|a| a == "close_session")
            .unwrap_or(false)
        {
            // Record a second event marked resolved=true so the resolution rate ticks up.
            metricas.record_turn(req.tenant_id.clone(), req.message.clone(), true);
        }
    }

    Ok(Json(ChatResponse {
        session_id: sid,
        downstream,
    }))
}
