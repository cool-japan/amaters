//! Cluster node configuration: TOML file + environment variable overrides + dynamic reload.
//!
//! # Usage
//!
//! ```rust,no_run
//! use amaters_cluster::config::NodeConfig;
//!
//! let cfg = NodeConfig::from_toml(r#"
//!     bind_addr = "0.0.0.0:7001"
//!     node_id = 1
//!     "#).expect("valid config");
//! assert_eq!(cfg.node_id, 1);
//! ```
//!
//! # Hot-reloadable fields
//!
//! The following fields can be updated at runtime without restarting the node
//! (accessible via [`NodeConfig::dynamic`]):
//!
//! - `heartbeat_interval_ms`
//! - `compaction_threshold`
//!
//! Fields that require a full restart:
//!
//! - `bind_addr`
//! - `node_id`
//! - `peers`
//! - `election_timeout_ms`
//! - `data_dir`
//! - `metrics_addr`

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Default helpers (used by serde)
// ---------------------------------------------------------------------------

fn default_heartbeat_ms() -> u64 {
    150
}

fn default_election_timeout_ms() -> u64 {
    300
}

fn default_compaction_threshold() -> usize {
    10_000
}

fn default_metrics_addr() -> String {
    "0.0.0.0:9091".to_string()
}

fn default_key_retention_count() -> usize {
    3
}

// ---------------------------------------------------------------------------
// NodeConfig
// ---------------------------------------------------------------------------

/// Full cluster node configuration.
///
/// Can be loaded from a TOML file with [`NodeConfig::load`] or from a TOML
/// string with [`NodeConfig::from_toml`].  After loading, call
/// [`NodeConfig::apply_env_overrides`] to layer environment variable
/// overrides on top.
///
/// # Field restart requirements
///
/// | Field | Hot-reloadable |
/// |-------|---------------|
/// | `heartbeat_interval_ms` | Yes (via [`DynamicConfig`]) |
/// | `compaction_threshold` | Yes (via [`DynamicConfig`]) |
/// | All other fields | No — requires restart |
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Bind address for Raft RPC server (e.g. `"0.0.0.0:7001"`).
    pub bind_addr: String,

    /// Node ID — must be unique across the cluster and `> 0`.
    pub node_id: u64,

    /// Peer addresses as `"node_id=addr"` strings, e.g. `["2=10.0.0.2:7001"]`.
    #[serde(default)]
    pub peers: Vec<String>,

    /// Raft heartbeat interval in milliseconds (default 150).
    ///
    /// Hot-reloadable.
    #[serde(default = "default_heartbeat_ms")]
    pub heartbeat_interval_ms: u64,

    /// Raft election timeout in milliseconds (default 300).
    ///
    /// Must be `>= 2 * heartbeat_interval_ms`.  Requires restart to change.
    #[serde(default = "default_election_timeout_ms")]
    pub election_timeout_ms: u64,

    /// Log compaction threshold: number of entries before triggering a
    /// snapshot (default 10 000).
    ///
    /// Hot-reloadable.
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: usize,

    /// Data directory for Raft log persistence.  `None` disables persistence.
    #[serde(default)]
    pub data_dir: Option<PathBuf>,

    /// Metrics HTTP endpoint address (default `"0.0.0.0:9091"`).
    ///
    /// Requires restart to change.
    #[serde(default = "default_metrics_addr")]
    pub metrics_addr: String,

    /// Optional automatic-rotation interval for the log-encryption master
    /// key, in seconds.  `None` disables time-based rotation; rotation can
    /// still be triggered manually via [`crate::key_rotation::KeyManager::rotate`].
    ///
    /// The background tokio task that drives time-based rotation is
    /// **deferred** to a future cycle; this field reserves the
    /// configuration surface for when it lands.
    #[serde(default)]
    pub key_rotation_interval_secs: Option<u64>,

    /// Number of [`crate::key_rotation::KeyVersion`]s the
    /// [`crate::key_rotation::KeyManager`] retains in its history (current
    /// and previous keys). Default 3. Lower values reclaim memory faster
    /// at the cost of decrypting fewer historical entries after rotation.
    #[serde(default = "default_key_retention_count")]
    pub key_retention_count: usize,
}

impl NodeConfig {
    /// Load from a TOML file, then layer environment variable overrides.
    ///
    /// Env vars override individual fields:
    ///
    /// | Env var | Field |
    /// |---------|-------|
    /// | `AMATERS_BIND_ADDR` | `bind_addr` |
    /// | `AMATERS_NODE_ID` | `node_id` |
    /// | `AMATERS_PEERS` | `peers` (comma-separated `id=addr` pairs) |
    /// | `AMATERS_HEARTBEAT_INTERVAL_MS` | `heartbeat_interval_ms` |
    /// | `AMATERS_ELECTION_TIMEOUT_MS` | `election_timeout_ms` |
    /// | `AMATERS_COMPACTION_THRESHOLD` | `compaction_threshold` |
    /// | `AMATERS_DATA_DIR` | `data_dir` |
    /// | `AMATERS_METRICS_ADDR` | `metrics_addr` |
    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path)?;
        let mut cfg = Self::from_toml(&raw)?;
        cfg.apply_env_overrides();
        Ok(cfg)
    }

    /// Parse from a TOML string directly.
    ///
    /// Useful in tests and when the configuration is provided via a
    /// secret store rather than a file on disk.  Does **not** apply env
    /// overrides; call [`apply_env_overrides`](Self::apply_env_overrides) if
    /// you also want those.
    pub fn from_toml(toml_str: &str) -> Result<Self, ConfigError> {
        let cfg: Self = toml::from_str(toml_str)?;
        Ok(cfg)
    }

    /// Apply environment variable overrides from the current process
    /// environment.
    ///
    /// Unset variables leave the corresponding field unchanged.
    pub fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("AMATERS_BIND_ADDR") {
            self.bind_addr = v;
        }
        if let Ok(v) = std::env::var("AMATERS_NODE_ID") {
            if let Ok(n) = v.parse::<u64>() {
                self.node_id = n;
            }
        }
        if let Ok(v) = std::env::var("AMATERS_PEERS") {
            // Comma-separated list of "node_id=addr" pairs
            self.peers = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Ok(v) = std::env::var("AMATERS_HEARTBEAT_INTERVAL_MS") {
            if let Ok(n) = v.parse::<u64>() {
                self.heartbeat_interval_ms = n;
            }
        }
        if let Ok(v) = std::env::var("AMATERS_ELECTION_TIMEOUT_MS") {
            if let Ok(n) = v.parse::<u64>() {
                self.election_timeout_ms = n;
            }
        }
        if let Ok(v) = std::env::var("AMATERS_COMPACTION_THRESHOLD") {
            if let Ok(n) = v.parse::<usize>() {
                self.compaction_threshold = n;
            }
        }
        if let Ok(v) = std::env::var("AMATERS_DATA_DIR") {
            self.data_dir = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("AMATERS_METRICS_ADDR") {
            self.metrics_addr = v;
        }
        if let Ok(v) = std::env::var("AMATERS_KEY_ROTATION_INTERVAL_SECS") {
            if let Ok(n) = v.parse::<u64>() {
                self.key_rotation_interval_secs = Some(n);
            }
        }
        if let Ok(v) = std::env::var("AMATERS_KEY_RETENTION_COUNT") {
            if let Ok(n) = v.parse::<usize>() {
                self.key_retention_count = n;
            }
        }
    }

    /// Extract the hot-reloadable subset of this configuration.
    ///
    /// The returned [`DynamicConfig`] can be stored in an
    /// `Arc<parking_lot::RwLock<DynamicConfig>>` and updated in place when
    /// the configuration is reloaded (e.g. on `SIGHUP` or via an admin RPC)
    /// without restarting the node.
    pub fn dynamic(&self) -> DynamicConfig {
        DynamicConfig {
            heartbeat_interval_ms: self.heartbeat_interval_ms,
            compaction_threshold: self.compaction_threshold,
        }
    }

    /// Validate configuration fields.
    ///
    /// Returns a list of [`ConfigError::Validation`] variants describing each
    /// detected problem.  An empty return value means the configuration is
    /// valid.
    pub fn validate(&self) -> Vec<ConfigError> {
        let mut errors = Vec::new();

        if self.bind_addr.is_empty() {
            errors.push(ConfigError::Validation {
                field: "bind_addr".to_string(),
                reason: "must not be empty".to_string(),
            });
        } else if !self.bind_addr.contains(':') {
            errors.push(ConfigError::Validation {
                field: "bind_addr".to_string(),
                reason: "must contain a ':' separator (e.g. \"0.0.0.0:7001\")".to_string(),
            });
        }

        if self.node_id == 0 {
            errors.push(ConfigError::Validation {
                field: "node_id".to_string(),
                reason: "must be > 0 (0 is reserved as a sentinel)".to_string(),
            });
        }

        if self.heartbeat_interval_ms == 0 {
            errors.push(ConfigError::Validation {
                field: "heartbeat_interval_ms".to_string(),
                reason: "must be > 0".to_string(),
            });
        }

        // election_timeout must be at least 2× the heartbeat interval so
        // that followers have a realistic chance to receive a heartbeat
        // before timing out.
        if self.heartbeat_interval_ms > 0
            && self.election_timeout_ms < 2 * self.heartbeat_interval_ms
        {
            errors.push(ConfigError::Validation {
                field: "election_timeout_ms".to_string(),
                reason: format!(
                    "must be >= 2 * heartbeat_interval_ms ({} >= {})",
                    self.election_timeout_ms,
                    2 * self.heartbeat_interval_ms,
                ),
            });
        }

        errors
    }
}

// ---------------------------------------------------------------------------
// DynamicConfig
// ---------------------------------------------------------------------------

/// The subset of [`NodeConfig`] that can be hot-reloaded at runtime.
///
/// Store this in an `Arc<parking_lot::RwLock<DynamicConfig>>` inside the
/// cluster node.  When the node receives a `SIGHUP` or an admin RPC requesting
/// a config reload, parse a new [`NodeConfig`] and replace the inner value:
///
/// ```rust,ignore
/// *dynamic_config.write() = new_node_config.dynamic();
/// ```
///
/// The Raft event loop reads from `Arc<RwLock<DynamicConfig>>` for the
/// heartbeat interval, so changes take effect on the **next tick** without
/// restarting the node.
#[derive(Debug, Clone)]
pub struct DynamicConfig {
    /// Raft heartbeat interval in milliseconds.
    pub heartbeat_interval_ms: u64,
    /// Log compaction threshold (entries before snapshot).
    pub compaction_threshold: usize,
}

// ---------------------------------------------------------------------------
// ConfigError
// ---------------------------------------------------------------------------

/// Errors that can occur while loading or validating a [`NodeConfig`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// The TOML source could not be parsed.
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    /// A field failed semantic validation.
    #[error("Validation error: field '{field}' — {reason}")]
    Validation { field: String, reason: String },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_TOML: &str = r#"
bind_addr = "0.0.0.0:7001"
node_id = 1
"#;

    /// Parsing a minimal TOML string fills required fields and applies
    /// serde defaults for optional ones.
    #[test]
    fn test_config_from_toml() {
        let cfg = NodeConfig::from_toml(MINIMAL_TOML).expect("valid TOML");

        assert_eq!(cfg.bind_addr, "0.0.0.0:7001");
        assert_eq!(cfg.node_id, 1);
        assert!(cfg.peers.is_empty());
        assert_eq!(cfg.heartbeat_interval_ms, 150);
        assert_eq!(cfg.election_timeout_ms, 300);
        assert_eq!(cfg.compaction_threshold, 10_000);
        assert!(cfg.data_dir.is_none());
        assert_eq!(cfg.metrics_addr, "0.0.0.0:9091");
        assert!(
            cfg.key_rotation_interval_secs.is_none(),
            "rotation interval defaults to None (manual rotation only)"
        );
        assert_eq!(cfg.key_retention_count, 3);
    }

    /// Parsing a TOML with explicit `[cluster.encryption]`-style fields
    /// applies them.
    #[test]
    fn test_config_encryption_fields_from_toml() {
        let toml = r#"
bind_addr = "0.0.0.0:7001"
node_id = 1
key_rotation_interval_secs = 86400
key_retention_count = 5
"#;
        let cfg = NodeConfig::from_toml(toml).expect("valid TOML");
        assert_eq!(cfg.key_rotation_interval_secs, Some(86_400));
        assert_eq!(cfg.key_retention_count, 5);
    }

    /// Parsing a fully-specified TOML overrides all default values.
    #[test]
    fn test_config_from_toml_full() {
        let toml = r#"
bind_addr = "127.0.0.1:8001"
node_id = 5
peers = ["2=10.0.0.2:7001", "3=10.0.0.3:7001"]
heartbeat_interval_ms = 50
election_timeout_ms = 200
compaction_threshold = 5000
data_dir = "/var/data/raft"
metrics_addr = "0.0.0.0:9999"
"#;
        let cfg = NodeConfig::from_toml(toml).expect("valid TOML");

        assert_eq!(cfg.bind_addr, "127.0.0.1:8001");
        assert_eq!(cfg.node_id, 5);
        assert_eq!(cfg.peers, vec!["2=10.0.0.2:7001", "3=10.0.0.3:7001"]);
        assert_eq!(cfg.heartbeat_interval_ms, 50);
        assert_eq!(cfg.election_timeout_ms, 200);
        assert_eq!(cfg.compaction_threshold, 5000);
        assert_eq!(cfg.data_dir, Some(PathBuf::from("/var/data/raft")));
        assert_eq!(cfg.metrics_addr, "0.0.0.0:9999");
    }

    /// Environment variables override the TOML-loaded values when
    /// `apply_env_overrides` is called.
    #[test]
    fn test_config_env_override() {
        // Isolate env-var changes from other tests by always cleaning up at the
        // end and using the unique `AMATERS_` prefix so other tests cannot see
        // these variables.
        let mut cfg = NodeConfig::from_toml(MINIMAL_TOML).expect("valid TOML");

        // Set env vars — unsafe in edition 2024 due to multi-thread unsafety.
        // SAFETY: All variable names are prefixed with `AMATERS_` which is
        //         unique to this test suite.  No other test in the binary sets
        //         these specific variables, so concurrent reads from other
        //         threads cannot observe a torn write.
        unsafe {
            std::env::set_var("AMATERS_BIND_ADDR", "10.0.0.1:9000");
            std::env::set_var("AMATERS_NODE_ID", "42");
            std::env::set_var("AMATERS_PEERS", "2=10.0.0.2:7001,3=10.0.0.3:7001");
            std::env::set_var("AMATERS_HEARTBEAT_INTERVAL_MS", "75");
            std::env::set_var("AMATERS_ELECTION_TIMEOUT_MS", "400");
            std::env::set_var("AMATERS_COMPACTION_THRESHOLD", "2000");
            std::env::set_var("AMATERS_METRICS_ADDR", "127.0.0.1:8080");
        }

        cfg.apply_env_overrides();

        assert_eq!(cfg.bind_addr, "10.0.0.1:9000");
        assert_eq!(cfg.node_id, 42);
        assert_eq!(cfg.peers, vec!["2=10.0.0.2:7001", "3=10.0.0.3:7001"]);
        assert_eq!(cfg.heartbeat_interval_ms, 75);
        assert_eq!(cfg.election_timeout_ms, 400);
        assert_eq!(cfg.compaction_threshold, 2000);
        assert_eq!(cfg.metrics_addr, "127.0.0.1:8080");

        // Clean up
        // SAFETY: Same as the set_var block above — unique AMATERS_ prefix
        //         ensures no concurrent readers observe this removal.
        unsafe {
            std::env::remove_var("AMATERS_BIND_ADDR");
            std::env::remove_var("AMATERS_NODE_ID");
            std::env::remove_var("AMATERS_PEERS");
            std::env::remove_var("AMATERS_HEARTBEAT_INTERVAL_MS");
            std::env::remove_var("AMATERS_ELECTION_TIMEOUT_MS");
            std::env::remove_var("AMATERS_COMPACTION_THRESHOLD");
            std::env::remove_var("AMATERS_METRICS_ADDR");
        }
    }

    /// A zero `node_id` must produce a validation error.
    #[test]
    fn test_config_validation_missing_field() {
        let toml = r#"
bind_addr = "0.0.0.0:7001"
node_id = 0
"#;
        let cfg = NodeConfig::from_toml(toml).expect("parse should succeed");
        let errors = cfg.validate();

        assert!(
            !errors.is_empty(),
            "expected validation errors for node_id = 0"
        );
        let has_node_id_error = errors
            .iter()
            .any(|e| matches!(e, ConfigError::Validation { field, .. } if field == "node_id"));
        assert!(
            has_node_id_error,
            "expected a Validation error for 'node_id'"
        );
    }

    /// When `election_timeout_ms < 2 * heartbeat_interval_ms`, validation must
    /// report an out-of-range error on `election_timeout_ms`.
    #[test]
    fn test_config_validation_out_of_range() {
        let toml = r#"
bind_addr = "0.0.0.0:7001"
node_id = 1
heartbeat_interval_ms = 200
election_timeout_ms = 300
"#;
        // 300 < 2 * 200 = 400 → must fail validation
        let cfg = NodeConfig::from_toml(toml).expect("parse should succeed");
        let errors = cfg.validate();

        assert!(
            !errors.is_empty(),
            "expected validation error: election_timeout_ms 300 < 2*200 = 400"
        );
        let has_timeout_error = errors.iter().any(|e| {
            matches!(e, ConfigError::Validation { field, .. } if field == "election_timeout_ms")
        });
        assert!(
            has_timeout_error,
            "expected a Validation error for 'election_timeout_ms'"
        );
    }

    /// A fully valid config must produce zero validation errors.
    #[test]
    fn test_config_validation_passes_for_valid_config() {
        let cfg = NodeConfig::from_toml(MINIMAL_TOML).expect("valid TOML");
        let errors = cfg.validate();
        assert!(
            errors.is_empty(),
            "expected no validation errors, got: {:?}",
            errors
        );
    }

    /// `dynamic()` returns the hot-reloadable subset with matching values.
    #[test]
    fn test_config_dynamic_extraction() {
        let toml = r#"
bind_addr = "0.0.0.0:7001"
node_id = 1
heartbeat_interval_ms = 100
compaction_threshold = 5000
"#;
        let cfg = NodeConfig::from_toml(toml).expect("valid TOML");
        let dyn_cfg = cfg.dynamic();

        assert_eq!(dyn_cfg.heartbeat_interval_ms, 100);
        assert_eq!(dyn_cfg.compaction_threshold, 5000);
    }

    /// Verifies that `NodeConfig::load` reads from an actual temp file.
    ///
    /// We verify the raw file-parse path (no env overrides) using
    /// `from_toml(file_contents)` so that a concurrently-running env-var test
    /// cannot contaminate this assertion.  A separate smoke-test path calls
    /// `NodeConfig::load` to ensure the function returns `Ok` for a valid file
    /// (we do not assert field values there because env overrides may be active).
    #[test]
    fn test_config_load_from_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("amaters_cluster_test_config_load.toml");
        std::fs::write(&path, MINIMAL_TOML).expect("write temp config");

        // Verify raw TOML parse from the file contents (no env override path)
        let raw = std::fs::read_to_string(&path).expect("read temp config");
        let cfg = NodeConfig::from_toml(&raw).expect("parse TOML from file");
        assert_eq!(cfg.bind_addr, "0.0.0.0:7001");
        assert_eq!(cfg.node_id, 1);

        // Verify that NodeConfig::load itself succeeds (env overrides are fine
        // here — we just check it doesn't error out).
        NodeConfig::load(&path).expect("load() must succeed for a valid file");

        // Clean up
        let _ = std::fs::remove_file(&path);
    }
}
