use reqwest::Client;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::AppError;

/// LLM parameters from the agent config.
#[derive(Debug, Clone, Deserialize)]
pub struct LlmParams {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub system_prompt: String,
}

/// A single tool permission entry from the agent config.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolPermission {
    pub tool_name: String,
    #[serde(default)]
    pub constraints: serde_json::Value,
}

/// Channel-specific formatting rules.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelFormatRules {
    pub whatsapp: Option<ChannelFormat>,
    pub telegram: Option<ChannelFormat>,
    pub web_widget: Option<ChannelFormat>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelFormat {
    pub max_chars: Option<u32>,
}

/// Full agent config response from the Agent Config Registry.
///
/// Returned by `GET /api/v1/tenants/:id/profiles/:pid/configs/active`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub id: Uuid,
    pub agent_profile_id: Uuid,
    pub version: u32,
    pub status: String,
    pub conversation_policy: serde_json::Value,
    pub escalation_rules: serde_json::Value,
    pub tool_permissions: Vec<ToolPermission>,
    pub llm_params: LlmParams,
    pub channel_format_rules: Option<ChannelFormatRules>,
    pub created_at: Option<String>,
    pub activated_at: Option<String>,
}

/// A single entry from the global tool registry.
///
/// The tool registry is a global catalog (not per-tenant) maintained by the
/// ACR service. Each entry contains the full OpenAI function-calling JSON
/// schema, which is the authoritative source for tool definitions sent to the LLM.
///
/// Returned by `GET /api/v1/tool-registry`.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolRegistryEntry {
    pub id: Uuid,
    pub tool_name: String,
    pub description: String,
    /// Full OpenAI function-calling definition (JSON object with `name`,
    /// `description`, `parameters`). This is the source of truth for tool
    /// schemas — it overrides the constraints-based fallback in turn_loop.rs.
    pub openai_function_def: serde_json::Value,
    pub is_active: bool,
    pub version: i32,
}

/// HTTP client for the Agent Config Registry (Go + Gin).
///
/// The ACR provides versioned runtime configuration for AI agents:
/// LLM parameters, tool permissions, conversation policy, and
/// channel formatting rules.
///
/// Also serves the global tool registry (not tenant-scoped).
#[derive(Clone)]
pub struct AcrClient {
    client: Client,
    base_url: String,
}

/// Maximum response body size for tool registry (1MB).
/// Prevents OOM from unbounded responses.
const MAX_TOOL_REGISTRY_RESPONSE_BYTES: usize = 1_048_576;

impl AcrClient {
    /// Create a new ACR client.
    ///
    /// The `base_url` should NOT have a trailing slash.
    pub fn new(client: Client, base_url: String) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Get the active agent config for a profile.
    ///
    /// Calls `GET /api/v1/tenants/:tenant_id/profiles/:profile_id/configs/active`.
    ///
    /// This is called on every conversation turn (with caching in front).
    pub async fn get_active_config(
        &self,
        tenant_id: Uuid,
        profile_id: Uuid,
    ) -> Result<AgentConfig, AppError> {
        let url = format!(
            "{}/api/v1/tenants/{}/profiles/{}/configs/active",
            self.base_url, tenant_id, profile_id
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("ACR service unreachable: {e}")))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::Downstream(format!(
                "no active config found for tenant={tenant_id} profile={profile_id}"
            )));
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "ACR service returned {status}: {body}"
            )));
        }

        resp.json()
            .await
            .map_err(|e| AppError::Downstream(format!("invalid agent config response: {e}")))
    }

    /// Get the global tool registry.
    ///
    /// Calls `GET /api/v1/tool-registry`.
    ///
    /// This is NOT tenant-scoped — the tool registry is a global catalog of
    /// all available tools with their OpenAI function-calling definitions.
    /// The method is named `get_global_tool_registry` to signal this clearly.
    ///
    /// Response body is limited to 1MB to prevent OOM from unbounded responses.
    /// Only active tools (is_active == true) are returned.
    pub async fn get_global_tool_registry(&self) -> Result<Vec<ToolRegistryEntry>, AppError> {
        let url = format!("{}/api/v1/tool-registry", self.base_url);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("ACR tool registry unreachable: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "ACR tool registry returned {status}: {body}"
            )));
        }

        // Enforce response size limit to prevent OOM
        let body_bytes = resp
            .bytes()
            .await
            .map_err(|e| AppError::Downstream(format!("failed to read tool registry body: {e}")))?;

        if body_bytes.len() > MAX_TOOL_REGISTRY_RESPONSE_BYTES {
            return Err(AppError::Downstream(format!(
                "tool registry response too large: {} bytes (max {})",
                body_bytes.len(),
                MAX_TOOL_REGISTRY_RESPONSE_BYTES
            )));
        }

        let entries: Vec<ToolRegistryEntry> = serde_json::from_slice(&body_bytes)
            .map_err(|e| AppError::Downstream(format!("invalid tool registry response: {e}")))?;

        // Filter to active tools only
        Ok(entries.into_iter().filter(|e| e.is_active).collect())
    }
}
