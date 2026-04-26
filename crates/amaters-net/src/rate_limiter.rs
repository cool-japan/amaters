//! Token bucket rate limiter for the network/server layer
//!
//! Provides per-client and global rate limiting using a token bucket algorithm.
//! Designed for concurrent access in a gRPC server environment.
//!
//! # Example
//!
//! ```rust,ignore
//! use amaters_net::rate_limiter::{RateLimiter, RateLimiterConfig};
//!
//! let config = RateLimiterConfig::new(100.0, 50);
//! let limiter = RateLimiter::new(config);
//!
//! // Check rate limit for a client
//! match limiter.check_rate_limit("client-1") {
//!     Ok(()) => { /* request allowed */ }
//!     Err(e) => { /* rate limited */ }
//! }
//! ```

use dashmap::DashMap;
use parking_lot::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Rate limiter configuration
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Maximum sustained requests per second
    pub requests_per_second: f64,
    /// Maximum burst size (peak capacity above sustained rate)
    pub burst_size: u32,
    /// Whether to track per-client limits
    pub per_client: bool,
    /// Optional global limit (requests per second across all clients)
    pub global_limit: Option<u32>,
    /// Duration after which idle client buckets are eligible for cleanup
    pub idle_timeout: Duration,
}

impl RateLimiterConfig {
    /// Create a new rate limiter configuration
    ///
    /// # Arguments
    /// * `requests_per_second` - Sustained request rate
    /// * `burst_size` - Maximum burst capacity
    pub fn new(requests_per_second: f64, burst_size: u32) -> Self {
        Self {
            requests_per_second,
            burst_size,
            per_client: true,
            global_limit: None,
            idle_timeout: Duration::from_secs(300), // 5 minutes default
        }
    }

    /// Set whether to use per-client tracking
    #[must_use]
    pub fn with_per_client(mut self, per_client: bool) -> Self {
        self.per_client = per_client;
        self
    }

    /// Set a global rate limit
    #[must_use]
    pub fn with_global_limit(mut self, limit: u32) -> Self {
        self.global_limit = Some(limit);
        self
    }

    /// Set the idle timeout for client bucket cleanup
    #[must_use]
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self::new(100.0, 50)
    }
}

/// Error returned when a rate limit is exceeded
#[derive(Debug, Clone)]
pub enum RateLimitError {
    /// The global rate limit has been exceeded
    GlobalLimitExceeded {
        /// Milliseconds the client should wait before retrying
        retry_after_ms: u64,
    },
    /// A per-client rate limit has been exceeded
    ClientLimitExceeded {
        /// Identifier of the rate-limited client
        client_id: String,
        /// Milliseconds the client should wait before retrying
        retry_after_ms: u64,
    },
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RateLimitError::GlobalLimitExceeded { retry_after_ms } => {
                write!(
                    f,
                    "Global rate limit exceeded, retry after {}ms",
                    retry_after_ms
                )
            }
            RateLimitError::ClientLimitExceeded {
                client_id,
                retry_after_ms,
            } => {
                write!(
                    f,
                    "Rate limit exceeded for client '{}', retry after {}ms",
                    client_id, retry_after_ms
                )
            }
        }
    }
}

impl std::error::Error for RateLimitError {}

/// Token bucket implementation
///
/// Tracks available tokens and refills them at a constant rate.
/// Allows bursts up to `max_tokens` followed by sustained usage at `refill_rate`.
#[derive(Debug)]
pub struct TokenBucket {
    /// Current number of available tokens (can be fractional during refill)
    tokens: f64,
    /// Maximum token capacity (determines burst size)
    max_tokens: f64,
    /// Tokens added per second
    refill_rate: f64,
    /// Timestamp of the last refill calculation
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new token bucket
    ///
    /// # Arguments
    /// * `max_tokens` - Maximum capacity (burst size)
    /// * `refill_rate` - Tokens per second refill rate
    pub fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens,
            max_tokens,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens based on elapsed time since last refill
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;
    }

    /// Try to consume one token, returning true if successful
    pub fn try_acquire(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Get the number of remaining whole tokens (after refill)
    pub fn remaining(&mut self) -> u32 {
        self.refill();
        self.tokens.floor().max(0.0) as u32
    }

    /// Calculate how many milliseconds until at least one token is available
    pub fn retry_after_ms(&self) -> u64 {
        if self.tokens >= 1.0 {
            return 0;
        }
        let deficit = 1.0 - self.tokens;
        if self.refill_rate <= 0.0 {
            return u64::MAX;
        }
        let seconds = deficit / self.refill_rate;
        (seconds * 1000.0).ceil() as u64
    }

    /// Reset the bucket to full capacity
    pub fn reset(&mut self) {
        self.tokens = self.max_tokens;
        self.last_refill = Instant::now();
    }

    /// Return the last time this bucket was accessed (for idle detection)
    pub fn last_access(&self) -> Instant {
        self.last_refill
    }
}

/// Rate limiter supporting per-client and global limits
///
/// Thread-safe: uses `parking_lot::Mutex` for the global bucket and
/// `DashMap` with per-entry `Mutex` for client buckets.
pub struct RateLimiter {
    /// Configuration
    config: RateLimiterConfig,
    /// Global token bucket (shared across all clients)
    global_bucket: Mutex<TokenBucket>,
    /// Per-client token buckets
    client_buckets: DashMap<String, Mutex<TokenBucket>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration
    pub fn new(config: RateLimiterConfig) -> Self {
        let global_max = config
            .global_limit
            .map(f64::from)
            .unwrap_or(config.requests_per_second * 2.0);
        let global_rate = config
            .global_limit
            .map(f64::from)
            .unwrap_or(config.requests_per_second * 2.0);

        Self {
            config: config.clone(),
            global_bucket: Mutex::new(TokenBucket::new(global_max, global_rate)),
            client_buckets: DashMap::new(),
        }
    }

    /// Check rate limit for a given client, returning an error if exceeded
    ///
    /// Checks the per-client limit first, then the global limit (if enabled).
    /// Tokens are only consumed from the global bucket if the per-client check passes,
    /// ensuring that a per-client rejection does not waste global capacity.
    pub fn check_rate_limit(&self, client_id: &str) -> Result<(), RateLimitError> {
        // Check per-client limit first (cheaper, more common rejection reason)
        if self.config.per_client {
            let bucket = self
                .client_buckets
                .entry(client_id.to_string())
                .or_insert_with(|| {
                    Mutex::new(TokenBucket::new(
                        f64::from(self.config.burst_size),
                        self.config.requests_per_second,
                    ))
                });

            let mut bucket_guard = bucket.lock();
            if !bucket_guard.try_acquire() {
                let retry_after_ms = bucket_guard.retry_after_ms();
                debug!(
                    client_id = %client_id,
                    retry_after_ms = retry_after_ms,
                    "Per-client rate limit exceeded"
                );
                return Err(RateLimitError::ClientLimitExceeded {
                    client_id: client_id.to_string(),
                    retry_after_ms,
                });
            }
        }

        // Check global limit (only if per-client check passed)
        if self.config.global_limit.is_some() {
            let mut global = self.global_bucket.lock();
            if !global.try_acquire() {
                let retry_after_ms = global.retry_after_ms();
                warn!(
                    client_id = %client_id,
                    retry_after_ms = retry_after_ms,
                    "Global rate limit exceeded"
                );
                return Err(RateLimitError::GlobalLimitExceeded { retry_after_ms });
            }
        }

        Ok(())
    }

    /// Try to acquire a token for the given client, returning true if allowed
    ///
    /// This is a convenience wrapper around `check_rate_limit`.
    pub fn try_acquire(&self, client_id: &str) -> bool {
        self.check_rate_limit(client_id).is_ok()
    }

    /// Get the number of remaining tokens for a client
    ///
    /// Returns the per-client remaining tokens, or if per-client tracking is
    /// disabled, returns the global remaining tokens.
    pub fn remaining_tokens(&self, client_id: &str) -> u32 {
        if self.config.per_client {
            if let Some(bucket) = self.client_buckets.get(client_id) {
                return bucket.lock().remaining();
            }
            // Client hasn't been seen yet, return full burst capacity
            return self.config.burst_size;
        }

        // Fall back to global bucket
        self.global_bucket.lock().remaining()
    }

    /// Remove client buckets that have been idle longer than the configured timeout
    ///
    /// Returns the number of buckets removed.
    pub fn cleanup_expired_buckets(&self) -> usize {
        let now = Instant::now();
        let timeout = self.config.idle_timeout;
        let mut removed = 0;

        // Collect keys to remove (avoid holding DashMap shard locks during removal)
        let expired_keys: Vec<String> = self
            .client_buckets
            .iter()
            .filter_map(|entry| {
                let bucket = entry.value().lock();
                if now.duration_since(bucket.last_access()) > timeout {
                    Some(entry.key().clone())
                } else {
                    None
                }
            })
            .collect();

        for key in &expired_keys {
            // Re-check under removal to avoid race conditions
            if let Some((_k, bucket)) = self.client_buckets.remove(key) {
                let guard = bucket.lock();
                if now.duration_since(guard.last_access()) > timeout {
                    removed += 1;
                    debug!(client_id = %key, "Cleaned up expired rate limiter bucket");
                } else {
                    // Bucket was accessed between our scan and removal; put it back
                    drop(guard);
                    self.client_buckets.insert(key.clone(), bucket);
                }
            }
        }

        if removed > 0 {
            debug!(count = removed, "Cleaned up expired rate limiter buckets");
        }

        removed
    }

    /// Reset the rate limiter state for a specific client
    pub fn reset(&self, client_id: &str) {
        if let Some(bucket) = self.client_buckets.get(client_id) {
            bucket.lock().reset();
        }
    }

    /// Reset all rate limiter state (global and all clients)
    pub fn reset_all(&self) {
        self.global_bucket.lock().reset();
        self.client_buckets.clear();
    }

    /// Get the current number of tracked clients
    pub fn tracked_client_count(&self) -> usize {
        self.client_buckets.len()
    }

    /// Get a reference to the configuration
    pub fn config(&self) -> &RateLimiterConfig {
        &self.config
    }
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("config", &self.config)
            .field("tracked_clients", &self.client_buckets.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_token_bucket_basic() {
        let mut bucket = TokenBucket::new(5.0, 10.0);

        // Should be able to acquire 5 tokens (burst size)
        for _ in 0..5 {
            assert!(
                bucket.try_acquire(),
                "Should acquire token from full bucket"
            );
        }

        // 6th should fail (no time for refill)
        assert!(!bucket.try_acquire(), "Should fail when bucket is depleted");
    }

    #[test]
    fn test_token_bucket_refill() {
        let mut bucket = TokenBucket::new(3.0, 100.0); // 100 tokens/sec

        // Drain all tokens
        for _ in 0..3 {
            assert!(bucket.try_acquire());
        }
        assert!(!bucket.try_acquire(), "Bucket should be empty");

        // Wait for refill (at 100 tokens/sec, 20ms should give ~2 tokens)
        thread::sleep(Duration::from_millis(25));

        assert!(
            bucket.try_acquire(),
            "Should have refilled at least one token after 25ms at 100/s"
        );
    }

    #[test]
    fn test_token_bucket_remaining() {
        let mut bucket = TokenBucket::new(10.0, 1.0);
        assert_eq!(bucket.remaining(), 10);

        assert!(bucket.try_acquire());
        assert_eq!(bucket.remaining(), 9);
    }

    #[test]
    fn test_token_bucket_retry_after() {
        let mut bucket = TokenBucket::new(1.0, 10.0); // 10 tokens/sec

        // Drain
        assert!(bucket.try_acquire());
        assert!(!bucket.try_acquire());

        let retry = bucket.retry_after_ms();
        // At 10 tokens/sec, refill one token takes 100ms
        // retry_after should be <= 100ms (accounting for time elapsed during test)
        assert!(
            retry <= 110,
            "retry_after_ms should be approximately 100ms, got {}",
            retry
        );
        assert!(retry > 0, "retry_after_ms should be > 0 when depleted");
    }

    #[test]
    fn test_token_bucket_reset() {
        let mut bucket = TokenBucket::new(5.0, 1.0);

        // Drain
        for _ in 0..5 {
            assert!(bucket.try_acquire());
        }
        assert!(!bucket.try_acquire());

        // Reset
        bucket.reset();
        assert_eq!(bucket.remaining(), 5);
        assert!(bucket.try_acquire());
    }

    #[test]
    fn test_per_client_isolation() {
        let config = RateLimiterConfig::new(1000.0, 3).with_per_client(true);
        let limiter = RateLimiter::new(config);

        // Exhaust client A's tokens
        for _ in 0..3 {
            assert!(limiter.check_rate_limit("client-a").is_ok());
        }
        assert!(
            limiter.check_rate_limit("client-a").is_err(),
            "Client A should be rate limited"
        );

        // Client B should still have tokens
        assert!(
            limiter.check_rate_limit("client-b").is_ok(),
            "Client B should not be affected by Client A's limit"
        );
    }

    #[test]
    fn test_global_limit() {
        let config = RateLimiterConfig::new(1000.0, 10)
            .with_per_client(false)
            .with_global_limit(3);
        let limiter = RateLimiter::new(config);

        // Exhaust global limit across different clients
        assert!(limiter.check_rate_limit("client-a").is_ok());
        assert!(limiter.check_rate_limit("client-b").is_ok());
        assert!(limiter.check_rate_limit("client-c").is_ok());

        // 4th request from any client should be denied
        let result = limiter.check_rate_limit("client-d");
        assert!(result.is_err(), "Global limit should be enforced");
        match result {
            Err(RateLimitError::GlobalLimitExceeded { retry_after_ms }) => {
                assert!(retry_after_ms > 0);
            }
            other => panic!("Expected GlobalLimitExceeded, got {:?}", other),
        }
    }

    #[test]
    fn test_burst_handling() {
        let config = RateLimiterConfig::new(10.0, 20).with_per_client(true);
        let limiter = RateLimiter::new(config);

        // Should allow burst of 20 requests quickly
        let mut allowed = 0;
        for _ in 0..25 {
            if limiter.check_rate_limit("burst-client").is_ok() {
                allowed += 1;
            }
        }

        assert_eq!(
            allowed, 20,
            "Should allow exactly burst_size requests in a burst"
        );
    }

    #[test]
    fn test_cleanup_expired() {
        let config = RateLimiterConfig::new(100.0, 5)
            .with_per_client(true)
            .with_idle_timeout(Duration::from_millis(50));
        let limiter = RateLimiter::new(config);

        // Create some client buckets
        assert!(limiter.check_rate_limit("client-1").is_ok());
        assert!(limiter.check_rate_limit("client-2").is_ok());
        assert_eq!(limiter.tracked_client_count(), 2);

        // Wait for idle timeout
        thread::sleep(Duration::from_millis(80));

        let removed = limiter.cleanup_expired_buckets();
        assert_eq!(removed, 2, "Both idle clients should be cleaned up");
        assert_eq!(limiter.tracked_client_count(), 0);
    }

    #[test]
    fn test_cleanup_keeps_active() {
        let config = RateLimiterConfig::new(100.0, 5)
            .with_per_client(true)
            .with_idle_timeout(Duration::from_millis(100));
        let limiter = RateLimiter::new(config);

        // Create client bucket
        assert!(limiter.check_rate_limit("active-client").is_ok());

        // Wait less than idle timeout
        thread::sleep(Duration::from_millis(30));

        // Touch the active client again
        assert!(limiter.check_rate_limit("active-client").is_ok());

        // Create another client that will be idle
        assert!(limiter.check_rate_limit("idle-client").is_ok());

        // Wait enough for idle-client to expire but not active-client
        thread::sleep(Duration::from_millis(120));

        // Touch active client right before cleanup
        assert!(limiter.check_rate_limit("active-client").is_ok());

        let removed = limiter.cleanup_expired_buckets();
        assert_eq!(removed, 1, "Only idle client should be cleaned up");
        assert_eq!(limiter.tracked_client_count(), 1);
    }

    #[test]
    fn test_rate_limit_error_display() {
        let global_err = RateLimitError::GlobalLimitExceeded { retry_after_ms: 42 };
        let msg = format!("{}", global_err);
        assert!(msg.contains("Global rate limit exceeded"));
        assert!(msg.contains("42ms"));

        let client_err = RateLimitError::ClientLimitExceeded {
            client_id: "test-client".to_string(),
            retry_after_ms: 100,
        };
        let msg = format!("{}", client_err);
        assert!(msg.contains("test-client"));
        assert!(msg.contains("100ms"));
    }

    #[test]
    fn test_rate_limit_error_details() {
        let config = RateLimiterConfig::new(10.0, 2).with_per_client(true);
        let limiter = RateLimiter::new(config);

        // Exhaust tokens
        assert!(limiter.check_rate_limit("err-client").is_ok());
        assert!(limiter.check_rate_limit("err-client").is_ok());

        let result = limiter.check_rate_limit("err-client");
        match result {
            Err(RateLimitError::ClientLimitExceeded {
                client_id,
                retry_after_ms,
            }) => {
                assert_eq!(client_id, "err-client");
                assert!(retry_after_ms > 0);
            }
            other => panic!("Expected ClientLimitExceeded, got {:?}", other),
        }
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;

        let config = RateLimiterConfig::new(1000.0, 100).with_per_client(true);
        let limiter = Arc::new(RateLimiter::new(config));

        let mut handles = Vec::new();
        for i in 0..8 {
            let limiter = Arc::clone(&limiter);
            let handle = thread::spawn(move || {
                let client_id = format!("thread-client-{}", i);
                let mut allowed = 0u32;
                for _ in 0..50 {
                    if limiter.check_rate_limit(&client_id).is_ok() {
                        allowed += 1;
                    }
                }
                allowed
            });
            handles.push(handle);
        }

        let mut total_allowed = 0u32;
        for handle in handles {
            let count = handle.join().expect("Thread panicked");
            total_allowed += count;
        }

        // Each of 8 threads has burst_size=100 tokens and makes 50 requests
        // All 50 from each thread should succeed (50 < 100)
        assert_eq!(
            total_allowed, 400,
            "All requests should be allowed (50 per thread * 8 threads)"
        );
    }

    #[test]
    fn test_concurrent_same_client() {
        use std::sync::Arc;

        // Use a very low refill rate so tokens don't replenish during the test
        let config = RateLimiterConfig::new(0.001, 50).with_per_client(true);
        let limiter = Arc::new(RateLimiter::new(config));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let limiter = Arc::clone(&limiter);
            let handle = thread::spawn(move || {
                let mut allowed = 0u32;
                for _ in 0..20 {
                    if limiter.check_rate_limit("shared-client").is_ok() {
                        allowed += 1;
                    }
                }
                allowed
            });
            handles.push(handle);
        }

        let mut total_allowed = 0u32;
        for handle in handles {
            let count = handle.join().expect("Thread panicked");
            total_allowed += count;
        }

        // 4 threads * 20 requests = 80 total attempts, but burst_size=50
        // With near-zero refill rate, total allowed should be exactly 50
        assert_eq!(
            total_allowed, 50,
            "Total allowed should equal burst_size for shared client"
        );
    }

    #[test]
    fn test_try_acquire_convenience() {
        let config = RateLimiterConfig::new(100.0, 2).with_per_client(true);
        let limiter = RateLimiter::new(config);

        assert!(limiter.try_acquire("client-x"));
        assert!(limiter.try_acquire("client-x"));
        assert!(!limiter.try_acquire("client-x"));
    }

    #[test]
    fn test_remaining_tokens() {
        let config = RateLimiterConfig::new(100.0, 5).with_per_client(true);
        let limiter = RateLimiter::new(config);

        // Before any requests, should return full burst capacity
        assert_eq!(limiter.remaining_tokens("new-client"), 5);

        // After one request
        assert!(limiter.check_rate_limit("new-client").is_ok());
        assert_eq!(limiter.remaining_tokens("new-client"), 4);
    }

    #[test]
    fn test_reset_client() {
        let config = RateLimiterConfig::new(100.0, 3).with_per_client(true);
        let limiter = RateLimiter::new(config);

        // Exhaust
        for _ in 0..3 {
            assert!(limiter.check_rate_limit("reset-client").is_ok());
        }
        assert!(limiter.check_rate_limit("reset-client").is_err());

        // Reset
        limiter.reset("reset-client");
        assert!(
            limiter.check_rate_limit("reset-client").is_ok(),
            "Should be able to make requests after reset"
        );
    }

    #[test]
    fn test_reset_all() {
        let config = RateLimiterConfig::new(100.0, 2)
            .with_per_client(true)
            .with_global_limit(5);
        let limiter = RateLimiter::new(config);

        assert!(limiter.check_rate_limit("a").is_ok());
        assert!(limiter.check_rate_limit("b").is_ok());
        assert_eq!(limiter.tracked_client_count(), 2);

        limiter.reset_all();
        assert_eq!(limiter.tracked_client_count(), 0);
    }

    #[test]
    fn test_config_default() {
        let config = RateLimiterConfig::default();
        assert!((config.requests_per_second - 100.0).abs() < f64::EPSILON);
        assert_eq!(config.burst_size, 50);
        assert!(config.per_client);
        assert!(config.global_limit.is_none());
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_config_builder_pattern() {
        let config = RateLimiterConfig::new(200.0, 100)
            .with_per_client(false)
            .with_global_limit(500)
            .with_idle_timeout(Duration::from_secs(60));

        assert!((config.requests_per_second - 200.0).abs() < f64::EPSILON);
        assert_eq!(config.burst_size, 100);
        assert!(!config.per_client);
        assert_eq!(config.global_limit, Some(500));
        assert_eq!(config.idle_timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_debug_impl() {
        let config = RateLimiterConfig::new(50.0, 10);
        let limiter = RateLimiter::new(config);
        let debug_str = format!("{:?}", limiter);
        assert!(debug_str.contains("RateLimiter"));
        assert!(debug_str.contains("tracked_clients"));
    }

    #[test]
    fn test_global_and_per_client_combined() {
        // Global limit of 5, per-client burst of 3, near-zero refill
        let config = RateLimiterConfig::new(0.001, 3)
            .with_per_client(true)
            .with_global_limit(5);
        let limiter = RateLimiter::new(config);

        // Client A uses 3 (hits per-client limit, also consumes 3 global tokens)
        assert!(limiter.check_rate_limit("a").is_ok());
        assert!(limiter.check_rate_limit("a").is_ok());
        assert!(limiter.check_rate_limit("a").is_ok());
        // 4th request: per-client exhausted, fails before touching global
        assert!(
            limiter.check_rate_limit("a").is_err(),
            "Client A should hit per-client limit"
        );

        // Client B: per-client has 3 tokens, but global only has 2 remaining (5-3)
        assert!(limiter.check_rate_limit("b").is_ok()); // global: 1 remaining
        assert!(limiter.check_rate_limit("b").is_ok()); // global: 0 remaining

        // Next request: per-client still has 1 token, but global is exhausted
        let result = limiter.check_rate_limit("b");
        assert!(result.is_err(), "Should hit global limit");
        assert!(
            matches!(result, Err(RateLimitError::GlobalLimitExceeded { .. })),
            "Error should be GlobalLimitExceeded"
        );
    }

    #[test]
    fn test_zero_refill_rate_retry_after() {
        let bucket = TokenBucket::new(0.0, 0.0);
        assert_eq!(bucket.retry_after_ms(), u64::MAX);
    }
}
