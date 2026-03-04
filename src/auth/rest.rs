// REST API endpoints for web widget / dashboard clients.
//
// These endpoints are authenticated via JWT (not webhook signatures).
// They are SYNCHRONOUS — they await the response and return it inline
// (unlike webhook handlers which spawn background tasks).
//
// Routes:
//   POST /conversation/entrypoint/open — create or resume a session
//   POST /conversation/chat/turn       — send a message, get LLM response

use axum::{
    extract::State,
    middleware,
    routing::post,
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::conversation;
use crate::error::AppError;
use crate::state::AppState;
use crate::types::ingest_message::ChannelType;
use crate::types::message_content::MessageContent;
use crate::types::resolved_message::ResolvedMessage;
use crate::types::session::{ConfigRefs, SessionKey};

use super::jwt::{jwt_middleware, JwtClaims};

/// Build REST API routes with JWT middleware.
pub fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/conversation/entrypoint/open", post(open_entrypoint))
        .route("/conversation/chat/turn", post(chat_turn))
        .layer(middleware::from_fn_with_state(state, jwt_middleware))
}

// --- Request / Response types -----------------------------------------------

#[derive(Debug, Deserialize)]
struct OpenEntrypointRequest {
    /// Which agent profile to use (from tenant config).
    agent_profile_id: Uuid,
    /// Optional: resume an existing session by token.
    session_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct OpenEntrypointResponse {
    session_token: String,
    conversation_id: Uuid,
    tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct ChatTurnRequest {
    /// Session token from open_entrypoint.
    session_token: String,
    /// User's message text.
    message: String,
}

#[derive(Debug, Serialize)]
struct ChatTurnResponse {
    /// The agent's reply text.
    reply: String,
    /// Conversation ID for reference.
    conversation_id: Uuid,
}

// --- Handlers ---------------------------------------------------------------

/// POST /conversation/entrypoint/open
///
/// Creates a new conversation session or resumes an existing one.
/// Returns a session token that must be passed to subsequent chat/turn calls.
///
/// This endpoint does NOT go through the semaphore pipeline — it's synchronous.
async fn open_entrypoint(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(req): Json<OpenEntrypointRequest>,
) -> Result<Json<OpenEntrypointResponse>, AppError> {
    // If resuming an existing session, look it up
    if let Some(ref token) = req.session_token {
        if let Some(session) = state.session_store.get_by_token(token).await? {
            return Ok(Json(OpenEntrypointResponse {
                session_token: session.session_token.clone(),
                conversation_id: session.conversation_id,
                tenant_id: session.tenant_id,
            }));
        }
        // Token expired or not found — create new session
    }

    // Create a new session
    let session_key = SessionKey {
        tenant_id: claims.tenant_id,
        channel_type: ChannelType::WebWidget,
        channel_user_id: claims.sub.clone(),
    };

    let config_refs = ConfigRefs {
        agent_profile_id: req.agent_profile_id,
        agent_config_id: Uuid::nil(), // Will be populated from ACR
        config_version: 0,
    };

    let session = state
        .session_store
        .get_or_create(&session_key, claims.tenant_id, config_refs)
        .await?;

    // Index the token for future lookups
    state
        .session_store
        .index_token(&session, &session_key)
        .await?;

    tracing::info!(
        conversation_id = %session.conversation_id,
        tenant_id = %claims.tenant_id,
        user_id = %claims.sub,
        "REST session opened"
    );

    Ok(Json(OpenEntrypointResponse {
        session_token: session.session_token.clone(),
        conversation_id: session.conversation_id,
        tenant_id: session.tenant_id,
    }))
}

/// POST /conversation/chat/turn
///
/// Sends a user message and returns the agent's response synchronously.
/// This does NOT go through the semaphore pipeline — it awaits the LLM
/// response directly and returns it in the HTTP body.
async fn chat_turn(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(req): Json<ChatTurnRequest>,
) -> Result<Json<ChatTurnResponse>, AppError> {
    // Look up session by token
    let session = state
        .session_store
        .get_by_token(&req.session_token)
        .await?
        .ok_or(AppError::SessionNotFound)?;

    // Verify the session belongs to this tenant
    if session.tenant_id != claims.tenant_id {
        return Err(AppError::Forbidden);
    }

    // Fetch agent config
    let agent_config = state
        .config_cache
        .resolve(
            session.tenant_id,
            session.config_refs.agent_profile_id,
            &state.acr_client,
        )
        .await?;

    // Fetch data sources for tool execution
    let data_sources = state
        .tenant_client
        .get_data_sources(session.tenant_id)
        .await
        .unwrap_or_default();

    // Fetch tool registry for enriched tool definitions (graceful degradation)
    let tool_registry = state
        .tool_registry_cache
        .resolve(&state.acr_client)
        .await;

    // Build a synthetic ResolvedMessage for the conversation logic
    let resolved = ResolvedMessage {
        id: Uuid::new_v4().to_string(),
        channel_type: ChannelType::WebWidget,
        channel_user_id: claims.sub.clone(),
        channel_key: "web_widget".to_string(),
        tenant_id: claims.tenant_id,
        tenant_slug: String::new(), // Not needed for REST responses
        agent_profile_id: session.config_refs.agent_profile_id,
        content: MessageContent::Text {
            text: req.message.clone(),
        },
        reply_to_id: None,
        timestamp: chrono::Utc::now(),
        raw_metadata: None,
    };

    // Process the conversation turn synchronously
    let response = conversation::process_turn(
        &state.llm_client,
        &state.tool_executor,
        &session,
        &agent_config,
        &resolved,
        &data_sources,
        &tool_registry,
    )
    .await?;

    // Extract text from response parts
    let reply = response
        .parts
        .iter()
        .filter_map(|p| match p {
            crate::types::agent_response::ResponsePart::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    Ok(Json(ChatTurnResponse {
        reply,
        conversation_id: session.conversation_id,
    }))
}
