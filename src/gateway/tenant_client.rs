use reqwest::Client;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::types::ingest_message::{ChannelLookupKey, TenantResolution};

/// Generic wrapper for paginated/listed responses from the Tenant Service.
/// The Go Tenant Service (and conversation-chat) wrap arrays as `{"data": [...]}`.
#[derive(Debug, Deserialize)]
struct DataWrapper<T> {
    data: Vec<T>,
}

/// Response from `GET /internal/resolve-channel`.
#[derive(Debug, Deserialize)]
struct ResolveChannelResponse {
    tenant_id: Uuid,
    tenant_slug: String,
    agent_profile_id: Uuid,
    webhook_secret_ref: String,
    is_active: bool,
}

/// Response from `GET /api/v1/tenants/:id`.
#[derive(Debug, Clone, Deserialize)]
pub struct TenantDetail {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub plan: String,
    pub status: String,
    pub branding_logo_url: Option<String>,
    pub branding_primary_color: Option<String>,
}

/// Response from `GET /api/v1/tenants/:id/profiles`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentProfile {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub scheduling_flow_rules: Option<serde_json::Value>,
    pub escalation_rules: Option<serde_json::Value>,
    pub allowed_specialties: Option<Vec<String>>,
    pub allowed_locations: Option<Vec<String>>,
    pub agent_config_id: Uuid,
}

/// Response from `GET /api/v1/tenants/:id/data-sources`.
#[derive(Debug, Clone, Deserialize)]
pub struct DataSource {
    pub id: Uuid,
    pub name: String,
    pub source_type: String,
    pub base_url: String,
    pub credential_ref: Option<String>,
    pub route_configs: serde_json::Value,
    pub is_active: bool,
}

/// HTTP client for the Tenant Service (Go + Gin).
///
/// All methods use the shared `reqwest::Client` with connection pooling.
/// Timeouts are set at the client level (10s for Tenant Service calls).
#[derive(Clone)]
pub struct TenantClient {
    client: Client,
    base_url: String,
}

impl TenantClient {
    /// Create a new Tenant Service client.
    ///
    /// The `base_url` should NOT have a trailing slash.
    pub fn new(client: Client, base_url: String) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Resolve a channel key to a tenant.
    ///
    /// Calls `GET /internal/resolve-channel?channel_type=X&channel_key=Y`.
    /// This is the critical path for WhatsApp tenant resolution.
    pub async fn resolve_channel(
        &self,
        key: &ChannelLookupKey,
    ) -> Result<TenantResolution, AppError> {
        let url = format!("{}/internal/resolve-channel", self.base_url);

        let resp = self
            .client
            .get(&url)
            .query(&[
                ("channel_type", key.channel_type.as_str()),
                ("channel_key", &key.channel_key),
            ])
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("tenant service unreachable: {e}")))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::TenantNotFound);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "tenant service returned {status}: {body}"
            )));
        }

        let data: ResolveChannelResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Downstream(format!("invalid resolve-channel response: {e}")))?;

        if !data.is_active {
            return Err(AppError::ChannelInactive);
        }

        Ok(TenantResolution {
            tenant_id: data.tenant_id,
            tenant_slug: data.tenant_slug,
            agent_profile_id: data.agent_profile_id,
            webhook_secret_ref: data.webhook_secret_ref,
            is_active: data.is_active,
        })
    }

    /// Get tenant detail by ID.
    ///
    /// Calls `GET /api/v1/tenants/:id`.
    pub async fn get_tenant(&self, tenant_id: Uuid) -> Result<TenantDetail, AppError> {
        let url = format!("{}/api/v1/tenants/{}", self.base_url, tenant_id);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("tenant service unreachable: {e}")))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::TenantNotFound);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "tenant service returned {status}: {body}"
            )));
        }

        resp.json()
            .await
            .map_err(|e| AppError::Downstream(format!("invalid tenant response: {e}")))
    }

    /// Get agent profiles for a tenant.
    ///
    /// Calls `GET /api/v1/tenants/:id/profiles`.
    /// Response format: `{"data": [...]}`
    pub async fn get_profiles(&self, tenant_id: Uuid) -> Result<Vec<AgentProfile>, AppError> {
        let url = format!("{}/api/v1/tenants/{}/profiles", self.base_url, tenant_id);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("tenant service unreachable: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "tenant service returned {status}: {body}"
            )));
        }

        let wrapper: DataWrapper<AgentProfile> = resp
            .json()
            .await
            .map_err(|e| AppError::Downstream(format!("invalid profiles response: {e}")))?;

        Ok(wrapper.data)
    }

    /// Get data sources for a tenant (used by tool executor for route_configs).
    ///
    /// Calls `GET /api/v1/tenants/:id/data-sources`.
    /// Response format: `{"data": [...]}`
    pub async fn get_data_sources(&self, tenant_id: Uuid) -> Result<Vec<DataSource>, AppError> {
        let url = format!(
            "{}/api/v1/tenants/{}/data-sources",
            self.base_url, tenant_id
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("tenant service unreachable: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Downstream(format!(
                "tenant service returned {status}: {body}"
            )));
        }

        let wrapper: DataWrapper<DataSource> = resp
            .json()
            .await
            .map_err(|e| AppError::Downstream(format!("invalid data-sources response: {e}")))?;

        Ok(wrapper.data)
    }
}
