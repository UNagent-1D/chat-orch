use anyhow::Result;
use tracing_subscriber::{fmt, EnvFilter};

mod config;
mod conversation;
mod error;
mod router;
mod state;

mod auth;
mod gateway;
mod ingest;
mod llm;
mod pipeline;
mod types;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file (ignore error if not present — production uses real env vars)
    dotenvy::dotenv().ok();

    // Initialize structured logging
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();

    tracing::info!("starting chat-orch general orchestrator");

    // Load configuration
    let config = config::AppConfig::from_env()?;
    let addr = format!("{}:{}", config.server_host, config.server_port);

    // Build application state (caches, clients, semaphore, Redis pools)
    let state = state::AppState::new(config).await?;

    // Build router with all routes and middleware
    let app = router::build_router(state);

    // Bind and serve with graceful shutdown
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(addr = %addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install ctrl+c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received ctrl+c"),
        _ = terminate => tracing::info!("received SIGTERM"),
    }
}
