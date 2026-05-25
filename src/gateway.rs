use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::channel::Channel;
use crate::error::AppError;

#[derive(Clone)]
pub struct ConversationChatClient {
    http: Client,
    base_url: String,
    channel: Channel,
}

#[derive(Clone)]
pub struct MetricasClient {
    http: Client,
    base_url: String,
    channel: Channel,
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
    /// Inline-keyboard button presses (e.g. CSAT star ratings).
    #[serde(default)]
    pub callback_query: Option<TelegramCallbackQuery>,
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
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

/// Incoming callback_query: fired when the user presses an inline-keyboard button.
#[derive(Debug, Deserialize)]
pub struct TelegramCallbackQuery {
    /// Unique identifier for the query — must be acknowledged via answerCallbackQuery.
    pub id: String,
    /// User who pressed the button.
    pub from: TelegramUser,
    /// The message that carried the inline keyboard, if available.
    #[serde(default)]
    pub message: Option<TelegramMessage>,
    /// Payload set in the button's `callback_data` field.
    #[serde(default)]
    pub data: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramUser {
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

/// sendMessage body with an inline keyboard attached.
#[derive(Serialize)]
struct SendMessageWithKeyboardBody<'a> {
    chat_id: i64,
    text: &'a str,
    reply_markup: InlineKeyboardMarkup,
}

#[derive(Serialize)]
struct InlineKeyboardMarkup {
    inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
}

#[derive(Serialize)]
struct InlineKeyboardButton {
    text: String,
    callback_data: String,
}

/// answerCallbackQuery body — required to dismiss the loading spinner in Telegram.
#[derive(Serialize)]
struct AnswerCallbackQueryBody<'a> {
    callback_query_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
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

    /// Sends a CSAT prompt with an inline keyboard of five star-rating buttons.
    /// The callback_data for each button is `"csat:<score>"` (e.g. `"csat:3"`).
    pub async fn send_csat_prompt(&self, chat_id: i64) -> Result<(), AppError> {
        let url = format!("{}/sendMessage", self.base_url);
        let body = SendMessageWithKeyboardBody {
            chat_id,
            text: "¿Cómo calificarías tu atención hoy? (1 = muy mala, 5 = excelente)",
            reply_markup: InlineKeyboardMarkup {
                inline_keyboard: vec![vec![
                    InlineKeyboardButton {
                        text: "⭐ 1".into(),
                        callback_data: "csat:1".into(),
                    },
                    InlineKeyboardButton {
                        text: "⭐⭐ 2".into(),
                        callback_data: "csat:2".into(),
                    },
                    InlineKeyboardButton {
                        text: "⭐⭐⭐ 3".into(),
                        callback_data: "csat:3".into(),
                    },
                    InlineKeyboardButton {
                        text: "⭐⭐⭐⭐ 4".into(),
                        callback_data: "csat:4".into(),
                    },
                    InlineKeyboardButton {
                        text: "⭐⭐⭐⭐⭐ 5".into(),
                        callback_data: "csat:5".into(),
                    },
                ]],
            },
        };
        let response = self.http.post(&url).json(&body).send().await?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            tracing::warn!(
                %status, body = %text.chars().take(200).collect::<String>(),
                "telegram send_csat_prompt non-2xx"
            );
        }
        Ok(())
    }

    /// Acknowledges an inline-keyboard button press, removing the loading spinner.
    /// Must be called within 10 s of the callback_query arriving.
    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> Result<(), AppError> {
        let url = format!("{}/answerCallbackQuery", self.base_url);
        let body = AnswerCallbackQueryBody {
            callback_query_id,
            text,
        };
        let response = self.http.post(&url).json(&body).send().await?;
        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            tracing::warn!(
                %status, body = %body_text.chars().take(200).collect::<String>(),
                "telegram answerCallbackQuery non-2xx"
            );
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
    pub fn new(http: Client, base_url: String, channel: Channel) -> Self {
        Self {
            http,
            base_url,
            channel,
        }
    }

    /// Fire-and-forget — logs a warning on failure and never returns an error.
    /// Spawned onto the tokio runtime so request latency is unaffected.
    pub fn record_turn(&self, tenant_id: String, message: String, resolved: bool) {
        let http = self.http.clone();
        let channel = self.channel.clone();
        let url = format!("{}/conversation/chat", self.base_url.trim_end_matches('/'));
        tokio::spawn(async move {
            let body = MetricasChatBody {
                message: &message,
                resolved,
            };
            let req = http.post(&url).header("X-Tenant-ID", &tenant_id);
            let req = match channel.apply_request(req, &body) {
                Ok(r) => r,
                Err(err) => {
                    tracing::warn!(error=%err, %url, "metricas emit seal failed");
                    return;
                }
            };
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) => tracing::warn!(status=%resp.status(), %url, "metricas emit non-2xx"),
                Err(err) => tracing::warn!(error=%err, %url, "metricas emit failed"),
            }
        });
    }

    /// Fire-and-forget audit emission for a rate-limit rejection.
    ///
    /// Posts to Compliance `/v1/event` so the 429 is persisted in the
    /// `audit_logs` collection. Used by the chat-orch handlers when the
    /// token bucket denies a request. Failures are logged and dropped:
    /// audit telemetry must never block the request path.
    pub fn record_rate_limit(&self, tenant_id: String, scope: &'static str, retry_after_secs: u64) {
        let http = self.http.clone();
        let channel = self.channel.clone();
        let url = format!("{}/v1/event", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "level": "WARN",
            "tenant_id": tenant_id,
            "component": "chat-orch",
            "action": "RATE_LIMITED",
            "metadata": {
                "scope": scope,
                "retry_after_secs": retry_after_secs,
            }
        });
        tokio::spawn(async move {
            let req = http.post(&url);
            let req = match channel.apply_request(req, &body) {
                Ok(r) => r,
                Err(err) => {
                    tracing::warn!(error=%err, %url, "compliance rate-limit seal failed");
                    return;
                }
            };
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) => {
                    tracing::warn!(status=%resp.status(), %url, "compliance rate-limit non-2xx")
                }
                Err(err) => tracing::warn!(error=%err, %url, "compliance rate-limit failed"),
            }
        });
    }

    /// Fire-and-forget CSAT feedback emission.
    pub fn record_feedback(&self, tenant_id: String, score: u8) {
        let http = self.http.clone();
        let channel = self.channel.clone();
        let url = format!("{}/feedback/csat", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({ "score": score });
        tokio::spawn(async move {
            let req = http.post(&url).header("X-Tenant-ID", &tenant_id);
            let req = match channel.apply_request(req, &body) {
                Ok(r) => r,
                Err(err) => {
                    tracing::warn!(error=%err, %url, "metricas feedback seal failed");
                    return;
                }
            };
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) => {
                    tracing::warn!(status=%resp.status(), %url, "metricas feedback non-2xx")
                }
                Err(err) => tracing::warn!(error=%err, %url, "metricas feedback failed"),
            }
        });
    }
}

#[derive(Serialize)]
struct CreateSessionBody<'a> {
    tenant_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    contact_email: Option<&'a str>,
}

#[derive(Deserialize)]
struct CreateSessionResponse {
    sid: String,
}

#[derive(Serialize)]
struct TurnBody<'a> {
    message: &'a str,
}

#[derive(Serialize)]
struct SubmitJobBody<'a> {
    session_id: &'a str,
    message: &'a str,
    chat_id: i64,
    tenant_id: &'a str,
}

#[derive(Deserialize)]
struct SubmitJobResponse {
    job_id: String,
}

#[derive(Deserialize)]
struct JobResultResponse {
    text: String,
}

impl ConversationChatClient {
    pub fn new(http: Client, base_url: String, channel: Channel) -> Self {
        Self {
            http,
            base_url,
            channel,
        }
    }

    pub async fn create_session(
        &self,
        tenant_id: &str,
        contact_email: Option<&str>,
    ) -> Result<String, AppError> {
        let url = format!("{}/api/v1/sessions", self.base_url.trim_end_matches('/'));
        let req = self.http.post(&url).bearer_auth("internal");
        let req = self.channel.apply_request(
            req,
            &CreateSessionBody {
                tenant_id,
                contact_email,
            },
        )?;
        let response = req.send().await?;
        let parsed: CreateSessionResponse = self.channel.decode_response(response).await?;
        Ok(parsed.sid)
    }

    pub async fn post_turn(&self, sid: &str, message: &str) -> Result<serde_json::Value, AppError> {
        let url = format!(
            "{}/api/v1/sessions/{sid}/turns",
            self.base_url.trim_end_matches('/')
        );
        let req = self.http.post(&url).bearer_auth("internal");
        let req = self.channel.apply_request(req, &TurnBody { message })?;
        let response = req.send().await?;
        let body: serde_json::Value = self.channel.decode_response(response).await?;
        Ok(body)
    }

    /// Submits an async job to the broker via agent-runtime.
    /// Returns the job_id assigned to this request.
    pub async fn submit_job(
        &self,
        session_id: &str,
        message: &str,
        chat_id: i64,
        tenant_id: &str,
    ) -> Result<String, AppError> {
        let url = format!("{}/api/v1/jobs", self.base_url.trim_end_matches('/'));
        let req = self.http.post(&url).bearer_auth("internal");
        let req = self.channel.apply_request(
            req,
            &SubmitJobBody {
                session_id,
                message,
                chat_id,
                tenant_id,
            },
        )?;
        let response = req.send().await?;
        let parsed: SubmitJobResponse = self.channel.decode_response(response).await?;
        Ok(parsed.job_id)
    }

    /// Long-polls agent-runtime for a job result.
    /// Returns `Some(text)` when the result is ready, or `None` if the
    /// agent-runtime returned 408 (timeout elapsed on its side).
    pub async fn wait_for_job(
        &self,
        job_id: &str,
        timeout_ms: u64,
    ) -> Result<Option<String>, AppError> {
        let url = format!(
            "{}/api/v1/jobs/{job_id}/wait",
            self.base_url.trim_end_matches('/')
        );
        // GET — no body to seal, but we still tag the request with the secure
        // channel header so the callee knows to encrypt its response. Client-
        // side timeout has a 5 s buffer above the server-side one so we never
        // abandon a result mid-flight.
        let mut req = self
            .http
            .get(&url)
            .query(&[("timeout", timeout_ms)])
            .bearer_auth("internal")
            .timeout(std::time::Duration::from_millis(timeout_ms + 5_000));
        if self.channel.active() {
            req = req.header(crate::channel::HEADER_NAME, crate::channel::HEADER_VALUE);
        }
        let response = req.send().await?;

        if response.status() == reqwest::StatusCode::REQUEST_TIMEOUT {
            return Ok(None);
        }
        let parsed: JobResultResponse = self.channel.decode_response(response).await?;
        Ok(Some(parsed.text))
    }
}
