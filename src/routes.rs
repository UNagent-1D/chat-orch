use axum::extract::State;
use axum::http::{header, HeaderValue, Method};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::error::AppError;
use crate::runtime::run_turn;
use crate::session::SessionStore;
use crate::sse::{chat_stream, StreamEvent};
use crate::AppState;

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE]);

    let cors = match state.config.cors_allow_origin.parse::<HeaderValue>() {
        Ok(origin) => cors.allow_origin(origin),
        Err(_) => {
            tracing::warn!(
                origin = %state.config.cors_allow_origin,
                "invalid CORS_ALLOW_ORIGIN; falling back to any origin"
            );
            cors.allow_origin(tower_http::cors::Any)
        }
    };

    Router::new()
        .route("/health", get(health))
        .route("/v1/chat", post(chat_forward))
        .route("/v1/chat/stream", get(chat_stream))
        .route("/v1/feedback", post(submit_feedback))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
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
    message: AssistantMessage,
}

#[derive(Debug, Serialize)]
struct AssistantMessage {
    text: String,
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

    if let Some(metricas) = &state.metricas {
        metricas.record_turn(req.tenant_id.clone(), req.message.clone(), false);
    }

    // Extract session_id first so it can be moved in exactly one branch.
    let session_id = req.session_id;

    let (sid, reply_text) = if let Some(ar) = &state.agent_runtime {
        let sid = match session_id.filter(|s| !s.trim().is_empty()) {
            Some(id) => id,
            None => ar.create_session(&req.tenant_id).await?,
        };
        let resp = ar.post_turn(&sid, &req.message).await?;
        let text = resp["message"]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        (sid, text)
    } else {
        let sid = match session_id {
            Some(id) if !id.trim().is_empty() => id,
            _ => SessionStore::new_session_id(),
        };
        let (text, resolved) = run_turn(
            &state.llm,
            &state.hospital,
            &state.sessions,
            &sid,
            &req.message,
        )
        .await;
        if resolved {
            if let Some(metricas) = &state.metricas {
                metricas.record_turn(req.tenant_id.clone(), req.message.clone(), true);
            }
        }
        (sid, text)
    };

    if !reply_text.is_empty() {
        state.hub.publish(
            &sid,
            StreamEvent {
                kind: "assistant".into(),
                text: reply_text.clone(),
            },
        );
    }

    Ok(Json(ChatResponse {
        session_id: sid,
        message: AssistantMessage { text: reply_text },
    }))
}

#[derive(Debug, Deserialize)]
struct FeedbackRequest {
    tenant_id: String,
    #[serde(default)]
    session_id: Option<String>,
    score: u8,
}

async fn submit_feedback(
    State(state): State<AppState>,
    Json(req): Json<FeedbackRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.tenant_id.trim().is_empty() {
        return Err(AppError::BadRequest("tenant_id is required".into()));
    }
    if req.score < 1 || req.score > 5 {
        return Err(AppError::BadRequest(
            "score must be between 1 and 5".into(),
        ));
    }
    let _ = &req.session_id; // accepted but unused; metricas aggregates per tenant
    if let Some(metricas) = &state.metricas {
        metricas.record_feedback(req.tenant_id.clone(), req.score);
    }
    Ok(Json(serde_json::json!({ "status": "ok" })))
}
