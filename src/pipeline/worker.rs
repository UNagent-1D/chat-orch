use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::conversation;
use crate::error::AppError;
use crate::state::AppState;
use crate::types::agent_response::AgentResponse;
use crate::types::ingest_message::IngestMessage;

/// Semaphore-bounded pipeline for processing messages.
///
/// Each message gets its own lightweight tokio task, bounded by a semaphore.
/// This scales to 100k msg/sec because:
/// - tokio tasks are cheap (~few KB each)
/// - work is I/O-bound (HTTP calls), not CPU-bound
/// - semaphore provides natural backpressure (503 when overloaded)
///
/// NOT a worker pool — we don't use MPSC channels because messages from
/// different users are independent and don't need ordering guarantees.
#[derive(Clone)]
pub struct Pipeline {
    semaphore: Arc<Semaphore>,
}

impl Pipeline {
    /// Create a new pipeline with the given concurrency limit.
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
        }
    }

    /// Try to process a message in a background task.
    ///
    /// Acquires a semaphore permit BEFORE returning. If the semaphore is
    /// exhausted, returns `AppError::Overloaded` (which becomes HTTP 503).
    /// The webhook handler should return 503 to the platform, which will retry.
    ///
    /// If a permit is acquired, spawns a tokio task and returns Ok(()).
    /// The webhook handler can then return 200 OK.
    pub fn try_process(&self, app: Arc<AppState>, msg: IngestMessage) -> Result<(), AppError> {
        // Acquire permit BEFORE returning 200 to webhook provider.
        // try_acquire is non-blocking — it either succeeds or fails immediately.
        let permit = self
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| AppError::Overloaded)?;

        tokio::spawn(async move {
            // Permit is held until this task completes
            let _permit = permit;

            if let Err(e) = process_message(&app, msg).await {
                tracing::error!(error = %e, "pipeline processing failed");
            }
        });

        Ok(())
    }

    /// Number of available permits (for metrics/monitoring).
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

/// Process a single message through the full pipeline.
///
/// This runs inside a spawned tokio task:
/// 1. Resolve tenant from channel_key (via cache → Tenant Service)
/// 2. Get/create session (Redis)
/// 3. Fetch agent config (via cache → ACR)
/// 4. Handle message routing (silent, fallback, or LLM)
/// 5. Process conversation turn (LLM + tool calls) if applicable
/// 6. Send reply back to originating channel
async fn process_message(app: &AppState, msg: IngestMessage) -> Result<(), AppError> {
    let channel_key = msg.channel_lookup_key();
    let channel_type = msg.channel_type;

    // 1. Resolve tenant
    let tenant = app
        .channel_cache
        .resolve(&channel_key, &app.tenant_client)
        .await?;

    // 2. TypeState transition: IngestMessage → ResolvedMessage
    let resolved = msg.resolve((*tenant).clone());

    // 3. Check message type routing BEFORE session creation
    // Silent messages (stickers, reactions) don't need a session at all
    if resolved.content.is_silent() {
        tracing::debug!(
            content_type = resolved.content.type_name(),
            channel = %channel_type,
            "silent message type — no reply"
        );
        return Ok(());
    }

    // 4. Get or create session + fetch agent config IN PARALLEL
    let session_key = resolved.session_key();
    let (session_result, config_result) = tokio::join!(
        app.session_store.get_or_create(
            &session_key,
            resolved.tenant_id,
            crate::types::session::ConfigRefs {
                agent_profile_id: resolved.agent_profile_id,
                agent_config_id: uuid::Uuid::nil(), // Filled from ACR response
                config_version: 0,
            },
        ),
        app.config_cache.resolve(
            resolved.tenant_id,
            resolved.agent_profile_id,
            &app.acr_client,
        ),
    );

    let session = session_result?;
    let agent_config = config_result?;

    // 5. Handle fallback messages (unsupported types that need a polite reply)
    if resolved.content.needs_fallback_reply() {
        let fallback_text = fallback_reply_text(resolved.content.type_name());
        let response = AgentResponse::text(fallback_text);
        app.reply_sender.send(&resolved, &response).await?;

        tracing::info!(
            conversation_id = %session.conversation_id,
            content_type = resolved.content.type_name(),
            channel = %channel_type,
            "sent fallback reply for unsupported content"
        );
        return Ok(());
    }

    // 6. Route to LLM for processing
    if resolved.content.should_route_to_llm() {
        // Fetch data sources for tool execution
        let data_sources = app
            .tenant_client
            .get_data_sources(resolved.tenant_id)
            .await
            .unwrap_or_default();

        let response = conversation::process_turn(
            &app.llm_client,
            &app.tool_executor,
            &session,
            &agent_config,
            &resolved,
            &data_sources,
        )
        .await?;

        // Send reply back to originating channel
        app.reply_sender.send(&resolved, &response).await?;

        tracing::info!(
            conversation_id = %session.conversation_id,
            tenant_id = %resolved.tenant_id,
            content_type = resolved.content.type_name(),
            channel = %channel_type,
            "message processed — reply sent"
        );
    }

    Ok(())
}

/// Generate a polite fallback reply for unsupported content types.
fn fallback_reply_text(content_type: &str) -> String {
    match content_type {
        "video" => "Thanks for the video! I can't process videos yet, but I can help you with text messages. How can I assist you today?".to_string(),
        "audio" => "Thanks for the audio message! I can't listen to audio yet, but I can help you with text messages. How can I assist you today?".to_string(),
        "document" => "Thanks for the document! I can't read documents yet, but I can help you with text messages. How can I assist you today?".to_string(),
        "contact" => "Thanks for sharing that contact! I can't process contacts directly, but I can help you with text messages. How can I assist you?".to_string(),
        "image" => "Thanks for the image! If you'd like me to help, please add a caption describing what you need. How can I assist you?".to_string(),
        _ => "I received your message, but I'm not sure how to process that type of content. Could you try sending a text message instead?".to_string(),
    }
}
