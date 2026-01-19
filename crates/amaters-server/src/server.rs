//! Server runtime module
//!
//! This module integrates all server components:
//! - Storage engine (amaters-core)
//! - Network layer (amaters-net)
//! - Consensus (amaters-cluster)
//! - Health checking
//! - Metrics collection

use crate::config::{ConfigResult, ServerConfig};
use crate::health::{HealthChecker, HealthStatus};
use crate::metrics::MetricsCollector;
use crate::service::NetworkService;
use crate::shutdown::ShutdownCoordinator;
use amaters_core::error::Result as CoreResult;
use amaters_core::storage::{
    BlockCacheConfig, CompactionConfig, LsmTreeConfig, LsmTreeStorage, MemoryStorage,
    MemtableConfig, SSTableConfig,
};
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use async_trait::async_trait;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::time::sleep;
use tracing::{error, info, warn};

/// Storage engine wrapper enum to support multiple storage types
#[derive(Clone)]
pub enum Storage {
    Memory(MemoryStorage),
    Lsm(LsmTreeStorage),
}

#[async_trait]
impl StorageEngine for Storage {
    async fn put(&self, key: &Key, value: &CipherBlob) -> CoreResult<()> {
        match self {
            Storage::Memory(s) => s.put(key, value).await,
            Storage::Lsm(s) => s.put(key, value).await,
        }
    }

    async fn get(&self, key: &Key) -> CoreResult<Option<CipherBlob>> {
        match self {
            Storage::Memory(s) => s.get(key).await,
            Storage::Lsm(s) => s.get(key).await,
        }
    }

    async fn atomic_update<F>(&self, key: &Key, f: F) -> CoreResult<()>
    where
        F: Fn(&CipherBlob) -> CoreResult<CipherBlob> + Send + Sync,
    {
        match self {
            Storage::Memory(s) => s.atomic_update(key, f).await,
            Storage::Lsm(s) => s.atomic_update(key, f).await,
        }
    }

    async fn delete(&self, key: &Key) -> CoreResult<()> {
        match self {
            Storage::Memory(s) => s.delete(key).await,
            Storage::Lsm(s) => s.delete(key).await,
        }
    }

    async fn range(&self, start: &Key, end: &Key) -> CoreResult<Vec<(Key, CipherBlob)>> {
        match self {
            Storage::Memory(s) => s.range(start, end).await,
            Storage::Lsm(s) => s.range(start, end).await,
        }
    }

    async fn keys(&self) -> CoreResult<Vec<Key>> {
        match self {
            Storage::Memory(s) => s.keys().await,
            Storage::Lsm(s) => s.keys().await,
        }
    }

    async fn flush(&self) -> CoreResult<()> {
        match self {
            Storage::Memory(s) => s.flush().await,
            Storage::Lsm(s) => s.flush().await,
        }
    }

    async fn close(&self) -> CoreResult<()> {
        match self {
            Storage::Memory(s) => s.close().await,
            Storage::Lsm(s) => s.close().await,
        }
    }
}

/// Server errors
#[derive(Error, Debug)]
pub enum ServerError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Configuration validation error: {0}")]
    ConfigValidation(String),

    #[error("Storage initialization failed: {0}")]
    Storage(String),

    #[error("Network initialization failed: {0}")]
    Network(String),

    #[error("Cluster initialization failed: {0}")]
    Cluster(String),

    #[error("TLS setup failed: {0}")]
    TlsSetup(String),

    #[error("Server already running")]
    AlreadyRunning,

    #[error("Failed to create directory: {0}")]
    DirectoryCreation(#[from] std::io::Error),

    #[error("Shutdown timeout")]
    ShutdownTimeout,

    #[error("Core error: {0}")]
    Core(#[from] amaters_core::error::AmateRSError),
}

pub type ServerResult<T> = Result<T, ServerError>;

/// Main server runtime
pub struct Server {
    /// Server configuration
    config: Arc<ServerConfig>,
    /// Storage engine (supports memory or LSM)
    storage: Option<Arc<Storage>>,
    /// Network service (AQL API)
    network: Option<NetworkService>,
    /// Shutdown coordinator
    shutdown: ShutdownCoordinator,
    /// Health checker
    health: HealthChecker,
    /// Metrics collector
    metrics: MetricsCollector,
}

impl Server {
    /// Create a new server with the given configuration
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config: Arc::new(config),
            storage: None,
            network: None,
            shutdown: ShutdownCoordinator::new(),
            health: HealthChecker::new(),
            metrics: MetricsCollector::new(),
        }
    }

    /// Initialize server components
    pub async fn initialize(&mut self) -> ServerResult<()> {
        info!("Initializing server components");

        // Create data directory if it doesn't exist
        self.ensure_data_directory()?;

        // Initialize storage
        self.initialize_storage().await?;

        // Initialize network service
        self.initialize_network().await?;

        // Initialize health checker
        self.health.set_status(HealthStatus::Starting);

        info!("Server components initialized successfully");
        Ok(())
    }

    /// Ensure data directory exists
    fn ensure_data_directory(&self) -> ServerResult<()> {
        let data_dir = &self.config.server.data_dir;
        if !data_dir.exists() {
            info!("Creating data directory: {}", data_dir.display());
            fs::create_dir_all(data_dir)?;
        }
        Ok(())
    }

    /// Initialize storage engine
    async fn initialize_storage(&mut self) -> ServerResult<()> {
        info!(
            "Initializing storage engine: {}",
            self.config.storage.engine
        );

        let storage = match self.config.storage.engine.as_str() {
            "memory" => {
                info!("Using in-memory storage engine");
                Storage::Memory(MemoryStorage::new())
            }
            "lsm" => {
                info!("Using LSM-Tree storage engine");
                let lsm_config = self.build_lsm_config()?;
                let lsm_storage = LsmTreeStorage::with_config(lsm_config).map_err(|e| {
                    ServerError::Storage(format!("Failed to create LSM storage: {}", e))
                })?;
                Storage::Lsm(lsm_storage)
            }
            other => {
                return Err(ServerError::Config(format!(
                    "Invalid storage engine: {}. Supported: memory, lsm",
                    other
                )));
            }
        };

        self.storage = Some(Arc::new(storage));
        self.health.set_storage_healthy(true);

        info!("Storage engine initialized successfully");
        Ok(())
    }

    /// Build LSM-Tree configuration from server config
    fn build_lsm_config(&self) -> ServerResult<LsmTreeConfig> {
        let data_dir = self.config.server.data_dir.join("lsm");
        let wal_dir = self
            .config
            .server
            .data_dir
            .join(self.config.storage.wal.dir.clone());

        // Create directories
        std::fs::create_dir_all(&data_dir).map_err(|e| {
            ServerError::Storage(format!("Failed to create LSM data directory: {}", e))
        })?;
        std::fs::create_dir_all(&wal_dir)
            .map_err(|e| ServerError::Storage(format!("Failed to create WAL directory: {}", e)))?;

        let memtable_config = MemtableConfig {
            max_size_bytes: self.config.storage.memtable_size_mb * 1024 * 1024,
            enable_wal: self.config.storage.wal.enabled,
        };

        let sstable_config = SSTableConfig {
            block_size: 4096,
            enable_compression: true,
        };

        let block_cache_config = BlockCacheConfig {
            max_size_bytes: self.config.storage.block_cache_size_mb * 1024 * 1024,
            enable_stats: true,
        };

        let compaction_config = CompactionConfig {
            strategy: match self.config.storage.compaction.strategy.as_str() {
                "tiered" => amaters_core::storage::CompactionStrategy::SizeTiered,
                _ => amaters_core::storage::CompactionStrategy::LevelBased,
            },
            l0_threshold: 4,
            level_multiplier: self.config.storage.compaction.level_multiplier,
            base_level_size: 10 * 1024 * 1024,       // 10 MB
            max_compaction_bytes: 100 * 1024 * 1024, // 100 MB
        };

        // Optional value log configuration for large values
        let value_log_config = None; // Disabled for now, can be enabled later

        Ok(LsmTreeConfig {
            data_dir,
            wal_dir,
            memtable_config,
            sstable_config,
            block_cache_config,
            compaction_config,
            value_log_config,
            max_levels: self.config.storage.compaction.num_levels,
            l0_compaction_threshold: 4,
            level_size_multiplier: self.config.storage.compaction.level_multiplier,
        })
    }

    /// Initialize network service
    async fn initialize_network(&mut self) -> ServerResult<()> {
        info!("Initializing network service");

        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ServerError::Config("Storage not initialized".to_string()))?
            .clone();

        let network = NetworkService::new(
            storage,
            self.config.clone(),
            self.health.clone(),
            self.metrics.clone(),
            self.shutdown.clone(),
        );

        self.network = Some(network);
        self.health.set_network_healthy(true);

        info!("Network service initialized successfully");
        Ok(())
    }

    /// Start the server
    pub async fn start(&mut self) -> ServerResult<()> {
        info!("Starting AmateRS server v{}", env!("CARGO_PKG_VERSION"));
        info!("Bind address: {}", self.config.server.bind_address);
        info!("Data directory: {}", self.config.server.data_dir.display());

        // Start network service
        if let Some(ref mut network) = self.network {
            network.start().await?;
        }

        // Mark server as healthy
        self.health.set_status(HealthStatus::Healthy);
        self.health.set_network_healthy(true);

        info!("Server started successfully");
        info!("Press Ctrl+C to shutdown");

        // Wait for shutdown signal
        let mut shutdown_rx = self.shutdown.subscribe();
        shutdown_rx
            .recv()
            .await
            .map_err(|e| ServerError::Network(format!("Shutdown channel error: {}", e)))?;

        info!("Shutdown signal received");
        Ok(())
    }

    /// Gracefully shutdown the server
    pub async fn shutdown(&mut self) -> ServerResult<()> {
        info!("Shutting down server gracefully");
        self.health.set_status(HealthStatus::ShuttingDown);

        let shutdown_timeout = self.config.shutdown_timeout();

        // Shutdown with timeout
        match tokio::time::timeout(shutdown_timeout, self.shutdown_internal()).await {
            Ok(result) => result,
            Err(_) => {
                error!("Shutdown timeout exceeded");
                Err(ServerError::ShutdownTimeout)
            }
        }
    }

    /// Internal shutdown logic
    async fn shutdown_internal(&mut self) -> ServerResult<()> {
        // 1. Stop accepting new connections
        info!("Stopping new connections");
        self.health.set_network_healthy(false);

        // 2. Stop network service
        if let Some(ref mut network) = self.network {
            network.stop().await?;
        }

        // 2. Wait for active connections to drain
        let max_wait = Duration::from_secs(5);
        let start = std::time::Instant::now();
        while self.metrics.snapshot().active_connections > 0 && start.elapsed() < max_wait {
            info!(
                "Waiting for {} active connections to drain",
                self.metrics.snapshot().active_connections
            );
            sleep(Duration::from_millis(100)).await;
        }

        // 3. Flush storage
        if let Some(ref storage) = self.storage {
            info!("Flushing storage");
            storage
                .flush()
                .await
                .map_err(|e| ServerError::Storage(format!("Failed to flush storage: {}", e)))?;
        }

        // 4. Close storage
        if let Some(ref storage) = self.storage {
            info!("Closing storage");
            storage
                .close()
                .await
                .map_err(|e| ServerError::Storage(format!("Failed to close storage: {}", e)))?;
        }

        self.health.set_storage_healthy(false);

        info!("Server shutdown complete");
        Ok(())
    }

    /// Get shutdown coordinator
    pub fn shutdown_coordinator(&self) -> &ShutdownCoordinator {
        &self.shutdown
    }

    /// Get health checker
    pub fn health_checker(&self) -> &HealthChecker {
        &self.health
    }

    /// Get metrics collector
    pub fn metrics_collector(&self) -> &MetricsCollector {
        &self.metrics
    }

    /// Get configuration
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Check if server is running (by checking PID file)
    pub fn is_running(config: &ServerConfig) -> bool {
        let pid_file = &config.server.pid_file;
        if !pid_file.exists() {
            return false;
        }

        // Read PID from file
        if let Ok(contents) = fs::read_to_string(pid_file) {
            if let Ok(pid) = contents.trim().parse::<i32>() {
                // Check if process exists (Unix-specific)
                #[cfg(unix)]
                {
                    use std::process::Command;
                    let output = Command::new("kill").arg("-0").arg(pid.to_string()).output();
                    if let Ok(output) = output {
                        return output.status.success();
                    }
                }
                #[cfg(not(unix))]
                {
                    // On non-Unix, assume running if PID file exists
                    return true;
                }
            }
        }

        false
    }

    /// Write PID file
    pub fn write_pid_file(config: &ServerConfig) -> ServerResult<()> {
        let pid = std::process::id();
        let pid_file = &config.server.pid_file;

        // Create parent directory if needed
        if let Some(parent) = pid_file.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(pid_file, pid.to_string())?;
        info!("PID file written: {} (pid: {})", pid_file.display(), pid);
        Ok(())
    }

    /// Remove PID file
    pub fn remove_pid_file(config: &ServerConfig) -> ServerResult<()> {
        let pid_file = &config.server.pid_file;
        if pid_file.exists() {
            fs::remove_file(pid_file)?;
            info!("PID file removed: {}", pid_file.display());
        }
        Ok(())
    }

    /// Send stop signal to running server
    #[cfg(unix)]
    pub fn stop_server(config: &ServerConfig, force: bool) -> ServerResult<()> {
        let pid_file = &config.server.pid_file;

        if !pid_file.exists() {
            warn!("PID file not found - server may not be running");
            return Ok(());
        }

        let contents = fs::read_to_string(pid_file)?;
        let pid = contents
            .trim()
            .parse::<i32>()
            .map_err(|e| ServerError::Config(format!("Invalid PID in file: {}", e)))?;

        let signal = if force { "SIGKILL" } else { "SIGTERM" };
        info!("Sending {} to process {}", signal, pid);

        use std::process::Command;
        let signal_arg = if force { "-9" } else { "-15" };

        let output = Command::new("kill")
            .arg(signal_arg)
            .arg(pid.to_string())
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ServerError::Network(format!(
                "Failed to stop server: {}",
                stderr
            )));
        }

        info!("Stop signal sent successfully");
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn stop_server(_config: &ServerConfig, _force: bool) -> ServerResult<()> {
        Err(ServerError::Config(
            "Stop command is not supported on this platform".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[tokio::test]
    async fn test_server_creation() {
        let config = ServerConfig::default();
        let server = Server::new(config);

        assert_eq!(server.health_checker().status(), HealthStatus::Starting);
    }

    #[tokio::test]
    async fn test_server_initialization() {
        let mut config = ServerConfig::default();
        config.server.data_dir = env::temp_dir().join("amaters_test_init");
        config.storage.engine = "memory".to_string();

        let mut server = Server::new(config);
        let result = server.initialize().await;

        assert!(result.is_ok());
        assert!(server.storage.is_some());

        // Cleanup
        if server.config.server.data_dir.exists() {
            fs::remove_dir_all(&server.config.server.data_dir).ok();
        }
    }

    #[tokio::test]
    async fn test_lsm_initialization() {
        let mut config = ServerConfig::default();
        config.server.data_dir = env::temp_dir().join("amaters_test_lsm");
        config.storage.engine = "lsm".to_string();

        let mut server = Server::new(config);
        let result = server.initialize().await;

        assert!(result.is_ok());
        assert!(server.storage.is_some());

        // Cleanup
        if server.config.server.data_dir.exists() {
            fs::remove_dir_all(&server.config.server.data_dir).ok();
        }
    }

    #[tokio::test]
    async fn test_data_directory_creation() {
        let mut config = ServerConfig::default();
        config.server.data_dir = env::temp_dir().join("amaters_test_dir");

        // Ensure directory doesn't exist
        if config.server.data_dir.exists() {
            fs::remove_dir_all(&config.server.data_dir).ok();
        }

        let mut server = Server::new(config.clone());
        server
            .ensure_data_directory()
            .expect("Failed to create directory");

        assert!(config.server.data_dir.exists());

        // Cleanup
        fs::remove_dir_all(&config.server.data_dir).ok();
    }

    #[tokio::test]
    async fn test_shutdown_coordinator() {
        let config = ServerConfig::default();
        let server = Server::new(config);

        let coordinator = server.shutdown_coordinator();
        assert!(!coordinator.is_shutting_down());

        coordinator.shutdown();
        assert!(coordinator.is_shutting_down());
    }
}
