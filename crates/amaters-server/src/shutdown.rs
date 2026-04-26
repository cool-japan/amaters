//! Graceful shutdown handling
//!
//! This module provides signal handling for graceful server shutdown,
//! coordinating the shutdown of all components in the correct order.
//! It supports connection draining, phased shutdown, state persistence
//! via shutdown hooks, and detailed status reporting.
//! It also handles SIGHUP for hot configuration reload on Unix platforms.

use crate::config::ReloadableConfig;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, broadcast, watch};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Shutdown phase
// ---------------------------------------------------------------------------

/// Phases of the shutdown lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShutdownPhase {
    /// Server is running normally and accepting requests
    Running,
    /// Server stopped accepting new connections; waiting for in-flight requests
    Draining,
    /// Executing shutdown hooks (WAL flush, memtable flush, metrics snapshot, etc.)
    FlushingState,
    /// Shutdown complete
    Terminated,
}

impl ShutdownPhase {
    /// Numeric representation for atomic storage
    fn as_u64(self) -> u64 {
        match self {
            Self::Running => 0,
            Self::Draining => 1,
            Self::FlushingState => 2,
            Self::Terminated => 3,
        }
    }

    fn from_u64(val: u64) -> Self {
        match val {
            0 => Self::Running,
            1 => Self::Draining,
            2 => Self::FlushingState,
            3 => Self::Terminated,
            _ => Self::Terminated,
        }
    }
}

impl fmt::Display for ShutdownPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "Running"),
            Self::Draining => write!(f, "Draining"),
            Self::FlushingState => write!(f, "FlushingState"),
            Self::Terminated => write!(f, "Terminated"),
        }
    }
}

// ---------------------------------------------------------------------------
// Drain configuration
// ---------------------------------------------------------------------------

/// Configuration for connection draining behaviour
#[derive(Debug, Clone)]
pub struct DrainConfig {
    /// Maximum time to wait for in-flight requests to complete
    pub drain_timeout: Duration,
    /// Interval at which we log draining progress
    pub check_interval: Duration,
    /// Timeout for the flushing-state phase (hook execution)
    pub flush_timeout: Duration,
}

impl Default for DrainConfig {
    fn default() -> Self {
        Self {
            drain_timeout: Duration::from_secs(30),
            check_interval: Duration::from_secs(1),
            flush_timeout: Duration::from_secs(30),
        }
    }
}

// ---------------------------------------------------------------------------
// Shutdown hook trait
// ---------------------------------------------------------------------------

/// A hook that runs during the `FlushingState` phase of shutdown.
///
/// Implementors should perform any final persistence / cleanup work
/// (e.g. flushing WAL, memtable, metrics) in [`on_shutdown`](ShutdownHook::on_shutdown).
#[async_trait::async_trait]
pub trait ShutdownHook: Send + Sync {
    /// Human-readable name of this hook (used in logging)
    fn name(&self) -> &str;

    /// Execute the hook. Errors are logged but do **not** prevent other hooks
    /// from running.
    async fn on_shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

// ---------------------------------------------------------------------------
// Storage integration traits
// ---------------------------------------------------------------------------

/// Trait for WAL sync operations during shutdown
pub trait WalWriter: Send + Sync {
    /// Sync all pending WAL data to disk
    fn sync(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    /// Return the current size of the WAL in bytes
    fn current_size(&self) -> u64;
}

/// Trait for memtable flush operations during shutdown
pub trait MemtableFlusher: Send + Sync {
    /// Flush the active memtable to an SSTable, returning the number of entries flushed
    fn flush_to_sstable(&self) -> Result<usize, Box<dyn std::error::Error + Send + Sync>>;
}

// ---------------------------------------------------------------------------
// Hook execution result
// ---------------------------------------------------------------------------

/// Result of a single shutdown hook execution
#[derive(Debug, Clone)]
pub struct HookExecutionResult {
    /// Name of the hook that was executed
    pub hook_name: String,
    /// Whether the hook completed successfully
    pub success: bool,
    /// How long the hook took to execute
    pub duration: Duration,
    /// Error message if the hook failed
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Built-in hooks
// ---------------------------------------------------------------------------

/// Flush the Write-Ahead Log to disk
pub struct WalFlushHook {
    /// Timeout for this individual hook
    pub timeout: Duration,
    /// Optional WAL writer for real storage integration
    writer: Option<Arc<dyn WalWriter>>,
}

impl WalFlushHook {
    /// Create a WAL flush hook with a real writer
    pub fn with_writer(writer: Arc<dyn WalWriter>, timeout: Duration) -> Self {
        Self {
            timeout,
            writer: Some(writer),
        }
    }
}

impl Default for WalFlushHook {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            writer: None,
        }
    }
}

#[async_trait::async_trait]
impl ShutdownHook for WalFlushHook {
    fn name(&self) -> &str {
        "WalFlush"
    }

    async fn on_shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match &self.writer {
            Some(writer) => {
                let size = writer.current_size();
                info!("Flushing WAL to disk ({} bytes)", size);
                writer.sync()?;
                info!("WAL flush complete ({} bytes synced)", size);
            }
            None => {
                info!("No WAL writer configured - skipping flush");
            }
        }
        Ok(())
    }
}

/// Flush the active memtable to an SSTable
pub struct MemtableFlushHook {
    /// Timeout for this individual hook
    pub timeout: Duration,
    /// Optional memtable flusher for real storage integration
    flusher: Option<Arc<dyn MemtableFlusher>>,
}

impl MemtableFlushHook {
    /// Create a memtable flush hook with a real flusher
    pub fn with_flusher(flusher: Arc<dyn MemtableFlusher>, timeout: Duration) -> Self {
        Self {
            timeout,
            flusher: Some(flusher),
        }
    }
}

impl Default for MemtableFlushHook {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            flusher: None,
        }
    }
}

#[async_trait::async_trait]
impl ShutdownHook for MemtableFlushHook {
    fn name(&self) -> &str {
        "MemtableFlush"
    }

    async fn on_shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match &self.flusher {
            Some(flusher) => {
                info!("Flushing active memtable to SSTable");
                let entries = flusher.flush_to_sstable()?;
                info!("Memtable flush complete ({} entries flushed)", entries);
            }
            None => {
                info!("No memtable flusher configured - skipping flush");
            }
        }
        Ok(())
    }
}

/// Drain active connections before shutdown
pub struct ConnectionDrainHook {
    /// Shared counter of active connections
    active_connections: Arc<AtomicUsize>,
    /// Maximum time to wait for connections to drain
    drain_timeout: Duration,
    /// Interval between polling the connection counter
    poll_interval: Duration,
}

impl ConnectionDrainHook {
    /// Create a new connection drain hook
    ///
    /// # Arguments
    /// * `active_connections` - Shared atomic counter of active connections
    /// * `drain_timeout` - Maximum time to wait for all connections to close
    pub fn new(active_connections: Arc<AtomicUsize>, drain_timeout: Duration) -> Self {
        Self {
            active_connections,
            drain_timeout,
            poll_interval: Duration::from_millis(100),
        }
    }

    /// Set a custom poll interval (default is 100ms)
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

#[async_trait::async_trait]
impl ShutdownHook for ConnectionDrainHook {
    fn name(&self) -> &str {
        "ConnectionDrain"
    }

    async fn on_shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let deadline = Instant::now() + self.drain_timeout;

        loop {
            let remaining = self.active_connections.load(Ordering::SeqCst);
            if remaining == 0 {
                info!("All connections drained");
                return Ok(());
            }

            if Instant::now() >= deadline {
                warn!(
                    "Connection drain timeout ({:?}) exceeded with {} connections remaining",
                    self.drain_timeout, remaining
                );
                return Err(format!(
                    "connection drain timed out with {} connections remaining",
                    remaining
                )
                .into());
            }

            info!("Draining connections: {} remaining", remaining);
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

/// Save a final metrics snapshot
pub struct MetricsSnapshotHook {
    /// Timeout for this individual hook
    pub timeout: Duration,
    /// Optional path to write metrics data to
    metrics_path: Option<PathBuf>,
    /// Optional provider that produces metrics data as bytes
    metrics_provider: Option<Arc<dyn Fn() -> Vec<u8> + Send + Sync>>,
}

impl MetricsSnapshotHook {
    /// Create a metrics snapshot hook with a provider and output path
    pub fn with_provider(
        provider: Arc<dyn Fn() -> Vec<u8> + Send + Sync>,
        path: PathBuf,
        timeout: Duration,
    ) -> Self {
        Self {
            timeout,
            metrics_path: Some(path),
            metrics_provider: Some(provider),
        }
    }
}

impl Default for MetricsSnapshotHook {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            metrics_path: None,
            metrics_provider: None,
        }
    }
}

#[async_trait::async_trait]
impl ShutdownHook for MetricsSnapshotHook {
    fn name(&self) -> &str {
        "MetricsSnapshot"
    }

    async fn on_shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match (&self.metrics_provider, &self.metrics_path) {
            (Some(provider), Some(path)) => {
                let data = provider();
                info!(
                    "Writing {} bytes of metrics to {}",
                    data.len(),
                    path.display()
                );
                std::fs::write(path, &data)?;
                info!("Metrics snapshot saved successfully");
            }
            _ => {
                info!("No metrics provider/path configured - skipping snapshot");
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Shutdown status
// ---------------------------------------------------------------------------

/// Snapshot of the current shutdown progress
#[derive(Debug, Clone)]
pub struct ShutdownStatus {
    /// Current phase
    pub phase: ShutdownPhase,
    /// Number of in-flight requests still being processed
    pub active_requests: usize,
    /// Number of hooks that have completed (successfully or not)
    pub hooks_completed: usize,
    /// Total registered hooks
    pub hooks_total: usize,
    /// Milliseconds elapsed since shutdown was initiated (0 if not yet initiated)
    pub elapsed_ms: u64,
}

// ---------------------------------------------------------------------------
// Shutdown coordinator
// ---------------------------------------------------------------------------

/// Shutdown coordinator
///
/// Manages graceful shutdown across all server components, including connection
/// draining, phased shutdown, and hook execution.
#[derive(Clone)]
pub struct ShutdownCoordinator {
    inner: Arc<ShutdownInner>,
}

struct ShutdownInner {
    /// Broadcast channel for the initial shutdown signal
    sender: broadcast::Sender<()>,
    /// Watch channel so late subscribers can observe phase changes
    phase_tx: watch::Sender<ShutdownPhase>,
    phase_rx: watch::Receiver<ShutdownPhase>,
    /// Atomic flag indicating shutdown initiated (idempotent)
    shutdown_initiated: AtomicBool,
    /// Current phase stored atomically for lock-free reads
    phase: AtomicU64,
    /// Number of active (in-flight) requests
    active_requests: AtomicUsize,
    /// Registered shutdown hooks (protected by Mutex for append + iteration)
    hooks: Mutex<Vec<Box<dyn ShutdownHook>>>,
    /// Number of hooks completed so far
    hooks_completed: AtomicUsize,
    /// Results from hook execution
    hook_results: Mutex<Vec<HookExecutionResult>>,
    /// Drain configuration
    drain_config: DrainConfig,
    /// Instant when shutdown was initiated (set once, then read-only)
    shutdown_start: Mutex<Option<Instant>>,
}

impl ShutdownCoordinator {
    /// Create a new shutdown coordinator with default drain configuration
    pub fn new() -> Self {
        Self::with_config(DrainConfig::default())
    }

    /// Create a new shutdown coordinator with the given drain configuration
    pub fn with_config(config: DrainConfig) -> Self {
        let (sender, _) = broadcast::channel(16);
        let (phase_tx, phase_rx) = watch::channel(ShutdownPhase::Running);

        Self {
            inner: Arc::new(ShutdownInner {
                sender,
                phase_tx,
                phase_rx,
                shutdown_initiated: AtomicBool::new(false),
                phase: AtomicU64::new(ShutdownPhase::Running.as_u64()),
                active_requests: AtomicUsize::new(0),
                hooks: Mutex::new(Vec::new()),
                hooks_completed: AtomicUsize::new(0),
                hook_results: Mutex::new(Vec::new()),
                drain_config: config,
                shutdown_start: Mutex::new(None),
            }),
        }
    }

    // -- Subscription -------------------------------------------------------

    /// Subscribe to the initial shutdown broadcast signal
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.inner.sender.subscribe()
    }

    /// Subscribe to phase changes via a `watch` channel
    pub fn phase_watch(&self) -> watch::Receiver<ShutdownPhase> {
        self.inner.phase_rx.clone()
    }

    // -- Active request tracking --------------------------------------------

    /// Increment the active request counter (called when a new request arrives)
    pub fn request_start(&self) {
        self.inner.active_requests.fetch_add(1, Ordering::SeqCst);
    }

    /// Decrement the active request counter (called when a request completes)
    pub fn request_end(&self) {
        self.inner.active_requests.fetch_sub(1, Ordering::SeqCst);
    }

    /// Current number of active (in-flight) requests
    pub fn active_request_count(&self) -> usize {
        self.inner.active_requests.load(Ordering::SeqCst)
    }

    // -- Hook registration --------------------------------------------------

    /// Register a shutdown hook that will run during the `FlushingState` phase
    pub async fn register_shutdown_hook(&self, hook: Box<dyn ShutdownHook>) {
        let mut hooks = self.inner.hooks.lock().await;
        info!("Registered shutdown hook: {}", hook.name());
        hooks.push(hook);
    }

    // -- Phase management ---------------------------------------------------

    /// Get the current shutdown phase
    pub fn current_phase(&self) -> ShutdownPhase {
        ShutdownPhase::from_u64(self.inner.phase.load(Ordering::SeqCst))
    }

    /// Whether we are currently accepting new connections
    pub fn is_accepting(&self) -> bool {
        self.current_phase() == ShutdownPhase::Running
    }

    fn set_phase(&self, phase: ShutdownPhase) {
        self.inner.phase.store(phase.as_u64(), Ordering::SeqCst);
        // Ignore send error -- it only fails when there are no receivers
        let _ = self.inner.phase_tx.send(phase);
        info!("Shutdown phase: {}", phase);
    }

    // -- Query --------------------------------------------------------------

    /// Check if shutdown has been initiated
    pub fn is_shutting_down(&self) -> bool {
        self.inner.shutdown_initiated.load(Ordering::SeqCst)
    }

    /// Returns `"shutting_down"` when draining/flushing/terminated, otherwise `"ok"`.
    ///
    /// Load balancers can poll this to detect that the server is shutting down
    /// and stop routing new requests.
    pub fn health_status_label(&self) -> &'static str {
        match self.current_phase() {
            ShutdownPhase::Running => "ok",
            _ => "shutting_down",
        }
    }

    /// Build a [`ShutdownStatus`] snapshot
    pub fn status(&self) -> ShutdownStatus {
        let elapsed_ms = {
            // Try-lock: if the mutex is contended we just report 0
            if let Ok(guard) = self.inner.shutdown_start.try_lock() {
                guard.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0)
            } else {
                0
            }
        };

        let hooks_total = if let Ok(hooks) = self.inner.hooks.try_lock() {
            hooks.len()
        } else {
            0
        };

        ShutdownStatus {
            phase: self.current_phase(),
            active_requests: self.active_request_count(),
            hooks_completed: self.inner.hooks_completed.load(Ordering::SeqCst),
            hooks_total,
            elapsed_ms,
        }
    }

    // -- Initiate shutdown --------------------------------------------------

    /// Initiate a graceful shutdown.
    ///
    /// This is idempotent -- calling it more than once is a no-op.
    /// The method broadcasts the shutdown signal, then (in a spawned task)
    /// drives the phase machine: Draining -> FlushingState -> Terminated.
    pub fn shutdown(&self) {
        if self.inner.shutdown_initiated.swap(true, Ordering::SeqCst) {
            // Already initiated
            debug!("Shutdown already initiated - ignoring duplicate signal");
            return;
        }

        info!("Initiating graceful shutdown");

        // Record start time
        if let Ok(mut guard) = self.inner.shutdown_start.try_lock() {
            *guard = Some(Instant::now());
        }

        // Broadcast to legacy subscribers
        if let Err(e) = self.inner.sender.send(()) {
            warn!("Failed to broadcast shutdown signal: {}", e);
        }

        // Spawn the phase-driver task
        let coord = self.clone();
        tokio::spawn(async move {
            coord.run_shutdown_sequence().await;
        });
    }

    /// Execute the full shutdown sequence: Draining -> FlushingState -> Terminated
    async fn run_shutdown_sequence(&self) {
        // ---- Phase 1: Draining ----
        self.set_phase(ShutdownPhase::Draining);
        self.drain_connections().await;

        // ---- Phase 2: Flushing state (run hooks) ----
        self.set_phase(ShutdownPhase::FlushingState);
        self.run_hooks().await;

        // ---- Phase 3: Terminated ----
        self.set_phase(ShutdownPhase::Terminated);
        info!("Shutdown complete");
    }

    /// Wait for all active requests to drain, up to `drain_timeout`.
    async fn drain_connections(&self) {
        let cfg = &self.inner.drain_config;
        let deadline = Instant::now() + cfg.drain_timeout;

        loop {
            let remaining = self.active_request_count();
            if remaining == 0 {
                info!("All in-flight requests drained");
                return;
            }

            if Instant::now() >= deadline {
                warn!(
                    "Drain timeout ({:?}) exceeded with {} requests remaining - force-closing",
                    cfg.drain_timeout, remaining
                );
                return;
            }

            info!("Draining: {} requests remaining", remaining);
            tokio::time::sleep(cfg.check_interval).await;
        }
    }

    /// Retrieve the results from all executed hooks.
    ///
    /// Returns an empty vector if shutdown has not been initiated or hooks
    /// have not yet finished executing.
    pub async fn hook_results(&self) -> Vec<HookExecutionResult> {
        self.inner.hook_results.lock().await.clone()
    }

    /// Execute all registered shutdown hooks, each with the global flush timeout.
    async fn run_hooks(&self) {
        let hooks = {
            let mut guard = self.inner.hooks.lock().await;
            std::mem::take(&mut *guard)
        };

        if hooks.is_empty() {
            info!("No shutdown hooks registered");
            return;
        }

        let flush_timeout = self.inner.drain_config.flush_timeout;
        info!("Executing {} shutdown hook(s)", hooks.len());

        for hook in &hooks {
            let name = hook.name().to_string();
            info!("Running shutdown hook: {}", name);

            let start = Instant::now();
            let result = match tokio::time::timeout(flush_timeout, hook.on_shutdown()).await {
                Ok(Ok(())) => {
                    info!("Shutdown hook '{}' completed successfully", name);
                    HookExecutionResult {
                        hook_name: name,
                        success: true,
                        duration: start.elapsed(),
                        error: None,
                    }
                }
                Ok(Err(e)) => {
                    let msg = e.to_string();
                    error!("Shutdown hook '{}' failed: {}", name, msg);
                    HookExecutionResult {
                        hook_name: name,
                        success: false,
                        duration: start.elapsed(),
                        error: Some(msg),
                    }
                }
                Err(_) => {
                    let msg = format!("timed out after {:?}", flush_timeout);
                    error!("Shutdown hook '{}' {}", name, msg);
                    HookExecutionResult {
                        hook_name: name,
                        success: false,
                        duration: start.elapsed(),
                        error: Some(msg),
                    }
                }
            };

            {
                let mut results = self.inner.hook_results.lock().await;
                results.push(result);
            }
            self.inner.hooks_completed.fetch_add(1, Ordering::SeqCst);
        }

        info!(
            "All shutdown hooks processed ({} total)",
            self.inner.hooks_completed.load(Ordering::SeqCst)
        );
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Signal handler setup
// ---------------------------------------------------------------------------

/// Setup signal handlers for graceful shutdown
///
/// Listens for SIGTERM and SIGINT signals and triggers shutdown
pub async fn setup_signal_handlers(coordinator: ShutdownCoordinator) {
    tokio::spawn(async move {
        if let Err(e) = wait_for_signal().await {
            warn!("Error setting up signal handlers: {}", e);
            return;
        }

        info!("Received shutdown signal");
        coordinator.shutdown();
    });
}

/// Setup SIGHUP handler for hot configuration reload (Unix only)
///
/// On SIGHUP, reloads configuration from the stored config file path.
/// On non-Unix platforms, this is a no-op; use `ReloadableConfig::manual_reload()` instead.
#[cfg(unix)]
pub async fn setup_sighup_handler(config: ReloadableConfig) {
    tokio::spawn(async move {
        let mut sighup = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to setup SIGHUP handler: {}", e);
                return;
            }
        };

        loop {
            sighup.recv().await;
            info!("Received SIGHUP - reloading configuration");

            match config.reload_from_stored_path() {
                Ok(report) => {
                    if report.success {
                        info!("Configuration reload completed: {}", report);
                    } else {
                        error!("Configuration reload failed: {}", report);
                    }
                }
                Err(e) => {
                    error!("Configuration reload error: {}", e);
                }
            }
        }
    });
}

/// No-op SIGHUP handler for non-Unix platforms.
///
/// Use `ReloadableConfig::manual_reload()` as an alternative.
#[cfg(not(unix))]
pub async fn setup_sighup_handler(_config: ReloadableConfig) {
    info!("SIGHUP handler not available on this platform; use manual_reload() instead");
}

/// Wait for shutdown signal (SIGTERM or SIGINT)
async fn wait_for_signal() -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;

        tokio::select! {
            _ = sigterm.recv() => {
                info!("Received SIGTERM");
            }
            _ = sigint.recv() => {
                info!("Received SIGINT");
            }
        }
    }

    #[cfg(not(unix))]
    {
        use tokio::signal;
        signal::ctrl_c().await?;
        info!("Received Ctrl+C");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Shutdown guard
// ---------------------------------------------------------------------------

/// Shutdown guard for automatic cleanup
///
/// Triggers shutdown when dropped (useful for panic recovery)
pub struct ShutdownGuard {
    coordinator: ShutdownCoordinator,
    disarmed: Arc<AtomicBool>,
}

impl ShutdownGuard {
    /// Create a new shutdown guard
    pub fn new(coordinator: ShutdownCoordinator) -> Self {
        Self {
            coordinator,
            disarmed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Disarm the guard (won't trigger shutdown on drop)
    pub fn disarm(&self) {
        self.disarmed.store(true, Ordering::SeqCst);
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if !self.disarmed.load(Ordering::SeqCst) {
            warn!("ShutdownGuard dropped without disarming - triggering shutdown");
            self.coordinator.shutdown();
        }
    }
}

// ---------------------------------------------------------------------------
// Request guard (RAII active-request tracking)
// ---------------------------------------------------------------------------

/// RAII guard that tracks an active request.
///
/// Calls `request_start()` on creation and `request_end()` on drop,
/// ensuring the active-request counter stays accurate even if the
/// request handler panics.
pub struct RequestGuard {
    coordinator: ShutdownCoordinator,
}

impl RequestGuard {
    /// Create a new request guard, incrementing the active request count
    pub fn new(coordinator: ShutdownCoordinator) -> Self {
        coordinator.request_start();
        Self { coordinator }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.coordinator.request_end();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool as StdAtomicBool;
    use std::time::Duration;
    use tokio::time::timeout;

    /// Helper: wait for a coordinator to reach Terminated phase (with timeout)
    async fn wait_terminated(coordinator: &ShutdownCoordinator, dur: Duration) {
        let mut watcher = coordinator.phase_watch();
        let _ = timeout(dur, async {
            loop {
                if *watcher.borrow() == ShutdownPhase::Terminated {
                    return;
                }
                if watcher.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
    }

    #[tokio::test]
    async fn test_shutdown_coordinator() {
        let coordinator = ShutdownCoordinator::new();
        let mut receiver = coordinator.subscribe();

        assert!(!coordinator.is_shutting_down());
        assert_eq!(coordinator.current_phase(), ShutdownPhase::Running);

        coordinator.shutdown();

        assert!(coordinator.is_shutting_down());

        // Should receive shutdown signal
        let result = timeout(Duration::from_millis(100), receiver.recv()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let coordinator = ShutdownCoordinator::new();
        let mut rx1 = coordinator.subscribe();
        let mut rx2 = coordinator.subscribe();
        let mut rx3 = coordinator.subscribe();

        coordinator.shutdown();

        assert!(
            timeout(Duration::from_millis(100), rx1.recv())
                .await
                .is_ok()
        );
        assert!(
            timeout(Duration::from_millis(100), rx2.recv())
                .await
                .is_ok()
        );
        assert!(
            timeout(Duration::from_millis(100), rx3.recv())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_shutdown_idempotent() {
        let coordinator = ShutdownCoordinator::new();

        coordinator.shutdown();
        coordinator.shutdown(); // Second call should be a no-op

        assert!(coordinator.is_shutting_down());

        // Let the phase driver complete
        wait_terminated(&coordinator, Duration::from_secs(2)).await;
        assert_eq!(coordinator.current_phase(), ShutdownPhase::Terminated);
    }

    #[test]
    fn test_shutdown_guard_disarm() {
        let coordinator = ShutdownCoordinator::new();
        let guard = ShutdownGuard::new(coordinator.clone());

        guard.disarm();
        drop(guard);

        assert!(!coordinator.is_shutting_down());
    }

    #[tokio::test]
    async fn test_shutdown_guard_trigger() {
        let coordinator = ShutdownCoordinator::new();
        let guard = ShutdownGuard::new(coordinator.clone());

        drop(guard);

        assert!(coordinator.is_shutting_down());

        // Let the spawned phase-driver complete
        wait_terminated(&coordinator, Duration::from_secs(2)).await;
    }

    // -- Phase transition tests ---------------------------------------------

    #[tokio::test]
    async fn test_phase_transitions() {
        let config = DrainConfig {
            drain_timeout: Duration::from_millis(200),
            check_interval: Duration::from_millis(50),
            flush_timeout: Duration::from_millis(200),
        };
        let coordinator = ShutdownCoordinator::with_config(config);

        assert_eq!(coordinator.current_phase(), ShutdownPhase::Running);

        coordinator.shutdown();

        wait_terminated(&coordinator, Duration::from_secs(2)).await;
        assert_eq!(coordinator.current_phase(), ShutdownPhase::Terminated);
    }

    #[tokio::test]
    async fn test_drain_waits_for_in_flight_requests() {
        let config = DrainConfig {
            drain_timeout: Duration::from_secs(2),
            check_interval: Duration::from_millis(50),
            flush_timeout: Duration::from_millis(200),
        };
        let coordinator = ShutdownCoordinator::with_config(config);

        // Simulate 3 in-flight requests
        coordinator.request_start();
        coordinator.request_start();
        coordinator.request_start();
        assert_eq!(coordinator.active_request_count(), 3);

        coordinator.shutdown();

        // Give the drainer a moment to start
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(coordinator.current_phase(), ShutdownPhase::Draining);

        // Complete requests one by one
        coordinator.request_end();
        tokio::time::sleep(Duration::from_millis(60)).await;
        coordinator.request_end();
        tokio::time::sleep(Duration::from_millis(60)).await;
        coordinator.request_end();

        wait_terminated(&coordinator, Duration::from_secs(2)).await;
        assert_eq!(coordinator.current_phase(), ShutdownPhase::Terminated);
    }

    #[tokio::test]
    async fn test_drain_timeout_forces_termination() {
        let config = DrainConfig {
            drain_timeout: Duration::from_millis(150),
            check_interval: Duration::from_millis(30),
            flush_timeout: Duration::from_millis(100),
        };
        let coordinator = ShutdownCoordinator::with_config(config);

        // Simulate a request that never finishes
        coordinator.request_start();

        coordinator.shutdown();

        wait_terminated(&coordinator, Duration::from_secs(2)).await;
        assert_eq!(coordinator.current_phase(), ShutdownPhase::Terminated);
        // The stuck request is still counted
        assert_eq!(coordinator.active_request_count(), 1);
    }

    #[tokio::test]
    async fn test_shutdown_hooks_execute_in_order() {
        let config = DrainConfig {
            drain_timeout: Duration::from_millis(100),
            check_interval: Duration::from_millis(20),
            flush_timeout: Duration::from_secs(1),
        };
        let coordinator = ShutdownCoordinator::with_config(config);

        let order = Arc::new(Mutex::new(Vec::<String>::new()));

        struct OrderHook {
            hook_name: String,
            order: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait::async_trait]
        impl ShutdownHook for OrderHook {
            fn name(&self) -> &str {
                &self.hook_name
            }
            async fn on_shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                let mut guard = self.order.lock().await;
                guard.push(self.hook_name.clone());
                Ok(())
            }
        }

        coordinator
            .register_shutdown_hook(Box::new(OrderHook {
                hook_name: "first".to_string(),
                order: order.clone(),
            }))
            .await;
        coordinator
            .register_shutdown_hook(Box::new(OrderHook {
                hook_name: "second".to_string(),
                order: order.clone(),
            }))
            .await;
        coordinator
            .register_shutdown_hook(Box::new(OrderHook {
                hook_name: "third".to_string(),
                order: order.clone(),
            }))
            .await;

        coordinator.shutdown();
        wait_terminated(&coordinator, Duration::from_secs(2)).await;

        let executed = order.lock().await;
        assert_eq!(*executed, vec!["first", "second", "third"]);
    }

    #[tokio::test]
    async fn test_hook_failure_does_not_block_others() {
        let config = DrainConfig {
            drain_timeout: Duration::from_millis(50),
            check_interval: Duration::from_millis(10),
            flush_timeout: Duration::from_secs(1),
        };
        let coordinator = ShutdownCoordinator::with_config(config);

        let completed = Arc::new(StdAtomicBool::new(false));

        struct FailingHook;

        #[async_trait::async_trait]
        impl ShutdownHook for FailingHook {
            fn name(&self) -> &str {
                "failing"
            }
            async fn on_shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                Err("intentional failure".into())
            }
        }

        struct SuccessHook {
            completed: Arc<StdAtomicBool>,
        }

        #[async_trait::async_trait]
        impl ShutdownHook for SuccessHook {
            fn name(&self) -> &str {
                "success_after_failure"
            }
            async fn on_shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                self.completed.store(true, Ordering::SeqCst);
                Ok(())
            }
        }

        coordinator
            .register_shutdown_hook(Box::new(FailingHook))
            .await;
        coordinator
            .register_shutdown_hook(Box::new(SuccessHook {
                completed: completed.clone(),
            }))
            .await;

        coordinator.shutdown();
        wait_terminated(&coordinator, Duration::from_secs(2)).await;

        assert!(
            completed.load(Ordering::SeqCst),
            "Hook after failing hook should still run"
        );
        assert_eq!(coordinator.inner.hooks_completed.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_status_reporting() {
        let config = DrainConfig {
            drain_timeout: Duration::from_secs(1),
            check_interval: Duration::from_millis(50),
            flush_timeout: Duration::from_millis(200),
        };
        let coordinator = ShutdownCoordinator::with_config(config);

        // Before shutdown
        let st = coordinator.status();
        assert_eq!(st.phase, ShutdownPhase::Running);
        assert_eq!(st.active_requests, 0);
        assert_eq!(st.hooks_completed, 0);
        assert_eq!(st.elapsed_ms, 0);

        coordinator.request_start();
        coordinator.request_start();

        let st = coordinator.status();
        assert_eq!(st.active_requests, 2);

        coordinator.request_end();
        coordinator.request_end();

        coordinator.shutdown();

        // Give it a moment so elapsed_ms is measurably > 0
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Check while still running or after completion
        let st = coordinator.status();
        assert!(st.elapsed_ms > 0, "elapsed_ms should be > 0 after shutdown");

        // Wait for full completion
        wait_terminated(&coordinator, Duration::from_secs(2)).await;

        let st = coordinator.status();
        assert_eq!(st.phase, ShutdownPhase::Terminated);
    }

    #[tokio::test]
    async fn test_zero_active_requests_fast_shutdown() {
        let config = DrainConfig {
            drain_timeout: Duration::from_secs(30),
            check_interval: Duration::from_millis(50),
            flush_timeout: Duration::from_millis(100),
        };
        let coordinator = ShutdownCoordinator::with_config(config);

        let start = Instant::now();
        coordinator.shutdown();

        wait_terminated(&coordinator, Duration::from_secs(1)).await;

        assert_eq!(coordinator.current_phase(), ShutdownPhase::Terminated);
        // With zero requests the drain phase should be nearly instant
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "Fast shutdown should complete quickly, took {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_health_status_label() {
        let coordinator = ShutdownCoordinator::new();
        assert_eq!(coordinator.health_status_label(), "ok");

        coordinator.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Once shutdown begins, label changes
        assert_eq!(coordinator.health_status_label(), "shutting_down");
    }

    #[tokio::test]
    async fn test_request_guard_raii() {
        let coordinator = ShutdownCoordinator::new();
        assert_eq!(coordinator.active_request_count(), 0);

        {
            let _g1 = RequestGuard::new(coordinator.clone());
            assert_eq!(coordinator.active_request_count(), 1);
            {
                let _g2 = RequestGuard::new(coordinator.clone());
                assert_eq!(coordinator.active_request_count(), 2);
            }
            // g2 dropped
            assert_eq!(coordinator.active_request_count(), 1);
        }
        // g1 dropped
        assert_eq!(coordinator.active_request_count(), 0);
    }

    #[tokio::test]
    async fn test_is_accepting() {
        let coordinator = ShutdownCoordinator::new();
        assert!(coordinator.is_accepting());

        coordinator.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!coordinator.is_accepting());
    }

    #[tokio::test]
    async fn test_built_in_hooks() {
        let config = DrainConfig {
            drain_timeout: Duration::from_millis(50),
            check_interval: Duration::from_millis(10),
            flush_timeout: Duration::from_secs(5),
        };
        let coordinator = ShutdownCoordinator::with_config(config);

        coordinator
            .register_shutdown_hook(Box::new(WalFlushHook::default()))
            .await;
        coordinator
            .register_shutdown_hook(Box::new(MemtableFlushHook::default()))
            .await;
        coordinator
            .register_shutdown_hook(Box::new(MetricsSnapshotHook::default()))
            .await;

        let st = coordinator.status();
        assert_eq!(st.hooks_total, 3);

        coordinator.shutdown();
        wait_terminated(&coordinator, Duration::from_secs(2)).await;

        assert_eq!(coordinator.inner.hooks_completed.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_multiple_shutdown_signals_idempotent() {
        let coordinator = ShutdownCoordinator::new();
        let mut rx = coordinator.subscribe();

        // First call should succeed
        coordinator.shutdown();
        let recv_result = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(recv_result.is_ok());

        // Subsequent calls are no-ops (no additional broadcast)
        coordinator.shutdown();
        coordinator.shutdown();
        coordinator.shutdown();

        assert!(coordinator.is_shutting_down());

        wait_terminated(&coordinator, Duration::from_secs(2)).await;
        assert_eq!(coordinator.current_phase(), ShutdownPhase::Terminated);
    }

    #[tokio::test]
    async fn test_drain_config_default() {
        let cfg = DrainConfig::default();
        assert_eq!(cfg.drain_timeout, Duration::from_secs(30));
        assert_eq!(cfg.check_interval, Duration::from_secs(1));
        assert_eq!(cfg.flush_timeout, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_phase_display() {
        assert_eq!(format!("{}", ShutdownPhase::Running), "Running");
        assert_eq!(format!("{}", ShutdownPhase::Draining), "Draining");
        assert_eq!(format!("{}", ShutdownPhase::FlushingState), "FlushingState");
        assert_eq!(format!("{}", ShutdownPhase::Terminated), "Terminated");
    }

    // -- Storage integration hook tests -------------------------------------

    /// Mock WalWriter for testing
    struct MockWalWriter {
        sync_called: Arc<StdAtomicBool>,
        size: u64,
        should_fail: bool,
    }

    impl WalWriter for MockWalWriter {
        fn sync(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.sync_called.store(true, Ordering::SeqCst);
            if self.should_fail {
                return Err("WAL sync failed".into());
            }
            Ok(())
        }

        fn current_size(&self) -> u64 {
            self.size
        }
    }

    /// Mock MemtableFlusher for testing
    struct MockMemtableFlusher {
        flush_called: Arc<StdAtomicBool>,
        entries: usize,
        should_fail: bool,
    }

    impl MemtableFlusher for MockMemtableFlusher {
        fn flush_to_sstable(&self) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
            self.flush_called.store(true, Ordering::SeqCst);
            if self.should_fail {
                return Err("memtable flush failed".into());
            }
            Ok(self.entries)
        }
    }

    #[tokio::test]
    async fn test_wal_flush_hook_calls_sync() {
        let sync_called = Arc::new(StdAtomicBool::new(false));
        let writer = Arc::new(MockWalWriter {
            sync_called: sync_called.clone(),
            size: 4096,
            should_fail: false,
        });

        let hook = WalFlushHook::with_writer(writer, Duration::from_secs(5));
        let result = hook.on_shutdown().await;

        assert!(result.is_ok());
        assert!(
            sync_called.load(Ordering::SeqCst),
            "sync() should have been called"
        );
    }

    #[tokio::test]
    async fn test_wal_flush_hook_no_writer() {
        let hook = WalFlushHook::default();
        let result = hook.on_shutdown().await;
        assert!(result.is_ok(), "no-writer hook should succeed");
    }

    #[tokio::test]
    async fn test_wal_flush_hook_error() {
        let sync_called = Arc::new(StdAtomicBool::new(false));
        let writer = Arc::new(MockWalWriter {
            sync_called: sync_called.clone(),
            size: 1024,
            should_fail: true,
        });

        let hook = WalFlushHook::with_writer(writer, Duration::from_secs(5));
        let result = hook.on_shutdown().await;

        assert!(result.is_err());
        assert!(
            sync_called.load(Ordering::SeqCst),
            "sync() should have been called even on failure"
        );
        let err_msg = result.expect_err("should be error").to_string();
        assert!(
            err_msg.contains("WAL sync failed"),
            "error message should propagate"
        );
    }

    #[tokio::test]
    async fn test_memtable_flush_hook_calls_flush() {
        let flush_called = Arc::new(StdAtomicBool::new(false));
        let flusher = Arc::new(MockMemtableFlusher {
            flush_called: flush_called.clone(),
            entries: 42,
            should_fail: false,
        });

        let hook = MemtableFlushHook::with_flusher(flusher, Duration::from_secs(5));
        let result = hook.on_shutdown().await;

        assert!(result.is_ok());
        assert!(
            flush_called.load(Ordering::SeqCst),
            "flush_to_sstable() should have been called"
        );
    }

    #[tokio::test]
    async fn test_memtable_flush_hook_no_flusher() {
        let hook = MemtableFlushHook::default();
        let result = hook.on_shutdown().await;
        assert!(result.is_ok(), "no-flusher hook should succeed");
    }

    #[tokio::test]
    async fn test_connection_drain_immediate() {
        let conns = Arc::new(AtomicUsize::new(0));
        let hook = ConnectionDrainHook::new(conns, Duration::from_secs(5));

        let start = Instant::now();
        let result = hook.on_shutdown().await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(
            elapsed < Duration::from_millis(50),
            "should return immediately with 0 connections, took {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_connection_drain_waits_for_zero() {
        let conns = Arc::new(AtomicUsize::new(5));
        let hook = ConnectionDrainHook::new(conns.clone(), Duration::from_secs(5))
            .with_poll_interval(Duration::from_millis(50));

        // Spawn a task that decrements connections over time
        let conns_clone = conns.clone();
        tokio::spawn(async move {
            for _ in 0..5 {
                tokio::time::sleep(Duration::from_millis(30)).await;
                conns_clone.fetch_sub(1, Ordering::SeqCst);
            }
        });

        let result = hook.on_shutdown().await;
        assert!(result.is_ok());
        assert_eq!(conns.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_connection_drain_timeout() {
        let conns = Arc::new(AtomicUsize::new(10));
        let hook = ConnectionDrainHook::new(conns.clone(), Duration::from_millis(200))
            .with_poll_interval(Duration::from_millis(50));

        let start = Instant::now();
        let result = hook.on_shutdown().await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "should error on timeout");
        let err_msg = result.expect_err("should be error").to_string();
        assert!(
            err_msg.contains("timed out"),
            "error should mention timeout"
        );
        assert!(
            elapsed >= Duration::from_millis(200),
            "should have waited at least the timeout duration, elapsed {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_hook_execution_result_captured() {
        let config = DrainConfig {
            drain_timeout: Duration::from_millis(50),
            check_interval: Duration::from_millis(10),
            flush_timeout: Duration::from_secs(1),
        };
        let coordinator = ShutdownCoordinator::with_config(config);

        struct NamedHook {
            hook_name: String,
        }

        #[async_trait::async_trait]
        impl ShutdownHook for NamedHook {
            fn name(&self) -> &str {
                &self.hook_name
            }
            async fn on_shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                Ok(())
            }
        }

        coordinator
            .register_shutdown_hook(Box::new(NamedHook {
                hook_name: "test_hook".to_string(),
            }))
            .await;

        coordinator.shutdown();
        wait_terminated(&coordinator, Duration::from_secs(2)).await;

        let results = coordinator.hook_results().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].hook_name, "test_hook");
        assert!(results[0].success);
        assert!(results[0].error.is_none());
        assert!(results[0].duration < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn test_hook_error_result() {
        let config = DrainConfig {
            drain_timeout: Duration::from_millis(50),
            check_interval: Duration::from_millis(10),
            flush_timeout: Duration::from_secs(1),
        };
        let coordinator = ShutdownCoordinator::with_config(config);

        struct FailHook;

        #[async_trait::async_trait]
        impl ShutdownHook for FailHook {
            fn name(&self) -> &str {
                "fail_hook"
            }
            async fn on_shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                Err("catastrophic failure".into())
            }
        }

        coordinator.register_shutdown_hook(Box::new(FailHook)).await;

        coordinator.shutdown();
        wait_terminated(&coordinator, Duration::from_secs(2)).await;

        let results = coordinator.hook_results().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].hook_name, "fail_hook");
        assert!(!results[0].success);
        assert!(results[0].error.is_some());
        let err = results[0].error.as_ref().expect("error should be present");
        assert!(
            err.contains("catastrophic failure"),
            "error should contain the failure message"
        );
    }

    #[tokio::test]
    async fn test_metrics_snapshot_writes_file() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("metrics.bin");

        let expected_data = b"metric1=42\nmetric2=100\n".to_vec();
        let expected_clone = expected_data.clone();
        let provider: Arc<dyn Fn() -> Vec<u8> + Send + Sync> =
            Arc::new(move || expected_clone.clone());

        let hook =
            MetricsSnapshotHook::with_provider(provider, path.clone(), Duration::from_secs(5));
        let result = hook.on_shutdown().await;

        assert!(result.is_ok());
        let written = std::fs::read(&path).expect("should be able to read metrics file");
        assert_eq!(written, expected_data);
    }

    #[tokio::test]
    async fn test_metrics_snapshot_no_provider() {
        let hook = MetricsSnapshotHook::default();
        let result = hook.on_shutdown().await;
        assert!(result.is_ok(), "no-provider hook should succeed");
    }

    #[tokio::test]
    async fn test_connection_drain_poll_interval() {
        // Use a connection count that will require multiple polls
        let conns = Arc::new(AtomicUsize::new(1));
        let poll_interval = Duration::from_millis(80);
        let hook = ConnectionDrainHook::new(conns.clone(), Duration::from_secs(5))
            .with_poll_interval(poll_interval);

        // Spawn task to zero out connections after ~150ms
        let conns_clone = conns.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            conns_clone.store(0, Ordering::SeqCst);
        });

        let start = Instant::now();
        let result = hook.on_shutdown().await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        // Should have polled at least once (~80ms) before connections hit 0 at ~150ms
        assert!(
            elapsed >= Duration::from_millis(100),
            "should have polled at least once before completion, elapsed {:?}",
            elapsed
        );
    }
}
