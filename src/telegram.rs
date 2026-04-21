use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::gateway::{MetricasClient, TelegramClient, TelegramUpdate};
use crate::hospital::HospitalClient;
use crate::llm::LlmClient;
use crate::runtime::run_turn;
use crate::session::SessionStore;

const POLL_TIMEOUT_SECS: u64 = 30;
const BACKOFF_ON_ERROR: Duration = Duration::from_secs(2);

/// Background worker that long-polls the Telegram Bot API and runs each
/// incoming text message through the orch's LLM + hospital-mock runtime.
/// One chat_id maps to one SessionStore session for the lifetime of the
/// process.
pub struct TelegramLoop {
    telegram: TelegramClient,
    llm: Arc<LlmClient>,
    hospital: Arc<HospitalClient>,
    sessions: Arc<SessionStore>,
    metricas: Option<MetricasClient>,
    default_tenant_id: String,
    chat_sessions: Arc<Mutex<HashMap<i64, String>>>,
}

impl TelegramLoop {
    pub fn new(
        telegram: TelegramClient,
        llm: Arc<LlmClient>,
        hospital: Arc<HospitalClient>,
        sessions: Arc<SessionStore>,
        metricas: Option<MetricasClient>,
        default_tenant_id: String,
    ) -> Self {
        Self {
            telegram,
            llm,
            hospital,
            sessions,
            metricas,
            default_tenant_id,
            chat_sessions: Arc::new(Mutex::new(HashMap::new())),
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

        let sid = {
            let mut guard = self.chat_sessions.lock().await;
            guard
                .entry(chat_id)
                .or_insert_with(SessionStore::new_session_id)
                .clone()
        };

        if let Some(m) = &self.metricas {
            m.record_turn(self.default_tenant_id.clone(), text.clone(), false);
        }

        let (reply, resolved) =
            run_turn(&self.llm, &self.hospital, &self.sessions, &sid, &text).await;

        if resolved {
            if let Some(m) = &self.metricas {
                m.record_turn(self.default_tenant_id.clone(), text.clone(), true);
            }
        }

        let out = if reply.trim().is_empty() {
            "…"
        } else {
            reply.as_str()
        };
        self.telegram.send_message(chat_id, out).await?;
        Ok(())
    }
}
