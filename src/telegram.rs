use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::gateway::{
    ConversationChatClient, MetricasClient, TelegramCallbackQuery, TelegramClient, TelegramMessage,
    TelegramUpdate,
};
use crate::hospital::HospitalClient;
use crate::llm::LlmClient;
use crate::rate_limit::Limiters;
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
// Welcome shown on /start AND on the first message from an unknown chat_id
// (when no auth_state exists yet). Includes a clinic presentation so the
// user knows where they landed before being asked for their email.
const START_MESSAGE: &str = "👋 Hola, somos la Clínica San Ignacio. Te damos la bienvenida a nuestro asistente de agendamiento.\n\nPara empezar, por favor proporciónanos tu correo electrónico.";

// Three strikes on the OTP code and the session is closed. The user must
// /start over to try again, which clears the attempt counter.
const MAX_OTP_ATTEMPTS: u32 = 3;

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
    user_auth: Option<UserAuthClient>,
    auth_states: Arc<Mutex<HashMap<i64, AuthState>>>,
    // Failed OTP verify attempts per chat_id. Reset on success, /start, /reset,
    // and when the session is locked out (after MAX_OTP_ATTEMPTS).
    otp_attempts: Arc<Mutex<HashMap<i64, u32>>>,
    // Email captured during the OTP exchange, indexed by chat_id. Used
    // when creating the conversation-chat session so post-booking hooks
    // can fire confirmation emails without re-querying User-Auth.
    verified_emails: Arc<Mutex<HashMap<i64, String>>>,
    /// Shared rate-limiter used to enforce per-chat_id request throttling.
    limiters: Arc<Limiters>,
    /// chat_ids that have been shown a CSAT prompt and are awaiting a star rating.
    csat_pending: Arc<Mutex<HashMap<i64, ()>>>,
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
        user_auth: Option<UserAuthClient>,
        limiters: Arc<Limiters>,
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
            user_auth,
            auth_states: Arc::new(Mutex::new(HashMap::new())),
            otp_attempts: Arc::new(Mutex::new(HashMap::new())),
            verified_emails: Arc::new(Mutex::new(HashMap::new())),
            limiters,
            csat_pending: Arc::new(Mutex::new(HashMap::new())),
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
        // Inline-keyboard button press (e.g. CSAT star rating) — handle before
        // anything else since callback_query updates carry no `message` field.
        if let Some(cq) = update.callback_query {
            return self.handle_callback_query(cq).await;
        }

        let Some(msg) = update.message else { return Ok(()); };
        let chat_id = msg.chat.id;
        let Some(text) = msg.text.clone() else { return Ok(()); };
        if text.trim().is_empty() {
            return Ok(());
        }

        // /start and /reset: clear all state. If the user had an active
        // session, show the CSAT prompt before welcoming them fresh.
        let trimmed = text.trim();
        if trimmed == "/start" || trimmed == "/reset" {
            let had_session = self.chat_sessions.lock().await.remove(&chat_id).is_some();
            self.auth_states.lock().await.remove(&chat_id);
            self.otp_attempts.lock().await.remove(&chat_id);
            self.verified_emails.lock().await.remove(&chat_id);
            self.csat_pending.lock().await.remove(&chat_id);
            if had_session {
                // Ask for feedback on the conversation that just ended.
                self.csat_pending.lock().await.insert(chat_id, ());
                self.telegram.send_csat_prompt(chat_id).await?;
            } else {
                self.telegram.send_message(chat_id, START_MESSAGE).await?;
            }
            return Ok(());
        }

        // Per-user rate limit: keyed by chat_id so each Telegram user gets
        // their own token bucket, preventing any single user from flooding
        // the LLM pipeline regardless of which tenant they belong to.
        if let Err(retry_after) = self.limiters.tenant_chat.check(&chat_id.to_string()) {
            let secs = retry_after.as_secs().max(1);
            let notice = format!(
                "Estás enviando mensajes muy rápido. Por favor espera {secs} segundo(s) antes de continuar."
            );
            self.telegram.send_message(chat_id, &notice).await?;
            if let Some(m) = &self.metricas {
                m.record_rate_limit(self.default_tenant_id.clone(), "telegram_chat", secs);
            }
            return Ok(());
        }

        // OTP pre-registration gate. When USER_AUTH_URL is set, every
        // first-time chat_id has to provide an email + verify a 6-digit
        // code before the LLM sees the message. Returns true if the flow
        // absorbed the message (response already sent to Telegram).
        if self.run_pre_registration(&msg, &text).await? {
            return Ok(());
        }

        if let Some(ar) = &self.agent_runtime {
            self.handle_update_async(ar.clone(), chat_id, text).await
        } else {
            self.handle_update_sync(chat_id, &text).await
        }
    }

    /// Handles an inline-keyboard callback_query, specifically CSAT star ratings.
    async fn handle_callback_query(
        &self,
        cq: TelegramCallbackQuery,
    ) -> Result<(), crate::error::AppError> {
        // Resolve chat_id: prefer message.chat.id (accurate for group chats),
        // fall back to from.id (always the pressing user's private chat).
        let chat_id = cq
            .message
            .as_ref()
            .map(|m| m.chat.id)
            .unwrap_or(cq.from.id);

        let data = cq.data.as_deref().unwrap_or("");

        if let Some(score_str) = data.strip_prefix("csat:") {
            // Acknowledge immediately to clear the button-press spinner.
            let _ = self.telegram.answer_callback_query(&cq.id, None).await;

            // Ignore stale callbacks for sessions that were never marked CSAT-pending.
            let was_pending = self.csat_pending.lock().await.remove(&chat_id).is_some();
            if !was_pending {
                return Ok(());
            }

            match score_str.parse::<u8>().ok().filter(|&s| (1..=5).contains(&s)) {
                Some(score) => {
                    if let Some(m) = &self.metricas {
                        m.record_feedback(self.default_tenant_id.clone(), score);
                    }
                    self.telegram
                        .send_message(chat_id, "¡Gracias por tu calificación! 😊")
                        .await?;
                }
                None => {
                    tracing::warn!(data = %data, "received malformed CSAT callback_data");
                }
            }

            // After feedback, prompt the user to start a new conversation.
            self.telegram
                .send_message(chat_id, "Para iniciar una nueva consulta, escribe /start.")
                .await?;
        }
        Ok(())
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
                        // Successful verification resets the strike count
                        // so a future re-auth starts fresh.
                        self.otp_attempts.lock().await.remove(&chat_id);
                        self.telegram
                            .send_message(
                                chat_id,
                                "¡Listo! Cuéntame en qué puedo ayudarte.",
                            )
                            .await?;
                    }
                    Err(e) => {
                        tracing::info!(error=%e, "verify-code failed");
                        // Bump the strike count. After MAX_OTP_ATTEMPTS, drop
                        // all session state so the next message starts a fresh
                        // /start cycle.
                        let attempts = {
                            let mut g = self.otp_attempts.lock().await;
                            let n = g.get(&chat_id).copied().unwrap_or(0) + 1;
                            g.insert(chat_id, n);
                            n
                        };
                        if attempts >= MAX_OTP_ATTEMPTS {
                            self.auth_states.lock().await.remove(&chat_id);
                            self.chat_sessions.lock().await.remove(&chat_id);
                            self.otp_attempts.lock().await.remove(&chat_id);
                            self.verified_emails.lock().await.remove(&chat_id);
                            self.telegram
                                .send_message(
                                    chat_id,
                                    "Has alcanzado el máximo de intentos. Por seguridad cerramos esta sesión. Escribe /start cuando quieras intentarlo de nuevo. ¡Hasta pronto!",
                                )
                                .await?;
                        } else {
                            let remaining = MAX_OTP_ATTEMPTS - attempts;
                            let msg = format!(
                                "Código inválido o expirado. Te quedan {remaining} intento(s). Intenta otro, o escribe \"correo\" para reenviarlo."
                            );
                            self.telegram.send_message(chat_id, &msg).await?;
                        }
                    }
                }
                Ok(true)
            }

            // None (first contact) or Some(AwaitingEmail) (still missing email).
            _ => {
                if !looks_like_email(text) {
                    // First contact: full welcome + clinic presentation.
                    // Re-prompt: shorter nudge so we don't spam the welcome.
                    let is_first_contact = state.is_none();
                    self.auth_states
                        .lock()
                        .await
                        .insert(chat_id, AuthState::AwaitingEmail);
                    let prompt = if is_first_contact {
                        START_MESSAGE
                    } else {
                        "Eso no parece un correo electrónico válido. ¿Puedes escribirlo de nuevo?"
                    };
                    self.telegram.send_message(chat_id, prompt).await?;
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

                // Stash the email NOW so a successful OTP verify can
                // ship it through session creation without a re-prompt.
                // If the OTP later fails 3x, the lockout branch wipes
                // this entry alongside the auth state.
                self.verified_emails
                    .lock()
                    .await
                    .insert(chat_id, text.to_string());

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
        // If OTP verified, ship the user's email through so
        // conversation-chat can fire booking confirmations.
        let contact_email = self.verified_emails.lock().await.get(&chat_id).cloned();
        let session_id = {
            let mut guard = self.chat_sessions.lock().await;
            if let Some(sid) = guard.get(&chat_id) {
                sid.clone()
            } else {
                let sid = ar
                    .create_session(&self.default_tenant_id, contact_email.as_deref())
                    .await?;
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

        // Booking confirmed → ask for CSAT immediately (sync path only;
        // the async/broker path does not propagate `resolved` today).
        if resolved {
            self.csat_pending.lock().await.insert(chat_id, ());
            if let Err(e) = self.telegram.send_csat_prompt(chat_id).await {
                tracing::warn!(error=%e, "failed to send CSAT prompt after resolved turn");
            }
        }
        Ok(())
    }
}

/// Loose RFC-5322 heuristic — good enough to distinguish "an email" from
/// a 6-digit OTP code or free-form chat. We only need this before we hand
/// the value off to User-Auth, which does its own validation.
pub(crate) fn looks_like_email(text: &str) -> bool {
    let s = text.trim();
    if s.len() < 5 || s.len() > 254 {
        return false;
    }
    let mut at_count = 0;
    let mut has_dot_after_at = false;
    let mut seen_at = false;
    let mut local_len = 0usize;
    for c in s.chars() {
        if c == '@' {
            at_count += 1;
            seen_at = true;
            continue;
        }
        if seen_at && c == '.' {
            has_dot_after_at = true;
        }
        if c.is_whitespace() {
            return false;
        }
        if !seen_at {
            local_len += 1;
        }
    }
    at_count == 1 && has_dot_after_at && local_len > 0
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

// ── Unit tests ───────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    // ── looks_like_email ─────────────────────────────────────────────────────

    #[test]
    fn email_valid_cases() {
        let valid = [
            "user@example.com",
            "user.name+tag@sub.domain.co",
            "u@x.io",
            "first.last@hospital.org",
        ];
        for addr in valid {
            assert!(looks_like_email(addr), "{addr:?} should be recognised as email");
        }
    }

    #[test]
    fn email_invalid_cases() {
        let invalid = [
            "",
            "123456",      // OTP code
            "plaintext",   // single word
            "no-at-sign",
            "double@@sign.com",
            "missing-dot@domain",
            "spaces in@example.com",
            "@nolocal.com",
            "noDomain@",
        ];
        for s in invalid {
            assert!(!looks_like_email(s), "{s:?} should NOT be recognised as email");
        }
    }

    #[test]
    fn email_otp_codes_are_not_emails() {
        // Six-digit OTP codes must never be interpreted as emails — they would
        // cause the bot to loop forever trying to register an OTP as an address.
        for code in ["123456", "000000", "999999", "012345"] {
            assert!(!looks_like_email(code), "OTP {code:?} must not look like an email");
        }
    }

    // ── CSAT callback_data parsing ───────────────────────────────────────────

    #[test]
    fn csat_prefix_strips_correctly() {
        for (data, expected) in [
            ("csat:1", Some(1u8)),
            ("csat:5", Some(5u8)),
            ("csat:3", Some(3u8)),
        ] {
            let score = data
                .strip_prefix("csat:")
                .and_then(|s| s.parse::<u8>().ok())
                .filter(|&s| (1..=5).contains(&s));
            assert_eq!(score, expected, "data={data:?}");
        }
    }

    #[test]
    fn csat_out_of_range_rejected() {
        for bad in ["csat:0", "csat:6", "csat:99"] {
            let score = bad
                .strip_prefix("csat:")
                .and_then(|s| s.parse::<u8>().ok())
                .filter(|&s| (1..=5).contains(&s));
            assert!(score.is_none(), "{bad:?} should be filtered out");
        }
    }

    #[test]
    fn csat_malformed_data_rejected() {
        for bad in ["", "nope", "csat:", "csat:abc", "rating:3"] {
            let score = bad
                .strip_prefix("csat:")
                .and_then(|s| s.parse::<u8>().ok())
                .filter(|&s| (1..=5).contains(&s));
            assert!(score.is_none(), "{bad:?} should produce None");
        }
    }

    // ── Rate limit key isolation ─────────────────────────────────────────────

    #[test]
    fn telegram_rate_limit_keys_on_chat_id_not_tenant() {
        // Each chat_id should exhaust independently; different chat_ids don't
        // share quota. This mirrors what handle_update() does in production.
        use crate::rate_limit::{BucketConfig, Limiters, RateLimiter};
        use std::sync::Arc;

        let limiters = Arc::new(Limiters {
            tenant_chat: RateLimiter::new(BucketConfig::new(1.0, 0.0001)),
            tenant_feedback: RateLimiter::new(BucketConfig::new(100.0, 1.0)),
        });

        let chat_a: i64 = 9_001;
        let chat_b: i64 = 9_002;

        // Exhaust chat_a
        limiters.tenant_chat.check(&chat_a.to_string()).unwrap();
        assert!(
            limiters.tenant_chat.check(&chat_a.to_string()).is_err(),
            "chat_a should be rate-limited"
        );

        // chat_b is unaffected
        assert!(
            limiters.tenant_chat.check(&chat_b.to_string()).is_ok(),
            "chat_b should still have quota"
        );
    }
}
