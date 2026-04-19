use std::sync::Arc;

pub mod config;
pub mod error;
pub mod gateway;
pub mod routes;

pub use error::AppError;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config::AppConfig>,
    pub conversation_chat: gateway::ConversationChatClient,
    pub metricas: Option<gateway::MetricasClient>,
}
