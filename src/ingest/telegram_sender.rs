// Telegram-specific send utilities.
//
// The main reply sending is handled by gateway::reply_sender (channel-agnostic).
// This module provides Telegram-specific operations like answering callback queries.

use reqwest::Client;

use crate::error::AppError;

/// Answer a Telegram callback query (acknowledge button press).
///
/// If not answered within 30 seconds, Telegram shows a loading indicator.
/// We always answer immediately, even if processing continues in the background.
pub async fn answer_callback_query(
    client: &Client,
    bot_token: &str,
    callback_query_id: &str,
    text: Option<&str>,
) -> Result<(), AppError> {
    let url = format!(
        "https://api.telegram.org/bot{bot_token}/answerCallbackQuery"
    );

    let mut body = serde_json::json!({
        "callback_query_id": callback_query_id,
    });

    if let Some(t) = text {
        body["text"] = serde_json::Value::String(t.to_string());
    }

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Downstream(format!("Telegram API unreachable: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let resp_body = resp.text().await.unwrap_or_default();
        tracing::warn!(%status, body = %resp_body, "answerCallbackQuery failed");
    }

    Ok(())
}
