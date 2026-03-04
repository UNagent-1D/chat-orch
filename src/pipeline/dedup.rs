use crate::error::AppError;

/// Redis-backed message deduplication.
///
/// Uses `SET key value NX EX ttl` (atomic SETNX with expiry) to guarantee
/// exactly-once processing per message ID within a 24-hour window.
///
/// Key format: `dedup:{channel_type}:{message_id}`
///
/// Both Telegram and WhatsApp can deliver the same webhook multiple times
/// (at-least-once delivery). This store prevents double-processing.
///
/// The check happens in the webhook handler BEFORE entering the pipeline,
/// so duplicates never consume semaphore permits.
#[derive(Clone)]
pub struct RedisDedup {
    client: redis::Client,
    ttl_secs: u64,
}

impl RedisDedup {
    /// Create a new dedup store.
    pub fn new(redis_url: &str, ttl_secs: u64) -> Result<Self, AppError> {
        let client = redis::Client::open(redis_url)
            .map_err(|e| AppError::Redis(format!("failed to create redis client for dedup: {e}")))?;

        Ok(Self { client, ttl_secs })
    }

    /// Check if a message has already been seen. If not, mark it as seen.
    ///
    /// Returns `true` if this is a NEW message (first time seen).
    /// Returns `false` if the message is a DUPLICATE (already processed).
    ///
    /// This is atomic: `SET key "1" NX EX ttl` returns:
    /// - `Some("OK")` if the key was set (new message)
    /// - `None` if the key already existed (duplicate)
    ///
    /// The TTL ensures old entries are cleaned up automatically.
    pub async fn check_and_mark(
        &self,
        channel_type: &str,
        message_id: &str,
    ) -> Result<bool, AppError> {
        let key = format!("dedup:{channel_type}:{message_id}");
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| AppError::Redis(format!("redis connection failed: {e}")))?;

        // SET key "1" NX EX ttl — atomic set-if-not-exists with expiry
        let result: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg("1")
            .arg("NX")
            .arg("EX")
            .arg(self.ttl_secs)
            .query_async(&mut conn)
            .await
            .map_err(|e| AppError::Redis(format!("redis SETNX failed: {e}")))?;

        // SET NX returns "OK" if set succeeded (new), None if key existed (dup)
        Ok(result.is_some())
    }
}
