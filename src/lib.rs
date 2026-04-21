use std::sync::Arc;

pub mod config;
pub mod error;
pub mod gateway;
pub mod hospital;
pub mod llm;
pub mod routes;
pub mod runtime;
pub mod session;
pub mod sse;
pub mod telegram;

pub use error::AppError;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config::AppConfig>,
    pub llm: Arc<llm::LlmClient>,
    pub hospital: Arc<hospital::HospitalClient>,
    pub sessions: Arc<session::SessionStore>,
    pub metricas: Option<gateway::MetricasClient>,
    pub agent_runtime: Option<gateway::ConversationChatClient>,
    pub hub: Arc<sse::SseHub>,
}
