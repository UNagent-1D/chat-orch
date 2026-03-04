use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::gateway::acr_client::AcrClient;
use crate::gateway::channel_cache::ChannelCache;
use crate::gateway::config_cache::ConfigCache;
use crate::gateway::reply_sender::ReplySender;
use crate::gateway::tenant_client::TenantClient;
use crate::gateway::tool_registry_cache::ToolRegistryCache;
use crate::llm::client::{LlmClient, OpenAiClient};
use crate::llm::tool_executor::ToolExecutor;
use crate::pipeline::dedup::RedisDedup;
use crate::pipeline::session::RedisSessionStore;
use crate::pipeline::worker::Pipeline;
use crate::types::ingest_message::TenantResolution;

/// JSON format for a single entry in the WHATSAPP_STATIC_TENANT_MAP env var.
///
/// Example:
/// ```json
/// [
///   {
///     "phone_number_id": "123456789012345",
///     "tenant_id": "550e8400-e29b-41d4-a716-446655440000",
///     "tenant_slug": "hospital-san-ignacio",
///     "agent_profile_id": "770e8400-e29b-41d4-a716-446655440002"
///   }
/// ]
/// ```
#[derive(Debug, Deserialize)]
struct StaticTenantEntry {
    phone_number_id: String,
    tenant_id: Uuid,
    tenant_slug: String,
    agent_profile_id: Uuid,
}

/// Shared application state passed to all Axum handlers via `State<AppState>`.
///
/// All fields are `Clone`-friendly (wrapped in `Arc` internally or inherently cloneable).
/// This struct is assembled once in `main.rs` and never mutated.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,

    // --- Gateway clients (Task 5) ---
    pub tenant_client: TenantClient,
    pub acr_client: AcrClient,

    // --- Caches (Task 6) ---
    pub channel_cache: ChannelCache,
    pub config_cache: ConfigCache,

    // --- Tool registry cache (Task 4 — tool definitions from ACR) ---
    pub tool_registry_cache: ToolRegistryCache,

    // --- Redis session store (Task 7) ---
    pub session_store: RedisSessionStore,

    // --- Redis dedup (Task 8) ---
    pub dedup: RedisDedup,

    // --- LLM (Task 9) ---
    pub llm_client: Arc<dyn LlmClient>,
    pub tool_executor: ToolExecutor,

    // --- Reply sender (Task 12) ---
    pub reply_sender: ReplySender,

    // --- Concurrency pipeline (Task 11) ---
    pub pipeline: Pipeline,
}

impl AppState {
    pub async fn new(config: AppConfig) -> anyhow::Result<Self> {
        // Shared reqwest client with connection pooling for all downstream HTTP calls.
        // pool_max_idle_per_host = 2000 to sustain 100k msg/sec across downstream services.
        let http_client = reqwest::Client::builder()
            .pool_max_idle_per_host(config.http_pool_size)
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;

        // Task 5: Gateway clients
        let tenant_client = TenantClient::new(http_client.clone(), config.tenant_service_url.clone());
        let acr_client = AcrClient::new(http_client.clone(), config.acr_service_url.clone());

        // Parse static tenant map from JSON env var (MVP workaround)
        let static_overrides = parse_static_tenant_map(&config.whatsapp_static_tenant_map)?;

        // Task 6: In-memory caches with thundering-herd protection
        let channel_cache = ChannelCache::new(
            config.channel_cache_ttl_secs,
            config.channel_cache_max_entries,
            static_overrides,
        );
        let config_cache = ConfigCache::new(
            config.config_cache_ttl_secs,
            config.config_cache_max_entries,
        );

        // Tool registry cache (global, TTL 5 min)
        let tool_registry_cache = ToolRegistryCache::new(300, 1);

        // Task 7: Redis session store
        let session_store = RedisSessionStore::new(&config.redis_url, config.session_ttl_secs)?;

        // Task 8: Redis dedup (SETNX with configurable TTL, default 24h)
        let dedup = RedisDedup::new(&config.redis_url, config.dedup_ttl_secs)?;

        // Task 9: LLM client (longer timeout — LLM calls can take 10-30s)
        let llm_http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build LLM HTTP client: {e}"))?;

        let llm_client: Arc<dyn LlmClient> = Arc::new(OpenAiClient::new(
            llm_http_client,
            config.openai_base_url.clone(),
            config.openai_api_key.clone(),
        ));

        // Tool executor shares the standard HTTP client (10s timeout is fine for data source APIs)
        let tool_executor = ToolExecutor::new(http_client.clone());

        // Task 12: Reply sender for pushing responses back to channels
        // Uses a separate HTTP client with a 15s timeout for channel API calls
        let channel_http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build channel HTTP client: {e}"))?;

        let reply_sender = ReplySender::new(
            channel_http_client,
            config.telegram_bot_token.clone(),
            config.whatsapp_access_token.clone(),
            config.whatsapp_api_version.clone(),
        );

        // Task 11: Semaphore-bounded pipeline
        let pipeline = Pipeline::new(config.max_concurrency);

        let config = Arc::new(config);

        Ok(Self {
            config,
            tenant_client,
            acr_client,
            channel_cache,
            config_cache,
            tool_registry_cache,
            session_store,
            dedup,
            llm_client,
            tool_executor,
            reply_sender,
            pipeline,
        })
    }
}

/// Parse the WHATSAPP_STATIC_TENANT_MAP JSON env var into a HashMap.
///
/// Returns an empty HashMap if the env var is not set.
/// Panics on malformed JSON (fail-fast at startup, not at runtime).
fn parse_static_tenant_map(
    raw: &Option<String>,
) -> anyhow::Result<HashMap<String, TenantResolution>> {
    let Some(json_str) = raw else {
        return Ok(HashMap::new());
    };

    let entries: Vec<StaticTenantEntry> = serde_json::from_str(json_str).map_err(|e| {
        anyhow::anyhow!(
            "failed to parse WHATSAPP_STATIC_TENANT_MAP — expected JSON array of \
             {{\"phone_number_id\": \"...\", \"tenant_id\": \"...\", \
             \"tenant_slug\": \"...\", \"agent_profile_id\": \"...\"}}: {e}"
        )
    })?;

    let mut map = HashMap::with_capacity(entries.len());
    for entry in entries {
        map.insert(
            entry.phone_number_id.clone(),
            TenantResolution {
                tenant_id: entry.tenant_id,
                tenant_slug: entry.tenant_slug,
                agent_profile_id: entry.agent_profile_id,
                // webhook_secret_ref is not used for WhatsApp resolution —
                // HMAC signature verification occurs BEFORE tenant resolution
                // in the webhook handler. Set to empty string intentionally.
                webhook_secret_ref: String::new(),
                is_active: true,
            },
        );
    }

    Ok(map)
}
