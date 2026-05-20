use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::gateway::{ConversationChatClient, MetricasClient, TelegramClient, TelegramUpdate};
use crate::hospital::HospitalClient;
use crate::llm::LlmClient;
use crate::runtime::run_turn;
use crate::session::SessionStore;

const POLL_TIMEOUT_SECS: u64 = 30;
const BACKOFF_ON_ERROR: Duration = Duration::from_secs(2);

/// How long chat-orch waits for an immediate LLM reply before sending
/// the "working..." message and switching to background delivery.
const FAST_REPLY_TIMEOUT_MS: u64 = 5_000;

/// How long the background task waits for the final reply after the
/// "working..." message has been sent.
const BACKGROUND_WAIT_TIMEOUT_MS: u64 = 120_000;

const WORKING_MESSAGE: &str =
    "Estamos trabajando para responder tu solicitud, por favor permanece en línea.";

/// Reply to the /start (or /reset) command. Clearing the chat's cached
/// session means the next message opens a fresh conversation.
const START_MESSAGE: &str = "Hola, ¿en qué puedo ayudarte hoy?";

/// How often the outbound loop polls conversation-chat for operator
/// messages waiting to be delivered to Telegram users.
const OUTBOUND_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Background worker that long-polls the Telegram Bot API and runs each
/// incoming text message through the orch's LLM + hospital-mock runtime.
///
/// When `agent_runtime` is set, messages are dispatched asynchronously via
/// RabbitMQ (through agent-runtime). A 5-second timeout determines whether
/// the reply is sent immediately or after a "working…" acknowledgement.
///
/// When `agent_runtime` is None, the original synchronous `run_turn()` path
/// is used as a fallback.
pub struct TelegramLoop {
    telegram: TelegramClient,
    llm: Arc<LlmClient>,
    hospital: Arc<HospitalClient>,
    sessions: Arc<SessionStore>,
    metricas: Option<MetricasClient>,
    default_tenant_id: String,
    chat_sessions: Arc<Mutex<HashMap<i64, String>>>,
    agent_runtime: Option<Arc<ConversationChatClient>>,
    http: reqwest::Client,
    conversation_chat_url: String,
}

impl TelegramLoop {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        telegram: TelegramClient,
        llm: Arc<LlmClient>,
        hospital: Arc<HospitalClient>,
        sessions: Arc<SessionStore>,
        metricas: Option<MetricasClient>,
        default_tenant_id: String,
        agent_runtime: Option<Arc<ConversationChatClient>>,
        http: reqwest::Client,
        conversation_chat_url: String,
    ) -> Self {
        Self {
            telegram,
            llm,
            hospital,
            sessions,
            metricas,
            default_tenant_id,
            chat_sessions: Arc::new(Mutex::new(HashMap::new())),
            agent_runtime,
            http,
            conversation_chat_url,
        }
    }

    pub fn spawn(self) {
        // Operator-message delivery: poll conversation-chat for replies a
        // human operator typed and push them to the Telegram user. Only
        // meaningful on the async path (operator handoff lives there).
        if self.agent_runtime.is_some() {
            let telegram = self.telegram.clone();
            let chat_sessions = self.chat_sessions.clone();
            let http = self.http.clone();
            let url = self.conversation_chat_url.clone();
            tokio::spawn(async move {
                outbound_loop(telegram, chat_sessions, http, url).await;
            });
        }

        tokio::spawn(async move {
            tracing::info!(
                tenant = %self.default_tenant_id,
                "telegram loop started",
            );
            self.run().await;
        });
    }

    async fn run(self) {
        let mut offset: Option<i64> = None;
        loop {
            match self.telegram.get_updates(offset, POLL_TIMEOUT_SECS).await {
                Ok(updates) => {
                    for update in updates {
                        offset = Some(update.update_id + 1);
                        if let Err(err) = self.handle_update(update).await {
                            tracing::warn!(error=%err, "telegram update handling failed");
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(error=%err, "telegram getUpdates failed, backing off");
                    tokio::time::sleep(BACKOFF_ON_ERROR).await;
                }
            }
        }
    }

    async fn handle_update(&self, update: TelegramUpdate) -> Result<(), crate::error::AppError> {
        let Some(msg) = update.message else { return Ok(()); };
        let chat_id = msg.chat.id;
        let Some(text) = msg.text else { return Ok(()); };
        if text.trim().is_empty() {
            return Ok(());
        }

        // /start and /reset drop the cached session so the next message
        // opens a fresh conversation instead of reusing a stale one.
        let trimmed = text.trim();
        if trimmed == "/start" || trimmed == "/reset" {
            self.chat_sessions.lock().await.remove(&chat_id);
            self.telegram.send_message(chat_id, START_MESSAGE).await?;
            return Ok(());
        }

        if let Some(ar) = &self.agent_runtime {
            self.handle_update_async(ar.clone(), chat_id, text).await
        } else {
            self.handle_update_sync(chat_id, &text).await
        }
    }

    /// Async path: dispatch job through agent-runtime → RabbitMQ → conversation-chat.
    async fn handle_update_async(
        &self,
        ar: Arc<ConversationChatClient>,
        chat_id: i64,
        text: String,
    ) -> Result<(), crate::error::AppError> {
        if let Some(m) = &self.metricas {
            m.record_turn(self.default_tenant_id.clone(), text.clone(), false);
        }

        // Get or create a conversation-chat session for this chat_id.
        let session_id = {
            let mut guard = self.chat_sessions.lock().await;
            if let Some(sid) = guard.get(&chat_id) {
                sid.clone()
            } else {
                let sid = ar.create_session(&self.default_tenant_id).await?;
                guard.insert(chat_id, sid.clone());
                sid
            }
        };

        let job_id = ar
            .submit_job(&session_id, &text, chat_id, &self.default_tenant_id)
            .await?;

        // Wait up to FAST_REPLY_TIMEOUT_MS for an immediate response.
        let immediate = tokio::time::timeout(
            Duration::from_millis(FAST_REPLY_TIMEOUT_MS),
            ar.wait_for_job(&job_id, FAST_REPLY_TIMEOUT_MS),
        )
        .await;

        match immediate {
            // Result arrived within the fast window — send directly.
            // An empty reply is intentional (e.g. operator handoff pending);
            // send nothing rather than a bare placeholder.
            Ok(Ok(Some(reply))) => {
                if !reply.trim().is_empty() {
                    self.telegram.send_message(chat_id, &reply).await?;
                }
            }

            // Timeout or error — send "working…" and wait in background.
            _ => {
                self.telegram.send_message(chat_id, WORKING_MESSAGE).await?;

                let ar2 = ar.clone();
                let tg = self.telegram.clone();
                let jid = job_id.clone();
                tokio::spawn(async move {
                    match ar2.wait_for_job(&jid, BACKGROUND_WAIT_TIMEOUT_MS).await {
                        Ok(Some(reply)) => {
                            if !reply.trim().is_empty() {
                                if let Err(e) = tg.send_message(chat_id, &reply).await {
                                    tracing::warn!(error=%e, "background telegram send failed");
                                }
                            }
                        }
                        Ok(None) => {
                            tracing::warn!(job_id=%jid, "background wait timed out — no reply sent");
                        }
                        Err(e) => {
                            tracing::warn!(error=%e, "background wait_for_job error");
                        }
                    }
                });
            }
        }

        Ok(())
    }

    /// Synchronous fallback path: calls run_turn() directly (no broker).
    async fn handle_update_sync(
        &self,
        chat_id: i64,
        text: &str,
    ) -> Result<(), crate::error::AppError> {
        let sid = {
            let mut guard = self.chat_sessions.lock().await;
            guard
                .entry(chat_id)
                .or_insert_with(SessionStore::new_session_id)
                .clone()
        };

        if let Some(m) = &self.metricas {
            m.record_turn(self.default_tenant_id.clone(), text.to_string(), false);
        }

        let (reply, resolved) =
            run_turn(&self.llm, &self.hospital, &self.sessions, &sid, text).await;

        if resolved {
            if let Some(m) = &self.metricas {
                m.record_turn(self.default_tenant_id.clone(), text.to_string(), true);
            }
        }

        if !reply.trim().is_empty() {
            self.telegram.send_message(chat_id, reply.as_str()).await?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct OutboundDrainResponse {
    #[serde(default)]
    messages: HashMap<String, Vec<String>>,
}

/// Polls conversation-chat for operator messages and delivers each one to the
/// matching Telegram chat. conversation-chat owns the operator-handoff state;
/// chat-orch owns the `chat_id -> session_id` map, so delivery happens here.
async fn outbound_loop(
    telegram: TelegramClient,
    chat_sessions: Arc<Mutex<HashMap<i64, String>>>,
    http: reqwest::Client,
    conversation_chat_url: String,
) {
    let url = format!(
        "{}/api/v1/outbound/drain",
        conversation_chat_url.trim_end_matches('/')
    );
    tracing::info!(%url, "telegram outbound loop started");

    loop {
        tokio::time::sleep(OUTBOUND_POLL_INTERVAL).await;

        let pairs: Vec<(i64, String)> = {
            let guard = chat_sessions.lock().await;
            guard.iter().map(|(k, v)| (*k, v.clone())).collect()
        };
        if pairs.is_empty() {
            continue;
        }

        let session_ids: Vec<&String> = pairs.iter().map(|(_, sid)| sid).collect();
        let response = match http
            .post(&url)
            .json(&serde_json::json!({ "session_ids": session_ids }))
            .send()
            .await
        {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(error=%err, "outbound drain request failed");
                continue;
            }
        };

        let body: OutboundDrainResponse = match response.json().await {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(error=%err, "outbound drain decode failed");
                continue;
            }
        };

        for (chat_id, sid) in &pairs {
            let Some(msgs) = body.messages.get(sid) else {
                continue;
            };
            for msg in msgs {
                if let Err(err) = telegram.send_message(*chat_id, msg).await {
                    tracing::warn!(error=%err, %sid, "outbound telegram send failed");
                }
            }
        }
    }
}
