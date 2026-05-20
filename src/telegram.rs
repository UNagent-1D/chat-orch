use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::gateway::{
    ConversationChatClient, MetricasClient, TelegramClient, TelegramMessage, TelegramUpdate,
};
use crate::hospital::HospitalClient;
use crate::llm::LlmClient;
use crate::runtime::run_turn;
use crate::session::SessionStore;
use crate::user_auth::{CreateUserBody, UserAuthClient};

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
/// Per-chat authentication state for the OTP pre-registration flow.
/// When `user_auth` is set on TelegramLoop, every incoming message is
/// gated by this state machine before reaching the LLM.
#[derive(Debug, Clone)]
enum AuthState {
    /// First contact OR explicit reset. Bot prompts for email next.
    AwaitingEmail,
    /// Bot has emailed the OTP, waiting for the user to type 6 digits.
    AwaitingOtp,
    /// OTP verified, session JWT obtained. Pass-through to the LLM.
    Authenticated,
}

pub struct TelegramLoop {
    telegram: TelegramClient,
    llm: Arc<LlmClient>,
    hospital: Arc<HospitalClient>,
    sessions: Arc<SessionStore>,
    metricas: Option<MetricasClient>,
    default_tenant_id: String,
    default_tenant_slug: String,
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
        default_tenant_slug: String,
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
            default_tenant_slug,
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

    /// Pre-registration state machine. Returns:
    /// - Ok(true)  → message was absorbed by the flow; do not invoke the LLM.
    /// - Ok(false) → user is already Authenticated; fall through to LLM.
    async fn run_pre_registration(
        &self,
        msg: &TelegramMessage,
        text: &str,
    ) -> Result<bool, crate::error::AppError> {
        let chat_id = msg.chat.id;
        let ua = match &self.user_auth {
            Some(c) => c,
            None => return Ok(false),
        };

        let state = {
            let guard = self.auth_states.lock().await;
            guard.get(&chat_id).cloned()
        };

        match state {
            Some(AuthState::Authenticated) => Ok(false),

            Some(AuthState::AwaitingOtp) => {
                let code = text.trim();
                let is_six_digits = code.len() == 6 && code.chars().all(|c| c.is_ascii_digit());
                if !is_six_digits {
                    if code.eq_ignore_ascii_case("correo") || code.eq_ignore_ascii_case("email") {
                        // Resend code to the same document (chat_id).
                        let doc = chat_id.to_string();
                        match ua.request_code(&doc).await {
                            Ok(()) => {
                                self.telegram
                                    .send_message(chat_id, "Te reenvié el código.")
                                    .await?;
                            }
                            Err(e) => {
                                tracing::warn!(error=%e, "request-code resend failed");
                                self.telegram
                                    .send_message(chat_id, "No pude reenviar el código.")
                                    .await?;
                            }
                        }
                    } else {
                        self.telegram
                            .send_message(
                                chat_id,
                                "Espero un código de 6 dígitos. Escribe \"correo\" si quieres que te lo reenvíe.",
                            )
                            .await?;
                    }
                    return Ok(true);
                }

                let doc = chat_id.to_string();
                match ua.verify_code(&doc, code).await {
                    Ok(_) => {
                        self.auth_states
                            .lock()
                            .await
                            .insert(chat_id, AuthState::Authenticated);
                        self.telegram
                            .send_message(
                                chat_id,
                                "¡Listo! Cuéntame en qué puedo ayudarte.",
                            )
                            .await?;
                    }
                    Err(e) => {
                        tracing::info!(error=%e, "verify-code failed");
                        self.telegram
                            .send_message(
                                chat_id,
                                "Código inválido o expirado. Intenta otro, o escribe \"correo\" para reenviarlo.",
                            )
                            .await?;
                    }
                }
                Ok(true)
            }

            // None or Some(AwaitingEmail) → expect email-shaped text.
            _ => {
                if !looks_like_email(text) {
                    self.auth_states
                        .lock()
                        .await
                        .insert(chat_id, AuthState::AwaitingEmail);
                    self.telegram
                        .send_message(
                            chat_id,
                            "¡Hola! Antes de empezar, ¿cuál es tu correo electrónico?",
                        )
                        .await?;
                    return Ok(true);
                }

                let doc = chat_id.to_string();
                let first_name = msg.chat.first_name.clone().unwrap_or_default();
                let last_name = msg.chat.last_name.clone().unwrap_or_default();
                let display_first = if first_name.is_empty() {
                    "Usuario"
                } else {
                    first_name.as_str()
                };
                let display_last = if last_name.is_empty() {
                    "Telegram"
                } else {
                    last_name.as_str()
                };

                // Create user (idempotent on conflict).
                if let Err(e) = ua
                    .create_user(&CreateUserBody {
                        tenant_id: &self.default_tenant_id,
                        tenant_slug: &self.default_tenant_slug,
                        user_name: display_first,
                        user_last_name: display_last,
                        user_document: &doc,
                        user_email: text,
                    })
                    .await
                {
                    tracing::warn!(error=%e, "user-auth create_user failed");
                    self.telegram
                        .send_message(
                            chat_id,
                            "Hubo un problema registrando tu correo. Intenta de nuevo en un momento.",
                        )
                        .await?;
                    return Ok(true);
                }

                if let Err(e) = ua.request_code(&doc).await {
                    tracing::warn!(error=%e, "user-auth request-code failed");
                    self.telegram
                        .send_message(
                            chat_id,
                            "Te registramos, pero no pudimos enviar el código. Intenta de nuevo en un momento.",
                        )
                        .await?;
                    return Ok(true);
                }

                self.auth_states
                    .lock()
                    .await
                    .insert(chat_id, AuthState::AwaitingOtp);
                self.telegram
                    .send_message(
                        chat_id,
                        "Te envié un código de 6 dígitos a tu correo. Pégalo aquí. (Caduca en 5 minutos.)",
                    )
                    .await?;
                Ok(true)
            }
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
