use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ingest_message::ChannelType;
use super::message_content::MessageContent;
use super::session::SessionKey;

/// Phase 2 message: tenant is resolved and guaranteed present.
///
/// Created via `IngestMessage::resolve(tenant)`. The `tenant_id` and
/// `agent_profile_id` fields are NOT `Option` — they are guaranteed to
/// exist at compile time. This prevents any code path from accidentally
/// processing a message without a known tenant.
///
/// # Usage
/// ```text
/// // You CANNOT construct this directly — must go through IngestMessage::resolve()
/// let resolved: ResolvedMessage = ingest_msg.resolve(tenant_resolution);
/// // Now tenant_id is guaranteed:
/// let tenant = resolved.tenant_id; // Uuid, not Option<Uuid>
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedMessage {
    /// Channel-specific message ID.
    pub id: String,

    /// Which platform this message came from.
    pub channel_type: ChannelType,

    /// User identifier within the channel.
    pub channel_user_id: String,

    /// Channel-level identifier (phone_number_id or tenant_slug).
    pub channel_key: String,

    /// The tenant this message belongs to. **Guaranteed present.**
    pub tenant_id: Uuid,

    /// The tenant's URL-safe slug (e.g., "hospital-san-ignacio").
    pub tenant_slug: String,

    /// Which agent profile handles this tenant's conversations. **Guaranteed present.**
    pub agent_profile_id: Uuid,

    /// The actual message content.
    pub content: MessageContent,

    /// If this message is a reply to another message.
    pub reply_to_id: Option<String>,

    /// When the message was sent by the user.
    pub timestamp: DateTime<Utc>,

    /// Preserved raw channel-specific metadata for debugging.
    pub raw_metadata: Option<serde_json::Value>,
}

impl ResolvedMessage {
    /// Build the session key for Redis session lookup.
    ///
    /// The resulting `SessionKey` serializes to Redis key format:
    /// `session:{tenant_id}:{channel_type}:{channel_user_id}`
    pub fn session_key(&self) -> SessionKey {
        SessionKey {
            tenant_id: self.tenant_id,
            channel_type: self.channel_type,
            channel_user_id: self.channel_user_id.clone(),
        }
    }
}
