// Task 13: Full assembly pending — depends on all module implementations.
// AppState holds all shared application resources: caches, clients, semaphore, Redis pools.

use crate::config::AppConfig;
use std::sync::Arc;

/// Shared application state passed to all Axum handlers via `State<AppState>`.
///
/// All fields are `Clone`-friendly (wrapped in `Arc` internally or inherently cloneable).
/// This struct is assembled once in `main.rs` and never mutated.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    // TODO(task-5):  tenant_client: TenantClient
    // TODO(task-5):  acr_client: AcrClient
    // TODO(task-6):  channel_cache: ChannelCache
    // TODO(task-6):  config_cache: ConfigCache
    // TODO(task-7):  session_store: RedisSessionStore
    // TODO(task-8):  dedup: RedisDedup
    // TODO(task-9):  llm_client: Arc<dyn LlmClient>
    // TODO(task-11): pipeline: Pipeline
    // TODO(task-12): reply_sender: ReplySender
}

impl AppState {
    pub async fn new(_config: AppConfig) -> anyhow::Result<Self> {
        let config = Arc::new(_config);

        // TODO: Initialize all subsystems here
        // Each task will add its initialization step

        Ok(Self { config })
    }
}
