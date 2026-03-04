use redis::AsyncCommands;
use uuid::Uuid;

use crate::error::AppError;
use crate::types::session::{ConfigRefs, Session, SessionKey};

/// Redis-backed session store for conversation state.
///
/// Sessions are stored as JSON in Redis with a composite key:
/// `session:{tenant_id}:{channel_type}:{channel_user_id}`
///
/// The idle TTL is refreshed on every access (both read and write),
/// so active conversations stay alive while idle ones expire.
///
/// This store is designed for horizontal scaling: all orchestrator
/// replicas share the same Redis instance, preventing split-brain sessions.
#[derive(Clone)]
pub struct RedisSessionStore {
    client: redis::Client,
    ttl_secs: u64,
}

impl RedisSessionStore {
    /// Create a new session store.
    pub fn new(redis_url: &str, ttl_secs: u64) -> Result<Self, AppError> {
        let client = redis::Client::open(redis_url)
            .map_err(|e| AppError::Redis(format!("failed to create redis client: {e}")))?;

        Ok(Self { client, ttl_secs })
    }

    /// Get an existing session or create a new one.
    ///
    /// If a session exists for this key, it is returned and its TTL is refreshed.
    /// If no session exists, a new one is created with the given config refs.
    pub async fn get_or_create(
        &self,
        key: &SessionKey,
        tenant_id: Uuid,
        config_refs: ConfigRefs,
    ) -> Result<Session, AppError> {
        let redis_key = key.to_redis_key();
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| AppError::Redis(format!("redis connection failed: {e}")))?;

        // Try to get existing session
        let existing: Option<String> = conn
            .get(&redis_key)
            .await
            .map_err(|e| AppError::Redis(format!("redis GET failed: {e}")))?;

        if let Some(json) = existing {
            // Refresh TTL on access
            let _: () = conn
                .expire(&redis_key, self.ttl_secs as i64)
                .await
                .map_err(|e| AppError::Redis(format!("redis EXPIRE failed: {e}")))?;

            let session: Session = serde_json::from_str(&json)
                .map_err(|e| AppError::Redis(format!("invalid session JSON: {e}")))?;

            return Ok(session);
        }

        // Create new session
        let session = Session::new(tenant_id, config_refs);
        let json = serde_json::to_string(&session)
            .map_err(|e| AppError::SessionCreation(format!("failed to serialize session: {e}")))?;

        // SET with TTL
        let _: () = conn
            .set_ex(&redis_key, &json, self.ttl_secs)
            .await
            .map_err(|e| AppError::Redis(format!("redis SET failed: {e}")))?;

        tracing::info!(
            conversation_id = %session.conversation_id,
            tenant_id = %tenant_id,
            "new session created"
        );

        Ok(session)
    }

    /// Get a session by its token (for REST API clients).
    ///
    /// Scans is expensive — for MVP we store a reverse index:
    /// `token:{session_token}` → redis_key
    pub async fn get_by_token(&self, token: &str) -> Result<Option<Session>, AppError> {
        let token_key = format!("token:{token}");
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| AppError::Redis(format!("redis connection failed: {e}")))?;

        // Look up the session key from the token index
        let session_key: Option<String> = conn
            .get(&token_key)
            .await
            .map_err(|e| AppError::Redis(format!("redis GET token failed: {e}")))?;

        match session_key {
            Some(redis_key) => {
                let json: Option<String> = conn
                    .get(&redis_key)
                    .await
                    .map_err(|e| AppError::Redis(format!("redis GET session failed: {e}")))?;

                match json {
                    Some(j) => {
                        let session: Session = serde_json::from_str(&j)
                            .map_err(|e| AppError::Redis(format!("invalid session JSON: {e}")))?;
                        Ok(Some(session))
                    }
                    None => Ok(None), // Session expired but token index lingered
                }
            }
            None => Ok(None),
        }
    }

    /// Store a token → session_key reverse index for REST API lookups.
    pub async fn index_token(
        &self,
        session: &Session,
        session_key: &SessionKey,
    ) -> Result<(), AppError> {
        let token_redis_key = format!("token:{}", session.session_token);
        let session_redis_key = session_key.to_redis_key();
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| AppError::Redis(format!("redis connection failed: {e}")))?;

        let _: () = conn
            .set_ex(&token_redis_key, &session_redis_key, self.ttl_secs)
            .await
            .map_err(|e| AppError::Redis(format!("redis SET token index failed: {e}")))?;

        Ok(())
    }

    /// Invalidate a session (e.g., on 401 from downstream).
    pub async fn invalidate(&self, key: &SessionKey) -> Result<(), AppError> {
        let redis_key = key.to_redis_key();
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| AppError::Redis(format!("redis connection failed: {e}")))?;

        let _: () = conn
            .del(&redis_key)
            .await
            .map_err(|e| AppError::Redis(format!("redis DEL failed: {e}")))?;

        Ok(())
    }

    /// Check Redis connectivity (for readiness probe).
    pub async fn ping(&self) -> Result<(), AppError> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| AppError::Redis(format!("redis connection failed: {e}")))?;

        let _: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .map_err(|e| AppError::Redis(format!("redis PING failed: {e}")))?;

        Ok(())
    }
}
