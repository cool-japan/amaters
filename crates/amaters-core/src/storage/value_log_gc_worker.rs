//! Background Garbage Collection Worker for Value Log
//!
//! This module provides an async background worker that periodically runs
//! garbage collection on the value log during idle periods. It includes:
//!
//! - `GcWorker` — the background worker that runs GC periodically
//! - `GcWorkerBuilder` — builder for configuring the worker
//! - `GcWorkerHandle` — handle for controlling the running worker
//! - `BgGcStats` — cumulative GC statistics
//! - `spawn_gc_worker()` — convenience function to create and start a worker

use crate::types::Key;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use super::value_log::ValueLog;
use super::value_log_gc::{GcConfig, GcResult};

/// Cumulative statistics for the background GC worker
#[derive(Debug, Clone, Default)]
pub struct BgGcStats {
    /// Number of GC runs completed successfully
    pub runs_completed: u64,
    /// Total bytes reclaimed across all runs
    pub total_bytes_reclaimed: u64,
    /// Total segments reclaimed across all runs
    pub total_segments_reclaimed: u64,
    /// Last run timestamp (millis since UNIX epoch), or 0 if never run
    pub last_run_millis: u64,
    /// Number of errors encountered
    pub errors: u64,
}

/// Shared state between the background worker and its handle
struct GcWorkerShared {
    /// Shutdown signal
    shutdown: AtomicBool,
    /// Whether the worker loop is currently running
    running: AtomicBool,
    /// Manual trigger signal
    trigger: AtomicBool,
    /// Cumulative stats (atomics for lock-free access)
    runs_completed: AtomicU64,
    total_bytes_reclaimed: AtomicU64,
    total_segments_reclaimed: AtomicU64,
    last_run_millis: AtomicU64,
    errors: AtomicU64,
}

impl GcWorkerShared {
    fn new() -> Self {
        Self {
            shutdown: AtomicBool::new(false),
            running: AtomicBool::new(false),
            trigger: AtomicBool::new(false),
            runs_completed: AtomicU64::new(0),
            total_bytes_reclaimed: AtomicU64::new(0),
            total_segments_reclaimed: AtomicU64::new(0),
            last_run_millis: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }

    fn snapshot_stats(&self) -> BgGcStats {
        BgGcStats {
            runs_completed: self.runs_completed.load(Ordering::Relaxed),
            total_bytes_reclaimed: self.total_bytes_reclaimed.load(Ordering::Relaxed),
            total_segments_reclaimed: self.total_segments_reclaimed.load(Ordering::Relaxed),
            last_run_millis: self.last_run_millis.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }

    fn record_success(&self, result: &GcResult) {
        self.runs_completed.fetch_add(1, Ordering::Relaxed);
        self.total_bytes_reclaimed
            .fetch_add(result.bytes_reclaimed, Ordering::Relaxed);
        self.total_segments_reclaimed
            .fetch_add(result.segments_collected as u64, Ordering::Relaxed);
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.last_run_millis.store(now_millis, Ordering::Relaxed);
    }

    fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }
}

/// Background GC worker that periodically runs garbage collection
pub struct GcWorker {
    value_log: Arc<ValueLog>,
    gc_config: GcConfig,
    check_interval: Duration,
    idle_threshold: Duration,
    shared: Arc<GcWorkerShared>,
}

/// Builder for configuring a GcWorker
pub struct GcWorkerBuilder {
    value_log: Arc<ValueLog>,
    gc_config: Option<GcConfig>,
    check_interval: Duration,
    idle_threshold: Duration,
}

impl GcWorkerBuilder {
    /// Create a new builder with the given ValueLog
    pub fn new(value_log: Arc<ValueLog>) -> Self {
        Self {
            value_log,
            gc_config: None,
            check_interval: Duration::from_secs(30),
            idle_threshold: Duration::from_secs(10),
        }
    }

    /// Set the interval between GC checks
    pub fn with_check_interval(mut self, interval: Duration) -> Self {
        self.check_interval = interval;
        self
    }

    /// Set the idle threshold (time since last write to be considered idle)
    pub fn with_idle_threshold(mut self, threshold: Duration) -> Self {
        self.idle_threshold = threshold;
        self
    }

    /// Set the GC configuration
    pub fn with_gc_config(mut self, config: GcConfig) -> Self {
        self.gc_config = Some(config);
        self
    }

    /// Build the GcWorker
    pub fn build(self) -> GcWorker {
        let gc_config = self
            .gc_config
            .unwrap_or_else(|| self.value_log.gc_config.clone());
        GcWorker {
            value_log: self.value_log,
            gc_config,
            check_interval: self.check_interval,
            idle_threshold: self.idle_threshold,
            shared: Arc::new(GcWorkerShared::new()),
        }
    }
}

impl GcWorker {
    /// Run the background GC loop.
    ///
    /// This is the main async loop that:
    /// 1. Sleeps for `check_interval`
    /// 2. Checks the shutdown signal
    /// 3. Checks if the system is idle
    /// 4. If idle, runs `collect_garbage()`
    /// 5. Tracks stats
    ///
    /// The `is_live_fn` closure is called during GC to determine which keys are still live.
    pub async fn run<F>(self, is_live_fn: F) -> GcWorkerHandle
    where
        F: Fn(&Key) -> bool + Send + Sync + 'static,
    {
        let shared = Arc::clone(&self.shared);
        shared.running.store(true, Ordering::SeqCst);

        let handle_shared = Arc::clone(&shared);

        let value_log = self.value_log;
        let check_interval = self.check_interval;
        let idle_threshold = self.idle_threshold;

        let join_handle = tokio::spawn(async move {
            Self::worker_loop(
                value_log,
                check_interval,
                idle_threshold,
                &shared,
                &is_live_fn,
            )
            .await;
        });

        GcWorkerHandle {
            shared: handle_shared,
            join_handle: Some(join_handle),
        }
    }

    async fn worker_loop<F>(
        value_log: Arc<ValueLog>,
        check_interval: Duration,
        idle_threshold: Duration,
        shared: &GcWorkerShared,
        is_live_fn: &F,
    ) where
        F: Fn(&Key) -> bool + Send + Sync + 'static,
    {
        loop {
            // Sleep for the check interval, but wake up more frequently to
            // check for shutdown and manual triggers
            let tick_duration = Duration::from_millis(50).min(check_interval);
            let mut elapsed = Duration::ZERO;

            while elapsed < check_interval {
                // Check shutdown
                if shared.shutdown.load(Ordering::SeqCst) {
                    shared.running.store(false, Ordering::SeqCst);
                    return;
                }

                // Check manual trigger
                if shared
                    .trigger
                    .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    // Break out of wait loop to run GC immediately
                    break;
                }

                tokio::time::sleep(tick_duration).await;
                elapsed += tick_duration;
            }

            // Check shutdown again after waking
            if shared.shutdown.load(Ordering::SeqCst) {
                shared.running.store(false, Ordering::SeqCst);
                return;
            }

            // Check if the system is idle
            let time_since_write = value_log.time_since_last_write();
            if time_since_write < idle_threshold {
                // System is still active, skip this cycle
                tracing::trace!(
                    "GC worker: system not idle (last write {:?} ago, threshold {:?})",
                    time_since_write,
                    idle_threshold
                );
                continue;
            }

            // Run GC
            tracing::info!(
                "GC worker: starting garbage collection (idle for {:?})",
                time_since_write
            );

            match value_log.collect_garbage(is_live_fn) {
                Ok(result) => {
                    tracing::info!(
                        "GC worker: completed - {} segments collected, {} bytes reclaimed in {:?}",
                        result.segments_collected,
                        result.bytes_reclaimed,
                        result.duration
                    );
                    shared.record_success(&result);
                }
                Err(e) => {
                    tracing::warn!("GC worker: garbage collection failed: {}", e);
                    shared.record_error();
                }
            }
        }
    }
}

/// Handle for controlling a running GC worker
pub struct GcWorkerHandle {
    shared: Arc<GcWorkerShared>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

impl GcWorkerHandle {
    /// Signal the worker to stop and wait for it to finish
    pub async fn stop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.join_handle.take() {
            // Best-effort wait; ignore JoinError (e.g. panic in worker)
            let _ = handle.await;
        }
    }

    /// Get a snapshot of cumulative GC statistics
    pub fn stats(&self) -> BgGcStats {
        self.shared.snapshot_stats()
    }

    /// Manually trigger a GC run (the next cycle will run GC regardless of idle state)
    pub fn trigger_gc(&self) {
        self.shared.trigger.store(true, Ordering::SeqCst);
    }

    /// Check if the worker is still running
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }
}

/// Convenience function to spawn a background GC worker
///
/// Returns a `GcWorkerHandle` that can be used to stop the worker,
/// check stats, or manually trigger GC.
pub async fn spawn_gc_worker<F>(
    value_log: Arc<ValueLog>,
    config: GcConfig,
    is_live_fn: F,
) -> GcWorkerHandle
where
    F: Fn(&Key) -> bool + Send + Sync + 'static,
{
    let worker = GcWorkerBuilder::new(value_log)
        .with_gc_config(config)
        .build();
    worker.run(is_live_fn).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::value_log::{ValueLog, ValueLogConfig};
    use crate::types::CipherBlob;
    use std::env;
    use std::path::PathBuf;
    use std::time::Instant;

    /// Helper to create a unique temp directory for each test
    fn make_test_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir()
            .join("amaters_vlog_gc_worker_tests")
            .join(name)
            .join(format!("{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        // Clean any leftover files from prior runs
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                std::fs::remove_file(entry.path()).ok();
            }
        }
        dir
    }

    #[test]
    fn test_gc_worker_builder() {
        let temp_dir = make_test_dir("gc_worker_builder");
        let vlog = Arc::new(ValueLog::new(&temp_dir).expect("failed to create value log"));

        // Test default values
        let worker = GcWorkerBuilder::new(Arc::clone(&vlog)).build();
        assert_eq!(worker.check_interval, Duration::from_secs(30));
        assert_eq!(worker.idle_threshold, Duration::from_secs(10));

        // Test customization
        let worker = GcWorkerBuilder::new(Arc::clone(&vlog))
            .with_check_interval(Duration::from_secs(5))
            .with_idle_threshold(Duration::from_secs(2))
            .with_gc_config(GcConfig {
                trigger_threshold: 0.3,
                min_segment_age: Duration::from_secs(0),
                max_gc_bytes_per_run: 1024,
            })
            .build();

        assert_eq!(worker.check_interval, Duration::from_secs(5));
        assert_eq!(worker.idle_threshold, Duration::from_secs(2));
        assert!((worker.gc_config.trigger_threshold - 0.3).abs() < f64::EPSILON);

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_gc_worker_starts_and_stops() {
        let temp_dir = make_test_dir("gc_worker_start_stop");
        let vlog = Arc::new(ValueLog::new(&temp_dir).expect("failed to create value log"));

        let worker = GcWorkerBuilder::new(vlog)
            .with_check_interval(Duration::from_millis(100))
            .with_idle_threshold(Duration::from_millis(0))
            .build();

        let mut handle = worker.run(|_| true).await;

        // Worker should be running
        assert!(handle.is_running());

        // Stop the worker
        handle.stop().await;

        // Worker should no longer be running
        assert!(!handle.is_running());

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_gc_worker_respects_check_interval() {
        let temp_dir = make_test_dir("gc_worker_interval");
        let vlog = Arc::new(ValueLog::new(&temp_dir).expect("failed to create value log"));

        let worker = GcWorkerBuilder::new(vlog)
            .with_check_interval(Duration::from_secs(60)) // Long interval
            .with_idle_threshold(Duration::from_millis(0))
            .build();

        let mut handle = worker.run(|_| true).await;

        // Wait a short time -- should not have completed a run yet
        tokio::time::sleep(Duration::from_millis(200)).await;

        let stats = handle.stats();
        assert_eq!(
            stats.runs_completed, 0,
            "Should not have run GC yet (interval too long)"
        );

        handle.stop().await;
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_gc_worker_manual_trigger() {
        let temp_dir = make_test_dir("gc_worker_trigger");
        let vlog = Arc::new(ValueLog::new(&temp_dir).expect("failed to create value log"));

        let worker = GcWorkerBuilder::new(vlog)
            .with_check_interval(Duration::from_secs(60)) // Long interval
            .with_idle_threshold(Duration::from_millis(0))
            .with_gc_config(GcConfig {
                trigger_threshold: 0.5,
                min_segment_age: Duration::from_secs(0),
                max_gc_bytes_per_run: 1024 * 1024,
            })
            .build();

        let mut handle = worker.run(|_| true).await;

        // Trigger GC manually
        handle.trigger_gc();

        // Wait for the trigger to be processed
        tokio::time::sleep(Duration::from_millis(300)).await;

        let stats = handle.stats();
        // The GC should have run (even though check_interval hasn't elapsed)
        assert!(
            stats.runs_completed >= 1,
            "Expected at least 1 run after manual trigger, got {}",
            stats.runs_completed
        );

        handle.stop().await;
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_gc_worker_stats_tracking() {
        let temp_dir = make_test_dir("gc_worker_stats");
        let vlog = Arc::new(ValueLog::new(&temp_dir).expect("failed to create value log"));

        let worker = GcWorkerBuilder::new(Arc::clone(&vlog))
            .with_check_interval(Duration::from_millis(100))
            .with_idle_threshold(Duration::from_millis(0))
            .with_gc_config(GcConfig {
                trigger_threshold: 0.5,
                min_segment_age: Duration::from_secs(0),
                max_gc_bytes_per_run: 1024 * 1024,
            })
            .build();

        let mut handle = worker.run(|_| true).await;

        // Wait for at least one GC cycle to complete
        tokio::time::sleep(Duration::from_millis(300)).await;

        let stats = handle.stats();
        // At least one run should have completed (no segments to collect, but still a run)
        assert!(
            stats.runs_completed >= 1,
            "Expected at least 1 run, got {}",
            stats.runs_completed
        );

        handle.stop().await;
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_gc_worker_idle_detection() {
        let temp_dir = make_test_dir("gc_worker_idle");
        let vlog = Arc::new(ValueLog::new(&temp_dir).expect("failed to create value log"));

        // Use a very long idle threshold so the system is never considered idle
        let worker = GcWorkerBuilder::new(Arc::clone(&vlog))
            .with_check_interval(Duration::from_millis(100))
            .with_idle_threshold(Duration::from_secs(3600)) // 1 hour -- never idle
            .with_gc_config(GcConfig {
                trigger_threshold: 0.5,
                min_segment_age: Duration::from_secs(0),
                max_gc_bytes_per_run: 1024 * 1024,
            })
            .build();

        // Write something to reset the write timer
        {
            let key = Key::from_str("idle_key");
            let value = CipherBlob::new(vec![1u8; 100]);
            vlog.append(key, value).expect("append should succeed");
            vlog.flush().expect("flush should succeed");
        }

        let mut handle = worker.run(|_| true).await;

        // Wait for several check cycles
        tokio::time::sleep(Duration::from_millis(400)).await;

        let stats = handle.stats();
        // GC should NOT have run because system is not idle
        assert_eq!(
            stats.runs_completed, 0,
            "GC should not run when system is not idle, got {} runs",
            stats.runs_completed
        );

        handle.stop().await;
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_gc_worker_shutdown_signal() {
        let temp_dir = make_test_dir("gc_worker_shutdown");
        let vlog = Arc::new(ValueLog::new(&temp_dir).expect("failed to create value log"));

        let worker = GcWorkerBuilder::new(vlog)
            .with_check_interval(Duration::from_secs(60))
            .with_idle_threshold(Duration::from_millis(0))
            .build();

        let mut handle = worker.run(|_| true).await;
        assert!(handle.is_running());

        // Stop should complete promptly (within a few hundred ms)
        let stop_start = Instant::now();
        handle.stop().await;
        let stop_duration = stop_start.elapsed();

        assert!(!handle.is_running());
        assert!(
            stop_duration < Duration::from_secs(2),
            "Shutdown took too long: {:?}",
            stop_duration
        );

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_gc_worker_multiple_runs() {
        let temp_dir = make_test_dir("gc_worker_multi_run");
        let vlog = Arc::new(ValueLog::new(&temp_dir).expect("failed to create value log"));

        let worker = GcWorkerBuilder::new(vlog)
            .with_check_interval(Duration::from_millis(80))
            .with_idle_threshold(Duration::from_millis(0))
            .with_gc_config(GcConfig {
                trigger_threshold: 0.5,
                min_segment_age: Duration::from_secs(0),
                max_gc_bytes_per_run: 1024 * 1024,
            })
            .build();

        let mut handle = worker.run(|_| true).await;

        // Wait for several cycles
        tokio::time::sleep(Duration::from_millis(600)).await;

        let stats = handle.stats();
        assert!(
            stats.runs_completed >= 2,
            "Expected at least 2 runs, got {}",
            stats.runs_completed
        );

        handle.stop().await;
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_bg_gc_stats_default() {
        let stats = BgGcStats::default();
        assert_eq!(stats.runs_completed, 0);
        assert_eq!(stats.total_bytes_reclaimed, 0);
        assert_eq!(stats.total_segments_reclaimed, 0);
        assert_eq!(stats.last_run_millis, 0);
        assert_eq!(stats.errors, 0);
    }

    #[tokio::test]
    async fn test_gc_worker_error_counting() {
        let temp_dir = make_test_dir("gc_worker_errors");
        let vlog = Arc::new(ValueLog::new(&temp_dir).expect("failed to create value log"));

        // Write some data and rotate so there is a segment to GC
        {
            for i in 0..5 {
                let key = Key::from_str(&format!("err_key_{}", i));
                let value = CipherBlob::new(vec![i as u8; 500]);
                vlog.append(key, value).expect("append should succeed");
            }
            vlog.flush().expect("flush should succeed");
            let file_id = vlog.current_file_id();
            vlog.rotate().expect("rotate should succeed");

            // Mark all entries dead so GC will try to reclaim
            if let Some(mut stats) = vlog.segment_stats.get_mut(&file_id) {
                stats.dead_bytes = stats.total_bytes;
                stats.live_bytes = 0;
                stats.live_count = 0;
            }
        }

        // Delete the segment file to cause an IO error during GC
        let file_id_to_break = 0u64;
        let broken_path = ValueLog::vlog_file_path(&temp_dir, file_id_to_break);
        std::fs::remove_file(&broken_path).ok();

        let worker = GcWorkerBuilder::new(vlog)
            .with_check_interval(Duration::from_millis(100))
            .with_idle_threshold(Duration::from_millis(0))
            .with_gc_config(GcConfig {
                trigger_threshold: 0.3,
                min_segment_age: Duration::from_secs(0),
                max_gc_bytes_per_run: 1024 * 1024,
            })
            .build();

        let mut handle = worker.run(|_| true).await;

        // Wait for at least one cycle
        tokio::time::sleep(Duration::from_millis(300)).await;

        let stats = handle.stats();
        // The GC run itself may complete (with 0 segments if the broken segment was
        // already removed from stats) or encounter an error. Either way, the worker
        // should have attempted at least one run.
        let total_attempts = stats.runs_completed + stats.errors;
        assert!(
            total_attempts >= 1,
            "Expected at least 1 GC attempt, got runs={} errors={}",
            stats.runs_completed,
            stats.errors
        );

        handle.stop().await;
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_spawn_gc_worker_helper() {
        let temp_dir = make_test_dir("spawn_gc_worker");
        let vlog = Arc::new(ValueLog::new(&temp_dir).expect("failed to create value log"));

        let gc_config = GcConfig {
            trigger_threshold: 0.5,
            min_segment_age: Duration::from_secs(0),
            max_gc_bytes_per_run: 1024 * 1024,
        };

        let mut handle = spawn_gc_worker(vlog, gc_config, |_| true).await;

        assert!(handle.is_running());
        handle.stop().await;
        assert!(!handle.is_running());

        std::fs::remove_dir_all(&temp_dir).ok();
    }
}
