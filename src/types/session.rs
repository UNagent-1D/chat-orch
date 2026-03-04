use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ingest_message::ChannelType;

/// Composite key for session lookup in Redis.
///
/// Format when serialized: `session:{tenant_id}:{channel_type}:{channel_user_id}`
///
/// This key includes `tenant_id` to prevent cross-tenant session leakage:
/// the same WhatsApp user talking to two different tenants' bots gets
/// two separate sessions.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionKey {
    pub tenant_id: Uuid,
    pub channel_type: ChannelType,
    pub channel_user_id: String,
}

impl SessionKey {
    /// Serialize to a Redis key string.
    ///
    /// Format: `session:{tenant_id}:{channel_type}:{channel_user_id}`
    pub fn to_redis_key(&self) -> String {
        format!(
            "session:{}:{}:{}",
            self.tenant_id,
            self.channel_type.as_str(),
            self.channel_user_id
        )
    }
}

/// Configuration references for an agent's active runtime config.
///
/// Returned by the Entrypoint when a conversation is opened,
/// and used to fetch the agent config from the ACR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigRefs {
    pub agent_profile_id: Uuid,
    pub agent_config_id: Uuid,
    pub config_version: u32,
}

/// A conversation session stored in Redis.
///
/// Sessions are keyed by `SessionKey` and have an idle TTL (default 30 min).
/// Each message refreshes the TTL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique conversation identifier.
    pub conversation_id: Uuid,

    /// Opaque session token for REST API clients.
    pub session_token: String,

    /// Which tenant this session belongs to.
    pub tenant_id: Uuid,

    /// Config references for this session's agent.
    pub config_refs: ConfigRefs,

    /// When the session was first created.
    pub created_at: DateTime<Utc>,

    /// When the last message was processed in this session.
    pub last_activity: DateTime<Utc>,
}

impl Session {
    /// Create a new session with a fresh conversation ID and token.
    pub fn new(tenant_id: Uuid, config_refs: ConfigRefs) -> Self {
        let now = Utc::now();
        Self {
            conversation_id: Uuid::new_v4(),
            session_token: format!("ses_{}", Uuid::new_v4().simple()),
            tenant_id,
            config_refs,
            created_at: now,
            last_activity: now,
        }
    }

    /// Touch the session to refresh its idle TTL.
    pub fn touch(&mut self) {
        self.last_activity = Utc::now();
    }
}
