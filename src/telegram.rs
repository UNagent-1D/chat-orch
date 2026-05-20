use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
const FAST_REPLY_TIMEOUT_MS: u64 = 30_000;

/// How long the background task waits for the final reply after the
/// "working..." message has been sent.
const BACKGROUND_WAIT_TIMEOUT_MS: u64 = 120_000;

const WORKING_MESSAGE: &str =
    "Estamos trabajando para responder tu solicitud, por favor permanece en línea.";

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
}

impl TelegramLoop {
    pub fn new(
        telegram: TelegramClient,
        llm: Arc<LlmClient>,
        hospital: Arc<HospitalClient>,
        sessions: Arc<SessionStore>,
        metricas: Option<MetricasClient>,
        default_tenant_id: String,
        agent_runtime: Option<Arc<ConversationChatClient>>,
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
        }
    }

    pub fn spawn(self) {
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
            Ok(Ok(Some(reply))) => {
                let out = if reply.trim().is_empty() { "…" } else { &reply };
                self.telegram.send_message(chat_id, out).await?;
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
                            let out = if reply.trim().is_empty() { "…" } else { &reply };
                            if let Err(e) = tg.send_message(chat_id, out).await {
                                tracing::warn!(error=%e, "background telegram send failed");
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

        let out = if reply.trim().is_empty() { "…" } else { reply.as_str() };
        self.telegram.send_message(chat_id, out).await?;
        Ok(())
    }
}
