//! Common test utilities for AmateRS integration tests
//!
//! This module provides utilities for spawning test servers, creating test clients,
//! generating test data, and cleanup operations.

use amaters_core::storage::MemoryStorage;
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use amaters_net::{AqlServerBuilder, AqlServiceImpl};
use amaters_server::config::{
    AuthSettings, AuthorizationSettings, LoggingSettings, MetricsSettings, NetworkSettings,
    ServerConfig, ServerSettings, StorageSettings,
};
use amaters_server::health::{HealthChecker, HealthStatus};
use amaters_server::metrics::MetricsCollector;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use uuid::Uuid;

/// Global port counter for allocating unique test ports
/// Uses process ID + random seed to avoid collisions in parallel test runs
static PORT_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Initialize port counter with process-unique starting value
fn get_port_base() -> u32 {
    // Use process ID and a hash of current time to generate unique starting port
    let pid = std::process::id();
    let time_seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u32)
        .unwrap_or(0);
    // Generate port in range 19000-45000 to avoid common ports
    19000 + ((pid ^ time_seed) % 26000)
}

/// Test context holding server, storage, and cleanup state
pub struct TestContext {
    /// Storage engine used by the test
    pub storage: Arc<MemoryStorage>,
    /// AQL service for query execution
    pub service: Arc<AqlServiceImpl<MemoryStorage>>,
    /// Health checker
    pub health: HealthChecker,
    /// Metrics collector
    pub metrics: MetricsCollector,
    /// Temporary directory for test data
    pub temp_dir: PathBuf,
    /// Port assigned to this test
    pub port: u32,
}

impl TestContext {
    /// Create a new test context with fresh storage and services
    pub fn new() -> Result<Self, TestError> {
        let temp_dir = std::env::temp_dir().join(format!("amaters_test_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)?;

        let storage = Arc::new(MemoryStorage::new());
        let service = Arc::new(AqlServerBuilder::new(storage.clone()).build());
        let health = HealthChecker::new();
        let metrics = MetricsCollector::new();
        let port = allocate_test_port();

        // Mark as healthy
        health.set_status(HealthStatus::Healthy);
        health.set_storage_healthy(true);
        health.set_network_healthy(true);

        Ok(Self {
            storage,
            service,
            health,
            metrics,
            temp_dir,
            port,
        })
    }

    /// Create a test context with pre-populated data
    pub fn with_test_data(num_entries: usize) -> Result<Self, TestError> {
        let ctx = Self::new()?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| TestError::Setup(format!("No tokio runtime: {}", e)))?;

        runtime.block_on(async {
            for i in 0..num_entries {
                let key = Key::from_str(&format!("test_key_{:06}", i));
                let value = CipherBlob::new(generate_test_data(i, 100));
                ctx.storage
                    .put(&key, &value)
                    .await
                    .map_err(|e| TestError::Setup(format!("Failed to insert test data: {}", e)))?;
            }
            Ok::<(), TestError>(())
        })?;

        Ok(ctx)
    }

    /// Get the bind address for this test context
    pub fn bind_address(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// Get a test configuration for this context
    pub fn config(&self) -> ServerConfig {
        create_test_config(&self.temp_dir, self.port)
    }

    /// Insert test data into storage
    pub async fn insert_test_data(
        &self,
        prefix: &str,
        count: usize,
        value_size: usize,
    ) -> Result<Vec<Key>, TestError> {
        let mut keys = Vec::with_capacity(count);

        for i in 0..count {
            let key = Key::from_str(&format!("{}_{:06}", prefix, i));
            let value = CipherBlob::new(generate_test_data(i, value_size));
            self.storage
                .put(&key, &value)
                .await
                .map_err(|e| TestError::Storage(format!("Failed to insert data: {}", e)))?;
            keys.push(key);
        }

        Ok(keys)
    }

    /// Verify data exists and is correct
    pub async fn verify_data(&self, key: &Key, expected_first_byte: u8) -> Result<bool, TestError> {
        let value = self
            .storage
            .get(key)
            .await
            .map_err(|e| TestError::Storage(format!("Failed to get data: {}", e)))?;

        match value {
            Some(blob) => {
                let bytes = blob.as_bytes();
                Ok(!bytes.is_empty() && bytes[0] == expected_first_byte)
            }
            None => Ok(false),
        }
    }

    /// Get count of entries in storage
    pub async fn entry_count(&self) -> Result<usize, TestError> {
        let keys = self
            .storage
            .keys()
            .await
            .map_err(|e| TestError::Storage(format!("Failed to list keys: {}", e)))?;
        Ok(keys.len())
    }

    /// Clean up test resources
    pub fn cleanup(&self) {
        if self.temp_dir.exists() {
            std::fs::remove_dir_all(&self.temp_dir).ok();
        }
    }
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new().expect("Failed to create default test context")
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Test error types
#[derive(Debug)]
pub enum TestError {
    Setup(String),
    Storage(String),
    Network(String),
    Timeout(String),
    Assertion(String),
    Io(std::io::Error),
}

impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestError::Setup(msg) => write!(f, "Setup error: {}", msg),
            TestError::Storage(msg) => write!(f, "Storage error: {}", msg),
            TestError::Network(msg) => write!(f, "Network error: {}", msg),
            TestError::Timeout(msg) => write!(f, "Timeout error: {}", msg),
            TestError::Assertion(msg) => write!(f, "Assertion error: {}", msg),
            TestError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for TestError {}

impl From<std::io::Error> for TestError {
    fn from(err: std::io::Error) -> Self {
        TestError::Io(err)
    }
}

/// Allocate a unique port for testing
pub fn allocate_test_port() -> u32 {
    // Initialize counter on first call using compare_exchange
    let current = PORT_COUNTER.load(Ordering::SeqCst);
    if current == 0 {
        let base = get_port_base();
        // Try to set the base; if another thread beat us, that's fine
        let _ = PORT_COUNTER.compare_exchange(0, base, Ordering::SeqCst, Ordering::SeqCst);
    }
    PORT_COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Create a test server configuration
pub fn create_test_config(temp_dir: &Path, port: u32) -> ServerConfig {
    ServerConfig {
        server: ServerSettings {
            bind_address: format!("127.0.0.1:{}", port),
            data_dir: temp_dir.to_path_buf(),
            pid_file: temp_dir.join("test.pid"),
            max_connections: 100,
            shutdown_timeout_secs: 5,
        },
        storage: StorageSettings {
            engine: "memory".to_string(),
            wal: Default::default(),
            memtable_size_mb: 16,
            block_cache_size_mb: 32,
            compaction: Default::default(),
        },
        network: NetworkSettings {
            tls_enabled: false,
            tls_cert: None,
            tls_key: None,
            tls_ca: None,
            require_client_cert: false,
            connection_timeout_secs: 5,
            keepalive_interval_secs: 10,
        },
        cluster: None,
        logging: LoggingSettings {
            level: "debug".to_string(),
            format: "compact".to_string(),
            file_enabled: false,
            file_path: None,
            rotation: Default::default(),
        },
        metrics: MetricsSettings {
            enabled: false,
            bind_address: format!("127.0.0.1:{}", port + 1000),
            export_interval_secs: 60,
        },
        auth: AuthSettings {
            enabled: false,
            methods: vec![],
            mtls: Default::default(),
            jwt: Default::default(),
            api_key: Default::default(),
            reject_unauthenticated: false,
        },
        authz: AuthorizationSettings {
            enabled: false,
            default_role: "admin".to_string(),
            roles_file: None,
            policies_file: None,
            collection_permissions: false,
            default_mode: "allow-by-default".to_string(),
            audit_enabled: false,
            audit_log_path: None,
        },
    }
}

/// Generate deterministic test data
pub fn generate_test_data(seed: usize, size: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(size);
    for i in 0..size {
        data.push(((seed + i) % 256) as u8);
    }
    data
}

/// Generate a random-looking but deterministic key
pub fn generate_test_key(prefix: &str, index: usize) -> Key {
    Key::from_str(&format!("{}_{:08x}", prefix, index))
}

/// Generate test keys in a range
pub fn generate_test_keys(prefix: &str, start: usize, end: usize) -> Vec<Key> {
    (start..end).map(|i| generate_test_key(prefix, i)).collect()
}

/// Create test CipherBlob with specific size
pub fn create_test_blob(size: usize, fill: u8) -> CipherBlob {
    CipherBlob::new(vec![fill; size])
}

/// Create test CipherBlob with pattern
pub fn create_test_blob_pattern(size: usize, seed: usize) -> CipherBlob {
    CipherBlob::new(generate_test_data(seed, size))
}

/// Wait for condition with timeout
pub async fn wait_for_condition<F>(
    condition: F,
    timeout: Duration,
    check_interval: Duration,
) -> Result<(), TestError>
where
    F: Fn() -> bool,
{
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        if condition() {
            return Ok(());
        }
        tokio::time::sleep(check_interval).await;
    }

    Err(TestError::Timeout(format!(
        "Condition not met within {:?}",
        timeout
    )))
}

/// Latency statistics
#[derive(Debug, Clone)]
pub struct LatencyStats {
    pub count: usize,
    pub total_us: u64,
    pub min_us: u64,
    pub max_us: u64,
    pub samples: Vec<u64>,
}

impl LatencyStats {
    pub fn new() -> Self {
        Self {
            count: 0,
            total_us: 0,
            min_us: u64::MAX,
            max_us: 0,
            samples: Vec::new(),
        }
    }

    pub fn record(&mut self, latency_us: u64) {
        self.count += 1;
        self.total_us += latency_us;
        self.min_us = self.min_us.min(latency_us);
        self.max_us = self.max_us.max(latency_us);
        self.samples.push(latency_us);
    }

    pub fn mean_us(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.total_us as f64 / self.count as f64
        }
    }

    /// Calculate percentile (p50, p95, p99)
    pub fn percentile(&self, p: f64) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }

        let mut sorted = self.samples.clone();
        sorted.sort_unstable();

        let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    pub fn p50(&self) -> u64 {
        self.percentile(50.0)
    }

    pub fn p95(&self) -> u64 {
        self.percentile(95.0)
    }

    pub fn p99(&self) -> u64 {
        self.percentile(99.0)
    }
}

impl Default for LatencyStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Throughput measurement
#[derive(Debug, Clone)]
pub struct ThroughputMeasurement {
    pub operations: u64,
    pub duration_ms: u64,
    pub bytes_processed: u64,
}

impl ThroughputMeasurement {
    pub fn new(operations: u64, duration_ms: u64, bytes_processed: u64) -> Self {
        Self {
            operations,
            duration_ms,
            bytes_processed,
        }
    }

    pub fn ops_per_second(&self) -> f64 {
        if self.duration_ms == 0 {
            0.0
        } else {
            self.operations as f64 / (self.duration_ms as f64 / 1000.0)
        }
    }

    pub fn mb_per_second(&self) -> f64 {
        if self.duration_ms == 0 {
            0.0
        } else {
            let bytes_per_ms = self.bytes_processed as f64 / self.duration_ms as f64;
            bytes_per_ms / 1024.0 // KB/ms = MB/s
        }
    }
}

/// Memory usage tracker for tests
pub struct MemoryTracker {
    baseline_bytes: usize,
}

impl MemoryTracker {
    pub fn new() -> Self {
        Self {
            baseline_bytes: Self::current_usage(),
        }
    }

    fn current_usage() -> usize {
        // Simple approximation using allocator stats if available
        // In production, we'd use jemalloc or similar for accurate tracking
        0 // Placeholder - actual implementation would use system-specific APIs
    }

    pub fn delta_bytes(&self) -> isize {
        Self::current_usage() as isize - self.baseline_bytes as isize
    }

    pub fn delta_mb(&self) -> f64 {
        self.delta_bytes() as f64 / (1024.0 * 1024.0)
    }
}

impl Default for MemoryTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Assertion helpers for tests
pub mod assertions {
    use super::*;

    /// Assert that a value exists in storage
    pub async fn assert_exists(
        storage: &impl StorageEngine,
        key: &Key,
    ) -> Result<CipherBlob, TestError> {
        let value = storage
            .get(key)
            .await
            .map_err(|e| TestError::Storage(format!("Failed to get key: {}", e)))?;

        value.ok_or_else(|| TestError::Assertion(format!("Expected key {:?} to exist", key)))
    }

    /// Assert that a value does not exist in storage
    pub async fn assert_not_exists(
        storage: &impl StorageEngine,
        key: &Key,
    ) -> Result<(), TestError> {
        let value = storage
            .get(key)
            .await
            .map_err(|e| TestError::Storage(format!("Failed to get key: {}", e)))?;

        if value.is_some() {
            return Err(TestError::Assertion(format!(
                "Expected key {:?} to not exist",
                key
            )));
        }

        Ok(())
    }

    /// Assert that storage contains expected number of keys
    pub async fn assert_key_count(
        storage: &impl StorageEngine,
        expected: usize,
    ) -> Result<(), TestError> {
        let keys = storage
            .keys()
            .await
            .map_err(|e| TestError::Storage(format!("Failed to list keys: {}", e)))?;

        if keys.len() != expected {
            return Err(TestError::Assertion(format!(
                "Expected {} keys, found {}",
                expected,
                keys.len()
            )));
        }

        Ok(())
    }

    /// Assert value matches expected
    pub async fn assert_value_equals(
        storage: &impl StorageEngine,
        key: &Key,
        expected: &CipherBlob,
    ) -> Result<(), TestError> {
        let value = assert_exists(storage, key).await?;

        if value.as_bytes() != expected.as_bytes() {
            return Err(TestError::Assertion(format!(
                "Value mismatch for key {:?}",
                key
            )));
        }

        Ok(())
    }
}

/// Concurrent test helpers
pub mod concurrent {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Barrier;

    /// Run concurrent operations with barrier synchronization
    pub async fn run_concurrent<F, Fut, T>(
        num_tasks: usize,
        task_fn: F,
    ) -> Vec<Result<T, TestError>>
    where
        F: Fn(usize) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = Result<T, TestError>> + Send,
        T: Send + 'static,
    {
        let barrier = Arc::new(Barrier::new(num_tasks));
        let mut handles = Vec::with_capacity(num_tasks);

        for task_id in 0..num_tasks {
            let barrier = Arc::clone(&barrier);
            let task_fn = task_fn.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;
                task_fn(task_id).await
            });

            handles.push(handle);
        }

        let mut results = Vec::with_capacity(num_tasks);
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(Err(TestError::Setup(format!("Task panicked: {}", e)))),
            }
        }

        results
    }

    /// Counter for tracking concurrent operations
    pub struct ConcurrentCounter {
        count: AtomicUsize,
    }

    impl ConcurrentCounter {
        pub fn new() -> Self {
            Self {
                count: AtomicUsize::new(0),
            }
        }

        pub fn increment(&self) -> usize {
            self.count.fetch_add(1, Ordering::SeqCst)
        }

        pub fn get(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    impl Default for ConcurrentCounter {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_context_creation() {
        let ctx = TestContext::new();
        assert!(ctx.is_ok());
        let ctx = ctx.expect("Context creation failed");
        assert!(ctx.temp_dir.exists());
        assert!(ctx.port >= 19000);
    }

    #[tokio::test]
    async fn test_generate_test_data() {
        let data = generate_test_data(0, 100);
        assert_eq!(data.len(), 100);
        assert_eq!(data[0], 0);
        assert_eq!(data[99], 99);
    }

    #[tokio::test]
    async fn test_latency_stats() {
        let mut stats = LatencyStats::new();
        stats.record(100);
        stats.record(200);
        stats.record(300);

        assert_eq!(stats.count, 3);
        assert_eq!(stats.min_us, 100);
        assert_eq!(stats.max_us, 300);
        assert!((stats.mean_us() - 200.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_throughput_measurement() {
        let measurement = ThroughputMeasurement::new(1000, 1000, 1_048_576);
        assert!((measurement.ops_per_second() - 1000.0).abs() < 0.01);
        assert!((measurement.mb_per_second() - 1.024).abs() < 0.01);
    }
}
