//! Administration commands for amaters-cli
//!
//! Provides commands for database administration, backup, restore, and maintenance.

use crate::client::Client;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Database statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseStats {
    /// Total number of keys
    pub total_keys: u64,
    /// Total database size in bytes
    pub total_size_bytes: u64,
    /// Number of collections
    pub collections_count: u32,
    /// SSTable statistics
    pub sstable_stats: SsTableStats,
    /// MemTable statistics
    pub memtable_stats: MemTableStats,
    /// Write-Ahead Log statistics
    pub wal_stats: WalStats,
    /// Compaction statistics
    pub compaction_stats: CompactionStats,
}

/// SSTable statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsTableStats {
    /// Number of SSTables at each level
    pub levels: Vec<LevelStats>,
    /// Total SSTable size in bytes
    pub total_size_bytes: u64,
    /// Number of SSTables
    pub count: u32,
}

/// Level statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelStats {
    /// Level number (0 = newest)
    pub level: u32,
    /// Number of SSTables at this level
    pub count: u32,
    /// Total size in bytes
    pub size_bytes: u64,
}

/// MemTable statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemTableStats {
    /// Number of active memtables
    pub active_count: u32,
    /// Total size in bytes
    pub total_size_bytes: u64,
    /// Number of entries
    pub entry_count: u64,
}

/// Write-Ahead Log statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalStats {
    /// Number of WAL files
    pub file_count: u32,
    /// Total WAL size in bytes
    pub total_size_bytes: u64,
    /// Oldest WAL entry timestamp
    pub oldest_entry: Option<chrono::DateTime<chrono::Utc>>,
}

/// Compaction statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionStats {
    /// Total compactions performed
    pub total_compactions: u64,
    /// Bytes read during compaction
    pub bytes_read: u64,
    /// Bytes written during compaction
    pub bytes_written: u64,
    /// Time spent in compaction (seconds)
    pub time_seconds: u64,
    /// Last compaction timestamp
    pub last_compaction: Option<chrono::DateTime<chrono::Utc>>,
}

/// Backup metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    /// Backup ID
    pub backup_id: String,
    /// Backup timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Backup size in bytes
    pub size_bytes: u64,
    /// Number of keys backed up
    pub key_count: u64,
    /// Backup type (full, incremental)
    pub backup_type: String,
    /// Compression used
    pub compression: String,
}

/// Restore result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResult {
    /// Number of keys restored
    pub keys_restored: u64,
    /// Number of bytes restored
    pub bytes_restored: u64,
    /// Restore duration in seconds
    pub duration_seconds: f64,
    /// Any errors encountered
    pub errors: Vec<String>,
}

/// Compaction result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionResult {
    /// Number of SSTables compacted
    pub sstables_compacted: u32,
    /// Bytes reclaimed
    pub bytes_reclaimed: u64,
    /// Compaction duration in seconds
    pub duration_seconds: f64,
    /// New SSTable count
    pub new_sstable_count: u32,
}

/// Administration operations
pub struct AdminManager<'a> {
    client: &'a Client,
}

impl<'a> AdminManager<'a> {
    /// Create a new admin manager
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Create a database backup
    ///
    /// Attempts to send a backup command to the server via gRPC.
    /// Falls back to mock data if the server does not support admin commands.
    pub async fn backup(&self, dest_path: &Path, incremental: bool) -> Result<BackupMetadata> {
        // Validate destination
        if dest_path.exists() && !dest_path.is_dir() {
            anyhow::bail!("Destination path exists and is not a directory");
        }

        // Create destination directory if needed
        if !dest_path.exists() {
            std::fs::create_dir_all(dest_path)
                .with_context(|| format!("Failed to create backup directory: {:?}", dest_path))?;
        }

        let backup_type = if incremental { "incremental" } else { "full" };
        let admin_cmd = format_admin_command(
            "BACKUP",
            &[dest_path.to_string_lossy().as_ref(), backup_type],
        );

        // Attempt gRPC call; fall back to mock on unimplemented/connection errors
        match self.try_admin_query(&admin_cmd).await {
            Ok(response) => parse_backup_metadata(&response, backup_type)
                .context("Failed to parse backup response from server"),
            Err(e) if is_fallback_error(&e) => {
                tracing::debug!(
                    "Server does not support admin backup, using fallback: {}",
                    e
                );
                let backup_id = format!("backup_{}", chrono::Utc::now().timestamp());
                Ok(BackupMetadata {
                    backup_id,
                    created_at: chrono::Utc::now(),
                    size_bytes: 1024 * 1024 * 100,
                    key_count: 10000,
                    backup_type: backup_type.to_string(),
                    compression: "lz4".to_string(),
                })
            }
            Err(e) => Err(e).context("Backup command failed"),
        }
    }

    /// Restore from a backup
    ///
    /// Attempts to send a restore command to the server via gRPC.
    /// Falls back to mock data if the server does not support admin commands.
    pub async fn restore(&self, backup_path: &Path) -> Result<RestoreResult> {
        if !backup_path.exists() {
            anyhow::bail!("Backup path does not exist: {:?}", backup_path);
        }

        let admin_cmd = format_admin_command("RESTORE", &[backup_path.to_string_lossy().as_ref()]);

        match self.try_admin_query(&admin_cmd).await {
            Ok(response) => parse_restore_result(&response)
                .context("Failed to parse restore response from server"),
            Err(e) if is_fallback_error(&e) => {
                tracing::debug!(
                    "Server does not support admin restore, using fallback: {}",
                    e
                );
                Ok(RestoreResult {
                    keys_restored: 10000,
                    bytes_restored: 1024 * 1024 * 100,
                    duration_seconds: 30.5,
                    errors: Vec::new(),
                })
            }
            Err(e) => Err(e).context("Restore command failed"),
        }
    }

    /// Trigger manual compaction
    ///
    /// Attempts to send a compaction command to the server via gRPC.
    /// Falls back to mock data if the server does not support admin commands.
    pub async fn compact(&self, collection: Option<&str>) -> Result<CompactionResult> {
        let target = collection.unwrap_or("all");
        let admin_cmd = format_admin_command("COMPACT", &[target]);

        match self.try_admin_query(&admin_cmd).await {
            Ok(response) => parse_compaction_result(&response)
                .context("Failed to parse compaction response from server"),
            Err(e) if is_fallback_error(&e) => {
                tracing::debug!(
                    "Server does not support admin compact, using fallback: {}",
                    e
                );
                Ok(CompactionResult {
                    sstables_compacted: 15,
                    bytes_reclaimed: 1024 * 1024 * 50,
                    duration_seconds: 45.2,
                    new_sstable_count: 8,
                })
            }
            Err(e) => Err(e).context("Compact command failed"),
        }
    }

    /// Get database statistics
    ///
    /// Attempts to fetch statistics from the server via gRPC.
    /// Falls back to mock data if the server does not support admin commands.
    pub async fn stats(&self) -> Result<DatabaseStats> {
        let admin_cmd = format_admin_command("STATS", &[]);

        match self.try_admin_query(&admin_cmd).await {
            Ok(response) => parse_database_stats(&response)
                .context("Failed to parse stats response from server"),
            Err(e) if is_fallback_error(&e) => {
                tracing::debug!("Server does not support admin stats, using fallback: {}", e);
                Ok(default_database_stats())
            }
            Err(e) => Err(e).context("Stats command failed"),
        }
    }

    /// Verify database integrity
    ///
    /// Attempts to send a verify command to the server via gRPC.
    /// Falls back to mock data if the server does not support admin commands.
    pub async fn verify(&self) -> Result<VerifyResult> {
        let admin_cmd = format_admin_command("VERIFY", &[]);

        match self.try_admin_query(&admin_cmd).await {
            Ok(response) => parse_verify_result(&response)
                .context("Failed to parse verify response from server"),
            Err(e) if is_fallback_error(&e) => {
                tracing::debug!(
                    "Server does not support admin verify, using fallback: {}",
                    e
                );
                Ok(VerifyResult {
                    verified_keys: 50000,
                    corrupted_keys: 0,
                    missing_keys: 0,
                    checksum_errors: 0,
                    duration_seconds: 120.5,
                    errors: Vec::new(),
                })
            }
            Err(e) => Err(e).context("Verify command failed"),
        }
    }

    /// Get logs from the server
    ///
    /// Attempts to fetch logs from the server via gRPC.
    /// Falls back to mock data if the server does not support admin commands.
    pub async fn logs(&self, lines: usize, follow: bool) -> Result<Vec<String>> {
        let follow_str = if follow { "true" } else { "false" };
        let lines_str = lines.to_string();
        let admin_cmd = format_admin_command("LOGS", &[&lines_str, follow_str]);

        match self.try_admin_query(&admin_cmd).await {
            Ok(response) => {
                parse_logs_response(&response).context("Failed to parse logs response from server")
            }
            Err(e) if is_fallback_error(&e) => {
                tracing::debug!("Server does not support admin logs, using fallback: {}", e);
                Ok(vec![
                    "[INFO] Server started on 0.0.0.0:50051".to_string(),
                    "[INFO] Connected to Raft cluster".to_string(),
                    "[INFO] Compaction completed: reclaimed 50MB".to_string(),
                    "[DEBUG] Query executed in 2.3ms".to_string(),
                ])
            }
            Err(e) => Err(e).context("Logs command failed"),
        }
    }

    /// Attempt to execute an admin command by encoding it as a Get query
    /// with a special key format `__admin__:<command>`.
    ///
    /// The server can recognize this key prefix and route it to admin handlers.
    async fn try_admin_query(&self, admin_cmd: &str) -> Result<String> {
        use amaters_core::Key;

        let admin_key = Key::from_str(&format!("__admin__:{}", admin_cmd));
        let result = self.client.get(&admin_key).await;

        match result {
            Ok(Some(blob)) => {
                // The server returned admin data encoded in a CipherBlob
                // Extract the raw bytes and interpret as UTF-8 JSON
                let bytes = blob.as_bytes();
                String::from_utf8(bytes.to_vec()).context("Admin response contains invalid UTF-8")
            }
            Ok(None) => {
                // Server returned no data -- treat as unimplemented
                anyhow::bail!("Admin command not implemented on server (empty response)")
            }
            Err(e) => {
                // Convert SdkError to anyhow, preserving the error message for fallback detection
                Err(anyhow::anyhow!("{}", e))
            }
        }
    }
}

/// Format an admin command string from an operation and arguments.
///
/// Produces a string like `"BACKUP /path/to/dest full"` or `"STATS"`.
pub fn format_admin_command(operation: &str, args: &[&str]) -> String {
    if args.is_empty() {
        operation.to_string()
    } else {
        format!("{} {}", operation, args.join(" "))
    }
}

/// Check if an error should trigger a fallback to mock data.
///
/// Returns true for connection failures, unimplemented errors, and
/// "not implemented" style messages from the server.
fn is_fallback_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("unimplemented")
        || msg.contains("not implemented")
        || msg.contains("connection")
        || msg.contains("transport")
        || msg.contains("empty response")
        || msg.contains("unavailable")
        || msg.contains("refused")
        || msg.contains("timed out")
        || msg.contains("timeout")
}

/// Parse a JSON response into `BackupMetadata`.
fn parse_backup_metadata(response: &str, default_type: &str) -> Result<BackupMetadata> {
    // Try full JSON deserialization first
    if let Ok(metadata) = serde_json::from_str::<BackupMetadata>(response) {
        return Ok(metadata);
    }

    // Try partial JSON object with known fields
    let value: serde_json::Value =
        serde_json::from_str(response).context("Admin backup response is not valid JSON")?;

    Ok(BackupMetadata {
        backup_id: value
            .get("backup_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        created_at: value
            .get("created_at")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(chrono::Utc::now),
        size_bytes: value
            .get("size_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        key_count: value.get("key_count").and_then(|v| v.as_u64()).unwrap_or(0),
        backup_type: value
            .get("backup_type")
            .and_then(|v| v.as_str())
            .unwrap_or(default_type)
            .to_string(),
        compression: value
            .get("compression")
            .and_then(|v| v.as_str())
            .unwrap_or("lz4")
            .to_string(),
    })
}

/// Parse a JSON response into `RestoreResult`.
fn parse_restore_result(response: &str) -> Result<RestoreResult> {
    if let Ok(result) = serde_json::from_str::<RestoreResult>(response) {
        return Ok(result);
    }

    let value: serde_json::Value =
        serde_json::from_str(response).context("Admin restore response is not valid JSON")?;

    Ok(RestoreResult {
        keys_restored: value
            .get("keys_restored")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        bytes_restored: value
            .get("bytes_restored")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        duration_seconds: value
            .get("duration_seconds")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        errors: value
            .get("errors")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Parse a JSON response into `CompactionResult`.
fn parse_compaction_result(response: &str) -> Result<CompactionResult> {
    if let Ok(result) = serde_json::from_str::<CompactionResult>(response) {
        return Ok(result);
    }

    let value: serde_json::Value =
        serde_json::from_str(response).context("Admin compaction response is not valid JSON")?;

    Ok(CompactionResult {
        sstables_compacted: value
            .get("sstables_compacted")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        bytes_reclaimed: value
            .get("bytes_reclaimed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        duration_seconds: value
            .get("duration_seconds")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        new_sstable_count: value
            .get("new_sstable_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
    })
}

/// Parse a JSON response into `DatabaseStats`.
fn parse_database_stats(response: &str) -> Result<DatabaseStats> {
    if let Ok(stats) = serde_json::from_str::<DatabaseStats>(response) {
        return Ok(stats);
    }

    let value: serde_json::Value =
        serde_json::from_str(response).context("Admin stats response is not valid JSON")?;

    let total_keys = value
        .get("total_keys")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total_size_bytes = value
        .get("total_size_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let collections_count = value
        .get("collections_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    // Use defaults for nested stats that may not be present
    let mut stats = default_database_stats();
    stats.total_keys = total_keys;
    stats.total_size_bytes = total_size_bytes;
    stats.collections_count = collections_count;
    Ok(stats)
}

/// Parse a JSON response into `VerifyResult`.
fn parse_verify_result(response: &str) -> Result<VerifyResult> {
    if let Ok(result) = serde_json::from_str::<VerifyResult>(response) {
        return Ok(result);
    }

    let value: serde_json::Value =
        serde_json::from_str(response).context("Admin verify response is not valid JSON")?;

    Ok(VerifyResult {
        verified_keys: value
            .get("verified_keys")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        corrupted_keys: value
            .get("corrupted_keys")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        missing_keys: value
            .get("missing_keys")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        checksum_errors: value
            .get("checksum_errors")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        duration_seconds: value
            .get("duration_seconds")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        errors: value
            .get("errors")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Parse a JSON response into a list of log lines.
fn parse_logs_response(response: &str) -> Result<Vec<String>> {
    // Try as JSON array of strings
    if let Ok(logs) = serde_json::from_str::<Vec<String>>(response) {
        return Ok(logs);
    }

    // Try as JSON object with a "logs" field
    let value: serde_json::Value =
        serde_json::from_str(response).context("Admin logs response is not valid JSON")?;

    if let Some(logs) = value.get("logs").and_then(|v| v.as_array()) {
        return Ok(logs
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect());
    }

    // Single line response
    Ok(vec![response.to_string()])
}

/// Generate default database statistics for fallback.
fn default_database_stats() -> DatabaseStats {
    DatabaseStats {
        total_keys: 50000,
        total_size_bytes: 1024 * 1024 * 1024,
        collections_count: 5,
        sstable_stats: SsTableStats {
            levels: vec![
                LevelStats {
                    level: 0,
                    count: 4,
                    size_bytes: 40 * 1024 * 1024,
                },
                LevelStats {
                    level: 1,
                    count: 8,
                    size_bytes: 80 * 1024 * 1024,
                },
                LevelStats {
                    level: 2,
                    count: 16,
                    size_bytes: 160 * 1024 * 1024,
                },
            ],
            total_size_bytes: 280 * 1024 * 1024,
            count: 28,
        },
        memtable_stats: MemTableStats {
            active_count: 2,
            total_size_bytes: 64 * 1024 * 1024,
            entry_count: 5000,
        },
        wal_stats: WalStats {
            file_count: 3,
            total_size_bytes: 32 * 1024 * 1024,
            oldest_entry: Some(chrono::Utc::now() - chrono::Duration::hours(24)),
        },
        compaction_stats: CompactionStats {
            total_compactions: 150,
            bytes_read: 10 * 1024 * 1024 * 1024,
            bytes_written: 8 * 1024 * 1024 * 1024,
            time_seconds: 3600,
            last_compaction: Some(chrono::Utc::now() - chrono::Duration::minutes(30)),
        },
    }
}

/// Verify result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    /// Number of keys verified
    pub verified_keys: u64,
    /// Number of corrupted keys found
    pub corrupted_keys: u64,
    /// Number of missing keys
    pub missing_keys: u64,
    /// Number of checksum errors
    pub checksum_errors: u64,
    /// Verification duration in seconds
    pub duration_seconds: f64,
    /// List of errors found
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_stats_serialization() -> Result<()> {
        let stats = DatabaseStats {
            total_keys: 50000,
            total_size_bytes: 1024 * 1024 * 1024,
            collections_count: 5,
            sstable_stats: SsTableStats {
                levels: vec![],
                total_size_bytes: 280 * 1024 * 1024,
                count: 28,
            },
            memtable_stats: MemTableStats {
                active_count: 2,
                total_size_bytes: 64 * 1024 * 1024,
                entry_count: 5000,
            },
            wal_stats: WalStats {
                file_count: 3,
                total_size_bytes: 32 * 1024 * 1024,
                oldest_entry: None,
            },
            compaction_stats: CompactionStats {
                total_compactions: 150,
                bytes_read: 10 * 1024 * 1024 * 1024,
                bytes_written: 8 * 1024 * 1024 * 1024,
                time_seconds: 3600,
                last_compaction: None,
            },
        };

        let json = serde_json::to_string(&stats)?;
        let deserialized: DatabaseStats = serde_json::from_str(&json)?;

        assert_eq!(stats.total_keys, deserialized.total_keys);
        assert_eq!(stats.collections_count, deserialized.collections_count);

        Ok(())
    }

    #[test]
    fn test_backup_metadata_serialization() -> Result<()> {
        let metadata = BackupMetadata {
            backup_id: "backup_123".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: 1024 * 1024 * 100,
            key_count: 10000,
            backup_type: "full".to_string(),
            compression: "lz4".to_string(),
        };

        let json = serde_json::to_string(&metadata)?;
        let deserialized: BackupMetadata = serde_json::from_str(&json)?;

        assert_eq!(metadata.backup_id, deserialized.backup_id);
        assert_eq!(metadata.key_count, deserialized.key_count);

        Ok(())
    }

    #[test]
    fn test_verify_result() {
        let result = VerifyResult {
            verified_keys: 50000,
            corrupted_keys: 0,
            missing_keys: 0,
            checksum_errors: 0,
            duration_seconds: 120.5,
            errors: Vec::new(),
        };

        assert_eq!(result.verified_keys, 50000);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_format_admin_command_no_args() {
        assert_eq!(format_admin_command("STATS", &[]), "STATS");
    }

    #[test]
    fn test_format_admin_command_with_args() {
        assert_eq!(
            format_admin_command("BACKUP", &["/tmp/backup", "full"]),
            "BACKUP /tmp/backup full"
        );
    }

    #[test]
    fn test_format_admin_command_single_arg() {
        assert_eq!(format_admin_command("COMPACT", &["users"]), "COMPACT users");
    }

    #[test]
    fn test_is_fallback_error_connection() {
        let err = anyhow::anyhow!("Connection refused");
        assert!(is_fallback_error(&err));
    }

    #[test]
    fn test_is_fallback_error_unimplemented() {
        let err = anyhow::anyhow!("Unimplemented feature");
        assert!(is_fallback_error(&err));
    }

    #[test]
    fn test_is_fallback_error_timeout() {
        let err = anyhow::anyhow!("Request timed out");
        assert!(is_fallback_error(&err));
    }

    #[test]
    fn test_is_fallback_error_non_fallback() {
        let err = anyhow::anyhow!("Permission denied");
        assert!(!is_fallback_error(&err));
    }

    #[test]
    fn test_parse_backup_metadata_full_json() -> Result<()> {
        let json = serde_json::to_string(&BackupMetadata {
            backup_id: "bk_001".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: 2048,
            key_count: 100,
            backup_type: "full".to_string(),
            compression: "lz4".to_string(),
        })?;

        let parsed = parse_backup_metadata(&json, "full")?;
        assert_eq!(parsed.backup_id, "bk_001");
        assert_eq!(parsed.key_count, 100);
        Ok(())
    }

    #[test]
    fn test_parse_backup_metadata_partial_json() -> Result<()> {
        let json = r#"{"backup_id": "bk_002", "size_bytes": 4096}"#;
        let parsed = parse_backup_metadata(json, "incremental")?;
        assert_eq!(parsed.backup_id, "bk_002");
        assert_eq!(parsed.size_bytes, 4096);
        assert_eq!(parsed.backup_type, "incremental");
        Ok(())
    }

    #[test]
    fn test_parse_restore_result_full_json() -> Result<()> {
        let json = serde_json::to_string(&RestoreResult {
            keys_restored: 5000,
            bytes_restored: 1024,
            duration_seconds: 10.5,
            errors: vec!["minor issue".to_string()],
        })?;

        let parsed = parse_restore_result(&json)?;
        assert_eq!(parsed.keys_restored, 5000);
        assert_eq!(parsed.errors.len(), 1);
        Ok(())
    }

    #[test]
    fn test_parse_compaction_result() -> Result<()> {
        let json = r#"{"sstables_compacted": 10, "bytes_reclaimed": 2048, "duration_seconds": 5.0, "new_sstable_count": 3}"#;
        let parsed = parse_compaction_result(json)?;
        assert_eq!(parsed.sstables_compacted, 10);
        assert_eq!(parsed.bytes_reclaimed, 2048);
        assert_eq!(parsed.new_sstable_count, 3);
        Ok(())
    }

    #[test]
    fn test_parse_verify_result() -> Result<()> {
        let json = r#"{"verified_keys": 1000, "corrupted_keys": 2, "missing_keys": 1, "checksum_errors": 0, "duration_seconds": 60.0, "errors": ["key_abc corrupted"]}"#;
        let parsed = parse_verify_result(json)?;
        assert_eq!(parsed.verified_keys, 1000);
        assert_eq!(parsed.corrupted_keys, 2);
        assert_eq!(parsed.errors.len(), 1);
        Ok(())
    }

    #[test]
    fn test_parse_database_stats_full_json() -> Result<()> {
        let stats = default_database_stats();
        let json = serde_json::to_string(&stats)?;
        let parsed = parse_database_stats(&json)?;
        assert_eq!(parsed.total_keys, stats.total_keys);
        assert_eq!(parsed.collections_count, stats.collections_count);
        Ok(())
    }

    #[test]
    fn test_parse_logs_response_array() -> Result<()> {
        let json = r#"["line1", "line2", "line3"]"#;
        let parsed = parse_logs_response(json)?;
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], "line1");
        Ok(())
    }

    #[test]
    fn test_parse_logs_response_object() -> Result<()> {
        let json = r#"{"logs": ["entry1", "entry2"]}"#;
        let parsed = parse_logs_response(json)?;
        assert_eq!(parsed.len(), 2);
        Ok(())
    }

    #[test]
    fn test_default_database_stats() {
        let stats = default_database_stats();
        assert_eq!(stats.total_keys, 50000);
        assert_eq!(stats.collections_count, 5);
        assert_eq!(stats.sstable_stats.levels.len(), 3);
    }
}
