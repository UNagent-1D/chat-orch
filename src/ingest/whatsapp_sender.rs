// WhatsApp-specific send utilities.
//
// The main reply sending is handled by gateway::reply_sender (channel-agnostic).
// This module provides WhatsApp-specific operations like marking messages as read.

use reqwest::Client;

use crate::error::AppError;

/// Mark a message as "read" in WhatsApp.
///
/// Sends the blue checkmarks to the user, indicating their message was seen.
/// Optional — can be called after processing starts.
pub async fn mark_as_read(
    client: &Client,
    access_token: &str,
    api_version: &str,
    phone_number_id: &str,
    message_id: &str,
) -> Result<(), AppError> {
    let url = format!(
        "https://graph.facebook.com/{api_version}/{phone_number_id}/messages"
    );

    let body = serde_json::json!({
        "messaging_product": "whatsapp",
        "status": "read",
        "message_id": message_id
    });

    let resp = client
        .post(&url)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Downstream(format!("WhatsApp API unreachable: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let resp_body = resp.text().await.unwrap_or_default();
        tracing::warn!(%status, body = %resp_body, "mark_as_read failed");
    }

    Ok(())
}
