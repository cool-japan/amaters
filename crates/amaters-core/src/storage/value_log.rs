//! Value Log (vLog) implementation for WiscKey-style value separation
//!
//! The value log stores large values separately from the LSM-Tree to reduce
//! write amplification. Small values are stored inline, while large values
//! (>threshold) are stored in the vLog and referenced by pointers.
//!
//! Key features:
//! - Sequential append-only writes for high throughput
//! - File rotation when files reach size threshold
//! - Garbage collection to reclaim space from dead values
//! - Value pointers stored in LSM-Tree for indirection

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::{CipherBlob, Key};
use parking_lot::RwLock;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
struct VLogEntry {
    /// Key (for GC to identify ownership)
    key: Key,
    /// Value data
    value: CipherBlob,
    /// CRC32 checksum
    checksum: u32,
}

impl VLogEntry {
    fn new(key: Key, value: CipherBlob) -> Self {
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

    fn encode(&self) -> Vec<u8> {
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
    config: ValueLogConfig,
    /// Current vLog file number
    current_file_id: Arc<RwLock<u64>>,
    /// Current file writer
    writer: Arc<RwLock<BufWriter<File>>>,
    /// Current file offset
    current_offset: Arc<RwLock<u64>>,
    /// Current file size
    current_size: Arc<RwLock<u64>>,
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

        Ok(Self {
            config,
            current_file_id: Arc::new(RwLock::new(file_id)),
            writer: Arc::new(RwLock::new(BufWriter::new(file))),
            current_offset: Arc::new(RwLock::new(current_size)),
            current_size: Arc::new(RwLock::new(current_size)),
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
    fn rotate(&self) -> Result<()> {
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
            *writer = BufWriter::new(file);
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

        Ok(())
    }

    /// Find the latest vLog file number
    fn find_latest_vlog(config: &ValueLogConfig) -> Result<u64> {
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
    fn vlog_file_path(vlog_dir: &Path, file_id: u64) -> PathBuf {
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

    /// Perform garbage collection on a vLog file
    ///
    /// Scans the file and rewrites live values to a new file, discarding dead values.
    /// This is typically called when a file has too much garbage.
    ///
    /// `is_live_fn`: Function that checks if a key is still live in the LSM-Tree
    pub fn garbage_collect<F>(&self, file_id: u64, is_live_fn: F) -> Result<GcStats>
    where
        F: Fn(&Key) -> bool,
    {
        let file_path = Self::vlog_file_path(&self.config.vlog_dir, file_id);

        // Open old file for reading
        let old_file = File::open(&file_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to open vLog file for GC: {}",
                e
            )))
        })?;

        let file_size = old_file
            .metadata()
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to get file size: {}", e)))
            })?
            .len();

        let mut reader = BufReader::new(old_file);
        let mut offset = 0u64;

        let mut live_values = Vec::new();
        let mut dead_count = 0usize;
        let mut live_count = 0usize;

        // Scan file and identify live values
        while offset < file_size {
            // Read entry length (magic + key_len + key + value_len + value + checksum)
            let start_offset = offset;

            // Try to read entry
            let mut magic_bytes = [0u8; 4];
            match reader.read_exact(&mut magic_bytes) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // End of file
                    break;
                }
                Err(e) => {
                    return Err(AmateRSError::IoError(ErrorContext::new(format!(
                        "Failed to read magic: {}",
                        e
                    ))));
                }
            }

            // Verify magic
            let magic = u32::from_le_bytes(magic_bytes);
            if magic != 0x564C4F47 {
                // Corrupted entry, skip
                break;
            }

            // Read key length
            let mut key_len_bytes = [0u8; 4];
            reader.read_exact(&mut key_len_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read key length: {}",
                    e
                )))
            })?;
            let key_len = u32::from_le_bytes(key_len_bytes) as usize;

            // Read key
            let mut key_bytes = vec![0u8; key_len];
            reader.read_exact(&mut key_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read key: {}", e)))
            })?;
            let key = Key::from_slice(&key_bytes);

            // Read value length
            let mut value_len_bytes = [0u8; 4];
            reader.read_exact(&mut value_len_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read value length: {}",
                    e
                )))
            })?;
            let value_len = u32::from_le_bytes(value_len_bytes) as usize;

            // Read value
            let mut value_bytes = vec![0u8; value_len];
            reader.read_exact(&mut value_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read value: {}", e)))
            })?;
            let value = CipherBlob::new(value_bytes);

            // Read checksum
            let mut checksum_bytes = [0u8; 4];
            reader.read_exact(&mut checksum_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read checksum: {}", e)))
            })?;

            // Calculate entry size
            let entry_size = 4 + 4 + key_len + 4 + value_len + 4;
            offset += entry_size as u64;

            // Check if value is live
            if is_live_fn(&key) {
                live_values.push((key, value));
                live_count += 1;
            } else {
                dead_count += 1;
            }
        }

        // Rewrite live values to new file
        let new_file_id = Self::find_latest_vlog(&self.config)? + 1;
        let new_file_path = Self::vlog_file_path(&self.config.vlog_dir, new_file_id);

        let new_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&new_file_path)
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to create new vLog file: {}",
                    e
                )))
            })?;

        let mut new_writer = BufWriter::new(new_file);

        for (key, value) in live_values {
            let entry = VLogEntry::new(key, value);
            let entry_bytes = entry.encode();
            new_writer.write_all(&entry_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to write GC entry: {}",
                    e
                )))
            })?;
        }

        new_writer.flush().map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to flush GC file: {}", e)))
        })?;

        // Delete old file
        std::fs::remove_file(&file_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to delete old vLog file: {}",
                e
            )))
        })?;

        Ok(GcStats {
            file_id,
            live_count,
            dead_count,
            reclaimed_bytes: file_size
                - new_writer
                    .get_ref()
                    .metadata()
                    .map_err(|e| {
                        AmateRSError::IoError(ErrorContext::new(format!(
                            "Failed to get new file size: {}",
                            e
                        )))
                    })?
                    .len(),
        })
    }

    /// Calculate garbage ratio for a vLog file
    ///
    /// Returns the ratio of dead values to total values.
    /// This can be used to determine if GC is needed.
    pub fn calculate_garbage_ratio<F>(&self, file_id: u64, is_live_fn: F) -> Result<f64>
    where
        F: Fn(&Key) -> bool,
    {
        let file_path = Self::vlog_file_path(&self.config.vlog_dir, file_id);

        let file = File::open(&file_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to open vLog file: {}",
                e
            )))
        })?;

        let file_size = file
            .metadata()
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to get file size: {}", e)))
            })?
            .len();

        let mut reader = BufReader::new(file);
        let mut offset = 0u64;

        let mut live_bytes = 0u64;
        let mut dead_bytes = 0u64;

        while offset < file_size {
            let start_offset = offset;

            // Try to read entry
            let mut magic_bytes = [0u8; 4];
            match reader.read_exact(&mut magic_bytes) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    return Err(AmateRSError::IoError(ErrorContext::new(format!(
                        "Failed to read magic: {}",
                        e
                    ))));
                }
            }

            let magic = u32::from_le_bytes(magic_bytes);
            if magic != 0x564C4F47 {
                break;
            }

            // Read key length
            let mut key_len_bytes = [0u8; 4];
            reader.read_exact(&mut key_len_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read key length: {}",
                    e
                )))
            })?;
            let key_len = u32::from_le_bytes(key_len_bytes) as usize;

            // Read key
            let mut key_bytes = vec![0u8; key_len];
            reader.read_exact(&mut key_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read key: {}", e)))
            })?;
            let key = Key::from_slice(&key_bytes);

            // Read value length
            let mut value_len_bytes = [0u8; 4];
            reader.read_exact(&mut value_len_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read value length: {}",
                    e
                )))
            })?;
            let value_len = u32::from_le_bytes(value_len_bytes) as usize;

            // Skip value
            let mut value_bytes = vec![0u8; value_len];
            reader.read_exact(&mut value_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read value: {}", e)))
            })?;

            // Skip checksum
            let mut checksum_bytes = [0u8; 4];
            reader.read_exact(&mut checksum_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read checksum: {}", e)))
            })?;

            let entry_size = 4 + 4 + key_len + 4 + value_len + 4;
            offset += entry_size as u64;

            if is_live_fn(&key) {
                live_bytes += entry_size as u64;
            } else {
                dead_bytes += entry_size as u64;
            }
        }

        let total_bytes = live_bytes + dead_bytes;
        if total_bytes == 0 {
            Ok(0.0)
        } else {
            Ok(dead_bytes as f64 / total_bytes as f64)
        }
    }
}

/// Garbage collection statistics
#[derive(Debug, Clone)]
pub struct GcStats {
    /// File ID that was garbage collected
    pub file_id: u64,
    /// Number of live values kept
    pub live_count: usize,
    /// Number of dead values removed
    pub dead_count: usize,
    /// Bytes reclaimed
    pub reclaimed_bytes: u64,
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
        let stats = vlog.garbage_collect(file_id, is_live)?;

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
