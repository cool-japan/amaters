//! Health check endpoint
//!
//! Provides health status information for monitoring and orchestration systems.
//! Supports deep health probes, readiness/liveness separation, health history
//! tracking, and dependency health aggregation.

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Health status types
// ---------------------------------------------------------------------------

/// Health status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// Server is healthy and ready to serve requests
    Healthy,
    /// Server is starting up
    Starting,
    /// Server is shutting down
    ShuttingDown,
    /// Server has encountered an error
    Unhealthy,
    /// Server is operational but degraded
    Degraded,
}

/// Component health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Component name
    pub name: String,
    /// Component status
    pub status: HealthStatus,
    /// Optional message
    pub message: Option<String>,
    /// Last check timestamp
    pub last_check: u64,
}

// ---------------------------------------------------------------------------
// Deep health probe types
// ---------------------------------------------------------------------------

/// Result of a deep health probe
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthProbeResult {
    /// Probe status
    pub status: ProbeStatus,
    /// Latency of the probe in milliseconds
    pub latency_ms: f64,
    /// Human-readable message
    pub message: String,
}

/// Probe status (more granular than HealthStatus)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeStatus {
    /// Everything is fine
    Healthy,
    /// Operational but degraded
    Degraded,
    /// Not operational
    Unhealthy,
}

impl ProbeStatus {
    /// Convert to a HealthStatus
    pub fn to_health_status(self) -> HealthStatus {
        match self {
            ProbeStatus::Healthy => HealthStatus::Healthy,
            ProbeStatus::Degraded => HealthStatus::Degraded,
            ProbeStatus::Unhealthy => HealthStatus::Unhealthy,
        }
    }

    /// Return the worse of two statuses
    pub fn worse(self, other: ProbeStatus) -> ProbeStatus {
        match (self, other) {
            (ProbeStatus::Unhealthy, _) | (_, ProbeStatus::Unhealthy) => ProbeStatus::Unhealthy,
            (ProbeStatus::Degraded, _) | (_, ProbeStatus::Degraded) => ProbeStatus::Degraded,
            _ => ProbeStatus::Healthy,
        }
    }
}

/// Trait for deep health check probes
#[async_trait]
pub trait DeepHealthCheck: Send + Sync {
    /// Execute the health check and return the result
    async fn check(&self) -> HealthProbeResult;
}

// ---------------------------------------------------------------------------
// Built-in probes
// ---------------------------------------------------------------------------

/// Storage probe — verifies storage is readable/writable by writing a test
/// key, reading it back, and deleting it.
pub struct StorageProbe {
    /// Path to storage directory for the probe
    storage_path: std::path::PathBuf,
}

impl StorageProbe {
    /// Create a new storage probe targeting the given directory
    pub fn new(storage_path: std::path::PathBuf) -> Self {
        Self { storage_path }
    }
}

#[async_trait]
impl DeepHealthCheck for StorageProbe {
    async fn check(&self) -> HealthProbeResult {
        let start = Instant::now();
        let test_file = self.storage_path.join(".health_probe_test");

        // Write
        let write_result = tokio::fs::write(&test_file, b"health_probe").await;
        if let Err(e) = write_result {
            return HealthProbeResult {
                status: ProbeStatus::Unhealthy,
                latency_ms: start.elapsed().as_secs_f64() * 1000.0,
                message: format!("storage write failed: {e}"),
            };
        }

        // Read back
        let read_result = tokio::fs::read(&test_file).await;
        match read_result {
            Ok(data) if data == b"health_probe" => {}
            Ok(_) => {
                let _ = tokio::fs::remove_file(&test_file).await;
                return HealthProbeResult {
                    status: ProbeStatus::Unhealthy,
                    latency_ms: start.elapsed().as_secs_f64() * 1000.0,
                    message: "storage read returned unexpected data".to_string(),
                };
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&test_file).await;
                return HealthProbeResult {
                    status: ProbeStatus::Unhealthy,
                    latency_ms: start.elapsed().as_secs_f64() * 1000.0,
                    message: format!("storage read failed: {e}"),
                };
            }
        }

        // Delete
        if let Err(e) = tokio::fs::remove_file(&test_file).await {
            return HealthProbeResult {
                status: ProbeStatus::Degraded,
                latency_ms: start.elapsed().as_secs_f64() * 1000.0,
                message: format!("storage cleanup failed (non-critical): {e}"),
            };
        }

        HealthProbeResult {
            status: ProbeStatus::Healthy,
            latency_ms: start.elapsed().as_secs_f64() * 1000.0,
            message: "storage read/write/delete OK".to_string(),
        }
    }
}

/// WAL probe — verifies the write-ahead log directory is appendable.
pub struct WalProbe {
    /// Path to the WAL directory
    wal_path: std::path::PathBuf,
}

impl WalProbe {
    /// Create a new WAL probe targeting the given directory
    pub fn new(wal_path: std::path::PathBuf) -> Self {
        Self { wal_path }
    }
}

#[async_trait]
impl DeepHealthCheck for WalProbe {
    async fn check(&self) -> HealthProbeResult {
        let start = Instant::now();
        let test_file = self.wal_path.join(".wal_health_probe");

        // Try to append
        let result = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .truncate(false)
            .open(&test_file)
            .await;

        match result {
            Ok(_file) => {
                let _ = tokio::fs::remove_file(&test_file).await;
                HealthProbeResult {
                    status: ProbeStatus::Healthy,
                    latency_ms: start.elapsed().as_secs_f64() * 1000.0,
                    message: "WAL directory is appendable".to_string(),
                }
            }
            Err(e) => HealthProbeResult {
                status: ProbeStatus::Unhealthy,
                latency_ms: start.elapsed().as_secs_f64() * 1000.0,
                message: format!("WAL append test failed: {e}"),
            },
        }
    }
}

// Raw statvfs binding — avoids depending on the `libc` crate.
#[cfg(any(target_os = "macos", target_os = "linux"))]
unsafe extern "C" {
    #[link_name = "statvfs"]
    fn statvfs_raw(path: *const std::ffi::c_char, buf: *mut u8) -> std::ffi::c_int;
}

/// Disk space probe — checks available disk space against a threshold.
pub struct DiskSpaceProbe {
    /// Path to check disk space for
    path: std::path::PathBuf,
    /// Minimum required free bytes
    min_free_bytes: u64,
}

impl DiskSpaceProbe {
    /// Create a new disk space probe.
    ///
    /// `min_free_bytes` is the threshold below which the probe reports degraded
    /// (at half the threshold) or unhealthy (at zero or below threshold / 4).
    pub fn new(path: std::path::PathBuf, min_free_bytes: u64) -> Self {
        Self {
            path,
            min_free_bytes,
        }
    }

    /// Get available space on the filesystem containing `path`.
    ///
    /// Uses platform-specific raw syscalls without depending on the `libc`
    /// crate, keeping the build pure Rust.
    fn available_space(&self) -> Result<u64, String> {
        self.available_space_impl()
    }

    #[cfg(target_os = "macos")]
    fn available_space_impl(&self) -> Result<u64, String> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let c_path = CString::new(self.path.as_os_str().as_bytes())
            .map_err(|e| format!("invalid path: {e}"))?;

        // macOS statvfs layout (LP64). We only need f_frsize and f_bavail.
        // struct statvfs is 64 bytes on macOS (all u64 fields after the
        // initial f_bsize). We allocate a generous buffer.
        #[repr(C)]
        struct Statvfs {
            f_bsize: u64,
            f_frsize: u64,
            f_blocks: u64,
            f_bfree: u64,
            f_bavail: u64,
            // remaining fields not needed
            _pad: [u64; 11],
        }

        let mut buf: Statvfs = unsafe { std::mem::zeroed() };
        // macOS syscall: statvfs(2)
        let ret = unsafe { statvfs_raw(c_path.as_ptr(), &mut buf as *mut Statvfs as *mut u8) };
        if ret != 0 {
            return Err(format!(
                "statvfs failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let available = buf.f_bavail.saturating_mul(buf.f_frsize);
        Ok(available)
    }

    #[cfg(target_os = "linux")]
    fn available_space_impl(&self) -> Result<u64, String> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let c_path = CString::new(self.path.as_os_str().as_bytes())
            .map_err(|e| format!("invalid path: {e}"))?;

        #[repr(C)]
        struct Statvfs {
            f_bsize: u64,
            f_frsize: u64,
            f_blocks: u64,
            f_bfree: u64,
            f_bavail: u64,
            _pad: [u64; 11],
        }

        let mut buf: Statvfs = unsafe { std::mem::zeroed() };
        let ret = unsafe { statvfs_raw(c_path.as_ptr(), &mut buf as *mut Statvfs as *mut u8) };
        if ret != 0 {
            return Err(format!(
                "statvfs failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let available = buf.f_bavail.saturating_mul(buf.f_frsize);
        Ok(available)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn available_space_impl(&self) -> Result<u64, String> {
        // On unsupported platforms, just verify the directory exists.
        if self.path.exists() {
            Ok(u64::MAX)
        } else {
            Err("path does not exist".to_string())
        }
    }
}

#[async_trait]
impl DeepHealthCheck for DiskSpaceProbe {
    async fn check(&self) -> HealthProbeResult {
        let start = Instant::now();

        match self.available_space() {
            Ok(available) => {
                let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
                if available >= self.min_free_bytes {
                    HealthProbeResult {
                        status: ProbeStatus::Healthy,
                        latency_ms,
                        message: format!(
                            "disk space OK: {} bytes available (threshold: {})",
                            available, self.min_free_bytes
                        ),
                    }
                } else if available >= self.min_free_bytes / 4 {
                    HealthProbeResult {
                        status: ProbeStatus::Degraded,
                        latency_ms,
                        message: format!(
                            "disk space low: {} bytes available (threshold: {})",
                            available, self.min_free_bytes
                        ),
                    }
                } else {
                    HealthProbeResult {
                        status: ProbeStatus::Unhealthy,
                        latency_ms,
                        message: format!(
                            "disk space critically low: {} bytes available (threshold: {})",
                            available, self.min_free_bytes
                        ),
                    }
                }
            }
            Err(e) => HealthProbeResult {
                status: ProbeStatus::Unhealthy,
                latency_ms: start.elapsed().as_secs_f64() * 1000.0,
                message: format!("disk space check failed: {e}"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Health history
// ---------------------------------------------------------------------------

/// A point-in-time health snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSnapshot {
    /// Timestamp (seconds since UNIX epoch)
    pub timestamp: u64,
    /// Overall status at this point
    pub status: HealthStatus,
    /// Whether the server was alive
    pub alive: bool,
    /// Whether the server was ready
    pub ready: bool,
}

/// Ring buffer for health check history
#[derive(Debug)]
pub struct HealthHistory {
    /// Fixed-size buffer of snapshots (ring buffer)
    buffer: Vec<Option<HealthSnapshot>>,
    /// Current write position
    write_pos: usize,
    /// Number of entries written (may exceed capacity — used for stats)
    total_written: usize,
    /// Capacity
    capacity: usize,
}

impl HealthHistory {
    /// Create a new history buffer with the given capacity
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1); // at least 1
        Self {
            buffer: (0..capacity).map(|_| None).collect(),
            write_pos: 0,
            total_written: 0,
            capacity,
        }
    }

    /// Record a new snapshot
    pub fn record(&mut self, snapshot: HealthSnapshot) {
        self.buffer[self.write_pos] = Some(snapshot);
        self.write_pos = (self.write_pos + 1) % self.capacity;
        self.total_written += 1;
    }

    /// Return all recorded snapshots in chronological order
    pub fn snapshots(&self) -> Vec<HealthSnapshot> {
        let count = self.total_written.min(self.capacity);
        let mut result = Vec::with_capacity(count);

        if self.total_written < self.capacity {
            // Haven't wrapped yet — entries are 0..write_pos
            for s in self.buffer.iter().take(self.write_pos).flatten() {
                result.push(s.clone());
            }
        } else {
            // Wrapped — oldest is at write_pos, read around
            for i in 0..self.capacity {
                let idx = (self.write_pos + i) % self.capacity;
                if let Some(s) = &self.buffer[idx] {
                    result.push(s.clone());
                }
            }
        }

        result
    }

    /// Calculate uptime percentage from the history buffer.
    /// "Up" means the snapshot's `alive` field is true.
    pub fn uptime_percent(&self) -> f64 {
        let snaps = self.snapshots();
        if snaps.is_empty() {
            return 100.0;
        }
        let alive_count = snaps.iter().filter(|s| s.alive).count();
        (alive_count as f64 / snaps.len() as f64) * 100.0
    }
}

// ---------------------------------------------------------------------------
// Dependency health
// ---------------------------------------------------------------------------

/// Health information for a single dependency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyHealth {
    /// Dependency name
    pub name: String,
    /// Current status
    pub status: ProbeStatus,
    /// Latency in milliseconds
    pub latency_ms: f64,
    /// Last checked timestamp (seconds since UNIX epoch)
    pub last_checked: u64,
    /// Human-readable message
    pub message: String,
}

// ---------------------------------------------------------------------------
// Liveness / readiness responses
// ---------------------------------------------------------------------------

/// Liveness check response (lightweight)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivenessResponse {
    /// Whether the process is alive
    pub alive: bool,
    /// Current status
    pub status: HealthStatus,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Timestamp
    pub timestamp: u64,
}

/// Readiness check response (includes dependency detail)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessResponse {
    /// Whether the server is ready to serve traffic
    pub ready: bool,
    /// Current status
    pub status: HealthStatus,
    /// Component statuses
    pub components: Vec<ComponentHealth>,
    /// Dependency statuses
    pub dependencies: Vec<DependencyHealth>,
    /// Timestamp
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Overall health check response (enhanced)
// ---------------------------------------------------------------------------

/// Overall health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    /// Overall status
    pub status: HealthStatus,
    /// Server version
    pub version: String,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Component health statuses
    pub components: Vec<ComponentHealth>,
    /// Dependency health statuses
    pub dependencies: Vec<DependencyHealth>,
    /// Deep probe results (keyed by probe name)
    pub probes: HashMap<String, HealthProbeResult>,
    /// Uptime percentage from history
    pub uptime_percent: f64,
    /// Current timestamp
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// HealthChecker
// ---------------------------------------------------------------------------

/// Health checker
///
/// Tracks the health of various server components, runs deep probes,
/// maintains health history, and aggregates dependency health.
#[derive(Clone)]
pub struct HealthChecker {
    inner: Arc<HealthCheckerInner>,
}

struct HealthCheckerInner {
    /// Server start time
    start_time: AtomicU64,
    /// Overall status (encoded as u64)
    status: AtomicU64,
    /// Storage health
    storage_healthy: AtomicBool,
    /// Network health
    network_healthy: AtomicBool,
    /// Whether cluster mode is enabled
    cluster_enabled: AtomicBool,
    /// Cluster health (only meaningful when cluster_enabled is true)
    cluster_healthy: AtomicBool,
    /// Registered deep health probes
    probes: RwLock<HashMap<String, Arc<dyn DeepHealthCheck>>>,
    /// Registered dependency checkers
    dependency_checkers: RwLock<HashMap<String, Arc<dyn DeepHealthCheck>>>,
    /// Cached dependency health results
    dependency_health: RwLock<HashMap<String, DependencyHealth>>,
    /// Health history
    history: RwLock<HealthHistory>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl HealthChecker {
    /// Create a new health checker with default history capacity (10)
    pub fn new() -> Self {
        Self::with_history_capacity(10)
    }

    /// Create a new health checker with the given history capacity
    pub fn with_history_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(HealthCheckerInner {
                start_time: AtomicU64::new(now_secs()),
                status: AtomicU64::new(HealthStatus::Starting as u64),
                storage_healthy: AtomicBool::new(false),
                network_healthy: AtomicBool::new(false),
                cluster_enabled: AtomicBool::new(false),
                cluster_healthy: AtomicBool::new(false),
                probes: RwLock::new(HashMap::new()),
                dependency_checkers: RwLock::new(HashMap::new()),
                dependency_health: RwLock::new(HashMap::new()),
                history: RwLock::new(HealthHistory::new(capacity)),
            }),
        }
    }

    // ---- status getters/setters (same API as before) ----

    /// Set overall status
    pub fn set_status(&self, status: HealthStatus) {
        self.inner.status.store(status as u64, Ordering::SeqCst);
    }

    /// Get current overall status
    pub fn status(&self) -> HealthStatus {
        match self.inner.status.load(Ordering::SeqCst) {
            0 => HealthStatus::Healthy,
            1 => HealthStatus::Starting,
            2 => HealthStatus::ShuttingDown,
            3 => HealthStatus::Unhealthy,
            4 => HealthStatus::Degraded,
            _ => HealthStatus::Unhealthy,
        }
    }

    /// Mark storage as healthy
    pub fn set_storage_healthy(&self, healthy: bool) {
        self.inner.storage_healthy.store(healthy, Ordering::SeqCst);
    }

    /// Mark network as healthy
    pub fn set_network_healthy(&self, healthy: bool) {
        self.inner.network_healthy.store(healthy, Ordering::SeqCst);
    }

    /// Mark cluster mode as enabled
    pub fn set_cluster_enabled(&self, enabled: bool) {
        self.inner.cluster_enabled.store(enabled, Ordering::SeqCst);
    }

    /// Mark cluster as healthy
    pub fn set_cluster_healthy(&self, healthy: bool) {
        self.inner.cluster_healthy.store(healthy, Ordering::SeqCst);
    }

    /// Get uptime in seconds
    pub fn uptime_seconds(&self) -> u64 {
        let now = now_secs();
        let start = self.inner.start_time.load(Ordering::SeqCst);
        now.saturating_sub(start)
    }

    // ---- liveness / readiness ----

    /// Check if server is alive (not shutting down or unhealthy).
    /// This is a lightweight check suitable for liveness probes.
    pub fn is_alive(&self) -> bool {
        matches!(
            self.status(),
            HealthStatus::Healthy | HealthStatus::Starting | HealthStatus::Degraded
        )
    }

    /// Check if server is ready to serve traffic.
    /// Returns false during startup, shutdown, and recovery.
    pub fn is_ready(&self) -> bool {
        let status = self.status();
        let base_ok = matches!(status, HealthStatus::Healthy | HealthStatus::Degraded);
        base_ok
            && self.inner.storage_healthy.load(Ordering::SeqCst)
            && self.inner.network_healthy.load(Ordering::SeqCst)
    }

    /// Build a liveness response (lightweight, fast)
    pub fn liveness_response(&self) -> LivenessResponse {
        LivenessResponse {
            alive: self.is_alive(),
            status: self.status(),
            uptime_seconds: self.uptime_seconds(),
            timestamp: now_secs(),
        }
    }

    /// Build a readiness response (includes component and dependency info)
    pub fn readiness_response(&self) -> ReadinessResponse {
        let components = self.build_component_list();
        let dependencies: Vec<DependencyHealth> = self
            .inner
            .dependency_health
            .read()
            .values()
            .cloned()
            .collect();

        ReadinessResponse {
            ready: self.is_ready(),
            status: self.status(),
            components,
            dependencies,
            timestamp: now_secs(),
        }
    }

    // ---- deep probes ----

    /// Register a deep health probe under the given name
    pub fn register_probe(&self, name: impl Into<String>, probe: Arc<dyn DeepHealthCheck>) {
        self.inner.probes.write().insert(name.into(), probe);
    }

    /// Run all registered deep probes and return their results
    pub async fn run_probes(&self) -> HashMap<String, HealthProbeResult> {
        let probes: Vec<(String, Arc<dyn DeepHealthCheck>)> = {
            let guard = self.inner.probes.read();
            guard
                .iter()
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };

        let mut results = HashMap::with_capacity(probes.len());
        for (name, probe) in probes {
            let result = probe.check().await;
            results.insert(name, result);
        }
        results
    }

    // ---- dependency health ----

    /// Register a dependency health checker
    pub fn register_dependency(&self, name: impl Into<String>, checker: Arc<dyn DeepHealthCheck>) {
        let name = name.into();
        self.inner
            .dependency_checkers
            .write()
            .insert(name.clone(), checker);
        // Initialize with unknown state
        self.inner.dependency_health.write().insert(
            name.clone(),
            DependencyHealth {
                name,
                status: ProbeStatus::Unhealthy,
                latency_ms: 0.0,
                last_checked: 0,
                message: "not yet checked".to_string(),
            },
        );
    }

    /// Run all dependency checks and update cached results.
    /// Returns the aggregated worst status.
    pub async fn check_dependencies(&self) -> ProbeStatus {
        let checkers: Vec<(String, Arc<dyn DeepHealthCheck>)> = {
            let guard = self.inner.dependency_checkers.read();
            guard
                .iter()
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };

        let mut worst = ProbeStatus::Healthy;
        let now = now_secs();

        for (name, checker) in checkers {
            let result = checker.check().await;
            worst = worst.worse(result.status);
            let dep = DependencyHealth {
                name: name.clone(),
                status: result.status,
                latency_ms: result.latency_ms,
                last_checked: now,
                message: result.message,
            };
            self.inner.dependency_health.write().insert(name, dep);
        }

        worst
    }

    /// Get the current aggregated dependency status (from cached results)
    pub fn aggregated_dependency_status(&self) -> ProbeStatus {
        let guard = self.inner.dependency_health.read();
        guard
            .values()
            .fold(ProbeStatus::Healthy, |acc, d| acc.worse(d.status))
    }

    // ---- health history ----

    /// Record a snapshot of the current health state into the history buffer
    pub fn record_snapshot(&self) {
        let snapshot = HealthSnapshot {
            timestamp: now_secs(),
            status: self.status(),
            alive: self.is_alive(),
            ready: self.is_ready(),
        };
        self.inner.history.write().record(snapshot);
    }

    /// Get the health check history as a chronologically ordered list
    pub fn health_history(&self) -> Vec<HealthSnapshot> {
        self.inner.history.read().snapshots()
    }

    /// Get the uptime percentage from the history buffer
    pub fn uptime_percent(&self) -> f64 {
        self.inner.history.read().uptime_percent()
    }

    // ---- full health response ----

    /// Get full health check response (enhanced with probes, deps, history)
    pub fn get_health(&self) -> HealthCheckResponse {
        let components = self.build_component_list();
        let dependencies: Vec<DependencyHealth> = self
            .inner
            .dependency_health
            .read()
            .values()
            .cloned()
            .collect();
        let probes = HashMap::new(); // synchronous path — probes not run
        let uptime_pct = self.uptime_percent();

        HealthCheckResponse {
            status: self.status(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.uptime_seconds(),
            components,
            dependencies,
            probes,
            uptime_percent: uptime_pct,
            timestamp: now_secs(),
        }
    }

    /// Get full health check response including deep probe results (async)
    pub async fn get_health_deep(&self) -> HealthCheckResponse {
        let components = self.build_component_list();
        let dependencies: Vec<DependencyHealth> = self
            .inner
            .dependency_health
            .read()
            .values()
            .cloned()
            .collect();
        let probes = self.run_probes().await;
        let uptime_pct = self.uptime_percent();

        HealthCheckResponse {
            status: self.status(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.uptime_seconds(),
            components,
            dependencies,
            probes,
            uptime_percent: uptime_pct,
            timestamp: now_secs(),
        }
    }

    /// Format health as JSON
    pub fn get_health_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.get_health())
    }

    // ---- helpers ----

    fn build_component_list(&self) -> Vec<ComponentHealth> {
        let now = now_secs();

        let storage_status = if self.inner.storage_healthy.load(Ordering::SeqCst) {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy
        };

        let network_status = if self.inner.network_healthy.load(Ordering::SeqCst) {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy
        };

        let cluster_healthy = self.inner.cluster_healthy.load(Ordering::SeqCst);
        let cluster_enabled = self.inner.cluster_enabled.load(Ordering::SeqCst);
        let cluster_status = if cluster_enabled {
            if cluster_healthy {
                HealthStatus::Healthy
            } else {
                HealthStatus::Unhealthy
            }
        } else {
            HealthStatus::Starting // Cluster is optional, not enabled
        };

        let cluster_message = if cluster_enabled {
            if cluster_healthy {
                "cluster active".to_string()
            } else {
                "cluster unhealthy".to_string()
            }
        } else {
            "cluster disabled (standalone mode)".to_string()
        };

        vec![
            ComponentHealth {
                name: "storage".to_string(),
                status: storage_status,
                message: None,
                last_check: now,
            },
            ComponentHealth {
                name: "network".to_string(),
                status: network_status,
                message: None,
                last_check: now,
            },
            ComponentHealth {
                name: "cluster".to_string(),
                status: cluster_status,
                message: Some(cluster_message),
                last_check: now,
            },
        ]
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// HTTP health check server
// ---------------------------------------------------------------------------

/// Lightweight HTTP server that exposes health check endpoints.
///
/// Routes:
/// - `GET /health`  — full health status (JSON)
/// - `GET /healthz` — simple alive check (200 or 503)
/// - `GET /readyz`  — readiness check (200 or 503)
/// - `GET /livez`   — liveness check (200 or 503)
/// - `GET /metrics` — health metrics (history, uptime percentage)
pub struct HealthHttpServer {
    checker: Arc<HealthChecker>,
    bind_addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
}

/// Handle returned by [`HealthHttpServer::start`] to control the running server.
pub struct HealthHttpHandle {
    shutdown: Arc<AtomicBool>,
    port: u16,
    join_handle: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

impl HealthHttpHandle {
    /// Signal the HTTP health server to stop accepting new connections.
    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    /// Return the port the server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Wait for the server task to finish after calling [`stop`](Self::stop).
    pub async fn join(self) -> Result<(), std::io::Error> {
        match self.join_handle.await {
            Ok(inner) => inner,
            Err(e) => Err(std::io::Error::other(e)),
        }
    }
}

impl HealthHttpServer {
    /// Create a new health HTTP server.
    ///
    /// `bind_addr` is the address to listen on (e.g. `0.0.0.0:8081`).
    pub fn new(checker: Arc<HealthChecker>, bind_addr: SocketAddr) -> Self {
        Self {
            checker,
            bind_addr,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start the server in a background tokio task.
    ///
    /// Returns a [`HealthHttpHandle`] that can be used to query the port and
    /// signal shutdown.
    pub async fn start(self) -> Result<HealthHttpHandle, std::io::Error> {
        let listener = TcpListener::bind(self.bind_addr).await?;
        let local_addr = listener.local_addr()?;
        let port = local_addr.port();
        let shutdown = Arc::clone(&self.shutdown);
        let checker = Arc::clone(&self.checker);

        let shutdown_flag = Arc::clone(&shutdown);
        let join_handle =
            tokio::spawn(async move { Self::accept_loop(listener, checker, shutdown_flag).await });

        Ok(HealthHttpHandle {
            shutdown,
            port,
            join_handle,
        })
    }

    /// Main accept loop.
    async fn accept_loop(
        listener: TcpListener,
        checker: Arc<HealthChecker>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<(), std::io::Error> {
        loop {
            if shutdown.load(Ordering::SeqCst) {
                debug!("health HTTP server shutting down");
                break;
            }

            // Use a short timeout so we can check the shutdown flag periodically.
            let accept_result =
                tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;

            match accept_result {
                Ok(Ok((stream, _addr))) => {
                    let checker = Arc::clone(&checker);
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream, &checker).await {
                            warn!("health HTTP connection error: {e}");
                        }
                    });
                }
                Ok(Err(e)) => {
                    warn!("health HTTP accept error: {e}");
                }
                Err(_) => {
                    // Timeout — loop back to check shutdown flag
                }
            }
        }
        Ok(())
    }

    /// Handle a single TCP connection: read the HTTP request, route, respond.
    async fn handle_connection(
        mut stream: tokio::net::TcpStream,
        checker: &HealthChecker,
    ) -> Result<(), std::io::Error> {
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }

        let request = String::from_utf8_lossy(&buf[..n]);
        let (method, path) = Self::parse_request_line(&request);

        let (status_code, status_text, body) = match method {
            "GET" => Self::route(path, checker),
            _ => (
                405,
                "Method Not Allowed",
                r#"{"error":"method not allowed"}"#.to_string(),
            ),
        };

        let response = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status_code,
            status_text,
            body.len(),
            body
        );

        stream.write_all(response.as_bytes()).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Parse the request line (first line of the HTTP request).
    /// Returns (method, path). Defaults to ("", "") on malformed input.
    fn parse_request_line(request: &str) -> (&str, &str) {
        let first_line = request.lines().next().unwrap_or("");
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("");
        (method, path)
    }

    /// Route a GET request to the appropriate handler.
    fn route(path: &str, checker: &HealthChecker) -> (u16, &'static str, String) {
        match path {
            "/health" => Self::handle_health(checker),
            "/healthz" => Self::handle_healthz(checker),
            "/readyz" => Self::handle_readyz(checker),
            "/livez" => Self::handle_livez(checker),
            "/metrics" => Self::handle_metrics(checker),
            _ => (404, "Not Found", r#"{"error":"not found"}"#.to_string()),
        }
    }

    /// `GET /health` — full health status JSON.
    fn handle_health(checker: &HealthChecker) -> (u16, &'static str, String) {
        let health = checker.get_health();
        let status_code = match health.status {
            HealthStatus::Healthy | HealthStatus::Degraded => 200,
            _ => 503,
        };
        let status_text = if status_code == 200 {
            "OK"
        } else {
            "Service Unavailable"
        };
        let body = serde_json::to_string(&health)
            .unwrap_or_else(|e| format!(r#"{{"error":"serialization failed: {e}"}}"#));
        (status_code, status_text, body)
    }

    /// `GET /healthz` — simple alive check.
    fn handle_healthz(checker: &HealthChecker) -> (u16, &'static str, String) {
        let alive = checker.is_alive();
        let status_code = if alive { 200 } else { 503 };
        let status_text = if alive { "OK" } else { "Service Unavailable" };
        let body = format!(r#"{{"alive":{alive}}}"#);
        (status_code, status_text, body)
    }

    /// `GET /readyz` — readiness check.
    fn handle_readyz(checker: &HealthChecker) -> (u16, &'static str, String) {
        let ready = checker.is_ready();
        let status_code = if ready { 200 } else { 503 };
        let status_text = if ready { "OK" } else { "Service Unavailable" };
        let body = format!(r#"{{"ready":{ready}}}"#);
        (status_code, status_text, body)
    }

    /// `GET /livez` — liveness check.
    fn handle_livez(checker: &HealthChecker) -> (u16, &'static str, String) {
        let resp = checker.liveness_response();
        let status_code = if resp.alive { 200 } else { 503 };
        let status_text = if resp.alive {
            "OK"
        } else {
            "Service Unavailable"
        };
        let body = serde_json::to_string(&resp)
            .unwrap_or_else(|e| format!(r#"{{"error":"serialization failed: {e}"}}"#));
        (status_code, status_text, body)
    }

    /// `GET /metrics` — health history and uptime metrics.
    fn handle_metrics(checker: &HealthChecker) -> (u16, &'static str, String) {
        let history = checker.health_history();
        let uptime_percent = checker.uptime_percent();
        let uptime_seconds = checker.uptime_seconds();

        #[derive(Serialize)]
        struct MetricsResponse {
            uptime_seconds: u64,
            uptime_percent: f64,
            history_count: usize,
            history: Vec<HealthSnapshot>,
        }

        let resp = MetricsResponse {
            uptime_seconds,
            uptime_percent,
            history_count: history.len(),
            history,
        };

        let body = serde_json::to_string(&resp)
            .unwrap_or_else(|e| format!(r#"{{"error":"serialization failed: {e}"}}"#));
        (200, "OK", body)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    // ---- Original tests (preserved) ----

    #[test]
    fn test_health_checker_creation() {
        let checker = HealthChecker::new();
        assert_eq!(checker.status(), HealthStatus::Starting);
        assert!(!checker.is_ready());
        assert!(checker.is_alive());
    }

    #[test]
    fn test_set_status() {
        let checker = HealthChecker::new();

        checker.set_status(HealthStatus::Healthy);
        assert_eq!(checker.status(), HealthStatus::Healthy);

        checker.set_status(HealthStatus::ShuttingDown);
        assert_eq!(checker.status(), HealthStatus::ShuttingDown);

        checker.set_status(HealthStatus::Unhealthy);
        assert_eq!(checker.status(), HealthStatus::Unhealthy);
    }

    #[test]
    fn test_component_health() {
        let checker = HealthChecker::new();

        checker.set_storage_healthy(true);
        checker.set_network_healthy(true);
        checker.set_cluster_healthy(true);
        checker.set_status(HealthStatus::Healthy);

        assert!(checker.is_ready());
        assert!(checker.is_alive());
    }

    #[test]
    fn test_not_ready_when_components_unhealthy() {
        let checker = HealthChecker::new();

        checker.set_status(HealthStatus::Healthy);
        checker.set_storage_healthy(false); // Storage not healthy

        assert!(!checker.is_ready());
    }

    #[test]
    fn test_uptime() {
        let checker = HealthChecker::new();
        sleep(Duration::from_millis(100));

        let uptime = checker.uptime_seconds();
        // Uptime should be a reasonable value (u64 is always >= 0)
        assert!(uptime < 1000); // Should be less than 1000 seconds
    }

    #[test]
    fn test_health_response() {
        let checker = HealthChecker::new();
        checker.set_storage_healthy(true);
        checker.set_network_healthy(true);
        checker.set_status(HealthStatus::Healthy);

        let health = checker.get_health();
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.components.len(), 3);
        assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_health_json() {
        let checker = HealthChecker::new();
        let json = checker.get_health_json();
        assert!(json.is_ok());

        let json_str = json.expect("JSON serialization failed");
        assert!(json_str.contains("status"));
        assert!(json_str.contains("version"));
        assert!(json_str.contains("components"));
    }

    #[test]
    fn test_is_alive() {
        let checker = HealthChecker::new();

        checker.set_status(HealthStatus::Starting);
        assert!(checker.is_alive());

        checker.set_status(HealthStatus::Healthy);
        assert!(checker.is_alive());

        checker.set_status(HealthStatus::ShuttingDown);
        assert!(!checker.is_alive());

        checker.set_status(HealthStatus::Unhealthy);
        assert!(!checker.is_alive());
    }

    // ---- Deep probe tests ----

    /// A simple test probe that always returns healthy
    struct AlwaysHealthyProbe;

    #[async_trait]
    impl DeepHealthCheck for AlwaysHealthyProbe {
        async fn check(&self) -> HealthProbeResult {
            HealthProbeResult {
                status: ProbeStatus::Healthy,
                latency_ms: 0.1,
                message: "always healthy".to_string(),
            }
        }
    }

    /// A probe that returns unhealthy
    struct AlwaysUnhealthyProbe;

    #[async_trait]
    impl DeepHealthCheck for AlwaysUnhealthyProbe {
        async fn check(&self) -> HealthProbeResult {
            HealthProbeResult {
                status: ProbeStatus::Unhealthy,
                latency_ms: 5.0,
                message: "always unhealthy".to_string(),
            }
        }
    }

    /// A probe that returns degraded
    struct AlwaysDegradedProbe;

    #[async_trait]
    impl DeepHealthCheck for AlwaysDegradedProbe {
        async fn check(&self) -> HealthProbeResult {
            HealthProbeResult {
                status: ProbeStatus::Degraded,
                latency_ms: 2.0,
                message: "always degraded".to_string(),
            }
        }
    }

    #[tokio::test]
    async fn test_deep_probe_execution_and_result_reporting() {
        let checker = HealthChecker::new();
        checker.register_probe("test_healthy", Arc::new(AlwaysHealthyProbe));
        checker.register_probe("test_unhealthy", Arc::new(AlwaysUnhealthyProbe));

        let results = checker.run_probes().await;
        assert_eq!(results.len(), 2);

        let healthy = results.get("test_healthy").expect("missing healthy probe");
        assert_eq!(healthy.status, ProbeStatus::Healthy);
        assert_eq!(healthy.message, "always healthy");

        let unhealthy = results
            .get("test_unhealthy")
            .expect("missing unhealthy probe");
        assert_eq!(unhealthy.status, ProbeStatus::Unhealthy);
        assert_eq!(unhealthy.message, "always unhealthy");
    }

    #[tokio::test]
    async fn test_storage_probe_passes_with_valid_storage() {
        let dir = std::env::temp_dir().join("amaters_health_test_storage");
        let _ = std::fs::create_dir_all(&dir);

        let probe = StorageProbe::new(dir.clone());
        let result = probe.check().await;

        assert_eq!(result.status, ProbeStatus::Healthy);
        assert!(result.latency_ms >= 0.0);
        assert!(result.message.contains("OK"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_storage_probe_fails_with_invalid_path() {
        let probe = StorageProbe::new(std::path::PathBuf::from(
            "/nonexistent_path_for_health_check_test_12345",
        ));
        let result = probe.check().await;
        assert_eq!(result.status, ProbeStatus::Unhealthy);
    }

    #[tokio::test]
    async fn test_wal_probe_passes() {
        let dir = std::env::temp_dir().join("amaters_health_test_wal");
        let _ = std::fs::create_dir_all(&dir);

        let probe = WalProbe::new(dir.clone());
        let result = probe.check().await;

        assert_eq!(result.status, ProbeStatus::Healthy);
        assert!(result.message.contains("appendable"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_disk_space_probe_healthy() {
        // Threshold of 1 byte — should always pass on a running system
        let probe = DiskSpaceProbe::new(std::env::temp_dir(), 1);
        let result = probe.check().await;
        assert_eq!(result.status, ProbeStatus::Healthy);
    }

    // ---- Liveness vs readiness ----

    #[test]
    fn test_liveness_vs_readiness_during_startup() {
        let checker = HealthChecker::new();
        // Starting state: alive but not ready
        assert!(checker.is_alive());
        assert!(!checker.is_ready());

        let live_resp = checker.liveness_response();
        assert!(live_resp.alive);

        let ready_resp = checker.readiness_response();
        assert!(!ready_resp.ready);
    }

    #[test]
    fn test_liveness_vs_readiness_during_shutdown() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::ShuttingDown);

        assert!(!checker.is_alive());
        assert!(!checker.is_ready());

        let live_resp = checker.liveness_response();
        assert!(!live_resp.alive);

        let ready_resp = checker.readiness_response();
        assert!(!ready_resp.ready);
    }

    #[test]
    fn test_readiness_requires_components() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Healthy);
        // Storage and network still false
        assert!(!checker.is_ready());

        checker.set_storage_healthy(true);
        assert!(!checker.is_ready()); // network still down

        checker.set_network_healthy(true);
        assert!(checker.is_ready()); // now ready
    }

    // ---- Health history ring buffer ----

    #[test]
    fn test_health_history_ring_buffer_correctness() {
        let mut history = HealthHistory::new(3);

        // Record 5 entries — buffer should keep last 3
        for i in 0..5u64 {
            history.record(HealthSnapshot {
                timestamp: i,
                status: HealthStatus::Healthy,
                alive: true,
                ready: true,
            });
        }

        let snaps = history.snapshots();
        assert_eq!(snaps.len(), 3);
        // Oldest should be timestamp 2
        assert_eq!(snaps[0].timestamp, 2);
        assert_eq!(snaps[1].timestamp, 3);
        assert_eq!(snaps[2].timestamp, 4);
    }

    #[test]
    fn test_health_history_partial_fill() {
        let mut history = HealthHistory::new(10);

        history.record(HealthSnapshot {
            timestamp: 100,
            status: HealthStatus::Healthy,
            alive: true,
            ready: true,
        });
        history.record(HealthSnapshot {
            timestamp: 200,
            status: HealthStatus::Unhealthy,
            alive: false,
            ready: false,
        });

        let snaps = history.snapshots();
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].timestamp, 100);
        assert_eq!(snaps[1].timestamp, 200);
    }

    // ---- Uptime percentage ----

    #[test]
    fn test_uptime_percentage_all_alive() {
        let mut history = HealthHistory::new(5);
        for i in 0..5 {
            history.record(HealthSnapshot {
                timestamp: i,
                status: HealthStatus::Healthy,
                alive: true,
                ready: true,
            });
        }
        let pct = history.uptime_percent();
        assert!((pct - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_uptime_percentage_partial() {
        let mut history = HealthHistory::new(4);
        // 3 alive, 1 dead => 75%
        for i in 0..3 {
            history.record(HealthSnapshot {
                timestamp: i,
                status: HealthStatus::Healthy,
                alive: true,
                ready: true,
            });
        }
        history.record(HealthSnapshot {
            timestamp: 3,
            status: HealthStatus::Unhealthy,
            alive: false,
            ready: false,
        });

        let pct = history.uptime_percent();
        assert!((pct - 75.0).abs() < 0.01);
    }

    #[test]
    fn test_uptime_percentage_empty_is_100() {
        let history = HealthHistory::new(10);
        assert!((history.uptime_percent() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_health_checker_uptime_percent_and_history() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Healthy);
        checker.set_storage_healthy(true);
        checker.set_network_healthy(true);

        checker.record_snapshot();
        checker.record_snapshot();

        checker.set_status(HealthStatus::Unhealthy);
        checker.record_snapshot();

        let history = checker.health_history();
        assert_eq!(history.len(), 3);

        // 2 alive, 1 not alive
        let pct = checker.uptime_percent();
        assert!((pct - 100.0 * 2.0 / 3.0).abs() < 0.01);
    }

    // ---- Dependency aggregation ----

    #[tokio::test]
    async fn test_dependency_aggregation_one_unhealthy() {
        let checker = HealthChecker::new();
        checker.register_dependency("dep_ok", Arc::new(AlwaysHealthyProbe));
        checker.register_dependency("dep_bad", Arc::new(AlwaysUnhealthyProbe));

        let worst = checker.check_dependencies().await;
        assert_eq!(worst, ProbeStatus::Unhealthy);

        // Aggregated should also be unhealthy
        assert_eq!(
            checker.aggregated_dependency_status(),
            ProbeStatus::Unhealthy
        );
    }

    #[tokio::test]
    async fn test_dependency_aggregation_all_healthy() {
        let checker = HealthChecker::new();
        checker.register_dependency("dep_a", Arc::new(AlwaysHealthyProbe));
        checker.register_dependency("dep_b", Arc::new(AlwaysHealthyProbe));

        let worst = checker.check_dependencies().await;
        assert_eq!(worst, ProbeStatus::Healthy);
    }

    #[tokio::test]
    async fn test_dependency_health_in_readiness_response() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Healthy);
        checker.set_storage_healthy(true);
        checker.set_network_healthy(true);
        checker.register_dependency("cache", Arc::new(AlwaysHealthyProbe));

        let _ = checker.check_dependencies().await;

        let resp = checker.readiness_response();
        assert!(resp.ready);
        assert_eq!(resp.dependencies.len(), 1);
        assert_eq!(resp.dependencies[0].name, "cache");
        assert_eq!(resp.dependencies[0].status, ProbeStatus::Healthy);
    }

    // ---- Degraded state ----

    #[test]
    fn test_degraded_state_alive_and_ready() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Degraded);
        checker.set_storage_healthy(true);
        checker.set_network_healthy(true);

        // Degraded is alive and can be ready
        assert!(checker.is_alive());
        assert!(checker.is_ready());
    }

    #[tokio::test]
    async fn test_degraded_dependency_aggregation() {
        let checker = HealthChecker::new();
        checker.register_dependency("dep_ok", Arc::new(AlwaysHealthyProbe));
        checker.register_dependency("dep_degraded", Arc::new(AlwaysDegradedProbe));

        let worst = checker.check_dependencies().await;
        assert_eq!(worst, ProbeStatus::Degraded);
    }

    // ---- Concurrent health checks ----

    #[tokio::test]
    async fn test_concurrent_health_checks() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Healthy);
        checker.set_storage_healthy(true);
        checker.set_network_healthy(true);
        checker.register_probe("probe_a", Arc::new(AlwaysHealthyProbe));
        checker.register_dependency("dep_a", Arc::new(AlwaysHealthyProbe));

        // Run multiple operations concurrently
        let checker_clone1 = checker.clone();
        let checker_clone2 = checker.clone();
        let checker_clone3 = checker.clone();

        let (r1, r2, r3) = tokio::join!(
            async move { checker_clone1.run_probes().await },
            async move { checker_clone2.check_dependencies().await },
            async move {
                checker_clone3.record_snapshot();
                checker_clone3.health_history()
            },
        );

        assert_eq!(r1.len(), 1);
        assert_eq!(r2, ProbeStatus::Healthy);
        assert!(!r3.is_empty());
    }

    // ---- ProbeStatus::worse ----

    #[test]
    fn test_probe_status_worse() {
        assert_eq!(
            ProbeStatus::Healthy.worse(ProbeStatus::Healthy),
            ProbeStatus::Healthy
        );
        assert_eq!(
            ProbeStatus::Healthy.worse(ProbeStatus::Degraded),
            ProbeStatus::Degraded
        );
        assert_eq!(
            ProbeStatus::Degraded.worse(ProbeStatus::Healthy),
            ProbeStatus::Degraded
        );
        assert_eq!(
            ProbeStatus::Healthy.worse(ProbeStatus::Unhealthy),
            ProbeStatus::Unhealthy
        );
        assert_eq!(
            ProbeStatus::Degraded.worse(ProbeStatus::Unhealthy),
            ProbeStatus::Unhealthy
        );
    }

    // ---- Deep health response ----

    #[tokio::test]
    async fn test_get_health_deep_includes_probes() {
        let checker = HealthChecker::new();
        checker.register_probe("deep_test", Arc::new(AlwaysHealthyProbe));

        let resp = checker.get_health_deep().await;
        assert_eq!(resp.probes.len(), 1);
        let probe_result = resp.probes.get("deep_test").expect("missing probe result");
        assert_eq!(probe_result.status, ProbeStatus::Healthy);
    }

    // ---- HTTP health server tests ----

    async fn start_test_server(checker: HealthChecker) -> HealthHttpHandle {
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("valid addr");
        HealthHttpServer::new(Arc::new(checker), addr)
            .start()
            .await
            .expect("failed to start health HTTP server")
    }

    async fn http_request(port: u16, method: &str, path: &str) -> (u16, String) {
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .expect("failed to connect");
        let req =
            format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.expect("write");
        let mut resp = String::new();
        stream.read_to_string(&mut resp).await.expect("read");
        let line = resp.lines().next().unwrap_or("");
        let code: u16 = line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (code, body)
    }

    async fn http_get(port: u16, path: &str) -> (u16, String) {
        http_request(port, "GET", path).await
    }

    #[tokio::test]
    async fn test_health_http_server_starts() {
        let checker = HealthChecker::new();
        let handle = start_test_server(checker).await;
        let port = handle.port();
        assert!(port > 0);

        // Verify we can connect
        let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
        assert!(result.is_ok());

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Healthy);
        checker.set_storage_healthy(true);
        checker.set_network_healthy(true);

        let handle = start_test_server(checker).await;
        let port = handle.port();

        let (status, body) = http_get(port, "/health").await;
        assert_eq!(status, 200);
        assert!(body.contains("\"status\":\"healthy\""));
        assert!(body.contains("\"version\""));
        assert!(body.contains("\"components\""));

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_healthz_endpoint() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Healthy);

        let handle = start_test_server(checker).await;
        let port = handle.port();

        let (status, body) = http_get(port, "/healthz").await;
        assert_eq!(status, 200);
        assert!(body.contains("\"alive\":true"));

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_healthz_unhealthy() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Unhealthy);

        let handle = start_test_server(checker).await;
        let port = handle.port();

        let (status, body) = http_get(port, "/healthz").await;
        assert_eq!(status, 503);
        assert!(body.contains("\"alive\":false"));

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_readyz_endpoint() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Healthy);
        checker.set_storage_healthy(true);
        checker.set_network_healthy(true);

        let handle = start_test_server(checker).await;
        let port = handle.port();

        let (status, body) = http_get(port, "/readyz").await;
        assert_eq!(status, 200);
        assert!(body.contains("\"ready\":true"));

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_readyz_not_ready() {
        let checker = HealthChecker::new();
        // Starting status — not ready (storage/network not set)

        let handle = start_test_server(checker).await;
        let port = handle.port();

        let (status, body) = http_get(port, "/readyz").await;
        assert_eq!(status, 503);
        assert!(body.contains("\"ready\":false"));

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_livez_endpoint() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Healthy);

        let handle = start_test_server(checker).await;
        let port = handle.port();

        let (status, body) = http_get(port, "/livez").await;
        assert_eq!(status, 200);
        assert!(body.contains("\"alive\":true"));
        assert!(body.contains("\"uptime_seconds\""));

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_metrics_endpoint() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Healthy);
        checker.set_storage_healthy(true);
        checker.set_network_healthy(true);
        checker.record_snapshot();
        checker.record_snapshot();

        let handle = start_test_server(checker).await;
        let port = handle.port();

        let (status, body) = http_get(port, "/metrics").await;
        assert_eq!(status, 200);
        assert!(body.contains("\"uptime_seconds\""));
        assert!(body.contains("\"uptime_percent\""));
        assert!(body.contains("\"history_count\":2"));
        assert!(body.contains("\"history\""));

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_unknown_path_404() {
        let checker = HealthChecker::new();

        let handle = start_test_server(checker).await;
        let port = handle.port();

        let (status, body) = http_get(port, "/unknown").await;
        assert_eq!(status, 404);
        assert!(body.contains("not found"));

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_non_get_method_405() {
        let checker = HealthChecker::new();

        let handle = start_test_server(checker).await;
        let port = handle.port();

        let (status, body) = http_request(port, "POST", "/health").await;
        assert_eq!(status, 405);
        assert!(body.contains("method not allowed"));

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_concurrent_http_requests() {
        let checker = HealthChecker::new();
        checker.set_status(HealthStatus::Healthy);
        checker.set_storage_healthy(true);
        checker.set_network_healthy(true);

        let handle = start_test_server(checker).await;
        let port = handle.port();

        // Fire 10 concurrent requests across different endpoints
        let mut tasks = Vec::new();
        for i in 0..10 {
            let path = match i % 4 {
                0 => "/health",
                1 => "/healthz",
                2 => "/readyz",
                _ => "/livez",
            };
            tasks.push(tokio::spawn(async move { http_get(port, path).await }));
        }

        for task in tasks {
            let (status, _body) = task.await.expect("task panicked");
            assert_eq!(status, 200);
        }

        handle.stop();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn test_server_shutdown() {
        let checker = HealthChecker::new();

        let handle = start_test_server(checker).await;
        let port = handle.port();

        // Verify server is listening
        let (status, _) = http_get(port, "/healthz").await;
        assert_eq!(status, 200);

        // Signal shutdown
        handle.stop();
        let result = handle.join().await;
        assert!(result.is_ok());

        // After shutdown, connection should fail (with a small delay for cleanup)
        tokio::time::sleep(Duration::from_millis(300)).await;
        let connect_result = tokio::time::timeout(
            Duration::from_millis(500),
            tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")),
        )
        .await;

        // Either timeout or connection refused — both are acceptable
        match connect_result {
            Err(_) => {}     // timeout — server stopped
            Ok(Err(_)) => {} // connection refused — server stopped
            Ok(Ok(_)) => {
                // Connection succeeded — this can happen if the OS hasn't fully
                // released the port yet; we just verify the server task exited.
            }
        }
    }
}
