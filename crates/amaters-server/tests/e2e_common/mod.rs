//! Shared infrastructure for E2E tests
//!
//! This module provides the test context and utilities for end-to-end testing.

use amaters_sdk_rust::{AmateRSClient, ClientConfig};
use amaters_server::config::{
    AuthSettings, AuthorizationSettings, CompactionSettings, LoggingSettings, MetricsSettings,
    NetworkSettings, ServerConfig, ServerSettings, StorageSettings, WalSettings,
};
use amaters_server::server::Server;
use amaters_server::shutdown::ShutdownCoordinator;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use uuid::Uuid;

/// Allocate a unique port for E2E testing using OS-assigned ephemeral ports
pub fn allocate_e2e_port() -> u32 {
    // Bind to port 0 to get an OS-assigned ephemeral port
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind to ephemeral port");
    let port = listener
        .local_addr()
        .expect("Failed to get local address")
        .port() as u32;
    // Drop listener to release the port immediately
    drop(listener);
    port
}

/// E2E test context with running server and client
pub struct E2eTestContext {
    /// Running server instance
    server_handle: JoinHandle<()>,
    /// Connected client
    pub client: AmateRSClient,
    /// Server address
    pub addr: SocketAddr,
    /// Temporary directory for test data
    pub temp_dir: PathBuf,
    /// Shutdown coordinator for graceful server shutdown
    shutdown_coordinator: ShutdownCoordinator,
    /// Storage engine type used for this test
    storage_engine: String,
}

impl E2eTestContext {
    /// Create new E2E test context with memory storage
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_storage("memory").await
    }

    /// Create new E2E test context with specified storage engine
    pub async fn with_storage(engine: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("amaters_e2e_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)?;

        let port = allocate_e2e_port();
        let bind_address = format!("127.0.0.1:{}", port);
        let addr: SocketAddr = bind_address.parse()?;

        // Create server configuration
        let config = create_e2e_server_config(&temp_dir, port, engine);

        // Start server
        let (server_handle, shutdown_coordinator) = start_test_server(config).await?;

        // Create client with retry logic for server startup
        let client_addr = format!("http://{}", bind_address);
        let client_config = ClientConfig::new(client_addr)
            .with_connect_timeout(Duration::from_secs(5))
            .with_request_timeout(Duration::from_secs(120));

        // Retry connection with exponential backoff
        let mut retry_delay = Duration::from_millis(50);
        let max_retries = 10;
        let mut client = None;

        for attempt in 0..max_retries {
            match AmateRSClient::connect_with_config(client_config.clone()).await {
                Ok(c) => {
                    client = Some(c);
                    break;
                }
                Err(e) if attempt < max_retries - 1 => {
                    sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(Duration::from_secs(2));
                }
                Err(e) => {
                    return Err(
                        format!("Failed to connect after {} attempts: {}", max_retries, e).into(),
                    );
                }
            }
        }

        let client = client.ok_or("Failed to establish client connection")?;

        Ok(Self {
            server_handle,
            client,
            addr,
            temp_dir,
            shutdown_coordinator,
            storage_engine: engine.to_string(),
        })
    }

    /// Create multiple clients for concurrent testing
    pub async fn create_additional_client(
        &self,
    ) -> Result<AmateRSClient, Box<dyn std::error::Error>> {
        let client_addr = format!("http://{}", self.addr);
        let client_config = ClientConfig::new(client_addr)
            .with_connect_timeout(Duration::from_secs(5))
            .with_request_timeout(Duration::from_secs(120));

        Ok(AmateRSClient::connect_with_config(client_config).await?)
    }

    /// Restart the server (useful for persistence tests)
    pub async fn restart_server(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Trigger graceful shutdown through the server's coordinator
        // This ensures storage is properly flushed before shutdown
        self.shutdown_coordinator.shutdown();

        // Wait for server to stop gracefully
        let _ = tokio::time::timeout(Duration::from_secs(10), &mut self.server_handle).await;

        // Additional wait to ensure all I/O is complete
        sleep(Duration::from_millis(200)).await;

        // Start new server with same config and storage engine
        let port = self.addr.port() as u32;
        let config = create_e2e_server_config(&self.temp_dir, port, &self.storage_engine);
        let (server_handle, shutdown_coordinator) = start_test_server(config).await?;

        self.server_handle = server_handle;
        self.shutdown_coordinator = shutdown_coordinator;

        // Wait for server to be ready
        sleep(Duration::from_millis(300)).await;

        // Reconnect client with retry logic
        let client_addr = format!("http://{}", self.addr);
        let client_config = ClientConfig::new(client_addr)
            .with_connect_timeout(Duration::from_secs(5))
            .with_request_timeout(Duration::from_secs(120));

        let mut retry_delay = Duration::from_millis(100);
        for attempt in 0..10 {
            match AmateRSClient::connect_with_config(client_config.clone()).await {
                Ok(c) => {
                    self.client = c;
                    return Ok(());
                }
                Err(_) if attempt < 9 => {
                    sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(Duration::from_secs(2));
                }
                Err(e) => {
                    return Err(format!("Failed to reconnect after restart: {}", e).into());
                }
            }
        }

        Ok(())
    }

    /// Cleanup test resources
    pub async fn cleanup(self) {
        // Trigger graceful shutdown through the server's coordinator
        self.shutdown_coordinator.shutdown();

        // Wait for server to stop gracefully
        let _ = tokio::time::timeout(Duration::from_secs(10), self.server_handle).await;

        // Additional wait to ensure all I/O is complete
        sleep(Duration::from_millis(100)).await;

        // Cleanup temp directory
        if self.temp_dir.exists() {
            std::fs::remove_dir_all(&self.temp_dir).ok();
        }
    }
}

/// Start a test server and return handle + shutdown coordinator
pub async fn start_test_server(
    config: ServerConfig,
) -> Result<(JoinHandle<()>, ShutdownCoordinator), Box<dyn std::error::Error>> {
    // Create server and get its shutdown coordinator before spawning
    let mut server = Server::new(config);
    let shutdown_coordinator = server.shutdown_coordinator().clone();

    // Initialize server before spawning
    server
        .initialize()
        .await
        .map_err(|e| format!("Failed to initialize server: {}", e))?;

    let handle = tokio::spawn(async move {
        if let Err(e) = server.start().await {
            eprintln!("Server error: {}", e);
        }

        // Graceful shutdown when server.start() returns
        if let Err(e) = server.shutdown().await {
            eprintln!("Server shutdown error: {}", e);
        }
    });

    Ok((handle, shutdown_coordinator))
}

/// Create E2E server configuration
pub fn create_e2e_server_config(temp_dir: &Path, port: u32, engine: &str) -> ServerConfig {
    ServerConfig {
        server: ServerSettings {
            bind_address: format!("127.0.0.1:{}", port),
            data_dir: temp_dir.to_path_buf(),
            pid_file: temp_dir.join("test.pid"),
            max_connections: 100,
            shutdown_timeout_secs: 5,
        },
        storage: StorageSettings {
            engine: engine.to_string(),
            wal: WalSettings {
                enabled: true,
                dir: PathBuf::from("wal"),
                segment_size_mb: 64,
                sync_mode: "interval".to_string(),
            },
            memtable_size_mb: 16,
            block_cache_size_mb: 32,
            compaction: CompactionSettings {
                strategy: "leveled".to_string(),
                num_levels: 7,
                level_multiplier: 10,
                max_concurrent: 2,
            },
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
            level: "info".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_e2e_context_creation() {
        let ctx = E2eTestContext::new().await;
        assert!(ctx.is_ok());
        if let Ok(ctx) = ctx {
            ctx.cleanup().await;
        }
    }

    #[test]
    fn test_port_allocation() {
        let port1 = allocate_e2e_port();
        let port2 = allocate_e2e_port();
        // Ports should be different (OS allocates unique ones)
        assert_ne!(port1, port2);
        // Ports should be in ephemeral range
        assert!(port1 > 1024);
        assert!(port2 > 1024);
    }
}
