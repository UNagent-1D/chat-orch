use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chat_orch::channel::{Channel, ChannelKey};
use chat_orch::config::AppConfig;
use chat_orch::gateway::{ConversationChatClient, MetricasClient, TelegramClient};
use chat_orch::hospital::HospitalClient;
use chat_orch::llm::LlmClient;
use chat_orch::rate_limit::{LimiterSettings, Limiters};
use chat_orch::session::SessionStore;
use chat_orch::sse::SseHub;
use chat_orch::telegram::TelegramLoop;
use chat_orch::{routes, AppState};
use tokio::net::TcpListener;
use tokio::signal;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let config = AppConfig::from_env().context("loading AppConfig from environment")?;
    init_tracing(&config.rust_log, &config.log_format);

    let http = reqwest::Client::builder()
        .pool_idle_timeout(Duration::from_secs(30))
        .build()
        .context("building reqwest client")?;

    let channel_key = config
        .backend_channel_key
        .as_deref()
        .map(ChannelKey::from_base64)
        .transpose()
        .context("decoding BACKEND_CHANNEL_KEY")?;
    let channel = Channel::new(channel_key, config.backend_channel_enabled)
        .context("constructing secure channel")?;
    if channel.active() {
        tracing::info!("secure channel enabled (AES-256-GCM)");
    } else {
        tracing::info!("secure channel disabled — backend traffic is plaintext");
    }

    let llm = Arc::new(LlmClient::new(
        http.clone(),
        config.openai_base_url.clone(),
        config.openai_api_key.clone(),
        config.openai_default_model.clone(),
    ));

    let hospital = Arc::new(HospitalClient::new(
        http.clone(),
        config.hospital_mock_url.clone(),
        channel.clone(),
    ));

    let sessions = SessionStore::new();

    let metricas = config
        .metricas_url
        .clone()
        .map(|url| MetricasClient::new(http.clone(), url, channel.clone()));

    let hub = SseHub::new();

    let agent_runtime = config
        .agent_runtime_url
        .clone()
        .map(|url| ConversationChatClient::new(http.clone(), url, channel.clone()));

    // Build the rate limiters early so they can be shared with TelegramLoop.
    let limiter_settings = LimiterSettings::from_env();
    tracing::info!(
        chat_burst = limiter_settings.chat_burst,
        chat_per_sec = limiter_settings.chat_per_sec,
        feedback_burst = limiter_settings.feedback_burst,
        feedback_per_sec = limiter_settings.feedback_per_sec,
        "rate limiter initialised"
    );
    let limiters = Arc::new(Limiters::from_settings(&limiter_settings));

    if let (Some(token), Some(tenant_id)) = (
        config.telegram_bot_token.clone(),
        config.telegram_default_tenant_id.clone(),
    ) {
        let telegram = TelegramClient::new(http.clone(), &token);
        let ar_for_telegram = agent_runtime.clone().map(Arc::new);
        let user_auth = config
            .user_auth_url
            .clone()
            .map(|url| chat_orch::user_auth::UserAuthClient::new(http.clone(), url));
        if user_auth.is_some() {
            tracing::info!("telegram OTP pre-registration enabled (USER_AUTH_URL set)");
        }
        let tenant_slug = config
            .telegram_default_tenant_slug
            .clone()
            .unwrap_or_else(|| "demo-hospital".to_string());
        TelegramLoop::new(
            telegram,
            llm.clone(),
            hospital.clone(),
            sessions.clone(),
            metricas.clone(),
            tenant_id,
            tenant_slug,
            ar_for_telegram,
            http.clone(),
            config.conversation_chat_url.clone(),
            user_auth,
            limiters.clone(),
        )
        .spawn();
    } else {
        tracing::info!(
            "telegram loop disabled (TELEGRAM_BOT_TOKEN and/or TELEGRAM_DEFAULT_TENANT_ID unset)"
        );
    }

    let addr: SocketAddr = format!("{}:{}", config.server_host, config.server_port)
        .parse()
        .with_context(|| {
            format!(
                "invalid server bind address {}:{}",
                config.server_host, config.server_port
            )
        })?;

    let state = AppState {
        config: Arc::new(config),
        llm,
        hospital,
        sessions,
        metricas,
        agent_runtime,
        hub,
        limiters,
    };

    let app = routes::build_router(state);

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding listener on {addr}"))?;

    tracing::info!(%addr, "chat-orch listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum server error")?;

    Ok(())
}

fn init_tracing(rust_log: &str, log_format: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(rust_log));

    let registry = tracing_subscriber::registry().with(filter);
    if log_format.eq_ignore_ascii_case("json") {
        registry.with(fmt::layer().json()).init();
    } else {
        registry.with(fmt::layer()).init();
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
