use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
    user_auth: Option<UserAuthClient>,
    auth_states: Arc<Mutex<HashMap<i64, AuthState>>>,
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
        user_auth: Option<UserAuthClient>,
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
            user_auth,
            auth_states: Arc::new(Mutex::new(HashMap::new())),
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
        let Some(msg) = update.message else {
            return Ok(());
        };
        let chat_id = msg.chat.id;
        let Some(ref text_ref) = msg.text else {
            return Ok(());
        };
        let text = text_ref.trim().to_string();
        if text.is_empty() {
            return Ok(());
        }

        // OTP pre-registration: when User-Auth is wired, every message goes
        // through the gate first. Returns Ok(true) → message absorbed, no
        // LLM call. Returns Ok(false) → user is already authenticated,
        // continue to the LLM path below.
        if self.user_auth.is_some() {
            let handled = self.run_pre_registration(&msg, &text).await?;
            if handled {
                return Ok(());
            }
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
            Ok(Ok(Some(reply))) => {
                let out = if reply.trim().is_empty() {
                    "…"
                } else {
                    &reply
                };
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
                            let out = if reply.trim().is_empty() {
                                "…"
                            } else {
                                &reply
                            };
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

        let out = if reply.trim().is_empty() {
            "…"
        } else {
            reply.as_str()
        };
        self.telegram.send_message(chat_id, out).await?;
        Ok(())
    }
}

/// Cheap heuristic: any string containing "@" with at least one char on
/// each side and a "." somewhere after the "@". Good enough for the OTP
/// gate; the real validator lives in User-Auth (Postgres unique
/// constraint on the column).
fn looks_like_email(s: &str) -> bool {
    let s = s.trim();
    let at = match s.find('@') {
        Some(i) => i,
        None => return false,
    };
    if at == 0 || at == s.len() - 1 {
        return false;
    }
    s[at + 1..].contains('.')
}
