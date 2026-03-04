use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;

use crate::error::AppError;

use super::acr_client::{AcrClient, ToolRegistryEntry};

/// Cache for the global tool registry from the ACR service.
///
/// This is a **global singleton cache** (not per-tenant). The tool registry
/// is a single catalog of all available tools with their OpenAI function
/// definitions. It changes rarely (only when tools are registered/updated).
///
/// # Design Note: key=() is intentional
///
/// Using `()` as the cache key and max_capacity=1 is deliberate. The tool
/// registry is a single global list, not a per-key lookup. We use moka
/// (rather than a custom `RwLock<Option<...>>`) for consistency with the
/// 4 other caches in this codebase: same API, same thundering-herd
/// protection via `try_get_with()`, same TTL behavior.
///
/// - TTL: 5 minutes (tools change rarely)
/// - Max capacity: 1 entry (it's a single global list)
/// - Thundering herd protection: `try_get_with()` ensures only one
///   inflight fetch when the cache is cold
#[derive(Clone)]
pub struct ToolRegistryCache {
    inner: Cache<(), Arc<Vec<ToolRegistryEntry>>>,
}

impl ToolRegistryCache {
    /// Create a new tool registry cache with the given TTL and max capacity.
    pub fn new(ttl_secs: u64, max_entries: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_entries)
            .time_to_live(Duration::from_secs(ttl_secs))
            .build();

        Self { inner: cache }
    }

    /// Get the tool registry, using the cache or falling back to the ACR service.
    ///
    /// On fetch failure, returns an empty registry with a warning log.
    /// This means LLM turns gracefully degrade to the constraints-only
    /// behavior (auto-generated descriptions + empty parameter schemas)
    /// rather than blocking all conversation turns.
    pub async fn resolve(
        &self,
        acr_client: &AcrClient,
    ) -> Arc<Vec<ToolRegistryEntry>> {
        let client = acr_client.clone();

        let result = self
            .inner
            .try_get_with((), async move {
                let entries = client.get_global_tool_registry().await?;
                Ok::<_, AppError>(Arc::new(entries))
            })
            .await;

        match result {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "tool registry unavailable — using constraints-only tool definitions"
                );
                Arc::new(vec![])
            }
        }
    }

    /// Number of entries in the cache (0 or 1).
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }
}
