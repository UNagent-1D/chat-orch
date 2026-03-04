use reqwest::Client;

use crate::error::AppError;
use crate::types::agent_response::{AgentResponse, ResponsePart};
use crate::types::ingest_message::ChannelType;
use crate::types::resolved_message::ResolvedMessage;

/// Dispatches agent responses back to the originating channel.
///
/// Maps channel-agnostic `AgentResponse` to channel-native message formats:
/// - Telegram: `POST /bot<TOKEN>/sendMessage` (+ sendPhoto, etc.)
/// - WhatsApp: `POST /v18.0/<PNID>/messages` (Graph API)
///
/// Each response part is sent as a separate message to preserve ordering
/// (e.g., text first, then interactive menu).
#[derive(Clone)]
pub struct ReplySender {
    client: Client,
    telegram_bot_token: Option<String>,
    whatsapp_access_token: Option<String>,
    whatsapp_api_version: String,
}

impl ReplySender {
    pub fn new(
        client: Client,
        telegram_bot_token: Option<String>,
        whatsapp_access_token: Option<String>,
        whatsapp_api_version: String,
    ) -> Self {
        Self {
            client,
            telegram_bot_token,
            whatsapp_access_token,
            whatsapp_api_version,
        }
    }

    /// Send a response back to the originating channel.
    pub async fn send(
        &self,
        resolved: &ResolvedMessage,
        response: &AgentResponse,
    ) -> Result<(), AppError> {
        if response.is_empty() {
            tracing::warn!(
                tenant_id = %resolved.tenant_id,
                "empty agent response — nothing to send"
            );
            return Ok(());
        }

        for part in &response.parts {
            match resolved.channel_type {
                ChannelType::Telegram => {
                    self.send_telegram(resolved, part).await?;
                }
                ChannelType::Whatsapp => {
                    self.send_whatsapp(resolved, part).await?;
                }
                ChannelType::WebWidget => {
                    // Web widget responses are returned inline via REST API,
                    // not pushed via a sender. This path should not be hit.
                    tracing::warn!("ReplySender called for web_widget — responses are inline");
                }
            }
        }

        Ok(())
    }

    // ─── Telegram ─────────────────────────────────────────────────────

    async fn send_telegram(
        &self,
        resolved: &ResolvedMessage,
        part: &ResponsePart,
    ) -> Result<(), AppError> {
        let token = self
            .telegram_bot_token
            .as_ref()
            .ok_or_else(|| AppError::Internal("TELEGRAM_BOT_TOKEN not configured".into()))?;

        let base = format!("https://api.telegram.org/bot{token}");
        let chat_id = &resolved.channel_user_id;

        match part {
            ResponsePart::Text { text } => {
                let body = serde_json::json!({
                    "chat_id": chat_id,
                    "text": text,
                    "parse_mode": "Markdown"
                });
                self.post_json(&format!("{base}/sendMessage"), &body)
                    .await?;
            }

            ResponsePart::Media {
                url,
                media_type,
                caption,
            } => {
                let (method, key) = match media_type {
                    crate::types::agent_response::MediaType::Image => ("sendPhoto", "photo"),
                    crate::types::agent_response::MediaType::Video => ("sendVideo", "video"),
                    crate::types::agent_response::MediaType::Audio => ("sendAudio", "audio"),
                    crate::types::agent_response::MediaType::Document => {
                        ("sendDocument", "document")
                    }
                };

                let mut body = serde_json::json!({
                    "chat_id": chat_id,
                    key: url,
                });
                if let Some(cap) = caption {
                    body["caption"] = serde_json::Value::String(cap.clone());
                }
                self.post_json(&format!("{base}/{method}"), &body).await?;
            }

            ResponsePart::QuickReplies { prompt, options } => {
                // Telegram reply keyboard
                let buttons: Vec<Vec<serde_json::Value>> = options
                    .iter()
                    .map(|opt| {
                        vec![serde_json::json!({ "text": opt.label })]
                    })
                    .collect();

                let body = serde_json::json!({
                    "chat_id": chat_id,
                    "text": prompt,
                    "reply_markup": {
                        "keyboard": buttons,
                        "one_time_keyboard": true,
                        "resize_keyboard": true
                    }
                });
                self.post_json(&format!("{base}/sendMessage"), &body)
                    .await?;
            }

            ResponsePart::InteractiveMenu {
                header: _,
                body,
                sections,
            } => {
                // Telegram inline keyboard (grouped by sections)
                let rows: Vec<Vec<serde_json::Value>> = sections
                    .iter()
                    .flat_map(|section| {
                        section.options.iter().map(|opt| {
                            vec![serde_json::json!({
                                "text": opt.title,
                                "callback_data": opt.id
                            })]
                        })
                    })
                    .collect();

                let msg_body = serde_json::json!({
                    "chat_id": chat_id,
                    "text": body,
                    "reply_markup": {
                        "inline_keyboard": rows
                    }
                });
                self.post_json(&format!("{base}/sendMessage"), &msg_body)
                    .await?;
            }
        }

        Ok(())
    }

    // ─── WhatsApp ─────────────────────────────────────────────────────

    async fn send_whatsapp(
        &self,
        resolved: &ResolvedMessage,
        part: &ResponsePart,
    ) -> Result<(), AppError> {
        let access_token = self
            .whatsapp_access_token
            .as_ref()
            .ok_or_else(|| AppError::Internal("WHATSAPP_ACCESS_TOKEN not configured".into()))?;

        // channel_key for WhatsApp is the phone_number_id
        let phone_number_id = &resolved.channel_key;
        let to = &resolved.channel_user_id; // recipient phone number
        let url = format!(
            "https://graph.facebook.com/{}/{phone_number_id}/messages",
            self.whatsapp_api_version
        );

        match part {
            ResponsePart::Text { text } => {
                let body = serde_json::json!({
                    "messaging_product": "whatsapp",
                    "to": to,
                    "type": "text",
                    "text": { "body": text }
                });
                self.post_json_with_bearer(&url, &body, access_token)
                    .await?;
            }

            ResponsePart::Media {
                url: media_url,
                media_type,
                caption,
            } => {
                let wa_type = match media_type {
                    crate::types::agent_response::MediaType::Image => "image",
                    crate::types::agent_response::MediaType::Video => "video",
                    crate::types::agent_response::MediaType::Audio => "audio",
                    crate::types::agent_response::MediaType::Document => "document",
                };

                let mut media_obj = serde_json::json!({ "link": media_url });
                if let Some(cap) = caption {
                    media_obj["caption"] = serde_json::Value::String(cap.clone());
                }

                let body = serde_json::json!({
                    "messaging_product": "whatsapp",
                    "to": to,
                    "type": wa_type,
                    wa_type: media_obj
                });
                self.post_json_with_bearer(&url, &body, access_token)
                    .await?;
            }

            ResponsePart::QuickReplies { prompt, options } => {
                // WhatsApp button message (max 3 buttons)
                let buttons: Vec<serde_json::Value> = options
                    .iter()
                    .take(3) // WhatsApp limit
                    .map(|opt| {
                        serde_json::json!({
                            "type": "reply",
                            "reply": {
                                "id": opt.value,
                                "title": opt.label
                            }
                        })
                    })
                    .collect();

                let body = serde_json::json!({
                    "messaging_product": "whatsapp",
                    "to": to,
                    "type": "interactive",
                    "interactive": {
                        "type": "button",
                        "body": { "text": prompt },
                        "action": { "buttons": buttons }
                    }
                });
                self.post_json_with_bearer(&url, &body, access_token)
                    .await?;
            }

            ResponsePart::InteractiveMenu {
                header,
                body: menu_body,
                sections,
            } => {
                // WhatsApp list picker
                let wa_sections: Vec<serde_json::Value> = sections
                    .iter()
                    .map(|section| {
                        let rows: Vec<serde_json::Value> = section
                            .options
                            .iter()
                            .map(|opt| {
                                let mut row = serde_json::json!({
                                    "id": opt.id,
                                    "title": opt.title
                                });
                                if let Some(desc) = &opt.description {
                                    row["description"] =
                                        serde_json::Value::String(desc.clone());
                                }
                                row
                            })
                            .collect();

                        serde_json::json!({
                            "title": section.title,
                            "rows": rows
                        })
                    })
                    .collect();

                let mut interactive = serde_json::json!({
                    "type": "list",
                    "body": { "text": menu_body },
                    "action": {
                        "button": "View Options",
                        "sections": wa_sections
                    }
                });

                if let Some(h) = header {
                    interactive["header"] = serde_json::json!({
                        "type": "text",
                        "text": h
                    });
                }

                let body = serde_json::json!({
                    "messaging_product": "whatsapp",
                    "to": to,
                    "type": "interactive",
                    "interactive": interactive
                });
                self.post_json_with_bearer(&url, &body, access_token)
                    .await?;
            }
        }

        Ok(())
    }

    // ─── HTTP helpers ─────────────────────────────────────────────────

    async fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<(), AppError> {
        let resp = self
            .client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("channel API unreachable: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let response_body = resp.text().await.unwrap_or_default();
            tracing::error!(
                %url, %status, body = %response_body,
                "channel API returned error"
            );
            return Err(AppError::Downstream(format!(
                "channel API returned {status}"
            )));
        }

        Ok(())
    }

    async fn post_json_with_bearer(
        &self,
        url: &str,
        body: &serde_json::Value,
        token: &str,
    ) -> Result<(), AppError> {
        let resp = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("channel API unreachable: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let response_body = resp.text().await.unwrap_or_default();
            tracing::error!(
                %url, %status, body = %response_body,
                "channel API returned error"
            );
            return Err(AppError::Downstream(format!(
                "channel API returned {status}"
            )));
        }

        Ok(())
    }
}
