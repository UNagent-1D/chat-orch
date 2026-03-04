use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;

use crate::error::AppError;
use crate::types::ingest_message::{ChannelLookupKey, TenantResolution};

use super::tenant_client::TenantClient;

/// Cache for channel_key -> tenant resolution.
///
/// This is the highest-frequency cache in the system: every webhook message
/// must resolve its channel to a tenant before processing.
///
/// Uses `moka::future::Cache::try_get_with()` for thundering-herd protection:
/// if 10 messages arrive simultaneously for the same unknown channel, only
/// ONE call to the Tenant Service is made. The other 9 wait on the same future.
///
/// ## Static Overrides (MVP Workaround)
///
/// The `static_overrides` field holds an optional mapping of `channel_key`
/// to `TenantResolution`, populated from the `WHATSAPP_STATIC_TENANT_MAP`
/// env var at startup. This is an **explicit override** checked BEFORE the
/// HTTP call to the Tenant Service, NOT a fallback for network errors.
///
/// This exists because the Go team has not yet implemented the
/// `GET /internal/resolve-channel` endpoint. Once they do, this field
/// should be removed and the env var deprecated.
///
/// - TTL: 5 minutes (channel->tenant mapping changes rarely)
/// - Max capacity: 100K entries
/// - Eviction: LRU when capacity is reached
#[derive(Clone)]
pub struct ChannelCache {
    inner: Cache<ChannelLookupKey, Arc<TenantResolution>>,
    /// Static tenant overrides keyed by channel_key (e.g., phone_number_id).
    /// Checked BEFORE the HTTP call to the Tenant Service.
    /// Empty HashMap if no static map is configured.
    static_overrides: Arc<HashMap<String, TenantResolution>>,
}

impl ChannelCache {
    /// Create a new channel cache with the given TTL and max capacity.
    ///
    /// `static_overrides` is a mapping of `channel_key -> TenantResolution`
    /// for MVP tenant resolution without the Go team's resolve-channel endpoint.
    /// Pass an empty HashMap if no static overrides are configured.
    pub fn new(
        ttl_secs: u64,
        max_entries: u64,
        static_overrides: HashMap<String, TenantResolution>,
    ) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_entries)
            .time_to_live(Duration::from_secs(ttl_secs))
            .build();

        Self {
            inner: cache,
            static_overrides: Arc::new(static_overrides),
        }
    }

    /// Resolve a channel to its tenant, using the cache or falling back to
    /// the Tenant Service.
    ///
    /// Resolution order:
    /// 1. Check moka cache (in-memory)
    /// 2. On cache miss -> check static_overrides by channel_key (explicit override)
    /// 3. If not in static map -> call Tenant Service HTTP endpoint
    /// 4. On HTTP failure -> return error (no silent fallback to avoid split-brain)
    ///
    /// `try_get_with` ensures only one inflight request per key (thundering herd).
    pub async fn resolve(
        &self,
        key: &ChannelLookupKey,
        tenant_client: &TenantClient,
    ) -> Result<Arc<TenantResolution>, AppError> {
        let key = key.clone();
        let client = tenant_client.clone();
        let overrides = self.static_overrides.clone();

        self.inner
            .try_get_with(key.clone(), async move {
                // Check static overrides first (explicit override, not a fallback)
                if let Some(resolution) = overrides.get(&key.channel_key) {
                    tracing::debug!(
                        channel_key = %key.channel_key,
                        tenant_id = %resolution.tenant_id,
                        "resolved tenant from static override map"
                    );
                    return Ok::<_, AppError>(Arc::new(resolution.clone()));
                }

                // Fall through to HTTP call to Tenant Service
                let resolution = client.resolve_channel(&key).await?;
                Ok(Arc::new(resolution))
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
