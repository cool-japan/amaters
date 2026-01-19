//! Command-line interface module
//!
//! This module defines the CLI structure using clap, supporting:
//! - start: Start the server
//! - stop: Stop a running server
//! - status: Check server status
//! - version: Display version information
//! - validate-config: Validate configuration file

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// AmateRS Server - Fully Homomorphic Encrypted Database Server
#[derive(Parser, Debug)]
#[command(name = "amaters-server")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "AmateRS Fully Homomorphic Encrypted Database Server", long_about = None)]
#[command(author = "COOLJAPAN OU (Team KitaSan)")]
pub struct Cli {
    /// Configuration file path
    #[arg(short, long, default_value = "./config.toml", global = true)]
    pub config: PathBuf,

    /// Override bind address
    #[arg(short, long, global = true)]
    pub bind: Option<String>,

    /// Override data directory
    #[arg(short, long, global = true)]
    pub data_dir: Option<PathBuf>,

    /// Override log level (trace, debug, info, warn, error)
    #[arg(long, global = true)]
    pub log_level: Option<String>,

    /// Subcommand to execute
    #[command(subcommand)]
    pub command: Command,
}

/// Server commands
#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Start the server
    Start {
        /// Run in foreground (don't daemonize)
        #[arg(short, long)]
        foreground: bool,

        /// Generate default config file if it doesn't exist
        #[arg(long)]
        generate_config: bool,
    },

    /// Stop a running server
    Stop {
        /// Force stop (SIGKILL instead of SIGTERM)
        #[arg(short, long)]
        force: bool,

        /// Timeout in seconds before force stop
        #[arg(short, long, default_value = "30")]
        timeout: u64,
    },

    /// Check server status
    Status {
        /// Output format (human, json)
        #[arg(short, long, default_value = "human")]
        format: String,
    },

    /// Display version information
    Version {
        /// Verbose version info (include dependencies)
        #[arg(short, long)]
        verbose: bool,
    },

    /// Validate configuration file
    ValidateConfig {
        /// Show full configuration after validation
        #[arg(short, long)]
        show: bool,
    },
}

impl Cli {
    /// Parse CLI arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Get configuration file path
    pub fn config_path(&self) -> &PathBuf {
        &self.config
    }

    /// Check if config file override is present
    pub fn has_config_override(&self) -> bool {
        self.config.to_str() != Some("./config.toml")
    }
}

/// Display formats for status command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFormat {
    Human,
    Json,
}

impl StatusFormat {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            _ => Err(format!("Invalid format: {}. Must be 'human' or 'json'", s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_format_parsing() {
        assert_eq!(
            StatusFormat::from_str("human").ok(),
            Some(StatusFormat::Human)
        );
        assert_eq!(
            StatusFormat::from_str("json").ok(),
            Some(StatusFormat::Json)
        );
        assert_eq!(
            StatusFormat::from_str("Human").ok(),
            Some(StatusFormat::Human)
        );
        assert!(StatusFormat::from_str("invalid").is_err());
    }
}
