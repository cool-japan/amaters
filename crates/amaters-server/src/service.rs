//! Network service module
//!
//! This module integrates the AQL service with the server runtime.

use crate::config::ServerConfig;
use crate::health::HealthChecker;
use crate::metrics::MetricsCollector;
use crate::server::{ServerError, ServerResult, Storage};
use crate::shutdown::ShutdownCoordinator;
use crate::tls_config::TlsServerBuilder;
use amaters_net::grpc_service::AqlGrpcService;
use amaters_net::{AqlServerBuilder, AqlServiceImpl};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{error, info};

/// Network service manager
///
/// Manages the AQL gRPC service
pub struct NetworkService {
    /// AQL service implementation
    service: Arc<AqlServiceImpl<Storage>>,
    /// Server configuration
    config: Arc<ServerConfig>,
    /// Health checker
    health: HealthChecker,
    /// Metrics collector
    metrics: MetricsCollector,
    /// Shutdown receiver
    shutdown: ShutdownCoordinator,
    /// Server task handle
    server_handle: Option<JoinHandle<Result<(), tonic::transport::Error>>>,
}

impl NetworkService {
    /// Create a new network service
    pub fn new(
        storage: Arc<Storage>,
        config: Arc<ServerConfig>,
        health: HealthChecker,
        metrics: MetricsCollector,
        shutdown: ShutdownCoordinator,
    ) -> Self {
        let service = Arc::new(AqlServerBuilder::new(storage).build());

        Self {
            service,
            config,
            health,
            metrics,
            shutdown,
            server_handle: None,
        }
    }

    /// Start the network service
    pub async fn start(&mut self) -> ServerResult<()> {
        let addr: SocketAddr = self
            .config
            .server
            .bind_address
            .parse()
            .map_err(|e| ServerError::Config(format!("Invalid bind address: {}", e)))?;

        info!("Starting AQL gRPC service on {}", addr);

        // Build TLS configuration if enabled
        let tls_config = TlsServerBuilder::build(&self.config)?;

        if tls_config.is_some() {
            info!("TLS enabled for gRPC server");
        } else {
            info!("TLS not enabled (development mode only)");
        }

        // Create gRPC service bridge
        let grpc_service = AqlGrpcService::new(self.service.clone());

        // Build gRPC server
        use amaters_net::proto::aql::aql_service_server::AqlServiceServer;

        let mut server_builder = tonic::transport::Server::builder();

        // Add TLS if configured
        if let Some(tls) = tls_config {
            server_builder = server_builder
                .tls_config(tls)
                .map_err(|e| ServerError::TlsSetup(format!("Failed to configure TLS: {}", e)))?;
        }

        let server = server_builder.add_service(AqlServiceServer::new(grpc_service));

        // Setup graceful shutdown
        let mut shutdown_rx = self.shutdown.subscribe();

        // Spawn server task
        let handle = tokio::spawn(async move {
            server
                .serve_with_shutdown(addr, async {
                    shutdown_rx.recv().await.ok();
                    info!("Received shutdown signal, stopping gRPC server");
                })
                .await
        });

        self.server_handle = Some(handle);

        info!("AQL gRPC service started successfully on {}", addr);
        Ok(())
    }

    /// Stop the network service
    pub async fn stop(&mut self) -> ServerResult<()> {
        info!("Stopping network service");

        if let Some(handle) = self.server_handle.take() {
            // The server will shutdown gracefully via the shutdown signal
            // Just wait for it to complete
            match handle.await {
                Ok(result) => {
                    if let Err(e) = result {
                        error!("gRPC server stopped with error: {}", e);
                        return Err(ServerError::Network(format!("gRPC server error: {}", e)));
                    }
                }
                Err(e) => {
                    error!("Failed to join server task: {}", e);
                    return Err(ServerError::Network(format!("Join error: {}", e)));
                }
            }
            info!("Network service stopped");
        }

        Ok(())
    }

    /// Get reference to the AQL service
    pub fn service(&self) -> &Arc<AqlServiceImpl<Storage>> {
        &self.service
    }
}

impl Drop for NetworkService {
    fn drop(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use amaters_core::storage::MemoryStorage;

    #[tokio::test]
    async fn test_network_service_creation() {
        let storage = Arc::new(Storage::Memory(MemoryStorage::new()));
        let config = Arc::new(ServerConfig::default());
        let health = HealthChecker::new();
        let metrics = MetricsCollector::new();
        let shutdown = ShutdownCoordinator::new();

        let service = NetworkService::new(storage, config, health, metrics, shutdown);
        assert!(service.server_handle.is_none());
    }

    #[tokio::test]
    async fn test_network_service_start_stop() {
        let storage = Arc::new(Storage::Memory(MemoryStorage::new()));
        let mut config = ServerConfig::default();
        config.server.bind_address = "127.0.0.1:18787".to_string();
        let config = Arc::new(config);
        let health = HealthChecker::new();
        let metrics = MetricsCollector::new();
        let shutdown = ShutdownCoordinator::new();

        let mut service = NetworkService::new(storage, config, health, metrics, shutdown.clone());

        // Start the service
        let result = service.start().await;
        assert!(result.is_ok());

        // Give it a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Trigger shutdown
        shutdown.shutdown();

        // Stop the service
        let stop_result = service.stop().await;
        assert!(stop_result.is_ok());
    }
}
