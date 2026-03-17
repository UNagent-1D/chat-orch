use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ingest_message::ChannelType;

/// Lookup key for resolving channel -> tenant mapping.
///
/// Used as a cache key in the channel_cache (moka).
/// `channel_type + channel_key` together are globally unique.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ChannelLookupKey {
    pub channel_type: ChannelType,
    /// For Telegram: bot token hash. For WhatsApp: `phone_number_id`.
    pub channel_key: String,
}

/// Result of resolving a channel to its tenant.
///
/// Returned by `GET /internal/resolve-channel` on the Tenant Service
/// (or from the moka cache).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantResolution {
    pub tenant_id: Uuid,
    pub tenant_slug: String,
    pub agent_profile_id: Uuid,
    pub webhook_secret_ref: String,
    pub is_active: bool,
}
