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
