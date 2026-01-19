//! Configuration management for amaters-cli
//!
//! Supports loading configuration from:
//! - Environment variables (AMATERS_*)
//! - Configuration file (~/.amaters/config.toml)
//! - Command line arguments

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
        let config_dir = config_path.parent().context("Invalid config path")?;

        // Create config directory if it doesn't exist
        std::fs::create_dir_all(config_dir)
            .with_context(|| format!("Failed to create config directory: {:?}", config_dir))?;

        let contents = toml::to_string_pretty(self).context("Failed to serialize configuration")?;

        std::fs::write(&config_path, contents)
            .with_context(|| format!("Failed to write config file: {:?}", config_path))?;

        Ok(())
    }
}

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
}
