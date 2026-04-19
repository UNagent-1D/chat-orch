use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub server_host: String,
    pub server_port: u16,
    pub conversation_chat_url: String,
    pub tenant_service_url: String,
    pub hospital_mock_url: String,
    pub metricas_url: Option<String>,
    pub telegram_bot_token: Option<String>,
    pub telegram_default_tenant_id: Option<String>,
    pub cors_allow_origin: String,
    pub openai_api_key: String,
    pub openai_base_url: String,
    pub openai_default_model: String,
    pub rust_log: String,
    pub log_format: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AppError> {
        Ok(Self {
            server_host: env_or("SERVER_HOST", "0.0.0.0"),
            server_port: env_or("SERVER_PORT", "3000")
                .parse()
                .map_err(|e: std::num::ParseIntError| {
                    AppError::Internal(format!("SERVER_PORT not a valid u16: {e}"))
                })?,
            conversation_chat_url: env_required("CONVERSATION_CHAT_URL")?,
            tenant_service_url: env_required("TENANT_SERVICE_URL")?,
            hospital_mock_url: env_or("HOSPITAL_MOCK_URL", "http://hospital-mock:8080"),
            metricas_url: env_opt("METRICAS_URL"),
            telegram_bot_token: env_opt("TELEGRAM_BOT_TOKEN"),
            telegram_default_tenant_id: env_opt("TELEGRAM_DEFAULT_TENANT_ID"),
            cors_allow_origin: env_or("CORS_ALLOW_ORIGIN", "http://localhost:3000"),
            openai_api_key: env_required("OPENAI_API_KEY")?,
            openai_base_url: env_or("OPENAI_BASE_URL", "https://openrouter.ai/api/v1"),
            openai_default_model: env_or(
                "OPENAI_DEFAULT_MODEL",
                "nvidia/nemotron-3-super-120b-a12b:free",
            ),
            rust_log: env_or("RUST_LOG", "chat_orch=info,tower_http=info"),
            log_format: env_or("LOG_FORMAT", "pretty"),
        })
    }
}

fn env_required(key: &str) -> Result<String, AppError> {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err(AppError::MissingEnv(key.to_string())),
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Use a dedicated mutex so concurrent tests don't race on process-wide env.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn set_required() {
        std::env::set_var("CONVERSATION_CHAT_URL", "http://conversation-chat:8082");
        std::env::set_var("TENANT_SERVICE_URL", "http://tenant:8080");
        std::env::set_var("OPENAI_API_KEY", "sk-test");
    }

    fn clear_all() {
        for k in [
            "SERVER_HOST",
            "SERVER_PORT",
            "CONVERSATION_CHAT_URL",
            "TENANT_SERVICE_URL",
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_DEFAULT_MODEL",
            "RUST_LOG",
            "LOG_FORMAT",
        ] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn from_env_happy_path_applies_defaults() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        set_required();

        let cfg = AppConfig::from_env().expect("config should load");
        assert_eq!(cfg.server_host, "0.0.0.0");
        assert_eq!(cfg.server_port, 3000);
        assert_eq!(cfg.conversation_chat_url, "http://conversation-chat:8082");
        assert_eq!(cfg.tenant_service_url, "http://tenant:8080");
        assert_eq!(cfg.openai_api_key, "sk-test");
        assert_eq!(cfg.openai_base_url, "https://openrouter.ai/api/v1");
        assert_eq!(
            cfg.openai_default_model,
            "nvidia/nemotron-3-super-120b-a12b:free"
        );
        assert_eq!(cfg.log_format, "pretty");

        clear_all();
    }

    #[test]
    fn from_env_missing_conversation_chat_url_errors() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        std::env::set_var("TENANT_SERVICE_URL", "http://tenant:8080");
        std::env::set_var("OPENAI_API_KEY", "sk-test");

        let err = AppConfig::from_env().expect_err("should fail without CONVERSATION_CHAT_URL");
        match err {
            AppError::MissingEnv(k) => assert_eq!(k, "CONVERSATION_CHAT_URL"),
            other => panic!("expected MissingEnv, got {other:?}"),
        }

        clear_all();
    }
}
