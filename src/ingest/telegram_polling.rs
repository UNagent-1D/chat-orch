// Telegram long-polling fallback for local development.
//
// When `TELEGRAM_USE_POLLING=true`, this module spawns a background task that
// calls `getUpdates` in a loop instead of relying on webhooks.
// This avoids the need for HTTPS / a public domain during development.
//
// NOT for production use — webhooks are more efficient and lower-latency.

use std::sync::Arc;

use reqwest::Client;
use serde::Deserialize;

use crate::state::AppState;

use super::telegram_types::TelegramUpdate;

/// Response from `getUpdates`.
#[derive(Debug, Deserialize)]
struct GetUpdatesResponse {
    ok: bool,
    result: Vec<TelegramUpdate>,
}

/// Start the long-polling loop in a background task.
///
/// Call this from `main.rs` when `TELEGRAM_USE_POLLING=true`.
pub async fn start_polling(app: Arc<AppState>) {
    let bot_token = match &app.config.telegram_bot_token {
        Some(token) => token.clone(),
        None => {
            tracing::warn!("TELEGRAM_USE_POLLING=true but TELEGRAM_BOT_TOKEN not set — skipping");
            return;
        }
    };

    let client = reqwest::Client::new();
    let url = format!("https://api.telegram.org/bot{bot_token}/getUpdates");

    tracing::info!("starting Telegram long-polling");

    tokio::spawn(async move {
        let mut offset: i64 = 0;

        loop {
            match poll_once(&client, &url, offset).await {
                Ok(updates) => {
                    for update in updates {
                        let next_offset = update.update_id + 1;
                        if next_offset > offset {
                            offset = next_offset;
                        }

                        // Serialize the update and inject it as if it were a webhook
                        // For simplicity in polling mode, we process inline (not via HTTP)
                        tracing::debug!(update_id = update.update_id, "polled Telegram update");

                        // TODO: Process the update through the same normalization path
                        // For now, this is a placeholder — full implementation would call
                        // the same normalize + pipeline logic from telegram.rs
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Telegram polling failed — retrying in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    });
}

async fn poll_once(
    client: &Client,
    url: &str,
    offset: i64,
) -> Result<Vec<TelegramUpdate>, Box<dyn std::error::Error + Send + Sync>> {
    let resp = client
        .get(url)
        .query(&[
            ("offset", offset.to_string()),
            ("timeout", "30".to_string()),
        ])
        .send()
        .await?;

    let data: GetUpdatesResponse = resp.json().await?;

    if !data.ok {
        return Err("Telegram getUpdates returned ok=false".into());
    }

    Ok(data.result)
}
