//! Manifest system for tracking SSTable metadata and LSM-Tree state
//!
//! The manifest provides:
//! - Persistent tracking of SSTable metadata across restarts
//! - Version management for atomic updates
//! - Crash recovery support
//! - Efficient append-only log with periodic compaction

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::SSTableMetadata;
use crate::types::Key;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Manifest version number (monotonically increasing)
pub type ManifestVersion = u64;

/// Type of manifest entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestEntryType {
    /// Add a new SSTable to a level
    AddSSTable = 0,
    /// Remove an SSTable from a level
    RemoveSSTable = 1,
    /// Version change marker
    VersionChange = 2,
}

impl ManifestEntryType {
    fn to_u8(self) -> u8 {
        match self {
            Self::AddSSTable => 0,
            Self::RemoveSSTable => 1,
            Self::VersionChange => 2,
        }
    }

    fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::AddSSTable),
            1 => Ok(Self::RemoveSSTable),
            2 => Ok(Self::VersionChange),
            _ => Err(AmateRSError::ValidationError(ErrorContext::new(format!(
                "Invalid manifest entry type: {}",
                value
            )))),
        }
    }
}

/// Manifest entry representing a single operation
#[derive(Debug, Clone)]
pub struct ManifestEntry {
    /// Entry type
    pub entry_type: ManifestEntryType,
    /// Version number
    pub version: ManifestVersion,
    /// Level number (for SSTable operations)
    pub level: Option<usize>,
    /// SSTable path (for SSTable operations)
    pub path: Option<PathBuf>,
    /// Minimum key (for AddSSTable)
    pub min_key: Option<Vec<u8>>,
    /// Maximum key (for AddSSTable)
    pub max_key: Option<Vec<u8>>,
    /// Number of entries (for AddSSTable)
    pub num_entries: Option<usize>,
    /// File size (for AddSSTable)
    pub file_size: Option<u64>,
}

impl ManifestEntry {
    /// Create an AddSSTable entry
    pub fn add_sstable(version: ManifestVersion, metadata: &SSTableMetadata) -> Self {
        Self {
            entry_type: ManifestEntryType::AddSSTable,
            version,
            level: Some(metadata.level),
            path: Some(metadata.path.clone()),
            min_key: Some(metadata.min_key.as_bytes().to_vec()),
            max_key: Some(metadata.max_key.as_bytes().to_vec()),
            num_entries: Some(metadata.num_entries),
            file_size: Some(metadata.file_size),
        }
    }

    /// Create a RemoveSSTable entry
    pub fn remove_sstable(version: ManifestVersion, level: usize, path: PathBuf) -> Self {
        Self {
            entry_type: ManifestEntryType::RemoveSSTable,
            version,
            level: Some(level),
            path: Some(path),
            min_key: None,
            max_key: None,
            num_entries: None,
            file_size: None,
        }
    }

    /// Create a VersionChange entry
    pub fn version_change(version: ManifestVersion) -> Self {
        Self {
            entry_type: ManifestEntryType::VersionChange,
            version,
            level: None,
            path: None,
            min_key: None,
            max_key: None,
            num_entries: None,
            file_size: None,
        }
    }

    /// Convert to SSTableMetadata (only valid for AddSSTable entries)
    pub fn to_sstable_metadata(&self) -> Result<SSTableMetadata> {
        if self.entry_type != ManifestEntryType::AddSSTable {
            return Err(AmateRSError::ValidationError(ErrorContext::new(
                "Cannot convert non-AddSSTable entry to metadata",
            )));
        }

        let level = self.level.ok_or_else(|| {
            AmateRSError::ValidationError(ErrorContext::new("Missing level in AddSSTable entry"))
        })?;

        let path = self.path.clone().ok_or_else(|| {
            AmateRSError::ValidationError(ErrorContext::new("Missing path in AddSSTable entry"))
        })?;

        let min_key = self.min_key.as_ref().ok_or_else(|| {
            AmateRSError::ValidationError(ErrorContext::new("Missing min_key in AddSSTable entry"))
        })?;

        let max_key = self.max_key.as_ref().ok_or_else(|| {
            AmateRSError::ValidationError(ErrorContext::new("Missing max_key in AddSSTable entry"))
        })?;

        let num_entries = self.num_entries.ok_or_else(|| {
            AmateRSError::ValidationError(ErrorContext::new(
                "Missing num_entries in AddSSTable entry",
            ))
        })?;

        let file_size = self.file_size.ok_or_else(|| {
            AmateRSError::ValidationError(ErrorContext::new(
                "Missing file_size in AddSSTable entry",
            ))
        })?;

        Ok(SSTableMetadata {
            path,
            min_key: Key::from_slice(min_key),
            max_key: Key::from_slice(max_key),
            num_entries,
            file_size,
            level,
        })
    }

    /// Encode entry to bytes
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Entry type (1 byte)
        buf.push(self.entry_type.to_u8());

        // Version (8 bytes)
        buf.extend_from_slice(&self.version.to_le_bytes());

        // Level (1 byte present flag + optional usize)
        if let Some(level) = self.level {
            buf.push(1);
            buf.extend_from_slice(&(level as u64).to_le_bytes());
        } else {
            buf.push(0);
        }

        // Path (1 byte present flag + optional PathBuf as UTF-8 string)
        if let Some(ref path) = self.path {
            buf.push(1);
            let path_str = path.to_string_lossy();
            let path_bytes = path_str.as_bytes();
            buf.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(path_bytes);
        } else {
            buf.push(0);
        }

        // Min key (1 byte present flag + optional bytes)
        if let Some(ref min_key) = self.min_key {
            buf.push(1);
            buf.extend_from_slice(&(min_key.len() as u32).to_le_bytes());
            buf.extend_from_slice(min_key);
        } else {
            buf.push(0);
        }

        // Max key (1 byte present flag + optional bytes)
        if let Some(ref max_key) = self.max_key {
            buf.push(1);
            buf.extend_from_slice(&(max_key.len() as u32).to_le_bytes());
            buf.extend_from_slice(max_key);
        } else {
            buf.push(0);
        }

        // Num entries (1 byte present flag + optional usize)
        if let Some(num_entries) = self.num_entries {
            buf.push(1);
            buf.extend_from_slice(&(num_entries as u64).to_le_bytes());
        } else {
            buf.push(0);
        }

        // File size (1 byte present flag + optional u64)
        if let Some(file_size) = self.file_size {
            buf.push(1);
            buf.extend_from_slice(&file_size.to_le_bytes());
        } else {
            buf.push(0);
        }

        buf
    }

    /// Decode entry from bytes
    fn decode(buf: &[u8]) -> Result<Self> {
        let mut offset = 0;

        // Entry type
        if offset >= buf.len() {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Unexpected end of manifest entry",
            )));
        }
        let entry_type = ManifestEntryType::from_u8(buf[offset])?;
        offset += 1;

        // Version
        if offset + 8 > buf.len() {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Unexpected end reading version",
            )));
        }
        let version = u64::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
            buf[offset + 4],
            buf[offset + 5],
            buf[offset + 6],
            buf[offset + 7],
        ]);
        offset += 8;

        // Level
        if offset >= buf.len() {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Unexpected end reading level flag",
            )));
        }
        let level = if buf[offset] == 1 {
            offset += 1;
            if offset + 8 > buf.len() {
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Unexpected end reading level",
                )));
            }
            let val = u64::from_le_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
                buf[offset + 4],
                buf[offset + 5],
                buf[offset + 6],
                buf[offset + 7],
            ]) as usize;
            offset += 8;
            Some(val)
        } else {
            offset += 1;
            None
        };

        // Path
        if offset >= buf.len() {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Unexpected end reading path flag",
            )));
        }
        let path = if buf[offset] == 1 {
            offset += 1;
            if offset + 4 > buf.len() {
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Unexpected end reading path length",
                )));
            }
            let len = u32::from_le_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
            ]) as usize;
            offset += 4;
            if offset + len > buf.len() {
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Unexpected end reading path data",
                )));
            }
            let path_str = String::from_utf8(buf[offset..offset + len].to_vec()).map_err(|e| {
                AmateRSError::SerializationError(ErrorContext::new(format!(
                    "Invalid UTF-8 in path: {}",
                    e
                )))
            })?;
            offset += len;
            Some(PathBuf::from(path_str))
        } else {
            offset += 1;
            None
        };

        // Min key
        if offset >= buf.len() {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Unexpected end reading min_key flag",
            )));
        }
        let min_key = if buf[offset] == 1 {
            offset += 1;
            if offset + 4 > buf.len() {
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Unexpected end reading min_key length",
                )));
            }
            let len = u32::from_le_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
            ]) as usize;
            offset += 4;
            if offset + len > buf.len() {
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Unexpected end reading min_key data",
                )));
            }
            let key_data = buf[offset..offset + len].to_vec();
            offset += len;
            Some(key_data)
        } else {
            offset += 1;
            None
        };

        // Max key
        if offset >= buf.len() {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Unexpected end reading max_key flag",
            )));
        }
        let max_key = if buf[offset] == 1 {
            offset += 1;
            if offset + 4 > buf.len() {
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Unexpected end reading max_key length",
                )));
            }
            let len = u32::from_le_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
            ]) as usize;
            offset += 4;
            if offset + len > buf.len() {
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Unexpected end reading max_key data",
                )));
            }
            let key_data = buf[offset..offset + len].to_vec();
            offset += len;
            Some(key_data)
        } else {
            offset += 1;
            None
        };

        // Num entries
        if offset >= buf.len() {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Unexpected end reading num_entries flag",
            )));
        }
        let num_entries = if buf[offset] == 1 {
            offset += 1;
            if offset + 8 > buf.len() {
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Unexpected end reading num_entries",
                )));
            }
            let val = u64::from_le_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
                buf[offset + 4],
                buf[offset + 5],
                buf[offset + 6],
                buf[offset + 7],
            ]) as usize;
            offset += 8;
            Some(val)
        } else {
            offset += 1;
            None
        };

        // File size
        if offset >= buf.len() {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Unexpected end reading file_size flag",
            )));
        }
        let file_size = if buf[offset] == 1 {
            offset += 1;
            if offset + 8 > buf.len() {
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Unexpected end reading file_size",
                )));
            }
            let val = u64::from_le_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
                buf[offset + 4],
                buf[offset + 5],
                buf[offset + 6],
                buf[offset + 7],
            ]);
            Some(val)
        } else {
            None
        };

        Ok(Self {
            entry_type,
            version,
            level,
            path,
            min_key,
            max_key,
            num_entries,
            file_size,
        })
    }
}

/// Manifest configuration
#[derive(Debug, Clone)]
pub struct ManifestConfig {
    /// Directory for storing manifest files
    pub manifest_dir: PathBuf,
    /// Maximum manifest file size before compaction (default: 10MB)
    pub max_file_size: u64,
    /// Enable fsync after each write (default: true for durability)
    pub sync_on_write: bool,
}

impl Default for ManifestConfig {
    fn default() -> Self {
        Self {
            manifest_dir: PathBuf::from("./manifest"),
            max_file_size: 10 * 1024 * 1024, // 10MB
            sync_on_write: true,
        }
    }
}

/// Manifest state snapshot
#[derive(Debug, Clone)]
pub struct ManifestSnapshot {
    /// Current version
    pub version: ManifestVersion,
    /// SSTables organized by level
    pub levels: BTreeMap<usize, Vec<SSTableMetadata>>,
}

impl ManifestSnapshot {
    /// Create an empty snapshot
    pub fn new() -> Self {
        Self {
            version: 0,
            levels: BTreeMap::new(),
        }
    }

    /// Apply a manifest entry to the snapshot
    pub fn apply_entry(&mut self, entry: &ManifestEntry) -> Result<()> {
        match entry.entry_type {
            ManifestEntryType::AddSSTable => {
                let metadata = entry.to_sstable_metadata()?;
                let level = metadata.level;
                self.levels.entry(level).or_default().push(metadata);
            }
            ManifestEntryType::RemoveSSTable => {
                if let (Some(level), Some(path)) = (entry.level, &entry.path) {
                    if let Some(sstables) = self.levels.get_mut(&level) {
                        sstables.retain(|s| &s.path != path);
                    }
                }
            }
            ManifestEntryType::VersionChange => {
                self.version = entry.version;
            }
        }
        Ok(())
    }

    /// Get total number of SSTables
    pub fn total_sstables(&self) -> usize {
        self.levels.values().map(|v| v.len()).sum()
    }

    /// Get total size in bytes
    pub fn total_size(&self) -> u64 {
        self.levels
            .values()
            .flat_map(|v| v.iter())
            .map(|s| s.file_size)
            .sum()
    }
}

impl Default for ManifestSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

/// Manifest manager for persisting LSM-Tree metadata
pub struct Manifest {
    /// Configuration
    config: ManifestConfig,
    /// Current manifest file
    current_file: Arc<RwLock<BufWriter<File>>>,
    /// Current manifest path
    current_path: PathBuf,
    /// Current version
    current_version: Arc<RwLock<ManifestVersion>>,
    /// In-memory snapshot of manifest state
    snapshot: Arc<RwLock<ManifestSnapshot>>,
}

impl Manifest {
    /// Create a new manifest or recover from existing one
    pub fn new<P: AsRef<Path>>(manifest_dir: P) -> Result<Self> {
        let config = ManifestConfig {
            manifest_dir: manifest_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        Self::with_config(config)
    }

    /// Create a new manifest with custom configuration
    pub fn with_config(config: ManifestConfig) -> Result<Self> {
        // Create manifest directory
        std::fs::create_dir_all(&config.manifest_dir).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to create manifest directory: {}",
                e
            )))
        })?;

        // Try to recover from existing manifest
        let (snapshot, version) = Self::recover_from_manifest(&config)?;

        // Open current manifest file for appending
        let current_path = config.manifest_dir.join("MANIFEST-CURRENT");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&current_path)
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to open manifest file: {}",
                    e
                )))
            })?;

        let writer = BufWriter::new(file);

        Ok(Self {
            config,
            current_file: Arc::new(RwLock::new(writer)),
            current_path,
            current_version: Arc::new(RwLock::new(version)),
            snapshot: Arc::new(RwLock::new(snapshot)),
        })
    }

    /// Recover manifest state from existing files
    fn recover_from_manifest(
        config: &ManifestConfig,
    ) -> Result<(ManifestSnapshot, ManifestVersion)> {
        let current_path = config.manifest_dir.join("MANIFEST-CURRENT");

        // If no manifest exists, return empty state
        if !current_path.exists() {
            return Ok((ManifestSnapshot::new(), 0));
        }

        let file = File::open(&current_path).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to open manifest file for recovery: {}",
                e
            )))
        })?;

        let mut reader = BufReader::new(file);
        let mut snapshot = ManifestSnapshot::new();
        let mut max_version = 0u64;

        // Read all entries
        loop {
            // Try to decode an entry
            match Self::read_entry(&mut reader) {
                Ok(entry) => {
                    if entry.version > max_version {
                        max_version = entry.version;
                    }
                    snapshot.apply_entry(&entry)?;
                }
                Err(e) => {
                    // Check if it's EOF or a real error
                    if let AmateRSError::SerializationError(_) = e {
                        // Likely EOF, stop reading
                        break;
                    }
                    return Err(e);
                }
            }
        }

        Ok((snapshot, max_version))
    }

    /// Read a single entry from the reader
    fn read_entry<R: Read>(reader: &mut R) -> Result<ManifestEntry> {
        // Read entry length (u32)
        let mut len_buf = [0u8; 4];
        if reader.read_exact(&mut len_buf).is_err() {
            // EOF or read error
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Failed to read entry length (likely EOF)",
            )));
        }

        let entry_len = u32::from_le_bytes(len_buf) as usize;

        // Sanity check
        if entry_len > 10 * 1024 * 1024 {
            // 10MB max entry size
            return Err(AmateRSError::ValidationError(ErrorContext::new(
                "Manifest entry too large",
            )));
        }

        // Read entry data
        let mut entry_buf = vec![0u8; entry_len];
        reader.read_exact(&mut entry_buf).map_err(|e| {
            AmateRSError::SerializationError(ErrorContext::new(format!(
                "Failed to read entry data: {}",
                e
            )))
        })?;

        // Deserialize entry
        ManifestEntry::decode(&entry_buf)
    }

    /// Write an entry to the manifest
    fn write_entry(&self, entry: &ManifestEntry) -> Result<()> {
        let mut writer = self.current_file.write();

        // Serialize entry
        let entry_bytes = entry.encode();

        // Write length prefix
        let len_bytes = (entry_bytes.len() as u32).to_le_bytes();
        writer.write_all(&len_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to write entry length: {}",
                e
            )))
        })?;

        // Write entry data
        writer.write_all(&entry_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to write entry data: {}",
                e
            )))
        })?;

        // Sync if configured
        if self.config.sync_on_write {
            writer.flush().map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to flush manifest: {}",
                    e
                )))
            })?;

            writer.get_ref().sync_all().map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to sync manifest: {}",
                    e
                )))
            })?;
        }

        Ok(())
    }

    /// Add an SSTable to the manifest
    pub fn add_sstable(&self, metadata: &SSTableMetadata) -> Result<()> {
        let version = {
            let mut ver = self.current_version.write();
            *ver += 1;
            *ver
        };

        let entry = ManifestEntry::add_sstable(version, metadata);
        self.write_entry(&entry)?;

        // Update snapshot
        let mut snapshot = self.snapshot.write();
        snapshot.apply_entry(&entry)?;

        Ok(())
    }

    /// Remove an SSTable from the manifest
    pub fn remove_sstable(&self, level: usize, path: PathBuf) -> Result<()> {
        let version = {
            let mut ver = self.current_version.write();
            *ver += 1;
            *ver
        };

        let entry = ManifestEntry::remove_sstable(version, level, path);
        self.write_entry(&entry)?;

        // Update snapshot
        let mut snapshot = self.snapshot.write();
        snapshot.apply_entry(&entry)?;

        Ok(())
    }

    /// Record a version change
    pub fn record_version_change(&self) -> Result<ManifestVersion> {
        let version = {
            let mut ver = self.current_version.write();
            *ver += 1;
            *ver
        };

        let entry = ManifestEntry::version_change(version);
        self.write_entry(&entry)?;

        Ok(version)
    }

    /// Get current snapshot
    pub fn snapshot(&self) -> ManifestSnapshot {
        self.snapshot.read().clone()
    }

    /// Get current version
    pub fn version(&self) -> ManifestVersion {
        *self.current_version.read()
    }

    /// Compact the manifest file (remove redundant entries)
    pub fn compact(&self) -> Result<()> {
        // Get current snapshot
        let snapshot = self.snapshot.read().clone();

        // Create a new temporary manifest file
        let temp_path = self.config.manifest_dir.join("MANIFEST-TEMP");
        let temp_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to create temp manifest: {}",
                    e
                )))
            })?;

        let mut temp_writer = BufWriter::new(temp_file);

        // Write version change entry
        let version = *self.current_version.read();
        let version_entry = ManifestEntry::version_change(version);
        let version_bytes = version_entry.encode();
        let len_bytes = (version_bytes.len() as u32).to_le_bytes();
        temp_writer.write_all(&len_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to write version length: {}",
                e
            )))
        })?;
        temp_writer.write_all(&version_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to write version data: {}",
                e
            )))
        })?;

        // Write all current SSTables
        for sstables in snapshot.levels.values() {
            for sstable in sstables {
                let entry = ManifestEntry::add_sstable(version, sstable);
                let entry_bytes = entry.encode();
                let len_bytes = (entry_bytes.len() as u32).to_le_bytes();
                temp_writer.write_all(&len_bytes).map_err(|e| {
                    AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                        "Failed to write entry length: {}",
                        e
                    )))
                })?;
                temp_writer.write_all(&entry_bytes).map_err(|e| {
                    AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                        "Failed to write entry data: {}",
                        e
                    )))
                })?;
            }
        }

        temp_writer.flush().map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to flush temp manifest: {}",
                e
            )))
        })?;

        temp_writer.get_ref().sync_all().map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to sync temp manifest: {}",
                e
            )))
        })?;

        drop(temp_writer);

        // Replace old manifest with new one
        {
            let mut writer = self.current_file.write();
            drop(writer); // Close the current file
        }

        std::fs::rename(&temp_path, &self.current_path).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to replace manifest file: {}",
                e
            )))
        })?;

        // Reopen the file
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.current_path)
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to reopen manifest file: {}",
                    e
                )))
            })?;

        *self.current_file.write() = BufWriter::new(file);

        Ok(())
    }

    /// Check if manifest needs compaction
    pub fn needs_compaction(&self) -> Result<bool> {
        let metadata = std::fs::metadata(&self.current_path).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to get manifest metadata: {}",
                e
            )))
        })?;

        Ok(metadata.len() > self.config.max_file_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Key;
    use std::env;

    #[test]
    fn test_manifest_creation() -> Result<()> {
        let dir = env::temp_dir().join("test_manifest_creation");
        std::fs::create_dir_all(&dir).ok();

        let manifest = Manifest::new(&dir)?;
        assert_eq!(manifest.version(), 0);
        assert_eq!(manifest.snapshot().total_sstables(), 0);

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_manifest_add_sstable() -> Result<()> {
        let dir = env::temp_dir().join("test_manifest_add");
        std::fs::create_dir_all(&dir).ok();

        let manifest = Manifest::new(&dir)?;

        let metadata = SSTableMetadata {
            path: PathBuf::from("/test/table1.sst"),
            min_key: Key::from_str("aaa"),
            max_key: Key::from_str("zzz"),
            num_entries: 100,
            file_size: 1024,
            level: 0,
        };

        manifest.add_sstable(&metadata)?;

        let snapshot = manifest.snapshot();
        assert_eq!(snapshot.total_sstables(), 1);
        assert_eq!(manifest.version(), 1);

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_manifest_remove_sstable() -> Result<()> {
        let dir = env::temp_dir().join("test_manifest_remove");
        std::fs::create_dir_all(&dir).ok();

        let manifest = Manifest::new(&dir)?;

        let path = PathBuf::from("/test/table1.sst");
        let metadata = SSTableMetadata {
            path: path.clone(),
            min_key: Key::from_str("aaa"),
            max_key: Key::from_str("zzz"),
            num_entries: 100,
            file_size: 1024,
            level: 0,
        };

        manifest.add_sstable(&metadata)?;
        assert_eq!(manifest.snapshot().total_sstables(), 1);

        manifest.remove_sstable(0, path)?;
        assert_eq!(manifest.snapshot().total_sstables(), 0);

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_manifest_recovery() -> Result<()> {
        let dir = env::temp_dir().join("test_manifest_recovery");
        std::fs::create_dir_all(&dir).ok();

        // Create manifest and add some entries
        {
            let manifest = Manifest::new(&dir)?;

            for i in 0..5 {
                let metadata = SSTableMetadata {
                    path: PathBuf::from(format!("/test/table{}.sst", i)),
                    min_key: Key::from_str(&format!("key{:03}", i * 100)),
                    max_key: Key::from_str(&format!("key{:03}", (i + 1) * 100 - 1)),
                    num_entries: 100,
                    file_size: 1024 * (i as u64 + 1),
                    level: i % 3,
                };
                manifest.add_sstable(&metadata)?;
            }

            assert_eq!(manifest.snapshot().total_sstables(), 5);
        }

        // Recover and verify
        {
            let manifest = Manifest::new(&dir)?;
            let snapshot = manifest.snapshot();

            assert_eq!(snapshot.total_sstables(), 5);
            assert!(manifest.version() >= 5);
        }

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_manifest_compaction() -> Result<()> {
        let dir = env::temp_dir().join("test_manifest_compaction");
        std::fs::create_dir_all(&dir).ok();

        let manifest = Manifest::new(&dir)?;

        // Add many entries
        for i in 0..20 {
            let metadata = SSTableMetadata {
                path: PathBuf::from(format!("/test/table{}.sst", i)),
                min_key: Key::from_str(&format!("key{:04}", i)),
                max_key: Key::from_str(&format!("key{:04}", i + 1)),
                num_entries: 10,
                file_size: 512,
                level: 0,
            };
            manifest.add_sstable(&metadata)?;
        }

        let before_size = std::fs::metadata(manifest.current_path.clone())?.len();

        // Compact manifest
        manifest.compact()?;

        let after_size = std::fs::metadata(manifest.current_path.clone())?.len();

        // After compaction, snapshot should still have all entries
        assert_eq!(manifest.snapshot().total_sstables(), 20);

        // File size should be similar or smaller (compaction removes redundancy)
        // In this case, we only have AddSSTable entries, so size might be similar

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_manifest_version_tracking() -> Result<()> {
        let dir = env::temp_dir().join("test_manifest_version");
        std::fs::create_dir_all(&dir).ok();

        let manifest = Manifest::new(&dir)?;
        assert_eq!(manifest.version(), 0);

        manifest.record_version_change()?;
        assert_eq!(manifest.version(), 1);

        let metadata = SSTableMetadata {
            path: PathBuf::from("/test/table1.sst"),
            min_key: Key::from_str("aaa"),
            max_key: Key::from_str("zzz"),
            num_entries: 100,
            file_size: 1024,
            level: 0,
        };
        manifest.add_sstable(&metadata)?;
        assert_eq!(manifest.version(), 2);

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}
