//! AmateRS Server - Fully Homomorphic Encrypted Database Server
//!
//! Main binary entry point for the AmateRS server.

mod cli;

use amaters_server::config::ServerConfig;
use amaters_server::hot_reload::{HotReloadError, TlsCreds, spawn_config_reloader, spawn_tls_reloader};
use amaters_server::server::Server;
use amaters_server::shutdown::setup_signal_handlers;
use arc_swap::ArcSwap;
use cli::{Cli, Command, StatusFormat};
use parking_lot::RwLock;
use std::process;
use std::sync::Arc;
use tracing::{Level, error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Main entry point
#[tokio::main]
async fn main() {
    // Parse CLI arguments
    let cli = Cli::parse_args();

    // Setup logging early (will be reconfigured based on config for start command)
    setup_early_logging();

    // Execute command (clone command to avoid borrow issues)
    let command = cli.command.clone();
    let result = match command {
        Command::Start {
            foreground,
            generate_config,
        } => handle_start(cli, foreground, generate_config).await,
        Command::Stop { force, timeout } => handle_stop(cli, force, timeout).await,
        Command::Status { format } => handle_status(cli, format).await,
        Command::Version { verbose } => handle_version(verbose).await,
        Command::ValidateConfig { show } => handle_validate_config(cli, show).await,
    };

    // Handle result
    if let Err(e) = result {
        error!("Error: {}", e);
        process::exit(1);
    }
}

/// Setup early logging before configuration is loaded
fn setup_early_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false).with_thread_ids(false))
        .init();
}

/// Setup logging based on configuration
fn setup_logging(config: &ServerConfig) {
    let level = match config.logging.level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };

    let filter = EnvFilter::from_default_env().add_directive(level.into());

    let subscriber = tracing_subscriber::registry().with(filter);

    // Format based on config
    match config.logging.format.as_str() {
        "json" => {
            // JSON format requires tracing-subscriber json feature
            // For now, use compact format instead
            subscriber
                .with(fmt::layer().compact().with_target(true))
                .init();
        }
        "compact" => {
            subscriber
                .with(fmt::layer().compact().with_target(false))
                .init();
        }
        _ => {
            // Pretty format (default)
            subscriber
                .with(fmt::layer().with_target(false).with_thread_ids(false))
                .init();
        }
    }

    info!(
        "Logging initialized: level={}, format={}",
        config.logging.level, config.logging.format
    );
}

/// Handle start command
async fn handle_start(
    cli: Cli,
    _foreground: bool,
    generate_config: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Load or generate configuration
    let mut config = load_config(&cli, generate_config)?;

    // Apply CLI overrides
    apply_cli_overrides(&mut config, &cli);

    // Validate configuration
    config
        .validate()
        .map_err(|e| format!("Configuration validation failed: {}", e))?;

    // Check if already running
    if Server::is_running(&config) {
        return Err("Server is already running".into());
    }

    // Shared mutable config for hot-reload support.
    let shared_config = Arc::new(RwLock::new(config.clone()));

    // Spawn SIGHUP config reloader.
    //
    // NOTE: Server holds an immutable Arc<ServerConfig> snapshot built at
    // start time; SIGHUP reload updates `shared_config` but the Server's
    // internal snapshot remains frozen until it is refactored to read from
    // a shared lock at runtime (Phase 9 work).  The reloadable sections
    // (logging, metrics, rate-limits, compaction) will take effect for any
    // code that reads from `shared_config` directly.
    let config_path = cli.config_path();
    if config_path.exists() {
        spawn_config_reloader(config_path.to_path_buf(), Arc::clone(&shared_config)).await;
    } else {
        warn!(
            "Config file not found at {:?}; SIGHUP reload will be a no-op",
            config_path
        );
    }

    // Spawn TLS certificate file watcher when TLS is enabled.
    //
    // NOTE: tonic's ServerTlsConfig is consumed once at serve_with_shutdown
    // time.  The ArcSwap<TlsCreds> store is kept live so a custom rustls
    // acceptor (Phase 9) can read the latest credentials per-handshake.
    // Until that acceptor is wired, this watcher logs rotations but the
    // live server continues using the cert negotiated at startup.
    if config.network.tls_enabled {
        if let (Some(cert_path), Some(key_path)) =
            (&config.network.tls_cert, &config.network.tls_key)
        {
            match TlsCreds::load_from_files(cert_path, key_path) {
                Ok(initial_creds) => {
                    let tls_store: Arc<ArcSwap<TlsCreds>> =
                        Arc::new(ArcSwap::from_pointee(initial_creds));
                    match spawn_tls_reloader(
                        cert_path.clone(),
                        key_path.clone(),
                        Arc::clone(&tls_store),
                    )
                    .await
                    {
                        Ok(()) => {
                            info!(
                                "TLS file watcher active — live cert rotation requires \
                                 custom rustls acceptor (Phase 9)"
                            );
                        }
                        Err(e) => {
                            warn!("TLS reloader could not start, continuing without live cert rotation: {}", e);
                        }
                    }
                }
                Err(e) => {
                    return Err(
                        format!("Failed to load initial TLS credentials: {}", e).into()
                    );
                }
            }
        }
    }

    // Create and initialize server
    let mut server = Server::new(config.clone());
    server
        .initialize()
        .await
        .map_err(|e| format!("Server initialization failed: {}", e))?;

    // Setup signal handlers (SIGTERM / SIGINT for graceful shutdown)
    let shutdown_coordinator = server.shutdown_coordinator().clone();
    setup_signal_handlers(shutdown_coordinator.clone()).await;

    // Write PID file
    Server::write_pid_file(&config).map_err(|e| format!("Failed to write PID file: {}", e))?;

    // Start server
    info!("Starting server...");
    let start_result = server.start().await;

    // Cleanup
    if let Err(ref e) = start_result {
        error!("Server error: {}", e);
    }

    // Shutdown
    info!("Initiating shutdown...");
    if let Err(e) = server.shutdown().await {
        error!("Shutdown error: {}", e);
    }

    // Remove PID file
    Server::remove_pid_file(&config).map_err(|e| format!("Failed to remove PID file: {}", e))?;

    start_result.map_err(|e| e.into())
}

/// Handle stop command
async fn handle_stop(
    cli: Cli,
    force: bool,
    _timeout: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config(&cli, false)?;

    if !Server::is_running(&config) {
        info!("Server is not running");
        return Ok(());
    }

    info!("Stopping server...");
    Server::stop_server(&config, force).map_err(|e| format!("Failed to stop server: {}", e))?;

    info!("Server stopped successfully");
    Ok(())
}

/// Handle status command
async fn handle_status(cli: Cli, format: String) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config(&cli, false)?;
    let format = StatusFormat::from_str(&format).map_err(|e| format!("Invalid format: {}", e))?;

    let is_running = Server::is_running(&config);

    match format {
        StatusFormat::Human => {
            println!("AmateRS Server Status");
            println!("=====================");
            println!("Status: {}", if is_running { "Running" } else { "Stopped" });
            println!("PID file: {}", config.server.pid_file.display());
            println!("Data directory: {}", config.server.data_dir.display());
            println!("Bind address: {}", config.server.bind_address);
        }
        StatusFormat::Json => {
            let status = serde_json::json!({
                "status": if is_running { "running" } else { "stopped" },
                "pid_file": config.server.pid_file,
                "data_dir": config.server.data_dir,
                "bind_address": config.server.bind_address,
            });
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
    }

    Ok(())
}

/// Handle version command
async fn handle_version(verbose: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("AmateRS Server {}", env!("CARGO_PKG_VERSION"));
    println!("Copyright (c) 2024-2026 COOLJAPAN OU (Team KitaSan)");

    if verbose {
        println!("\nComponent Versions:");
        println!("  amaters-core: {}", amaters_core::VERSION);
        println!("  amaters-net: {}", amaters_net::VERSION);
        #[cfg(feature = "cluster")]
        println!("  amaters-cluster: {}", amaters_cluster::VERSION);
        println!("\nBuild Information:");
        println!("  Rust version: {}", env!("CARGO_PKG_RUST_VERSION"));
        println!(
            "  Build profile: {}",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        if let Ok(target) = std::env::var("TARGET") {
            println!("  Target: {}", target);
        }
    }

    Ok(())
}

/// Handle validate-config command
async fn handle_validate_config(cli: Cli, show: bool) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config(&cli, false)?;

    config
        .validate()
        .map_err(|e| format!("Configuration validation failed: {}", e))?;

    info!("Configuration is valid");

    if show {
        let toml_str = toml::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        println!("\nConfiguration:");
        println!("{}", toml_str);
    }

    Ok(())
}

/// Load configuration from file or defaults
fn load_config(cli: &Cli, generate: bool) -> Result<ServerConfig, Box<dyn std::error::Error>> {
    let config_path = cli.config_path();

    if generate && !config_path.exists() {
        info!(
            "Generating default configuration at {}",
            config_path.display()
        );
        let config = ServerConfig::default();
        config
            .save_to_file(config_path)
            .map_err(|e| format!("Failed to save config: {}", e))?;
        return Ok(config);
    }

    if config_path.exists() {
        info!("Loading configuration from {}", config_path.display());
        ServerConfig::from_file_with_env(config_path).map_err(|e| e.into())
    } else if cli.has_config_override() {
        Err(format!("Configuration file not found: {}", config_path.display()).into())
    } else {
        info!("Using default configuration");
        Ok(ServerConfig::default())
    }
}

/// Apply CLI argument overrides to configuration
fn apply_cli_overrides(config: &mut ServerConfig, cli: &Cli) {
    if let Some(ref bind) = cli.bind {
        config.server.bind_address = bind.clone();
    }

    if let Some(ref data_dir) = cli.data_dir {
        config.server.data_dir = data_dir.clone();
    }

    if let Some(ref log_level) = cli.log_level {
        config.logging.level = log_level.clone();
    }
}
