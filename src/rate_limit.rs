// In-memory token-bucket rate limiter.
//
// Two independent limiter sets are exposed:
//   * `tenant_chat` — keyed by `tenant_id`, guarding `/v1/chat` and
//     `/v1/chat/stream`. Defends against LLM-cost denial-of-wallet and
//     keeps one noisy tenant from starving the others.
//   * `tenant_feedback` — keyed by `tenant_id`, guarding `/v1/feedback`.
//
// The limiter is process-local. The single-replica P2/P3 deployment makes
// that adequate; if the platform later scales horizontally the buckets can
// be moved to Redis (already present in `docker-compose.yml`) without
// changing call sites.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct BucketConfig {
    pub capacity: f64,
    pub refill_per_sec: f64,
}

impl BucketConfig {
    pub fn new(capacity: f64, refill_per_sec: f64) -> Self {
        Self {
            capacity: capacity.max(1.0),
            refill_per_sec: refill_per_sec.max(0.0001),
        }
    }
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

impl Bucket {
    fn full(cfg: BucketConfig) -> Self {
        Self {
            tokens: cfg.capacity,
            last_refill: Instant::now(),
        }
    }

    fn try_consume(&mut self, cfg: BucketConfig) -> Result<(), Duration> {
        let now = Instant::now();
        let elapsed = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f64();
        self.tokens = (self.tokens + elapsed * cfg.refill_per_sec).min(cfg.capacity);
        self.last_refill = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            let missing = 1.0 - self.tokens;
            let wait = (missing / cfg.refill_per_sec).max(0.001);
            Err(Duration::from_secs_f64(wait))
        }
    }
}

#[derive(Debug)]
pub struct RateLimiter {
    cfg: BucketConfig,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    pub fn new(cfg: BucketConfig) -> Self {
        Self {
            cfg,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, key: &str) -> Result<(), Duration> {
        let mut guard = self.buckets.lock().expect("rate-limit mutex poisoned");
        let bucket = guard
            .entry(key.to_string())
            .or_insert_with(|| Bucket::full(self.cfg));
        bucket.try_consume(self.cfg)
    }
}

#[derive(Debug)]
pub struct Limiters {
    pub tenant_chat: RateLimiter,
    pub tenant_feedback: RateLimiter,
}

impl Limiters {
    pub fn from_settings(cfg: &LimiterSettings) -> Self {
        Self {
            tenant_chat: RateLimiter::new(BucketConfig::new(cfg.chat_burst, cfg.chat_per_sec)),
            tenant_feedback: RateLimiter::new(BucketConfig::new(
                cfg.feedback_burst,
                cfg.feedback_per_sec,
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LimiterSettings {
    pub chat_burst: f64,
    pub chat_per_sec: f64,
    pub feedback_burst: f64,
    pub feedback_per_sec: f64,
}

impl LimiterSettings {
    pub fn from_env() -> Self {
        Self {
            chat_burst: parse_env_f64("RATE_LIMIT_CHAT_BURST", 20.0),
            chat_per_sec: parse_env_f64("RATE_LIMIT_CHAT_PER_SEC", 1.0),
            feedback_burst: parse_env_f64("RATE_LIMIT_FEEDBACK_BURST", 10.0),
            feedback_per_sec: parse_env_f64("RATE_LIMIT_FEEDBACK_PER_SEC", 0.5),
        }
    }
}

/// Per-chat flood guard for the Telegram ingress, keyed by `chat_id`.
/// Burst 5 then 1 message every 2 s — humans never notice, scripts do.
pub fn telegram_bucket_from_env() -> BucketConfig {
    BucketConfig::new(
        parse_env_f64("TELEGRAM_RATE_LIMIT_BURST", 5.0),
        parse_env_f64("TELEGRAM_RATE_LIMIT_PER_SEC", 0.5),
    )
}

/// Throttle for the "slow down" warning itself (one per chat every 30 s),
/// so the guard can't be abused to make the bot spam its own warnings.
pub fn telegram_warn_bucket() -> BucketConfig {
    BucketConfig::new(1.0, 1.0 / 30.0)
}

fn parse_env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_capacity_then_denies() {
        let limiter = RateLimiter::new(BucketConfig::new(3.0, 0.0001));
        assert!(limiter.check("t1").is_ok());
        assert!(limiter.check("t1").is_ok());
        assert!(limiter.check("t1").is_ok());
        assert!(limiter.check("t1").is_err());
    }

    #[test]
    fn buckets_are_independent_per_key() {
        let limiter = RateLimiter::new(BucketConfig::new(1.0, 0.0001));
        assert!(limiter.check("a").is_ok());
        assert!(limiter.check("a").is_err());
        assert!(limiter.check("b").is_ok());
    }
}
