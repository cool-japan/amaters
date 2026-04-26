//! gRPC client implementation with connection pooling
//!
//! Provides a high-level client interface for AQL queries with automatic
//! connection pooling, load balancing, circuit breaker protection, retry logic
//! with exponential backoff, and request/response compression.

use crate::balancer::BalancingStrategy;
use crate::circuit_breaker::CircuitBreaker;
use crate::error::{NetError, NetResult};
use crate::pool::{ConnectionPool, ConnectionPoolBuilder, PoolConfig, PoolStats};
use crate::proto::aql::aql_service_client::AqlServiceClient;
use crate::proto::aql::{
    BatchRequest, BatchResponse, HealthCheckRequest, HealthCheckResponse, QueryRequest,
    QueryResponse,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

/// Compression algorithm for gRPC requests/responses
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionAlgorithm {
    /// No compression (identity)
    #[default]
    Identity,
    /// Gzip compression
    Gzip,
}

/// Configuration for request/response compression
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Whether compression is enabled
    pub enabled: bool,
    /// Compression algorithm to use
    pub algorithm: CompressionAlgorithm,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            algorithm: CompressionAlgorithm::Identity,
        }
    }
}

impl CompressionConfig {
    /// Create a new compression config with gzip enabled
    pub fn gzip() -> Self {
        Self {
            enabled: true,
            algorithm: CompressionAlgorithm::Gzip,
        }
    }

    /// Convert algorithm to tonic CompressionEncoding
    fn to_tonic_encoding(&self) -> Option<CompressionEncoding> {
        if !self.enabled {
            return None;
        }
        match self.algorithm {
            CompressionAlgorithm::Identity => None,
            CompressionAlgorithm::Gzip => Some(CompressionEncoding::Gzip),
        }
    }
}

/// TLS configuration for the AQL client
///
/// Configures how the client establishes secure connections to servers.
/// Supports server certificate verification (CA cert), mutual TLS (client cert + key),
/// custom domain names, and a skip-verification flag for development use.
#[derive(Debug, Clone, Default)]
pub struct TlsClientConfig {
    /// Path to the CA certificate PEM file for server verification
    pub ca_cert_path: Option<PathBuf>,
    /// Path to the client certificate PEM file for mTLS
    pub client_cert_path: Option<PathBuf>,
    /// Path to the client private key PEM file for mTLS
    pub client_key_path: Option<PathBuf>,
    /// Override the domain name used for TLS verification
    pub domain_name: Option<String>,
    /// Skip server certificate verification (development/testing only)
    pub skip_verification: bool,
}

impl TlsClientConfig {
    /// Create a new empty TLS configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the CA certificate path for server verification
    pub fn with_ca_cert(mut self, path: impl Into<PathBuf>) -> Self {
        self.ca_cert_path = Some(path.into());
        self
    }

    /// Set client certificate and key paths for mutual TLS
    pub fn with_client_identity(
        mut self,
        cert_path: impl Into<PathBuf>,
        key_path: impl Into<PathBuf>,
    ) -> Self {
        self.client_cert_path = Some(cert_path.into());
        self.client_key_path = Some(key_path.into());
        self
    }

    /// Set the domain name override for TLS verification
    pub fn with_domain_name(mut self, domain: impl Into<String>) -> Self {
        self.domain_name = Some(domain.into());
        self
    }

    /// Enable skip verification mode (development/testing only)
    ///
    /// WARNING: This disables server certificate verification and should
    /// never be used in production environments.
    pub fn with_skip_verification(mut self, skip: bool) -> Self {
        self.skip_verification = skip;
        self
    }

    /// Validate the TLS configuration for consistency
    ///
    /// Returns an error if the configuration is invalid, such as having
    /// a client certificate without a key or vice versa.
    pub fn validate(&self) -> NetResult<()> {
        // Check for mismatched client cert/key
        match (&self.client_cert_path, &self.client_key_path) {
            (Some(_), None) => {
                return Err(NetError::TlsError(
                    "Client certificate specified without client key".to_string(),
                ));
            }
            (None, Some(_)) => {
                return Err(NetError::TlsError(
                    "Client key specified without client certificate".to_string(),
                ));
            }
            _ => {}
        }
        Ok(())
    }

    /// Build a tonic `ClientTlsConfig` from this configuration
    ///
    /// Loads certificate and key files from disk and assembles
    /// the tonic TLS configuration object.
    pub fn build_tonic_tls_config(&self) -> NetResult<ClientTlsConfig> {
        self.validate()?;

        let mut tls_config = ClientTlsConfig::new();

        // Load and set CA certificate
        if let Some(ref ca_path) = self.ca_cert_path {
            let ca_pem = std::fs::read(ca_path).map_err(|e| {
                NetError::TlsError(format!(
                    "Failed to read CA certificate file '{}': {}",
                    ca_path.display(),
                    e
                ))
            })?;
            let ca_cert = Certificate::from_pem(ca_pem);
            tls_config = tls_config.ca_certificate(ca_cert);
        }

        // Load and set client identity for mTLS
        if let Some(ref cert_path) = self.client_cert_path {
            let key_path = self.client_key_path.as_ref().ok_or_else(|| {
                NetError::TlsError("Client key path missing for mTLS".to_string())
            })?;

            let cert_pem = std::fs::read(cert_path).map_err(|e| {
                NetError::TlsError(format!(
                    "Failed to read client certificate file '{}': {}",
                    cert_path.display(),
                    e
                ))
            })?;
            let key_pem = std::fs::read(key_path).map_err(|e| {
                NetError::TlsError(format!(
                    "Failed to read client key file '{}': {}",
                    key_path.display(),
                    e
                ))
            })?;

            let identity = Identity::from_pem(cert_pem, key_pem);
            tls_config = tls_config.identity(identity);
        }

        // Set domain name override
        if let Some(ref domain) = self.domain_name {
            tls_config = tls_config.domain_name(domain.clone());
        }

        Ok(tls_config)
    }
}

/// Retry policy determining when to retry failed requests
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetryPolicy {
    /// Never retry, fail immediately
    Never,
    /// Retry on any error
    OnError,
    /// Only retry on transient/unavailable errors (timeouts, connection issues, server unavailable)
    #[default]
    OnTransient,
}

impl RetryPolicy {
    /// Determine if a given error should be retried under this policy
    pub fn should_retry(&self, error: &NetError) -> bool {
        match self {
            RetryPolicy::Never => false,
            RetryPolicy::OnError => true,
            RetryPolicy::OnTransient => error.is_retryable(),
        }
    }
}

/// Configuration for retry logic with exponential backoff
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: usize,
    /// Initial backoff duration before first retry
    pub initial_backoff: Duration,
    /// Maximum backoff duration (cap)
    pub max_backoff: Duration,
    /// Multiplier applied to backoff after each retry
    pub backoff_multiplier: f64,
    /// Retry policy determining which errors to retry
    pub policy: RetryPolicy,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            policy: RetryPolicy::OnTransient,
        }
    }
}

impl RetryConfig {
    /// Calculate the backoff duration for a given retry attempt (0-indexed)
    ///
    /// Uses exponential backoff: initial_backoff * (backoff_multiplier ^ attempt),
    /// capped at max_backoff. Adds deterministic jitter based on attempt number.
    pub fn backoff_duration(&self, attempt: usize) -> Duration {
        let base_ms =
            self.initial_backoff.as_millis() as f64 * self.backoff_multiplier.powi(attempt as i32);
        let capped_ms = base_ms.min(self.max_backoff.as_millis() as f64);

        // Add deterministic jitter: use attempt-based offset to spread retries
        // Jitter is up to 25% of the base duration, derived from attempt index
        let jitter_factor = ((attempt as f64 * 0.618033988) % 1.0) * 0.25;
        let jittered_ms = capped_ms * (1.0 + jitter_factor);
        let final_ms = jittered_ms.min(self.max_backoff.as_millis() as f64);

        Duration::from_millis(final_ms as u64)
    }

    /// Create a retry config that never retries
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            policy: RetryPolicy::Never,
            ..Default::default()
        }
    }
}

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
    /// Retry configuration
    pub retry: RetryConfig,
    /// Compression configuration
    pub compression: CompressionConfig,
    /// TLS configuration (None means plaintext connections)
    pub tls: Option<TlsClientConfig>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            keep_alive: true,
            keep_alive_interval: Duration::from_secs(60),
            pool: PoolConfig::default(),
            retry: RetryConfig::default(),
            compression: CompressionConfig::default(),
            tls: None,
        }
    }
}

/// AQL client with connection pooling, retry logic, and compression
pub struct AqlClient {
    pool: Arc<ConnectionPool>,
    config: ClientConfig,
    circuit_breaker: Option<CircuitBreaker>,
}

impl AqlClient {
    /// Create a new client with default configuration
    ///
    /// Default configuration uses plaintext (no TLS), so this never fails.
    pub fn new() -> Self {
        // Default config has no TLS, so build_tonic_tls_config is never called
        Self::with_config(ClientConfig::default())
            .expect("Default ClientConfig should always be valid")
    }

    /// Create a new client with custom configuration
    ///
    /// # Errors
    ///
    /// Returns `NetError::TlsError` if TLS configuration is invalid or
    /// certificate/key files cannot be read.
    pub fn with_config(config: ClientConfig) -> NetResult<Self> {
        let cb = if config.pool.enable_circuit_breaker {
            Some(CircuitBreaker::new())
        } else {
            None
        };

        let pool = if let Some(ref tls_cfg) = config.tls {
            let tonic_tls = tls_cfg.build_tonic_tls_config()?;
            ConnectionPool::with_tls(config.pool.clone(), tonic_tls)
        } else {
            ConnectionPool::new(config.pool.clone())
        };

        Ok(Self {
            pool: Arc::new(pool),
            config,
            circuit_breaker: cb,
        })
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

    /// Get a gRPC AQL service client from the connection pool
    ///
    /// Applies compression settings if the `compression` feature is enabled and
    /// the compression config is set to a non-identity algorithm.
    pub async fn get_service_client(&self) -> NetResult<AqlServiceClient<Channel>> {
        let conn = self.pool.get_connection().await?;
        let mut client = AqlServiceClient::new(conn.channel().clone());

        #[cfg(feature = "compression")]
        if let Some(encoding) = self.config.compression.to_tonic_encoding() {
            client = client.send_compressed(encoding);
            client = client.accept_compressed(encoding);
        }

        Ok(client)
    }

    /// Execute a single AQL query with retry and circuit breaker protection
    pub async fn execute_query(&self, request: QueryRequest) -> NetResult<QueryResponse> {
        self.execute_with_retry(|mut client| {
            let req = request.clone();
            Box::pin(async move {
                client
                    .execute_query(req)
                    .await
                    .map(|resp| resp.into_inner())
                    .map_err(NetError::from)
            })
        })
        .await
    }

    /// Execute a batch of AQL queries with retry and circuit breaker protection
    pub async fn execute_batch(&self, request: BatchRequest) -> NetResult<BatchResponse> {
        self.execute_with_retry(|mut client| {
            let req = request.clone();
            Box::pin(async move {
                client
                    .execute_batch(req)
                    .await
                    .map(|resp| resp.into_inner())
                    .map_err(NetError::from)
            })
        })
        .await
    }

    /// Perform a health check against the server
    pub async fn health_check(&self, service: Option<String>) -> NetResult<HealthCheckResponse> {
        self.execute_with_retry(|mut client| {
            let svc = service.clone();
            Box::pin(async move {
                let request = HealthCheckRequest { service: svc };
                client
                    .health_check(request)
                    .await
                    .map(|resp| resp.into_inner())
                    .map_err(NetError::from)
            })
        })
        .await
    }

    /// Execute an operation with retry logic, circuit breaker, and exponential backoff
    ///
    /// The `operation` closure receives an `AqlServiceClient<Channel>` and returns
    /// a boxed future resolving to `NetResult<T>`.
    pub async fn execute_with_retry<F, T>(&self, operation: F) -> NetResult<T>
    where
        F: Fn(
            AqlServiceClient<Channel>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = NetResult<T>> + Send>>,
        T: Send + 'static,
    {
        let retry_config = &self.config.retry;
        let mut last_error: Option<NetError> = None;

        for attempt in 0..=retry_config.max_retries {
            // Check circuit breaker before attempting
            if let Some(ref cb) = self.circuit_breaker {
                cb.is_request_allowed()?;
            }

            let client = match self.get_service_client().await {
                Ok(c) => c,
                Err(e) => {
                    if let Some(ref cb) = self.circuit_breaker {
                        cb.record_failure();
                    }
                    if attempt < retry_config.max_retries && retry_config.policy.should_retry(&e) {
                        last_error = Some(e);
                        let backoff = retry_config.backoff_duration(attempt);
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(e);
                }
            };

            match operation(client).await {
                Ok(result) => {
                    if let Some(ref cb) = self.circuit_breaker {
                        cb.record_success();
                    }
                    return Ok(result);
                }
                Err(e) => {
                    if let Some(ref cb) = self.circuit_breaker {
                        cb.record_failure();
                    }
                    if attempt < retry_config.max_retries && retry_config.policy.should_retry(&e) {
                        last_error = Some(e);
                        let backoff = retry_config.backoff_duration(attempt);
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        // Should not reach here, but handle gracefully
        Err(last_error.unwrap_or_else(|| {
            NetError::Unknown("Retry loop exhausted without producing a result".to_string())
        }))
    }

    /// Get connection pool statistics
    pub fn pool_stats(&self) -> PoolStats {
        self.pool.stats()
    }

    /// Get circuit breaker statistics
    pub fn circuit_breaker_stats(&self) -> Option<crate::circuit_breaker::CircuitBreakerStats> {
        self.pool.circuit_breaker_stats()
    }

    /// Get the client's retry configuration
    pub fn retry_config(&self) -> &RetryConfig {
        &self.config.retry
    }

    /// Get the client's compression configuration
    pub fn compression_config(&self) -> &CompressionConfig {
        &self.config.compression
    }

    /// Get the client's TLS configuration
    pub fn tls_config(&self) -> Option<&TlsClientConfig> {
        self.config.tls.as_ref()
    }

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

/// Builder for AQL client with fluent configuration
pub struct AqlClientBuilder {
    config: ClientConfig,
    pool_builder: ConnectionPoolBuilder,
    circuit_breaker: Option<CircuitBreaker>,
    tls_client_config: Option<TlsClientConfig>,
}

impl AqlClientBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            config: ClientConfig::default(),
            pool_builder: ConnectionPoolBuilder::new(),
            circuit_breaker: None,
            tls_client_config: None,
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
        if enabled {
            self.circuit_breaker = Some(CircuitBreaker::new());
        } else {
            self.circuit_breaker = None;
        }
        self
    }

    /// Configure retry logic
    pub fn with_retry(mut self, retry_config: RetryConfig) -> Self {
        self.config.retry = retry_config;
        self
    }

    /// Configure compression
    pub fn with_compression(mut self, compression_config: CompressionConfig) -> Self {
        self.config.compression = compression_config;
        self
    }

    /// Set request timeout (alias for builder fluency)
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.config.request_timeout = timeout;
        self
    }

    /// Set a full TLS configuration for the client
    ///
    /// Configures the client to use TLS for all connections using
    /// the provided `TlsClientConfig`.
    pub fn with_tls_config(mut self, tls_config: TlsClientConfig) -> Self {
        self.tls_client_config = Some(tls_config);
        self
    }

    /// Enable TLS with a CA certificate for server verification
    ///
    /// The client will verify the server's certificate against the
    /// provided CA certificate. Connections use `https://`.
    pub fn with_ca_cert(mut self, ca_cert_path: impl Into<PathBuf>) -> Self {
        let config = self
            .tls_client_config
            .take()
            .unwrap_or_default()
            .with_ca_cert(ca_cert_path);
        self.tls_client_config = Some(config);
        self
    }

    /// Enable mutual TLS (mTLS) with client certificate and key
    ///
    /// The client presents its certificate to the server for mutual
    /// authentication. Both the client cert and key must be PEM-encoded.
    pub fn with_mtls(
        mut self,
        cert_path: impl Into<PathBuf>,
        key_path: impl Into<PathBuf>,
    ) -> Self {
        let config = self
            .tls_client_config
            .take()
            .unwrap_or_default()
            .with_client_identity(cert_path, key_path);
        self.tls_client_config = Some(config);
        self
    }

    /// Set the TLS domain name override
    ///
    /// Overrides the domain name used for server certificate verification.
    /// Useful when connecting to servers via IP address or non-standard hostnames.
    pub fn with_tls_domain(mut self, domain: impl Into<String>) -> Self {
        let config = self
            .tls_client_config
            .take()
            .unwrap_or_default()
            .with_domain_name(domain);
        self.tls_client_config = Some(config);
        self
    }

    /// Enable TLS with skip verification (development/testing only)
    ///
    /// WARNING: This disables server certificate verification.
    /// Never use in production.
    pub fn with_tls_skip_verification(mut self) -> Self {
        let config = self
            .tls_client_config
            .take()
            .unwrap_or_default()
            .with_skip_verification(true);
        self.tls_client_config = Some(config);
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
    ///
    /// # Errors
    ///
    /// Returns `NetError::TlsError` if TLS is configured but the
    /// configuration is invalid (e.g., cert without key) or certificate
    /// files cannot be read.
    pub fn build(self) -> NetResult<AqlClient> {
        let pool_builder = if let Some(ref tls_cfg) = self.tls_client_config {
            let tonic_tls = tls_cfg.build_tonic_tls_config()?;
            self.pool_builder.tls_config(tonic_tls)
        } else {
            self.pool_builder
        };

        let pool = pool_builder.build();
        let cb = if self.config.pool.enable_circuit_breaker {
            self.circuit_breaker.or_else(|| Some(CircuitBreaker::new()))
        } else {
            None
        };

        let mut config = self.config;
        config.tls = self.tls_client_config;

        Ok(AqlClient {
            pool: Arc::new(pool),
            config,
            circuit_breaker: cb,
        })
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
        // Verify new defaults
        assert_eq!(config.retry.max_retries, 3);
        assert_eq!(config.retry.initial_backoff, Duration::from_millis(100));
        assert_eq!(config.retry.max_backoff, Duration::from_secs(10));
        assert!((config.retry.backoff_multiplier - 2.0).abs() < f64::EPSILON);
        assert!(!config.compression.enabled);
        assert!(config.tls.is_none());
    }

    #[tokio::test]
    async fn test_client_creation() {
        let config = ClientConfig::default();
        let _client = AqlClient::with_config(config).expect("default config should be valid");
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
            .build()
            .expect("builder should succeed without TLS");

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
            .build()
            .expect("builder should succeed");

        let stats = client.pool_stats();
        assert_eq!(stats.total_connections, 0);
    }

    #[tokio::test]
    async fn test_client_drain() {
        let client = AqlClient::builder()
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string())
            .build()
            .expect("builder should succeed");

        let result = client.drain().await;
        assert!(result.is_ok());
    }

    // --- RetryConfig tests ---

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_backoff, Duration::from_millis(100));
        assert_eq!(config.max_backoff, Duration::from_secs(10));
        assert!((config.backoff_multiplier - 2.0).abs() < f64::EPSILON);
        assert_eq!(config.policy, RetryPolicy::OnTransient);
    }

    #[test]
    fn test_retry_config_no_retry() {
        let config = RetryConfig::no_retry();
        assert_eq!(config.max_retries, 0);
        assert_eq!(config.policy, RetryPolicy::Never);
    }

    #[test]
    fn test_retry_config_custom() {
        let config = RetryConfig {
            max_retries: 5,
            initial_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(30),
            backoff_multiplier: 3.0,
            policy: RetryPolicy::OnError,
        };
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.initial_backoff, Duration::from_millis(200));
        assert_eq!(config.max_backoff, Duration::from_secs(30));
        assert!((config.backoff_multiplier - 3.0).abs() < f64::EPSILON);
        assert_eq!(config.policy, RetryPolicy::OnError);
    }

    // --- Exponential backoff calculation tests ---

    #[test]
    fn test_backoff_duration_exponential_growth() {
        let config = RetryConfig {
            initial_backoff: Duration::from_millis(100),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
            ..Default::default()
        };

        // Attempt 0: base = 100ms
        let d0 = config.backoff_duration(0);
        // Attempt 1: base = 200ms
        let d1 = config.backoff_duration(1);
        // Attempt 2: base = 400ms
        let d2 = config.backoff_duration(2);

        // Each should be roughly double the previous (with jitter, so check range)
        assert!(d0.as_millis() >= 100, "d0 should be >= 100ms, got {d0:?}");
        assert!(
            d0.as_millis() <= 130,
            "d0 should be <= 130ms (jitter), got {d0:?}"
        );
        assert!(d1.as_millis() >= 200, "d1 should be >= 200ms, got {d1:?}");
        assert!(
            d1.as_millis() <= 260,
            "d1 should be <= 260ms (jitter), got {d1:?}"
        );
        assert!(d2.as_millis() >= 400, "d2 should be >= 400ms, got {d2:?}");
        assert!(
            d2.as_millis() <= 520,
            "d2 should be <= 520ms (jitter), got {d2:?}"
        );
    }

    #[test]
    fn test_backoff_duration_capped_at_max() {
        let config = RetryConfig {
            initial_backoff: Duration::from_secs(1),
            backoff_multiplier: 10.0,
            max_backoff: Duration::from_secs(5),
            ..Default::default()
        };

        // Attempt 0: base = 1000ms, capped at 5000
        let d0 = config.backoff_duration(0);
        assert!(d0.as_millis() >= 1000);
        assert!(d0.as_millis() <= 1300);

        // Attempt 2: base = 100_000ms, capped at 5000ms
        let d2 = config.backoff_duration(2);
        assert!(
            d2.as_millis() <= 5000,
            "Should be capped at max_backoff, got {d2:?}"
        );
    }

    #[test]
    fn test_backoff_duration_with_multiplier_one() {
        let config = RetryConfig {
            initial_backoff: Duration::from_millis(500),
            backoff_multiplier: 1.0,
            max_backoff: Duration::from_secs(60),
            ..Default::default()
        };

        // All attempts should have roughly the same base (500ms + jitter)
        let d0 = config.backoff_duration(0);
        let d1 = config.backoff_duration(1);
        let d2 = config.backoff_duration(2);

        assert!(d0.as_millis() >= 500 && d0.as_millis() <= 650);
        assert!(d1.as_millis() >= 500 && d1.as_millis() <= 650);
        assert!(d2.as_millis() >= 500 && d2.as_millis() <= 650);
    }

    // --- RetryPolicy tests ---

    #[test]
    fn test_retry_policy_never() {
        let policy = RetryPolicy::Never;
        assert!(!policy.should_retry(&NetError::Timeout("test".to_string())));
        assert!(!policy.should_retry(&NetError::ServerUnavailable("test".to_string())));
        assert!(!policy.should_retry(&NetError::InvalidRequest("test".to_string())));
    }

    #[test]
    fn test_retry_policy_on_error() {
        let policy = RetryPolicy::OnError;
        assert!(policy.should_retry(&NetError::Timeout("test".to_string())));
        assert!(policy.should_retry(&NetError::ServerUnavailable("test".to_string())));
        assert!(policy.should_retry(&NetError::InvalidRequest("test".to_string())));
        assert!(policy.should_retry(&NetError::AuthFailed("test".to_string())));
    }

    #[test]
    fn test_retry_policy_on_transient() {
        let policy = RetryPolicy::OnTransient;

        // Transient/retryable errors should be retried
        assert!(policy.should_retry(&NetError::Timeout("test".to_string())));
        assert!(policy.should_retry(&NetError::ConnectionRefused("test".to_string())));
        assert!(policy.should_retry(&NetError::ConnectionReset("test".to_string())));
        assert!(policy.should_retry(&NetError::ServerUnavailable("test".to_string())));
        assert!(policy.should_retry(&NetError::ServerOverloaded("test".to_string())));

        // Non-transient errors should NOT be retried
        assert!(!policy.should_retry(&NetError::InvalidRequest("test".to_string())));
        assert!(!policy.should_retry(&NetError::AuthFailed("test".to_string())));
        assert!(!policy.should_retry(&NetError::InsufficientPermissions("test".to_string())));
        assert!(!policy.should_retry(&NetError::MalformedMessage("test".to_string())));
        assert!(!policy.should_retry(&NetError::ServerInternal("test".to_string())));
    }

    #[test]
    fn test_retry_policy_default_is_on_transient() {
        let policy = RetryPolicy::default();
        assert_eq!(policy, RetryPolicy::OnTransient);
    }

    // --- CompressionConfig tests ---

    #[test]
    fn test_compression_config_default() {
        let config = CompressionConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.algorithm, CompressionAlgorithm::Identity);
        assert!(config.to_tonic_encoding().is_none());
    }

    #[test]
    fn test_compression_config_gzip() {
        let config = CompressionConfig::gzip();
        assert!(config.enabled);
        assert_eq!(config.algorithm, CompressionAlgorithm::Gzip);
        assert!(config.to_tonic_encoding().is_some());
    }

    #[test]
    fn test_compression_identity_returns_none() {
        let config = CompressionConfig {
            enabled: true,
            algorithm: CompressionAlgorithm::Identity,
        };
        // Even if enabled, identity means no compression encoding
        assert!(config.to_tonic_encoding().is_none());
    }

    #[test]
    fn test_compression_disabled_returns_none() {
        let config = CompressionConfig {
            enabled: false,
            algorithm: CompressionAlgorithm::Gzip,
        };
        assert!(config.to_tonic_encoding().is_none());
    }

    // --- Client builder with new options ---

    #[tokio::test]
    async fn test_builder_with_retry() {
        let retry = RetryConfig {
            max_retries: 5,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_secs(5),
            backoff_multiplier: 1.5,
            policy: RetryPolicy::OnError,
        };

        let client = AqlClient::builder()
            .with_retry(retry)
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string())
            .build()
            .expect("builder should succeed");

        assert_eq!(client.retry_config().max_retries, 5);
        assert_eq!(
            client.retry_config().initial_backoff,
            Duration::from_millis(50)
        );
        assert_eq!(client.retry_config().policy, RetryPolicy::OnError);
    }

    #[tokio::test]
    async fn test_builder_with_compression() {
        let client = AqlClient::builder()
            .with_compression(CompressionConfig::gzip())
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string())
            .build()
            .expect("builder should succeed");

        assert!(client.compression_config().enabled);
        assert_eq!(
            client.compression_config().algorithm,
            CompressionAlgorithm::Gzip
        );
    }

    #[tokio::test]
    async fn test_builder_with_timeout() {
        let client = AqlClient::builder()
            .with_timeout(Duration::from_secs(60))
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string())
            .build()
            .expect("builder should succeed");

        assert_eq!(client.config.request_timeout, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_builder_full_chain() {
        let client = AqlClient::builder()
            .connect_timeout(Duration::from_secs(5))
            .request_timeout(Duration::from_secs(15))
            .keep_alive(true)
            .keep_alive_interval(Duration::from_secs(30))
            .min_pool_size(2)
            .max_pool_size(20)
            .idle_timeout(Duration::from_secs(120))
            .max_lifetime(Duration::from_secs(600))
            .health_check_interval(Duration::from_secs(10))
            .balancing_strategy(BalancingStrategy::RoundRobin)
            .circuit_breaker(true)
            .with_retry(RetryConfig {
                max_retries: 5,
                policy: RetryPolicy::OnTransient,
                ..Default::default()
            })
            .with_compression(CompressionConfig::gzip())
            .with_timeout(Duration::from_secs(20))
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string())
            .add_endpoint_with_weight("ep2".to_string(), "localhost:50052".to_string(), 3)
            .build()
            .expect("builder should succeed");

        assert_eq!(client.config.connect_timeout, Duration::from_secs(5));
        // with_timeout overrides request_timeout
        assert_eq!(client.config.request_timeout, Duration::from_secs(20));
        assert!(client.config.keep_alive);
        assert_eq!(client.retry_config().max_retries, 5);
        assert!(client.compression_config().enabled);
    }

    // --- Circuit breaker integration with retry ---

    #[tokio::test]
    async fn test_circuit_breaker_blocks_retries() {
        use crate::circuit_breaker::CircuitBreakerConfig;

        // Create a client with a very low failure threshold
        let cb_config = CircuitBreakerConfig {
            failure_threshold: 2,
            ..Default::default()
        };

        let cb = CircuitBreaker::with_config(cb_config);

        // Manually trip the circuit breaker
        cb.is_request_allowed().ok();
        cb.record_failure();
        cb.is_request_allowed().ok();
        cb.record_failure();

        // Circuit should now be open
        assert_eq!(cb.state(), crate::circuit_breaker::CircuitState::Open);

        // Verify that is_request_allowed returns error when circuit is open
        assert!(cb.is_request_allowed().is_err());
    }

    #[tokio::test]
    async fn test_circuit_breaker_enabled_in_builder() {
        let client = AqlClient::builder()
            .circuit_breaker(true)
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string())
            .build()
            .expect("builder should succeed");

        assert!(client.circuit_breaker.is_some());
    }

    #[tokio::test]
    async fn test_circuit_breaker_disabled_in_builder() {
        let client = AqlClient::builder()
            .circuit_breaker(false)
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string())
            .build()
            .expect("builder should succeed");

        assert!(client.circuit_breaker.is_none());
    }

    #[tokio::test]
    async fn test_default_client_has_circuit_breaker() {
        // Default pool config has enable_circuit_breaker = true
        let client = AqlClient::new();
        assert!(client.circuit_breaker.is_some());
    }

    // --- Compression algorithm tests ---

    #[test]
    fn test_compression_algorithm_default() {
        let algo = CompressionAlgorithm::default();
        assert_eq!(algo, CompressionAlgorithm::Identity);
    }

    #[test]
    fn test_compression_algorithm_variants() {
        assert_ne!(CompressionAlgorithm::Gzip, CompressionAlgorithm::Identity);
    }

    // --- Builder default tests ---

    #[tokio::test]
    async fn test_builder_default() {
        let builder = AqlClientBuilder::default();
        let client = builder.build().expect("default builder should succeed");
        assert_eq!(client.config.connect_timeout, Duration::from_secs(10));
        assert_eq!(client.config.request_timeout, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_client_default() {
        let client = AqlClient::default();
        assert_eq!(client.config.connect_timeout, Duration::from_secs(10));
    }

    // --- TlsClientConfig tests ---

    #[test]
    fn test_tls_config_default() {
        let config = TlsClientConfig::default();
        assert!(config.ca_cert_path.is_none());
        assert!(config.client_cert_path.is_none());
        assert!(config.client_key_path.is_none());
        assert!(config.domain_name.is_none());
        assert!(!config.skip_verification);
    }

    #[test]
    fn test_tls_config_with_ca_cert() {
        let config = TlsClientConfig::new().with_ca_cert("/tmp/ca.pem");
        assert_eq!(config.ca_cert_path, Some(PathBuf::from("/tmp/ca.pem")));
        assert!(config.client_cert_path.is_none());
        assert!(config.client_key_path.is_none());
    }

    #[test]
    fn test_tls_config_with_mtls() {
        let config =
            TlsClientConfig::new().with_client_identity("/tmp/client.pem", "/tmp/client.key");
        assert!(config.ca_cert_path.is_none());
        assert_eq!(
            config.client_cert_path,
            Some(PathBuf::from("/tmp/client.pem"))
        );
        assert_eq!(
            config.client_key_path,
            Some(PathBuf::from("/tmp/client.key"))
        );
    }

    #[test]
    fn test_tls_config_missing_key() {
        let config = TlsClientConfig {
            client_cert_path: Some(PathBuf::from("/tmp/client.pem")),
            client_key_path: None,
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        let err = result.expect_err("should be TlsError");
        assert!(
            err.to_string().contains("without client key"),
            "Error should mention missing key: {}",
            err
        );
    }

    #[test]
    fn test_tls_config_missing_cert() {
        let config = TlsClientConfig {
            client_cert_path: None,
            client_key_path: Some(PathBuf::from("/tmp/client.key")),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        let err = result.expect_err("should be TlsError");
        assert!(
            err.to_string().contains("without client certificate"),
            "Error should mention missing cert: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_builder_with_ca_cert() {
        // Builder chains the CA cert method correctly
        let builder = AqlClient::builder()
            .with_ca_cert("/tmp/test-ca.pem")
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string());

        // Verify the TLS config was set on the builder
        let tls_cfg = builder
            .tls_client_config
            .as_ref()
            .expect("TLS config should be set");
        assert_eq!(
            tls_cfg.ca_cert_path,
            Some(PathBuf::from("/tmp/test-ca.pem"))
        );
    }

    #[tokio::test]
    async fn test_builder_with_mtls() {
        let builder = AqlClient::builder()
            .with_mtls("/tmp/client.pem", "/tmp/client.key")
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string());

        let tls_cfg = builder
            .tls_client_config
            .as_ref()
            .expect("TLS config should be set");
        assert_eq!(
            tls_cfg.client_cert_path,
            Some(PathBuf::from("/tmp/client.pem"))
        );
        assert_eq!(
            tls_cfg.client_key_path,
            Some(PathBuf::from("/tmp/client.key"))
        );
    }

    #[tokio::test]
    async fn test_builder_with_full_config() {
        let tls_config = TlsClientConfig::new()
            .with_ca_cert("/tmp/ca.pem")
            .with_client_identity("/tmp/client.pem", "/tmp/client.key")
            .with_domain_name("example.com");

        let builder = AqlClient::builder()
            .with_tls_config(tls_config)
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string());

        let tls_cfg = builder
            .tls_client_config
            .as_ref()
            .expect("TLS config should be set");
        assert_eq!(tls_cfg.ca_cert_path, Some(PathBuf::from("/tmp/ca.pem")));
        assert_eq!(
            tls_cfg.client_cert_path,
            Some(PathBuf::from("/tmp/client.pem"))
        );
        assert_eq!(
            tls_cfg.client_key_path,
            Some(PathBuf::from("/tmp/client.key"))
        );
        assert_eq!(tls_cfg.domain_name, Some("example.com".to_string()));
    }

    #[test]
    fn test_tls_config_domain_name() {
        let config = TlsClientConfig::new().with_domain_name("my.server.com");
        assert_eq!(config.domain_name, Some("my.server.com".to_string()));
    }

    #[test]
    fn test_tls_config_skip_verification() {
        let config = TlsClientConfig::new().with_skip_verification(true);
        assert!(config.skip_verification);

        let config2 = TlsClientConfig::new().with_skip_verification(false);
        assert!(!config2.skip_verification);
    }

    #[test]
    fn test_tls_integration_invalid_cert() {
        let tmp_dir = std::env::temp_dir();
        let fake_cert_path = tmp_dir.join("nonexistent_test_cert_amaters.pem");

        let config = TlsClientConfig::new().with_ca_cert(&fake_cert_path);

        // Validation passes (paths are not checked during validation)
        assert!(config.validate().is_ok());

        // Building the tonic config fails because the file does not exist
        let result = config.build_tonic_tls_config();
        assert!(result.is_err());
        let err = result.expect_err("should fail for missing file");
        assert!(
            err.to_string().contains("Failed to read CA certificate"),
            "Error should mention CA cert read failure: {}",
            err
        );
    }

    #[test]
    fn test_tls_config_validate_valid_configs() {
        // Empty config is valid
        assert!(TlsClientConfig::default().validate().is_ok());

        // CA-only is valid
        assert!(
            TlsClientConfig::new()
                .with_ca_cert("/tmp/ca.pem")
                .validate()
                .is_ok()
        );

        // Full mTLS config is valid
        assert!(
            TlsClientConfig::new()
                .with_ca_cert("/tmp/ca.pem")
                .with_client_identity("/tmp/client.pem", "/tmp/client.key")
                .validate()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_builder_with_tls_domain() {
        let builder = AqlClient::builder()
            .with_tls_domain("custom.domain.io")
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string());

        let tls_cfg = builder
            .tls_client_config
            .as_ref()
            .expect("TLS config should be set");
        assert_eq!(tls_cfg.domain_name, Some("custom.domain.io".to_string()));
    }

    #[tokio::test]
    async fn test_builder_with_tls_skip_verification() {
        let builder = AqlClient::builder()
            .with_tls_skip_verification()
            .add_endpoint("ep1".to_string(), "localhost:50051".to_string());

        let tls_cfg = builder
            .tls_client_config
            .as_ref()
            .expect("TLS config should be set");
        assert!(tls_cfg.skip_verification);
    }

    #[test]
    fn test_tls_config_chaining() {
        // Test that all builder methods can be chained together
        let config = TlsClientConfig::new()
            .with_ca_cert("/tmp/ca.pem")
            .with_client_identity("/tmp/cert.pem", "/tmp/key.pem")
            .with_domain_name("example.com")
            .with_skip_verification(false);

        assert_eq!(config.ca_cert_path, Some(PathBuf::from("/tmp/ca.pem")));
        assert_eq!(
            config.client_cert_path,
            Some(PathBuf::from("/tmp/cert.pem"))
        );
        assert_eq!(config.client_key_path, Some(PathBuf::from("/tmp/key.pem")));
        assert_eq!(config.domain_name, Some("example.com".to_string()));
        assert!(!config.skip_verification);
        assert!(config.validate().is_ok());
    }
}
