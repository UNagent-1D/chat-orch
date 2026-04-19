use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chat_orch::config::AppConfig;
use chat_orch::gateway::ConversationChatClient;
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

    let conversation_chat =
        ConversationChatClient::new(http, config.conversation_chat_url.clone());

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
        conversation_chat,
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
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(rust_log));

    let registry = tracing_subscriber::registry().with(filter);
    if log_format.eq_ignore_ascii_case("json") {
        registry.with(fmt::layer().json()).init();
    } else {
        registry.with(fmt::layer()).init();
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
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
