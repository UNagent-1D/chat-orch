use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ingest_message::ChannelType;
use super::message_content::MessageContent;
use super::session::SessionKey;

/// Phase 2 message: tenant is resolved and guaranteed present.
///
/// Created exclusively via [`IngestMessage::resolve()`]. Fields are
/// `pub(crate)` so nothing outside this crate can construct the struct
/// directly — the TypeState transition is the only entry point.
///
/// Read access is provided through public accessor methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedMessage {
    /// Channel-specific message ID.
    pub(crate) id: String,

    /// Which platform this message came from.
    pub(crate) channel_type: ChannelType,

    /// User identifier within the channel.
    pub(crate) channel_user_id: String,

    /// Channel-level identifier (phone_number_id or tenant_slug).
    pub(crate) channel_key: String,

    /// The tenant this message belongs to. **Guaranteed present.**
    pub(crate) tenant_id: Uuid,

    /// The tenant's URL-safe slug (e.g., "hospital-san-ignacio").
    pub(crate) tenant_slug: String,

    /// Which agent profile handles this tenant's conversations. **Guaranteed present.**
    pub(crate) agent_profile_id: Uuid,

    /// The actual message content.
    pub(crate) content: MessageContent,

    /// If this message is a reply to another message.
    pub(crate) reply_to_id: Option<String>,

    /// When the message was sent by the user.
    pub(crate) timestamp: DateTime<Utc>,

    /// Raw channel-specific metadata preserved for debugging.
    /// Already truncated by the webhook handler that built the `IngestMessage`.
    pub(crate) raw_metadata: Option<serde_json::Value>,
}

impl ResolvedMessage {
    // ── Accessors ────────────────────────────────────────────────────

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn channel_type(&self) -> ChannelType {
        self.channel_type
    }

    pub fn channel_user_id(&self) -> &str {
        &self.channel_user_id
    }

    pub fn channel_key(&self) -> &str {
        &self.channel_key
    }

    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    pub fn tenant_slug(&self) -> &str {
        &self.tenant_slug
    }

    pub fn agent_profile_id(&self) -> Uuid {
        self.agent_profile_id
    }

    pub fn content(&self) -> &MessageContent {
        &self.content
    }

    pub fn reply_to_id(&self) -> Option<&str> {
        self.reply_to_id.as_deref()
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }

    pub fn raw_metadata(&self) -> Option<&serde_json::Value> {
        self.raw_metadata.as_ref()
    }

    // ── Derived helpers ──────────────────────────────────────────────

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
