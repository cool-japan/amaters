//! gRPC client implementation with connection pooling
//!
//! Provides a high-level client interface for AQL queries with automatic
//! connection pooling, load balancing, and circuit breaker protection.

use crate::balancer::BalancingStrategy;
use crate::error::{NetError, NetResult};
use crate::pool::{ConnectionPool, ConnectionPoolBuilder, PoolConfig, PoolStats};
// TODO: Enable when tonic service generation is configured
// use crate::proto::aql::aql_service_client::AqlServiceClient;
use std::sync::Arc;
use std::time::Duration;
use tonic::transport::Channel;

/// Configuration for AQL client
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Request timeout
    pub request_timeout: Duration,
    /// Enable keep-alive
    pub keep_alive: bool,
    /// Keep-alive interval
    pub keep_alive_interval: Duration,
    /// Connection pool configuration
    pub pool: PoolConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            keep_alive: true,
            keep_alive_interval: Duration::from_secs(60),
            pool: PoolConfig::default(),
        }
    }
}

/// AQL client with connection pooling
pub struct AqlClient {
    pool: Arc<ConnectionPool>,
    config: ClientConfig,
}

impl AqlClient {
    /// Create a new client with default configuration
    pub fn new() -> Self {
        Self::with_config(ClientConfig::default())
    }

    /// Create a new client with custom configuration
    pub fn with_config(config: ClientConfig) -> Self {
        let pool = ConnectionPool::new(config.pool.clone());

        Self {
            pool: Arc::new(pool),
            config,
        }
    }

    /// Create a client using a builder pattern
    pub fn builder() -> AqlClientBuilder {
        AqlClientBuilder::new()
    }

    /// Add an endpoint to the client's connection pool
    pub fn add_endpoint(&self, id: String, address: String) {
        self.pool.add_endpoint(id, address);
    }

    /// Add an endpoint with weight for weighted load balancing
    pub fn add_endpoint_with_weight(&self, id: String, address: String, weight: u32) {
        self.pool.add_endpoint_with_weight(id, address, weight);
    }

    /// Remove an endpoint from the connection pool
    pub fn remove_endpoint(&self, endpoint_id: &str) -> bool {
        self.pool.remove_endpoint(endpoint_id)
    }

    // Get a gRPC service client
    // TODO: Enable when tonic service generation is configured
    /*
    pub async fn get_service_client(&self) -> NetResult<AqlServiceClient<Channel>> {
        let conn = self.pool.get_connection().await?;
        Ok(AqlServiceClient::new(conn.channel().clone()))
    }
    */

    /// Get connection pool statistics
    pub fn pool_stats(&self) -> PoolStats {
        self.pool.stats()
    }

    /// Get circuit breaker statistics
    pub fn circuit_breaker_stats(&self) -> Option<crate::circuit_breaker::CircuitBreakerStats> {
        self.pool.circuit_breaker_stats()
    }

    // Execute a request with retry logic
    // TODO: Enable when tonic service generation is configured
    /*
    pub async fn execute_with_retry<F, T>(&self, operation: F, max_retries: usize) -> NetResult<T>
    where
        F: Fn(AqlServiceClient<Channel>) -> futures::future::BoxFuture<'static, NetResult<T>>,
    {
        let mut retries = 0;
        let mut backoff = Duration::from_millis(100);

        loop {
            let client = self.get_service_client().await?;
            match operation(client).await {
                Ok(result) => return Ok(result),
                Err(e) if e.is_retryable() && retries < max_retries => {
                    retries += 1;
                    // Exponential backoff with jitter
                    tokio::time::sleep(backoff).await;
                    backoff = backoff * 2;
                    if backoff > Duration::from_secs(10) {
                        backoff = Duration::from_secs(10);
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    */

    /// Drain the connection pool (prepare for graceful shutdown)
    pub async fn drain(&self) -> NetResult<()> {
        self.pool.drain().await
    }

    /// Shutdown the client gracefully
    pub async fn shutdown(self) -> NetResult<()> {
        Arc::try_unwrap(self.pool)
            .map_err(|_| {
                NetError::ServerInternal("Cannot shutdown: pool still has references".to_string())
            })?
            .shutdown()
            .await
    }
}

impl Default for AqlClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for AQL client
pub struct AqlClientBuilder {
    config: ClientConfig,
    pool_builder: ConnectionPoolBuilder,
}

impl AqlClientBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            config: ClientConfig::default(),
            pool_builder: ConnectionPoolBuilder::new(),
        }
    }

    /// Set connection timeout
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.config.connect_timeout = timeout;
        self.pool_builder = self.pool_builder.connect_timeout(timeout);
        self
    }

    /// Set request timeout
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.config.request_timeout = timeout;
        self
    }

    /// Enable or disable keep-alive
    pub fn keep_alive(mut self, enabled: bool) -> Self {
        self.config.keep_alive = enabled;
        self
    }

    /// Set keep-alive interval
    pub fn keep_alive_interval(mut self, interval: Duration) -> Self {
        self.config.keep_alive_interval = interval;
        self
    }

    /// Set minimum pool size
    pub fn min_pool_size(mut self, size: usize) -> Self {
        self.config.pool.min_size = size;
        self.pool_builder = self.pool_builder.min_size(size);
        self
    }

    /// Set maximum pool size
    pub fn max_pool_size(mut self, size: usize) -> Self {
        self.config.pool.max_size = size;
        self.pool_builder = self.pool_builder.max_size(size);
        self
    }

    /// Set idle timeout
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.config.pool.idle_timeout = timeout;
        self.pool_builder = self.pool_builder.idle_timeout(timeout);
        self
    }

    /// Set max connection lifetime
    pub fn max_lifetime(mut self, lifetime: Duration) -> Self {
        self.config.pool.max_lifetime = lifetime;
        self.pool_builder = self.pool_builder.max_lifetime(lifetime);
        self
    }

    /// Set health check interval
    pub fn health_check_interval(mut self, interval: Duration) -> Self {
        self.config.pool.health_check_interval = interval;
        self.pool_builder = self.pool_builder.health_check_interval(interval);
        self
    }

    /// Set load balancing strategy
    pub fn balancing_strategy(mut self, strategy: BalancingStrategy) -> Self {
        self.config.pool.balancing_strategy = strategy;
        self.pool_builder = self.pool_builder.balancing_strategy(strategy);
        self
    }

    /// Enable or disable circuit breaker
    pub fn circuit_breaker(mut self, enabled: bool) -> Self {
        self.config.pool.enable_circuit_breaker = enabled;
        self.pool_builder = self.pool_builder.circuit_breaker(enabled);
        self
    }

    /// Add an endpoint
    pub fn add_endpoint(mut self, id: String, address: String) -> Self {
        self.pool_builder = self.pool_builder.add_endpoint(id, address);
        self
    }

    /// Add an endpoint with weight
    pub fn add_endpoint_with_weight(mut self, id: String, address: String, weight: u32) -> Self {
        self.pool_builder = self
            .pool_builder
            .add_endpoint_with_weight(id, address, weight);
        self
    }

    /// Build the client
    pub fn build(self) -> AqlClient {
        let pool = self.pool_builder.build();

        AqlClient {
            pool: Arc::new(pool),
            config: self.config,
        }
    }
}

impl Default for AqlClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_config_default() {
        let config = ClientConfig::default();
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert!(config.keep_alive);
    }

    #[tokio::test]
    async fn test_client_creation() {
        let config = ClientConfig::default();
        let _client = AqlClient::with_config(config);
    }

    #[tokio::test]
    async fn test_client_builder() {
        let client = AqlClient::builder()
            .connect_timeout(Duration::from_secs(5))
            .request_timeout(Duration::from_secs(15))
            .min_pool_size(3)
            .max_pool_size(15)
            .balancing_strategy(BalancingStrategy::RoundRobin)
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string())
            .add_endpoint("ep2".to_string(), "localhost:50052".to_string())
            .build();

        let stats = client.pool_stats();
        assert_eq!(stats.active_connections, 0);
    }

    #[tokio::test]
    async fn test_client_add_remove_endpoint() {
        let client = AqlClient::new();

        client.add_endpoint("ep1".to_string(), "localhost:50051".to_string());
        client.add_endpoint("ep2".to_string(), "localhost:50052".to_string());

        assert!(client.remove_endpoint("ep1"));
        assert!(!client.remove_endpoint("ep3"));
    }

    #[tokio::test]
    async fn test_client_pool_stats() {
        let client = AqlClient::builder()
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string())
            .build();

        let stats = client.pool_stats();
        assert_eq!(stats.total_connections, 0);
    }

    #[tokio::test]
    async fn test_client_drain() {
        let client = AqlClient::builder()
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string())
            .build();

        let result = client.drain().await;
        assert!(result.is_ok());
    }
}
