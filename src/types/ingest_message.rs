use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::message_content::MessageContent;
use super::resolved_message::ResolvedMessage;
use super::tenant::{ChannelLookupKey, TenantResolution};

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

    /// Raw channel-specific metadata preserved for debugging.
    ///
    /// **Callers MUST truncate this to a reasonable size (e.g. 1 KB) before
    /// constructing `IngestMessage`.** The type itself does not enforce a size
    /// limit — truncation is the responsibility of the webhook handler.
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
