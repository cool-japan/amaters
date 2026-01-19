//! SSTable (Sorted String Table) implementation
//!
//! SSTables are immutable, on-disk sorted key-value stores used in LSM-Tree.
//! They store memtable snapshots persistently with efficient read access.

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::{BloomFilter, BloomFilterConfig, BloomFilterMetadata};
use crate::types::{CipherBlob, Key};
use crate::utils::{calculate_checksum, verify_checksum};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// SSTable magic number: "SSTA" (0x53535441)
const SSTABLE_MAGIC: u32 = 0x53535441;

/// SSTable format version
const SSTABLE_VERSION: u32 = 2; // Version 2 adds bloom filters

/// Default block size (4KB)
const DEFAULT_BLOCK_SIZE: usize = 4096;

/// SSTable configuration
#[derive(Debug, Clone)]
pub struct SSTableConfig {
    /// Block size in bytes
    pub block_size: usize,
    /// Enable compression (future feature)
    pub enable_compression: bool,
}

impl Default for SSTableConfig {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE,
            enable_compression: false,
        }
    }
}

/// Index entry pointing to a data block
#[derive(Debug, Clone)]
struct IndexEntry {
    /// First key in the block
    key: Key,
    /// Offset of the block in the file
    offset: u64,
}

/// Data block containing key-value pairs
#[derive(Debug, Clone)]
struct DataBlock {
    entries: Vec<(Key, CipherBlob)>,
    size: usize,
}

impl DataBlock {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            size: 0,
        }
    }

    fn add_entry(&mut self, key: Key, value: CipherBlob) {
        let entry_size = 8 + key.as_bytes().len() + value.as_bytes().len();
        self.entries.push((key, value));
        self.size += entry_size;
    }

    fn is_full(&self, block_size: usize) -> bool {
        self.size >= block_size
    }

    fn encode(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(self.size + 8);

        // Number of entries (4 bytes)
        bytes.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());

        // Entries
        for (key, value) in &self.entries {
            let key_bytes = key.as_bytes();
            let value_bytes = value.as_bytes();

            // Key length (4 bytes) + Key
            bytes.extend_from_slice(&(key_bytes.len() as u32).to_le_bytes());
            bytes.extend_from_slice(key_bytes);

            // Value length (4 bytes) + Value
            bytes.extend_from_slice(&(value_bytes.len() as u32).to_le_bytes());
            bytes.extend_from_slice(value_bytes);
        }

        // Checksum (4 bytes)
        let checksum = calculate_checksum(&bytes);
        bytes.extend_from_slice(&checksum.to_le_bytes());

        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Data block too small".to_string(),
            )));
        }

        // Verify checksum
        let data_len = bytes.len() - 4;
        let checksum_bytes = &bytes[data_len..];
        let expected_checksum = u32::from_le_bytes([
            checksum_bytes[0],
            checksum_bytes[1],
            checksum_bytes[2],
            checksum_bytes[3],
        ]);
        verify_checksum(&bytes[..data_len], expected_checksum)?;

        let mut cursor = 0;
        let num_entries = u32::from_le_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]) as usize;
        cursor += 4;

        let mut block = DataBlock::new();

        for _ in 0..num_entries {
            // Read key
            if cursor + 4 > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Incomplete key length".to_string(),
                )));
            }
            let key_len = u32::from_le_bytes([
                bytes[cursor],
                bytes[cursor + 1],
                bytes[cursor + 2],
                bytes[cursor + 3],
            ]) as usize;
            cursor += 4;

            if cursor + key_len > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Incomplete key data".to_string(),
                )));
            }
            let key = Key::from_slice(&bytes[cursor..cursor + key_len]);
            cursor += key_len;

            // Read value
            if cursor + 4 > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Incomplete value length".to_string(),
                )));
            }
            let value_len = u32::from_le_bytes([
                bytes[cursor],
                bytes[cursor + 1],
                bytes[cursor + 2],
                bytes[cursor + 3],
            ]) as usize;
            cursor += 4;

            if cursor + value_len > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Incomplete value data".to_string(),
                )));
            }
            let value = CipherBlob::new(bytes[cursor..cursor + value_len].to_vec());
            cursor += value_len;

            block.add_entry(key, value);
        }

        Ok(block)
    }
}

/// SSTable footer containing metadata
#[derive(Debug, Clone)]
struct Footer {
    magic: u32,
    version: u32,
    index_offset: u64,
    bloom_filter_offset: u64,
    block_size: u32,
    num_blocks: u32,
    checksum: u32,
}

impl Footer {
    fn new(index_offset: u64, bloom_filter_offset: u64, block_size: u32, num_blocks: u32) -> Self {
        let mut footer = Self {
            magic: SSTABLE_MAGIC,
            version: SSTABLE_VERSION,
            index_offset,
            bloom_filter_offset,
            block_size,
            num_blocks,
            checksum: 0,
        };

        // Calculate checksum of footer (excluding checksum field)
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&footer.magic.to_le_bytes());
        bytes.extend_from_slice(&footer.version.to_le_bytes());
        bytes.extend_from_slice(&footer.index_offset.to_le_bytes());
        bytes.extend_from_slice(&footer.bloom_filter_offset.to_le_bytes());
        bytes.extend_from_slice(&footer.block_size.to_le_bytes());
        bytes.extend_from_slice(&footer.num_blocks.to_le_bytes());
        footer.checksum = calculate_checksum(&bytes);

        footer
    }

    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(36);
        bytes.extend_from_slice(&self.magic.to_le_bytes());
        bytes.extend_from_slice(&self.version.to_le_bytes());
        bytes.extend_from_slice(&self.index_offset.to_le_bytes());
        bytes.extend_from_slice(&self.bloom_filter_offset.to_le_bytes());
        bytes.extend_from_slice(&self.block_size.to_le_bytes());
        bytes.extend_from_slice(&self.num_blocks.to_le_bytes());
        bytes.extend_from_slice(&self.checksum.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 36 {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Footer too small".to_string(),
            )));
        }

        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let index_offset = u64::from_le_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]);
        let bloom_filter_offset = u64::from_le_bytes([
            bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
        ]);
        let block_size = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
        let num_blocks = u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]);
        let checksum = u32::from_le_bytes([bytes[32], bytes[33], bytes[34], bytes[35]]);

        if magic != SSTABLE_MAGIC {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Invalid SSTable magic: expected {}, got {}",
                SSTABLE_MAGIC, magic
            ))));
        }

        if version != SSTABLE_VERSION {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Unsupported SSTable version: {}",
                version
            ))));
        }

        // Verify checksum
        let mut verify_bytes = Vec::new();
        verify_bytes.extend_from_slice(&magic.to_le_bytes());
        verify_bytes.extend_from_slice(&version.to_le_bytes());
        verify_bytes.extend_from_slice(&index_offset.to_le_bytes());
        verify_bytes.extend_from_slice(&bloom_filter_offset.to_le_bytes());
        verify_bytes.extend_from_slice(&block_size.to_le_bytes());
        verify_bytes.extend_from_slice(&num_blocks.to_le_bytes());
        verify_checksum(&verify_bytes, checksum)?;

        Ok(Self {
            magic,
            version,
            index_offset,
            bloom_filter_offset,
            block_size,
            num_blocks,
            checksum,
        })
    }
}

/// SSTable writer - builds SSTable from sorted entries
pub struct SSTableWriter {
    path: PathBuf,
    config: SSTableConfig,
    writer: Option<BufWriter<File>>,
    current_block: DataBlock,
    index: Vec<IndexEntry>,
    current_offset: u64,
    bloom_filter: BloomFilter,
}

impl SSTableWriter {
    /// Create a new SSTable writer
    pub fn new<P: AsRef<Path>>(path: P, config: SSTableConfig) -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path.as_ref())
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to create SSTable file: {}",
                    e
                )))
            })?;

        // Create bloom filter with default configuration
        let bloom_filter = BloomFilter::new(BloomFilterConfig {
            expected_elements: 10000,  // Default estimate
            false_positive_rate: 0.01, // 1%
        });

        Ok(Self {
            path: path.as_ref().to_path_buf(),
            config,
            writer: Some(BufWriter::new(file)),
            current_block: DataBlock::new(),
            index: Vec::new(),
            current_offset: 0,
            bloom_filter,
        })
    }

    /// Add a key-value pair (must be in sorted order)
    pub fn add(&mut self, key: Key, value: CipherBlob) -> Result<()> {
        // If adding this entry would exceed block size, flush current block
        let entry_size = 8 + key.as_bytes().len() + value.as_bytes().len();
        if self.current_block.size + entry_size > self.config.block_size
            && !self.current_block.entries.is_empty()
        {
            self.flush_block()?;
        }

        // If this is the first entry in the block, add to index
        if self.current_block.entries.is_empty() {
            self.index.push(IndexEntry {
                key: key.clone(),
                offset: self.current_offset,
            });
        }

        // Insert key into bloom filter
        self.bloom_filter.insert(&key);

        self.current_block.add_entry(key, value);
        Ok(())
    }

    /// Flush current block to disk
    fn flush_block(&mut self) -> Result<()> {
        if self.current_block.entries.is_empty() {
            return Ok(());
        }

        let writer = self.writer.as_mut().ok_or_else(|| {
            AmateRSError::StorageIntegrity(ErrorContext::new(
                "SSTable writer already finalized".to_string(),
            ))
        })?;

        let block_bytes = self.current_block.encode()?;
        writer.write_all(&block_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to write block: {}",
                e
            )))
        })?;

        self.current_offset += block_bytes.len() as u64;
        self.current_block = DataBlock::new();

        Ok(())
    }

    /// Finalize the SSTable (write index and footer)
    pub fn finish(mut self) -> Result<()> {
        // Flush remaining block
        self.flush_block()?;

        let writer = self.writer.as_mut().ok_or_else(|| {
            AmateRSError::StorageIntegrity(ErrorContext::new(
                "SSTable writer already finalized".to_string(),
            ))
        })?;

        // Write index block
        let index_offset = self.current_offset;
        let mut index_bytes = Vec::new();

        // Number of index entries
        index_bytes.extend_from_slice(&(self.index.len() as u32).to_le_bytes());

        for entry in &self.index {
            let key_bytes = entry.key.as_bytes();
            index_bytes.extend_from_slice(&(key_bytes.len() as u32).to_le_bytes());
            index_bytes.extend_from_slice(key_bytes);
            index_bytes.extend_from_slice(&entry.offset.to_le_bytes());
        }

        let index_checksum = calculate_checksum(&index_bytes);
        index_bytes.extend_from_slice(&index_checksum.to_le_bytes());

        writer.write_all(&index_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to write index: {}",
                e
            )))
        })?;
        self.current_offset += index_bytes.len() as u64;

        // Write bloom filter
        let bloom_filter_offset = self.current_offset;

        // Write bloom filter metadata
        let bloom_metadata = self.bloom_filter.metadata();
        let metadata_bytes = bloom_metadata.to_bytes();
        writer.write_all(&metadata_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to write bloom filter metadata: {}",
                e
            )))
        })?;
        self.current_offset += metadata_bytes.len() as u64;

        // Write bloom filter data
        let bloom_data = self.bloom_filter.as_bytes();
        writer.write_all(bloom_data).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to write bloom filter data: {}",
                e
            )))
        })?;
        self.current_offset += bloom_data.len() as u64;

        // Write footer
        let footer = Footer::new(
            index_offset,
            bloom_filter_offset,
            self.config.block_size as u32,
            self.index.len() as u32,
        );
        let footer_bytes = footer.encode();
        writer.write_all(&footer_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to write footer: {}",
                e
            )))
        })?;

        // Flush and sync
        writer.flush().map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!("Failed to flush: {}", e)))
        })?;

        writer.get_ref().sync_all().map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!("Failed to sync: {}", e)))
        })?;

        self.writer = None;

        Ok(())
    }
}

/// SSTable reader - provides read access to SSTable
pub struct SSTableReader {
    path: PathBuf,
    file: Arc<File>,
    footer: Footer,
    index: Vec<IndexEntry>,
    bloom_filter: BloomFilter,
}

impl SSTableReader {
    /// Open an existing SSTable for reading
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path.as_ref()).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to open SSTable: {}",
                e
            )))
        })?;

        // Read footer
        let file_size = file
            .metadata()
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to get file metadata: {}",
                    e
                )))
            })?
            .len();

        if file_size < 36 {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "SSTable file too small".to_string(),
            )));
        }

        let mut reader = BufReader::new(&file);
        reader.seek(SeekFrom::End(-36)).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to seek to footer: {}",
                e
            )))
        })?;

        let mut footer_bytes = [0u8; 36];
        reader.read_exact(&mut footer_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to read footer: {}",
                e
            )))
        })?;

        let footer = Footer::decode(&footer_bytes)?;

        // Read index
        reader
            .seek(SeekFrom::Start(footer.index_offset))
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to seek to index: {}",
                    e
                )))
            })?;

        // Calculate index size (between index_offset and bloom_filter_offset)
        let index_size = footer.bloom_filter_offset - footer.index_offset;
        let mut index_bytes = vec![0u8; index_size as usize];
        reader.read_exact(&mut index_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to read index: {}",
                e
            )))
        })?;

        // Verify checksum
        let data_len = index_bytes.len() - 4;
        let checksum_bytes = &index_bytes[data_len..];
        let expected_checksum = u32::from_le_bytes([
            checksum_bytes[0],
            checksum_bytes[1],
            checksum_bytes[2],
            checksum_bytes[3],
        ]);
        verify_checksum(&index_bytes[..data_len], expected_checksum)?;

        // Parse index
        let mut cursor = 0;
        let num_entries = u32::from_le_bytes([
            index_bytes[cursor],
            index_bytes[cursor + 1],
            index_bytes[cursor + 2],
            index_bytes[cursor + 3],
        ]) as usize;
        cursor += 4;

        let mut index = Vec::with_capacity(num_entries);

        for _ in 0..num_entries {
            let key_len = u32::from_le_bytes([
                index_bytes[cursor],
                index_bytes[cursor + 1],
                index_bytes[cursor + 2],
                index_bytes[cursor + 3],
            ]) as usize;
            cursor += 4;

            let key = Key::from_slice(&index_bytes[cursor..cursor + key_len]);
            cursor += key_len;

            let offset = u64::from_le_bytes([
                index_bytes[cursor],
                index_bytes[cursor + 1],
                index_bytes[cursor + 2],
                index_bytes[cursor + 3],
                index_bytes[cursor + 4],
                index_bytes[cursor + 5],
                index_bytes[cursor + 6],
                index_bytes[cursor + 7],
            ]);
            cursor += 8;

            index.push(IndexEntry { key, offset });
        }

        // Read bloom filter
        reader
            .seek(SeekFrom::Start(footer.bloom_filter_offset))
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to seek to bloom filter: {}",
                    e
                )))
            })?;

        // Read bloom filter metadata
        let mut metadata_bytes = [0u8; 24];
        reader.read_exact(&mut metadata_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to read bloom filter metadata: {}",
                e
            )))
        })?;

        let bloom_metadata = BloomFilterMetadata::from_bytes(&metadata_bytes)?;

        // Read bloom filter data
        let bloom_size = (bloom_metadata.num_bits + 7) / 8;
        let mut bloom_data = vec![0u8; bloom_size];
        reader.read_exact(&mut bloom_data).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to read bloom filter data: {}",
                e
            )))
        })?;

        let bloom_filter = BloomFilter::from_bytes(
            bloom_data,
            bloom_metadata.num_bits,
            bloom_metadata.num_hash_functions,
            bloom_metadata.num_elements,
        )?;

        Ok(Self {
            path: path.as_ref().to_path_buf(),
            file: Arc::new(file),
            footer,
            index,
            bloom_filter,
        })
    }

    /// Check if a key may be in the SSTable (using bloom filter)
    ///
    /// Returns:
    /// - true: key MAY be in the SSTable (should check with get())
    /// - false: key is DEFINITELY NOT in the SSTable
    pub fn may_contain(&self, key: &Key) -> bool {
        self.bloom_filter.may_contain(key)
    }

    /// Get a value by key
    pub fn get(&self, key: &Key) -> Result<Option<CipherBlob>> {
        // Check bloom filter first for fast negative lookups
        if !self.may_contain(key) {
            return Ok(None);
        }

        // Find the block that might contain this key
        let Some(block_index) = self.find_block_index(key) else {
            return Ok(None);
        };
        let block = self.read_block(block_index)?;

        // Search for key in block
        for (k, v) in &block.entries {
            if k == key {
                return Ok(Some(v.clone()));
            }
        }

        Ok(None)
    }

    /// Find the block index that might contain the key
    fn find_block_index(&self, key: &Key) -> Option<usize> {
        // Binary search in index
        match self.index.binary_search_by(|entry| entry.key.cmp(key)) {
            Ok(idx) => Some(idx),
            Err(idx) => {
                if idx == 0 {
                    None
                } else {
                    Some(idx - 1)
                }
            }
        }
    }

    /// Read a block from disk
    fn read_block(&self, block_index: usize) -> Result<DataBlock> {
        if block_index >= self.index.len() {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Block index out of bounds".to_string(),
            )));
        }

        let offset = self.index[block_index].offset;
        let next_offset = if block_index + 1 < self.index.len() {
            self.index[block_index + 1].offset
        } else {
            self.footer.index_offset
        };

        let block_size = (next_offset - offset) as usize;
        let mut block_bytes = vec![0u8; block_size];

        let mut reader = BufReader::new(self.file.as_ref());
        reader.seek(SeekFrom::Start(offset)).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to seek to block: {}",
                e
            )))
        })?;

        reader.read_exact(&mut block_bytes).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to read block: {}",
                e
            )))
        })?;

        DataBlock::decode(&block_bytes)
    }

    /// Get all entries in the SSTable (for iteration)
    pub fn iter(&self) -> Result<Vec<(Key, CipherBlob)>> {
        let mut entries = Vec::new();

        for i in 0..self.index.len() {
            let block = self.read_block(i)?;
            entries.extend(block.entries);
        }

        Ok(entries)
    }

    /// Get SSTable metadata (min_key, max_key, num_entries)
    pub fn metadata(&self) -> Result<(Key, Key, usize)> {
        if self.index.is_empty() {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "SSTable has no entries".to_string(),
            )));
        }

        // Get all entries to find min/max keys
        let entries = self.iter()?;

        if entries.is_empty() {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "SSTable has no data entries".to_string(),
            )));
        }

        let min_key = entries
            .first()
            .ok_or_else(|| {
                AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Failed to get first entry".to_string(),
                ))
            })?
            .0
            .clone();

        let max_key = entries
            .last()
            .ok_or_else(|| {
                AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Failed to get last entry".to_string(),
                ))
            })?
            .0
            .clone();

        Ok((min_key, max_key, entries.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_sstable_basic_write_read() -> Result<()> {
        let dir = env::temp_dir();
        let path = dir.join("test_sstable_basic.sst");

        // Write SSTable
        {
            let config = SSTableConfig::default();
            let mut writer = SSTableWriter::new(&path, config)?;

            for i in 0..10 {
                let key = Key::from_str(&format!("key_{:03}", i));
                let value = CipherBlob::new(vec![i as u8; 100]);
                writer.add(key, value)?;
            }

            writer.finish()?;
        }

        // Read SSTable
        {
            let reader = SSTableReader::open(&path)?;

            // Check we can read all keys
            for i in 0..10 {
                let key = Key::from_str(&format!("key_{:03}", i));
                let value = reader.get(&key)?;
                assert!(value.is_some());
                let value = value.expect("Value should exist in SSTable");
                assert_eq!(value.as_bytes()[0], i as u8);
            }

            // Non-existent key
            let key = Key::from_str("nonexistent");
            let value = reader.get(&key)?;
            assert!(value.is_none());
        }

        // Cleanup
        std::fs::remove_file(&path).ok();

        Ok(())
    }

    #[test]
    fn test_sstable_multiple_blocks() -> Result<()> {
        let dir = env::temp_dir();
        let path = dir.join("test_sstable_blocks.sst");

        // Write with small block size to force multiple blocks
        {
            let config = SSTableConfig {
                block_size: 256,
                enable_compression: false,
            };
            let mut writer = SSTableWriter::new(&path, config)?;

            for i in 0..100 {
                let key = Key::from_str(&format!("key_{:03}", i));
                let value = CipherBlob::new(vec![i as u8; 50]);
                writer.add(key, value)?;
            }

            writer.finish()?;
        }

        // Read and verify
        {
            let reader = SSTableReader::open(&path)?;

            for i in 0..100 {
                let key = Key::from_str(&format!("key_{:03}", i));
                let value = reader.get(&key)?;
                assert!(value.is_some());
            }
        }

        std::fs::remove_file(&path).ok();

        Ok(())
    }

    #[test]
    fn test_sstable_iteration() -> Result<()> {
        let dir = env::temp_dir();
        let path = dir.join("test_sstable_iter.sst");

        // Write
        {
            let config = SSTableConfig::default();
            let mut writer = SSTableWriter::new(&path, config)?;

            for i in 0..50 {
                let key = Key::from_str(&format!("key_{:03}", i));
                let value = CipherBlob::new(vec![i as u8; 100]);
                writer.add(key, value)?;
            }

            writer.finish()?;
        }

        // Iterate
        {
            let reader = SSTableReader::open(&path)?;
            let entries = reader.iter()?;

            assert_eq!(entries.len(), 50);

            // Check ordering
            for i in 0..49 {
                assert!(entries[i].0 < entries[i + 1].0);
            }
        }

        std::fs::remove_file(&path).ok();

        Ok(())
    }

    #[test]
    fn test_sstable_empty() -> Result<()> {
        let dir = env::temp_dir();
        let path = dir.join("test_sstable_empty.sst");

        // Write empty SSTable
        {
            let config = SSTableConfig::default();
            let writer = SSTableWriter::new(&path, config)?;
            writer.finish()?;
        }

        // Read
        {
            let reader = SSTableReader::open(&path)?;
            let entries = reader.iter()?;
            assert_eq!(entries.len(), 0);

            let key = Key::from_str("any_key");
            let value = reader.get(&key)?;
            assert!(value.is_none());
        }

        std::fs::remove_file(&path).ok();

        Ok(())
    }

    #[test]
    fn test_sstable_large_values() -> Result<()> {
        let dir = env::temp_dir();
        let path = dir.join("test_sstable_large.sst");

        // Write with large values
        {
            let config = SSTableConfig::default();
            let mut writer = SSTableWriter::new(&path, config)?;

            for i in 0..10 {
                let key = Key::from_str(&format!("key_{:03}", i));
                let value = CipherBlob::new(vec![i as u8; 10000]); // 10KB values
                writer.add(key, value)?;
            }

            writer.finish()?;
        }

        // Read
        {
            let reader = SSTableReader::open(&path)?;

            for i in 0..10 {
                let key = Key::from_str(&format!("key_{:03}", i));
                let value = reader.get(&key)?;
                assert!(value.is_some());
                let value = value.expect("Value should exist in SSTable");
                assert_eq!(value.as_bytes().len(), 10000);
            }
        }

        std::fs::remove_file(&path).ok();

        Ok(())
    }

    #[test]
    fn test_sstable_corruption_detection() -> Result<()> {
        let dir = env::temp_dir();
        let path = dir.join("test_sstable_corrupt.sst");

        // Write valid SSTable
        {
            let config = SSTableConfig::default();
            let mut writer = SSTableWriter::new(&path, config)?;

            for i in 0..10 {
                let key = Key::from_str(&format!("key_{:03}", i));
                let value = CipherBlob::new(vec![i as u8; 100]);
                writer.add(key, value)?;
            }

            writer.finish()?;
        }

        // Corrupt the footer (last 28 bytes contain the footer)
        {
            let mut file = OpenOptions::new().write(true).open(&path)?;
            // Corrupt the checksum bytes in the footer
            file.seek(SeekFrom::End(-4))?;
            file.write_all(&[0xFF, 0xFF, 0xFF, 0xFF])?;
        }

        // Try to read - should detect corruption
        let result = SSTableReader::open(&path);
        assert!(result.is_err());

        std::fs::remove_file(&path).ok();

        Ok(())
    }
}
