//! Write-Ahead Log (WAL) implementation
//!
//! The WAL provides durability by logging all writes before they're applied to the memtable.
//! In case of crash, the WAL can be replayed to recover the memtable state.

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::{CipherBlob, Key};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// Statistics from WAL recovery
#[derive(Debug, Clone, Default)]
pub struct RecoveryStats {
    /// Number of entries successfully recovered
    pub entries_recovered: u64,
    /// Number of corrupted entries encountered
    pub entries_corrupted: u64,
    /// Total bytes recovered
    pub bytes_recovered: u64,
}

/// WAL entry type
#[derive(Debug, Clone, PartialEq)]
pub enum WalEntryType {
    Put = 1,
    Delete = 2,
}

/// WAL entry
#[derive(Debug, Clone, PartialEq)]
pub struct WalEntry {
    /// Sequence number for ordering
    pub sequence: u64,
    /// Entry type
    pub entry_type: WalEntryType,
    /// Key
    pub key: Key,
    /// Value (None for deletes)
    pub value: Option<CipherBlob>,
    /// CRC32 checksum for integrity
    pub checksum: u32,
}

impl WalEntry {
    /// Create a Put entry
    pub fn put(sequence: u64, key: Key, value: CipherBlob) -> Self {
        let mut entry = Self {
            sequence,
            entry_type: WalEntryType::Put,
            key,
            value: Some(value),
            checksum: 0,
        };
        entry.checksum = entry.calculate_checksum();
        entry
    }

    /// Create a Delete entry
    pub fn delete(sequence: u64, key: Key) -> Self {
        let mut entry = Self {
            sequence,
            entry_type: WalEntryType::Delete,
            key,
            value: None,
            checksum: 0,
        };
        entry.checksum = entry.calculate_checksum();
        entry
    }

    /// Calculate checksum for the entry
    fn calculate_checksum(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();

        // Hash sequence
        hasher.update(&self.sequence.to_le_bytes());

        // Hash entry type
        hasher.update(&[self.entry_type.clone() as u8]);

        // Hash key
        hasher.update(self.key.as_bytes());

        // Hash value if present
        if let Some(ref value) = self.value {
            hasher.update(value.as_bytes());
        }

        hasher.finalize()
    }

    /// Verify checksum
    pub fn verify_checksum(&self) -> Result<()> {
        let calculated = self.calculate_checksum();
        if calculated == self.checksum {
            Ok(())
        } else {
            Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "WAL entry checksum mismatch: expected {}, got {}",
                self.checksum, calculated
            ))))
        }
    }

    /// Encode entry to bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Magic number (0x57414C = "WAL" in hex)
        bytes.extend_from_slice(&0x57414Cu32.to_le_bytes());

        // Sequence
        bytes.extend_from_slice(&self.sequence.to_le_bytes());

        // Entry type
        bytes.push(self.entry_type.clone() as u8);

        // Key length and data
        bytes.extend_from_slice(&(self.key.len() as u32).to_le_bytes());
        bytes.extend_from_slice(self.key.as_bytes());

        // Value length and data
        if let Some(ref value) = self.value {
            bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        } else {
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }

        // Checksum
        bytes.extend_from_slice(&self.checksum.to_le_bytes());

        bytes
    }

    /// Decode entry from bytes
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 17 {
            // Magic (4) + Sequence (8) + Type (1) + KeyLen (4)
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "WAL entry too short",
            )));
        }

        let mut offset = 0;

        // Verify magic number
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if magic != 0x57414C {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                "Invalid WAL entry magic number",
            )));
        }
        offset += 4;

        // Sequence
        let sequence = u64::from_le_bytes(bytes[offset..offset + 8].try_into().map_err(|_| {
            AmateRSError::SerializationError(ErrorContext::new("Failed to read sequence"))
        })?);
        offset += 8;

        // Entry type
        let entry_type = match bytes[offset] {
            1 => WalEntryType::Put,
            2 => WalEntryType::Delete,
            _ => {
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Invalid WAL entry type",
                )));
            }
        };
        offset += 1;

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

        let value = if value_len > 0 {
            let value_bytes = &bytes[offset..offset + value_len];
            Some(CipherBlob::new(value_bytes.to_vec()))
        } else {
            None
        };
        offset += value_len;

        // Checksum
        let checksum = u32::from_le_bytes(bytes[offset..offset + 4].try_into().map_err(|_| {
            AmateRSError::SerializationError(ErrorContext::new("Failed to read checksum"))
        })?);

        let entry = Self {
            sequence,
            entry_type,
            key,
            value,
            checksum,
        };

        // Verify checksum
        entry.verify_checksum()?;

        Ok(entry)
    }
}

/// WAL configuration
#[derive(Debug, Clone)]
pub struct WalConfig {
    /// Directory for WAL files
    pub wal_dir: PathBuf,
    /// Maximum WAL file size before rotation (default: 64MB)
    pub max_file_size: u64,
    /// Maximum number of WAL files to keep (default: 10)
    pub max_wal_files: usize,
    /// Whether to sync after each write (default: true for durability)
    pub sync_on_write: bool,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            wal_dir: PathBuf::from("./wal"),
            max_file_size: 64 * 1024 * 1024, // 64MB
            max_wal_files: 10,
            sync_on_write: true,
        }
    }
}

/// Write-Ahead Log
pub struct Wal {
    /// Configuration
    config: WalConfig,
    /// Current WAL file path
    current_path: PathBuf,
    /// Writer for current WAL file
    writer: BufWriter<File>,
    /// Global sequence number across all WAL files
    sequence: u64,
    /// Current file size in bytes
    current_file_size: u64,
    /// Current WAL file number
    current_file_number: u64,
}

impl Wal {
    /// Create or open a WAL file (simple API for backward compatibility)
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let parent = path.parent().ok_or_else(|| {
            AmateRSError::IoError(ErrorContext::new("WAL path has no parent directory"))
        })?;

        let config = WalConfig {
            wal_dir: parent.to_path_buf(),
            ..Default::default()
        };

        Self::with_config(config)
    }

    /// Create a new WAL with custom configuration
    pub fn with_config(config: WalConfig) -> Result<Self> {
        // Create WAL directory if it doesn't exist
        std::fs::create_dir_all(&config.wal_dir).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to create WAL directory: {}",
                e
            )))
        })?;

        // Find the latest WAL file or create a new one
        let (file_number, sequence) = Self::find_latest_wal(&config)?;

        let current_path = Self::wal_file_path(&config.wal_dir, file_number);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&current_path)
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to open WAL: {}", e)))
            })?;

        let current_file_size = file
            .metadata()
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to get WAL file size: {}",
                    e
                )))
            })?
            .len();

        Ok(Self {
            config,
            current_path,
            writer: BufWriter::new(file),
            sequence,
            current_file_size,
            current_file_number: file_number,
        })
    }

    /// Find the latest WAL file and sequence number
    fn find_latest_wal(config: &WalConfig) -> Result<(u64, u64)> {
        let mut max_file_number = 0u64;
        let mut max_sequence = 0u64;

        if config.wal_dir.exists() {
            let wal_file_numbers = Self::list_wal_file_numbers(&config.wal_dir)?;

            if let Some(&last) = wal_file_numbers.last() {
                max_file_number = last;
            }

            // Scan all WAL files to recover the max sequence number
            for file_num in &wal_file_numbers {
                let file_path = Self::wal_file_path(&config.wal_dir, *file_num);
                if let Ok(mut reader) = WalReader::open(&file_path) {
                    loop {
                        match reader.read_entry() {
                            Ok(Some(entry)) => {
                                if entry.sequence >= max_sequence {
                                    max_sequence = entry.sequence + 1;
                                }
                            }
                            Ok(None) => break,
                            Err(_) => {
                                tracing::warn!(
                                    "Corrupted entry found in WAL file {} during startup",
                                    file_path.display()
                                );
                                continue;
                            }
                        }
                    }
                }
            }
        }

        Ok((max_file_number, max_sequence))
    }

    /// Generate WAL file path for a given file number
    fn wal_file_path(wal_dir: &Path, file_number: u64) -> PathBuf {
        wal_dir.join(format!("wal_{:08}.log", file_number))
    }

    /// List all WAL file numbers in the directory, sorted ascending
    fn list_wal_file_numbers(wal_dir: &Path) -> Result<Vec<u64>> {
        let entries = std::fs::read_dir(wal_dir).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to read WAL directory: {}",
                e
            )))
        })?;

        let mut numbers = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read directory entry: {}",
                    e
                )))
            })?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if name.starts_with("wal_") && name.ends_with(".log") {
                if let Ok(number) = name[4..name.len() - 4].parse::<u64>() {
                    numbers.push(number);
                }
            }
        }
        numbers.sort_unstable();
        Ok(numbers)
    }

    /// Append a Put entry
    pub fn put(&mut self, key: Key, value: CipherBlob) -> Result<u64> {
        let sequence = self.sequence;
        self.sequence += 1;

        let entry = WalEntry::put(sequence, key, value);
        self.write_entry(&entry)?;

        Ok(sequence)
    }

    /// Append a Delete entry
    pub fn delete(&mut self, key: Key) -> Result<u64> {
        let sequence = self.sequence;
        self.sequence += 1;

        let entry = WalEntry::delete(sequence, key);
        self.write_entry(&entry)?;

        Ok(sequence)
    }

    /// Write an entry to the log
    fn write_entry(&mut self, entry: &WalEntry) -> Result<()> {
        let bytes = entry.encode();

        // Write length prefix
        let len = bytes.len() as u32;
        self.writer.write_all(&len.to_le_bytes()).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to write WAL entry: {}",
                e
            )))
        })?;

        // Write entry
        self.writer.write_all(&bytes).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to write WAL entry: {}",
                e
            )))
        })?;

        // Update file size
        let entry_size = (4 + bytes.len()) as u64; // 4 bytes for length prefix
        self.current_file_size += entry_size;

        // Optional: sync after each write for durability
        if self.config.sync_on_write {
            self.writer.flush().map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to flush WAL: {}", e)))
            })?;
        }

        // Check if rotation is needed
        if self.current_file_size >= self.config.max_file_size {
            self.rotate()?;
        }

        Ok(())
    }

    /// Rotate to a new WAL file
    pub fn rotate(&mut self) -> Result<()> {
        // Flush current file
        self.flush()?;

        // Increment file number
        self.current_file_number += 1;

        // Create new WAL file
        let new_path = Self::wal_file_path(&self.config.wal_dir, self.current_file_number);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&new_path)
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to create new WAL file: {}",
                    e
                )))
            })?;

        self.current_path = new_path;
        self.writer = BufWriter::new(file);
        self.current_file_size = 0;

        // Clean up old WAL files
        self.cleanup_old_wal_files()?;

        Ok(())
    }

    /// Clean up old WAL files beyond the retention limit
    fn cleanup_old_wal_files(&self) -> Result<()> {
        let wal_files = Self::list_wal_file_numbers(&self.config.wal_dir)?;

        if wal_files.len() > self.config.max_wal_files {
            let files_to_delete = wal_files.len() - self.config.max_wal_files;

            for &file_number in wal_files.iter().take(files_to_delete) {
                let file_path = Self::wal_file_path(&self.config.wal_dir, file_number);
                std::fs::remove_file(&file_path).map_err(|e| {
                    AmateRSError::IoError(ErrorContext::new(format!(
                        "Failed to delete old WAL file: {}",
                        e
                    )))
                })?;
            }
        }

        Ok(())
    }

    /// Manually trigger cleanup of old WAL files
    pub fn cleanup(&self) -> Result<()> {
        self.cleanup_old_wal_files()
    }

    /// Get current WAL file size
    pub fn current_file_size(&self) -> u64 {
        self.current_file_size
    }

    /// Get current WAL file number
    pub fn current_file_number(&self) -> u64 {
        self.current_file_number
    }

    /// Flush buffered writes to disk
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush().map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to flush WAL: {}", e)))
        })?;

        self.writer.get_ref().sync_all().map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to sync WAL: {}", e)))
        })?;

        Ok(())
    }

    /// Get current sequence number
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Get WAL file path
    pub fn path(&self) -> &Path {
        &self.current_path
    }

    /// Recover all entries from WAL files in a directory
    ///
    /// Reads all WAL files in sequence order and returns recovered entries.
    /// Handles corrupted and incomplete entries gracefully by skipping them.
    ///
    /// Returns (entries, max_sequence) where max_sequence is the highest
    /// sequence number found during recovery.
    pub fn recover(wal_dir: impl AsRef<Path>) -> Result<(Vec<WalEntry>, u64)> {
        let wal_dir = wal_dir.as_ref();

        if !wal_dir.exists() {
            return Ok((Vec::new(), 0));
        }

        let wal_files = Self::list_wal_file_numbers(wal_dir)?;

        let mut all_entries = Vec::new();
        let mut max_sequence = 0u64;

        for file_number in wal_files {
            let file_path = Self::wal_file_path(wal_dir, file_number);
            let mut reader = WalReader::open(&file_path)?;

            loop {
                match reader.read_entry() {
                    Ok(Some(entry)) => {
                        if entry.sequence > max_sequence {
                            max_sequence = entry.sequence;
                        }
                        all_entries.push(entry);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!(
                            "Skipping corrupted entry in {}: {}",
                            file_path.display(),
                            e
                        );
                        continue;
                    }
                }
            }
        }

        Ok((all_entries, max_sequence))
    }

    /// Get current active WAL file size in bytes
    pub fn current_size(&self) -> u64 {
        self.current_file_size
    }

    /// Get total size of all WAL files in the WAL directory
    pub fn total_wal_size(&self) -> Result<u64> {
        let wal_files = Self::list_wal_file_numbers(&self.config.wal_dir)?;
        let mut total_size = 0u64;

        for file_number in wal_files {
            let file_path = Self::wal_file_path(&self.config.wal_dir, file_number);
            let metadata = std::fs::metadata(&file_path).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read WAL file metadata: {}",
                    e
                )))
            })?;
            total_size += metadata.len();
        }

        Ok(total_size)
    }

    /// Truncate WAL files whose max sequence number is <= the given sequence.
    ///
    /// This is used after a memtable flush to remove WAL files that are no longer needed.
    /// The current active WAL file is never removed.
    ///
    /// Returns the number of files truncated (removed).
    pub fn truncate_before(&mut self, sequence: u64) -> Result<u64> {
        self.flush()?;

        let all_files = Self::list_wal_file_numbers(&self.config.wal_dir)?;
        // Exclude the current active file
        let wal_files: Vec<u64> = all_files
            .into_iter()
            .filter(|&n| n != self.current_file_number)
            .collect();

        let mut files_truncated = 0u64;

        for file_number in wal_files {
            let file_path = Self::wal_file_path(&self.config.wal_dir, file_number);

            // Read the file to find its max sequence
            let mut file_max_seq = 0u64;
            if let Ok(mut reader) = WalReader::open(&file_path) {
                loop {
                    match reader.read_entry() {
                        Ok(Some(entry)) => {
                            if entry.sequence > file_max_seq {
                                file_max_seq = entry.sequence;
                            }
                        }
                        Ok(None) => break,
                        Err(_) => continue,
                    }
                }
            }

            // If all entries in this file are <= the given sequence, remove it
            if file_max_seq <= sequence {
                std::fs::remove_file(&file_path).map_err(|e| {
                    AmateRSError::IoError(ErrorContext::new(format!(
                        "Failed to remove WAL file {}: {}",
                        file_path.display(),
                        e
                    )))
                })?;
                files_truncated += 1;
            }
        }

        Ok(files_truncated)
    }

    /// Recover all entries from WAL files with detailed statistics
    ///
    /// Like `recover()`, but also returns `RecoveryStats` with counts of
    /// recovered entries, corrupted entries, and total bytes recovered.
    pub fn recover_with_stats(
        wal_dir: impl AsRef<Path>,
    ) -> Result<(Vec<WalEntry>, u64, RecoveryStats)> {
        let wal_dir = wal_dir.as_ref();
        let mut stats = RecoveryStats::default();

        if !wal_dir.exists() {
            return Ok((Vec::new(), 0, stats));
        }

        let wal_files = Self::list_wal_file_numbers(wal_dir)?;

        let mut all_entries = Vec::new();
        let mut max_sequence = 0u64;

        for file_number in wal_files {
            let file_path = Self::wal_file_path(wal_dir, file_number);
            let mut reader = WalReader::open(&file_path)?;

            loop {
                match reader.read_entry() {
                    Ok(Some(entry)) => {
                        let entry_bytes = entry.encode().len() as u64 + 4; // +4 for length prefix
                        stats.bytes_recovered += entry_bytes;
                        stats.entries_recovered += 1;
                        if entry.sequence > max_sequence {
                            max_sequence = entry.sequence;
                        }
                        all_entries.push(entry);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        stats.entries_corrupted += 1;
                        tracing::warn!(
                            "Skipping corrupted entry in {}: {}",
                            file_path.display(),
                            e
                        );
                        continue;
                    }
                }
            }
        }

        Ok((all_entries, max_sequence, stats))
    }

    /// Replay WAL entries to a memtable
    ///
    /// Applies all entries from the WAL directory to the provided memtable.
    /// This is used during crash recovery to rebuild memtable state.
    ///
    /// Returns the maximum sequence number found during replay.
    pub fn replay_to_memtable(
        wal_dir: impl AsRef<Path>,
        memtable: &crate::storage::memtable::Memtable,
    ) -> Result<u64> {
        let (entries, max_sequence) = Self::recover(wal_dir)?;

        for entry in entries {
            match entry.entry_type {
                WalEntryType::Put => {
                    if let Some(value) = entry.value {
                        memtable.put(entry.key, value)?;
                    }
                }
                WalEntryType::Delete => {
                    memtable.delete(entry.key)?;
                }
            }
        }

        Ok(max_sequence)
    }
}

/// WAL reader for reading entries from a WAL file
pub struct WalReader {
    reader: BufReader<File>,
}

impl WalReader {
    /// Open a WAL file for reading
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path.as_ref()).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to open WAL file: {}", e)))
        })?;

        Ok(Self {
            reader: BufReader::new(file),
        })
    }

    /// Read the next entry from the WAL file
    ///
    /// Returns:
    /// - Ok(Some(entry)) if an entry was successfully read
    /// - Ok(None) if end of file reached
    /// - Err if a corrupted or incomplete entry is encountered
    pub fn read_entry(&mut self) -> Result<Option<WalEntry>> {
        // Read length prefix (4 bytes)
        let mut len_bytes = [0u8; 4];
        match self.reader.read_exact(&mut len_bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // End of file or incomplete length prefix
                return Ok(None);
            }
            Err(e) => {
                return Err(AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read WAL entry length: {}",
                    e
                ))));
            }
        }

        let len = u32::from_le_bytes(len_bytes) as usize;

        // Sanity check: reject unreasonably large entries (>100MB)
        if len > 100 * 1024 * 1024 {
            return Err(AmateRSError::SerializationError(ErrorContext::new(
                format!("WAL entry too large: {} bytes", len),
            )));
        }

        // Read entry bytes
        let mut entry_bytes = vec![0u8; len];
        match self.reader.read_exact(&mut entry_bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // Incomplete entry (crash during write)
                return Err(AmateRSError::SerializationError(ErrorContext::new(
                    "Incomplete WAL entry (truncated file)",
                )));
            }
            Err(e) => {
                return Err(AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read WAL entry: {}",
                    e
                ))));
            }
        }

        // Decode entry (this includes checksum verification)
        let entry = WalEntry::decode(&entry_bytes)?;

        Ok(Some(entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Memtable;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_wal_entry_encode_decode() -> Result<()> {
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);
        let entry = WalEntry::put(42, key.clone(), value.clone());

        let bytes = entry.encode();
        let decoded = WalEntry::decode(&bytes)?;

        assert_eq!(decoded.sequence, 42);
        assert_eq!(decoded.entry_type, WalEntryType::Put);
        assert_eq!(decoded.key, key);
        assert_eq!(decoded.value, Some(value));

        Ok(())
    }

    #[test]
    fn test_wal_delete_entry() -> Result<()> {
        let key = Key::from_str("delete_me");
        let entry = WalEntry::delete(99, key.clone());

        let bytes = entry.encode();
        let decoded = WalEntry::decode(&bytes)?;

        assert_eq!(decoded.sequence, 99);
        assert_eq!(decoded.entry_type, WalEntryType::Delete);
        assert_eq!(decoded.key, key);
        assert_eq!(decoded.value, None);

        Ok(())
    }

    #[test]
    fn test_wal_checksum_verification() -> Result<()> {
        let key = Key::from_str("test");
        let value = CipherBlob::new(vec![1, 2, 3]);
        let entry = WalEntry::put(1, key, value);

        // Verify should pass
        entry.verify_checksum()?;

        // Corrupt checksum
        let mut corrupted = entry.clone();
        corrupted.checksum = 0;

        // Verify should fail
        assert!(corrupted.verify_checksum().is_err());

        Ok(())
    }

    #[test]
    fn test_wal_basic_operations() -> Result<()> {
        let temp_dir = tempdir().map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to create temp dir: {}",
                e
            )))
        })?;
        let wal_path = temp_dir.path().join("test.wal");

        let mut wal = Wal::create(&wal_path)?;

        // Write some entries
        let seq1 = wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
        let seq2 = wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
        let seq3 = wal.delete(Key::from_str("key1"))?;

        assert_eq!(seq1, 0);
        assert_eq!(seq2, 1);
        assert_eq!(seq3, 2);

        wal.flush()?;

        // Verify a WAL file was created (may be rotated, so check path() returns something that exists)
        assert!(wal.path().exists());

        Ok(())
    }

    #[test]
    fn test_wal_sequence_increment() -> Result<()> {
        let temp_dir = tempdir().map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to create temp dir: {}",
                e
            )))
        })?;
        let wal_path = temp_dir.path().join("test_seq.wal");

        let mut wal = Wal::create(&wal_path)?;

        assert_eq!(wal.sequence(), 0);

        wal.put(Key::from_str("key"), CipherBlob::new(vec![1]))?;
        assert_eq!(wal.sequence(), 1);

        wal.delete(Key::from_str("key"))?;
        assert_eq!(wal.sequence(), 2);

        Ok(())
    }

    #[test]
    fn test_wal_entry_large_value() -> Result<()> {
        let key = Key::from_str("large");
        let large_value = CipherBlob::new(vec![0u8; 10_000]);
        let entry = WalEntry::put(1, key.clone(), large_value.clone());

        let bytes = entry.encode();
        let decoded = WalEntry::decode(&bytes)?;

        assert_eq!(decoded.key, key);
        assert_eq!(decoded.value, Some(large_value));

        Ok(())
    }

    #[test]
    fn test_wal_rotation() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_rotation");
        std::fs::create_dir_all(&temp_dir).ok();

        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            max_file_size: 1024,  // Small size to trigger rotation
            sync_on_write: false, // Disable for speed
            ..Default::default()
        };

        let mut wal = Wal::with_config(config)?;

        let initial_file_number = wal.current_file_number();

        // Write enough data to trigger rotation
        for i in 0..20 {
            wal.put(
                Key::from_str(&format!("key_{}", i)),
                CipherBlob::new(vec![i as u8; 100]),
            )?;
        }

        // File number should have increased due to rotation
        assert!(wal.current_file_number() > initial_file_number);

        // Verify new file exists
        assert!(wal.path().exists());

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_cleanup() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_cleanup");
        std::fs::create_dir_all(&temp_dir).ok();

        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            max_file_size: 512, // Very small to trigger many rotations
            max_wal_files: 3,   // Keep only 3 files
            sync_on_write: false,
        };

        let mut wal = Wal::with_config(config)?;

        // Write enough data to create many WAL files
        for i in 0..100 {
            wal.put(
                Key::from_str(&format!("key_{}", i)),
                CipherBlob::new(vec![i as u8; 100]),
            )?;
        }

        // Count WAL files
        let wal_file_count = std::fs::read_dir(&temp_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("wal_")
                    && e.file_name().to_string_lossy().ends_with(".log")
            })
            .count();

        // Should have at most max_wal_files
        assert!(wal_file_count <= 3);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_manual_cleanup() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_manual_cleanup");
        std::fs::create_dir_all(&temp_dir).ok();

        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            max_file_size: 512,
            max_wal_files: 5,
            sync_on_write: false,
        };

        let mut wal = Wal::with_config(config)?;

        // Create several WAL files
        for i in 0..80 {
            wal.put(
                Key::from_str(&format!("key_{}", i)),
                CipherBlob::new(vec![i as u8; 100]),
            )?;
        }

        // Manually trigger cleanup
        wal.cleanup()?;

        // Count files
        let wal_file_count = std::fs::read_dir(&temp_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("wal_")
                    && e.file_name().to_string_lossy().ends_with(".log")
            })
            .count();

        assert!(wal_file_count <= 5);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_recovery_basic() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_recovery_basic");
        std::fs::create_dir_all(&temp_dir).ok();

        // Write some entries
        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                sync_on_write: true,
                ..Default::default()
            };

            let mut wal = Wal::with_config(config)?;

            wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
            wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
            wal.delete(Key::from_str("key1"))?;
            wal.put(Key::from_str("key3"), CipherBlob::new(vec![7, 8, 9]))?;

            wal.flush()?;
        }

        // Recover entries
        let (entries, max_sequence) = Wal::recover(&temp_dir)?;

        assert_eq!(entries.len(), 4);
        assert_eq!(max_sequence, 3);

        // Verify entries
        assert_eq!(entries[0].key, Key::from_str("key1"));
        assert_eq!(entries[0].entry_type, WalEntryType::Put);
        assert_eq!(entries[0].value, Some(CipherBlob::new(vec![1, 2, 3])));

        assert_eq!(entries[1].key, Key::from_str("key2"));
        assert_eq!(entries[1].entry_type, WalEntryType::Put);

        assert_eq!(entries[2].key, Key::from_str("key1"));
        assert_eq!(entries[2].entry_type, WalEntryType::Delete);
        assert_eq!(entries[2].value, None);

        assert_eq!(entries[3].key, Key::from_str("key3"));
        assert_eq!(entries[3].entry_type, WalEntryType::Put);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_recovery_multiple_files() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_recovery_multiple");
        std::fs::create_dir_all(&temp_dir).ok();

        // Write entries across multiple WAL files
        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                max_file_size: 512, // Small to trigger rotation
                sync_on_write: true,
                ..Default::default()
            };

            let mut wal = Wal::with_config(config)?;

            // Write enough data to create multiple files
            for i in 0..20 {
                wal.put(
                    Key::from_str(&format!("key_{}", i)),
                    CipherBlob::new(vec![i as u8; 100]),
                )?;
            }

            wal.flush()?;
        }

        // Recover all entries
        let (entries, max_sequence) = Wal::recover(&temp_dir)?;

        assert_eq!(entries.len(), 20);
        assert_eq!(max_sequence, 19);

        // Verify entries are in sequence order
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.sequence, i as u64);
            assert_eq!(entry.key, Key::from_str(&format!("key_{}", i)));
        }

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_recovery_empty_directory() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_recovery_empty");
        std::fs::create_dir_all(&temp_dir).ok();

        // Recover from empty directory
        let (entries, max_sequence) = Wal::recover(&temp_dir)?;

        assert_eq!(entries.len(), 0);
        assert_eq!(max_sequence, 0);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_recovery_nonexistent_directory() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("nonexistent_wal_dir_12345");

        // Recover from non-existent directory
        let (entries, max_sequence) = Wal::recover(&temp_dir)?;

        assert_eq!(entries.len(), 0);
        assert_eq!(max_sequence, 0);

        Ok(())
    }

    #[test]
    fn test_wal_replay_to_memtable() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_replay_memtable");
        std::fs::create_dir_all(&temp_dir).ok();

        // Write some entries
        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                sync_on_write: true,
                ..Default::default()
            };

            let mut wal = Wal::with_config(config)?;

            wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
            wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
            wal.delete(Key::from_str("key1"))?;
            wal.put(Key::from_str("key3"), CipherBlob::new(vec![7, 8, 9]))?;

            wal.flush()?;
        }

        // Create a new memtable and replay WAL
        let memtable = Memtable::new();
        let max_sequence = Wal::replay_to_memtable(&temp_dir, &memtable)?;

        assert_eq!(max_sequence, 3);

        // Verify memtable state
        assert_eq!(memtable.get(&Key::from_str("key1"))?, None); // Deleted
        assert_eq!(
            memtable.get(&Key::from_str("key2"))?,
            Some(CipherBlob::new(vec![4, 5, 6]))
        );
        assert_eq!(
            memtable.get(&Key::from_str("key3"))?,
            Some(CipherBlob::new(vec![7, 8, 9]))
        );

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_reader_basic() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_reader_basic");
        std::fs::create_dir_all(&temp_dir).ok();

        let wal_file = temp_dir.join("test.wal");

        // Write some entries
        {
            let mut wal = Wal::create(&wal_file)?;
            wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
            wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
            wal.flush()?;
        }

        // Read entries with WalReader
        let wal_file_actual = temp_dir.join("wal_00000000.log");
        let mut reader = WalReader::open(&wal_file_actual)?;

        let entry1 = reader.read_entry()?.expect("Should have entry 1");
        assert_eq!(entry1.sequence, 0);
        assert_eq!(entry1.key, Key::from_str("key1"));

        let entry2 = reader.read_entry()?.expect("Should have entry 2");
        assert_eq!(entry2.sequence, 1);
        assert_eq!(entry2.key, Key::from_str("key2"));

        let entry3 = reader.read_entry()?;
        assert_eq!(entry3, None); // End of file

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_recovery_with_truncated_file() -> Result<()> {
        use std::env;
        use std::io::Write as IoWrite;

        let temp_dir = env::temp_dir().join("test_wal_recovery_truncated");
        std::fs::create_dir_all(&temp_dir).ok();

        // Write some valid entries, then truncate the last one
        let wal_file = temp_dir.join("wal_00000000.log");
        {
            let mut wal = Wal::create(&wal_file)?;
            wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
            wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
            wal.flush()?;

            // Append incomplete entry (length prefix only, no data)
            let mut file = OpenOptions::new().append(true).open(&wal_file)?;
            let incomplete_len = 1234u32;
            file.write_all(&incomplete_len.to_le_bytes())?;
            file.flush()?;
        }

        // Recovery should handle truncated entry gracefully
        let (entries, _) = Wal::recover(&temp_dir)?;

        // Should recover the 2 complete entries, skip the truncated one
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, Key::from_str("key1"));
        assert_eq!(entries[1].key, Key::from_str("key2"));

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_sequence_recovery_after_crash() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_seq_recovery_crash");
        // Clean up from any previous run
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        // Phase 1: Write entries and then drop (simulate crash)
        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                sync_on_write: true,
                ..Default::default()
            };

            let mut wal = Wal::with_config(config)?;

            wal.put(Key::from_str("a"), CipherBlob::new(vec![1]))?;
            wal.put(Key::from_str("b"), CipherBlob::new(vec![2]))?;
            wal.put(Key::from_str("c"), CipherBlob::new(vec![3]))?;
            wal.put(Key::from_str("d"), CipherBlob::new(vec![4]))?;
            wal.put(Key::from_str("e"), CipherBlob::new(vec![5]))?;
            wal.flush()?;
            // sequences 0..4 written, next should be 5
        }

        // Phase 2: Open a new WAL instance - should recover sequence
        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                sync_on_write: true,
                ..Default::default()
            };

            let mut wal = Wal::with_config(config)?;

            // Sequence should continue from 5 (max was 4, so next is 5)
            assert_eq!(wal.sequence(), 5);

            // Write more entries
            let seq = wal.put(Key::from_str("f"), CipherBlob::new(vec![6]))?;
            assert_eq!(seq, 5);

            let seq = wal.put(Key::from_str("g"), CipherBlob::new(vec![7]))?;
            assert_eq!(seq, 6);

            wal.flush()?;
        }

        // Phase 3: Verify all entries are recoverable
        let (entries, max_sequence) = Wal::recover(&temp_dir)?;
        assert_eq!(entries.len(), 7);
        assert_eq!(max_sequence, 6);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_corruption_detection_and_partial_recovery() -> Result<()> {
        use std::env;
        use std::io::Write as IoWrite;

        let temp_dir = env::temp_dir().join("test_wal_corruption_detect");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        let wal_file = temp_dir.join("wal_00000000.log");

        // Write valid entries
        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                sync_on_write: true,
                ..Default::default()
            };

            let mut wal = Wal::with_config(config)?;
            wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
            wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
            wal.put(Key::from_str("key3"), CipherBlob::new(vec![7, 8, 9]))?;
            wal.flush()?;
        }

        // Corrupt the middle entry by modifying bytes in the WAL file
        {
            let data = std::fs::read(&wal_file).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read WAL: {}", e)))
            })?;

            let mut corrupted_data = data.clone();
            // The first entry starts at offset 0: 4 bytes length prefix + entry data
            // Find the start of the second entry by reading the first entry's length
            let first_entry_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
            let second_entry_start = 4 + first_entry_len;

            // Corrupt bytes inside the second entry (after length prefix, corrupt the checksum area)
            let corrupt_offset = second_entry_start + 4 + 10; // Skip length prefix and some bytes
            if corrupt_offset < corrupted_data.len() {
                corrupted_data[corrupt_offset] ^= 0xFF;
            }

            let mut file = File::create(&wal_file).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to create file: {}", e)))
            })?;
            file.write_all(&corrupted_data).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to write file: {}", e)))
            })?;
            file.flush().map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to flush file: {}", e)))
            })?;
        }

        // Recovery should detect corruption and skip the corrupted entry
        let (entries, _max_seq, stats) = Wal::recover_with_stats(&temp_dir)?;

        // We should have recovered 2 out of 3 entries (one corrupted)
        assert_eq!(stats.entries_corrupted, 1);
        assert_eq!(stats.entries_recovered, entries.len() as u64);
        assert!(stats.bytes_recovered > 0);
        // At least 2 entries should be recovered (first and possibly third)
        assert!(entries.len() >= 2);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_truncate_before() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_truncate_before");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            max_file_size: 512, // Small to trigger rotation
            max_wal_files: 100, // Don't auto-cleanup
            sync_on_write: true,
        };

        let mut wal = Wal::with_config(config)?;

        // Write enough entries to create multiple WAL files
        for i in 0..30 {
            wal.put(
                Key::from_str(&format!("key_{}", i)),
                CipherBlob::new(vec![i as u8; 100]),
            )?;
        }
        wal.flush()?;

        // Ensure multiple files were created
        let file_count_before = std::fs::read_dir(&temp_dir)
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read dir: {}", e)))
            })?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("wal_") && name.ends_with(".log")
            })
            .count();
        assert!(file_count_before > 1, "Should have multiple WAL files");

        // Truncate all entries with sequence <= 10
        let truncated = wal.truncate_before(10)?;

        // Should have truncated at least one file
        assert!(truncated > 0, "Should have truncated at least one file");

        // Verify remaining entries all have sequence > 10 or are in the current file
        let (remaining_entries, _) = Wal::recover(&temp_dir)?;
        // Entries in remaining files should include those with seq > 10
        // (some with seq <= 10 may remain if they share a file with seq > 10 entries)
        let has_high_seq = remaining_entries.iter().any(|e| e.sequence > 10);
        assert!(has_high_seq, "Should still have entries with sequence > 10");

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_size_tracking() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_size_tracking");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            sync_on_write: true,
            ..Default::default()
        };

        let mut wal = Wal::with_config(config)?;

        // Initial size should be 0
        assert_eq!(wal.current_size(), 0);

        // Write an entry
        wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
        let size_after_one = wal.current_size();
        assert!(size_after_one > 0, "Size should increase after writing");

        // Write another entry
        wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
        let size_after_two = wal.current_size();
        assert!(
            size_after_two > size_after_one,
            "Size should increase with more entries"
        );

        wal.flush()?;

        // Total WAL size should match current size (single file)
        let total = wal.total_wal_size()?;
        assert_eq!(total, size_after_two);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_total_size_multiple_files() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_total_size_multi");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            max_file_size: 512,
            max_wal_files: 100,
            sync_on_write: true,
        };

        let mut wal = Wal::with_config(config)?;

        for i in 0..20 {
            wal.put(
                Key::from_str(&format!("key_{}", i)),
                CipherBlob::new(vec![i as u8; 100]),
            )?;
        }
        wal.flush()?;

        let total = wal.total_wal_size()?;
        assert!(total > 0, "Total WAL size should be positive");

        // Total should be larger than current file size if we have multiple files
        if wal.current_file_number() > 0 {
            assert!(
                total >= wal.current_size(),
                "Total size should be >= current file size"
            );
        }

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_empty_recovery() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_empty_recovery");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        // Create an empty WAL file
        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                sync_on_write: true,
                ..Default::default()
            };
            let wal = Wal::with_config(config)?;
            drop(wal);
        }

        // Recovery from directory with empty WAL file
        let (entries, max_seq, stats) = Wal::recover_with_stats(&temp_dir)?;
        assert_eq!(entries.len(), 0);
        assert_eq!(max_seq, 0);
        assert_eq!(stats.entries_recovered, 0);
        assert_eq!(stats.entries_corrupted, 0);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_single_entry_recovery() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_single_entry_recovery");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                sync_on_write: true,
                ..Default::default()
            };

            let mut wal = Wal::with_config(config)?;
            wal.put(Key::from_str("only_key"), CipherBlob::new(vec![42]))?;
            wal.flush()?;
        }

        let (entries, max_seq, stats) = Wal::recover_with_stats(&temp_dir)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(max_seq, 0);
        assert_eq!(stats.entries_recovered, 1);
        assert_eq!(stats.entries_corrupted, 0);
        assert!(stats.bytes_recovered > 0);
        assert_eq!(entries[0].key, Key::from_str("only_key"));

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_large_recovery() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_large_recovery");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        let entry_count = 500;

        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                max_file_size: 4096,
                max_wal_files: 1000,
                sync_on_write: false,
            };

            let mut wal = Wal::with_config(config)?;

            for i in 0..entry_count {
                wal.put(
                    Key::from_str(&format!("large_key_{:05}", i)),
                    CipherBlob::new(vec![(i % 256) as u8; 50]),
                )?;
            }
            wal.flush()?;
        }

        // Recover and verify
        let (entries, max_seq, stats) = Wal::recover_with_stats(&temp_dir)?;
        assert_eq!(entries.len(), entry_count);
        assert_eq!(max_seq, (entry_count - 1) as u64);
        assert_eq!(stats.entries_recovered, entry_count as u64);
        assert_eq!(stats.entries_corrupted, 0);
        assert!(stats.bytes_recovered > 0);

        // Verify sequence order
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.sequence, i as u64);
        }

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_truncate_keeps_current_file() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_truncate_keeps_current");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            max_file_size: 512,
            max_wal_files: 100,
            sync_on_write: true,
        };

        let mut wal = Wal::with_config(config)?;

        for i in 0..30 {
            wal.put(
                Key::from_str(&format!("key_{}", i)),
                CipherBlob::new(vec![i as u8; 100]),
            )?;
        }
        wal.flush()?;

        let current_file_num = wal.current_file_number();

        // Truncate everything (use a very high sequence number)
        wal.truncate_before(u64::MAX)?;

        // Current file should still exist
        let current_path = Wal::wal_file_path(&temp_dir, current_file_num);
        assert!(
            current_path.exists(),
            "Current active WAL file should not be removed"
        );

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_sequence_recovery_across_rotations() -> Result<()> {
        use std::env;

        let temp_dir = env::temp_dir().join("test_wal_seq_recovery_rotation");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        // Phase 1: Write entries across multiple rotations
        let entries_written;
        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                max_file_size: 512,
                max_wal_files: 100,
                sync_on_write: true,
            };

            let mut wal = Wal::with_config(config)?;

            for i in 0..25 {
                wal.put(
                    Key::from_str(&format!("rkey_{}", i)),
                    CipherBlob::new(vec![i as u8; 80]),
                )?;
            }
            wal.flush()?;
            entries_written = wal.sequence();
        }

        // Phase 2: Open new WAL and verify sequence continues
        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                max_file_size: 512,
                max_wal_files: 100,
                sync_on_write: true,
            };

            let wal = Wal::with_config(config)?;
            assert_eq!(
                wal.sequence(),
                entries_written,
                "Sequence should continue from where it left off"
            );
        }

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_wal_recovery_stats_with_corruption() -> Result<()> {
        use std::env;
        use std::io::Write as IoWrite;

        let temp_dir = env::temp_dir().join("test_wal_recovery_stats_corrupt");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        let wal_file = temp_dir.join("wal_00000000.log");

        // Write valid entries
        {
            let config = WalConfig {
                wal_dir: temp_dir.clone(),
                sync_on_write: true,
                ..Default::default()
            };

            let mut wal = Wal::with_config(config)?;
            wal.put(Key::from_str("s1"), CipherBlob::new(vec![10]))?;
            wal.put(Key::from_str("s2"), CipherBlob::new(vec![20]))?;
            wal.flush()?;
        }

        // Append garbage data that looks like a valid length prefix but has bad content
        {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&wal_file)
                .map_err(|e| {
                    AmateRSError::IoError(ErrorContext::new(format!(
                        "Failed to open for corruption: {}",
                        e
                    )))
                })?;
            // Write a length prefix for 30 bytes, then 30 bytes of garbage
            let fake_len = 30u32;
            file.write_all(&fake_len.to_le_bytes()).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("write error: {}", e)))
            })?;
            file.write_all(&[0xDE; 30]).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("write error: {}", e)))
            })?;
            file.flush().map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("flush error: {}", e)))
            })?;
        }

        let (_entries, _max_seq, stats) = Wal::recover_with_stats(&temp_dir)?;

        assert_eq!(stats.entries_recovered, 2);
        assert!(
            stats.entries_corrupted >= 1,
            "Should detect at least one corrupted entry"
        );

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }
}
