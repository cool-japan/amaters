//! AmateRS CLI tool
//!
//! Command-line interface for interacting with AmateRS encrypted database.

mod admin;
mod client;
mod config;
mod keys;
mod output;
mod progress;
mod server;

use amaters_core::{CipherBlob, Key};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use client::Client;
use config::Config;
use output::OutputFormat;

#[derive(Parser)]
#[command(name = "amaters-cli")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "AmateRS CLI - Manage your encrypted database", long_about = None)]
struct Cli {
    /// Server URL (overrides config)
    #[arg(short, long)]
    server: Option<String>,

    /// Collection name (overrides config)
    #[arg(short, long)]
    collection: Option<String>,

    /// Output format: json, table
    #[arg(short, long)]
    format: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Set a key-value pair
    Set {
        /// Key to set
        key: String,
        /// Value to set (will be encrypted)
        value: String,
    },

    /// Get a value by key
    Get {
        /// Key to retrieve
        key: String,
    },

    /// Delete a key
    Delete {
        /// Key to delete
        key: String,
    },

    /// Range query (scan from start to end key)
    Range {
        /// Start key (inclusive)
        start: String,
        /// End key (exclusive)
        end: String,
    },

    /// Query with filter expression
    Query {
        /// Filter expression (AQL syntax)
        filter: String,
    },

    /// FHE key management
    #[command(subcommand)]
    Key(KeyCommands),

    /// Server management
    #[command(subcommand)]
    Server(ServerCommands),

    /// Administration commands
    #[command(subcommand)]
    Admin(AdminCommands),

    /// Show configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration
    Show,
    /// Initialize configuration file
    Init,
}

#[derive(Subcommand)]
enum KeyCommands {
    /// Generate new FHE keys
    Generate {
        /// Key name
        name: String,
        /// Optional description
        #[arg(short, long)]
        description: Option<String>,
    },

    /// Import keys from a file
    Import {
        /// Key name
        name: String,
        /// Source file path
        #[arg(short, long)]
        file: std::path::PathBuf,
        /// Optional description
        #[arg(short, long)]
        description: Option<String>,
    },

    /// Export keys to a file
    Export {
        /// Key name
        name: String,
        /// Destination file path
        #[arg(short, long)]
        file: std::path::PathBuf,
    },

    /// List all available keys
    List,

    /// Delete a key
    Delete {
        /// Key name
        name: String,
    },
}

#[derive(Subcommand)]
enum ServerCommands {
    /// Show detailed server status
    Status,

    /// Perform health check
    Health,

    /// Show server metrics
    Metrics,

    /// Show cluster information
    Cluster,

    /// Show node information
    Nodes,
}

#[derive(Subcommand)]
enum AdminCommands {
    /// Create a database backup
    Backup {
        /// Backup destination directory
        dest: std::path::PathBuf,
        /// Create incremental backup
        #[arg(short, long)]
        incremental: bool,
    },

    /// Restore from a backup
    Restore {
        /// Backup source directory
        source: std::path::PathBuf,
    },

    /// Trigger manual compaction
    Compact {
        /// Optional collection name (default: all collections)
        #[arg(short, long)]
        collection: Option<String>,
    },

    /// Show database statistics
    Stats,

    /// Verify database integrity
    Verify,

    /// Show server logs
    Logs {
        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "100")]
        lines: usize,
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    let env_filter = match tracing_subscriber::EnvFilter::try_from_default_env() {
        Ok(filter) => filter,
        Err(_) => tracing_subscriber::EnvFilter::new("info"),
    };

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let cli = Cli::parse();

    // Load configuration
    let mut config = Config::load().context("Failed to load configuration")?;

    // Override with CLI arguments
    if let Some(server) = cli.server {
        config.server_url = server;
    }
    if let Some(collection) = cli.collection {
        config.default_collection = collection;
    }
    if let Some(format) = cli.format {
        config.output_format = format;
    }

    // Parse output format
    let output_format = OutputFormat::from_str(&config.output_format)
        .context(format!("Invalid output format: {}", config.output_format))?;

    // Execute command
    if let Err(e) = execute_command(cli.command, &config, output_format).await {
        output::print_error(&e, output_format);
        std::process::exit(1);
    }

    Ok(())
}

async fn execute_command(command: Commands, config: &Config, format: OutputFormat) -> Result<()> {
    match command {
        Commands::Config { action } => {
            execute_config_command(action, config)?;
        }
        Commands::Key(key_cmd) => {
            execute_key_command(key_cmd, format).await?;
        }
        _ => {
            // Connect to server for all other commands
            let client = Client::connect(&config.server_url, config.default_collection.clone())
                .await
                .context("Failed to connect to AmateRS server")?;

            match command {
                Commands::Set { key, value } => {
                    execute_set(&client, &key, &value, format).await?;
                }
                Commands::Get { key } => {
                    execute_get(&client, &key, format).await?;
                }
                Commands::Delete { key } => {
                    execute_delete(&client, &key, format).await?;
                }
                Commands::Range { start, end } => {
                    execute_range(&client, &start, &end, format).await?;
                }
                Commands::Query { filter } => {
                    execute_query(&client, &filter, format).await?;
                }
                Commands::Server(server_cmd) => {
                    execute_server_command(server_cmd, &client, format).await?;
                }
                Commands::Admin(admin_cmd) => {
                    execute_admin_command(admin_cmd, &client, format).await?;
                }
                Commands::Config { .. } | Commands::Key(_) => {
                    // Already handled above
                }
            }
        }
    }

    Ok(())
}

async fn execute_set(client: &Client, key: &str, value: &str, format: OutputFormat) -> Result<()> {
    let key = Key::from_str(key);
    // For now, we'll encrypt the value as UTF-8 bytes
    // In a real implementation, this would use proper FHE encryption
    let encrypted_value = CipherBlob::new(value.as_bytes().to_vec());

    client
        .set(&key, &encrypted_value)
        .await
        .context("Failed to set key-value pair")?;

    output::print_set_result(&key, format)?;

    Ok(())
}

async fn execute_get(client: &Client, key: &str, format: OutputFormat) -> Result<()> {
    let key = Key::from_str(key);

    let value = client.get(&key).await.context("Failed to get value")?;

    output::print_get_result(&key, value.as_ref(), format)?;

    Ok(())
}

async fn execute_delete(client: &Client, key: &str, format: OutputFormat) -> Result<()> {
    let key = Key::from_str(key);

    client.delete(&key).await.context("Failed to delete key")?;

    output::print_delete_result(&key, format)?;

    Ok(())
}

async fn execute_range(
    client: &Client,
    start: &str,
    end: &str,
    format: OutputFormat,
) -> Result<()> {
    let start_key = Key::from_str(start);
    let end_key = Key::from_str(end);

    let results = client
        .range(&start_key, &end_key)
        .await
        .context("Failed to execute range query")?;

    output::print_range_result(&results, format)?;

    Ok(())
}

async fn execute_query(client: &Client, filter: &str, format: OutputFormat) -> Result<()> {
    let results = client
        .query(filter)
        .await
        .context("Failed to execute query")?;

    output::print_range_result(&results, format)?;

    Ok(())
}

fn execute_config_command(action: ConfigAction, config: &Config) -> Result<()> {
    match action {
        ConfigAction::Show => {
            let config_str =
                toml::to_string_pretty(config).context("Failed to serialize configuration")?;
            println!("{}", config_str);
        }
        ConfigAction::Init => {
            let config_path = Config::config_path()?;
            if config_path.exists() {
                anyhow::bail!(
                    "Configuration file already exists at: {}",
                    config_path.display()
                );
            }

            config.save()?;
            println!("✓ Configuration initialized at: {}", config_path.display());
        }
    }

    Ok(())
}

async fn execute_key_command(command: KeyCommands, format: OutputFormat) -> Result<()> {
    let key_manager = keys::KeyManager::new().context("Failed to initialize key manager")?;

    match command {
        KeyCommands::Generate { name, description } => {
            let metadata =
                progress::with_spinner("Generating FHE keys (this may take a while)...", async {
                    key_manager.generate(&name, description)
                })
                .await?;

            output::print_success(
                &format!(
                    "Generated key '{}' ({} bytes)",
                    metadata.name, metadata.size_bytes
                ),
                format,
            )?;
        }
        KeyCommands::Import {
            name,
            file,
            description,
        } => {
            let metadata = key_manager
                .import(&name, &file, description)
                .context("Failed to import key")?;

            output::print_success(
                &format!("Imported key '{}' from {:?}", metadata.name, file),
                format,
            )?;
        }
        KeyCommands::Export { name, file } => {
            key_manager
                .export(&name, &file)
                .context("Failed to export key")?;

            output::print_success(&format!("Exported key '{}' to {:?}", name, file), format)?;
        }
        KeyCommands::List => {
            let keys = key_manager.list().context("Failed to list keys")?;

            output::print_value(&keys, format)?;
        }
        KeyCommands::Delete { name } => {
            key_manager.delete(&name).context("Failed to delete key")?;

            output::print_success(&format!("Deleted key '{}'", name), format)?;
        }
    }

    Ok(())
}

async fn execute_server_command(
    command: ServerCommands,
    client: &Client,
    format: OutputFormat,
) -> Result<()> {
    let server_manager = server::ServerManager::new(client);

    match command {
        ServerCommands::Status => {
            let status = server_manager
                .status()
                .await
                .context("Failed to get server status")?;

            output::print_value(&status, format)?;
        }
        ServerCommands::Health => {
            let health = server_manager
                .health()
                .await
                .context("Failed to perform health check")?;

            output::print_value(&health, format)?;
        }
        ServerCommands::Metrics => {
            let metrics = server_manager
                .metrics()
                .await
                .context("Failed to get server metrics")?;

            output::print_value(&metrics, format)?;
        }
        ServerCommands::Cluster => {
            let cluster = server_manager
                .cluster_info()
                .await
                .context("Failed to get cluster information")?;

            output::print_value(&cluster, format)?;
        }
        ServerCommands::Nodes => {
            let nodes = server_manager
                .nodes()
                .await
                .context("Failed to get node information")?;

            output::print_value(&nodes, format)?;
        }
    }

    Ok(())
}

async fn execute_admin_command(
    command: AdminCommands,
    client: &Client,
    format: OutputFormat,
) -> Result<()> {
    let admin_manager = admin::AdminManager::new(client);

    match command {
        AdminCommands::Backup { dest, incremental } => {
            let metadata = progress::with_spinner(
                "Creating backup...",
                admin_manager.backup(&dest, incremental),
            )
            .await?;

            output::print_value(&metadata, format)?;
        }
        AdminCommands::Restore { source } => {
            let result =
                progress::with_spinner("Restoring from backup...", admin_manager.restore(&source))
                    .await?;

            output::print_value(&result, format)?;
        }
        AdminCommands::Compact { collection } => {
            let result = progress::with_spinner(
                "Running compaction...",
                admin_manager.compact(collection.as_deref()),
            )
            .await?;

            output::print_value(&result, format)?;
        }
        AdminCommands::Stats => {
            let stats = admin_manager
                .stats()
                .await
                .context("Failed to get database statistics")?;

            output::print_value(&stats, format)?;
        }
        AdminCommands::Verify => {
            let result =
                progress::with_spinner("Verifying database integrity...", admin_manager.verify())
                    .await?;

            output::print_value(&result, format)?;
        }
        AdminCommands::Logs { lines, follow } => {
            let logs = admin_manager
                .logs(lines, follow)
                .await
                .context("Failed to get logs")?;

            for line in logs {
                println!("{}", line);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parsing() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn test_output_format_parsing() {
        assert!(OutputFormat::from_str("json").is_some());
        assert!(OutputFormat::from_str("table").is_some());
        assert!(OutputFormat::from_str("invalid").is_none());
    }
}
