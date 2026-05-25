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
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

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
    if req.message.len() > 4096 {
        return Err(AppError::BadRequest("message exceeds maximum length of 4096 characters".into()));
    }

    if let Err(retry_after) = state.limiters.tenant_chat.check(&req.tenant_id) {
        let retry_secs = retry_after.as_secs().max(1);
        tracing::warn!(
            tenant_id = %req.tenant_id,
            retry_after_secs = retry_secs,
            "rate_limited_chat"
        );
        if let Some(metricas) = &state.metricas {
            metricas.record_rate_limit(req.tenant_id.clone(), "chat", retry_secs);
        }
        return Err(AppError::TooManyRequests {
            retry_after_secs: retry_secs,
            scope: "chat",
        });
    }

    if let Some(metricas) = &state.metricas {
        metricas.record_turn(req.tenant_id.clone(), req.message.clone(), false);
    }

    // Extract session_id first so it can be moved in exactly one branch.
    let session_id = req.session_id;

    let (sid, reply_text) = if let Some(ar) = &state.agent_runtime {
        let sid = match session_id.filter(|s| !s.trim().is_empty()) {
            Some(id) => id,
            // HTTP /v1/chat has no OTP gate (browser auth handles it),
            // so we don't know the user's email here. The session is
            // created without one; conversation-chat's email hook
            // becomes a no-op for this channel.
            None => ar.create_session(&req.tenant_id, None).await?,
        };
        let resp = ar.post_turn(&sid, &req.message).await?;
        let text = resp["message"]["text"].as_str().unwrap_or("").to_string();
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
        return Err(AppError::BadRequest("score must be between 1 and 5".into()));
    }
    if let Err(retry_after) = state.limiters.tenant_feedback.check(&req.tenant_id) {
        let retry_secs = retry_after.as_secs().max(1);
        tracing::warn!(
            tenant_id = %req.tenant_id,
            retry_after_secs = retry_secs,
            "rate_limited_feedback"
        );
        if let Some(metricas) = &state.metricas {
            metricas.record_rate_limit(req.tenant_id.clone(), "feedback", retry_secs);
        }
        return Err(AppError::TooManyRequests {
            retry_after_secs: retry_secs,
            scope: "feedback",
        });
    }
    let _ = &req.session_id; // accepted but unused; metricas aggregates per tenant
    if let Some(metricas) = &state.metricas {
        metricas.record_feedback(req.tenant_id.clone(), req.score);
    }
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

// ── Integration tests ────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::channel::Channel;
    use crate::config::AppConfig;
    use crate::hospital::HospitalClient;
    use crate::llm::LlmClient;
    use crate::rate_limit::{BucketConfig, Limiters, RateLimiter};
    use crate::session::SessionStore;
    use crate::sse::SseHub;

    fn test_config() -> AppConfig {
        AppConfig {
            server_host: "127.0.0.1".into(),
            server_port: 0,
            conversation_chat_url: "http://localhost:0".into(),
            tenant_service_url: "http://localhost:0".into(),
            hospital_mock_url: "http://localhost:0".into(),
            metricas_url: None,
            telegram_bot_token: None,
            telegram_default_tenant_id: None,
            telegram_default_tenant_slug: None,
            user_auth_url: None,
            cors_allow_origin: "*".into(),
            openai_api_key: "sk-test".into(),
            openai_base_url: "http://localhost:0/v1".into(),
            openai_default_model: "test-model".into(),
            agent_runtime_url: None,
            backend_channel_key: None,
            backend_channel_enabled: false,
            rust_log: "error".into(),
            log_format: "pretty".into(),
        }
    }

    fn build_state(limiters: Arc<Limiters>) -> AppState {
        let http = reqwest::Client::new();
        let channel = Channel::disabled();
        AppState {
            config: Arc::new(test_config()),
            llm: Arc::new(LlmClient::new(
                http.clone(),
                "http://localhost:0/v1".into(),
                "sk-test".into(),
                "test-model".into(),
            )),
            hospital: Arc::new(HospitalClient::new(
                http,
                "http://localhost:0".into(),
                channel,
            )),
            sessions: SessionStore::new(),
            metricas: None,
            agent_runtime: None,
            hub: SseHub::new(),
            limiters,
        }
    }

    fn tight_limiters() -> Arc<Limiters> {
        Arc::new(Limiters {
            tenant_chat: RateLimiter::new(BucketConfig::new(2.0, 0.0001)),
            tenant_feedback: RateLimiter::new(BucketConfig::new(2.0, 0.0001)),
        })
    }

    async fn body_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let state = build_state(tight_limiters());
        let app = build_router(state);
        let resp = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn chat_rejects_empty_tenant_id() {
        let state = build_state(tight_limiters());
        let app = build_router(state);
        let body = r#"{"tenant_id":"","message":"hello"}"#;
        let resp = app
            .oneshot(
                Request::post("/v1/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn chat_rejects_empty_message() {
        let state = build_state(tight_limiters());
        let app = build_router(state);
        let body = r#"{"tenant_id":"t1","message":""}"#;
        let resp = app
            .oneshot(
                Request::post("/v1/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn chat_rejects_oversized_message() {
        let state = build_state(tight_limiters());
        let app = build_router(state);
        let big_message = "x".repeat(4097);
        let body = format!(r#"{{"tenant_id":"t1","message":"{}"}}"#, big_message);
        let resp = app
            .oneshot(
                Request::post("/v1/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn chat_rate_limits_after_burst_exhausted() {
        // Burst=2: first two succeed (or fail with 502 from dead LLM), third → 429.
        let limiters = Arc::new(Limiters {
            tenant_chat: RateLimiter::new(BucketConfig::new(2.0, 0.0001)),
            tenant_feedback: RateLimiter::new(BucketConfig::new(100.0, 1.0)),
        });
        let router = build_router(build_state(limiters.clone()));

        // Pre-exhaust the bucket outside the router to isolate the 429 path.
        limiters.tenant_chat.check("tenant-ratelimit-test").unwrap();
        limiters.tenant_chat.check("tenant-ratelimit-test").unwrap();
        // Bucket is now empty; next request must be rejected.
        let app = build_router(build_state(limiters));
        let body = r#"{"tenant_id":"tenant-ratelimit-test","message":"hi"}"#;
        let resp = app
            .oneshot(
                Request::post("/v1/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "exhausted bucket must yield 429"
        );
        let retry_after = resp
            .headers()
            .get("Retry-After")
            .expect("429 must carry Retry-After header")
            .to_str()
            .unwrap()
            .parse::<u64>()
            .expect("Retry-After must be a number");
        assert!(retry_after >= 1);
    }

    #[tokio::test]
    async fn feedback_rejects_out_of_range_score() {
        let state = build_state(tight_limiters());
        let app = build_router(state);
        for bad_score in [0u8, 6u8] {
            let body = format!(r#"{{"tenant_id":"t1","score":{bad_score}}}"#);
            let resp = build_router(build_state(tight_limiters()))
                .oneshot(
                    Request::post("/v1/feedback")
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "score={bad_score} should be rejected"
            );
        }
    }

    #[tokio::test]
    async fn feedback_rate_limits_after_burst_exhausted() {
        let limiters = Arc::new(Limiters {
            tenant_chat: RateLimiter::new(BucketConfig::new(100.0, 1.0)),
            tenant_feedback: RateLimiter::new(BucketConfig::new(2.0, 0.0001)),
        });
        limiters.tenant_feedback.check("t-fb").unwrap();
        limiters.tenant_feedback.check("t-fb").unwrap();

        let app = build_router(build_state(limiters));
        let body = r#"{"tenant_id":"t-fb","score":4}"#;
        let resp = app
            .oneshot(
                Request::post("/v1/feedback")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(resp.headers().contains_key("Retry-After"));
    }
}
