//! Retry logic for transient failures in the AmateRS server.
//!
//! This module provides:
//! - [`RetryPolicy`] — configurable exponential backoff with jitter.
//! - [`ErrorClassification`] — trait for classifying errors as transient or permanent.
//! - [`retry_with_backoff`] — generic async retry driver.
//!
//! **Important:** Only use [`retry_with_backoff`] for *idempotent* operations.
//! Non-idempotent writes MUST NOT be wrapped in retry logic without sequence
//! numbers or other deduplication mechanisms at the caller level.

use std::time::Duration;

// ---------------------------------------------------------------------------
// Jitter PRNG
// ---------------------------------------------------------------------------

/// Minimal xorshift64 PRNG seeded from the current system time.
///
/// Used to produce approximate uniform jitter without pulling in an external
/// PRNG crate.  The output is sufficient for backoff jitter purposes; it is
/// NOT cryptographically secure.
struct Xorshift64(u64);

impl Xorshift64 {
    /// Seed from the current wall clock (nanoseconds since UNIX epoch).
    /// Falls back to a non-zero constant on platforms where the clock is
    /// unavailable.
    fn seeded() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xDEAD_BEEF_CAFE_BABEu64);
        // xorshift requires a non-zero state.
        Self(if seed == 0 {
            0xDEAD_BEEF_CAFE_BABEu64
        } else {
            seed
        })
    }

    /// Produce the next pseudo-random u64.
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Produce a value in `[0.0, 1.0)`.
    fn next_f64(&mut self) -> f64 {
        // Use the top 53 bits for a clean f64 mantissa.
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Retry policy for transient failures.
///
/// ## Safety note
/// IMPORTANT: Only use for idempotent operations — non-idempotent writes must
/// not be wrapped in [`retry_with_backoff`] without explicit caller opt-in and
/// deduplication.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total number of attempts including the first (1 = no retry).
    pub max_attempts: u32,
    /// Base delay in milliseconds for the first retry.
    pub base_delay_ms: u64,
    /// Maximum delay cap in milliseconds.
    pub max_delay_ms: u64,
    /// Jitter factor applied to each computed delay.
    ///
    /// `0.0` = no jitter; `0.1` = ±10% uniform jitter.
    /// Valid range: `[0.0, 1.0)`.
    pub jitter_factor: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 100,
            max_delay_ms: 5_000,
            jitter_factor: 0.1,
        }
    }
}

impl RetryPolicy {
    /// Compute the sleep duration for retry attempt `n` (0-indexed; n=0 is the
    /// first retry, i.e. after the first failed attempt).
    ///
    /// Formula: `min(max_delay_ms, base_delay_ms * 2^n) * uniform(1 - jitter, 1 + jitter)`
    fn delay_for_attempt(&self, n: u32, rng: &mut Xorshift64) -> Duration {
        // Saturating exponentiation to avoid u64 overflow.
        // 2^n using checked_shl to guard against n >= 64.
        let multiplier: u64 = 1u64.checked_shl(n).unwrap_or(u64::MAX);
        let base: u64 = self.base_delay_ms.saturating_mul(multiplier);
        let capped = base.min(self.max_delay_ms);

        let factor = if self.jitter_factor <= 0.0 {
            1.0_f64
        } else {
            let j = self.jitter_factor.min(1.0);
            // uniform in [1 - j, 1 + j]
            let r = rng.next_f64(); // [0, 1)
            1.0 - j + 2.0 * j * r
        };

        let ms = (capped as f64 * factor).max(0.0) as u64;
        Duration::from_millis(ms)
    }
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/// Trait for classifying errors as transient (retriable) or permanent.
///
/// Transient errors are those where a retry might succeed (e.g. a momentary
/// I/O interruption).  Permanent errors (e.g. `NotFound`, auth failure) should
/// return `false` so they are surfaced immediately.
pub trait ErrorClassification {
    /// Returns `true` if this error is transient and the operation should be
    /// retried (subject to the [`RetryPolicy`] limits).
    fn is_transient(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Core retry driver
// ---------------------------------------------------------------------------

/// Retry `op` with exponential backoff and jitter according to `policy`.
///
/// - If `op` returns `Ok(v)`, returns immediately.
/// - If `op` returns `Err(e)` and `e.is_transient()` is `true`, sleeps for
///   the computed delay and tries again (up to `policy.max_attempts` times total).
/// - If `op` returns `Err(e)` and `e.is_transient()` is `false`, returns
///   the error immediately (no further attempts).
/// - After exhausting all attempts, returns the last error.
///
/// # Example
/// ```rust,no_run
/// # use amaters_server::retry::{RetryPolicy, ErrorClassification, retry_with_backoff};
/// # #[derive(Debug)] struct MyErr { transient: bool }
/// # impl std::fmt::Display for MyErr { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "err") } }
/// # impl ErrorClassification for MyErr { fn is_transient(&self) -> bool { self.transient } }
/// # async fn demo() {
/// let policy = RetryPolicy::default();
/// let result = retry_with_backoff(|| async { Ok::<_, MyErr>(42) }, &policy).await;
/// # }
/// ```
pub async fn retry_with_backoff<F, T, E, Fut>(mut op: F, policy: &RetryPolicy) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: ErrorClassification + std::fmt::Debug,
{
    let mut rng = Xorshift64::seeded();
    let max = policy.max_attempts.max(1);

    for attempt in 0..max {
        match op().await {
            Ok(val) => return Ok(val),
            Err(err) => {
                let is_last = attempt + 1 >= max;
                if is_last || !err.is_transient() {
                    return Err(err);
                }
                // Compute retry delay: n = attempt (0-indexed first retry).
                let delay = policy.delay_for_attempt(attempt, &mut rng);
                tokio::time::sleep(delay).await;
            }
        }
    }

    // Unreachable: the loop always returns on the last attempt, but the
    // compiler cannot see that without an explicit unreachable.  Calling op
    // one final time satisfies the type-checker without introducing a panic.
    op().await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ---- Minimal test error types -----------------------------------------

    #[derive(Debug, Clone, PartialEq)]
    enum TestError {
        Transient,
        Permanent,
    }

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                TestError::Transient => write!(f, "transient error"),
                TestError::Permanent => write!(f, "permanent error"),
            }
        }
    }

    impl ErrorClassification for TestError {
        fn is_transient(&self) -> bool {
            matches!(self, TestError::Transient)
        }
    }

    // ---- Tests ------------------------------------------------------------

    /// op fails with a transient error on attempts 1 and 2, succeeds on attempt 3.
    #[tokio::test]
    async fn test_retry_succeeds_on_third_attempt() {
        let call_count = Arc::new(Mutex::new(0u32));
        let counter = Arc::clone(&call_count);

        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 1,
            max_delay_ms: 5,
            jitter_factor: 0.0,
        };

        let result = retry_with_backoff(
            || {
                let counter = Arc::clone(&counter);
                async move {
                    let mut guard = counter.lock().expect("lock poisoned");
                    *guard += 1;
                    let n = *guard;
                    drop(guard);
                    if n < 3 {
                        Err(TestError::Transient)
                    } else {
                        Ok(n)
                    }
                }
            },
            &policy,
        )
        .await;

        assert!(result.is_ok(), "expected success on third attempt");
        assert_eq!(result.expect("ok"), 3);
        assert_eq!(*call_count.lock().expect("lock"), 3);
    }

    /// A permanent error must not be retried — total calls should be 1.
    #[tokio::test]
    async fn test_retry_permanent_error_not_retried() {
        let call_count = Arc::new(Mutex::new(0u32));
        let counter = Arc::clone(&call_count);

        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 1,
            max_delay_ms: 10,
            jitter_factor: 0.0,
        };

        let result: Result<u32, TestError> = retry_with_backoff(
            || {
                let counter = Arc::clone(&counter);
                async move {
                    let mut guard = counter.lock().expect("lock poisoned");
                    *guard += 1;
                    Err(TestError::Permanent)
                }
            },
            &policy,
        )
        .await;

        assert_eq!(result, Err(TestError::Permanent));
        assert_eq!(
            *call_count.lock().expect("lock"),
            1,
            "permanent error must not be retried"
        );
    }

    /// When every attempt returns a transient error, total calls must equal
    /// `policy.max_attempts`.
    #[tokio::test]
    async fn test_retry_respects_max_attempts() {
        let call_count = Arc::new(Mutex::new(0u32));
        let counter = Arc::clone(&call_count);

        let policy = RetryPolicy {
            max_attempts: 4,
            base_delay_ms: 1,
            max_delay_ms: 5,
            jitter_factor: 0.0,
        };

        let result: Result<u32, TestError> = retry_with_backoff(
            || {
                let counter = Arc::clone(&counter);
                async move {
                    let mut guard = counter.lock().expect("lock poisoned");
                    *guard += 1;
                    Err(TestError::Transient)
                }
            },
            &policy,
        )
        .await;

        assert_eq!(result, Err(TestError::Transient));
        assert_eq!(
            *call_count.lock().expect("lock"),
            policy.max_attempts,
            "total calls must equal max_attempts"
        );
    }

    /// With `base_delay_ms = 50` and no jitter, two inter-attempt delays
    /// are 50 ms and 100 ms, totalling ≥ 150 ms.
    #[tokio::test]
    async fn test_retry_backoff_increases_exponentially() {
        let call_count = Arc::new(Mutex::new(0u32));
        let counter = Arc::clone(&call_count);

        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 50,
            max_delay_ms: 5_000,
            jitter_factor: 0.0, // no jitter so we can assert exact lower bound
        };

        let start = std::time::Instant::now();

        let result: Result<u32, TestError> = retry_with_backoff(
            || {
                let counter = Arc::clone(&counter);
                async move {
                    let mut guard = counter.lock().expect("lock poisoned");
                    *guard += 1;
                    Err(TestError::Transient)
                }
            },
            &policy,
        )
        .await;

        let elapsed = start.elapsed();

        assert!(result.is_err());
        // Two sleeps: 50 ms + 100 ms = 150 ms minimum.
        assert!(
            elapsed >= Duration::from_millis(148), // 2 ms tolerance for timer precision
            "expected elapsed >= 150 ms, got {:?}",
            elapsed
        );
        assert_eq!(*call_count.lock().expect("lock"), 3);
    }

    // ---- Xorshift64 smoke-tests -------------------------------------------

    #[test]
    fn test_xorshift64_non_zero() {
        let mut rng = Xorshift64::seeded();
        // Ten consecutive values should all be non-zero (the seed is non-zero
        // and xorshift preserves that).
        for _ in 0..10 {
            assert_ne!(rng.next(), 0);
        }
    }

    #[test]
    fn test_xorshift64_f64_in_range() {
        let mut rng = Xorshift64::seeded();
        for _ in 0..1000 {
            let v = rng.next_f64();
            assert!((0.0..1.0).contains(&v), "out of range: {v}");
        }
    }

    // ---- RetryPolicy delay computation ------------------------------------

    #[test]
    fn test_delay_for_attempt_no_jitter() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 100,
            max_delay_ms: 1_000,
            jitter_factor: 0.0,
        };
        let mut rng = Xorshift64::seeded();
        assert_eq!(
            policy.delay_for_attempt(0, &mut rng),
            Duration::from_millis(100)
        );
        assert_eq!(
            policy.delay_for_attempt(1, &mut rng),
            Duration::from_millis(200)
        );
        assert_eq!(
            policy.delay_for_attempt(2, &mut rng),
            Duration::from_millis(400)
        );
        assert_eq!(
            policy.delay_for_attempt(3, &mut rng),
            Duration::from_millis(800)
        );
        // Capped at max_delay_ms.
        assert_eq!(
            policy.delay_for_attempt(4, &mut rng),
            Duration::from_millis(1_000)
        );
    }
}
