// Task 3: Full implementation pending
// This module loads AppConfig from environment variables via dotenvy.

/// Application configuration loaded from environment variables.
///
/// All required variables must be set or the service panics on startup.
/// See .env.example for the complete list.
pub struct AppConfig {
    // Server
    pub server_host: String,
    pub server_port: u16,
    pub max_concurrency: usize,

    // Redis
    pub redis_url: String,
    pub session_ttl_secs: u64,
    pub dedup_ttl_secs: u64,

    // Downstream services
    pub tenant_service_url: String,
    pub acr_service_url: String,
    pub http_pool_size: usize,

    // LLM
    pub openai_api_key: String,
    pub openai_base_url: String,
    pub openai_default_model: String,

    // Telegram
    pub telegram_bot_token: Option<String>,
    pub telegram_webhook_secret: Option<String>,
    pub telegram_use_polling: bool,

    // WhatsApp
    pub whatsapp_access_token: Option<String>,
    pub whatsapp_verify_token: Option<String>,
    pub whatsapp_app_secret: Option<String>,
    pub whatsapp_api_version: String,

    // JWT
    pub jwt_secret: String,
    pub jwt_issuer: String,

    // Caches
    pub channel_cache_ttl_secs: u64,
    pub channel_cache_max_entries: u64,
    pub config_cache_ttl_secs: u64,
    pub config_cache_max_entries: u64,

    // Observability
    pub log_format: String,
}

impl AppConfig {
    /// Load configuration from environment variables.
    ///
    /// Panics with a descriptive error if a required variable is missing.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            // Server
            server_host: env_or("SERVER_HOST", "0.0.0.0"),
            server_port: env_or("SERVER_PORT", "3000").parse()?,
            max_concurrency: env_or("MAX_CONCURRENCY", "10000").parse()?,

            // Redis
            redis_url: env_required("REDIS_URL")?,
            session_ttl_secs: env_or("SESSION_TTL_SECS", "1800").parse()?,
            dedup_ttl_secs: env_or("DEDUP_TTL_SECS", "86400").parse()?,

            // Downstream
            tenant_service_url: env_required("TENANT_SERVICE_URL")?,
            acr_service_url: env_required("ACR_SERVICE_URL")?,
            http_pool_size: env_or("HTTP_POOL_SIZE", "2000").parse()?,

            // LLM
            openai_api_key: env_required("OPENAI_API_KEY")?,
            openai_base_url: env_or("OPENAI_BASE_URL", "https://api.openai.com/v1"),
            openai_default_model: env_or("OPENAI_DEFAULT_MODEL", "gpt-4o"),

            // Telegram
            telegram_bot_token: env_opt("TELEGRAM_BOT_TOKEN"),
            telegram_webhook_secret: env_opt("TELEGRAM_WEBHOOK_SECRET"),
            telegram_use_polling: env_or("TELEGRAM_USE_POLLING", "false")
                .parse()
                .unwrap_or(false),

            // WhatsApp
            whatsapp_access_token: env_opt("WHATSAPP_ACCESS_TOKEN"),
            whatsapp_verify_token: env_opt("WHATSAPP_VERIFY_TOKEN"),
            whatsapp_app_secret: env_opt("WHATSAPP_APP_SECRET"),
            whatsapp_api_version: env_or("WHATSAPP_API_VERSION", "v18.0"),

            // JWT
            jwt_secret: env_required("JWT_SECRET")?,
            jwt_issuer: env_or("JWT_ISSUER", "tenant-service"),

            // Caches
            channel_cache_ttl_secs: env_or("CHANNEL_CACHE_TTL_SECS", "300").parse()?,
            channel_cache_max_entries: env_or("CHANNEL_CACHE_MAX_ENTRIES", "100000").parse()?,
            config_cache_ttl_secs: env_or("CONFIG_CACHE_TTL_SECS", "120").parse()?,
            config_cache_max_entries: env_or("CONFIG_CACHE_MAX_ENTRIES", "50000").parse()?,

            // Observability
            log_format: env_or("LOG_FORMAT", "pretty"),
        })
    }
}

fn env_required(key: &str) -> anyhow::Result<String> {
    std::env::var(key).map_err(|_| anyhow::anyhow!("required env var {key} is not set"))
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
