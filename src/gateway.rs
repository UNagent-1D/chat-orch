use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Clone)]
pub struct ConversationChatClient {
    http: Client,
    base_url: String,
}

#[derive(Clone)]
pub struct MetricasClient {
    http: Client,
    base_url: String,
}

#[derive(Clone)]
pub struct TelegramClient {
    http: Client,
    base_url: String,
}

#[derive(Debug, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramMessage {
    pub chat: TelegramChat,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
struct GetUpdatesResponse {
    ok: bool,
    #[serde(default)]
    result: Vec<TelegramUpdate>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Serialize)]
struct SendMessageBody<'a> {
    chat_id: i64,
    text: &'a str,
}

impl TelegramClient {
    pub fn new(http: Client, bot_token: &str) -> Self {
        Self {
            http,
            base_url: format!("https://api.telegram.org/bot{bot_token}"),
        }
    }

    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout_secs: u64,
    ) -> Result<Vec<TelegramUpdate>, AppError> {
        let url = format!("{}/getUpdates", self.base_url);
        let mut req = self.http.get(&url).query(&[("timeout", timeout_secs)]);
        if let Some(offset) = offset {
            req = req.query(&[("offset", offset)]);
        }
        // Long-poll budget: allow a little extra over the server-side timeout.
        let response = req
            .timeout(std::time::Duration::from_secs(timeout_secs + 10))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "telegram getUpdates {status}: {}",
                body.chars().take(200).collect::<String>()
            )));
        }
        let parsed: GetUpdatesResponse = response.json().await?;
        if !parsed.ok {
            return Err(AppError::Downstream(format!(
                "telegram getUpdates not ok: {}",
                parsed.description.unwrap_or_default()
            )));
        }
        Ok(parsed.result)
    }

    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), AppError> {
        let url = format!("{}/sendMessage", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(&SendMessageBody { chat_id, text })
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "telegram sendMessage {status}: {}",
                body.chars().take(200).collect::<String>()
            )));
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct MetricasChatBody<'a> {
    message: &'a str,
    resolved: bool,
}

impl MetricasClient {
    pub fn new(http: Client, base_url: String) -> Self {
        Self { http, base_url }
    }

    /// Fire-and-forget — logs a warning on failure and never returns an error.
    /// Spawned onto the tokio runtime so request latency is unaffected.
    pub fn record_turn(&self, tenant_id: String, message: String, resolved: bool) {
        let http = self.http.clone();
        let url = format!(
            "{}/conversation/chat",
            self.base_url.trim_end_matches('/')
        );
        tokio::spawn(async move {
            let result = http
                .post(&url)
                .header("X-Tenant-ID", &tenant_id)
                .json(&MetricasChatBody { message: &message, resolved })
                .send()
                .await;
            match result {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) => tracing::warn!(status=%resp.status(), %url, "metricas emit non-2xx"),
                Err(err) => tracing::warn!(error=%err, %url, "metricas emit failed"),
            }
        });
    }

    /// Fire-and-forget CSAT feedback emission.
    pub fn record_feedback(&self, tenant_id: String, score: u8) {
        let http = self.http.clone();
        let url = format!("{}/feedback/csat", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({ "score": score });
        tokio::spawn(async move {
            let result = http
                .post(&url)
                .header("X-Tenant-ID", &tenant_id)
                .json(&body)
                .send()
                .await;
            match result {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) => tracing::warn!(status=%resp.status(), %url, "metricas feedback non-2xx"),
                Err(err) => tracing::warn!(error=%err, %url, "metricas feedback failed"),
            }
        });
    }
}

#[derive(Serialize)]
struct CreateSessionBody<'a> {
    tenant_id: &'a str,
}

#[derive(Deserialize)]
struct CreateSessionResponse {
    sid: String,
}

#[derive(Serialize)]
struct TurnBody<'a> {
    message: &'a str,
}

impl ConversationChatClient {
    pub fn new(http: Client, base_url: String) -> Self {
        Self { http, base_url }
    }

    pub async fn create_session(&self, tenant_id: &str) -> Result<String, AppError> {
        let url = format!("{}/api/v1/sessions", self.base_url.trim_end_matches('/'));
        let response = self
            .http
            .post(&url)
            .bearer_auth("internal")
            .json(&CreateSessionBody { tenant_id })
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "POST {url} returned {status}: {}",
                body.chars().take(200).collect::<String>()
            )));
        }

        let parsed: CreateSessionResponse = response.json().await?;
        Ok(parsed.sid)
    }

    pub async fn post_turn(
        &self,
        sid: &str,
        message: &str,
    ) -> Result<serde_json::Value, AppError> {
        let url = format!(
            "{}/api/v1/sessions/{sid}/turns",
            self.base_url.trim_end_matches('/')
        );
        let response = self
            .http
            .post(&url)
            .bearer_auth("internal")
            .json(&TurnBody { message })
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "POST {url} returned {status}: {}",
                body.chars().take(200).collect::<String>()
            )));
        }

        let body: serde_json::Value = response.json().await?;
        Ok(body)
    }
}
