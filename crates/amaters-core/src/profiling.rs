//! CPU and memory profiling infrastructure for AmateRS.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Global counter for tracked allocation events.
pub static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);

/// Global counter for tracked allocated bytes.
pub static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

/// A guard that measures wall-clock time for a named operation.
pub struct ProfilingGuard {
    name: &'static str,
    start: Instant,
}

impl ProfilingGuard {
    /// Start timing an operation named `name`.
    ///
    /// # Example
    ///
    /// ```
    /// use amaters_core::profiling::ProfilingGuard;
    ///
    /// let guard = ProfilingGuard::new("my_operation");
    /// // Do some work...
    /// let summary = guard.finish();
    /// println!("Operation took {}µs", summary.elapsed.as_micros());
    /// ```
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            start: Instant::now(),
        }
    }

    /// Return the elapsed duration since this guard was created.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Consume the guard and return a [`ProfilingSummary`].
    pub fn finish(self) -> ProfilingSummary {
        let elapsed = self.elapsed();
        let alloc_count_delta = ALLOC_COUNT.load(Ordering::Relaxed);
        let alloc_bytes_delta = ALLOC_BYTES.load(Ordering::Relaxed);

        tracing::debug!(
            target: "amaters::profiling",
            name = self.name,
            elapsed_us = elapsed.as_micros() as u64,
            alloc_count = alloc_count_delta,
            alloc_bytes = alloc_bytes_delta,
            "profiling guard finished"
        );

        let name = self.name;
        // Prevent the Drop impl from also logging
        std::mem::forget(self);

        ProfilingSummary {
            name,
            elapsed,
            alloc_count_delta,
            alloc_bytes_delta,
        }
    }
}

impl Drop for ProfilingGuard {
    fn drop(&mut self) {
        tracing::debug!(
            target: "amaters::profiling",
            name = self.name,
            elapsed_us = self.elapsed().as_micros() as u64,
            "profiling guard dropped"
        );
    }
}

/// Summary of a profiled operation.
#[derive(Debug, Clone)]
pub struct ProfilingSummary {
    /// Name passed to [`ProfilingGuard::new`].
    pub name: &'static str,
    /// Wall-clock time measured by the guard.
    pub elapsed: Duration,
    /// Global `ALLOC_COUNT` value at `finish()` time.
    pub alloc_count_delta: u64,
    /// Global `ALLOC_BYTES` value at `finish()` time.
    pub alloc_bytes_delta: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_profiling_guard_measures_elapsed() {
        let guard = ProfilingGuard::new("test_op");
        thread::sleep(Duration::from_millis(5));
        let elapsed = guard.elapsed();
        assert!(elapsed >= Duration::from_millis(1), "elapsed: {elapsed:?}");
        drop(guard);
    }

    #[test]
    fn test_profiling_summary_has_valid_duration() {
        let guard = ProfilingGuard::new("summary_op");
        thread::sleep(Duration::from_millis(2));
        let summary = guard.finish();
        assert_eq!(summary.name, "summary_op");
        assert!(
            summary.elapsed >= Duration::from_millis(1),
            "elapsed: {:?}",
            summary.elapsed
        );
    }

    #[test]
    fn test_profiling_guard_drop_does_not_panic() {
        let guard = ProfilingGuard::new("drop_test");
        drop(guard);
    }
}
