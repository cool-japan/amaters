//! Server configuration module
//!
//! This module handles configuration loading from multiple sources:
//! 1. Default values
//! 2. TOML configuration file
//! 3. Environment variables
//! 4. CLI arguments (highest priority)

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::{info, warn};

/// Configuration errors
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read configuration file: {0}")]
    ReadFile(#[from] std::io::Error),

    #[error("Failed to parse TOML: {0}")]
    ParseToml(#[from] toml::de::Error),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Invalid socket address: {0}")]
    InvalidAddress(#[from] std::net::AddrParseError),
}

pub type ConfigResult<T> = Result<T, ConfigError>;

/// Main server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Server settings
    pub server: ServerSettings,

    /// Storage settings
    pub storage: StorageSettings,

    /// Network settings
    pub network: NetworkSettings,

    /// Cluster settings (optional)
    #[serde(default)]
    pub cluster: Option<ClusterSettings>,

    /// Logging settings
    pub logging: LoggingSettings,

    /// Metrics settings
    pub metrics: MetricsSettings,

    /// Authentication settings
    #[serde(default)]
    pub auth: AuthSettings,

    /// Authorization settings
    #[serde(default)]
    pub authz: AuthorizationSettings,
}

/// Server-specific settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    /// Server bind address
    pub bind_address: String,

    /// Data directory
    pub data_dir: PathBuf,

    /// PID file location (for stop/status commands)
    #[serde(default = "default_pid_file")]
    pub pid_file: PathBuf,

    /// Maximum number of concurrent connections
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,

    /// Shutdown timeout
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout_secs: u64,
}

/// Storage engine settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSettings {
    /// Storage engine type (memory, lsm)
    #[serde(default = "default_storage_engine")]
    pub engine: String,

    /// Write-ahead log settings
    #[serde(default)]
    pub wal: WalSettings,

    /// Memtable size in MB
    #[serde(default = "default_memtable_size")]
    pub memtable_size_mb: usize,

    /// Block cache size in MB
    #[serde(default = "default_block_cache_size")]
    pub block_cache_size_mb: usize,

    /// Compaction settings
    #[serde(default)]
    pub compaction: CompactionSettings,
}

/// Write-ahead log settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalSettings {
    /// Enable WAL
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// WAL directory (relative to data_dir)
    #[serde(default = "default_wal_dir")]
    pub dir: PathBuf,

    /// WAL segment size in MB
    #[serde(default = "default_wal_segment_size")]
    pub segment_size_mb: usize,

    /// Sync mode (always, interval, none)
    #[serde(default = "default_sync_mode")]
    pub sync_mode: String,
}

/// Compaction settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSettings {
    /// Compaction strategy (leveled, tiered, universal)
    #[serde(default = "default_compaction_strategy")]
    pub strategy: String,

    /// Number of levels
    #[serde(default = "default_num_levels")]
    pub num_levels: usize,

    /// Level size multiplier
    #[serde(default = "default_level_multiplier")]
    pub level_multiplier: usize,

    /// Maximum number of concurrent compactions
    #[serde(default = "default_max_compactions")]
    pub max_concurrent: usize,
}

/// Network settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSettings {
    /// Enable TLS
    #[serde(default = "default_false")]
    pub tls_enabled: bool,

    /// TLS certificate file
    pub tls_cert: Option<PathBuf>,

    /// TLS key file
    pub tls_key: Option<PathBuf>,

    /// TLS CA file (for mTLS)
    pub tls_ca: Option<PathBuf>,

    /// Require client certificates (mTLS)
    #[serde(default = "default_false")]
    pub require_client_cert: bool,

    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_secs: u64,

    /// Keep-alive interval in seconds
    #[serde(default = "default_keepalive_interval")]
    pub keepalive_interval_secs: u64,
}

/// Cluster settings (Raft consensus)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterSettings {
    /// Enable clustering
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Node ID (must be unique in cluster)
    pub node_id: u64,

    /// Cluster peers (node_id:address)
    pub peers: Vec<String>,

    /// Raft heartbeat interval in milliseconds
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_ms: u64,

    /// Raft election timeout in milliseconds
    #[serde(default = "default_election_timeout")]
    pub election_timeout_ms: u64,
}

/// Logging settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingSettings {
    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Log format (json, pretty, compact)
    #[serde(default = "default_log_format")]
    pub format: String,

    /// Log to file
    #[serde(default = "default_false")]
    pub file_enabled: bool,

    /// Log file path
    pub file_path: Option<PathBuf>,

    /// Log rotation settings
    #[serde(default)]
    pub rotation: LogRotationSettings,
}

/// Log rotation settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRotationSettings {
    /// Enable rotation
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Max file size in MB
    #[serde(default = "default_log_max_size")]
    pub max_size_mb: usize,

    /// Max number of backup files
    #[serde(default = "default_log_max_backups")]
    pub max_backups: usize,
}

/// Metrics settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSettings {
    /// Enable metrics collection
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Metrics bind address
    #[serde(default = "default_metrics_address")]
    pub bind_address: String,

    /// Metrics export interval in seconds
    #[serde(default = "default_metrics_interval")]
    pub export_interval_secs: u64,
}

/// Authentication settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSettings {
    /// Enable authentication
    #[serde(default = "default_false")]
    pub enabled: bool,

    /// Allowed authentication methods
    #[serde(default = "default_auth_methods")]
    pub methods: Vec<String>,

    /// mTLS settings
    #[serde(default)]
    pub mtls: MtlsSettings,

    /// JWT settings
    #[serde(default)]
    pub jwt: JwtSettings,

    /// API key settings
    #[serde(default)]
    pub api_key: ApiKeySettings,

    /// Reject unauthenticated requests
    #[serde(default = "default_true")]
    pub reject_unauthenticated: bool,
}

/// mTLS authentication settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtlsSettings {
    /// Enable mTLS authentication
    #[serde(default = "default_false")]
    pub enabled: bool,

    /// Trusted CA certificates directory
    pub ca_certs_dir: Option<PathBuf>,

    /// Certificate revocation list (CRL) path
    pub crl_path: Option<PathBuf>,

    /// Verify client certificate CN matches user identity
    #[serde(default = "default_true")]
    pub verify_cn: bool,

    /// Allowed certificate organizations (empty = allow all)
    #[serde(default)]
    pub allowed_organizations: Vec<String>,
}

/// JWT authentication settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtSettings {
    /// Enable JWT authentication
    #[serde(default = "default_false")]
    pub enabled: bool,

    /// JWT secret key (for HMAC algorithms: HS256, HS384, HS512)
    pub secret: Option<String>,

    /// RSA public key path (for RS256, RS384, RS512)
    pub public_key_path: Option<PathBuf>,

    /// EC public key path (for ES256, ES384)
    pub ec_public_key_path: Option<PathBuf>,

    /// Ed25519 public key path (for EdDSA)
    pub ed_public_key_path: Option<PathBuf>,

    /// JWT algorithm (HS256, HS384, HS512, RS256, RS384, RS512, ES256, ES384, EdDSA)
    #[serde(default = "default_jwt_algorithm")]
    pub algorithm: String,

    /// Token expiration time in seconds
    #[serde(default = "default_jwt_expiration")]
    pub expiration_secs: u64,

    /// Issuer to verify
    pub issuer: Option<String>,

    /// Audience to verify
    pub audience: Option<String>,
}

/// API key authentication settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeySettings {
    /// Enable API key authentication
    #[serde(default = "default_false")]
    pub enabled: bool,

    /// API keys file path (JSON format)
    pub keys_file: Option<PathBuf>,

    /// API key header name
    #[serde(default = "default_api_key_header")]
    pub header_name: String,

    /// Hash API keys for storage
    #[serde(default = "default_true")]
    pub hash_keys: bool,
}

/// Authorization settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationSettings {
    /// Enable authorization
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Default role for authenticated users
    #[serde(default = "default_user_role")]
    pub default_role: String,

    /// Role definitions file (JSON/TOML)
    pub roles_file: Option<PathBuf>,

    /// Policy definitions file (JSON/TOML)
    pub policies_file: Option<PathBuf>,

    /// Enable collection-level permissions
    #[serde(default = "default_true")]
    pub collection_permissions: bool,

    /// Default permission mode (deny-by-default or allow-by-default)
    #[serde(default = "default_permission_mode")]
    pub default_mode: String,

    /// Enable audit logging for authorization decisions
    #[serde(default = "default_true")]
    pub audit_enabled: bool,

    /// Audit log file path
    pub audit_log_path: Option<PathBuf>,
}

// Default value functions
fn default_pid_file() -> PathBuf {
    PathBuf::from("/var/run/amaters-server.pid")
}

fn default_max_connections() -> usize {
    1000
}

fn default_shutdown_timeout() -> u64 {
    30
}

fn default_storage_engine() -> String {
    "lsm".to_string()
}

fn default_memtable_size() -> usize {
    64
}

fn default_block_cache_size() -> usize {
    256
}

fn default_wal_dir() -> PathBuf {
    PathBuf::from("wal")
}

fn default_wal_segment_size() -> usize {
    64
}

fn default_sync_mode() -> String {
    "interval".to_string()
}

fn default_compaction_strategy() -> String {
    "leveled".to_string()
}

fn default_num_levels() -> usize {
    7
}

fn default_level_multiplier() -> usize {
    10
}

fn default_max_compactions() -> usize {
    4
}

fn default_connection_timeout() -> u64 {
    30
}

fn default_keepalive_interval() -> u64 {
    60
}

fn default_heartbeat_interval() -> u64 {
    100
}

fn default_election_timeout() -> u64 {
    300
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "pretty".to_string()
}

fn default_log_max_size() -> usize {
    100
}

fn default_log_max_backups() -> usize {
    10
}

fn default_metrics_address() -> String {
    "127.0.0.1:9090".to_string()
}

fn default_metrics_interval() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_auth_methods() -> Vec<String> {
    vec!["mtls".to_string()]
}

fn default_jwt_algorithm() -> String {
    "HS256".to_string()
}

fn default_jwt_expiration() -> u64 {
    3600 // 1 hour
}

fn default_api_key_header() -> String {
    "X-API-Key".to_string()
}

fn default_user_role() -> String {
    "user".to_string()
}

fn default_permission_mode() -> String {
    "deny-by-default".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            server: ServerSettings {
                bind_address: "0.0.0.0:7878".to_string(),
                data_dir: PathBuf::from("./data"),
                pid_file: default_pid_file(),
                max_connections: default_max_connections(),
                shutdown_timeout_secs: default_shutdown_timeout(),
            },
            storage: StorageSettings {
                engine: default_storage_engine(),
                wal: WalSettings::default(),
                memtable_size_mb: default_memtable_size(),
                block_cache_size_mb: default_block_cache_size(),
                compaction: CompactionSettings::default(),
            },
            network: NetworkSettings {
                tls_enabled: false,
                tls_cert: None,
                tls_key: None,
                tls_ca: None,
                require_client_cert: false,
                connection_timeout_secs: default_connection_timeout(),
                keepalive_interval_secs: default_keepalive_interval(),
            },
            cluster: None,
            logging: LoggingSettings {
                level: default_log_level(),
                format: default_log_format(),
                file_enabled: false,
                file_path: None,
                rotation: LogRotationSettings::default(),
            },
            metrics: MetricsSettings {
                enabled: true,
                bind_address: default_metrics_address(),
                export_interval_secs: default_metrics_interval(),
            },
            auth: AuthSettings::default(),
            authz: AuthorizationSettings::default(),
        }
    }
}

impl Default for WalSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: default_wal_dir(),
            segment_size_mb: default_wal_segment_size(),
            sync_mode: default_sync_mode(),
        }
    }
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            strategy: default_compaction_strategy(),
            num_levels: default_num_levels(),
            level_multiplier: default_level_multiplier(),
            max_concurrent: default_max_compactions(),
        }
    }
}

impl Default for LogRotationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size_mb: default_log_max_size(),
            max_backups: default_log_max_backups(),
        }
    }
}

impl Default for AuthSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            methods: default_auth_methods(),
            mtls: MtlsSettings::default(),
            jwt: JwtSettings::default(),
            api_key: ApiKeySettings::default(),
            reject_unauthenticated: true,
        }
    }
}

impl Default for MtlsSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ca_certs_dir: None,
            crl_path: None,
            verify_cn: true,
            allowed_organizations: Vec::new(),
        }
    }
}

impl Default for JwtSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            secret: None,
            public_key_path: None,
            ec_public_key_path: None,
            ed_public_key_path: None,
            algorithm: default_jwt_algorithm(),
            expiration_secs: default_jwt_expiration(),
            issuer: None,
            audience: None,
        }
    }
}

impl Default for ApiKeySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            keys_file: None,
            header_name: default_api_key_header(),
            hash_keys: true,
        }
    }
}

impl Default for AuthorizationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            default_role: default_user_role(),
            roles_file: None,
            policies_file: None,
            collection_permissions: true,
            default_mode: default_permission_mode(),
            audit_enabled: true,
            audit_log_path: None,
        }
    }
}

impl ServerConfig {
    /// Load configuration from TOML file
    pub fn from_file(path: impl AsRef<Path>) -> ConfigResult<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: ServerConfig = toml::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    /// Load configuration with environment variable overrides
    pub fn from_file_with_env(path: impl AsRef<Path>) -> ConfigResult<Self> {
        let mut config = Self::from_file(path)?;
        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    /// Apply environment variable overrides
    pub fn apply_env_overrides(&mut self) {
        if let Ok(bind) = std::env::var("AMATERS_BIND_ADDRESS") {
            self.server.bind_address = bind;
        }
        if let Ok(data_dir) = std::env::var("AMATERS_DATA_DIR") {
            self.server.data_dir = PathBuf::from(data_dir);
        }
        if let Ok(log_level) = std::env::var("AMATERS_LOG_LEVEL") {
            self.logging.level = log_level;
        }
        if let Ok(tls_enabled) = std::env::var("AMATERS_TLS_ENABLED") {
            self.network.tls_enabled = tls_enabled.parse().unwrap_or(false);
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> ConfigResult<()> {
        // Validate bind address
        let _: SocketAddr = self
            .server
            .bind_address
            .parse()
            .map_err(|e| ConfigError::Validation(format!("Invalid bind address: {}", e)))?;

        // Validate data directory is not empty
        if self.server.data_dir.as_os_str().is_empty() {
            return Err(ConfigError::Validation(
                "Data directory cannot be empty".to_string(),
            ));
        }

        // Validate storage engine
        match self.storage.engine.as_str() {
            "memory" | "lsm" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "Invalid storage engine: {}. Must be 'memory' or 'lsm'",
                    other
                )));
            }
        }

        // Validate TLS configuration
        if self.network.tls_enabled {
            if self.network.tls_cert.is_none() {
                return Err(ConfigError::Validation(
                    "TLS enabled but no certificate file specified".to_string(),
                ));
            }
            if self.network.tls_key.is_none() {
                return Err(ConfigError::Validation(
                    "TLS enabled but no key file specified".to_string(),
                ));
            }
            if self.network.require_client_cert && self.network.tls_ca.is_none() {
                return Err(ConfigError::Validation(
                    "Client certificate required but no CA file specified".to_string(),
                ));
            }
        }

        // Validate cluster configuration
        if let Some(ref cluster) = self.cluster {
            if cluster.enabled && cluster.peers.is_empty() {
                return Err(ConfigError::Validation(
                    "Cluster enabled but no peers specified".to_string(),
                ));
            }
        }

        // Validate log level
        match self.logging.level.to_lowercase().as_str() {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "Invalid log level: {}. Must be one of: trace, debug, info, warn, error",
                    other
                )));
            }
        }

        // Validate metrics address
        let _: SocketAddr = self
            .metrics
            .bind_address
            .parse()
            .map_err(|e| ConfigError::Validation(format!("Invalid metrics address: {}", e)))?;

        // Validate authentication settings
        if self.auth.enabled {
            // Validate at least one auth method is enabled
            let has_enabled_method = (self.auth.mtls.enabled
                && self.auth.methods.contains(&"mtls".to_string()))
                || (self.auth.jwt.enabled && self.auth.methods.contains(&"jwt".to_string()))
                || (self.auth.api_key.enabled
                    && self.auth.methods.contains(&"api_key".to_string()));

            if !has_enabled_method {
                return Err(ConfigError::Validation(
                    "Authentication enabled but no valid auth methods configured".to_string(),
                ));
            }

            // Validate JWT settings
            if self.auth.jwt.enabled {
                match self.auth.jwt.algorithm.as_str() {
                    "HS256" => {
                        if self.auth.jwt.secret.is_none() {
                            return Err(ConfigError::Validation(
                                "JWT HS256 enabled but no secret key provided".to_string(),
                            ));
                        }
                    }
                    "RS256" => {
                        if self.auth.jwt.public_key_path.is_none() {
                            return Err(ConfigError::Validation(
                                "JWT RS256 enabled but no public key path provided".to_string(),
                            ));
                        }
                    }
                    other => {
                        return Err(ConfigError::Validation(format!(
                            "Invalid JWT algorithm: {}. Supported: HS256, RS256",
                            other
                        )));
                    }
                }
            }

            // Validate API key settings
            if self.auth.api_key.enabled && self.auth.api_key.keys_file.is_none() {
                return Err(ConfigError::Validation(
                    "API key auth enabled but no keys file specified".to_string(),
                ));
            }

            // Validate mTLS settings
            if self.auth.mtls.enabled && self.auth.mtls.ca_certs_dir.is_none() {
                return Err(ConfigError::Validation(
                    "mTLS enabled but no CA certificates directory specified".to_string(),
                ));
            }
        }

        // Validate authorization settings
        if self.authz.enabled {
            match self.authz.default_mode.as_str() {
                "deny-by-default" | "allow-by-default" => {}
                other => {
                    return Err(ConfigError::Validation(format!(
                        "Invalid authorization default mode: {}. Must be 'deny-by-default' or 'allow-by-default'",
                        other
                    )));
                }
            }
        }

        Ok(())
    }

    /// Get shutdown timeout as Duration
    pub fn shutdown_timeout(&self) -> Duration {
        Duration::from_secs(self.server.shutdown_timeout_secs)
    }

    /// Get connection timeout as Duration
    pub fn connection_timeout(&self) -> Duration {
        Duration::from_secs(self.network.connection_timeout_secs)
    }

    /// Get keepalive interval as Duration
    pub fn keepalive_interval(&self) -> Duration {
        Duration::from_secs(self.network.keepalive_interval_secs)
    }

    /// Save configuration to TOML file
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> ConfigResult<()> {
        let contents = toml::to_string_pretty(self)
            .map_err(|e| ConfigError::Validation(format!("Failed to serialize config: {}", e)))?;
        std::fs::write(path, contents)?;
        Ok(())
    }

    /// Generate example configuration file
    pub fn example() -> Self {
        Self::default()
    }
}

/// Identifies configuration sections that can be hot-reloaded without restart
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReloadableSection {
    /// Log level and format - always safe to reload
    Logging,
    /// Metrics export interval - always safe to reload
    Metrics,
    /// Compaction strategy parameters - safe between compaction runs
    Compaction,
    /// Rate limiting parameters - always safe to reload
    RateLimit,
}

impl std::fmt::Display for ReloadableSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReloadableSection::Logging => write!(f, "logging"),
            ReloadableSection::Metrics => write!(f, "metrics"),
            ReloadableSection::Compaction => write!(f, "compaction"),
            ReloadableSection::RateLimit => write!(f, "rate_limit"),
        }
    }
}

/// Identifies configuration sections that require a server restart
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NonReloadableSection {
    /// Server bind address requires restart
    BindAddress,
    /// Server port requires restart
    Port,
    /// TLS certificate path requires restart
    TlsCertPath,
    /// TLS key path requires restart
    TlsKeyPath,
    /// Storage engine type requires restart
    StorageEngine,
    /// Data directory requires restart
    DataDir,
    /// Cluster node ID requires restart
    ClusterNodeId,
}

impl std::fmt::Display for NonReloadableSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NonReloadableSection::BindAddress => write!(f, "bind_address"),
            NonReloadableSection::Port => write!(f, "port"),
            NonReloadableSection::TlsCertPath => write!(f, "tls_cert_path"),
            NonReloadableSection::TlsKeyPath => write!(f, "tls_key_path"),
            NonReloadableSection::StorageEngine => write!(f, "storage_engine"),
            NonReloadableSection::DataDir => write!(f, "data_dir"),
            NonReloadableSection::ClusterNodeId => write!(f, "cluster_node_id"),
        }
    }
}

/// Tracks which fields changed between two configurations
#[derive(Debug, Clone, Default)]
pub struct ConfigDiff {
    /// Reloadable sections that changed
    pub reloadable_changes: Vec<ReloadableSection>,
    /// Non-reloadable sections that changed (require restart)
    pub non_reloadable_changes: Vec<NonReloadableSection>,
}

impl ConfigDiff {
    /// Returns true if there are no changes
    pub fn is_empty(&self) -> bool {
        self.reloadable_changes.is_empty() && self.non_reloadable_changes.is_empty()
    }

    /// Returns true if any non-reloadable sections changed
    pub fn has_non_reloadable_changes(&self) -> bool {
        !self.non_reloadable_changes.is_empty()
    }
}

/// Compare two configs and produce a diff of what changed
pub fn diff(old: &ServerConfig, new: &ServerConfig) -> ConfigDiff {
    let mut result = ConfigDiff::default();

    // Check reloadable sections
    if old.logging.level != new.logging.level
        || old.logging.format != new.logging.format
        || old.logging.file_enabled != new.logging.file_enabled
        || old.logging.file_path != new.logging.file_path
        || old.logging.rotation.enabled != new.logging.rotation.enabled
        || old.logging.rotation.max_size_mb != new.logging.rotation.max_size_mb
        || old.logging.rotation.max_backups != new.logging.rotation.max_backups
    {
        result.reloadable_changes.push(ReloadableSection::Logging);
    }

    if old.metrics.export_interval_secs != new.metrics.export_interval_secs
        || old.metrics.enabled != new.metrics.enabled
    {
        result.reloadable_changes.push(ReloadableSection::Metrics);
    }

    if old.storage.compaction.strategy != new.storage.compaction.strategy
        || old.storage.compaction.num_levels != new.storage.compaction.num_levels
        || old.storage.compaction.level_multiplier != new.storage.compaction.level_multiplier
        || old.storage.compaction.max_concurrent != new.storage.compaction.max_concurrent
    {
        result
            .reloadable_changes
            .push(ReloadableSection::Compaction);
    }

    if old.server.max_connections != new.server.max_connections {
        result.reloadable_changes.push(ReloadableSection::RateLimit);
    }

    // Check non-reloadable sections
    if old.server.bind_address != new.server.bind_address {
        result
            .non_reloadable_changes
            .push(NonReloadableSection::BindAddress);
    }

    if old.server.data_dir != new.server.data_dir {
        result
            .non_reloadable_changes
            .push(NonReloadableSection::DataDir);
    }

    if old.storage.engine != new.storage.engine {
        result
            .non_reloadable_changes
            .push(NonReloadableSection::StorageEngine);
    }

    if old.network.tls_cert != new.network.tls_cert {
        result
            .non_reloadable_changes
            .push(NonReloadableSection::TlsCertPath);
    }

    if old.network.tls_key != new.network.tls_key {
        result
            .non_reloadable_changes
            .push(NonReloadableSection::TlsKeyPath);
    }

    if let (Some(old_cluster), Some(new_cluster)) = (&old.cluster, &new.cluster) {
        if old_cluster.node_id != new_cluster.node_id {
            result
                .non_reloadable_changes
                .push(NonReloadableSection::ClusterNodeId);
        }
    }

    result
}

/// Report of a configuration reload operation
#[derive(Debug, Clone)]
pub struct ReloadReport {
    /// Sections that were successfully updated
    pub sections_updated: Vec<ReloadableSection>,
    /// Non-reloadable sections that were skipped (would require restart)
    pub sections_skipped: Vec<NonReloadableSection>,
    /// Errors encountered during reload
    pub errors: Vec<String>,
    /// Whether the reload was overall successful
    pub success: bool,
}

impl std::fmt::Display for ReloadReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.success {
            write!(f, "Config reload successful. ")?;
        } else {
            write!(f, "Config reload failed. ")?;
        }
        if !self.sections_updated.is_empty() {
            write!(f, "Updated: ")?;
            for (i, s) in self.sections_updated.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", s)?;
            }
            write!(f, ". ")?;
        }
        if !self.sections_skipped.is_empty() {
            write!(f, "Skipped (restart required): ")?;
            for (i, s) in self.sections_skipped.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", s)?;
            }
            write!(f, ". ")?;
        }
        for err in &self.errors {
            write!(f, "Error: {}. ", err)?;
        }
        Ok(())
    }
}

/// Wrapper around `ServerConfig` that supports hot-reloading
///
/// Uses `Arc<RwLock<ServerConfig>>` so that readers can access the config
/// concurrently, and reloads atomically swap the inner config.
#[derive(Clone)]
pub struct ReloadableConfig {
    inner: Arc<RwLock<ServerConfig>>,
    /// Path to the config file (used for SIGHUP reload)
    config_path: Arc<RwLock<Option<PathBuf>>>,
}

impl ReloadableConfig {
    /// Create a new reloadable config from an existing config
    pub fn new(config: ServerConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(config)),
            config_path: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a new reloadable config from a file
    pub fn from_file(path: &str) -> ConfigResult<Self> {
        let config = ServerConfig::from_file(path)?;
        let rc = Self::new(config);
        *rc.config_path.write() = Some(PathBuf::from(path));
        Ok(rc)
    }

    /// Set the config file path (for future reloads)
    pub fn set_config_path(&self, path: PathBuf) {
        *self.config_path.write() = Some(path);
    }

    /// Get a read guard to the current configuration
    pub fn read(&self) -> parking_lot::RwLockReadGuard<'_, ServerConfig> {
        self.inner.read()
    }

    /// Get a clone of the current configuration
    pub fn snapshot(&self) -> ServerConfig {
        self.inner.read().clone()
    }

    /// Reload configuration from a file path
    ///
    /// Parses the new config, validates it, computes a diff, and applies
    /// only the reloadable sections. Non-reloadable changes are skipped
    /// with warnings. If validation fails, the old config is preserved.
    pub fn reload_from_file(&self, path: &str) -> ConfigResult<ReloadReport> {
        // Parse new config from file
        let contents = std::fs::read_to_string(path)?;
        let new_config: ServerConfig = toml::from_str(&contents)?;

        // Validate before applying
        if let Err(e) = new_config.validate() {
            return Ok(ReloadReport {
                sections_updated: Vec::new(),
                sections_skipped: Vec::new(),
                errors: vec![format!("Validation failed: {}", e)],
                success: false,
            });
        }

        self.apply_reload(new_config)
    }

    /// Reload from the stored config path (used by SIGHUP handler)
    pub fn reload_from_stored_path(&self) -> ConfigResult<ReloadReport> {
        let path = self.config_path.read().clone();
        match path {
            Some(p) => {
                let path_str = p.to_string_lossy().to_string();
                self.reload_from_file(&path_str)
            }
            None => Ok(ReloadReport {
                sections_updated: Vec::new(),
                sections_skipped: Vec::new(),
                errors: vec!["No config file path set for reload".to_string()],
                success: false,
            }),
        }
    }

    /// Apply a new config, returning a reload report
    fn apply_reload(&self, new_config: ServerConfig) -> ConfigResult<ReloadReport> {
        let mut report = ReloadReport {
            sections_updated: Vec::new(),
            sections_skipped: Vec::new(),
            errors: Vec::new(),
            success: true,
        };

        let config_diff = {
            let current = self.inner.read();
            diff(&current, &new_config)
        };

        if config_diff.is_empty() {
            info!("Config reload: no changes detected");
            return Ok(report);
        }

        // Warn about non-reloadable changes
        for section in &config_diff.non_reloadable_changes {
            warn!(
                "Config reload: section '{}' changed but requires restart - skipping",
                section
            );
            report.sections_skipped.push(*section);
        }

        // Apply reloadable changes atomically
        if !config_diff.reloadable_changes.is_empty() {
            let mut current = self.inner.write();

            for section in &config_diff.reloadable_changes {
                match section {
                    ReloadableSection::Logging => {
                        current.logging = new_config.logging.clone();
                        info!("Config reload: updated logging settings");
                    }
                    ReloadableSection::Metrics => {
                        // Only update export_interval and enabled, not bind_address
                        current.metrics.export_interval_secs =
                            new_config.metrics.export_interval_secs;
                        current.metrics.enabled = new_config.metrics.enabled;
                        info!("Config reload: updated metrics settings");
                    }
                    ReloadableSection::Compaction => {
                        current.storage.compaction = new_config.storage.compaction.clone();
                        info!("Config reload: updated compaction settings");
                    }
                    ReloadableSection::RateLimit => {
                        current.server.max_connections = new_config.server.max_connections;
                        info!("Config reload: updated rate limit settings");
                    }
                }
                report.sections_updated.push(*section);
            }
        }

        Ok(report)
    }

    /// Manual reload trigger (useful on non-Unix platforms or for testing)
    pub fn manual_reload(&self) -> ConfigResult<ReloadReport> {
        self.reload_from_stored_path()
    }
}

impl std::fmt::Debug for ReloadableConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReloadableConfig")
            .field("config", &*self.inner.read())
            .field("config_path", &*self.config_path.read())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_default_config() {
        let config = ServerConfig::default();
        assert_eq!(config.server.bind_address, "0.0.0.0:7878");
        assert_eq!(config.storage.engine, "lsm");
        assert_eq!(config.logging.level, "info");
    }

    #[test]
    fn test_config_validation() {
        let config = ServerConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_invalid_bind_address() {
        let mut config = ServerConfig::default();
        config.server.bind_address = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_storage_engine() {
        let mut config = ServerConfig::default();
        config.storage.engine = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_tls_validation() {
        let mut config = ServerConfig::default();
        config.network.tls_enabled = true;
        assert!(config.validate().is_err()); // No cert/key specified
    }

    #[test]
    fn test_env_overrides() {
        unsafe {
            env::set_var("AMATERS_BIND_ADDRESS", "127.0.0.1:9999");
            env::set_var("AMATERS_LOG_LEVEL", "debug");
        }

        let mut config = ServerConfig::default();
        config.apply_env_overrides();

        assert_eq!(config.server.bind_address, "127.0.0.1:9999");
        assert_eq!(config.logging.level, "debug");

        unsafe {
            env::remove_var("AMATERS_BIND_ADDRESS");
            env::remove_var("AMATERS_LOG_LEVEL");
        }
    }

    #[test]
    fn test_save_and_load() {
        let temp_dir = env::temp_dir();
        let config_path = temp_dir.join("test_config.toml");

        let config = ServerConfig::default();
        config
            .save_to_file(&config_path)
            .expect("Failed to save config");

        let loaded = ServerConfig::from_file(&config_path).expect("Failed to load config");
        assert_eq!(config.server.bind_address, loaded.server.bind_address);

        std::fs::remove_file(&config_path).ok();
    }

    // --- Reload tests ---

    /// Helper to save a config to a temp file and return the path
    fn save_temp_config(config: &ServerConfig, name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("amaters_reload_test_{}.toml", name));
        config
            .save_to_file(&path)
            .expect("Failed to save temp config");
        path
    }

    #[test]
    fn test_reload_logging_section() {
        let config = ServerConfig::default();
        let path = save_temp_config(&config, "reload_logging");

        let reloadable = ReloadableConfig::new(config);
        reloadable.set_config_path(path.clone());

        // Modify the file to change logging
        let mut new_config = reloadable.snapshot();
        new_config.logging.level = "debug".to_string();
        new_config.logging.format = "json".to_string();
        new_config
            .save_to_file(&path)
            .expect("Failed to save modified config");

        let report = reloadable
            .reload_from_file(path.to_str().expect("path should be valid utf-8"))
            .expect("Reload should succeed");

        assert!(report.success);
        assert!(
            report
                .sections_updated
                .contains(&ReloadableSection::Logging)
        );
        assert_eq!(reloadable.read().logging.level, "debug");
        assert_eq!(reloadable.read().logging.format, "json");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_reload_metrics_section() {
        let config = ServerConfig::default();
        let path = save_temp_config(&config, "reload_metrics");

        let reloadable = ReloadableConfig::new(config);

        let mut new_config = reloadable.snapshot();
        new_config.metrics.export_interval_secs = 120;
        new_config
            .save_to_file(&path)
            .expect("Failed to save modified config");

        let report = reloadable
            .reload_from_file(path.to_str().expect("path should be valid utf-8"))
            .expect("Reload should succeed");

        assert!(report.success);
        assert!(
            report
                .sections_updated
                .contains(&ReloadableSection::Metrics)
        );
        assert_eq!(reloadable.read().metrics.export_interval_secs, 120);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_non_reloadable_section_skipped() {
        let config = ServerConfig::default();
        let path = save_temp_config(&config, "reload_non_reloadable");

        let reloadable = ReloadableConfig::new(config);

        let mut new_config = reloadable.snapshot();
        // Change bind address (non-reloadable)
        new_config.server.bind_address = "127.0.0.1:9999".to_string();
        // Also change logging (reloadable) to verify partial apply
        new_config.logging.level = "warn".to_string();
        new_config
            .save_to_file(&path)
            .expect("Failed to save modified config");

        let report = reloadable
            .reload_from_file(path.to_str().expect("path should be valid utf-8"))
            .expect("Reload should succeed");

        assert!(report.success);
        // Logging should be updated
        assert!(
            report
                .sections_updated
                .contains(&ReloadableSection::Logging)
        );
        assert_eq!(reloadable.read().logging.level, "warn");
        // Bind address should be skipped (old value preserved)
        assert!(
            report
                .sections_skipped
                .contains(&NonReloadableSection::BindAddress)
        );
        assert_eq!(reloadable.read().server.bind_address, "0.0.0.0:7878");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_invalid_config_rejected() {
        let config = ServerConfig::default();
        let path = save_temp_config(&config, "reload_invalid");

        let reloadable = ReloadableConfig::new(config);

        // Write invalid config (bad bind address)
        let mut new_config = reloadable.snapshot();
        new_config.server.bind_address = "not-an-address".to_string();
        // Manually write TOML since save_to_file doesn't validate
        let contents = toml::to_string_pretty(&new_config).expect("Failed to serialize config");
        std::fs::write(&path, contents).expect("Failed to write config");

        let report = reloadable
            .reload_from_file(path.to_str().expect("path should be valid utf-8"))
            .expect("Reload should return report");

        assert!(!report.success);
        assert!(!report.errors.is_empty());
        // Old config should be preserved
        assert_eq!(reloadable.read().server.bind_address, "0.0.0.0:7878");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_config_diff_detection() {
        let old = ServerConfig::default();
        let mut new = old.clone();

        // No changes
        let d = diff(&old, &new);
        assert!(d.is_empty());

        // Change logging
        new.logging.level = "error".to_string();
        let d = diff(&old, &new);
        assert!(d.reloadable_changes.contains(&ReloadableSection::Logging));
        assert!(!d.has_non_reloadable_changes());

        // Change bind address
        new.server.bind_address = "127.0.0.1:1234".to_string();
        let d = diff(&old, &new);
        assert!(d.has_non_reloadable_changes());
        assert!(
            d.non_reloadable_changes
                .contains(&NonReloadableSection::BindAddress)
        );

        // Change compaction
        new.storage.compaction.strategy = "tiered".to_string();
        let d = diff(&old, &new);
        assert!(
            d.reloadable_changes
                .contains(&ReloadableSection::Compaction)
        );

        // Change max_connections (rate limit)
        new.server.max_connections = 5000;
        let d = diff(&old, &new);
        assert!(d.reloadable_changes.contains(&ReloadableSection::RateLimit));
    }

    #[test]
    fn test_reload_report_contents() {
        let config = ServerConfig::default();
        let path = save_temp_config(&config, "reload_report");

        let reloadable = ReloadableConfig::new(config);

        // Change multiple sections
        let mut new_config = reloadable.snapshot();
        new_config.logging.level = "trace".to_string();
        new_config.metrics.export_interval_secs = 30;
        new_config.server.bind_address = "127.0.0.1:5555".to_string();
        new_config
            .save_to_file(&path)
            .expect("Failed to save modified config");

        let report = reloadable
            .reload_from_file(path.to_str().expect("path should be valid utf-8"))
            .expect("Reload should succeed");

        assert!(report.success);
        assert_eq!(report.sections_updated.len(), 2); // Logging + Metrics
        assert_eq!(report.sections_skipped.len(), 1); // BindAddress
        assert!(report.errors.is_empty());

        // Verify Display impl works
        let display = format!("{}", report);
        assert!(display.contains("Updated"));
        assert!(display.contains("Skipped"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_concurrent_reads_during_reload() {
        let config = ServerConfig::default();
        let path = save_temp_config(&config, "reload_concurrent");

        let reloadable = ReloadableConfig::new(config);

        // Spawn multiple reader threads
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let rc = reloadable.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        let _level = rc.read().logging.level.clone();
                    }
                })
            })
            .collect();

        // Perform reloads while readers are active
        let mut new_config = reloadable.snapshot();
        new_config.logging.level = "debug".to_string();
        new_config
            .save_to_file(&path)
            .expect("Failed to save modified config");

        let _report = reloadable
            .reload_from_file(path.to_str().expect("path should be valid utf-8"))
            .expect("Reload should succeed");

        for h in handles {
            h.join().expect("Reader thread should not panic");
        }

        assert_eq!(reloadable.read().logging.level, "debug");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_multiple_sequential_reloads() {
        let config = ServerConfig::default();
        let path = save_temp_config(&config, "reload_sequential");

        let reloadable = ReloadableConfig::new(config);

        let levels = ["debug", "warn", "error", "trace", "info"];
        for level in &levels {
            let mut new_config = reloadable.snapshot();
            new_config.logging.level = level.to_string();
            new_config
                .save_to_file(&path)
                .expect("Failed to save modified config");

            let report = reloadable
                .reload_from_file(path.to_str().expect("path should be valid utf-8"))
                .expect("Reload should succeed");

            assert!(report.success);
            assert_eq!(reloadable.read().logging.level, *level);
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_reload_no_stored_path() {
        let config = ServerConfig::default();
        let reloadable = ReloadableConfig::new(config);

        let report = reloadable
            .reload_from_stored_path()
            .expect("Should return report");

        assert!(!report.success);
        assert!(!report.errors.is_empty());
    }

    #[test]
    fn test_reloadable_config_from_file() {
        let config = ServerConfig::default();
        let path = save_temp_config(&config, "reload_from_file");

        let reloadable =
            ReloadableConfig::from_file(path.to_str().expect("path should be valid utf-8"))
                .expect("Should load from file");

        assert_eq!(reloadable.read().server.bind_address, "0.0.0.0:7878");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_manual_reload() {
        let config = ServerConfig::default();
        let path = save_temp_config(&config, "reload_manual");

        let reloadable = ReloadableConfig::new(config);
        reloadable.set_config_path(path.clone());

        let mut new_config = reloadable.snapshot();
        new_config.logging.level = "error".to_string();
        new_config
            .save_to_file(&path)
            .expect("Failed to save modified config");

        let report = reloadable
            .manual_reload()
            .expect("Manual reload should succeed");
        assert!(report.success);
        assert_eq!(reloadable.read().logging.level, "error");

        std::fs::remove_file(&path).ok();
    }
}
