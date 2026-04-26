//! Value Log (vLog) implementation for WiscKey-style value separation
//!
//! The value log stores large values separately from the LSM-Tree to reduce
//! write amplification. Small values are stored inline, while large values
//! (>threshold) are stored in the vLog and referenced by pointers.
//!
//! Key features:
//! - Sequential append-only writes for high throughput
//! - File rotation when files reach size threshold
//! - Garbage collection to reclaim space from dead values (see `value_log_gc`)
//! - Value pointers stored in LSM-Tree for indirection

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::{CipherBlob, Key};
use dashmap::DashMap;
use parking_lot::RwLock;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// Re-export GC types for backward compatibility
pub use super::value_log_gc::{GcConfig, GcResult, GcStats, SegmentStats};

/// Value pointer for referencing values in the vLog
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuePointer {
    /// File ID (vLog file number)
    pub file_id: u64,
    /// Offset within the file
    pub offset: u64,
    /// Length of the value
    pub length: u32,
    /// CRC32 checksum for verification
    pub checksum: u32,
}

impl ValuePointer {
    /// Create a new value pointer
    pub fn new(file_id: u64, offset: u64, length: u32, checksum: u32) -> Self {
        Self {
            file_id,
            offset,
            length,
            checksum,
        }
    }

    /// Encode to bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(24);
        bytes.extend_from_slice(&self.file_id.to_le_bytes());
        bytes.extend_from_slice(&self.offset.to_le_bytes());
        bytes.extend_from_slice(&self.length.to_le_bytes());
        bytes.extend_from_slice(&self.checksum.to_le_bytes());
        bytes
    }

    /// Decode from bytes
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 24 {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "ValuePointer too short",
            )));
        }

        let file_id = u64::from_le_bytes(bytes[0..8].try_into().map_err(|_| {
            AmateRSError::SerializationError(ErrorContext::new("Failed to read file_id"))
        })?);

        let offset = u64::from_le_bytes(bytes[8..16].try_into().map_err(|_| {
            AmateRSError::SerializationError(ErrorContext::new("Failed to read offset"))
        })?);

        let length = u32::from_le_bytes(bytes[16..20].try_into().map_err(|_| {
            AmateRSError::SerializationError(ErrorContext::new("Failed to read length"))
        })?);

        let checksum = u32::from_le_bytes(bytes[20..24].try_into().map_err(|_| {
            AmateRSError::SerializationError(ErrorContext::new("Failed to read checksum"))
        })?);

        Ok(Self {
            file_id,
            offset,
            length,
            checksum,
        })
    }
}

/// Value log configuration
#[derive(Debug, Clone)]
pub struct ValueLogConfig {
    /// Directory for vLog files
    pub vlog_dir: PathBuf,
    /// Maximum file size before rotation (default: 1GB)
    pub max_file_size: u64,
    /// Value size threshold for separation (default: 1KB)
    pub value_threshold: usize,
    /// Whether to sync after each write (default: false for performance)
    pub sync_on_write: bool,
    /// Garbage collection threshold (default: 0.5 = 50% garbage)
    pub gc_threshold: f64,
}

impl Default for ValueLogConfig {
    fn default() -> Self {
        Self {
            vlog_dir: PathBuf::from("./vlog"),
            max_file_size: 1024 * 1024 * 1024, // 1GB
            value_threshold: 1024,             // 1KB
            sync_on_write: false,
            gc_threshold: 0.5,
        }
    }
}

/// Value log entry
pub(crate) struct VLogEntry {
    /// Key (for GC to identify ownership)
    pub(crate) key: Key,
    /// Value data
    pub(crate) value: CipherBlob,
    /// CRC32 checksum
    pub(crate) checksum: u32,
}

impl VLogEntry {
    pub(crate) fn new(key: Key, value: CipherBlob) -> Self {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(key.as_bytes());
        hasher.update(value.as_bytes());
        let checksum = hasher.finalize();

        Self {
            key,
            value,
            checksum,
        }
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Magic number (0x564C4F47 = "VLOG" in hex)
        bytes.extend_from_slice(&0x564C4F47u32.to_le_bytes());

        // Key length and data
        bytes.extend_from_slice(&(self.key.len() as u32).to_le_bytes());
        bytes.extend_from_slice(self.key.as_bytes());

        // Value length and data
        bytes.extend_from_slice(&(self.value.len() as u32).to_le_bytes());
        bytes.extend_from_slice(self.value.as_bytes());

        // Checksum
        bytes.extend_from_slice(&self.checksum.to_le_bytes());

        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 16 {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "VLogEntry too short",
            )));
        }

        let mut offset = 0;

        // Verify magic number
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if magic != 0x564C4F47 {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Invalid vLog entry magic number",
            )));
        }
        offset += 4;

        // Key
        let key_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().map_err(|_| {
            AmateRSError::SerializationError(ErrorContext::new("Failed to read key length"))
        })?) as usize;
        offset += 4;

        let key_bytes = &bytes[offset..offset + key_len];
        let key = Key::from_slice(key_bytes);
        offset += key_len;

        // Value
        let value_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().map_err(|_| {
            AmateRSError::SerializationError(ErrorContext::new("Failed to read value length"))
        })?) as usize;
        offset += 4;

        let value_bytes = &bytes[offset..offset + value_len];
        let value = CipherBlob::new(value_bytes.to_vec());
        offset += value_len;

        // Checksum
        let checksum = u32::from_le_bytes(bytes[offset..offset + 4].try_into().map_err(|_| {
            AmateRSError::SerializationError(ErrorContext::new("Failed to read checksum"))
        })?);

        let entry = Self {
            key,
            value,
            checksum,
        };

        // Verify checksum
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(entry.key.as_bytes());
        hasher.update(entry.value.as_bytes());
        let calculated = hasher.finalize();

        if calculated != entry.checksum {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "vLog entry checksum mismatch: expected {}, got {}",
                entry.checksum, calculated
            ))));
        }

        Ok(entry)
    }
}

/// Value log for storing large values
pub struct ValueLog {
    /// Configuration
    pub(crate) config: ValueLogConfig,
    /// GC configuration
    pub(crate) gc_config: GcConfig,
    /// Current vLog file number
    pub(crate) current_file_id: Arc<RwLock<u64>>,
    /// Current file writer
    pub(crate) writer: Arc<RwLock<std::io::BufWriter<File>>>,
    /// Current file offset
    pub(crate) current_offset: Arc<RwLock<u64>>,
    /// Current file size
    pub(crate) current_size: Arc<RwLock<u64>>,
    /// Per-segment statistics
    pub(crate) segment_stats: Arc<DashMap<u64, SegmentStats>>,
    /// Whether GC is currently running
    pub(crate) gc_running: Arc<AtomicBool>,
    /// Active readers count per segment (prevents deletion during reads)
    pub(crate) segment_readers: Arc<DashMap<u64, Arc<RwLock<()>>>>,
    /// Timestamp (millis since UNIX epoch) of the last write operation
    pub(crate) last_write_time: Arc<AtomicU64>,
}

impl ValueLog {
    /// Create a new value log with default configuration
    pub fn new(vlog_dir: impl AsRef<Path>) -> Result<Self> {
        let config = ValueLogConfig {
            vlog_dir: vlog_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        Self::with_config(config)
    }

    /// Create a new value log with custom configuration
    pub fn with_config(config: ValueLogConfig) -> Result<Self> {
        Self::with_config_and_gc(config, GcConfig::default())
    }

    /// Create a new value log with custom configuration and GC configuration
    pub fn with_config_and_gc(config: ValueLogConfig, gc_config: GcConfig) -> Result<Self> {
        // Create vLog directory
        std::fs::create_dir_all(&config.vlog_dir).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to create vLog directory: {}",
                e
            )))
        })?;

        // Find the latest vLog file or create a new one
        let file_id = Self::find_latest_vlog(&config)?;
        let file_path = Self::vlog_file_path(&config.vlog_dir, file_id);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to open vLog: {}", e)))
            })?;

        let current_size = file
            .metadata()
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to get vLog file size: {}",
                    e
                )))
            })?
            .len();

        let segment_stats = Arc::new(DashMap::new());
        // Initialize stats for the current segment
        let mut initial_stats = SegmentStats::new();
        initial_stats.total_bytes = current_size;
        initial_stats.live_bytes = current_size;
        segment_stats.insert(file_id, initial_stats);

        let segment_readers = Arc::new(DashMap::new());
        segment_readers.insert(file_id, Arc::new(RwLock::new(())));

        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Ok(Self {
            config,
            gc_config,
            current_file_id: Arc::new(RwLock::new(file_id)),
            writer: Arc::new(RwLock::new(std::io::BufWriter::new(file))),
            current_offset: Arc::new(RwLock::new(current_size)),
            current_size: Arc::new(RwLock::new(current_size)),
            segment_stats,
            gc_running: Arc::new(AtomicBool::new(false)),
            segment_readers,
            last_write_time: Arc::new(AtomicU64::new(now_millis)),
        })
    }

    /// Check if a value should be stored in vLog
    pub fn should_separate(&self, value: &CipherBlob) -> bool {
        value.len() > self.config.value_threshold
    }

    /// Append a value to the vLog and return a pointer
    pub fn append(&self, key: Key, value: CipherBlob) -> Result<ValuePointer> {
        let entry = VLogEntry::new(key, value);
        let entry_bytes = entry.encode();
        let entry_len = entry_bytes.len() as u64;

        // Get current position
        let file_id = *self.current_file_id.read();
        let offset = *self.current_offset.read();

        // Write entry
        {
            let mut writer = self.writer.write();
            writer.write_all(&entry_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to write vLog entry: {}",
                    e
                )))
            })?;

            if self.config.sync_on_write {
                writer.flush().map_err(|e| {
                    AmateRSError::IoError(ErrorContext::new(format!("Failed to flush vLog: {}", e)))
                })?;
            }
        }

        // Update offset and size
        {
            let mut current_offset = self.current_offset.write();
            *current_offset += entry_len;
        }
        {
            let mut current_size = self.current_size.write();
            *current_size += entry_len;
        }

        // Update segment stats
        {
            let mut stats = self
                .segment_stats
                .entry(file_id)
                .or_insert_with(SegmentStats::new);
            stats.record_write(entry_len);
        }

        // Update last write timestamp
        {
            let now_millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            self.last_write_time.store(now_millis, Ordering::Release);
        }

        // Check if rotation is needed
        if *self.current_size.read() >= self.config.max_file_size {
            self.rotate()?;
        }

        // Create pointer
        let pointer = ValuePointer::new(file_id, offset, entry_bytes.len() as u32, entry.checksum);

        Ok(pointer)
    }

    /// Read a value from the vLog using a pointer
    pub fn read(&self, pointer: &ValuePointer) -> Result<CipherBlob> {
        let file_path = Self::vlog_file_path(&self.config.vlog_dir, pointer.file_id);

        // Acquire read lock on the segment to prevent deletion during read
        let reader_lock = self
            .segment_readers
            .entry(pointer.file_id)
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone();
        let _read_guard = reader_lock.read();

        // Open file for reading
        let mut file = File::open(&file_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to open vLog file for reading: {}",
                e
            )))
        })?;

        // Seek to offset
        file.seek(SeekFrom::Start(pointer.offset)).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to seek vLog file: {}",
                e
            )))
        })?;

        // Read entry
        let mut entry_bytes = vec![0u8; pointer.length as usize];
        file.read_exact(&mut entry_bytes).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to read vLog entry: {}",
                e
            )))
        })?;

        // Decode entry
        let entry = VLogEntry::decode(&entry_bytes)?;

        Ok(entry.value)
    }

    /// Rotate to a new vLog file
    pub(crate) fn rotate(&self) -> Result<()> {
        // Flush current file
        {
            let mut writer = self.writer.write();
            writer.flush().map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to flush vLog: {}", e)))
            })?;
        }

        // Increment file ID
        let new_file_id = {
            let mut file_id = self.current_file_id.write();
            *file_id += 1;
            *file_id
        };

        // Create new file
        let new_path = Self::vlog_file_path(&self.config.vlog_dir, new_file_id);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&new_path)
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to create new vLog file: {}",
                    e
                )))
            })?;

        // Update writer
        {
            let mut writer = self.writer.write();
            *writer = std::io::BufWriter::new(file);
        }

        // Reset offset and size
        {
            let mut offset = self.current_offset.write();
            *offset = 0;
        }
        {
            let mut size = self.current_size.write();
            *size = 0;
        }

        // Initialize stats and reader lock for new segment
        self.segment_stats.insert(new_file_id, SegmentStats::new());
        self.segment_readers
            .insert(new_file_id, Arc::new(RwLock::new(())));

        Ok(())
    }

    /// Find the latest vLog file number
    pub(crate) fn find_latest_vlog(config: &ValueLogConfig) -> Result<u64> {
        let mut max_file_id = 0u64;

        if config.vlog_dir.exists() {
            let entries = std::fs::read_dir(&config.vlog_dir).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read vLog directory: {}",
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

                let file_name = entry.file_name();
                let name = file_name.to_string_lossy();

                // Parse vLog file names: vlog_NNNNNNNN.log
                if name.starts_with("vlog_") && name.ends_with(".log") {
                    if let Ok(number) = name[5..name.len() - 4].parse::<u64>() {
                        if number > max_file_id {
                            max_file_id = number;
                        }
                    }
                }
            }
        }

        Ok(max_file_id)
    }

    /// Generate vLog file path
    pub(crate) fn vlog_file_path(vlog_dir: &Path, file_id: u64) -> PathBuf {
        vlog_dir.join(format!("vlog_{:08}.log", file_id))
    }

    /// Flush buffered writes
    pub fn flush(&self) -> Result<()> {
        let mut writer = self.writer.write();
        writer.flush().map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to flush vLog: {}", e)))
        })?;

        writer.get_ref().sync_all().map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to sync vLog: {}", e)))
        })?;

        Ok(())
    }

    /// Get current file ID
    pub fn current_file_id(&self) -> u64 {
        *self.current_file_id.read()
    }

    /// Get configuration
    pub fn config(&self) -> &ValueLogConfig {
        &self.config
    }

    /// Get the timestamp (millis since UNIX epoch) of the last write operation
    pub fn last_write_time_millis(&self) -> u64 {
        self.last_write_time.load(Ordering::Acquire)
    }

    /// Get the duration since the last write operation
    pub fn time_since_last_write(&self) -> std::time::Duration {
        let last_millis = self.last_write_time_millis();
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let elapsed_millis = now_millis.saturating_sub(last_millis);
        std::time::Duration::from_millis(elapsed_millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_value_pointer_encode_decode() -> Result<()> {
        let pointer = ValuePointer::new(42, 1024, 256, 0xDEADBEEF);

        let bytes = pointer.encode();
        let decoded = ValuePointer::decode(&bytes)?;

        assert_eq!(decoded.file_id, 42);
        assert_eq!(decoded.offset, 1024);
        assert_eq!(decoded.length, 256);
        assert_eq!(decoded.checksum, 0xDEADBEEF);

        Ok(())
    }

    #[test]
    fn test_vlog_entry_encode_decode() -> Result<()> {
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);
        let entry = VLogEntry::new(key.clone(), value.clone());

        let bytes = entry.encode();
        let decoded = VLogEntry::decode(&bytes)?;

        assert_eq!(decoded.key, key);
        assert_eq!(decoded.value, value);
        assert_eq!(decoded.checksum, entry.checksum);

        Ok(())
    }

    #[test]
    fn test_value_log_basic_operations() -> Result<()> {
        let temp_dir = env::temp_dir().join("test_vlog_basic");
        std::fs::create_dir_all(&temp_dir).ok();

        let vlog = ValueLog::new(&temp_dir)?;

        let key = Key::from_str("key1");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        // Append value
        let pointer = vlog.append(key.clone(), value.clone())?;
        vlog.flush()?; // Flush to ensure data is on disk

        // Read value back
        let read_value = vlog.read(&pointer)?;

        assert_eq!(read_value, value);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_value_log_should_separate() -> Result<()> {
        let temp_dir = env::temp_dir().join("test_vlog_should_separate");
        std::fs::create_dir_all(&temp_dir).ok();

        let vlog = ValueLog::new(&temp_dir)?;

        // Small value (< 1KB)
        let small = CipherBlob::new(vec![0u8; 512]);
        assert!(!vlog.should_separate(&small));

        // Large value (> 1KB)
        let large = CipherBlob::new(vec![0u8; 2048]);
        assert!(vlog.should_separate(&large));

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_value_log_multiple_values() -> Result<()> {
        let temp_dir = env::temp_dir().join("test_vlog_multiple");
        std::fs::create_dir_all(&temp_dir).ok();

        let vlog = ValueLog::new(&temp_dir)?;

        let mut pointers = Vec::new();

        // Write multiple values
        for i in 0..10 {
            let key = Key::from_str(&format!("key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 1000]);
            let pointer = vlog.append(key, value)?;
            pointers.push((pointer, i as u8));
        }

        vlog.flush()?; // Flush to ensure all data is on disk

        // Read values back
        for (pointer, expected_byte) in pointers {
            let value = vlog.read(&pointer)?;
            assert_eq!(value.as_bytes()[0], expected_byte);
        }

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_value_log_rotation() -> Result<()> {
        let temp_dir = env::temp_dir().join("test_vlog_rotation");
        std::fs::create_dir_all(&temp_dir).ok();

        let config = ValueLogConfig {
            vlog_dir: temp_dir.clone(),
            max_file_size: 4096, // Small size to trigger rotation
            sync_on_write: false,
            ..Default::default()
        };

        let vlog = ValueLog::with_config(config)?;

        let initial_file_id = vlog.current_file_id();

        // Write enough data to trigger rotation
        for i in 0..10 {
            let key = Key::from_str(&format!("key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 1000]);
            vlog.append(key, value)?;
        }

        // File ID should have increased
        assert!(vlog.current_file_id() > initial_file_id);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_value_log_garbage_collection() -> Result<()> {
        let temp_dir = env::temp_dir().join("test_vlog_gc");
        std::fs::create_dir_all(&temp_dir).ok();

        let vlog = ValueLog::new(&temp_dir)?;

        // Write some values
        let mut keys = Vec::new();
        for i in 0..10 {
            let key = Key::from_str(&format!("key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 1000]);
            vlog.append(key.clone(), value)?;
            keys.push(key);
        }

        vlog.flush()?;

        let file_id = vlog.current_file_id();

        // Simulate: keys 0-4 are live, keys 5-9 are dead
        let is_live = |key: &Key| -> bool {
            let key_str = String::from_utf8_lossy(key.as_bytes());
            if let Some(num_str) = key_str.strip_prefix("key_") {
                if let Ok(num) = num_str.parse::<usize>() {
                    return num < 5;
                }
            }
            false
        };

        // Calculate garbage ratio
        let ratio = vlog.calculate_garbage_ratio(file_id, is_live)?;
        assert!(ratio > 0.4 && ratio < 0.6); // Should be around 50%

        // Perform GC
        let stats = vlog.garbage_collect_file(file_id, is_live)?;

        assert_eq!(stats.live_count, 5);
        assert_eq!(stats.dead_count, 5);
        assert!(stats.reclaimed_bytes > 0);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_value_log_large_values() -> Result<()> {
        let temp_dir = env::temp_dir().join("test_vlog_large");
        std::fs::create_dir_all(&temp_dir).ok();

        let vlog = ValueLog::new(&temp_dir)?;

        // Write a large value (10KB)
        let key = Key::from_str("large_key");
        let large_value = CipherBlob::new(vec![42u8; 10_000]);

        let pointer = vlog.append(key, large_value.clone())?;
        vlog.flush()?;

        // Read it back
        let read_value = vlog.read(&pointer)?;

        assert_eq!(read_value, large_value);
        assert_eq!(read_value.len(), 10_000);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }
}
