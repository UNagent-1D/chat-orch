use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;

use crate::error::AppError;
use crate::types::ingest_message::{ChannelLookupKey, TenantResolution};

use super::tenant_client::TenantClient;

/// Cache for channel_key → tenant resolution.
///
/// This is the highest-frequency cache in the system: every webhook message
/// must resolve its channel to a tenant before processing.
///
/// Uses `moka::future::Cache::try_get_with()` for thundering-herd protection:
/// if 10 messages arrive simultaneously for the same unknown channel, only
/// ONE call to the Tenant Service is made. The other 9 wait on the same future.
///
/// - TTL: 5 minutes (channel→tenant mapping changes rarely)
/// - Max capacity: 100K entries
/// - Eviction: LRU when capacity is reached
#[derive(Clone)]
pub struct ChannelCache {
    inner: Cache<ChannelLookupKey, Arc<TenantResolution>>,
}

impl ChannelCache {
    /// Create a new channel cache with the given TTL and max capacity.
    pub fn new(ttl_secs: u64, max_entries: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_entries)
            .time_to_live(Duration::from_secs(ttl_secs))
            .build();

        Self { inner: cache }
    }

    /// Resolve a channel to its tenant, using the cache or falling back to
    /// the Tenant Service.
    ///
    /// `try_get_with` ensures only one inflight request per key (thundering herd).
    pub async fn resolve(
        &self,
        key: &ChannelLookupKey,
        tenant_client: &TenantClient,
    ) -> Result<Arc<TenantResolution>, AppError> {
        let key = key.clone();
        let client = tenant_client.clone();

        self.inner
            .try_get_with(key.clone(), async move {
                let resolution = client.resolve_channel(&key).await?;
                Ok::<_, AppError>(Arc::new(resolution))
            })
            .await
            .map_err(|e| AppError::Downstream(format!("channel resolution failed: {e}")))
    }

    /// Invalidate a specific cache entry (e.g., on 401 from downstream).
    pub async fn invalidate(&self, key: &ChannelLookupKey) {
        self.inner.invalidate(key).await;
    }

    /// Number of entries currently in the cache.
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }
}
