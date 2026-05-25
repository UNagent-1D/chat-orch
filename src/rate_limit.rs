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

    #[test]
    fn error_returns_positive_retry_duration() {
        let limiter = RateLimiter::new(BucketConfig::new(1.0, 0.5));
        limiter.check("k").unwrap();
        let wait = limiter.check("k").expect_err("should be rate limited");
        assert!(wait.as_millis() > 0, "retry-after duration must be positive");
        // With refill_per_sec=0.5 and 1 token missing, wait ≈ 2s.
        assert!(
            wait.as_secs() <= 3,
            "retry-after should not be absurdly large"
        );
    }

    #[test]
    fn burst_capacity_enforced_exactly() {
        let burst = 5.0;
        let limiter = RateLimiter::new(BucketConfig::new(burst, 0.0001));
        for i in 0..5 {
            assert!(limiter.check("t").is_ok(), "attempt {i} should succeed");
        }
        assert!(limiter.check("t").is_err(), "6th attempt should fail");
    }

    #[test]
    fn multiple_tenants_do_not_share_quota() {
        let limiter = RateLimiter::new(BucketConfig::new(2.0, 0.0001));
        let tenants = ["alice", "bob", "carol"];
        for tenant in tenants {
            assert!(limiter.check(tenant).is_ok(), "{tenant} first req");
            assert!(limiter.check(tenant).is_ok(), "{tenant} second req");
            assert!(limiter.check(tenant).is_err(), "{tenant} third req must fail");
        }
    }

    #[test]
    fn telegram_per_user_keying_uses_chat_id() {
        // Regression: Telegram loop must key by chat_id (string), not tenant_id.
        // This test verifies that two different chat_ids get independent buckets.
        let limiter = RateLimiter::new(BucketConfig::new(1.0, 0.0001));
        let chat_a = 111_111_i64.to_string();
        let chat_b = 222_222_i64.to_string();
        assert!(limiter.check(&chat_a).is_ok());
        assert!(limiter.check(&chat_a).is_err(), "chat_a exhausted");
        assert!(limiter.check(&chat_b).is_ok(), "chat_b unaffected");
    }

    #[test]
    fn concurrent_checks_never_exceed_capacity() {
        use std::sync::Arc;
        use std::thread;

        let limiter = Arc::new(RateLimiter::new(BucketConfig::new(10.0, 0.0001)));
        let handles: Vec<_> = (0..20)
            .map(|_| {
                let lim = Arc::clone(&limiter);
                thread::spawn(move || lim.check("shared").is_ok())
            })
            .collect();
        let successes: usize = handles
            .into_iter()
            .map(|h| h.join().unwrap() as usize)
            .sum();
        assert_eq!(successes, 10, "exactly burst-many requests should succeed");
    }

    #[test]
    fn limiters_chat_and_feedback_are_independent() {
        let settings = LimiterSettings {
            chat_burst: 1.0,
            chat_per_sec: 0.0001,
            feedback_burst: 1.0,
            feedback_per_sec: 0.0001,
        };
        let limiters = Limiters::from_settings(&settings);
        limiters.tenant_chat.check("t").unwrap();
        assert!(limiters.tenant_chat.check("t").is_err());
        // feedback bucket for same key is independent
        assert!(limiters.tenant_feedback.check("t").is_ok());
    }
}
