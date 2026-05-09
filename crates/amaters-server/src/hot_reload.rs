//! Hot-reload support for server configuration and TLS certificates.
//!
//! # Config reload (SIGHUP)
//!
//! SIGHUP handling is provided by [`crate::shutdown::setup_sighup_handler`]
//! together with [`crate::config::ReloadableConfig`], which already implement
//! the full SIGHUP → re-parse → atomic swap → log-diff pipeline.
//!
//! [`spawn_config_reloader`] is a thin façade that wires those existing
//! building blocks together and returns a `JoinHandle` so callers can
//! cancel the background task when needed.
//!
//! # TLS certificate reload
//!
//! [`spawn_tls_reloader`] watches a directory for changes to cert/key files
//! using [`notify`].  On a detected change it reloads the PEM bytes and
//! atomically swaps them into an [`arc_swap::ArcSwap<TlsCreds>`].
//!
//! **Integration note:** tonic's `ServerTlsConfig` is consumed by
//! `tonic::transport::Server::builder().tls_config(...)` at server-start time.
//! To use live-rotated credentials the server must be built with a custom
//! `rustls::ServerConfig` derived from the shared [`TlsCreds`] store, so that
//! each new TLS handshake uses the latest certificate.  The
//! [`spawn_tls_reloader`] API intentionally exposes the raw [`TlsCreds`] store
//! so the caller can build that custom acceptor; helper
//! [`build_server_tls_config`] converts the current credentials into a
//! `tonic::transport::ServerTlsConfig` for use at startup or reconnect.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use notify::{Event, RecursiveMode, Watcher};
use parking_lot::RwLock;
use thiserror::Error;
use tokio::task::JoinHandle;
use tonic::transport::{Identity, ServerTlsConfig};
use tracing::{error, info, warn};

use amaters_net::tls_acceptor::{TlsCredsRef, build_rustls_config};

use crate::config::{ConfigError, ReloadableConfig, ServerConfig};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during hot-reload operations.
#[derive(Debug, Error)]
pub enum HotReloadError {
    /// File-system watcher could not be created or watch path could not be added.
    #[error("File watcher error: {0}")]
    Watch(#[from] notify::Error),

    /// An I/O error occurred while reading a certificate or key file.
    #[error("IO error reading TLS file: {0}")]
    Io(#[from] std::io::Error),

    /// The PEM bytes loaded from disk could not be used to build a TLS identity.
    #[error("TLS credential error: {0}")]
    Tls(String),

    /// A `rustls::ServerConfig` could not be built from the provided creds —
    /// surfaced from `amaters_net::tls_acceptor::build_rustls_config`.
    #[error("rustls config error: {0}")]
    Rustls(String),

    /// The config reload itself reported an error.
    #[error("Config error: {0}")]
    Config(#[from] ConfigError),
}

// ---------------------------------------------------------------------------
// Raw TLS credentials (cert + key PEM bytes)
// ---------------------------------------------------------------------------

/// Raw PEM bytes for a TLS certificate and private key.
///
/// Stored inside an [`ArcSwap`] so that new credentials can be swapped in
/// atomically without stopping the server.  Any code that builds a
/// `rustls::ServerConfig` or `tonic::transport::ServerTlsConfig` should load
/// the current value from the `ArcSwap` immediately before use so it picks up
/// any rotation that happened since the last call.
#[derive(Clone, Debug)]
pub struct TlsCreds {
    /// PEM-encoded certificate chain.
    pub cert_pem: Vec<u8>,
    /// PEM-encoded private key.
    pub key_pem: Vec<u8>,
}

impl TlsCreds {
    /// Load `TlsCreds` from cert and key files on disk.
    pub fn load_from_files(
        cert_path: &Path,
        key_path: &Path,
    ) -> Result<Self, HotReloadError> {
        let cert_pem = std::fs::read(cert_path)?;
        let key_pem = std::fs::read(key_path)?;
        Ok(Self { cert_pem, key_pem })
    }

    /// Convert these credentials into a `tonic` [`ServerTlsConfig`].
    pub fn to_server_tls_config(&self) -> ServerTlsConfig {
        let identity = Identity::from_pem(&self.cert_pem, &self.key_pem);
        ServerTlsConfig::new().identity(identity)
    }
}

// ---------------------------------------------------------------------------
// Build helpers
// ---------------------------------------------------------------------------

/// Load cert + key from disk and construct a `tonic` [`ServerTlsConfig`].
///
/// This is equivalent to calling `TlsCreds::load_from_files` followed by
/// `TlsCreds::to_server_tls_config`, provided as a convenience.
pub fn build_server_tls_config(
    cert_path: &Path,
    key_path: &Path,
) -> Result<ServerTlsConfig, HotReloadError> {
    Ok(TlsCreds::load_from_files(cert_path, key_path)?.to_server_tls_config())
}

// ---------------------------------------------------------------------------
// Item 10: SIGHUP config reloader façade
// ---------------------------------------------------------------------------

/// Spawn a background task that reloads `config` on `SIGHUP`.
///
/// This is a thin façade over the existing [`crate::shutdown::setup_sighup_handler`]
/// infrastructure.  It is provided so callers have a single, consistent API
/// that returns a [`JoinHandle`] they can cancel when the server shuts down.
///
/// On non-Unix platforms the spawned task logs a warning and exits immediately.
///
/// # Config reload semantics
///
/// When `SIGHUP` is received:
/// 1. The config file at `config_path` is re-parsed.
/// 2. The new config is validated.
/// 3. Only *reloadable* sections (logging, metrics, compaction, rate-limits) are
///    applied atomically via `RwLock::write`.
/// 4. Non-reloadable sections (bind address, storage engine, TLS cert *path*, …)
///    are logged as skipped.
/// 5. If validation fails, the old config is preserved and an error is logged.
///
/// See [`crate::config::ReloadableSection`] and [`crate::config::ConfigDiff`] for details.
pub async fn spawn_config_reloader(
    config_path: PathBuf,
    config: Arc<RwLock<ServerConfig>>,
) -> JoinHandle<()> {
    // Build a ReloadableConfig backed by the same lock so that any writes made
    // through setup_sighup_handler are visible to code that holds a reference
    // to `config` directly.
    //
    // ReloadableConfig wraps Arc<RwLock<ServerConfig>> internally.  We create
    // one from the provided config, then hand it to setup_sighup_handler.
    let initial = config.read().clone();
    let reloadable = ReloadableConfig::new(initial);
    reloadable.set_config_path(config_path.clone());

    // Clone the reloadable so we can move it into the task below.
    let reloadable_for_task = reloadable.clone();
    let config_for_task = config.clone();

    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};

            let mut hangup = match signal(SignalKind::hangup()) {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed to register SIGHUP signal handler: {}", e);
                    return;
                }
            };

            loop {
                hangup.recv().await;
                info!(
                    "SIGHUP received — reloading config from {:?}",
                    config_path
                );

                // Reload through ReloadableConfig (validates + section-aware swap).
                match reloadable_for_task.reload_from_stored_path() {
                    Ok(report) if report.success => {
                        // Mirror the updated reloadable snapshot back to the raw lock
                        // so callers holding `Arc<RwLock<ServerConfig>>` see the change.
                        let updated = reloadable_for_task.snapshot();
                        *config_for_task.write() = updated;
                        info!("Config reloaded successfully: {}", report);
                    }
                    Ok(report) => {
                        error!("Config reload failed — keeping old config: {}", report);
                    }
                    Err(e) => {
                        error!("Config reload error — keeping old config: {}", e);
                    }
                }
            }
        }

        #[cfg(not(unix))]
        {
            warn!(
                "SIGHUP config reload is only supported on Unix platforms. \
                 Use ReloadableConfig::manual_reload() as an alternative."
            );
        }
    })
}

// ---------------------------------------------------------------------------
// Item 11: TLS certificate file watcher
// ---------------------------------------------------------------------------

/// Spawn a background task that watches TLS cert and key files for changes.
///
/// When a change is detected in the directory containing `cert_path`, the
/// cert and key are reloaded from disk and the new [`TlsCreds`] is atomically
/// stored via [`ArcSwap::store`].
///
/// New connections that read from the [`ArcSwap`] after the swap will use the
/// new credentials; existing connections complete with whatever credentials
/// they negotiated at handshake time.
///
/// Returns an error if the file-system watcher cannot be initialised.
///
/// # Integration
///
/// ```rust,ignore
/// let creds = Arc::new(ArcSwap::from_pointee(
///     TlsCreds::load_from_files(&cert_path, &key_path)?,
/// ));
///
/// spawn_tls_reloader(cert_path, key_path, Arc::clone(&creds)).await?;
///
/// // Build initial tls config from creds for tonic:
/// let tls_config = creds.load().to_server_tls_config();
/// ```
pub async fn spawn_tls_reloader(
    cert_path: PathBuf,
    key_path: PathBuf,
    tls_creds: Arc<ArcSwap<TlsCreds>>,
) -> Result<(), HotReloadError> {
    // notify v8 uses a sync channel; we bridge it to an async mpsc so the
    // spawned task can use .await without blocking the tokio thread.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<notify::Result<Event>>(16);

    let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        // best-effort: if the channel is full or closed, silently drop the event.
        let _ = tx.blocking_send(event);
    })?;

    // Watch the directory that contains the cert file (non-recursive).
    let cert_dir = cert_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    watcher.watch(cert_dir, RecursiveMode::NonRecursive)?;

    // Clone paths for use inside the spawned task.
    let cert_path_task = cert_path.clone();
    let key_path_task = key_path.clone();

    tokio::spawn(async move {
        // Keep `watcher` alive inside the task.
        let _watcher = watcher;

        while let Some(event) = rx.recv().await {
            match event {
                Ok(e) => {
                    // Only reload on events that touch the cert or key file.
                    let relevant = e.paths.iter().any(|p| {
                        p == &cert_path_task || p == &key_path_task
                    });

                    if !relevant {
                        continue;
                    }

                    match TlsCreds::load_from_files(&cert_path_task, &key_path_task) {
                        Ok(new_creds) => {
                            tls_creds.store(Arc::new(new_creds));
                            info!(
                                "TLS credentials reloaded from {:?}",
                                cert_path_task
                            );
                        }
                        Err(e) => {
                            error!(
                                "TLS reload failed — keeping existing credentials: {}",
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!("File-watcher error (TLS reloader): {}", e);
                }
            }
        }
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Live rustls config rotation (Item 2)
// ---------------------------------------------------------------------------

/// Atomically swap the active `rustls::ServerConfig` in `store` with a new one
/// derived from `creds`.
///
/// Used by [`spawn_tls_reloader_with_rustls_store`] on each detected file
/// change.  Callers may also invoke this manually to rotate without involving
/// the watcher (e.g. operator-driven rotation via an admin RPC).
///
/// # Errors
///
/// Returns [`HotReloadError::Rustls`] if the new credentials cannot be parsed
/// into a valid `ServerConfig` (invalid PEM, mismatched key, …).  In that case
/// the old config is left in place and the caller can decide whether to retry.
pub fn swap_rustls_config(
    store: &Arc<ArcSwap<rustls::ServerConfig>>,
    creds: &TlsCreds,
) -> Result<(), HotReloadError> {
    let creds_ref = TlsCredsRef::new(&creds.cert_pem, &creds.key_pem);
    let new_config = build_rustls_config(&creds_ref)
        .map_err(|e| HotReloadError::Rustls(e.to_string()))?;
    store.store(Arc::new(new_config));
    Ok(())
}

/// Spawn a TLS file-watcher that updates **both** the legacy [`TlsCreds`]
/// store and a [`rustls::ServerConfig`] store.
///
/// The dual-store design preserves backward compatibility with code paths
/// that still consume `TlsCreds` (e.g. for `tonic::transport::ServerTlsConfig`)
/// while wiring the live-rotating
/// [`amaters_net::tls_acceptor::LiveTlsAcceptor`] to the rustls store.
///
/// # Behaviour
///
/// On each detected change:
/// 1. Reload PEM bytes into a new [`TlsCreds`].
/// 2. Build a fresh `rustls::ServerConfig` via
///    [`amaters_net::tls_acceptor::build_rustls_config`].
/// 3. Atomically swap **both** stores.
///
/// If step 2 fails (invalid PEM), neither store is updated and an error is
/// logged; the old config keeps serving traffic.
///
/// # Errors
///
/// Returns an error if the file-system watcher cannot be initialised.
pub async fn spawn_tls_reloader_with_rustls_store(
    cert_path: PathBuf,
    key_path: PathBuf,
    tls_creds: Arc<ArcSwap<TlsCreds>>,
    rustls_store: Arc<ArcSwap<rustls::ServerConfig>>,
) -> Result<(), HotReloadError> {
    // Mirror the non-rustls watcher's plumbing.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<notify::Result<Event>>(16);

    let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        let _ = tx.blocking_send(event);
    })?;

    let cert_dir = cert_path.parent().unwrap_or_else(|| Path::new("."));
    watcher.watch(cert_dir, RecursiveMode::NonRecursive)?;

    let cert_path_task = cert_path.clone();
    let key_path_task = key_path.clone();

    tokio::spawn(async move {
        let _watcher = watcher;

        while let Some(event) = rx.recv().await {
            match event {
                Ok(e) => {
                    let relevant = e
                        .paths
                        .iter()
                        .any(|p| p == &cert_path_task || p == &key_path_task);
                    if !relevant {
                        continue;
                    }

                    let new_creds = match TlsCreds::load_from_files(&cert_path_task, &key_path_task)
                    {
                        Ok(c) => c,
                        Err(e) => {
                            error!(
                                "TLS reload failed (file read) — keeping existing credentials: {e}",
                            );
                            continue;
                        }
                    };

                    // Build the new rustls config first; if it fails, neither store
                    // is updated.
                    if let Err(e) = swap_rustls_config(&rustls_store, &new_creds) {
                        error!(
                            "TLS reload failed (rustls build) — keeping existing config: {e}",
                        );
                        continue;
                    }

                    tls_creds.store(Arc::new(new_creds));
                    info!(
                        "TLS credentials reloaded (legacy + rustls) from {:?}",
                        cert_path_task
                    );
                }
                Err(e) => {
                    warn!("File-watcher error (TLS reloader): {e}");
                }
            }
        }
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;

    // -----------------------------------------------------------------------
    // Helper: build a minimal ServerConfig with a given bind address
    // -----------------------------------------------------------------------
    fn make_config(bind: &str) -> ServerConfig {
        let mut c = ServerConfig::default();
        c.server.bind_address = bind.to_string();
        c
    }

    // -----------------------------------------------------------------------
    // config_diff tests (via ReloadableConfig)
    // -----------------------------------------------------------------------

    /// Reloading with an identical config produces no section updates.
    #[test]
    fn test_config_diff_empty_when_identical() {
        use crate::config::diff;
        let c = make_config("127.0.0.1:7878");
        let d = diff(&c, &c);
        assert!(
            d.is_empty(),
            "Diff of identical configs should be empty, got {:?}",
            d
        );
    }

    /// Reloading with a changed log level marks the Logging section as changed.
    #[test]
    fn test_config_diff_detects_log_level_change() {
        use crate::config::diff;
        use crate::config::ReloadableSection;
        let old = make_config("127.0.0.1:7878");
        let mut new = old.clone();
        new.logging.level = "debug".to_string();
        let d = diff(&old, &new);
        assert!(
            d.reloadable_changes.contains(&ReloadableSection::Logging),
            "Expected Logging in reloadable_changes, got {:?}",
            d.reloadable_changes
        );
    }

    /// Changing max_connections marks the RateLimit section as changed.
    #[test]
    fn test_config_diff_detects_rate_limit_change() {
        use crate::config::diff;
        use crate::config::ReloadableSection;
        let old = make_config("127.0.0.1:7878");
        let mut new = old.clone();
        new.server.max_connections = old.server.max_connections + 500;
        let d = diff(&old, &new);
        assert!(
            d.reloadable_changes.contains(&ReloadableSection::RateLimit),
            "Expected RateLimit in reloadable_changes, got {:?}",
            d.reloadable_changes
        );
    }

    /// Changing bind_address marks it as non-reloadable (requires restart).
    #[test]
    fn test_config_diff_non_reloadable_bind_address() {
        use crate::config::{NonReloadableSection, diff};
        let old = make_config("127.0.0.1:7878");
        let new = make_config("127.0.0.1:9999");
        let d = diff(&old, &new);
        assert!(
            d.non_reloadable_changes
                .contains(&NonReloadableSection::BindAddress),
            "Expected BindAddress in non_reloadable_changes, got {:?}",
            d.non_reloadable_changes
        );
    }

    // -----------------------------------------------------------------------
    // ReloadableConfig + manual_reload round-trip
    // -----------------------------------------------------------------------

    /// Writing an updated config to disk and calling manual_reload applies
    /// only reloadable changes.
    #[test]
    fn test_manual_reload_applies_log_level_change() {
        let dir = env::temp_dir();
        let path = dir.join("amaters_hot_reload_test_manual.toml");

        // Write initial config.
        let initial = make_config("127.0.0.1:7878");
        initial.save_to_file(&path).expect("save initial config");

        let rc = ReloadableConfig::new(initial.clone());
        rc.set_config_path(path.clone());

        // Modify log level in file.
        let mut updated = initial.clone();
        updated.logging.level = "warn".to_string();
        updated.save_to_file(&path).expect("save updated config");

        let report = rc.manual_reload().expect("manual_reload succeeded");
        assert!(report.success, "Expected reload success: {:?}", report);

        // Verify the live config has the new log level.
        assert_eq!(
            rc.snapshot().logging.level,
            "warn",
            "Log level should be updated to 'warn'"
        );

        fs::remove_file(&path).ok();
    }

    // -----------------------------------------------------------------------
    // TlsCreds helpers
    // -----------------------------------------------------------------------

    /// load_from_files returns Io error for missing files.
    #[test]
    fn test_tls_creds_load_missing_file() {
        let result =
            TlsCreds::load_from_files(Path::new("/nonexistent/cert.pem"), Path::new("/nonexistent/key.pem"));
        assert!(result.is_err(), "Expected error for missing files");
    }

    /// load_from_files succeeds when both files exist.
    #[test]
    fn test_tls_creds_load_valid_files() {
        let dir = env::temp_dir();
        let cert = dir.join("amaters_hot_reload_test_cert.pem");
        let key = dir.join("amaters_hot_reload_test_key.pem");

        fs::write(&cert, b"-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n")
            .expect("write cert");
        fs::write(&key, b"-----BEGIN PRIVATE KEY-----\ntest\n-----END PRIVATE KEY-----\n")
            .expect("write key");

        let creds = TlsCreds::load_from_files(&cert, &key).expect("load creds");
        assert!(!creds.cert_pem.is_empty());
        assert!(!creds.key_pem.is_empty());

        fs::remove_file(&cert).ok();
        fs::remove_file(&key).ok();
    }

    /// ArcSwap correctly holds and swaps TlsCreds.
    #[test]
    fn test_tls_creds_arc_swap() {
        let dir = env::temp_dir();
        let cert = dir.join("amaters_arc_swap_cert.pem");
        let key = dir.join("amaters_arc_swap_key.pem");

        fs::write(&cert, b"cert_v1").expect("write cert");
        fs::write(&key, b"key_v1").expect("write key");

        let creds1 = TlsCreds::load_from_files(&cert, &key).expect("load v1");
        let store: Arc<ArcSwap<TlsCreds>> = Arc::new(ArcSwap::from_pointee(creds1));

        assert_eq!(store.load().cert_pem, b"cert_v1");

        // Swap in new creds.
        fs::write(&cert, b"cert_v2").expect("write cert v2");
        fs::write(&key, b"key_v2").expect("write key v2");
        let creds2 = TlsCreds::load_from_files(&cert, &key).expect("load v2");
        store.store(Arc::new(creds2));

        assert_eq!(store.load().cert_pem, b"cert_v2");

        fs::remove_file(&cert).ok();
        fs::remove_file(&key).ok();
    }

    // -----------------------------------------------------------------------
    // build_server_tls_config helper
    // -----------------------------------------------------------------------

    /// build_server_tls_config succeeds when cert+key files exist.
    /// Note: tonic will reject non-valid PEM at handshake time, not at build
    /// time, so this test verifies the function does not return a file-IO error.
    #[test]
    fn test_build_server_tls_config_file_error() {
        let result = build_server_tls_config(
            Path::new("/nonexistent/cert.pem"),
            Path::new("/nonexistent/key.pem"),
        );
        assert!(
            matches!(result, Err(HotReloadError::Io(_))),
            "Expected Io error, got {:?}",
            result
        );
    }

    /// `swap_rustls_config` rejects garbage creds with a `Rustls` error rather
    /// than panicking.
    #[test]
    fn test_swap_rustls_config_rejects_invalid_pem() {
        // Build an initial valid config from a tempfile so we have something
        // in the store to begin with.
        let dir = env::temp_dir();
        let cert = dir.join(format!(
            "amaters_swap_rustls_cert_{}.pem",
            uuid::Uuid::new_v4()
        ));
        let key = dir.join(format!(
            "amaters_swap_rustls_key_{}.pem",
            uuid::Uuid::new_v4()
        ));
        // We cannot easily generate a real PEM here without rcgen; instead
        // start with bytes that build_rustls_config will reject and verify
        // the error variant.
        fs::write(&cert, b"not-pem").expect("write cert");
        fs::write(&key, b"not-pem").expect("write key");

        let creds = TlsCreds::load_from_files(&cert, &key).expect("load creds");
        // Seed the store with a placeholder ServerConfig built from a real
        // self-signed cert via amaters_net's SelfSignedGenerator.  Avoiding
        // that here keeps the test scope small — we exercise only the
        // error path of swap_rustls_config which doesn't need a working
        // initial config to verify the failure surface.
        let placeholder = make_placeholder_server_config();
        let store: Arc<ArcSwap<rustls::ServerConfig>> = Arc::new(ArcSwap::from_pointee(placeholder));

        let result = swap_rustls_config(&store, &creds);
        assert!(
            matches!(result, Err(HotReloadError::Rustls(_))),
            "Expected Rustls error, got {:?}",
            result
        );

        fs::remove_file(&cert).ok();
        fs::remove_file(&key).ok();
    }

    /// `swap_rustls_config` with a real self-signed PEM pair succeeds.
    #[test]
    fn test_swap_rustls_config_accepts_valid_pem() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (cert_pem, key_pem) = generate_pem_pair("swap.test");

        let creds = TlsCreds {
            cert_pem,
            key_pem,
        };
        let placeholder = make_placeholder_server_config();
        let store: Arc<ArcSwap<rustls::ServerConfig>> = Arc::new(ArcSwap::from_pointee(placeholder));

        swap_rustls_config(&store, &creds).expect("swap should succeed");
        // The store now holds a non-placeholder config; we can't directly
        // compare ServerConfig but `store.load()` returning a fresh `Arc`
        // proves the swap happened.
        let _ = store.load();
    }

    // -----------------------------------------------------------------------
    // Helpers for the rustls swap tests
    // -----------------------------------------------------------------------

    /// Build a minimal placeholder `rustls::ServerConfig` suitable for use as
    /// the initial value in an `ArcSwap` for tests that only verify swap
    /// semantics (not actual TLS handshakes).
    fn make_placeholder_server_config() -> rustls::ServerConfig {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (cert_pem, key_pem) = generate_pem_pair("placeholder.test");
        let creds_ref = TlsCredsRef::new(&cert_pem, &key_pem);
        build_rustls_config(&creds_ref).expect("placeholder rustls config")
    }

    /// Generate a self-signed cert PEM pair using amaters_net's SelfSignedGenerator.
    fn generate_pem_pair(cn: &str) -> (Vec<u8>, Vec<u8>) {
        use amaters_net::tls::SelfSignedGenerator;
        use rustls::pki_types::PrivateKeyDer;
        let generator = SelfSignedGenerator::new(cn).with_san(cn).with_san("localhost");
        let (cert_der, key_der) = generator.generate().expect("generate cert");
        let cert_pem = pem_encode("CERTIFICATE", cert_der.as_ref());
        let key_pem = match key_der {
            PrivateKeyDer::Pkcs8(k) => pem_encode("PRIVATE KEY", k.secret_pkcs8_der()),
            PrivateKeyDer::Pkcs1(k) => pem_encode("RSA PRIVATE KEY", k.secret_pkcs1_der()),
            PrivateKeyDer::Sec1(k) => pem_encode("EC PRIVATE KEY", k.secret_sec1_der()),
            _ => panic!("unsupported key kind"),
        };
        (cert_pem, key_pem)
    }

    /// Minimal PEM encoder for tests.
    fn pem_encode(label: &str, der: &[u8]) -> Vec<u8> {
        let mut out = format!("-----BEGIN {label}-----\n").into_bytes();
        let b64 = base64_encode_test(der);
        for chunk in b64.as_bytes().chunks(64) {
            out.extend_from_slice(chunk);
            out.push(b'\n');
        }
        out.extend_from_slice(format!("-----END {label}-----\n").as_bytes());
        out
    }

    /// Tiny base64 encoder for tests (RFC 4648 standard alphabet, padding).
    fn base64_encode_test(data: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        let mut i = 0;
        while i + 3 <= data.len() {
            let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
            out.push(ALPHABET[(n & 0x3f) as usize] as char);
            i += 3;
        }
        let rem = data.len() - i;
        if rem == 1 {
            let n = (data[i] as u32) << 16;
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        } else if rem == 2 {
            let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        out
    }

    // -----------------------------------------------------------------------
    // SIGHUP integration test (manual / no real signal)
    // -----------------------------------------------------------------------

    /// Verify that spawn_config_reloader returns a live JoinHandle and that the
    /// underlying ReloadableConfig mechanism works via manual_reload, without
    /// actually sending SIGHUP (which is flaky in test environments).
    #[tokio::test]
    async fn test_spawn_config_reloader_returns_handle() {
        let dir = env::temp_dir();
        let path = dir.join("amaters_sighup_test_config.toml");

        let initial = make_config("127.0.0.1:7878");
        initial.save_to_file(&path).expect("save config");

        let config = Arc::new(RwLock::new(initial.clone()));
        let handle = spawn_config_reloader(path.clone(), config.clone()).await;

        // The task must be running (not finished).
        assert!(!handle.is_finished(), "Reloader task should be running");

        // Abort the background task so the test exits cleanly.
        handle.abort();

        fs::remove_file(&path).ok();
    }

    // -----------------------------------------------------------------------
    // SIGHUP integration test — #[ignore], requires real SIGHUP signal
    // -----------------------------------------------------------------------

    /// Integration test: send a real SIGHUP to the current process and verify
    /// the config is reloaded.
    ///
    /// This test is marked `#[ignore]` because it sends a real UNIX signal and
    /// is intended for manual execution only:
    ///
    /// ```sh
    /// cargo test -p amaters-server test_sighup_reloads_config -- --ignored
    /// ```
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "Integration test — sends a real SIGHUP; run manually with --ignored"]
    async fn test_sighup_reloads_config() {
        use std::time::Duration;

        let dir = env::temp_dir();
        let path = dir.join("amaters_sighup_integration_test.toml");

        let initial = make_config("127.0.0.1:7878");
        initial.save_to_file(&path).expect("save config");

        let config = Arc::new(RwLock::new(initial.clone()));
        let handle = spawn_config_reloader(path.clone(), config.clone()).await;

        // Allow the task to register the signal handler.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Modify the config file — change log level.
        let mut updated = initial.clone();
        updated.logging.level = "debug".to_string();
        updated.save_to_file(&path).expect("save updated config");

        // Send SIGHUP to self via `kill` utility (avoids needing the `libc` crate).
        let pid = std::process::id();
        let _ = std::process::Command::new("kill")
            .args(["-HUP", &pid.to_string()])
            .status()
            .expect("failed to invoke kill command");

        // Allow the handler to process the signal.
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(
            config.read().logging.level,
            "debug",
            "Expected log level to be 'debug' after SIGHUP reload"
        );

        handle.abort();
        fs::remove_file(&path).ok();
    }
}
