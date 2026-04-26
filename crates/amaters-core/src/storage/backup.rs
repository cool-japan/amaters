//! Backup and restore functionality for the storage engine
//!
//! Provides full backup creation, restoration, integrity verification,
//! and backup lifecycle management. Backups are stored as direct file
//! copies (no compression/archiving) with CRC32 checksums for integrity.

use crate::error::{AmateRSError, ErrorContext, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Backup metadata persisted alongside backup data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    /// Unique identifier for this backup
    pub backup_id: String,
    /// Timestamp when the backup was created
    pub created_at: DateTime<Utc>,
    /// Source directory that was backed up
    pub source_dir: PathBuf,
    /// Total number of files in the backup
    pub total_files: usize,
    /// Total size in bytes of all backed-up files
    pub total_bytes: u64,
    /// CRC32 checksum of all files (deterministic ordering)
    pub checksum: u32,
    /// Type of backup (full or incremental)
    pub backup_type: BackupType,
    /// Software version at time of backup
    pub version: String,
}

/// Type of backup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackupType {
    /// Full backup containing all data
    Full,
    /// Incremental backup relative to a base backup
    Incremental {
        /// ID of the base backup this increment builds upon
        base_backup_id: String,
    },
}

/// Manages backup creation, restoration, and lifecycle
pub struct BackupManager {
    /// Root directory where all backups are stored
    backup_dir: PathBuf,
}

impl BackupManager {
    /// Create a new BackupManager with the given backup storage directory.
    ///
    /// Creates the directory if it does not exist.
    pub fn new(backup_dir: impl AsRef<Path>) -> Result<Self> {
        let backup_dir = backup_dir.as_ref().to_path_buf();
        fs::create_dir_all(&backup_dir).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to create backup directory '{}': {}",
                backup_dir.display(),
                e
            )))
        })?;
        Ok(Self { backup_dir })
    }

    /// Create a full backup of the source data directory.
    ///
    /// Copies all files recursively from `source_dir` into a new backup
    /// subdirectory, calculates a CRC32 checksum, and writes metadata.
    pub fn create_backup(&self, source_dir: &Path) -> Result<BackupMetadata> {
        if !source_dir.exists() {
            return Err(AmateRSError::ValidationError(ErrorContext::new(format!(
                "Source directory '{}' does not exist",
                source_dir.display()
            ))));
        }

        let backup_id = uuid::Uuid::new_v4().to_string();
        let backup_path = self.backup_dir.join(&backup_id);
        let data_path = backup_path.join("data");

        fs::create_dir_all(&data_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to create backup data directory: {}",
                e
            )))
        })?;

        let (total_files, total_bytes) = copy_dir_recursive(source_dir, &data_path)?;
        let checksum = calculate_dir_checksum(&data_path)?;

        let metadata = BackupMetadata {
            backup_id: backup_id.clone(),
            created_at: Utc::now(),
            source_dir: source_dir.to_path_buf(),
            total_files,
            total_bytes,
            checksum,
            backup_type: BackupType::Full,
            version: env!("CARGO_PKG_VERSION").to_string(),
        };

        let metadata_path = backup_path.join("metadata.json");
        let metadata_json = serde_json::to_string_pretty(&metadata).map_err(|e| {
            AmateRSError::SerializationError(ErrorContext::new(format!(
                "Failed to serialize backup metadata: {}",
                e
            )))
        })?;
        fs::write(&metadata_path, metadata_json).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to write backup metadata: {}",
                e
            )))
        })?;

        Ok(metadata)
    }

    /// Restore a backup to the given target directory.
    ///
    /// Verifies backup integrity before restoring. If `target_dir` already
    /// exists, it is cleared first.
    pub fn restore_backup(&self, backup_id: &str, target_dir: &Path) -> Result<BackupMetadata> {
        let backup_path = self.backup_dir.join(backup_id);
        if !backup_path.exists() {
            return Err(AmateRSError::ValidationError(ErrorContext::new(format!(
                "Backup '{}' does not exist",
                backup_id
            ))));
        }

        let metadata = self.load_metadata(backup_id)?;

        // Verify integrity before restoring
        if !self.verify_backup(backup_id)? {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Backup '{}' failed integrity check",
                backup_id
            ))));
        }

        // Clear target directory if it exists
        if target_dir.exists() {
            fs::remove_dir_all(target_dir).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to clear target directory '{}': {}",
                    target_dir.display(),
                    e
                )))
            })?;
        }

        fs::create_dir_all(target_dir).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to create target directory '{}': {}",
                target_dir.display(),
                e
            )))
        })?;

        let data_path = backup_path.join("data");
        copy_dir_recursive(&data_path, target_dir)?;

        // Verify restored data matches backup checksum
        let restored_checksum = calculate_dir_checksum(target_dir)?;
        if restored_checksum != metadata.checksum {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Restored data checksum mismatch: expected {}, got {}",
                metadata.checksum, restored_checksum
            ))));
        }

        Ok(metadata)
    }

    /// List all available backups sorted by creation time (newest first).
    pub fn list_backups(&self) -> Result<Vec<BackupMetadata>> {
        let mut backups = Vec::new();

        let entries = fs::read_dir(&self.backup_dir).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to read backup directory: {}",
                e
            )))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read directory entry: {}",
                    e
                )))
            })?;

            let path = entry.path();
            if path.is_dir() {
                let metadata_path = path.join("metadata.json");
                if metadata_path.exists() {
                    match self.load_metadata_from_path(&metadata_path) {
                        Ok(meta) => backups.push(meta),
                        Err(_) => {
                            // Skip directories without valid metadata
                            continue;
                        }
                    }
                }
            }
        }

        // Sort by creation time, newest first
        backups.sort_by_key(|b| std::cmp::Reverse(b.created_at));

        Ok(backups)
    }

    /// Delete a backup and all its data.
    pub fn delete_backup(&self, backup_id: &str) -> Result<()> {
        let backup_path = self.backup_dir.join(backup_id);
        if !backup_path.exists() {
            return Err(AmateRSError::ValidationError(ErrorContext::new(format!(
                "Backup '{}' does not exist",
                backup_id
            ))));
        }

        fs::remove_dir_all(&backup_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to delete backup '{}': {}",
                backup_id, e
            )))
        })?;

        Ok(())
    }

    /// Verify backup integrity by recalculating the CRC32 checksum.
    ///
    /// Returns `true` if the checksum matches, `false` otherwise.
    pub fn verify_backup(&self, backup_id: &str) -> Result<bool> {
        let backup_path = self.backup_dir.join(backup_id);
        if !backup_path.exists() {
            return Err(AmateRSError::ValidationError(ErrorContext::new(format!(
                "Backup '{}' does not exist",
                backup_id
            ))));
        }

        let metadata = self.load_metadata(backup_id)?;
        let data_path = backup_path.join("data");

        if !data_path.exists() {
            return Ok(metadata.total_files == 0 && metadata.checksum == 0);
        }

        let current_checksum = calculate_dir_checksum(&data_path)?;
        Ok(current_checksum == metadata.checksum)
    }

    /// Get the total size in bytes of a backup (data files only).
    pub fn backup_size(&self, backup_id: &str) -> Result<u64> {
        let backup_path = self.backup_dir.join(backup_id);
        if !backup_path.exists() {
            return Err(AmateRSError::ValidationError(ErrorContext::new(format!(
                "Backup '{}' does not exist",
                backup_id
            ))));
        }

        let data_path = backup_path.join("data");
        if !data_path.exists() {
            return Ok(0);
        }

        calculate_dir_size(&data_path)
    }

    /// Load backup metadata from the standard location.
    fn load_metadata(&self, backup_id: &str) -> Result<BackupMetadata> {
        let metadata_path = self.backup_dir.join(backup_id).join("metadata.json");
        self.load_metadata_from_path(&metadata_path)
    }

    /// Load backup metadata from an arbitrary path.
    fn load_metadata_from_path(&self, path: &Path) -> Result<BackupMetadata> {
        let content = fs::read_to_string(path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to read metadata file '{}': {}",
                path.display(),
                e
            )))
        })?;

        serde_json::from_str(&content).map_err(|e| {
            AmateRSError::SerializationError(ErrorContext::new(format!(
                "Failed to deserialize backup metadata: {}",
                e
            )))
        })
    }
}

/// Copy a directory recursively from `src` to `dst`.
///
/// Returns `(file_count, total_bytes)` of all files copied.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(usize, u64)> {
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;

    if !src.exists() {
        return Ok((0, 0));
    }

    fs::create_dir_all(dst).map_err(|e| {
        AmateRSError::IoError(ErrorContext::new(format!(
            "Failed to create directory '{}': {}",
            dst.display(),
            e
        )))
    })?;

    let entries = fs::read_dir(src).map_err(|e| {
        AmateRSError::IoError(ErrorContext::new(format!(
            "Failed to read directory '{}': {}",
            src.display(),
            e
        )))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to read directory entry: {}",
                e
            )))
        })?;

        let src_path = entry.path();
        let file_name = entry.file_name();
        let dst_path = dst.join(&file_name);

        if src_path.is_dir() {
            let (sub_files, sub_bytes) = copy_dir_recursive(&src_path, &dst_path)?;
            file_count += sub_files;
            total_bytes += sub_bytes;
        } else if src_path.is_file() {
            let bytes = fs::copy(&src_path, &dst_path).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to copy '{}' -> '{}': {}",
                    src_path.display(),
                    dst_path.display(),
                    e
                )))
            })?;
            file_count += 1;
            total_bytes += bytes;
        }
    }

    Ok((file_count, total_bytes))
}

/// Calculate CRC32 checksum of all files in a directory.
///
/// Files are processed in sorted order (by relative path) for determinism.
fn calculate_dir_checksum(dir: &Path) -> Result<u32> {
    let mut paths = collect_file_paths(dir, dir)?;
    paths.sort();

    let mut hasher = crc32fast::Hasher::new();

    for relative_path in &paths {
        let full_path = dir.join(relative_path);

        // Include the relative path in the checksum for structural integrity
        hasher.update(relative_path.to_string_lossy().as_bytes());

        let mut file = fs::File::open(&full_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to open file '{}' for checksum: {}",
                full_path.display(),
                e
            )))
        })?;

        let mut buffer = [0u8; 8192];
        loop {
            let bytes_read = file.read(&mut buffer).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read file '{}' for checksum: {}",
                    full_path.display(),
                    e
                )))
            })?;

            if bytes_read == 0 {
                break;
            }

            hasher.update(&buffer[..bytes_read]);
        }
    }

    Ok(hasher.finalize())
}

/// Collect all file paths relative to `base_dir` under `dir`.
fn collect_file_paths(dir: &Path, base_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();

    if !dir.exists() {
        return Ok(paths);
    }

    let entries = fs::read_dir(dir).map_err(|e| {
        AmateRSError::IoError(ErrorContext::new(format!(
            "Failed to read directory '{}': {}",
            dir.display(),
            e
        )))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to read directory entry: {}",
                e
            )))
        })?;

        let path = entry.path();

        if path.is_dir() {
            let sub_paths = collect_file_paths(&path, base_dir)?;
            paths.extend(sub_paths);
        } else if path.is_file() {
            let relative = path.strip_prefix(base_dir).map_err(|e| {
                AmateRSError::ValidationError(ErrorContext::new(format!(
                    "Failed to compute relative path: {}",
                    e
                )))
            })?;
            paths.push(relative.to_path_buf());
        }
    }

    Ok(paths)
}

/// Calculate total size of all files in a directory recursively.
fn calculate_dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0u64;

    let entries = fs::read_dir(dir).map_err(|e| {
        AmateRSError::IoError(ErrorContext::new(format!(
            "Failed to read directory '{}': {}",
            dir.display(),
            e
        )))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to read directory entry: {}",
                e
            )))
        })?;

        let path = entry.path();
        if path.is_dir() {
            total += calculate_dir_size(&path)?;
        } else if path.is_file() {
            let meta = fs::metadata(&path).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to get file metadata '{}': {}",
                    path.display(),
                    e
                )))
            })?;
            total += meta.len();
        }
    }

    Ok(total)
}

/// Verify that a directory's contents match an expected CRC32 checksum.
pub fn verify_directory(dir: &Path, expected_checksum: u32) -> Result<bool> {
    let actual = calculate_dir_checksum(dir)?;
    Ok(actual == expected_checksum)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a unique temp directory for a test
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("amaters_backup_tests")
            .join(name)
            .join(uuid::Uuid::new_v4().to_string());
        if dir.exists() {
            fs::remove_dir_all(&dir).ok();
        }
        fs::create_dir_all(&dir).ok();
        dir
    }

    /// Populate a directory with sample files for testing
    fn populate_source(dir: &Path) -> Result<()> {
        fs::create_dir_all(dir.join("subdir")).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("populate_source: {}", e)))
        })?;

        fs::write(dir.join("file1.dat"), b"hello world").map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("populate_source: {}", e)))
        })?;

        fs::write(dir.join("file2.dat"), b"test data 1234567890").map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("populate_source: {}", e)))
        })?;

        fs::write(dir.join("subdir").join("nested.dat"), b"nested content").map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("populate_source: {}", e)))
        })?;

        Ok(())
    }

    #[test]
    fn test_create_full_backup() -> Result<()> {
        let root = test_dir("create_full");
        let source = root.join("source");
        let backups = root.join("backups");

        populate_source(&source)?;
        let manager = BackupManager::new(&backups)?;
        let meta = manager.create_backup(&source)?;

        assert_eq!(meta.total_files, 3);
        assert!(meta.total_bytes > 0);
        assert!(matches!(meta.backup_type, BackupType::Full));

        // Verify backup directory exists with data and metadata
        let backup_path = backups.join(&meta.backup_id);
        assert!(backup_path.join("data").exists());
        assert!(backup_path.join("metadata.json").exists());
        assert!(backup_path.join("data").join("file1.dat").exists());
        assert!(
            backup_path
                .join("data")
                .join("subdir")
                .join("nested.dat")
                .exists()
        );

        fs::remove_dir_all(&root).ok();
        Ok(())
    }

    #[test]
    fn test_restore_backup() -> Result<()> {
        let root = test_dir("restore");
        let source = root.join("source");
        let backups = root.join("backups");
        let restored = root.join("restored");

        populate_source(&source)?;
        let manager = BackupManager::new(&backups)?;
        let meta = manager.create_backup(&source)?;

        let restored_meta = manager.restore_backup(&meta.backup_id, &restored)?;
        assert_eq!(restored_meta.backup_id, meta.backup_id);

        // Verify restored files match original
        let original_content = fs::read(source.join("file1.dat"))
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("read: {}", e))))?;
        let restored_content = fs::read(restored.join("file1.dat"))
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("read: {}", e))))?;
        assert_eq!(original_content, restored_content);

        let nested_original = fs::read(source.join("subdir").join("nested.dat"))
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("read: {}", e))))?;
        let nested_restored = fs::read(restored.join("subdir").join("nested.dat"))
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("read: {}", e))))?;
        assert_eq!(nested_original, nested_restored);

        fs::remove_dir_all(&root).ok();
        Ok(())
    }

    #[test]
    fn test_list_backups() -> Result<()> {
        let root = test_dir("list");
        let source = root.join("source");
        let backups = root.join("backups");

        populate_source(&source)?;
        let manager = BackupManager::new(&backups)?;

        // Create multiple backups
        let _meta1 = manager.create_backup(&source)?;
        let _meta2 = manager.create_backup(&source)?;
        let _meta3 = manager.create_backup(&source)?;

        let list = manager.list_backups()?;
        assert_eq!(list.len(), 3);

        // Should be sorted newest first
        assert!(list[0].created_at >= list[1].created_at);
        assert!(list[1].created_at >= list[2].created_at);

        fs::remove_dir_all(&root).ok();
        Ok(())
    }

    #[test]
    fn test_delete_backup() -> Result<()> {
        let root = test_dir("delete");
        let source = root.join("source");
        let backups = root.join("backups");

        populate_source(&source)?;
        let manager = BackupManager::new(&backups)?;
        let meta = manager.create_backup(&source)?;

        assert_eq!(manager.list_backups()?.len(), 1);

        manager.delete_backup(&meta.backup_id)?;

        assert_eq!(manager.list_backups()?.len(), 0);

        // Deleting non-existent backup should error
        let result = manager.delete_backup("nonexistent");
        assert!(result.is_err());

        fs::remove_dir_all(&root).ok();
        Ok(())
    }

    #[test]
    fn test_verify_backup() -> Result<()> {
        let root = test_dir("verify");
        let source = root.join("source");
        let backups = root.join("backups");

        populate_source(&source)?;
        let manager = BackupManager::new(&backups)?;
        let meta = manager.create_backup(&source)?;

        // Should pass verification
        assert!(manager.verify_backup(&meta.backup_id)?);

        // Corrupt a file and verify should fail
        let corrupt_path = backups.join(&meta.backup_id).join("data").join("file1.dat");
        fs::write(&corrupt_path, b"corrupted!")
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("write: {}", e))))?;

        assert!(!manager.verify_backup(&meta.backup_id)?);

        fs::remove_dir_all(&root).ok();
        Ok(())
    }

    #[test]
    fn test_backup_with_data() -> Result<()> {
        let root = test_dir("with_data");
        let source = root.join("source");
        let backups = root.join("backups");
        let restored = root.join("restored");

        // Create source with binary data simulating SSTable/WAL content
        fs::create_dir_all(source.join("wal"))
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("mkdir: {}", e))))?;
        fs::create_dir_all(source.join("sstables").join("L0"))
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("mkdir: {}", e))))?;

        let wal_data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
        fs::write(source.join("wal").join("000001.wal"), &wal_data)
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("write: {}", e))))?;

        let sst_data: Vec<u8> = (0..1024).map(|i| ((i * 7) % 256) as u8).collect();
        fs::write(
            source.join("sstables").join("L0").join("table_001.sst"),
            &sst_data,
        )
        .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("write: {}", e))))?;

        let manager = BackupManager::new(&backups)?;
        let meta = manager.create_backup(&source)?;

        assert_eq!(meta.total_files, 2);

        // Restore and verify binary content
        manager.restore_backup(&meta.backup_id, &restored)?;

        let restored_wal = fs::read(restored.join("wal").join("000001.wal"))
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("read: {}", e))))?;
        assert_eq!(restored_wal, wal_data);

        let restored_sst = fs::read(restored.join("sstables").join("L0").join("table_001.sst"))
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("read: {}", e))))?;
        assert_eq!(restored_sst, sst_data);

        fs::remove_dir_all(&root).ok();
        Ok(())
    }

    #[test]
    fn test_backup_metadata_serialization() -> Result<()> {
        let meta = BackupMetadata {
            backup_id: "test-id-123".to_string(),
            created_at: Utc::now(),
            source_dir: PathBuf::from("/tmp/source"),
            total_files: 42,
            total_bytes: 123456,
            checksum: 0xDEAD_BEEF,
            backup_type: BackupType::Full,
            version: "0.2.0".to_string(),
        };

        let json = serde_json::to_string(&meta).map_err(|e| {
            AmateRSError::SerializationError(ErrorContext::new(format!("serialize: {}", e)))
        })?;

        let deserialized: BackupMetadata = serde_json::from_str(&json).map_err(|e| {
            AmateRSError::SerializationError(ErrorContext::new(format!("deserialize: {}", e)))
        })?;

        assert_eq!(deserialized.backup_id, meta.backup_id);
        assert_eq!(deserialized.total_files, meta.total_files);
        assert_eq!(deserialized.total_bytes, meta.total_bytes);
        assert_eq!(deserialized.checksum, meta.checksum);
        assert!(matches!(deserialized.backup_type, BackupType::Full));

        // Test incremental variant
        let incremental_meta = BackupMetadata {
            backup_type: BackupType::Incremental {
                base_backup_id: "base-123".to_string(),
            },
            ..meta
        };

        let json2 = serde_json::to_string(&incremental_meta).map_err(|e| {
            AmateRSError::SerializationError(ErrorContext::new(format!("serialize: {}", e)))
        })?;

        let deser2: BackupMetadata = serde_json::from_str(&json2).map_err(|e| {
            AmateRSError::SerializationError(ErrorContext::new(format!("deserialize: {}", e)))
        })?;

        if let BackupType::Incremental { base_backup_id } = &deser2.backup_type {
            assert_eq!(base_backup_id, "base-123");
        } else {
            return Err(AmateRSError::ValidationError(ErrorContext::new(
                "Expected Incremental backup type",
            )));
        }

        Ok(())
    }

    #[test]
    fn test_backup_empty_database() -> Result<()> {
        let root = test_dir("empty_db");
        let source = root.join("source");
        let backups = root.join("backups");
        let restored = root.join("restored");

        // Create an empty source directory
        fs::create_dir_all(&source)
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("mkdir: {}", e))))?;

        let manager = BackupManager::new(&backups)?;
        let meta = manager.create_backup(&source)?;

        assert_eq!(meta.total_files, 0);
        assert_eq!(meta.total_bytes, 0);

        // Verify and restore empty backup
        assert!(manager.verify_backup(&meta.backup_id)?);
        manager.restore_backup(&meta.backup_id, &restored)?;

        assert!(restored.exists());

        fs::remove_dir_all(&root).ok();
        Ok(())
    }

    #[test]
    fn test_restore_to_existing_directory() -> Result<()> {
        let root = test_dir("restore_existing");
        let source = root.join("source");
        let backups = root.join("backups");
        let target = root.join("target");

        populate_source(&source)?;

        // Pre-create target with different content
        fs::create_dir_all(&target)
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("mkdir: {}", e))))?;
        fs::write(target.join("old_file.txt"), b"old content")
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("write: {}", e))))?;

        let manager = BackupManager::new(&backups)?;
        let meta = manager.create_backup(&source)?;

        // Restore should clear existing content
        manager.restore_backup(&meta.backup_id, &target)?;

        // Old file should be gone
        assert!(!target.join("old_file.txt").exists());

        // New files should be present
        assert!(target.join("file1.dat").exists());
        assert!(target.join("file2.dat").exists());
        assert!(target.join("subdir").join("nested.dat").exists());

        fs::remove_dir_all(&root).ok();
        Ok(())
    }

    #[test]
    fn test_backup_size() -> Result<()> {
        let root = test_dir("backup_size");
        let source = root.join("source");
        let backups = root.join("backups");

        populate_source(&source)?;
        let manager = BackupManager::new(&backups)?;
        let meta = manager.create_backup(&source)?;

        let size = manager.backup_size(&meta.backup_id)?;
        assert_eq!(size, meta.total_bytes);

        fs::remove_dir_all(&root).ok();
        Ok(())
    }

    #[test]
    fn test_verify_directory_helper() -> Result<()> {
        let root = test_dir("verify_dir");
        let source = root.join("source");

        populate_source(&source)?;

        let checksum = calculate_dir_checksum(&source)?;
        assert!(verify_directory(&source, checksum)?);
        assert!(!verify_directory(&source, checksum.wrapping_add(1))?);

        fs::remove_dir_all(&root).ok();
        Ok(())
    }

    #[test]
    fn test_restore_nonexistent_backup() {
        let root = test_dir("restore_nonexistent");
        let backups = root.join("backups");
        let target = root.join("target");

        let manager = BackupManager::new(&backups).expect("BackupManager creation should succeed");
        let result = manager.restore_backup("does-not-exist", &target);
        assert!(result.is_err());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_backup_nonexistent_source() {
        let root = test_dir("backup_nonexistent_source");
        let backups = root.join("backups");

        let manager = BackupManager::new(&backups).expect("BackupManager creation should succeed");
        let result = manager.create_backup(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(result.is_err());

        fs::remove_dir_all(&root).ok();
    }
}
