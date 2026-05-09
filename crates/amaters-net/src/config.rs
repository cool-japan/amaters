//! TOML-based configuration for [`crate::server::AqlServerBuilder`].
//!
//! `NetConfig` is a deserializable view of every builder knob that controls
//! the gRPC service: bind address, TLS enable + paths, Prometheus metrics
//! address, request/response logging verbosity + slow threshold, rate-limit
//! QPS, and JWT secret path for bearer-token auth.
//!
//! # Layering
//!
//! Config values are layered in this priority order (later wins):
//!
//! 1. Hard-coded builder defaults (when no config is loaded)
//! 2. TOML file values ([`NetConfig::from_path`])
//! 3. Environment variables ([`NetConfig::merge_env`] / [`NetConfig::load_layered`])
//! 4. Explicit builder method calls (`builder.with_logging(...)` after `apply_to`)
//!
//! Every field is `Option<T>`; a partial TOML never overwrites unset fields.
//!
//! # TOML schema
//!
//! ```toml
//! [net]
//! bind_addr = "0.0.0.0:50051"
//!
//! [net.tls]
//! enabled = true
//! cert_path = "certs/server.pem"
//! key_path = "certs/server.key"
//!
//! [net.metrics]
//! addr = "127.0.0.1:9091"
//!
//! [net.logging]
//! verbosity = "brief"        # off | brief | detailed
//! slow_threshold_ms = 100
//!
//! [net.rate_limit]
//! qps = 1000.0
//!
//! [net.auth]
//! jwt_secret_path = "secrets/jwt.key"
//! ```
//!
//! # Path resolution
//!
//! Cert/key paths and JWT secret paths inside the TOML are resolved relative to
//! the parent directory of the TOML file itself (consistent with most server
//! configs).  Absolute paths are passed through untouched.
//!
//! # Environment overrides
//!
//! When [`NetConfig::merge_env`] or [`NetConfig::load_layered`] is used, the
//! following variables override TOML values:
//!
//! | Env var                          | Field                          |
//! |----------------------------------|--------------------------------|
//! | `AMATERS_NET_BIND_ADDR`           | `net.bind_addr`                |
//! | `AMATERS_NET_TLS_ENABLED`         | `net.tls.enabled`              |
//! | `AMATERS_NET_TLS_CERT_PATH`       | `net.tls.cert_path`            |
//! | `AMATERS_NET_TLS_KEY_PATH`        | `net.tls.key_path`             |
//! | `AMATERS_NET_METRICS_ADDR`        | `net.metrics.addr`             |
//! | `AMATERS_NET_LOG_VERBOSITY`       | `net.logging.verbosity`        |
//! | `AMATERS_NET_SLOW_THRESHOLD_MS`   | `net.logging.slow_threshold_ms`|
//! | `AMATERS_NET_RATE_LIMIT_QPS`      | `net.rate_limit.qps`           |
//! | `AMATERS_NET_JWT_SECRET_PATH`     | `net.auth.jwt_secret_path`     |

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{NetError, NetResult};
use crate::logging_layer::LogVerbosity;
use crate::server::AqlServerBuilder;
use amaters_core::traits::StorageEngine;

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

/// Top-level network-layer configuration loaded from a TOML file.
///
/// All sections are optional; missing sections default to `None` and leave the
/// corresponding builder values untouched.  Use [`NetConfig::from_path`] to
/// load and [`NetConfig::apply_to`] to fold the values into a builder.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct NetConfig {
    /// `[net]` section.
    #[serde(default)]
    pub net: NetSection,
}

/// `[net]` body — bind address plus nested sections.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct NetSection {
    /// gRPC server bind address (e.g. `"0.0.0.0:50051"`).
    pub bind_addr: Option<SocketAddr>,
    /// `[net.tls]` subsection.
    #[serde(default)]
    pub tls: TlsSection,
    /// `[net.metrics]` subsection.
    #[serde(default)]
    pub metrics: MetricsSection,
    /// `[net.logging]` subsection.
    #[serde(default)]
    pub logging: LoggingSection,
    /// `[net.rate_limit]` subsection.
    #[serde(default)]
    pub rate_limit: RateLimitSection,
    /// `[net.auth]` subsection.
    #[serde(default)]
    pub auth: AuthSection,
}

/// `[net.tls]` — TLS enablement + cert/key paths.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct TlsSection {
    /// Enable TLS at the gRPC transport layer.
    pub enabled: Option<bool>,
    /// Path to the PEM-encoded certificate chain.
    pub cert_path: Option<PathBuf>,
    /// Path to the PEM-encoded private key.
    pub key_path: Option<PathBuf>,
}

/// `[net.metrics]` — Prometheus HTTP endpoint configuration.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct MetricsSection {
    /// Address on which the Prometheus `/metrics` HTTP server listens.
    pub addr: Option<SocketAddr>,
}

/// `[net.logging]` — request/response logging verbosity.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct LoggingSection {
    /// Log verbosity: `"off"`, `"brief"`, or `"detailed"`.
    pub verbosity: Option<LogVerbosityWire>,
    /// Slow-request threshold in milliseconds.  Requests slower than this are
    /// always logged when `verbosity = "brief"`.
    pub slow_threshold_ms: Option<u64>,
}

/// `[net.rate_limit]` — token-bucket rate limiter QPS.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct RateLimitSection {
    /// Steady-state queries-per-second cap.
    pub qps: Option<f64>,
}

/// `[net.auth]` — bearer-token authentication.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct AuthSection {
    /// Path to the file holding the JWT signing/decoding secret.
    pub jwt_secret_path: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// LogVerbosity wire wrapper
// ---------------------------------------------------------------------------

/// Wire-format wrapper for [`LogVerbosity`] supporting case-insensitive
/// `"off"` / `"brief"` / `"detailed"` strings in TOML and env vars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogVerbosityWire(pub LogVerbosity);

impl LogVerbosityWire {
    /// Parse a verbosity string (case-insensitive).
    pub fn parse(s: &str) -> NetResult<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self(LogVerbosity::Off)),
            "brief" => Ok(Self(LogVerbosity::Brief)),
            "detailed" => Ok(Self(LogVerbosity::Detailed)),
            other => Err(NetError::InvalidRequest(format!(
                "Invalid log verbosity '{other}': expected 'off', 'brief', or 'detailed'"
            ))),
        }
    }
}

impl<'de> Deserialize<'de> for LogVerbosityWire {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// NetConfig API
// ---------------------------------------------------------------------------

impl NetConfig {
    /// Load a [`NetConfig`] from a TOML file on disk.
    ///
    /// Cert/key paths and JWT secret paths in the TOML are resolved relative
    /// to the TOML file's parent directory.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::InvalidRequest`] if the file cannot be read or
    /// contains invalid TOML.
    pub fn from_path(path: impl AsRef<Path>) -> NetResult<Self> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| {
            NetError::InvalidRequest(format!(
                "Failed to read config file {}: {e}",
                path.display()
            ))
        })?;
        let text = std::str::from_utf8(&bytes).map_err(|e| {
            NetError::InvalidRequest(format!(
                "Config file {} is not valid UTF-8: {e}",
                path.display()
            ))
        })?;
        let mut cfg: Self = toml::from_str(text).map_err(|e| {
            NetError::InvalidRequest(format!(
                "Failed to parse config file {}: {e}",
                path.display()
            ))
        })?;

        // Resolve relative cert/key/secret paths against the config file's parent.
        if let Some(parent) = path.parent() {
            cfg.resolve_paths_relative_to(parent);
        }

        Ok(cfg)
    }

    /// Layer this config on top of environment-variable overrides and return
    /// the result.  TOML values are kept when the corresponding env var is
    /// unset.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::InvalidRequest`] if any env var present has an
    /// invalid value (unparseable address, non-numeric QPS, …).
    pub fn merge_env(mut self) -> NetResult<Self> {
        if let Some(val) = read_env("AMATERS_NET_BIND_ADDR")? {
            self.net.bind_addr = Some(parse_env::<SocketAddr>("AMATERS_NET_BIND_ADDR", &val)?);
        }
        if let Some(val) = read_env("AMATERS_NET_TLS_ENABLED")? {
            self.net.tls.enabled = Some(parse_env::<bool>("AMATERS_NET_TLS_ENABLED", &val)?);
        }
        if let Some(val) = read_env("AMATERS_NET_TLS_CERT_PATH")? {
            self.net.tls.cert_path = Some(PathBuf::from(val));
        }
        if let Some(val) = read_env("AMATERS_NET_TLS_KEY_PATH")? {
            self.net.tls.key_path = Some(PathBuf::from(val));
        }
        if let Some(val) = read_env("AMATERS_NET_METRICS_ADDR")? {
            self.net.metrics.addr =
                Some(parse_env::<SocketAddr>("AMATERS_NET_METRICS_ADDR", &val)?);
        }
        if let Some(val) = read_env("AMATERS_NET_LOG_VERBOSITY")? {
            self.net.logging.verbosity = Some(LogVerbosityWire::parse(&val)?);
        }
        if let Some(val) = read_env("AMATERS_NET_SLOW_THRESHOLD_MS")? {
            self.net.logging.slow_threshold_ms =
                Some(parse_env::<u64>("AMATERS_NET_SLOW_THRESHOLD_MS", &val)?);
        }
        if let Some(val) = read_env("AMATERS_NET_RATE_LIMIT_QPS")? {
            self.net.rate_limit.qps = Some(parse_env::<f64>("AMATERS_NET_RATE_LIMIT_QPS", &val)?);
        }
        if let Some(val) = read_env("AMATERS_NET_JWT_SECRET_PATH")? {
            self.net.auth.jwt_secret_path = Some(PathBuf::from(val));
        }
        Ok(self)
    }

    /// Convenience: load from file then layer env vars.
    ///
    /// Equivalent to `NetConfig::from_path(path)?.merge_env()`.
    pub fn load_layered(path: impl AsRef<Path>) -> NetResult<Self> {
        Self::from_path(path)?.merge_env()
    }

    /// Apply this config's values to an [`AqlServerBuilder`].
    ///
    /// Builder methods called *after* this fold continue to take precedence
    /// — `apply_to` only sets values for fields that are `Some` in the config.
    pub fn apply_to<S>(&self, mut builder: AqlServerBuilder<S>) -> AqlServerBuilder<S>
    where
        S: StorageEngine + Send + Sync + 'static,
    {
        if let Some(verbosity) = self.net.logging.verbosity {
            builder = builder.with_logging(verbosity.0);
        }
        if let Some(slow_ms) = self.net.logging.slow_threshold_ms {
            builder = builder.with_slow_threshold_ms(slow_ms);
        }
        if let Some(addr) = self.net.metrics.addr {
            builder = builder.with_metrics_addr(addr);
        }
        if let Some(addr) = self.net.bind_addr {
            builder = builder.with_bind_addr(addr);
        }
        if let Some(qps) = self.net.rate_limit.qps {
            builder = builder.with_rate_limit_qps(qps);
        }
        if let Some(ref path) = self.net.auth.jwt_secret_path {
            builder = builder.with_jwt_secret_path(path.clone());
        }
        builder
    }

    /// Resolve relative cert/key/secret paths against `base`.
    fn resolve_paths_relative_to(&mut self, base: &Path) {
        if let Some(p) = self.net.tls.cert_path.as_mut() {
            if p.is_relative() {
                *p = base.join(p.as_path());
            }
        }
        if let Some(p) = self.net.tls.key_path.as_mut() {
            if p.is_relative() {
                *p = base.join(p.as_path());
            }
        }
        if let Some(p) = self.net.auth.jwt_secret_path.as_mut() {
            if p.is_relative() {
                *p = base.join(p.as_path());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Env helpers
// ---------------------------------------------------------------------------

/// Read a process env var.  Returns `Ok(None)` for unset vars, `Ok(Some(...))`
/// for set values.
fn read_env(name: &str) -> NetResult<Option<String>> {
    match std::env::var(name) {
        Ok(v) => Ok(Some(v)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(NetError::InvalidRequest(format!(
            "Env var {name} is not valid UTF-8"
        ))),
    }
}

/// Parse an env value with a typed `FromStr`.
fn parse_env<T: std::str::FromStr>(name: &str, raw: &str) -> NetResult<T>
where
    T::Err: std::fmt::Display,
{
    raw.parse::<T>()
        .map_err(|e| NetError::InvalidRequest(format!("Invalid {name}={raw:?}: {e}")))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use amaters_core::storage::MemoryStorage;
    use serial_test::serial;
    use std::sync::Arc;

    /// Generate a unique scratch path under `temp_dir()` for a TOML config.
    fn scratch_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "amaters_net_config_test_{name}_{}.toml",
            uuid::Uuid::new_v4()
        ));
        p
    }

    /// Wipe every `AMATERS_NET_*` var so leakage between tests is impossible.
    fn clear_env_vars() {
        for v in [
            "AMATERS_NET_BIND_ADDR",
            "AMATERS_NET_TLS_ENABLED",
            "AMATERS_NET_TLS_CERT_PATH",
            "AMATERS_NET_TLS_KEY_PATH",
            "AMATERS_NET_METRICS_ADDR",
            "AMATERS_NET_LOG_VERBOSITY",
            "AMATERS_NET_SLOW_THRESHOLD_MS",
            "AMATERS_NET_RATE_LIMIT_QPS",
            "AMATERS_NET_JWT_SECRET_PATH",
        ] {
            // SAFETY: tests are serialized by serial_test; we only touch our
            // own well-known env vars.  Setting/removing process env is
            // unsafe in multi-threaded code in the 2024 edition, hence the
            // explicit unsafe block.
            unsafe { std::env::remove_var(v) };
        }
    }

    /// `from_path` round-trips a fully-populated TOML file.
    #[test]
    fn test_net_config_load_from_toml_file() {
        let path = scratch_path("full");
        std::fs::write(
            &path,
            r#"
[net]
bind_addr = "127.0.0.1:50051"

[net.tls]
enabled = true
cert_path = "certs/server.pem"
key_path = "certs/server.key"

[net.metrics]
addr = "127.0.0.1:9091"

[net.logging]
verbosity = "brief"
slow_threshold_ms = 250

[net.rate_limit]
qps = 1500.0

[net.auth]
jwt_secret_path = "secrets/jwt.key"
"#,
        )
        .expect("write toml");

        let cfg = NetConfig::from_path(&path).expect("load config");
        assert_eq!(
            cfg.net.bind_addr,
            Some("127.0.0.1:50051".parse().expect("addr"))
        );
        assert_eq!(cfg.net.tls.enabled, Some(true));
        // Path resolution: cert_path resolves against scratch dir parent
        let scratch_parent = path.parent().expect("parent");
        assert_eq!(
            cfg.net.tls.cert_path,
            Some(scratch_parent.join("certs/server.pem"))
        );
        assert_eq!(
            cfg.net.tls.key_path,
            Some(scratch_parent.join("certs/server.key"))
        );
        assert_eq!(
            cfg.net.metrics.addr,
            Some("127.0.0.1:9091".parse().expect("metrics addr"))
        );
        assert_eq!(
            cfg.net.logging.verbosity.map(|v| v.0),
            Some(LogVerbosity::Brief)
        );
        assert_eq!(cfg.net.logging.slow_threshold_ms, Some(250));
        assert_eq!(cfg.net.rate_limit.qps, Some(1500.0));
        assert_eq!(
            cfg.net.auth.jwt_secret_path,
            Some(scratch_parent.join("secrets/jwt.key"))
        );

        let _ = std::fs::remove_file(&path);
    }

    /// A TOML with missing sections yields a config whose missing fields are
    /// `None` — `apply_to` then falls through to whatever the builder already had.
    #[test]
    fn test_net_config_partial_toml_uses_builder_defaults() {
        let path = scratch_path("partial");
        // Only the metrics address is set.
        std::fs::write(
            &path,
            r#"
[net.metrics]
addr = "127.0.0.1:9092"
"#,
        )
        .expect("write toml");

        let cfg = NetConfig::from_path(&path).expect("load config");
        assert_eq!(cfg.net.bind_addr, None);
        assert_eq!(cfg.net.tls.enabled, None);
        assert_eq!(cfg.net.tls.cert_path, None);
        assert_eq!(cfg.net.logging.verbosity, None);
        assert_eq!(
            cfg.net.metrics.addr,
            Some("127.0.0.1:9092".parse().expect("metrics addr"))
        );

        let _ = std::fs::remove_file(&path);
    }

    /// `apply_to` overlays config values onto a builder — verified by reading
    /// back via builder accessor.
    #[test]
    fn test_net_config_apply_to_builder_overrides() {
        let path = scratch_path("apply");
        std::fs::write(
            &path,
            r#"
[net.logging]
verbosity = "detailed"
slow_threshold_ms = 50

[net.metrics]
addr = "127.0.0.1:9093"

[net.rate_limit]
qps = 250.0
"#,
        )
        .expect("write toml");

        let cfg = NetConfig::from_path(&path).expect("load config");
        let storage = Arc::new(MemoryStorage::new());
        let builder = AqlServerBuilder::new(storage);
        let builder = cfg.apply_to(builder);

        assert_eq!(builder.logging_verbosity(), Some(LogVerbosity::Detailed));
        assert_eq!(builder.slow_threshold_ms(), Some(50));
        assert_eq!(
            builder.metrics_addr(),
            Some("127.0.0.1:9093".parse().expect("metrics addr"))
        );
        assert_eq!(builder.rate_limit_qps(), Some(250.0));

        let _ = std::fs::remove_file(&path);
    }

    /// Invalid TOML → `NetError::InvalidRequest`.
    #[test]
    fn test_net_config_invalid_toml_returns_error() {
        let path = scratch_path("invalid");
        std::fs::write(&path, "this is not [net.tls valid toml = yes").expect("write toml");

        let result = NetConfig::from_path(&path);
        assert!(matches!(result, Err(NetError::InvalidRequest(_))));

        let _ = std::fs::remove_file(&path);
    }

    /// Full round-trip: parse a config, apply to a builder, verify every field.
    #[test]
    fn test_net_config_full_round_trip() {
        let path = scratch_path("roundtrip");
        std::fs::write(
            &path,
            r#"
[net]
bind_addr = "0.0.0.0:50052"

[net.tls]
enabled = false

[net.metrics]
addr = "0.0.0.0:9094"

[net.logging]
verbosity = "off"
slow_threshold_ms = 1000

[net.rate_limit]
qps = 5000.5
"#,
        )
        .expect("write toml");

        let cfg = NetConfig::from_path(&path).expect("load config");
        let storage = Arc::new(MemoryStorage::new());
        let builder = AqlServerBuilder::new(storage);
        let builder = cfg.apply_to(builder);

        assert_eq!(
            builder.bind_addr(),
            Some("0.0.0.0:50052".parse().expect("bind addr"))
        );
        assert_eq!(builder.logging_verbosity(), Some(LogVerbosity::Off));
        assert_eq!(builder.slow_threshold_ms(), Some(1000));
        assert_eq!(
            builder.metrics_addr(),
            Some("0.0.0.0:9094".parse().expect("metrics addr"))
        );
        assert_eq!(builder.rate_limit_qps(), Some(5000.5));

        let _ = std::fs::remove_file(&path);
    }

    /// Invalid log verbosity in TOML returns an error.
    #[test]
    fn test_net_config_invalid_log_verbosity_returns_error() {
        let path = scratch_path("invalid_verb");
        std::fs::write(
            &path,
            r#"
[net.logging]
verbosity = "loud"
"#,
        )
        .expect("write toml");

        let result = NetConfig::from_path(&path);
        assert!(matches!(result, Err(NetError::InvalidRequest(_))));

        let _ = std::fs::remove_file(&path);
    }

    // -----------------------------------------------------------------------
    // Env-var override tests (Item 4)
    // -----------------------------------------------------------------------

    #[test]
    #[serial]
    fn test_env_override_bind_addr() {
        clear_env_vars();
        // SAFETY: tests are serialized; only sets a well-known env var.
        unsafe { std::env::set_var("AMATERS_NET_BIND_ADDR", "127.0.0.1:60001") };

        let cfg = NetConfig::default().merge_env().expect("merge_env");

        assert_eq!(
            cfg.net.bind_addr,
            Some("127.0.0.1:60001".parse().expect("addr"))
        );

        clear_env_vars();
    }

    #[test]
    #[serial]
    fn test_env_override_tls_enabled_true() {
        clear_env_vars();
        // SAFETY: tests are serialized.
        unsafe { std::env::set_var("AMATERS_NET_TLS_ENABLED", "true") };

        let cfg = NetConfig::default().merge_env().expect("merge_env");
        assert_eq!(cfg.net.tls.enabled, Some(true));

        clear_env_vars();
    }

    #[test]
    #[serial]
    fn test_env_override_invalid_value_returns_error() {
        clear_env_vars();
        // SAFETY: tests are serialized.
        unsafe { std::env::set_var("AMATERS_NET_RATE_LIMIT_QPS", "not-a-number") };

        let result = NetConfig::default().merge_env();
        assert!(matches!(result, Err(NetError::InvalidRequest(_))));

        clear_env_vars();
    }

    #[test]
    #[serial]
    fn test_env_does_not_override_when_unset() {
        clear_env_vars();

        let mut cfg = NetConfig::default();
        cfg.net.bind_addr = Some("10.0.0.1:50051".parse().expect("addr"));
        cfg.net.tls.enabled = Some(false);

        let cfg = cfg.merge_env().expect("merge_env");
        assert_eq!(
            cfg.net.bind_addr,
            Some("10.0.0.1:50051".parse().expect("addr"))
        );
        assert_eq!(cfg.net.tls.enabled, Some(false));
    }

    #[test]
    #[serial]
    fn test_layered_load_combines_toml_and_env() {
        clear_env_vars();
        let path = scratch_path("layered");
        std::fs::write(
            &path,
            r#"
[net]
bind_addr = "127.0.0.1:50051"

[net.metrics]
addr = "127.0.0.1:9090"

[net.logging]
verbosity = "off"
"#,
        )
        .expect("write toml");

        // Env overrides bind_addr and verbosity, leaves metrics_addr alone.
        // SAFETY: tests are serialized.
        unsafe {
            std::env::set_var("AMATERS_NET_BIND_ADDR", "127.0.0.1:50099");
            std::env::set_var("AMATERS_NET_LOG_VERBOSITY", "detailed");
        }

        let cfg = NetConfig::load_layered(&path).expect("layered");
        assert_eq!(
            cfg.net.bind_addr,
            Some("127.0.0.1:50099".parse().expect("addr"))
        );
        assert_eq!(
            cfg.net.metrics.addr,
            Some("127.0.0.1:9090".parse().expect("metrics addr"))
        );
        assert_eq!(
            cfg.net.logging.verbosity.map(|v| v.0),
            Some(LogVerbosity::Detailed)
        );

        clear_env_vars();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[serial]
    fn test_env_override_log_verbosity_invalid() {
        clear_env_vars();
        // SAFETY: tests are serialized.
        unsafe { std::env::set_var("AMATERS_NET_LOG_VERBOSITY", "loud") };

        let result = NetConfig::default().merge_env();
        assert!(matches!(result, Err(NetError::InvalidRequest(_))));

        clear_env_vars();
    }
}
