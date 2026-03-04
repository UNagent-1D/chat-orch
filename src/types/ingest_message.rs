use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::message_content::MessageContent;
use super::resolved_message::ResolvedMessage;

/// The channel type of the originating platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Telegram,
    Whatsapp,
    WebWidget,
}

impl ChannelType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChannelType::Telegram => "telegram",
            ChannelType::Whatsapp => "whatsapp",
            ChannelType::WebWidget => "web_widget",
        }
    }
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lookup key for resolving channel → tenant mapping.
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

/// Phase 1 message: webhook parsed and normalized, but tenant is NOT yet known.
///
/// This is the output of the ingest layer (Telegram/WhatsApp webhook handlers).
/// It cannot be processed further until `resolve()` is called with a
/// `TenantResolution` — this is enforced at compile time by the TypeState pattern.
///
/// # TypeState Transition
/// ```text
/// IngestMessage --resolve(tenant)--> ResolvedMessage
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestMessage {
    /// Channel-specific message ID (Telegram: `update_id`, WhatsApp: `wamid.xxx`).
    pub id: String,

    /// Which platform this message came from.
    pub channel_type: ChannelType,

    /// User identifier within the channel (Telegram: user.id as string, WhatsApp: phone number).
    pub channel_user_id: String,

    /// Channel-level identifier for tenant resolution.
    /// Telegram: tenant_slug (from URL). WhatsApp: `phone_number_id` (from metadata).
    pub channel_key: String,

    /// The actual message content (text, image, location, etc.).
    pub content: MessageContent,

    /// If this message is a reply to another message.
    pub reply_to_id: Option<String>,

    /// When the message was sent by the user.
    pub timestamp: DateTime<Utc>,

    /// Preserved raw channel-specific metadata for debugging.
    /// Truncated to prevent memory bloat.
    pub raw_metadata: Option<serde_json::Value>,
}

impl IngestMessage {
    /// Resolve this message to a specific tenant, producing a `ResolvedMessage`.
    ///
    /// This is the TypeState transition — after this call, `tenant_id` and
    /// `agent_profile_id` are guaranteed to be present.
    pub fn resolve(self, tenant: TenantResolution) -> ResolvedMessage {
        ResolvedMessage {
            id: self.id,
            channel_type: self.channel_type,
            channel_user_id: self.channel_user_id,
            channel_key: self.channel_key,
            tenant_id: tenant.tenant_id,
            tenant_slug: tenant.tenant_slug,
            agent_profile_id: tenant.agent_profile_id,
            content: self.content,
            reply_to_id: self.reply_to_id,
            timestamp: self.timestamp,
            raw_metadata: self.raw_metadata,
        }
    }

    /// Build the channel lookup key for tenant resolution cache.
    pub fn channel_lookup_key(&self) -> ChannelLookupKey {
        ChannelLookupKey {
            channel_type: self.channel_type,
            channel_key: self.channel_key.clone(),
        }
    }
}
