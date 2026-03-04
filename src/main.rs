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

    // Startup warnings for auth and resolution configuration
    log_startup_warnings(&config);

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

/// Log startup warnings about configuration state.
fn log_startup_warnings(config: &config::AppConfig) {
    // Metrics API key
    if config.metrics_api_key.is_none() {
        tracing::warn!(
            "METRICS_API_KEY not set — /metrics/pipeline endpoint is disabled (returns 403)"
        );
    }

    // WhatsApp static tenant map
    if config.whatsapp_static_tenant_map.is_some() {
        tracing::warn!(
            "WHATSAPP_STATIC_TENANT_MAP is configured — using static tenant overrides. \
             This is an MVP workaround until the Go team delivers GET /internal/resolve-channel"
        );
    } else {
        tracing::info!(
            "no static tenant map configured — WhatsApp tenant resolution \
             depends on Tenant Service GET /internal/resolve-channel"
        );
    }
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
