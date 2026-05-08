//! Admin command handler for AmateRS network layer.
//!
//! This module provides the admin command infrastructure that backs the
//! `__admin__:<CMD>` key-intercept protocol in the gRPC server.  Admin
//! commands arrive as ordinary GET queries with a specially-prefixed key, which
//! lets the CLI reach server-side admin functionality without a dedicated RPC
//! method.
//!
//! # Commands
//!
//! | Command | Args | Description |
//! |---------|------|-------------|
//! | METRICS | — | Key count and uptime JSON |
//! | CLUSTER_INFO | — | Standalone cluster descriptor |
//! | NODES | — | Self-only node list |
//! | STATS | — | Byte-accurate size scan (capped at 100 000 keys) |
//! | VERIFY | — | Integrity check (always reports 0 corruption for MemoryStorage) |
//! | COMPACT | `[<collection>]` | Flush storage and return status |
//! | LOGS | `<lines=20> <follow=false>` | Return ring-buffered log entries |
//! | BACKUP | `<dir> <full\|incremental>` | Serialize all keys to `<dir>/` |
//! | RESTORE | `<dir>` | Replay keys from a previous backup |

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Maximum number of keys scanned for STATS and VERIFY before setting the
/// `"truncated"` flag to avoid excessive latency.
pub(crate) const STATS_KEY_LIMIT: usize = 100_000;

/// Capacity of the recent-log ring buffer.
pub const LOG_RING_CAPACITY: usize = 256;

// ─── BackupKind ───────────────────────────────────────────────────────────────

/// Whether a backup should capture the full dataset or only incremental changes.
///
/// At the MVP tier there is no real incremental logic; the flag is recorded in
/// the backup manifest so future tooling can act on it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackupKind {
    /// Capture the complete current dataset.
    Full,
    /// Mark as incremental (behaviour identical to `Full` for now).
    Incremental,
}

// ─── BackupMeta ───────────────────────────────────────────────────────────────

/// Metadata written to `<dir>/meta.bin` alongside a backup manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMeta {
    /// Bump this whenever the manifest format changes.
    pub schema_version: u32,
    /// Number of key-value pairs in the manifest.
    pub total_keys: usize,
    /// Total byte count of all values in the manifest.
    pub total_bytes: u64,
    /// Whether this backup is full or incremental.
    pub kind: BackupKind,
}

// ─── LogEntry ─────────────────────────────────────────────────────────────────

/// A single entry in the recent-log ring buffer.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Human-readable log message (method name + elapsed, or error description).
    pub message: String,
    /// Wall-clock timestamp of the entry.
    pub timestamp: SystemTime,
}

// ─── AdminArgs ────────────────────────────────────────────────────────────────

/// Parsed arguments for an admin command.
#[derive(Debug, Clone)]
pub struct AdminArgs {
    /// First positional argument (e.g. directory path for BACKUP/RESTORE,
    /// line count for LOGS).
    pub first: Option<String>,
    /// Second positional argument (e.g. "full"/"incremental" for BACKUP,
    /// "true"/"false" for LOGS follow flag).
    pub second: Option<String>,
}

/// Parse the argument string that follows an admin command name.
///
/// Splits on ASCII whitespace and extracts the first two tokens.  Missing
/// tokens are represented as `None` — callers apply defaults.
pub fn parse_admin_args(args: &str) -> AdminArgs {
    let mut tokens = args.split_ascii_whitespace();
    AdminArgs {
        first: tokens.next().map(str::to_owned),
        second: tokens.next().map(str::to_owned),
    }
}

// ─── Stats helper ─────────────────────────────────────────────────────────────

/// Compute `(key_count, total_bytes, truncated)` by scanning up to `limit` keys.
///
/// Splitting this out makes it injectable for unit tests without spinning up a
/// full gRPC server.
pub(crate) async fn compute_stats<S: StorageEngine>(
    storage: &Arc<S>,
    limit: usize,
) -> (u64, u64, bool) {
    let keys = match storage.keys().await {
        Ok(k) => k,
        Err(e) => {
            error!("STATS: failed to list keys: {}", e);
            return (0, 0, false);
        }
    };

    let total_keys = keys.len();
    let truncated = total_keys > limit;
    let scan_keys = if truncated { &keys[..limit] } else { &keys };

    let mut total_bytes: u64 = 0;
    for key in scan_keys {
        match storage.get(key).await {
            Ok(Some(blob)) => total_bytes += blob.as_bytes().len() as u64,
            Ok(None) => {}
            Err(e) => warn!("STATS: get failed for key {:?}: {}", key, e),
        }
    }

    (scan_keys.len() as u64, total_bytes, truncated)
}

// ─── handle_admin_command ─────────────────────────────────────────────────────

/// Execute an admin command and return a JSON string, or `None` for unknown
/// commands.
///
/// The `cmd` parameter is everything *after* the `__admin__:` prefix.
/// Commands may carry space-separated arguments, e.g. `"LOGS 50 false"`.
///
/// # Arguments
/// * `cmd` - Full command string including arguments.
/// * `uptime_secs` - Server uptime at call time (seconds).
/// * `recent_log` - The server's recent-log ring buffer.
/// * `storage` - Reference to the storage engine.
pub async fn handle_admin_command<S: StorageEngine>(
    cmd: &str,
    uptime_secs: u64,
    recent_log: &Arc<RwLock<VecDeque<LogEntry>>>,
    storage: &Arc<S>,
) -> Option<String> {
    // Split command name from arguments.
    let mut parts = cmd.splitn(2, ' ');
    let op = parts.next().unwrap_or("").trim().to_uppercase();
    let args_str = parts.next().unwrap_or("").trim();
    let args = parse_admin_args(args_str);

    match op.as_str() {
        // ── METRICS ──────────────────────────────────────────────────────────
        "METRICS" => {
            let key_count = storage
                .keys()
                .await
                .map(|k| k.len() as u64)
                .unwrap_or(0);
            let json = serde_json::json!({
                "key_count": key_count,
                "storage_type": "memory",
                "uptime_seconds": uptime_secs,
            });
            serde_json::to_string(&json).ok()
        }

        // ── CLUSTER_INFO ─────────────────────────────────────────────────────
        "CLUSTER_INFO" => {
            let json = serde_json::json!({
                "mode": "standalone",
                "version": env!("CARGO_PKG_VERSION"),
                "nodes": 1u32,
            });
            serde_json::to_string(&json).ok()
        }

        // ── NODES ─────────────────────────────────────────────────────────────
        "NODES" => {
            let json = serde_json::json!({
                "nodes": [{
                    "id": "self",
                    "addr": "0.0.0.0:50051",
                    "role": "leader",
                    "status": "healthy",
                }]
            });
            serde_json::to_string(&json).ok()
        }

        // ── STATS ─────────────────────────────────────────────────────────────
        "STATS" => {
            let (key_count, total_bytes, truncated) =
                compute_stats(storage, STATS_KEY_LIMIT).await;
            let json = serde_json::json!({
                "key_count": key_count,
                "total_bytes": total_bytes,
                "truncated": truncated,
            });
            serde_json::to_string(&json).ok()
        }

        // ── VERIFY ───────────────────────────────────────────────────────────
        "VERIFY" => {
            let (checked, _, _) = compute_stats(storage, STATS_KEY_LIMIT).await;
            let json = serde_json::json!({
                "corrupted_keys": 0u64,
                "checked": checked,
                "ok": true,
            });
            serde_json::to_string(&json).ok()
        }

        // ── COMPACT ───────────────────────────────────────────────────────────
        "COMPACT" => {
            let flushed = storage.flush().await.is_ok();
            let collection: serde_json::Value = args
                .first
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null);
            let json = serde_json::json!({
                "status": "ok",
                "collection": collection,
                "flushed": flushed,
            });
            serde_json::to_string(&json).ok()
        }

        // ── LOGS ──────────────────────────────────────────────────────────────
        "LOGS" => {
            let lines: usize = args
                .first
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(20);
            // follow flag is acknowledged but not implemented (MVP).
            let _follow: bool = args
                .second
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case("true"))
                .unwrap_or(false);

            let entries: Vec<serde_json::Value> = {
                let guard = recent_log.read();
                guard
                    .iter()
                    .rev()
                    .take(lines)
                    .map(|e| {
                        let ts = e
                            .timestamp
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        serde_json::json!({
                            "message": e.message,
                            "timestamp": ts,
                        })
                    })
                    .collect()
            };

            let json = serde_json::json!({
                "lines": entries,
                "follow_supported": false,
            });
            serde_json::to_string(&json).ok()
        }

        // ── BACKUP ────────────────────────────────────────────────────────────
        "BACKUP" => {
            let dir = match args.first.as_deref() {
                Some(d) if !d.is_empty() => d.to_owned(),
                _ => {
                    let json = serde_json::json!({"error": "missing backup directory"});
                    return serde_json::to_string(&json).ok();
                }
            };
            let kind = match args.second.as_deref().map(str::to_lowercase).as_deref() {
                Some("incremental") => BackupKind::Incremental,
                _ => BackupKind::Full,
            };

            // Create the backup directory.
            if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                let json = serde_json::json!({"error": format!("create_dir_all failed: {e}")});
                return serde_json::to_string(&json).ok();
            }

            // Collect all key-value pairs.
            let keys = match storage.keys().await {
                Ok(k) => k,
                Err(e) => {
                    let json = serde_json::json!({"error": format!("keys() failed: {e}")});
                    return serde_json::to_string(&json).ok();
                }
            };

            let mut manifest: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(keys.len());
            let mut total_bytes: u64 = 0;
            for key in &keys {
                match storage.get(key).await {
                    Ok(Some(blob)) => {
                        total_bytes += blob.as_bytes().len() as u64;
                        manifest.push((key.as_bytes().to_vec(), blob.as_bytes().to_vec()));
                    }
                    Ok(None) => {}
                    Err(e) => warn!("BACKUP: get failed for key {:?}: {}", key, e),
                }
            }

            let total_keys = manifest.len();

            // Serialize manifest.
            let manifest_bytes = match oxicode::serde::encode_serde(&manifest) {
                Ok(b) => b,
                Err(e) => {
                    let json = serde_json::json!({"error": format!("manifest encode failed: {e}")});
                    return serde_json::to_string(&json).ok();
                }
            };

            // Serialize metadata.
            let meta = BackupMeta {
                schema_version: 1,
                total_keys,
                total_bytes,
                kind: kind.clone(),
            };
            let meta_bytes = match oxicode::serde::encode_serde(&meta) {
                Ok(b) => b,
                Err(e) => {
                    let json = serde_json::json!({"error": format!("meta encode failed: {e}")});
                    return serde_json::to_string(&json).ok();
                }
            };

            // Write files.
            let manifest_path = format!("{dir}/manifest.bin");
            let meta_path = format!("{dir}/meta.bin");

            if let Err(e) = tokio::fs::write(&manifest_path, &manifest_bytes).await {
                let json = serde_json::json!({"error": format!("write manifest failed: {e}")});
                return serde_json::to_string(&json).ok();
            }
            if let Err(e) = tokio::fs::write(&meta_path, &meta_bytes).await {
                let json = serde_json::json!({"error": format!("write meta failed: {e}")});
                return serde_json::to_string(&json).ok();
            }

            info!(
                "BACKUP completed: dir={}, keys={}, bytes={}, kind={:?}",
                dir, total_keys, total_bytes, kind
            );

            let kind_str = match kind {
                BackupKind::Full => "full",
                BackupKind::Incremental => "incremental",
            };
            let json = serde_json::json!({
                "status": "ok",
                "path": dir,
                "key_count": total_keys,
                "byte_count": total_bytes,
                "kind": kind_str,
            });
            serde_json::to_string(&json).ok()
        }

        // ── RESTORE ───────────────────────────────────────────────────────────
        "RESTORE" => {
            let dir = match args.first.as_deref() {
                Some(d) if !d.is_empty() => d.to_owned(),
                _ => {
                    let json = serde_json::json!({"error": "missing restore directory"});
                    return serde_json::to_string(&json).ok();
                }
            };

            let meta_path = format!("{dir}/meta.bin");
            let manifest_path = format!("{dir}/manifest.bin");

            let meta_bytes = match tokio::fs::read(&meta_path).await {
                Ok(b) => b,
                Err(e) => {
                    let json = serde_json::json!({"error": format!("read meta.bin failed: {e}")});
                    return serde_json::to_string(&json).ok();
                }
            };
            let manifest_bytes = match tokio::fs::read(&manifest_path).await {
                Ok(b) => b,
                Err(e) => {
                    let json =
                        serde_json::json!({"error": format!("read manifest.bin failed: {e}")});
                    return serde_json::to_string(&json).ok();
                }
            };

            let meta: BackupMeta = match oxicode::serde::decode_serde(&meta_bytes) {
                Ok(m) => m,
                Err(e) => {
                    let json = serde_json::json!({"error": format!("decode meta failed: {e}")});
                    return serde_json::to_string(&json).ok();
                }
            };

            if meta.schema_version != 1 {
                let json = serde_json::json!({
                    "error": format!(
                        "unsupported schema_version {} (expected 1)",
                        meta.schema_version
                    )
                });
                return serde_json::to_string(&json).ok();
            }

            let manifest: Vec<(Vec<u8>, Vec<u8>)> =
                match oxicode::serde::decode_serde(&manifest_bytes) {
                    Ok(m) => m,
                    Err(e) => {
                        let json =
                            serde_json::json!({"error": format!("decode manifest failed: {e}")});
                        return serde_json::to_string(&json).ok();
                    }
                };

            let mut restored: usize = 0;
            for (key_bytes, value_bytes) in manifest {
                let key = Key::from_slice(&key_bytes);
                let blob = CipherBlob::new(value_bytes);
                match storage.put(&key, &blob).await {
                    Ok(()) => restored += 1,
                    Err(e) => warn!("RESTORE: put failed for key {:?}: {}", key, e),
                }
            }

            info!("RESTORE completed: dir={}, restored={}", dir, restored);

            let json = serde_json::json!({
                "status": "ok",
                "restored": restored,
                "schema_version": 1,
            });
            serde_json::to_string(&json).ok()
        }

        // ── Unknown ───────────────────────────────────────────────────────────
        _ => None,
    }
}

/// Push a log entry to the ring buffer, enforcing the 256-entry capacity bound.
///
/// Uses `try_write()` with a silent drop on contention to avoid deadlocks
/// during error-handling paths that may already hold the lock.
pub fn push_log_entry(
    recent_log: &Arc<RwLock<VecDeque<LogEntry>>>,
    message: String,
) {
    let entry = LogEntry {
        message,
        timestamp: SystemTime::now(),
    };
    if let Some(mut guard) = recent_log.try_write() {
        if guard.len() >= LOG_RING_CAPACITY {
            guard.pop_front();
        }
        guard.push_back(entry);
    }
    // If try_write() fails (lock held by reader/writer), we silently drop the
    // entry rather than block or deadlock.
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod admin_tests {
    use super::*;
    use amaters_core::storage::MemoryStorage;
    use std::sync::Arc;

    // Helper: fresh storage + recent_log
    fn make_store() -> Arc<MemoryStorage> {
        Arc::new(MemoryStorage::new())
    }

    fn make_log() -> Arc<RwLock<VecDeque<LogEntry>>> {
        Arc::new(RwLock::new(VecDeque::new()))
    }

    async fn run_cmd<S: StorageEngine>(
        cmd: &str,
        storage: &Arc<S>,
        log: &Arc<RwLock<VecDeque<LogEntry>>>,
    ) -> Option<String> {
        handle_admin_command(cmd, 0, log, storage).await
    }

    // ── test_admin_metrics_returns_real_data ──────────────────────────────────

    #[tokio::test]
    async fn test_admin_metrics_returns_real_data() {
        let storage = make_store();
        let log = make_log();

        // Insert two keys.
        for i in 0u8..2 {
            let k = Key::from_str(&format!("k{}", i));
            let v = CipherBlob::new(vec![i; 4]);
            storage.put(&k, &v).await.expect("put failed");
        }

        let json_str = run_cmd("METRICS", &storage, &log)
            .await
            .expect("METRICS returned None");
        let v: serde_json::Value = serde_json::from_str(&json_str).expect("invalid JSON");

        assert_eq!(v["key_count"], 2, "key_count should be 2");
        assert!(v["storage_type"].is_string());
        assert!(v["uptime_seconds"].is_number());
    }

    // ── test_admin_stats_returns_byte_accurate_size_under_threshold ──────────

    #[tokio::test]
    async fn test_admin_stats_returns_byte_accurate_size_under_threshold() {
        let storage = make_store();
        let log = make_log();

        // Insert 3 keys with known byte sizes.
        for i in 0u8..3 {
            let k = Key::from_str(&format!("key_{}", i));
            let v = CipherBlob::new(vec![i; 10]); // 10 bytes each → 30 total
            storage.put(&k, &v).await.expect("put failed");
        }

        let json_str = run_cmd("STATS", &storage, &log)
            .await
            .expect("STATS returned None");
        let v: serde_json::Value = serde_json::from_str(&json_str).expect("invalid JSON");

        assert_eq!(v["key_count"], 3u64, "key_count should be 3");
        assert_eq!(v["total_bytes"], 30u64, "total_bytes should be 30");
        assert_eq!(v["truncated"], false, "truncated should be false");
    }

    // ── test_admin_stats_returns_truncated_flag_over_threshold ───────────────

    #[tokio::test]
    async fn test_admin_stats_returns_truncated_flag_over_threshold() {
        // Use limit=2 with 3 keys — exercises the cap logic without inserting
        // 100 000 keys.
        let storage = make_store();
        for i in 0u8..3 {
            let k = Key::from_str(&format!("t_{}", i));
            let v = CipherBlob::new(vec![1u8; 5]);
            storage.put(&k, &v).await.expect("put failed");
        }

        let (key_count, total_bytes, truncated) = compute_stats(&storage, 2).await;
        assert_eq!(key_count, 2, "should scan only 2 keys");
        assert_eq!(total_bytes, 10, "2 keys × 5 bytes = 10");
        assert!(truncated, "truncated should be true when limit exceeded");
    }

    // ── test_admin_backup_creates_manifest ────────────────────────────────────

    #[tokio::test]
    async fn test_admin_backup_creates_manifest() {
        let storage = make_store();
        let log = make_log();

        let k = Key::from_str("bk_key");
        let v = CipherBlob::new(b"hello".to_vec());
        storage.put(&k, &v).await.expect("put failed");

        let dir = std::env::temp_dir()
            .join(format!("amaters_test_backup_{}", std::time::SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        let dir_str = dir.to_string_lossy().to_string();

        let json_str = run_cmd(&format!("BACKUP {dir_str} full"), &storage, &log)
            .await
            .expect("BACKUP returned None");
        let v: serde_json::Value = serde_json::from_str(&json_str).expect("invalid JSON");
        assert_eq!(v["status"], "ok", "status should be ok");
        assert_eq!(v["key_count"], 1u64);

        // Verify files exist.
        assert!(
            std::path::Path::new(&format!("{dir_str}/manifest.bin")).exists(),
            "manifest.bin should exist"
        );
        assert!(
            std::path::Path::new(&format!("{dir_str}/meta.bin")).exists(),
            "meta.bin should exist"
        );

        // Cleanup.
        let _ = tokio::fs::remove_dir_all(&dir_str).await;
    }

    // ── test_admin_backup_incremental_flag_recorded ───────────────────────────

    #[tokio::test]
    async fn test_admin_backup_incremental_flag_recorded() {
        let storage = make_store();
        let log = make_log();

        let k = Key::from_str("inc_key");
        let v = CipherBlob::new(vec![42u8; 3]);
        storage.put(&k, &v).await.expect("put failed");

        let dir = std::env::temp_dir()
            .join(format!("amaters_test_inc_{}", std::time::SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        let dir_str = dir.to_string_lossy().to_string();

        let json_str = run_cmd(
            &format!("BACKUP {dir_str} incremental"),
            &storage,
            &log,
        )
        .await
        .expect("BACKUP incremental returned None");

        let resp: serde_json::Value = serde_json::from_str(&json_str).expect("invalid JSON");
        assert_eq!(resp["kind"], "incremental");

        // Read meta.bin and verify BackupKind.
        let meta_bytes = tokio::fs::read(format!("{dir_str}/meta.bin"))
            .await
            .expect("meta.bin not found");
        let meta: BackupMeta =
            oxicode::serde::decode_serde(&meta_bytes).expect("decode meta failed");
        assert_eq!(meta.kind, BackupKind::Incremental);

        let _ = tokio::fs::remove_dir_all(&dir_str).await;
    }

    // ── test_admin_restore_replays_keys ───────────────────────────────────────

    #[tokio::test]
    async fn test_admin_restore_replays_keys() {
        let source = make_store();
        let log = make_log();

        // Insert two keys into source.
        let k1 = Key::from_str("restore_a");
        let k2 = Key::from_str("restore_b");
        source
            .put(&k1, &CipherBlob::new(b"alpha".to_vec()))
            .await
            .expect("put failed");
        source
            .put(&k2, &CipherBlob::new(b"beta".to_vec()))
            .await
            .expect("put failed");

        let dir = std::env::temp_dir()
            .join(format!("amaters_test_restore_{}", std::time::SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        let dir_str = dir.to_string_lossy().to_string();

        // Backup from source.
        run_cmd(&format!("BACKUP {dir_str} full"), &source, &log)
            .await
            .expect("BACKUP returned None");

        // Restore into a fresh store.
        let target = make_store();
        let json_str = run_cmd(&format!("RESTORE {dir_str}"), &target, &log)
            .await
            .expect("RESTORE returned None");
        let resp: serde_json::Value = serde_json::from_str(&json_str).expect("invalid JSON");
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["restored"], 2u64);

        // Verify both keys exist in target.
        let got_a = target.get(&k1).await.expect("get failed");
        assert_eq!(
            got_a.as_ref().map(|b| b.as_bytes()),
            Some(b"alpha".as_ref())
        );
        let got_b = target.get(&k2).await.expect("get failed");
        assert_eq!(
            got_b.as_ref().map(|b| b.as_bytes()),
            Some(b"beta".as_ref())
        );

        let _ = tokio::fs::remove_dir_all(&dir_str).await;
    }

    // ── test_admin_logs_default_lines ─────────────────────────────────────────

    #[tokio::test]
    async fn test_admin_logs_default_lines() {
        let storage = make_store();
        let log = make_log();

        // Push 5 entries.
        for i in 0..5u32 {
            push_log_entry(&log, format!("entry {}", i));
        }

        let json_str = run_cmd("LOGS", &storage, &log)
            .await
            .expect("LOGS returned None");
        let resp: serde_json::Value = serde_json::from_str(&json_str).expect("invalid JSON");
        assert!(resp["lines"].is_array());
        // Default is 20; we only have 5 entries.
        assert_eq!(
            resp["lines"].as_array().map(|a| a.len()).unwrap_or(0),
            5,
            "should return all 5 available entries"
        );
        assert_eq!(resp["follow_supported"], false);
    }

    // ── test_admin_args_parser_handles_missing ────────────────────────────────

    #[test]
    fn test_admin_args_parser_handles_missing() {
        let a = parse_admin_args("");
        assert!(a.first.is_none(), "first should be None for empty input");
        assert!(a.second.is_none(), "second should be None for empty input");

        let b = parse_admin_args("only_one");
        assert_eq!(b.first.as_deref(), Some("only_one"));
        assert!(b.second.is_none());

        let c = parse_admin_args("a b extra_ignored");
        assert_eq!(c.first.as_deref(), Some("a"));
        assert_eq!(c.second.as_deref(), Some("b"));
    }

    // ── test_recent_log_ring_buffer_bounded_at_256 ────────────────────────────

    #[test]
    fn test_recent_log_ring_buffer_bounded_at_256() {
        let log = make_log();

        for i in 0..256u32 {
            push_log_entry(&log, format!("msg {}", i));
        }

        let guard = log.read();
        assert_eq!(
            guard.len(),
            256,
            "ring buffer should hold exactly 256 entries"
        );
    }

    // ── test_recent_log_drop_oldest_on_overflow ───────────────────────────────

    #[test]
    fn test_recent_log_drop_oldest_on_overflow() {
        let log = make_log();

        // Fill to capacity, then push one more.
        for i in 0..=256u32 {
            push_log_entry(&log, format!("msg {}", i));
        }

        let guard = log.read();
        assert_eq!(guard.len(), 256, "capacity should not exceed 256");

        // The oldest entry ("msg 0") should have been dropped.
        let first = guard.front().expect("ring buffer must not be empty");
        assert_ne!(first.message, "msg 0", "oldest entry should be dropped");
        assert_eq!(
            first.message, "msg 1",
            "second entry should now be the oldest"
        );
    }
}
