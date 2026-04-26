//! Memory-use limiter with cooperative back-pressure.
//!
//! [`MemoryLimiter`] tracks a global byte count via an [`AtomicUsize`] counter.
//! Callers must obtain an [`AllocationGuard`] via [`MemoryLimiter::try_allocate`];
//! the guard decrements the counter automatically on drop, ensuring the tracked
//! usage stays accurate even across panics.
//!
//! # Design
//!
//! The compare-exchange loop in [`MemoryLimiter::try_allocate`] gives linearisable
//! semantics: either the allocation is fully visible or it is rejected, with no
//! window where the counter exceeds `max_bytes`.
//!
//! # Example
//!
//! ```rust
//! use amaters_core::memory_limiter::MemoryLimiter;
//!
//! let limiter = MemoryLimiter::new(1024);
//! let guard = limiter.try_allocate(512).expect("should fit");
//! assert_eq!(limiter.current_bytes(), 512);
//! drop(guard);
//! assert_eq!(limiter.current_bytes(), 0);
//! ```

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// OomError
// ---------------------------------------------------------------------------

/// Error returned when a [`MemoryLimiter`] rejects an allocation because the
/// requested bytes would exceed `max_bytes`.
#[derive(Debug)]
pub struct OomError {
    /// Configured maximum, in bytes.
    pub max_bytes: usize,
    /// Number of bytes currently tracked.
    pub current_bytes: usize,
    /// Number of bytes that were requested.
    pub requested: usize,
}

impl fmt::Display for OomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OOM: requested {} bytes but only {} / {} available",
            self.requested,
            self.max_bytes.saturating_sub(self.current_bytes),
            self.max_bytes,
        )
    }
}

impl std::error::Error for OomError {}

// ---------------------------------------------------------------------------
// MemoryLimiter
// ---------------------------------------------------------------------------

/// A cooperative memory limiter backed by an atomic byte counter.
///
/// Use [`try_allocate`](MemoryLimiter::try_allocate) to reserve bytes.
/// The returned [`AllocationGuard`] releases the reservation when dropped.
#[derive(Debug)]
pub struct MemoryLimiter {
    max_bytes: usize,
    current: Arc<AtomicUsize>,
}

impl MemoryLimiter {
    /// Create a new limiter with the given `max_bytes` ceiling.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            current: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Return the number of bytes currently reserved.
    pub fn current_bytes(&self) -> usize {
        self.current.load(Ordering::Acquire)
    }

    /// Return the configured maximum, in bytes.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Attempt to reserve `n` bytes.
    ///
    /// Succeeds if `current + n <= max_bytes`; otherwise returns [`OomError`].
    /// The reservation is released automatically when the returned
    /// [`AllocationGuard`] is dropped.
    ///
    /// The implementation uses a compare-exchange loop to ensure that the
    /// counter never transiently exceeds `max_bytes`, even under heavy
    /// concurrent load.
    pub fn try_allocate(&self, n: usize) -> Result<AllocationGuard, OomError> {
        loop {
            let cur = self.current.load(Ordering::Acquire);
            if cur + n > self.max_bytes {
                return Err(OomError {
                    max_bytes: self.max_bytes,
                    current_bytes: cur,
                    requested: n,
                });
            }
            match self
                .current
                .compare_exchange(cur, cur + n, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    return Ok(AllocationGuard {
                        n,
                        current: Arc::clone(&self.current),
                    });
                }
                Err(_) => continue,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AllocationGuard
// ---------------------------------------------------------------------------

/// RAII guard that releases a reservation from a [`MemoryLimiter`] on drop.
pub struct AllocationGuard {
    n: usize,
    current: Arc<AtomicUsize>,
}

impl Drop for AllocationGuard {
    fn drop(&mut self) {
        self.current.fetch_sub(self.n, Ordering::AcqRel);
    }
}

impl fmt::Debug for AllocationGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AllocationGuard")
            .field("reserved_bytes", &self.n)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_memory_limiter_allows_under_limit() {
        let limiter = MemoryLimiter::new(1024);
        let guard = limiter
            .try_allocate(512)
            .expect("should succeed under limit");
        assert_eq!(limiter.current_bytes(), 512);
        drop(guard);
        assert_eq!(limiter.current_bytes(), 0);
    }

    #[test]
    fn test_memory_limiter_rejects_over_limit() {
        let limiter = MemoryLimiter::new(1024);
        let _guard = limiter
            .try_allocate(900)
            .expect("first alloc should succeed");
        let err = limiter
            .try_allocate(200)
            .expect_err("should reject over limit");
        assert_eq!(err.max_bytes, 1024);
        assert_eq!(err.requested, 200);
        assert!(err.current_bytes >= 900);
    }

    #[test]
    fn test_memory_limiter_releases_on_drop() {
        let limiter = MemoryLimiter::new(1024);
        {
            let _guard = limiter.try_allocate(512).expect("should succeed");
            assert_eq!(limiter.current_bytes(), 512);
        }
        // Guard dropped — bytes should be released.
        assert_eq!(limiter.current_bytes(), 0);
        // A fresh allocation should now succeed.
        let _guard2 = limiter
            .try_allocate(1024)
            .expect("full budget available again");
        assert_eq!(limiter.current_bytes(), 1024);
    }

    #[test]
    fn test_memory_limiter_concurrent_allocations() {
        use std::sync::Barrier;
        use std::thread;

        // limiter max = 5 KB
        let max: usize = 5 * 1024;
        let limiter = Arc::new(MemoryLimiter::new(max));
        let successes = Arc::new(AtomicUsize::new(0));

        // All 10 threads rendezvous at the barrier before attempting allocations,
        // ensuring they race simultaneously so that at most 5 can succeed.
        let barrier = Arc::new(Barrier::new(10));

        // A second barrier ensures all allocation attempts are complete before any
        // guard is released, so the counter cannot dip between attempts.
        let barrier2 = Arc::new(Barrier::new(10));

        // Spawn 10 threads; each tries to allocate 1 KB.
        // Only 5 should succeed.
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let limiter = Arc::clone(&limiter);
                let successes = Arc::clone(&successes);
                let b1 = Arc::clone(&barrier);
                let b2 = Arc::clone(&barrier2);
                thread::spawn(move || {
                    // Wait until all threads are ready.
                    b1.wait();
                    let guard = limiter.try_allocate(1024);
                    if guard.is_ok() {
                        successes.fetch_add(1, Ordering::Relaxed);
                    }
                    // Synchronise so all threads have attempted allocation before
                    // any guard is released.
                    b2.wait();
                    // guard (if Some) dropped here, releasing its reservation.
                    drop(guard);
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        // After all threads finish, the counter must be 0 (all guards dropped).
        assert_eq!(limiter.current_bytes(), 0);

        // Exactly 5 threads should have succeeded (5 × 1024 == 5120 max).
        assert_eq!(successes.load(Ordering::Relaxed), 5);
    }
}
