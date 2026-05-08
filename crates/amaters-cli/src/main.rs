//! AmateRS CLI tool
//!
//! Command-line interface for interacting with AmateRS encrypted database.

mod admin;
mod batch;
mod client;
mod config;
mod diff;
mod keys;
mod output;
mod progress;
mod repl;
mod server;

use amaters_core::{CipherBlob, Key};
use amaters_sdk_rust::PaginationConfig;
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use client::Client;
use config::Config;
use output::OutputFormat;
use std::path::PathBuf;

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
        /// Maximum results to return
        #[arg(long, help = "Maximum results to return")]
        limit: Option<u64>,
        /// Number of results to skip
        #[arg(long, help = "Number of results to skip")]
        offset: Option<u64>,
        /// Continuation cursor for next page
        #[arg(long, help = "Continuation cursor for next page")]
        cursor: Option<String>,
    },

    /// Prefix scan (paginated)
    Scan {
        /// Key prefix to scan
        prefix: String,
        /// Maximum results to return
        #[arg(long, help = "Maximum results to return")]
        limit: Option<u64>,
        /// Number of results to skip
        #[arg(long, help = "Number of results to skip")]
        offset: Option<u64>,
        /// Continuation cursor for next page
        #[arg(long, help = "Continuation cursor for next page")]
        cursor: Option<String>,
    },

    /// Query with filter expression
    Query {
        /// Filter expression (AQL syntax)
        filter: String,
        /// Maximum results to return
        #[arg(long, help = "Maximum results to return")]
        limit: Option<u64>,
        /// Number of results to skip
        #[arg(long, help = "Number of results to skip")]
        offset: Option<u64>,
        /// Continuation cursor for next page
        #[arg(long, help = "Continuation cursor for next page")]
        cursor: Option<String>,
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

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for (bash, zsh, fish, powershell, elvish)
        #[arg(value_enum)]
        shell: clap_complete::Shell,
        /// Output file path (default: stdout)
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },

    /// Start interactive REPL mode
    Interactive {
        /// Server URL to connect to
        #[arg(short = 'u', long, default_value = "http://localhost:7878")]
        server: String,
    },

    /// Process batch operations from a file or stdin
    ///
    /// Line format: `<op> <key> [value]`  (op = "put" | "delete")
    ///
    /// Examples:
    ///   amaters-cli batch ops.txt
    ///   amaters-cli batch -
    Batch {
        /// Source file path, or "-" to read from stdin
        source: String,
    },

    /// Re-execute a command every <interval_secs> seconds
    ///
    /// Example:
    ///   amaters-cli watch 5 server status
    Watch {
        /// Interval in seconds between executions
        interval_secs: u64,
        /// Command arguments to execute repeatedly (e.g. "server status")
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

    /// Compare two keys (live) or two snapshot files (NDJSON on disk).
    ///
    /// Snapshot file format: one JSON object per line: `{"key":"...","value":"..."}`.
    /// Supports unified diff (default), JSON diff, and summary stats.
    ///
    /// Note: FHE ciphertexts cannot be diffed in plaintext form. Decrypt first.
    Diff {
        /// First file or key reference (`collection/key`)
        a: String,
        /// Second file or key reference (`collection/key`)
        b: String,
        /// Treat arguments as snapshot files rather than live keys
        #[arg(long)]
        files: bool,
        /// Output format: unified (default), json, stats
        #[arg(long, default_value = "unified")]
        diff_format: String,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration
    Show {
        /// Path to config file (default: ~/.amaters/config.toml)
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,
        /// Output format: toml, json, yaml
        #[arg(short, long, default_value = "toml")]
        format: String,
        /// Show only a specific section (server, storage, network, cluster, logging, metrics)
        #[arg(short, long)]
        section: Option<String>,
    },
    /// Initialize a config file with default template
    Init {
        /// Path to config file (default: ~/.amaters/config.toml)
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,
        /// Overwrite existing config file
        #[arg(long)]
        force: bool,
    },
    /// Validate a config file
    Validate {
        /// Path to config file (default: ~/.amaters/config.toml)
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,
    },
    /// Get a config value by dot-notation key (e.g. server.port)
    Get {
        /// Key in dot notation
        key: String,
        /// Path to config file
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,
    },
    /// Set a config value by dot-notation key
    Set {
        /// Key in dot notation
        key: String,
        /// Value to set
        value: String,
        /// Path to config file
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,
    },
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

    /// Get or set the default FHE key used when no --key flag is given.
    ///
    /// Examples:
    ///   amaters-cli key default my-key          # set default
    ///   amaters-cli key default --clear         # unset default
    ///   amaters-cli key default --show          # display current default
    ///   amaters-cli key default                 # same as --show
    ///
    /// Note: FHE ciphertexts cannot be diffed in plaintext form. If you need
    /// to diff encrypted values, decrypt them first with the appropriate key.
    Default {
        /// Key name to set as default (omit to display current default)
        name: Option<String>,
        /// Clear the current default key setting
        #[arg(long)]
        clear: bool,
        /// Show the current default key (default behaviour when no name given)
        #[arg(long)]
        show: bool,
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
    if let Err(e) = execute_command(cli.command, &mut config, output_format).await {
        output::print_error(&e, output_format);
        std::process::exit(1);
    }

    Ok(())
}

async fn execute_command(
    command: Commands,
    config: &mut Config,
    format: OutputFormat,
) -> Result<()> {
    match command {
        Commands::Config { action } => {
            execute_config_command(action, config)?;
        }
        Commands::Key(key_cmd) => {
            execute_key_command(key_cmd, config, format).await?;
        }
        Commands::Completions { shell, output } => {
            generate_completions(shell, output)?;
        }
        Commands::Interactive { server } => {
            let repl_config = repl::ReplConfig {
                server_url: server,
                default_collection: config.default_collection.clone(),
                output_format: format,
                ..repl::ReplConfig::default()
            };
            let mut repl_instance = repl::Repl::new(repl_config);
            repl_instance.run().await.context("REPL session failed")?;
        }
        Commands::Watch {
            interval_secs,
            args,
        } => {
            execute_watch(interval_secs, args, config, format).await?;
        }
        Commands::Diff {
            a,
            b,
            files,
            diff_format,
        } => {
            execute_diff(a, b, files, &diff_format).await?;
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
                Commands::Range {
                    start,
                    end,
                    limit,
                    offset,
                    cursor,
                } => {
                    execute_range(&client, &start, &end, limit, offset, cursor, format).await?;
                }
                Commands::Scan {
                    prefix,
                    limit,
                    offset,
                    cursor,
                } => {
                    execute_scan(&client, &prefix, limit, offset, cursor, format).await?;
                }
                Commands::Query {
                    filter,
                    limit,
                    offset,
                    cursor,
                } => {
                    execute_query(&client, &filter, limit, offset, cursor, format).await?;
                }
                Commands::Server(server_cmd) => {
                    execute_server_command(server_cmd, &client, format).await?;
                }
                Commands::Admin(admin_cmd) => {
                    execute_admin_command(admin_cmd, &client, format).await?;
                }
                Commands::Batch { source } => {
                    execute_batch(&client, &source, format).await?;
                }
                Commands::Config { .. }
                | Commands::Key(_)
                | Commands::Completions { .. }
                | Commands::Interactive { .. }
                | Commands::Watch { .. }
                | Commands::Diff { .. } => {
                    // Already handled above
                }
            }
        }
    }

    Ok(())
}

/// Generate shell completions and write them to stdout or a file.
///
/// When writing to stdout, installation hints are printed as comments
/// after the completion script.
fn generate_completions(shell: clap_complete::Shell, output: Option<PathBuf>) -> Result<()> {
    use clap_complete::Shell;

    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "amaters-cli", &mut buf);

    match output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("Failed to create directory: {}", parent.display())
                    })?;
                }
            }
            std::fs::write(&path, &buf)
                .with_context(|| format!("Failed to write completions to {}", path.display()))?;
            eprintln!("Completions written to {}", path.display());
        }
        None => {
            use std::io::Write;
            std::io::stdout()
                .write_all(&buf)
                .context("Failed to write completions to stdout")?;

            // Print installation hints as comments to stderr so they don't
            // interfere with piping the completion script.
            let hints = match shell {
                Shell::Bash => {
                    "# Installation:\n\
                     #   source <(amaters-cli completions bash)\n\
                     # Or persist:\n\
                     #   amaters-cli completions bash > ~/.local/share/bash-completion/completions/amaters-cli"
                }
                Shell::Zsh => {
                    "# Installation:\n\
                     #   amaters-cli completions zsh > ~/.zfunc/_amaters-cli\n\
                     #   # Then add to ~/.zshrc: fpath+=~/.zfunc; autoload -Uz compinit && compinit"
                }
                Shell::Fish => {
                    "# Installation:\n\
                     #   amaters-cli completions fish > ~/.config/fish/completions/amaters-cli.fish"
                }
                Shell::PowerShell => {
                    "# Installation:\n\
                     #   amaters-cli completions powershell >> $PROFILE"
                }
                Shell::Elvish => {
                    "# Installation:\n\
                     #   amaters-cli completions elvish > ~/.config/elvish/lib/amaters-cli.elv"
                }
                _ => "",
            };
            if !hints.is_empty() {
                eprintln!("\n{hints}");
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

/// Build a `PaginationConfig` from optional CLI pagination flags.
fn build_pagination(
    limit: Option<u64>,
    offset: Option<u64>,
    cursor: Option<String>,
) -> PaginationConfig {
    let mut cfg = PaginationConfig::default();
    if let Some(l) = limit {
        cfg.page_size = l as usize;
    }
    if let Some(o) = offset {
        cfg.offset = o as usize;
    }
    if let Some(c) = cursor {
        cfg.cursor = Some(c);
    }
    cfg
}

async fn execute_range(
    client: &Client,
    start: &str,
    end: &str,
    limit: Option<u64>,
    offset: Option<u64>,
    cursor: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let start_key = Key::from_str(start);
    let end_key = Key::from_str(end);

    // Use paginated path when any pagination flag is present, otherwise use the
    // simple (non-paginated) path for backwards-compatible behaviour.
    if limit.is_some() || offset.is_some() || cursor.is_some() {
        let pagination = build_pagination(limit, offset, cursor);
        let result = client
            .range_paginated(&start_key, &end_key, &pagination)
            .await
            .context("Failed to execute range query")?;
        output::print_paginated_result(&result.items, result.next_cursor.as_deref(), format)?;
    } else {
        let results = client
            .range(&start_key, &end_key)
            .await
            .context("Failed to execute range query")?;
        output::print_range_result(&results, format)?;
    }

    Ok(())
}

async fn execute_scan(
    client: &Client,
    prefix: &str,
    limit: Option<u64>,
    offset: Option<u64>,
    cursor: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let prefix_key = Key::from_str(prefix);
    let pagination = build_pagination(limit, offset, cursor);

    let result = client
        .scan(&prefix_key, &pagination)
        .await
        .context("Failed to execute scan")?;

    output::print_paginated_result(&result.items, result.next_cursor.as_deref(), format)?;

    Ok(())
}

async fn execute_query(
    client: &Client,
    filter: &str,
    _limit: Option<u64>,
    _offset: Option<u64>,
    _cursor: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let results = client
        .query(filter)
        .await
        .context("Failed to execute query")?;

    output::print_range_result(&results, format)?;

    Ok(())
}

async fn execute_batch(client: &Client, source: &str, _format: OutputFormat) -> Result<()> {
    use batch::{BatchCommand, BatchSource};

    let batch_source = if source == "-" {
        BatchSource::Stdin
    } else {
        BatchSource::File(std::path::PathBuf::from(source))
    };

    let cmd = BatchCommand::new(batch_source);
    let stats = cmd
        .execute(client)
        .await
        .context("Batch operation failed")?;

    println!(
        "Batch complete: {} total, {} succeeded, {} failed, {} skipped",
        stats.total, stats.succeeded, stats.failed, stats.skipped
    );

    Ok(())
}

/// Core watch loop — calls `f` every `interval` until `f` returns `false` or a
/// cancellation signal is received.
///
/// Extracted as a standalone async function so it can be tested with a simple
/// closure without involving CLI parsing or a real server.
///
/// The closure must return a `Pin<Box<dyn Future<Output = bool>>>` to allow
/// recursive call chains (e.g., watch re-dispatching to `execute_command`).
pub async fn watch_loop<F>(interval: std::time::Duration, mut f: F)
where
    F: FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool>>>,
{
    use tokio::time::MissedTickBehavior;

    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let keep_going = f().await;
                if !keep_going {
                    break;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }
}

async fn execute_watch(
    interval_secs: u64,
    args: Vec<String>,
    config: &Config,
    format: OutputFormat,
) -> Result<()> {
    use std::io::Write;

    if args.is_empty() {
        anyhow::bail!("watch requires at least one command argument");
    }

    let interval = std::time::Duration::from_secs(interval_secs);
    let config = config.clone();

    // Prefix the binary name so Cli::try_parse_from works correctly.
    let mut full_args = vec!["amaters-cli".to_string()];
    full_args.extend(args.iter().cloned());

    watch_loop(interval, || {
        let full_args = full_args.clone();
        let config = config.clone();
        Box::pin(async move {
            // Clear screen + cursor home (ANSI)
            print!("\x1b[2J\x1b[H");
            if let Err(e) = std::io::stdout().flush() {
                eprintln!("flush error: {e}");
            }

            match Cli::try_parse_from(full_args) {
                Ok(cli) => {
                    let mut cfg = config.clone();
                    if let Some(s) = cli.server {
                        cfg.server_url = s;
                    }
                    if let Some(c) = cli.collection {
                        cfg.default_collection = c;
                    }
                    let fmt = cli
                        .format
                        .as_deref()
                        .and_then(OutputFormat::from_str)
                        .unwrap_or(format);
                    if let Err(e) = execute_command(cli.command, &mut cfg, fmt).await {
                        output::print_error(&e, fmt);
                    }
                }
                Err(e) => {
                    eprintln!("watch: failed to parse command: {e}");
                }
            }
            true // keep looping
        })
    })
    .await;

    Ok(())
}

fn execute_config_command(action: ConfigAction, _config: &Config) -> Result<()> {
    match action {
        ConfigAction::Show {
            path,
            format,
            section,
        } => {
            config::cmd_show(path.as_deref(), &format, section.as_deref())?;
        }
        ConfigAction::Init { path, force } => {
            config::cmd_init(path.as_deref(), force)?;
        }
        ConfigAction::Validate { path } => {
            config::cmd_validate(path.as_deref())?;
        }
        ConfigAction::Get { key, path } => {
            config::cmd_get(&key, path.as_deref())?;
        }
        ConfigAction::Set { key, value, path } => {
            config::cmd_set(&key, &value, path.as_deref())?;
        }
    }

    Ok(())
}

async fn execute_diff(a: String, b: String, files: bool, diff_format_str: &str) -> Result<()> {
    use diff::{DiffFormat, DiffMode};

    let format = match diff_format_str.to_lowercase().as_str() {
        "json" => DiffFormat::Json,
        "stats" => DiffFormat::Stats,
        _ => DiffFormat::Unified,
    };

    let mode = if files {
        DiffMode::Snapshots {
            a: PathBuf::from(&a),
            b: PathBuf::from(&b),
        }
    } else {
        // Parse `collection/key` notation.
        let (coll_a, key_a) = parse_collection_key(&a)?;
        let (coll_b, key_b) = parse_collection_key(&b)?;
        DiffMode::Keys {
            collection_a: coll_a,
            key_a,
            collection_b: coll_b,
            key_b,
        }
    };

    let output = diff::run_diff(mode, format).await?;
    print!("{}", output);
    Ok(())
}

/// Parse a `collection/key` string into `(collection, key)`.
/// Falls back to `("default", key)` if no slash is present.
fn parse_collection_key(s: &str) -> Result<(String, String)> {
    match s.find('/') {
        Some(pos) => {
            let collection = s[..pos].to_string();
            let key = s[pos + 1..].to_string();
            if collection.is_empty() || key.is_empty() {
                anyhow::bail!("Invalid collection/key reference: '{}'", s);
            }
            Ok((collection, key))
        }
        None => Ok(("default".to_string(), s.to_string())),
    }
}

async fn execute_key_command(
    command: KeyCommands,
    config: &mut Config,
    format: OutputFormat,
) -> Result<()> {
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
        KeyCommands::Default { name, clear, show } => {
            let config_path = Config::config_path().context("Failed to determine config path")?;
            keys::handle_key_default(config, &config_path, name, clear, show)?;
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
    use clap_complete::Shell;

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

    /// Helper: generate completions for a given shell into a buffer.
    fn generate_to_buf(shell: Shell) -> Vec<u8> {
        let mut buf = Vec::new();
        clap_complete::generate(shell, &mut Cli::command(), "amaters-cli", &mut buf);
        buf
    }

    // --- Completion tests (10+) ---

    #[test]
    fn test_completions_bash() {
        let buf = generate_to_buf(Shell::Bash);
        assert!(!buf.is_empty(), "Bash completions should not be empty");
        let text = String::from_utf8_lossy(&buf);
        assert!(
            text.contains("amaters-cli"),
            "Bash completions should reference the binary name"
        );
    }

    #[test]
    fn test_completions_zsh() {
        let buf = generate_to_buf(Shell::Zsh);
        assert!(!buf.is_empty(), "Zsh completions should not be empty");
        let text = String::from_utf8_lossy(&buf);
        assert!(
            text.contains("amaters-cli"),
            "Zsh completions should reference the binary name"
        );
    }

    #[test]
    fn test_completions_fish() {
        let buf = generate_to_buf(Shell::Fish);
        assert!(!buf.is_empty(), "Fish completions should not be empty");
        let text = String::from_utf8_lossy(&buf);
        assert!(
            text.contains("amaters-cli"),
            "Fish completions should reference the binary name"
        );
    }

    #[test]
    fn test_completions_powershell() {
        let buf = generate_to_buf(Shell::PowerShell);
        assert!(
            !buf.is_empty(),
            "PowerShell completions should not be empty"
        );
        let text = String::from_utf8_lossy(&buf);
        assert!(
            text.contains("amaters-cli"),
            "PowerShell completions should reference the binary name"
        );
    }

    #[test]
    fn test_completions_elvish() {
        let buf = generate_to_buf(Shell::Elvish);
        assert!(!buf.is_empty(), "Elvish completions should not be empty");
    }

    #[test]
    fn test_completions_to_file() {
        let dir = std::env::temp_dir().join("amaters_test_completions_to_file");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("amaters-cli.bash");

        let result = generate_completions(Shell::Bash, Some(file_path.clone()));
        assert!(
            result.is_ok(),
            "generate_completions to file should succeed"
        );

        let content =
            std::fs::read_to_string(&file_path).expect("completion file should be readable");
        assert!(
            !content.is_empty(),
            "Written completion file should not be empty"
        );
        assert!(
            content.contains("amaters-cli"),
            "Written completion file should contain binary name"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_completions_contains_subcommands() {
        let buf = generate_to_buf(Shell::Bash);
        let text = String::from_utf8_lossy(&buf);
        // The completion script should reference known subcommands
        for subcmd in &["set", "get", "delete", "completions", "interactive"] {
            assert!(
                text.contains(subcmd),
                "Bash completions should contain subcommand '{subcmd}'"
            );
        }
    }

    #[test]
    fn test_completions_contains_flags() {
        let buf = generate_to_buf(Shell::Bash);
        let text = String::from_utf8_lossy(&buf);
        for flag in &["--help", "--version"] {
            assert!(
                text.contains(flag),
                "Bash completions should contain flag '{flag}'"
            );
        }
    }

    #[test]
    fn test_completions_binary_name() {
        // Verify that all shells use the correct binary name
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell] {
            let buf = generate_to_buf(shell);
            let text = String::from_utf8_lossy(&buf);
            assert!(
                text.contains("amaters-cli"),
                "Completions for {shell:?} must contain binary name 'amaters-cli'"
            );
        }
    }

    #[test]
    fn test_completions_overwrite_file() {
        let dir = std::env::temp_dir().join("amaters_test_completions_overwrite");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("amaters-cli.zsh");

        // Write once
        let r1 = generate_completions(Shell::Zsh, Some(file_path.clone()));
        assert!(r1.is_ok(), "First write should succeed");
        let content1 = std::fs::read(&file_path).expect("should read first write");

        // Write again (overwrite)
        let r2 = generate_completions(Shell::Zsh, Some(file_path.clone()));
        assert!(r2.is_ok(), "Second write (overwrite) should succeed");
        let content2 = std::fs::read(&file_path).expect("should read second write");

        assert_eq!(
            content1, content2,
            "Overwritten file should have same content"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_completions_cli_integration() {
        // Parse the completions subcommand through clap
        let result = Cli::try_parse_from(["amaters-cli", "completions", "bash"]);
        assert!(result.is_ok(), "Parsing 'completions bash' should succeed");
        let cli = result.expect("already asserted Ok");
        match cli.command {
            Commands::Completions { shell, output } => {
                assert_eq!(shell, Shell::Bash);
                assert!(output.is_none());
            }
            _ => panic!("Expected Completions command"),
        }
    }

    #[test]
    fn test_completions_cli_with_output_flag() {
        let result = Cli::try_parse_from([
            "amaters-cli",
            "completions",
            "fish",
            "--output",
            "/tmp/test.fish",
        ]);
        assert!(
            result.is_ok(),
            "Parsing 'completions fish --output ...' should succeed"
        );
        let cli = result.expect("already asserted Ok");
        match cli.command {
            Commands::Completions { shell, output } => {
                assert_eq!(shell, Shell::Fish);
                assert_eq!(output, Some(PathBuf::from("/tmp/test.fish")));
            }
            _ => panic!("Expected Completions command"),
        }
    }

    #[test]
    fn test_completions_creates_parent_dirs() {
        let dir = std::env::temp_dir()
            .join("amaters_test_parent_dirs")
            .join("nested")
            .join("deeply");
        let file_path = dir.join("comp.bash");

        // Ensure the directory does not already exist
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("amaters_test_parent_dirs"));

        let result = generate_completions(Shell::Bash, Some(file_path.clone()));
        assert!(
            result.is_ok(),
            "Should create parent directories automatically"
        );
        assert!(
            file_path.exists(),
            "Completion file should exist after generation"
        );

        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("amaters_test_parent_dirs"));
    }

    #[test]
    fn test_completions_invalid_shell_rejected() {
        let result = Cli::try_parse_from(["amaters-cli", "completions", "invalid_shell"]);
        assert!(
            result.is_err(),
            "An invalid shell name should be rejected by clap"
        );
    }

    // -----------------------------------------------------------------------
    // E1: Pagination flag tests
    // -----------------------------------------------------------------------

    /// Parsing `range --limit 10` should produce a `PaginationConfig` with `page_size == 10`.
    #[test]
    fn test_pagination_limit_respected() {
        let cli = Cli::try_parse_from(["amaters-cli", "range", "start", "end", "--limit", "10"])
            .expect("parse should succeed");

        match cli.command {
            Commands::Range {
                limit,
                offset,
                cursor,
                ..
            } => {
                assert_eq!(limit, Some(10));
                assert_eq!(offset, None);
                assert_eq!(cursor, None);

                // Verify build_pagination maps to PaginationConfig correctly.
                let pag = build_pagination(limit, offset, cursor);
                assert_eq!(pag.page_size, 10);
                assert_eq!(pag.offset, 0);
                assert!(pag.cursor.is_none());
            }
            _ => panic!("Expected Range command"),
        }
    }

    /// Parsing `range --offset 5 --cursor tok` should reflect in PaginationConfig.
    #[test]
    fn test_pagination_offset_and_cursor() {
        let cli = Cli::try_parse_from([
            "amaters-cli",
            "range",
            "start",
            "end",
            "--offset",
            "5",
            "--cursor",
            "tok123",
        ])
        .expect("parse should succeed");

        match cli.command {
            Commands::Range {
                limit,
                offset,
                cursor,
                ..
            } => {
                let pag = build_pagination(limit, offset, cursor);
                assert_eq!(pag.offset, 5);
                assert_eq!(pag.cursor.as_deref(), Some("tok123"));
            }
            _ => panic!("Expected Range command"),
        }
    }

    /// `scan --limit 20` should parse correctly.
    #[test]
    fn test_scan_pagination_flags() {
        let cli = Cli::try_parse_from([
            "amaters-cli",
            "scan",
            "prefix:",
            "--limit",
            "20",
            "--offset",
            "0",
        ])
        .expect("parse should succeed");

        match cli.command {
            Commands::Scan {
                limit,
                offset,
                cursor,
                ..
            } => {
                assert_eq!(limit, Some(20));
                assert_eq!(offset, Some(0));
                assert!(cursor.is_none());
            }
            _ => panic!("Expected Scan command"),
        }
    }

    /// `print_paginated_result` with a next cursor should include "Next cursor:" in table output.
    #[test]
    fn test_pagination_cursor_output_format() {
        // Capture is not straightforward in unit tests; we just verify the
        // function does not panic and returns Ok when a next cursor is present.
        let result = output::print_paginated_result(&[], Some("cursor_abc"), OutputFormat::Table);
        assert!(result.is_ok(), "print_paginated_result should succeed");
    }

    // -----------------------------------------------------------------------
    // E3: Watch mode tests
    // -----------------------------------------------------------------------

    /// Parsing the `watch` subcommand should succeed and extract interval and args.
    #[test]
    fn test_watch_cli_parsing() {
        let cli = Cli::try_parse_from(["amaters-cli", "watch", "5", "server", "status"])
            .expect("parse should succeed");

        match cli.command {
            Commands::Watch {
                interval_secs,
                args,
            } => {
                assert_eq!(interval_secs, 5);
                assert_eq!(args, vec!["server", "status"]);
            }
            _ => panic!("Expected Watch command"),
        }
    }

    /// `watch_loop` should invoke the closure at least twice within 150 ms at a 50 ms interval.
    #[tokio::test]
    async fn test_watch_executes_at_least_twice() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let interval = std::time::Duration::from_millis(50);
        let deadline = std::time::Duration::from_millis(150);

        let loop_fut = watch_loop(interval, move || {
            let c = counter_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                true // keep going
            })
        });

        // Run for ~150 ms then drop the future (cancel).
        let _ = tokio::time::timeout(deadline, loop_fut).await;

        let count = counter.load(Ordering::SeqCst);
        assert!(count >= 2, "expected >= 2 ticks, got {count}");
    }
}
