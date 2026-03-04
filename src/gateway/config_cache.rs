use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use uuid::Uuid;

use crate::error::AppError;
use crate::gateway::acr_client::AgentConfig;

use super::acr_client::AcrClient;

/// Cache key for agent config: (tenant_id, profile_id).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ConfigCacheKey {
    pub tenant_id: Uuid,
    pub profile_id: Uuid,
}

/// Cache for agent configs from the Agent Config Registry.
///
/// Agent configs change when an admin activates a new version, which happens
/// infrequently. A short TTL (2 min) balances freshness with reducing load
/// on the ACR service.
///
/// - TTL: 2 minutes
/// - Max capacity: 50K entries
/// - Thundering herd protection via `try_get_with`
#[derive(Clone)]
pub struct ConfigCache {
    inner: Cache<ConfigCacheKey, Arc<AgentConfig>>,
}

impl ConfigCache {
    /// Create a new config cache with the given TTL and max capacity.
    pub fn new(ttl_secs: u64, max_entries: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_entries)
            .time_to_live(Duration::from_secs(ttl_secs))
            .build();

        Self { inner: cache }
    }

    /// Get the active agent config, using the cache or falling back to the ACR.
    pub async fn resolve(
        &self,
        tenant_id: Uuid,
        profile_id: Uuid,
        acr_client: &AcrClient,
    ) -> Result<Arc<AgentConfig>, AppError> {
        let key = ConfigCacheKey {
            tenant_id,
            profile_id,
        };
        let client = acr_client.clone();

        self.inner
            .try_get_with(key, async move {
                let config = client.get_active_config(tenant_id, profile_id).await?;
                Ok::<_, AppError>(Arc::new(config))
            })
            .await
            .map_err(|e| AppError::Downstream(format!("config resolution failed: {e}")))
    }

    /// Invalidate a specific config entry.
    pub async fn invalidate(&self, tenant_id: Uuid, profile_id: Uuid) {
        let key = ConfigCacheKey {
            tenant_id,
            profile_id,
        };
        self.inner.invalidate(&key).await;
    }

    /// Number of entries currently in the cache.
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }
}
