// Conversation turn logic — the glue between session, agent config, and LLM.
//
// This module takes a ResolvedMessage + Session + AgentConfig, extracts the
// user's text content, runs the LLM turn loop, and returns an AgentResponse.
//
// For MVP this is a single file. Promote to a module directory if it grows
// past 300 lines.

use std::sync::Arc;

use crate::error::AppError;
use crate::gateway::acr_client::AgentConfig;
use crate::llm::client::LlmClient;
use crate::llm::tool_executor::ToolExecutor;
use crate::llm::turn_loop;
use crate::types::agent_response::AgentResponse;
use crate::types::message_content::MessageContent;
use crate::types::resolved_message::ResolvedMessage;
use crate::types::session::Session;

/// Process a single conversation turn.
///
/// This is the core function called by the pipeline worker (Task 11) and
/// the REST `/conversation/chat/turn` endpoint (Task 16).
///
/// # Flow
/// 1. Extract user text from the message content
/// 2. Handle non-text content (fallback replies for unsupported types)
/// 3. Fetch data sources for tool execution
/// 4. Run the LLM turn loop (may involve multiple tool calls)
/// 5. Return the agent's response
pub async fn process_turn(
    llm_client: &Arc<dyn LlmClient>,
    tool_executor: &ToolExecutor,
    _session: &Session,
    agent_config: &AgentConfig,
    resolved: &ResolvedMessage,
    data_sources: &[crate::gateway::tenant_client::DataSource],
) -> Result<AgentResponse, AppError> {
    // Extract user text based on content type
    let user_text = extract_user_text(&resolved.content)?;

    // Run the LLM turn loop
    // For MVP, we don't pass conversation history — each turn is stateless.
    // TODO: Load conversation history from Redis session for multi-turn context.
    let history = vec![];

    let response_text = turn_loop::execute_turn(
        llm_client.as_ref(),
        tool_executor,
        agent_config,
        data_sources,
        &user_text,
        &history,
    )
    .await?;

    tracing::info!(
        conversation_id = %_session.conversation_id,
        tenant_id = %resolved.tenant_id,
        response_len = response_text.len(),
        "conversation turn completed"
    );

    Ok(AgentResponse::text(response_text))
}

/// Extract user-facing text from the message content.
///
/// For text messages, returns the text directly.
/// For media with captions, returns the caption.
/// For locations, returns a formatted string.
/// For contacts, returns a formatted string.
/// For unsupported types, returns an error (should not reach here).
fn extract_user_text(content: &MessageContent) -> Result<String, AppError> {
    match content {
        MessageContent::Text { text } => Ok(text.clone()),

        MessageContent::Interactive { action_type, payload } => {
            // Send the structured selection as text for the LLM
            Ok(format!(
                "[user selected: {action_type}] {}",
                serde_json::to_string(payload).unwrap_or_default()
            ))
        }

        MessageContent::CallbackQuery { data, .. } => {
            // Button press — the data IS the user's selection
            Ok(data.clone())
        }

        MessageContent::Image { caption, .. } => Ok(caption
            .clone()
            .unwrap_or_else(|| "[user sent an image without caption]".to_string())),

        MessageContent::Video { caption, .. } => Ok(caption
            .clone()
            .unwrap_or_else(|| "[user sent a video without caption]".to_string())),

        MessageContent::Location { lat, lng } => {
            Ok(format!("My location is: latitude {lat}, longitude {lng}"))
        }

        MessageContent::Audio { .. } => {
            // For MVP, we can't process audio — tell the LLM
            Ok("[user sent an audio/voice message — transcription not available]".to_string())
        }

        MessageContent::Document { filename, .. } => {
            Ok(format!("[user sent a document: {filename}]"))
        }

        MessageContent::Contact { phone, name } => {
            Ok(format!("Here's a contact: {name} ({phone})"))
        }

        MessageContent::Sticker { emoji, .. } => {
            let emoji_str = emoji.as_deref().unwrap_or("a sticker");
            Ok(format!("[user sent {emoji_str}]"))
        }

        MessageContent::Reaction { emoji, .. } => {
            Ok(format!("[user reacted with {emoji}]"))
        }

        MessageContent::Unsupported { type_name, .. } => Err(AppError::BadRequest(format!(
            "unsupported message type: {type_name}"
        ))),
    }
}
