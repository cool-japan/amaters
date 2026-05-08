//! Configuration management for amaters-cli
//!
//! Supports loading configuration from:
//! - Environment variables (AMATERS_*)
//! - Configuration file (~/.amaters/config.toml)
//! - Command line arguments
//!
//! Provides CLI subcommands: show, init, validate, get, set

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Core Config struct (used by the rest of the CLI)
// ---------------------------------------------------------------------------

/// CLI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Server URL
    pub server_url: String,
    /// Default collection
    pub default_collection: String,
    /// TLS configuration
    #[serde(default)]
    pub tls: TlsConfig,
    /// Output format (json, table)
    #[serde(default = "default_output_format")]
    pub output_format: String,
    /// Enable colored output
    #[serde(default = "default_true")]
    pub color: bool,
    /// Default FHE key name used when no --key flag is provided
    #[serde(default)]
    pub default_key: Option<String>,
}

fn default_output_format() -> String {
    "table".to_string()
}

fn default_true() -> bool {
    true
}

/// TLS configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TlsConfig {
    /// Enable TLS
    #[serde(default)]
    pub enabled: bool,
    /// Path to CA certificate
    pub ca_cert: Option<PathBuf>,
    /// Path to client certificate
    pub client_cert: Option<PathBuf>,
    /// Path to client key
    pub client_key: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:50051".to_string(),
            default_collection: "default".to_string(),
            tls: TlsConfig::default(),
            output_format: default_output_format(),
            color: true,
            default_key: None,
        }
    }
}

impl Config {
    /// Load configuration from file or create default
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config file: {:?}", config_path))?;

            let mut config: Config = toml::from_str(&contents)
                .with_context(|| format!("Failed to parse config file: {:?}", config_path))?;

            // Override with environment variables
            config.apply_env();

            Ok(config)
        } else {
            // Create default config
            let mut config = Config::default();
            config.apply_env();
            Ok(config)
        }
    }

    /// Load configuration from a specific path
    pub fn load_from(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {:?}", path))?;

        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {:?}", path))?;

        Ok(config)
    }

    /// Get the configuration file path
    pub fn config_path() -> Result<PathBuf> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .context("Could not determine home directory")?;

        let config_dir = PathBuf::from(home).join(".amaters");
        Ok(config_dir.join("config.toml"))
    }

    /// Apply environment variable overrides
    fn apply_env(&mut self) {
        if let Ok(url) = std::env::var("AMATERS_SERVER_URL") {
            self.server_url = url;
        }
        if let Ok(collection) = std::env::var("AMATERS_COLLECTION") {
            self.default_collection = collection;
        }
        if let Ok(format) = std::env::var("AMATERS_OUTPUT_FORMAT") {
            self.output_format = format;
        }
        if let Ok(color) = std::env::var("AMATERS_COLOR") {
            self.color = color.parse().unwrap_or(true);
        }
    }

    /// Save configuration to file
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        self.save_to(&config_path)
    }

    /// Save configuration to a specific path
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let config_dir = path.parent().context("Invalid config path")?;

        // Create config directory if it doesn't exist
        std::fs::create_dir_all(config_dir)
            .with_context(|| format!("Failed to create config directory: {:?}", config_dir))?;

        let contents = toml::to_string_pretty(self).context("Failed to serialize configuration")?;

        std::fs::write(path, contents)
            .with_context(|| format!("Failed to write config file: {:?}", path))?;

        Ok(())
    }

    /// Save configuration atomically: write to a `.tmp` file first, then rename.
    ///
    /// This prevents partial writes from corrupting the config file on crash or
    /// concurrent access. The caller should pass the final (non-tmp) path; this
    /// method derives the temporary path by appending `.tmp` to the file name.
    pub fn save_atomic_to(&self, path: &Path) -> Result<()> {
        let config_dir = path.parent().context("Invalid config path")?;

        // Create config directory if it doesn't exist
        std::fs::create_dir_all(config_dir)
            .with_context(|| format!("Failed to create config directory: {:?}", config_dir))?;

        let contents = toml::to_string_pretty(self).context("Failed to serialize configuration")?;

        // Derive the temporary file path alongside the target.
        let tmp_path = path.with_extension("toml.tmp");

        std::fs::write(&tmp_path, &contents)
            .with_context(|| format!("Failed to write temporary config file: {:?}", tmp_path))?;

        std::fs::rename(&tmp_path, path).with_context(|| {
            format!(
                "Failed to rename {:?} -> {:?}",
                tmp_path,
                path
            )
        })?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Server-side config template (comprehensive TOML for `config init`)
// ---------------------------------------------------------------------------

/// Generate the default comprehensive TOML config template with comments.
pub fn default_config_template() -> String {
    r#"# AmateRS Configuration File
# Generated by amaters-cli

# ─── Server Settings ────────────────────────────────────────────────────────
[server]
# Host address to bind to
host = "0.0.0.0"
# Port to listen on (1-65535)
port = 50051
# Number of worker threads (0 = number of CPU cores)
workers = 0

# ─── Storage Settings ───────────────────────────────────────────────────────
[storage]
# Data directory for persistent storage
data_dir = "./data"
# Write-ahead log directory (relative to data_dir if not absolute)
wal_dir = "./data/wal"
# Memtable size in megabytes before flushing to disk
memtable_size_mb = 64
# Compaction strategy: "leveled", "tiered", "universal"
compaction_strategy = "leveled"

# ─── Network Settings ───────────────────────────────────────────────────────
[network]
# Enable TLS encryption for client connections
tls_enabled = false
# Path to TLS certificate file (PEM format)
# tls_cert = "/path/to/cert.pem"
# Path to TLS private key file (PEM format)
# tls_key = "/path/to/key.pem"
# Path to CA certificate for client verification (mTLS)
# tls_ca = "/path/to/ca.pem"
# TCP keepalive interval in seconds (0 = disabled)
keepalive_secs = 60
# Connection timeout in seconds
timeout_secs = 30

# ─── Cluster Settings ───────────────────────────────────────────────────────
[cluster]
# Enable distributed clustering
enabled = false
# Unique node identifier within the cluster
node_id = "node-1"
# Peer addresses for cluster communication
# peers = ["192.168.1.10:50052", "192.168.1.11:50052"]

# ─── Logging Settings ───────────────────────────────────────────────────────
[logging]
# Log level: "trace", "debug", "info", "warn", "error"
level = "info"
# Log format: "json", "pretty", "compact"
format = "pretty"
# Log file path (empty = stdout only)
# file = "/var/log/amaters/server.log"

# ─── Metrics Settings ───────────────────────────────────────────────────────
[metrics]
# Enable Prometheus-compatible metrics export
enabled = true
# Metrics export interval in seconds
export_interval_secs = 15
"#
    .to_string()
}

// ---------------------------------------------------------------------------
// Config subcommand implementations
// ---------------------------------------------------------------------------

/// Resolve the config file path from an optional CLI override.
fn resolve_config_path(cli_path: Option<&Path>) -> Result<PathBuf> {
    match cli_path {
        Some(p) => Ok(p.to_path_buf()),
        None => Config::config_path(),
    }
}

/// `amaters config show`
pub fn cmd_show(path: Option<&Path>, format: &str, section: Option<&str>) -> Result<()> {
    let config_path = resolve_config_path(path)?;

    if !config_path.exists() {
        anyhow::bail!(
            "Configuration file not found: {}\nRun `amaters config init` to create one.",
            config_path.display()
        );
    }

    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

    // Parse as generic toml::Value so we can filter sections and convert formats.
    let value: toml::Value = toml::from_str(&raw)
        .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;

    // Optionally narrow to a single section.
    let output_value = if let Some(sec) = section {
        value
            .as_table()
            .and_then(|t| t.get(sec))
            .ok_or_else(|| anyhow::anyhow!("Section '{}' not found in config", sec))?
            .clone()
    } else {
        value
    };

    // Render in the requested format.
    let rendered = match format {
        "json" => serde_json::to_string_pretty(&output_value)
            .context("Failed to render config as JSON")?,
        "yaml" => {
            serde_yaml::to_string(&output_value).context("Failed to render config as YAML")?
        }
        _ => toml::to_string_pretty(&output_value).context("Failed to render config as TOML")?,
    };

    println!("{}", rendered.trim_end());
    Ok(())
}

/// `amaters config init`
pub fn cmd_init(path: Option<&Path>, force: bool) -> Result<()> {
    let config_path = resolve_config_path(path)?;

    if config_path.exists() && !force {
        anyhow::bail!(
            "Configuration file already exists at: {}\nUse --force to overwrite.",
            config_path.display()
        );
    }

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    let template = default_config_template();
    std::fs::write(&config_path, &template)
        .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

    println!("Configuration initialized at: {}", config_path.display());
    Ok(())
}

/// `amaters config validate`
pub fn cmd_validate(path: Option<&Path>) -> Result<()> {
    let config_path = resolve_config_path(path)?;

    if !config_path.exists() {
        anyhow::bail!("Configuration file not found: {}", config_path.display());
    }

    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

    // Phase 1: Parse TOML syntax
    let value: toml::Value = match toml::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("TOML parse error in {}:", config_path.display());
            // toml::de::Error may contain span info; print it as-is.
            eprintln!("  {}", e);
            std::process::exit(1);
        }
    };

    let table = value
        .as_table()
        .ok_or_else(|| anyhow::anyhow!("Config root must be a TOML table"))?;

    let mut errors: Vec<String> = Vec::new();

    // Phase 2: Semantic validation
    validate_server_section(table, &mut errors);
    validate_storage_section(table, &mut errors);
    validate_network_section(table, &mut errors);
    validate_cluster_section(table, &mut errors);
    validate_logging_section(table, &mut errors);
    validate_metrics_section(table, &mut errors);

    if errors.is_empty() {
        println!("Configuration is valid: {}", config_path.display());
        Ok(())
    } else {
        eprintln!("Validation errors in {}:", config_path.display());
        for err in &errors {
            eprintln!("  - {}", err);
        }
        std::process::exit(1);
    }
}

/// `amaters config get <KEY>`
pub fn cmd_get(key: &str, path: Option<&Path>) -> Result<()> {
    let config_path = resolve_config_path(path)?;

    if !config_path.exists() {
        anyhow::bail!("Configuration file not found: {}", config_path.display());
    }

    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

    let value: toml::Value = toml::from_str(&raw)
        .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;

    let resolved = resolve_dotted_key(&value, key)
        .ok_or_else(|| anyhow::anyhow!("Key '{}' not found in config", key))?;

    // Print the value in a human-friendly way.
    match resolved {
        toml::Value::String(s) => println!("{}", s),
        toml::Value::Integer(n) => println!("{}", n),
        toml::Value::Float(f) => println!("{}", f),
        toml::Value::Boolean(b) => println!("{}", b),
        other => {
            let rendered = toml::to_string_pretty(&other).context("Failed to render value")?;
            println!("{}", rendered.trim_end());
        }
    }

    Ok(())
}

/// `amaters config set <KEY> <VALUE>`
pub fn cmd_set(key: &str, value: &str, path: Option<&Path>) -> Result<()> {
    let config_path = resolve_config_path(path)?;

    if !config_path.exists() {
        anyhow::bail!(
            "Configuration file not found: {}\nRun `amaters config init` first.",
            config_path.display()
        );
    }

    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

    let mut doc: toml::Value = toml::from_str(&raw)
        .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;

    // Infer the TOML type for the new value.
    let new_val = infer_toml_value(value);

    set_dotted_key(&mut doc, key, new_val)?;

    // Preserve as much structure as we can; write back as pretty TOML.
    let output = toml::to_string_pretty(&doc).context("Failed to serialize config")?;

    std::fs::write(&config_path, &output)
        .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

    println!("Set {} = {}", key, value);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a dot-notation key (e.g. "server.port") in a `toml::Value`.
fn resolve_dotted_key<'a>(value: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = value;
    for part in &parts {
        current = current.as_table()?.get(*part)?;
    }
    Some(current)
}

/// Set a dot-notation key in a `toml::Value`, creating intermediate tables as needed.
fn set_dotted_key(root: &mut toml::Value, key: &str, new_val: toml::Value) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        anyhow::bail!("Key must not be empty");
    }

    let mut current = root;
    for part in &parts[..parts.len() - 1] {
        // Ensure intermediate tables exist.
        if !current.as_table().is_some_and(|t| t.contains_key(*part)) {
            current
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("Expected table at key path"))?
                .insert(
                    (*part).to_string(),
                    toml::Value::Table(toml::map::Map::new()),
                );
        }
        current = current
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("Expected table at key path"))?
            .get_mut(*part)
            .ok_or_else(|| anyhow::anyhow!("Key path not found"))?;
    }

    let last = *parts
        .last()
        .ok_or_else(|| anyhow::anyhow!("Key must not be empty"))?;

    current
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("Expected table at key path"))?
        .insert(last.to_string(), new_val);

    Ok(())
}

/// Infer a TOML-typed value from a string typed on the CLI.
fn infer_toml_value(s: &str) -> toml::Value {
    // Boolean
    if s.eq_ignore_ascii_case("true") {
        return toml::Value::Boolean(true);
    }
    if s.eq_ignore_ascii_case("false") {
        return toml::Value::Boolean(false);
    }
    // Integer
    if let Ok(n) = s.parse::<i64>() {
        return toml::Value::Integer(n);
    }
    // Float
    if let Ok(f) = s.parse::<f64>() {
        return toml::Value::Float(f);
    }
    // Otherwise, string
    toml::Value::String(s.to_string())
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn validate_server_section(table: &toml::map::Map<String, toml::Value>, errors: &mut Vec<String>) {
    if let Some(server) = table.get("server").and_then(|v| v.as_table()) {
        if let Some(port) = server.get("port") {
            if let Some(p) = port.as_integer() {
                if !(1..=65535).contains(&p) {
                    errors.push(format!(
                        "[server] port must be between 1 and 65535 (got {})",
                        p
                    ));
                }
            } else {
                errors.push("[server] port must be an integer".to_string());
            }
        }
        if let Some(workers) = server.get("workers") {
            if let Some(w) = workers.as_integer() {
                if w < 0 {
                    errors.push(format!("[server] workers must be non-negative (got {})", w));
                }
            } else {
                errors.push("[server] workers must be an integer".to_string());
            }
        }
    }
}

fn validate_storage_section(table: &toml::map::Map<String, toml::Value>, errors: &mut Vec<String>) {
    if let Some(storage) = table.get("storage").and_then(|v| v.as_table()) {
        if let Some(memtable) = storage.get("memtable_size_mb") {
            if let Some(m) = memtable.as_integer() {
                if m < 1 {
                    errors.push(format!(
                        "[storage] memtable_size_mb must be >= 1 (got {})",
                        m
                    ));
                }
            } else {
                errors.push("[storage] memtable_size_mb must be an integer".to_string());
            }
        }
        if let Some(strategy) = storage.get("compaction_strategy") {
            if let Some(s) = strategy.as_str() {
                let allowed = ["leveled", "tiered", "universal"];
                if !allowed.contains(&s) {
                    errors.push(format!(
                        "[storage] compaction_strategy must be one of {:?} (got '{}')",
                        allowed, s
                    ));
                }
            }
        }
    }
}

fn validate_network_section(table: &toml::map::Map<String, toml::Value>, errors: &mut Vec<String>) {
    if let Some(network) = table.get("network").and_then(|v| v.as_table()) {
        if let Some(timeout) = network.get("timeout_secs") {
            if let Some(t) = timeout.as_integer() {
                if t < 0 {
                    errors.push(format!(
                        "[network] timeout_secs must be non-negative (got {})",
                        t
                    ));
                }
            } else {
                errors.push("[network] timeout_secs must be an integer".to_string());
            }
        }
        if let Some(ka) = network.get("keepalive_secs") {
            if let Some(k) = ka.as_integer() {
                if k < 0 {
                    errors.push(format!(
                        "[network] keepalive_secs must be non-negative (got {})",
                        k
                    ));
                }
            } else {
                errors.push("[network] keepalive_secs must be an integer".to_string());
            }
        }
        // If TLS is enabled, cert and key should be present.
        let tls_enabled = network
            .get("tls_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if tls_enabled {
            if network.get("tls_cert").is_none() {
                errors.push("[network] tls_cert is required when tls_enabled = true".to_string());
            }
            if network.get("tls_key").is_none() {
                errors.push("[network] tls_key is required when tls_enabled = true".to_string());
            }
        }
    }
}

fn validate_cluster_section(table: &toml::map::Map<String, toml::Value>, errors: &mut Vec<String>) {
    if let Some(cluster) = table.get("cluster").and_then(|v| v.as_table()) {
        let enabled = cluster
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if enabled && cluster.get("node_id").is_none() {
            errors.push("[cluster] node_id is required when clustering is enabled".to_string());
        }
    }
}

fn validate_logging_section(table: &toml::map::Map<String, toml::Value>, errors: &mut Vec<String>) {
    if let Some(logging) = table.get("logging").and_then(|v| v.as_table()) {
        if let Some(level) = logging.get("level") {
            if let Some(l) = level.as_str() {
                let allowed = ["trace", "debug", "info", "warn", "error"];
                if !allowed.contains(&l) {
                    errors.push(format!(
                        "[logging] level must be one of {:?} (got '{}')",
                        allowed, l
                    ));
                }
            }
        }
        if let Some(fmt) = logging.get("format") {
            if let Some(f) = fmt.as_str() {
                let allowed = ["json", "pretty", "compact"];
                if !allowed.contains(&f) {
                    errors.push(format!(
                        "[logging] format must be one of {:?} (got '{}')",
                        allowed, f
                    ));
                }
            }
        }
    }
}

fn validate_metrics_section(table: &toml::map::Map<String, toml::Value>, errors: &mut Vec<String>) {
    if let Some(metrics) = table.get("metrics").and_then(|v| v.as_table()) {
        if let Some(interval) = metrics.get("export_interval_secs") {
            if let Some(i) = interval.as_integer() {
                if i < 1 {
                    errors.push(format!(
                        "[metrics] export_interval_secs must be >= 1 (got {})",
                        i
                    ));
                }
            } else {
                errors.push("[metrics] export_interval_secs must be an integer".to_string());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server_url, "http://localhost:50051");
        assert_eq!(config.default_collection, "default");
        assert_eq!(config.output_format, "table");
        assert!(config.color);
    }

    #[test]
    fn test_env_override() {
        // Set test env vars
        unsafe {
            env::set_var("AMATERS_SERVER_URL", "http://test:8080");
            env::set_var("AMATERS_COLLECTION", "test_collection");
            env::set_var("AMATERS_OUTPUT_FORMAT", "json");
        }

        let mut config = Config::default();
        config.apply_env();

        assert_eq!(config.server_url, "http://test:8080");
        assert_eq!(config.default_collection, "test_collection");
        assert_eq!(config.output_format, "json");

        // Clean up
        unsafe {
            env::remove_var("AMATERS_SERVER_URL");
            env::remove_var("AMATERS_COLLECTION");
            env::remove_var("AMATERS_OUTPUT_FORMAT");
        }
    }

    #[test]
    fn test_config_serialization() -> Result<()> {
        let config = Config::default();
        let toml_str = toml::to_string(&config)?;
        let deserialized: Config = toml::from_str(&toml_str)?;

        assert_eq!(config.server_url, deserialized.server_url);
        assert_eq!(config.default_collection, deserialized.default_collection);

        Ok(())
    }

    #[test]
    fn test_default_template_is_valid_toml() {
        let template = default_config_template();
        let parsed: Result<toml::Value, _> = toml::from_str(&template);
        assert!(
            parsed.is_ok(),
            "Default template should be valid TOML: {:?}",
            parsed.err()
        );
    }

    #[test]
    fn test_default_template_has_all_sections() {
        let template = default_config_template();
        let value: toml::Value = toml::from_str(&template).expect("Template must be valid TOML");
        let table = value.as_table().expect("Root must be a table");

        for section in &[
            "server", "storage", "network", "cluster", "logging", "metrics",
        ] {
            assert!(
                table.contains_key(*section),
                "Template missing section: {}",
                section
            );
        }
    }

    #[test]
    fn test_config_init_creates_valid_toml() {
        let dir = std::env::temp_dir().join("amaters_test_init");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("cmd_init should succeed");

        assert!(config_path.exists(), "Config file should be created");

        let raw = std::fs::read_to_string(&config_path).expect("Should read created config");
        let parsed: Result<toml::Value, _> = toml::from_str(&raw);
        assert!(parsed.is_ok(), "Created config must be valid TOML");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_init_refuses_overwrite_without_force() {
        let dir = std::env::temp_dir().join("amaters_test_no_overwrite");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("First init should succeed");

        let result = cmd_init(Some(config_path.as_path()), false);
        assert!(result.is_err(), "Second init without --force should fail");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_init_force_overwrites() {
        let dir = std::env::temp_dir().join("amaters_test_force_overwrite");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("First init should succeed");

        // Should succeed with --force
        cmd_init(Some(config_path.as_path()), true).expect("Init with --force should succeed");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_show_reads_existing_file() {
        let dir = std::env::temp_dir().join("amaters_test_show");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        // Show should succeed in all formats
        for fmt in &["toml", "json", "yaml"] {
            let result = cmd_show(Some(config_path.as_path()), fmt, None);
            assert!(result.is_ok(), "Show in {} format should succeed", fmt);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_show_section_filter() {
        let dir = std::env::temp_dir().join("amaters_test_show_section");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        let result = cmd_show(Some(config_path.as_path()), "toml", Some("server"));
        assert!(result.is_ok(), "Show server section should succeed");

        let result = cmd_show(Some(config_path.as_path()), "toml", Some("nonexistent"));
        assert!(result.is_err(), "Show nonexistent section should fail");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_show_nonexistent_file() {
        let result = cmd_show(
            Some(Path::new("/tmp/amaters_does_not_exist.toml")),
            "toml",
            None,
        );
        assert!(result.is_err(), "Show on nonexistent file should fail");
    }

    #[test]
    fn test_config_get_returns_correct_value() {
        let dir = std::env::temp_dir().join("amaters_test_get");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        // Read and verify dot-notation access
        let raw = std::fs::read_to_string(&config_path).expect("Should read config");
        let value: toml::Value = toml::from_str(&raw).expect("Valid TOML");
        let port = resolve_dotted_key(&value, "server.port");
        assert!(port.is_some(), "server.port should exist");
        assert_eq!(port.expect("checked above").as_integer(), Some(50051));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_set_persists_change() {
        let dir = std::env::temp_dir().join("amaters_test_set");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        cmd_set("server.port", "9999", Some(config_path.as_path())).expect("Set should succeed");

        // Re-read and verify
        let raw = std::fs::read_to_string(&config_path).expect("Should read config");
        let value: toml::Value = toml::from_str(&raw).expect("Valid TOML");
        let port = resolve_dotted_key(&value, "server.port");
        assert_eq!(
            port.expect("server.port should exist").as_integer(),
            Some(9999)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_set_creates_intermediate_tables() {
        let dir = std::env::temp_dir().join("amaters_test_set_deep");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        cmd_set("custom.nested.key", "hello", Some(config_path.as_path()))
            .expect("Set should create intermediate tables");

        let raw = std::fs::read_to_string(&config_path).expect("Should read config");
        let value: toml::Value = toml::from_str(&raw).expect("Valid TOML");
        let resolved = resolve_dotted_key(&value, "custom.nested.key");
        assert_eq!(resolved.expect("Key should exist").as_str(), Some("hello"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_catches_invalid_port() {
        let dir = std::env::temp_dir().join("amaters_test_validate_port");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        // Set an invalid port
        cmd_set("server.port", "99999", Some(config_path.as_path())).expect("Set should succeed");

        // Validate using the internal helper directly (cmd_validate calls process::exit)
        let raw = std::fs::read_to_string(&config_path).expect("Should read config");
        let value: toml::Value = toml::from_str(&raw).expect("Valid TOML");
        let table = value.as_table().expect("Root is a table");

        let mut errors = Vec::new();
        validate_server_section(table, &mut errors);
        assert!(
            !errors.is_empty(),
            "Invalid port should produce validation errors"
        );
        assert!(
            errors.iter().any(|e| e.contains("port")),
            "Error should mention port"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_catches_invalid_compaction_strategy() {
        let dir = std::env::temp_dir().join("amaters_test_validate_compaction");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        cmd_set(
            "storage.compaction_strategy",
            "bogus",
            Some(config_path.as_path()),
        )
        .expect("Set should succeed");

        let raw = std::fs::read_to_string(&config_path).expect("Should read config");
        let value: toml::Value = toml::from_str(&raw).expect("Valid TOML");
        let table = value.as_table().expect("Root is a table");

        let mut errors = Vec::new();
        validate_storage_section(table, &mut errors);
        assert!(
            !errors.is_empty(),
            "Invalid compaction strategy should produce errors"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_catches_invalid_log_level() {
        let dir = std::env::temp_dir().join("amaters_test_validate_log");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        cmd_set("logging.level", "verbose", Some(config_path.as_path()))
            .expect("Set should succeed");

        let raw = std::fs::read_to_string(&config_path).expect("Should read config");
        let value: toml::Value = toml::from_str(&raw).expect("Valid TOML");
        let table = value.as_table().expect("Root is a table");

        let mut errors = Vec::new();
        validate_logging_section(table, &mut errors);
        assert!(
            !errors.is_empty(),
            "Invalid log level should produce errors"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_tls_requires_cert_and_key() {
        let dir = std::env::temp_dir().join("amaters_test_validate_tls");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        cmd_set("network.tls_enabled", "true", Some(config_path.as_path()))
            .expect("Set should succeed");

        let raw = std::fs::read_to_string(&config_path).expect("Should read config");
        let value: toml::Value = toml::from_str(&raw).expect("Valid TOML");
        let table = value.as_table().expect("Root is a table");

        let mut errors = Vec::new();
        validate_network_section(table, &mut errors);
        assert!(
            errors.len() >= 2,
            "TLS enabled without cert/key should produce at least 2 errors, got: {:?}",
            errors
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_valid_default_config() {
        let dir = std::env::temp_dir().join("amaters_test_validate_ok");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        let raw = std::fs::read_to_string(&config_path).expect("Should read config");
        let value: toml::Value = toml::from_str(&raw).expect("Valid TOML");
        let table = value.as_table().expect("Root is a table");

        let mut errors = Vec::new();
        validate_server_section(table, &mut errors);
        validate_storage_section(table, &mut errors);
        validate_network_section(table, &mut errors);
        validate_cluster_section(table, &mut errors);
        validate_logging_section(table, &mut errors);
        validate_metrics_section(table, &mut errors);

        assert!(
            errors.is_empty(),
            "Default config should be valid, got errors: {:?}",
            errors
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[allow(clippy::approx_constant)]
    #[test]
    fn test_infer_toml_value() {
        assert_eq!(infer_toml_value("true"), toml::Value::Boolean(true));
        assert_eq!(infer_toml_value("false"), toml::Value::Boolean(false));
        assert_eq!(infer_toml_value("42"), toml::Value::Integer(42));
        assert_eq!(infer_toml_value("3.14"), toml::Value::Float(3.14));
        assert_eq!(
            infer_toml_value("hello"),
            toml::Value::String("hello".to_string())
        );
    }

    #[test]
    fn test_resolve_dotted_key() {
        let toml_str = r#"
[a]
b = 42

[a.c]
d = "nested"
"#;
        let value: toml::Value = toml::from_str(toml_str).expect("Valid TOML");

        assert_eq!(
            resolve_dotted_key(&value, "a.b"),
            Some(&toml::Value::Integer(42))
        );
        assert_eq!(
            resolve_dotted_key(&value, "a.c.d"),
            Some(&toml::Value::String("nested".to_string()))
        );
        assert_eq!(resolve_dotted_key(&value, "x.y.z"), None);
    }

    #[test]
    fn test_config_get_nonexistent_key() {
        let dir = std::env::temp_dir().join("amaters_test_get_missing");
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        cmd_init(Some(config_path.as_path()), false).expect("Init should succeed");

        let result = cmd_get("nonexistent.key", Some(config_path.as_path()));
        assert!(result.is_err(), "Getting nonexistent key should fail");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_get_nonexistent_file() {
        let result = cmd_get(
            "server.port",
            Some(Path::new("/tmp/amaters_no_such_file.toml")),
        );
        assert!(result.is_err(), "Getting from nonexistent file should fail");
    }

    #[test]
    fn test_config_set_nonexistent_file() {
        let result = cmd_set(
            "server.port",
            "8080",
            Some(Path::new("/tmp/amaters_no_such_file_set.toml")),
        );
        assert!(result.is_err(), "Setting in nonexistent file should fail");
    }

    #[test]
    fn test_config_load_from_path() {
        let dir = std::env::temp_dir().join("amaters_test_load_from");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("Create dir");
        let config_path = dir.join("config.toml");

        let config = Config::default();
        config.save_to(&config_path).expect("Save should succeed");

        let loaded = Config::load_from(&config_path).expect("Load should succeed");
        assert_eq!(loaded.server_url, config.server_url);
        assert_eq!(loaded.default_collection, config.default_collection);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
