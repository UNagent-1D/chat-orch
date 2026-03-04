// WhatsApp Cloud API webhook handler.
//
// Routes:
//   POST /webhook/whatsapp — receive messages
//   GET  /webhook/whatsapp — webhook verification (echo hub.challenge)
//
// CRITICAL:
// - Filter out statuses[] (delivery receipts) — never route to LLM
// - channel_key is phone_number_id (NOT the phone number string)
// - A single webhook payload can contain MULTIPLE messages (batched)
// - HMAC-SHA256 signature verification using WHATSAPP_APP_SECRET

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Router,
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::AppError;
use crate::state::AppState;
use crate::types::ingest_message::{ChannelType, IngestMessage};
use crate::types::message_content::MessageContent;

use super::whatsapp_types::{WhatsAppMessage, WhatsAppWebhook};

type HmacSha256 = Hmac<Sha256>;

/// Build the WhatsApp webhook routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/webhook/whatsapp", post(handle_webhook))
        .route("/webhook/whatsapp", get(handle_verify))
}

/// Query params for webhook verification.
#[derive(Debug, serde::Deserialize)]
struct VerifyQuery {
    #[serde(rename = "hub.mode")]
    hub_mode: String,
    #[serde(rename = "hub.verify_token")]
    hub_verify_token: String,
    #[serde(rename = "hub.challenge")]
    hub_challenge: String,
}

/// GET /webhook/whatsapp — Meta webhook verification.
///
/// During webhook registration, Meta sends a GET request with a challenge.
/// We echo it back if the verify_token matches.
async fn handle_verify(
    State(state): State<AppState>,
    Query(query): Query<VerifyQuery>,
) -> Result<String, StatusCode> {
    let expected = state
        .config
        .whatsapp_verify_token
        .as_deref()
        .unwrap_or("");

    if query.hub_mode == "subscribe" && query.hub_verify_token == expected {
        tracing::info!("WhatsApp webhook verified");
        Ok(query.hub_challenge)
    } else {
        tracing::warn!("WhatsApp webhook verification failed");
        Err(StatusCode::FORBIDDEN)
    }
}

/// POST /webhook/whatsapp — receive messages.
///
/// Flow:
/// 1. Verify HMAC-SHA256 signature
/// 2. Parse webhook payload
/// 3. Filter out statuses (delivery receipts)
/// 4. Normalize each message to IngestMessage
/// 5. Dedup check per message
/// 6. Submit each to pipeline
/// 7. Return 200 OK
async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    // 1. Verify HMAC signature
    verify_signature(&state, &headers, &body)?;

    // 2. Parse webhook payload
    let webhook: WhatsAppWebhook = serde_json::from_slice(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid WhatsApp webhook: {e}")))?;

    if webhook.object != "whatsapp_business_account" {
        return Ok(StatusCode::OK); // Not a WhatsApp event
    }

    let app = Arc::new(state);

    // 3. Iterate all entries → changes → messages
    for entry in &webhook.entry {
        for change in &entry.changes {
            if change.field != "messages" {
                continue;
            }

            let phone_number_id = &change.value.metadata.phone_number_id;

            // CRITICAL: Skip if only statuses (delivery receipts)
            if change.value.messages.is_empty() {
                tracing::debug!(
                    phone_number_id = %phone_number_id,
                    status_count = change.value.statuses.len(),
                    "skipping status-only webhook"
                );
                continue;
            }

            // 4. Normalize each message
            for wa_msg in &change.value.messages {
                let msg = normalize_message(wa_msg, phone_number_id);

                // 5. Dedup check
                let is_new = app
                    .dedup
                    .check_and_mark(ChannelType::Whatsapp.as_str(), &msg.id)
                    .await?;

                if !is_new {
                    tracing::debug!(
                        message_id = %msg.id,
                        "duplicate WhatsApp message — skipping"
                    );
                    continue;
                }

                // 6. Submit to pipeline
                if let Err(e) = app.pipeline.try_process(app.clone(), msg) {
                    tracing::error!(error = %e, "failed to submit WhatsApp message to pipeline");
                    // Don't fail the whole batch — continue processing other messages
                }
            }
        }
    }

    Ok(StatusCode::OK)
}

/// Verify HMAC-SHA256 signature from the `X-Hub-Signature-256` header.
fn verify_signature(
    state: &AppState,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), AppError> {
    let secret = match &state.config.whatsapp_app_secret {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(()), // No secret configured — skip (dev mode)
    };

    let sig_header = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let expected_sig = sig_header.strip_prefix("sha256=").unwrap_or("");

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| AppError::Internal(format!("HMAC key error: {e}")))?;
    mac.update(body);
    let computed = hex::encode(mac.finalize().into_bytes());

    if computed != expected_sig {
        return Err(AppError::SignatureInvalid);
    }

    Ok(())
}

/// Normalize a WhatsApp message to IngestMessage.
fn normalize_message(wa_msg: &WhatsAppMessage, phone_number_id: &str) -> IngestMessage {
    let timestamp: DateTime<Utc> = wa_msg
        .timestamp
        .parse::<i64>()
        .ok()
        .and_then(|ts| DateTime::from_timestamp(ts, 0))
        .unwrap_or_else(Utc::now);

    let reply_to_id = wa_msg
        .context
        .as_ref()
        .and_then(|c| c.message_id.clone());

    let content = extract_content(wa_msg);

    IngestMessage {
        id: wa_msg.id.clone(),
        channel_type: ChannelType::Whatsapp,
        channel_user_id: wa_msg.from.clone(),
        channel_key: phone_number_id.to_string(),
        content,
        reply_to_id,
        timestamp,
        raw_metadata: None,
    }
}

/// Extract message content from the WhatsApp message based on its type.
fn extract_content(msg: &WhatsAppMessage) -> MessageContent {
    match msg.msg_type.as_str() {
        "text" => {
            let text = msg
                .text
                .as_ref()
                .map(|t| t.body.clone())
                .unwrap_or_default();
            MessageContent::Text { text }
        }

        "image" => {
            let media = msg.image.as_ref().unwrap();
            MessageContent::Image {
                file_id: media.id.clone(),
                caption: media.caption.clone(),
            }
        }

        "video" => {
            let media = msg.video.as_ref().unwrap();
            MessageContent::Video {
                file_id: media.id.clone(),
                caption: media.caption.clone(),
            }
        }

        "audio" | "voice" => {
            let media = msg.audio.as_ref().unwrap();
            MessageContent::Audio {
                file_id: media.id.clone(),
                duration_secs: None,
            }
        }

        "document" => {
            let doc = msg.document.as_ref().unwrap();
            MessageContent::Document {
                file_id: doc.id.clone(),
                filename: doc
                    .filename
                    .clone()
                    .unwrap_or_else(|| "document".to_string()),
            }
        }

        "location" => {
            let loc = msg.location.as_ref().unwrap();
            MessageContent::Location {
                lat: loc.latitude,
                lng: loc.longitude,
            }
        }

        "contacts" => {
            if let Some(contacts) = &msg.contacts {
                if let Some(first) = contacts.first() {
                    let phone = first
                        .phones
                        .as_ref()
                        .and_then(|p| p.first())
                        .map(|p| p.phone.clone())
                        .unwrap_or_default();

                    return MessageContent::Contact {
                        name: first.name.formatted_name.clone(),
                        phone,
                    };
                }
            }
            MessageContent::Unsupported {
                type_name: "contacts_empty".to_string(),
                raw_sample: None,
            }
        }

        "sticker" => {
            let media = msg.sticker.as_ref().unwrap();
            MessageContent::Sticker {
                file_id: media.id.clone(),
                emoji: None, // WhatsApp stickers don't have emoji metadata
            }
        }

        "interactive" => {
            if let Some(interactive) = &msg.interactive {
                let (action_type, payload) = match interactive.interactive_type.as_str() {
                    "list_reply" => {
                        let lr = interactive.list_reply.as_ref().unwrap();
                        (
                            "list_reply".to_string(),
                            serde_json::json!({
                                "id": lr.id,
                                "title": lr.title,
                                "description": lr.description
                            }),
                        )
                    }
                    "button_reply" => {
                        let br = interactive.button_reply.as_ref().unwrap();
                        (
                            "button_reply".to_string(),
                            serde_json::json!({
                                "id": br.id,
                                "title": br.title
                            }),
                        )
                    }
                    other => (
                        other.to_string(),
                        serde_json::json!({}),
                    ),
                };

                return MessageContent::Interactive {
                    action_type,
                    payload,
                };
            }
            MessageContent::Unsupported {
                type_name: "interactive_empty".to_string(),
                raw_sample: None,
            }
        }

        "button" => {
            if let Some(btn) = &msg.button {
                return MessageContent::Text {
                    text: btn.text.clone(),
                };
            }
            MessageContent::Unsupported {
                type_name: "button_empty".to_string(),
                raw_sample: None,
            }
        }

        "reaction" => {
            // WhatsApp reactions — silent acknowledge
            MessageContent::Reaction {
                emoji: "".to_string(), // Reaction emoji is in a nested field we don't parse
                target_message_id: msg.id.clone(),
            }
        }

        other => MessageContent::Unsupported {
            type_name: format!("whatsapp_{other}"),
            raw_sample: None,
        },
    }
}
