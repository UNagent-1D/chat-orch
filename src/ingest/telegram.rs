// Telegram webhook handler.
//
// Route: `POST /webhook/telegram/:tenant_slug`
//
// The tenant_slug in the URL path is used as the channel_key for tenant
// resolution. This allows each tenant to have a unique webhook URL.
//
// Signature verification: `X-Telegram-Bot-Api-Secret-Token` header must
// match `TELEGRAM_WEBHOOK_SECRET` from config.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use chrono::{DateTime, Utc};

use crate::error::AppError;
use crate::state::AppState;
use crate::types::ingest_message::{ChannelType, IngestMessage};
use crate::types::message_content::MessageContent;

use super::telegram_types::{TelegramMessage, TelegramUpdate};

/// Build the Telegram webhook routes.
pub fn routes() -> Router<AppState> {
    Router::new().route("/webhook/telegram/:tenant_slug", post(handle_webhook))
}

/// Handle incoming Telegram webhook updates.
///
/// Flow:
/// 1. Verify `X-Telegram-Bot-Api-Secret-Token` header
/// 2. Parse JSON body into TelegramUpdate
/// 3. Normalize to IngestMessage
/// 4. Dedup check (update_id)
/// 5. Submit to pipeline (acquire semaphore, spawn task)
/// 6. Return 200 OK (processing continues in background)
async fn handle_webhook(
    State(state): State<AppState>,
    Path(tenant_slug): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    // 1. Verify secret token
    verify_signature(&state, &headers)?;

    // 2. Parse update
    let update: TelegramUpdate = serde_json::from_slice(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid Telegram update: {e}")))?;

    // 3. Normalize to IngestMessage
    let msg = normalize_update(update, &tenant_slug)?;
    let Some(msg) = msg else {
        // Update type we don't handle (e.g., channel_post, inline_query)
        return Ok(StatusCode::OK);
    };

    // 4. Dedup check
    let is_new = state
        .dedup
        .check_and_mark(
            ChannelType::Telegram.as_str(),
            &msg.id,
        )
        .await?;

    if !is_new {
        tracing::debug!(message_id = %msg.id, "duplicate Telegram update — skipping");
        return Ok(StatusCode::OK);
    }

    // 5. Submit to pipeline
    let app = Arc::new(state);
    app.pipeline.try_process(app.clone(), msg)?;

    // 6. Return 200 immediately — processing happens in background
    Ok(StatusCode::OK)
}

/// Verify the `X-Telegram-Bot-Api-Secret-Token` header.
fn verify_signature(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let expected = match &state.config.telegram_webhook_secret {
        Some(secret) if !secret.is_empty() => secret,
        _ => return Ok(()), // No secret configured — skip verification (dev mode)
    };

    let actual = headers
        .get("x-telegram-bot-api-secret-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if actual != expected.as_str() {
        return Err(AppError::SignatureInvalid);
    }

    Ok(())
}

/// Normalize a Telegram update into an IngestMessage.
///
/// Returns `None` for update types we don't handle.
fn normalize_update(
    update: TelegramUpdate,
    tenant_slug: &str,
) -> Result<Option<IngestMessage>, AppError> {
    // Handle callback_query (inline keyboard button press)
    if let Some(cb) = update.callback_query {
        let user_id = cb.from.id.to_string();
        let data = cb.data.unwrap_or_default();
        let message_id = cb
            .message
            .as_ref()
            .map(|m| m.message_id.to_string())
            .unwrap_or_default();

        return Ok(Some(IngestMessage {
            id: update.update_id.to_string(),
            channel_type: ChannelType::Telegram,
            channel_user_id: user_id,
            channel_key: tenant_slug.to_string(),
            content: MessageContent::CallbackQuery {
                data,
                message_id,
            },
            reply_to_id: None,
            timestamp: Utc::now(),
            raw_metadata: None,
        }));
    }

    // Handle message or edited_message
    let telegram_msg = update
        .message
        .or(update.edited_message);

    let Some(msg) = telegram_msg else {
        // Update type we don't handle
        return Ok(None);
    };

    normalize_message(msg, update.update_id, tenant_slug).map(Some)
}

/// Normalize a single Telegram message into an IngestMessage.
fn normalize_message(
    msg: TelegramMessage,
    update_id: i64,
    tenant_slug: &str,
) -> Result<IngestMessage, AppError> {
    let user_id = msg
        .from
        .as_ref()
        .map(|u| u.id.to_string())
        .unwrap_or_else(|| msg.chat.id.to_string());

    let reply_to_id = msg
        .reply_to_message
        .as_ref()
        .map(|r| r.message_id.to_string());

    let timestamp: DateTime<Utc> = DateTime::from_timestamp(msg.date, 0)
        .unwrap_or_else(Utc::now);

    let content = extract_content(&msg);

    Ok(IngestMessage {
        id: update_id.to_string(),
        channel_type: ChannelType::Telegram,
        channel_user_id: user_id,
        channel_key: tenant_slug.to_string(),
        content,
        reply_to_id,
        timestamp,
        raw_metadata: None,
    })
}

/// Extract message content from the Telegram message.
///
/// Checks each content field in priority order (text first).
fn extract_content(msg: &TelegramMessage) -> MessageContent {
    if let Some(ref text) = msg.text {
        return MessageContent::Text {
            text: text.clone(),
        };
    }

    if let Some(ref photos) = msg.photo {
        // Take the largest photo (last in array)
        let best = photos.last().unwrap();
        return MessageContent::Image {
            file_id: best.file_id.clone(),
            caption: None, // Telegram puts caption in msg.text for photos
        };
    }

    if let Some(ref video) = msg.video {
        return MessageContent::Video {
            file_id: video.file_id.clone(),
            caption: None,
        };
    }

    if let Some(ref audio) = msg.audio {
        return MessageContent::Audio {
            file_id: audio.file_id.clone(),
            duration_secs: Some(audio.duration),
        };
    }

    if let Some(ref voice) = msg.voice {
        return MessageContent::Audio {
            file_id: voice.file_id.clone(),
            duration_secs: Some(voice.duration),
        };
    }

    if let Some(ref doc) = msg.document {
        return MessageContent::Document {
            file_id: doc.file_id.clone(),
            filename: doc.file_name.clone().unwrap_or_else(|| "document".to_string()),
        };
    }

    if let Some(ref sticker) = msg.sticker {
        return MessageContent::Sticker {
            file_id: sticker.file_id.clone(),
            emoji: sticker.emoji.clone(),
        };
    }

    if let Some(ref loc) = msg.location {
        return MessageContent::Location {
            lat: loc.latitude,
            lng: loc.longitude,
        };
    }

    if let Some(ref contact) = msg.contact {
        let name = match &contact.last_name {
            Some(last) => format!("{} {}", contact.first_name, last),
            None => contact.first_name.clone(),
        };
        return MessageContent::Contact {
            name,
            phone: contact.phone_number.clone(),
        };
    }

    // Unsupported message type
    MessageContent::Unsupported {
        type_name: "unknown_telegram".to_string(),
        raw_sample: None,
    }
}
