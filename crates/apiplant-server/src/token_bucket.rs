//! Per-client token buckets: the arithmetic behind `[rate_limit]`.
//!
//! This used to come from `ntex-ratelimiter`, of which only the buckets were
//! used (the middleware that picks a bucket per route is
//! [`crate::rate_limit`]'s own). Each release of that crate tracks one ntex
//! major, and taking a version behind ours compiles a second copy of the whole
//! web stack, so the hundred lines that matter live here instead. Same
//! algorithm, same names.
//!
//! A bucket holds `capacity` tokens and refills continuously at
//! `capacity / window` per second; a request takes one. Buckets nobody has
//! touched for `stale_threshold` are swept every `cleanup_interval` by a task
//! started with the limiter, so a scan from a million addresses does not stay
//! resident.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How a limiter is sized and swept.
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Requests allowed in a full bucket.
    pub capacity: usize,
    /// Seconds it takes an empty bucket to refill completely.
    pub window: u64,
    /// How often stale buckets are swept.
    pub cleanup_interval: Duration,
    /// How long a bucket may go untouched before it is swept.
    pub stale_threshold: Duration,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        RateLimiterConfig {
            capacity: 100,
            window: 60,
            cleanup_interval: Duration::from_secs(300),
            stale_threshold: Duration::from_secs(3600),
        }
    }
}

/// The answer for one request.
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    pub allowed: bool,
    /// Whole tokens left after this request.
    pub remaining: u32,
    /// Unix time at which the bucket will be full again.
    pub reset: u64,
    pub limit: usize,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// One limit, with a bucket per client address.
pub struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
    config: RateLimiterConfig,
}

impl RateLimiter {
    /// A limiter with the default sweep settings.
    #[cfg(test)]
    pub fn new(capacity: usize, window: u64) -> Arc<RateLimiter> {
        RateLimiter::with_config(RateLimiterConfig {
            capacity,
            window,
            ..RateLimiterConfig::default()
        })
    }

    /// A limiter, and, when called inside a tokio runtime, the task that
    /// sweeps it. The task holds a weak reference, so it ends with the
    /// limiter.
    pub fn with_config(config: RateLimiterConfig) -> Arc<RateLimiter> {
        assert!(
            config.window > 0,
            "a rate limit window must be longer than zero"
        );
        let limiter = Arc::new(RateLimiter {
            buckets: Mutex::new(HashMap::new()),
            config,
        });
        if tokio::runtime::Handle::try_current().is_ok() {
            let weak = Arc::downgrade(&limiter);
            let every = limiter.config.cleanup_interval.max(Duration::from_secs(1));
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(every);
                tick.tick().await; // the first tick is immediate
                loop {
                    tick.tick().await;
                    match weak.upgrade() {
                        Some(limiter) => limiter.sweep(),
                        None => break,
                    }
                }
            });
        }
        limiter
    }

    /// Take a token for `client`, if there is one.
    pub fn check_rate_limit(&self, client: IpAddr) -> RateLimitResult {
        let now = Instant::now();
        let capacity = self.config.capacity as f64;
        let rate = capacity / self.config.window as f64;

        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let bucket = buckets.entry(client).or_insert(Bucket {
            tokens: capacity,
            last_refill: now,
        });
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * rate).min(capacity);
        bucket.last_refill = now;

        let allowed = bucket.tokens >= 1.0;
        if allowed {
            bucket.tokens -= 1.0;
        }
        let missing = capacity - bucket.tokens;
        RateLimitResult {
            allowed,
            remaining: bucket.tokens.floor() as u32,
            reset: unix_now() + (missing / rate).ceil() as u64,
            limit: self.config.capacity,
        }
    }

    /// How many requests a full bucket allows.
    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.config.capacity
    }

    /// Drop every bucket untouched for longer than the stale threshold.
    fn sweep(&self) {
        let now = Instant::now();
        let stale = self.config.stale_threshold;
        self.buckets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, bucket| now.duration_since(bucket.last_refill) <= stale);
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_bucket_allows_its_capacity_then_refuses() {
        let limiter = RateLimiter::new(3, 60);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let results: Vec<_> = (0..4).map(|_| limiter.check_rate_limit(ip)).collect();
        assert!(results[..3].iter().all(|r| r.allowed));
        assert_eq!(results[2].remaining, 0);
        assert!(!results[3].allowed);
        assert!(results[3].reset > unix_now());
    }

    #[test]
    fn clients_have_separate_buckets() {
        let limiter = RateLimiter::new(1, 60);
        assert!(
            limiter
                .check_rate_limit("10.0.0.1".parse().unwrap())
                .allowed
        );
        assert!(
            limiter
                .check_rate_limit("10.0.0.2".parse().unwrap())
                .allowed
        );
        assert!(
            !limiter
                .check_rate_limit("10.0.0.1".parse().unwrap())
                .allowed
        );
    }

    #[test]
    fn stale_buckets_are_swept() {
        let limiter = RateLimiter::with_config(RateLimiterConfig {
            capacity: 1,
            window: 60,
            cleanup_interval: Duration::from_secs(60),
            stale_threshold: Duration::ZERO,
        });
        limiter.check_rate_limit("10.0.0.1".parse().unwrap());
        std::thread::sleep(Duration::from_millis(5));
        limiter.sweep();
        assert!(limiter.buckets.lock().unwrap().is_empty());
    }
}
