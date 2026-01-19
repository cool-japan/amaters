//! Client configuration types

use std::path::PathBuf;
use std::time::Duration;

/// Client configuration
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Server address (e.g., "http://localhost:50051")
    pub server_addr: String,

    /// Connection timeout
    pub connect_timeout: Duration,

    /// Request timeout
    pub request_timeout: Duration,

    /// Enable keep-alive
    pub keep_alive: bool,

    /// Keep-alive interval
    pub keep_alive_interval: Duration,

    /// Keep-alive timeout
    pub keep_alive_timeout: Duration,

    /// Maximum number of connections in pool
    pub max_connections: usize,

    /// Connection idle timeout
    pub idle_timeout: Duration,

    /// Enable TLS
    pub tls_enabled: bool,

    /// TLS configuration
    pub tls_config: Option<TlsConfig>,

    /// Retry configuration
    pub retry_config: RetryConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_addr: "http://localhost:50051".to_string(),
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            keep_alive: true,
            keep_alive_interval: Duration::from_secs(60),
            keep_alive_timeout: Duration::from_secs(20),
            max_connections: 10,
            idle_timeout: Duration::from_secs(300),
            tls_enabled: false,
            tls_config: None,
            retry_config: RetryConfig::default(),
        }
    }
}

impl ClientConfig {
    /// Create a new client configuration with server address
    pub fn new(server_addr: impl Into<String>) -> Self {
        Self {
            server_addr: server_addr.into(),
            ..Default::default()
        }
    }

    /// Set connection timeout
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set request timeout
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Set maximum connections
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Enable TLS with configuration
    pub fn with_tls(mut self, tls_config: TlsConfig) -> Self {
        self.tls_enabled = true;
        self.tls_config = Some(tls_config);
        self
    }

    /// Set retry configuration
    pub fn with_retry_config(mut self, retry_config: RetryConfig) -> Self {
        self.retry_config = retry_config;
        self
    }

    /// Disable keep-alive
    pub fn without_keep_alive(mut self) -> Self {
        self.keep_alive = false;
        self
    }
}

/// TLS configuration
#[derive(Debug, Clone, Default)]
pub struct TlsConfig {
    /// Path to CA certificate file
    pub ca_cert_path: Option<PathBuf>,

    /// Path to client certificate file (for mTLS)
    pub client_cert_path: Option<PathBuf>,

    /// Path to client key file (for mTLS)
    pub client_key_path: Option<PathBuf>,

    /// Domain name for SNI
    pub domain_name: Option<String>,

    /// Accept invalid certificates (for testing only)
    pub accept_invalid_certs: bool,
}

impl TlsConfig {
    /// Create a new TLS configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set CA certificate path
    pub fn with_ca_cert(mut self, path: impl Into<PathBuf>) -> Self {
        self.ca_cert_path = Some(path.into());
        self
    }

    /// Set client certificate and key for mTLS
    pub fn with_client_cert(
        mut self,
        cert_path: impl Into<PathBuf>,
        key_path: impl Into<PathBuf>,
    ) -> Self {
        self.client_cert_path = Some(cert_path.into());
        self.client_key_path = Some(key_path.into());
        self
    }

    /// Set domain name for SNI
    pub fn with_domain_name(mut self, domain: impl Into<String>) -> Self {
        self.domain_name = Some(domain.into());
        self
    }

    /// Accept invalid certificates (for testing only)
    pub fn accept_invalid_certs(mut self) -> Self {
        self.accept_invalid_certs = true;
        self
    }
}

/// Retry configuration
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: usize,

    /// Initial backoff duration
    pub initial_backoff: Duration,

    /// Maximum backoff duration
    pub max_backoff: Duration,

    /// Backoff multiplier
    pub backoff_multiplier: f64,

    /// Enable jitter for backoff
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }
}

impl RetryConfig {
    /// Create a new retry configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum retries
    pub fn with_max_retries(mut self, max: usize) -> Self {
        self.max_retries = max;
        self
    }

    /// Set initial backoff
    pub fn with_initial_backoff(mut self, backoff: Duration) -> Self {
        self.initial_backoff = backoff;
        self
    }

    /// Disable retries
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Calculate backoff duration for attempt
    pub fn backoff_duration(&self, attempt: usize) -> Duration {
        if attempt == 0 {
            return Duration::from_secs(0);
        }

        let base = self.initial_backoff.as_millis() as f64
            * self.backoff_multiplier.powi((attempt - 1) as i32);
        let backoff = Duration::from_millis(base.min(self.max_backoff.as_millis() as f64) as u64);

        if self.jitter {
            // Add jitter: random value between 0.5 and 1.5 times the backoff
            let jitter_factor = 0.5 + (attempt % 10) as f64 / 10.0; // Simple deterministic jitter
            Duration::from_millis((backoff.as_millis() as f64 * jitter_factor) as u64)
        } else {
            backoff
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ClientConfig::default();
        assert_eq!(config.server_addr, "http://localhost:50051");
        assert_eq!(config.max_connections, 10);
        assert!(config.keep_alive);
    }

    #[test]
    fn test_config_builder() {
        let config = ClientConfig::new("http://example.com:50051")
            .with_connect_timeout(Duration::from_secs(5))
            .with_max_connections(20)
            .without_keep_alive();

        assert_eq!(config.server_addr, "http://example.com:50051");
        assert_eq!(config.connect_timeout, Duration::from_secs(5));
        assert_eq!(config.max_connections, 20);
        assert!(!config.keep_alive);
    }

    #[test]
    fn test_tls_config() {
        let tls = TlsConfig::new()
            .with_ca_cert("/path/to/ca.pem")
            .with_domain_name("example.com");

        assert!(tls.ca_cert_path.is_some());
        assert_eq!(tls.domain_name, Some("example.com".to_string()));
    }

    #[test]
    fn test_retry_config() {
        let retry = RetryConfig::default();
        assert_eq!(retry.max_retries, 3);

        let backoff1 = retry.backoff_duration(1);
        let backoff2 = retry.backoff_duration(2);
        assert!(backoff2 > backoff1);
    }

    #[test]
    fn test_no_retry() {
        let retry = RetryConfig::no_retry();
        assert_eq!(retry.max_retries, 0);
    }
}
