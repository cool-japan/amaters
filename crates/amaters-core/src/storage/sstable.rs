//! SSTable (Sorted String Table) implementation
//!
//! SSTables are immutable, on-disk sorted key-value stores used in LSM-Tree.
//! They store memtable snapshots persistently with efficient read access.

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::compression::{self, CompressionType};
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
const SSTABLE_VERSION: u32 = 3; // Version 3 adds block compression

/// Default block size (4KB)
const DEFAULT_BLOCK_SIZE: usize = 4096;

/// SSTable configuration
#[derive(Debug, Clone)]
pub struct SSTableConfig {
    /// Block size in bytes (uncompressed target size)
    pub block_size: usize,
    /// Compression algorithm for data blocks
    pub compression_type: CompressionType,
}

impl Default for SSTableConfig {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE,
            compression_type: CompressionType::None,
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

/// Footer size in bytes
const FOOTER_SIZE: usize = 37;

/// SSTable footer containing metadata
#[derive(Debug, Clone)]
struct Footer {
    magic: u32,
    version: u32,
    index_offset: u64,
    bloom_filter_offset: u64,
    block_size: u32,
    num_blocks: u32,
    compression_type: CompressionType,
    checksum: u32,
}

impl Footer {
    fn new(
        index_offset: u64,
        bloom_filter_offset: u64,
        block_size: u32,
        num_blocks: u32,
        compression_type: CompressionType,
    ) -> Self {
        let mut footer = Self {
            magic: SSTABLE_MAGIC,
            version: SSTABLE_VERSION,
            index_offset,
            bloom_filter_offset,
            block_size,
            num_blocks,
            compression_type,
            checksum: 0,
        };

        footer.checksum = footer.compute_checksum();
        footer
    }

    fn compute_checksum(&self) -> u32 {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.magic.to_le_bytes());
        bytes.extend_from_slice(&self.version.to_le_bytes());
        bytes.extend_from_slice(&self.index_offset.to_le_bytes());
        bytes.extend_from_slice(&self.bloom_filter_offset.to_le_bytes());
        bytes.extend_from_slice(&self.block_size.to_le_bytes());
        bytes.extend_from_slice(&self.num_blocks.to_le_bytes());
        bytes.push(self.compression_type.to_byte());
        calculate_checksum(&bytes)
    }

    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(FOOTER_SIZE);
        bytes.extend_from_slice(&self.magic.to_le_bytes());
        bytes.extend_from_slice(&self.version.to_le_bytes());
        bytes.extend_from_slice(&self.index_offset.to_le_bytes());
        bytes.extend_from_slice(&self.bloom_filter_offset.to_le_bytes());
        bytes.extend_from_slice(&self.block_size.to_le_bytes());
        bytes.extend_from_slice(&self.num_blocks.to_le_bytes());
        bytes.push(self.compression_type.to_byte());
        bytes.extend_from_slice(&self.checksum.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < FOOTER_SIZE {
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
        let compression_type = CompressionType::from_byte(bytes[32])?;
        let checksum = u32::from_le_bytes([bytes[33], bytes[34], bytes[35], bytes[36]]);

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

        let footer = Self {
            magic,
            version,
            index_offset,
            bloom_filter_offset,
            block_size,
            num_blocks,
            compression_type,
            checksum,
        };

        // Verify checksum
        let expected = footer.compute_checksum();
        if checksum != expected {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Footer checksum mismatch: expected {}, got {}",
                expected, checksum
            ))));
        }

        Ok(footer)
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
    ///
    /// Block on-disk format (with compression):
    /// ```text
    /// [original_size: 4 bytes LE][compressed_size: 4 bytes LE][compressed_data: N bytes]
    /// ```
    ///
    /// When compression is `None`, `compressed_data` is the raw encoded block
    /// and `original_size == compressed_size`.
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
        let original_size = block_bytes.len() as u32;

        let compressed = compression::compress_block(&block_bytes, self.config.compression_type)?;
        let compressed_size = compressed.len() as u32;

        // Write block envelope: original_size + compressed_size + data
        writer
            .write_all(&original_size.to_le_bytes())
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to write block original size: {}",
                    e
                )))
            })?;
        writer
            .write_all(&compressed_size.to_le_bytes())
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to write block compressed size: {}",
                    e
                )))
            })?;
        writer.write_all(&compressed).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to write compressed block: {}",
                e
            )))
        })?;

        // 8 bytes header + compressed data
        self.current_offset += 8 + compressed.len() as u64;
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
            self.config.compression_type,
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
    compression_type: CompressionType,
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

        if file_size < FOOTER_SIZE as u64 {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "SSTable file too small".to_string(),
            )));
        }

        let mut reader = BufReader::new(&file);
        reader
            .seek(SeekFrom::End(-(FOOTER_SIZE as i64)))
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to seek to footer: {}",
                    e
                )))
            })?;

        let mut footer_bytes = [0u8; FOOTER_SIZE];
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
        let bloom_size = bloom_metadata.num_bits.div_ceil(8);
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

        let compression_type = footer.compression_type;

        Ok(Self {
            path: path.as_ref().to_path_buf(),
            file: Arc::new(file),
            footer,
            index,
            bloom_filter,
            compression_type,
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

    /// Read a block from disk, decompressing if necessary
    ///
    /// On-disk format per block:
    /// ```text
    /// [original_size: 4 bytes LE][compressed_size: 4 bytes LE][compressed_data: N bytes]
    /// ```
    fn read_block(&self, block_index: usize) -> Result<DataBlock> {
        if block_index >= self.index.len() {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Block index out of bounds".to_string(),
            )));
        }

        let offset = self.index[block_index].offset;

        let mut reader = BufReader::new(self.file.as_ref());
        reader.seek(SeekFrom::Start(offset)).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to seek to block: {}",
                e
            )))
        })?;

        // Read block envelope header (8 bytes)
        let mut header = [0u8; 8];
        reader.read_exact(&mut header).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to read block header: {}",
                e
            )))
        })?;

        let original_size =
            u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let compressed_size =
            u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;

        // Read compressed data
        let mut compressed_data = vec![0u8; compressed_size];
        reader.read_exact(&mut compressed_data).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to read compressed block data: {}",
                e
            )))
        })?;

        // Decompress
        let block_bytes =
            compression::decompress_block(&compressed_data, self.compression_type, original_size)?;

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
                compression_type: CompressionType::None,
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

    /// Helper to write and read back an SSTable with the given compression type
    fn write_read_roundtrip(
        filename: &str,
        compression_type: CompressionType,
        num_entries: usize,
        value_size: usize,
        block_size: usize,
    ) -> Result<()> {
        let dir = env::temp_dir();
        let path = dir.join(filename);

        // Write
        {
            let config = SSTableConfig {
                block_size,
                compression_type,
            };
            let mut writer = SSTableWriter::new(&path, config)?;

            for i in 0..num_entries {
                let key = Key::from_str(&format!("key_{:06}", i));
                // Create somewhat compressible data (repeated byte patterns)
                let mut value_data = Vec::with_capacity(value_size);
                for j in 0..value_size {
                    value_data.push(((i + j) % 256) as u8);
                }
                let value = CipherBlob::new(value_data);
                writer.add(key, value)?;
            }

            writer.finish()?;
        }

        // Read and verify
        {
            let reader = SSTableReader::open(&path)?;

            for i in 0..num_entries {
                let key = Key::from_str(&format!("key_{:06}", i));
                let value = reader.get(&key)?.ok_or_else(|| {
                    AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                        "Missing key {} with {:?} compression",
                        i, compression_type
                    )))
                })?;

                assert_eq!(value.as_bytes().len(), value_size);
                for j in 0..value_size {
                    assert_eq!(
                        value.as_bytes()[j],
                        ((i + j) % 256) as u8,
                        "Value mismatch at key={}, byte={}",
                        i,
                        j
                    );
                }
            }

            // Verify non-existent key
            let missing = Key::from_str("nonexistent_key");
            assert!(reader.get(&missing)?.is_none());

            // Verify iteration
            let entries = reader.iter()?;
            assert_eq!(entries.len(), num_entries);
        }

        std::fs::remove_file(&path).ok();
        Ok(())
    }

    #[test]
    fn test_sstable_compressed_lz4_basic() -> Result<()> {
        write_read_roundtrip(
            "test_sstable_lz4_basic.sst",
            CompressionType::Lz4,
            20,
            200,
            DEFAULT_BLOCK_SIZE,
        )
    }

    #[test]
    fn test_sstable_compressed_deflate_basic() -> Result<()> {
        write_read_roundtrip(
            "test_sstable_deflate_basic.sst",
            CompressionType::Deflate,
            20,
            200,
            DEFAULT_BLOCK_SIZE,
        )
    }

    #[test]
    fn test_sstable_compressed_lz4_multiple_blocks() -> Result<()> {
        write_read_roundtrip(
            "test_sstable_lz4_multiblock.sst",
            CompressionType::Lz4,
            100,
            100,
            256, // Small block size forces many blocks
        )
    }

    #[test]
    fn test_sstable_compressed_deflate_multiple_blocks() -> Result<()> {
        write_read_roundtrip(
            "test_sstable_deflate_multiblock.sst",
            CompressionType::Deflate,
            100,
            100,
            256,
        )
    }

    #[test]
    fn test_sstable_compression_ratio() -> Result<()> {
        let dir = env::temp_dir();
        let path_none = dir.join("test_sstable_ratio_none.sst");
        let path_lz4 = dir.join("test_sstable_ratio_lz4.sst");
        let path_deflate = dir.join("test_sstable_ratio_deflate.sst");

        // Write highly compressible data (repeated patterns)
        let num_entries = 200;
        let value_size = 500;

        for (path, ct) in [
            (&path_none, CompressionType::None),
            (&path_lz4, CompressionType::Lz4),
            (&path_deflate, CompressionType::Deflate),
        ] {
            let config = SSTableConfig {
                block_size: DEFAULT_BLOCK_SIZE,
                compression_type: ct,
            };
            let mut writer = SSTableWriter::new(path, config)?;

            for i in 0..num_entries {
                let key = Key::from_str(&format!("key_{:06}", i));
                // Highly repetitive data
                let value = CipherBlob::new(vec![(i % 10) as u8; value_size]);
                writer.add(key, value)?;
            }

            writer.finish()?;
        }

        let size_none = std::fs::metadata(&path_none)
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to get file size: {}",
                    e
                )))
            })?
            .len();
        let size_lz4 = std::fs::metadata(&path_lz4)
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to get file size: {}",
                    e
                )))
            })?
            .len();
        let size_deflate = std::fs::metadata(&path_deflate)
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to get file size: {}",
                    e
                )))
            })?
            .len();

        // Compressed files should be smaller than uncompressed
        assert!(
            size_lz4 < size_none,
            "LZ4 ({}) should be smaller than None ({})",
            size_lz4,
            size_none
        );
        assert!(
            size_deflate < size_none,
            "Deflate ({}) should be smaller than None ({})",
            size_deflate,
            size_none
        );

        // Verify all three can be read back correctly
        for path in [&path_none, &path_lz4, &path_deflate] {
            let reader = SSTableReader::open(path)?;
            let entries = reader.iter()?;
            assert_eq!(entries.len(), num_entries);
        }

        std::fs::remove_file(&path_none).ok();
        std::fs::remove_file(&path_lz4).ok();
        std::fs::remove_file(&path_deflate).ok();

        Ok(())
    }

    #[test]
    fn test_sstable_large_block_compression() -> Result<()> {
        // 64KB block with large values
        write_read_roundtrip(
            "test_sstable_large_block_comp.sst",
            CompressionType::Lz4,
            10,
            10000,
            65536,
        )
    }

    #[test]
    fn test_sstable_compressed_empty() -> Result<()> {
        let dir = env::temp_dir();

        for ct in [CompressionType::Lz4, CompressionType::Deflate] {
            let filename = format!("test_sstable_empty_{:?}.sst", ct);
            let path = dir.join(&filename);

            {
                let config = SSTableConfig {
                    block_size: DEFAULT_BLOCK_SIZE,
                    compression_type: ct,
                };
                let writer = SSTableWriter::new(&path, config)?;
                writer.finish()?;
            }

            {
                let reader = SSTableReader::open(&path)?;
                let entries = reader.iter()?;
                assert_eq!(entries.len(), 0);

                let key = Key::from_str("any_key");
                assert!(reader.get(&key)?.is_none());
            }

            std::fs::remove_file(&path).ok();
        }

        Ok(())
    }

    #[test]
    fn test_sstable_compressed_iteration_order() -> Result<()> {
        let dir = env::temp_dir();
        let path = dir.join("test_sstable_comp_iter_order.sst");

        {
            let config = SSTableConfig {
                block_size: 256,
                compression_type: CompressionType::Deflate,
            };
            let mut writer = SSTableWriter::new(&path, config)?;

            for i in 0..50 {
                let key = Key::from_str(&format!("key_{:06}", i));
                let value = CipherBlob::new(vec![i as u8; 100]);
                writer.add(key, value)?;
            }

            writer.finish()?;
        }

        {
            let reader = SSTableReader::open(&path)?;
            let entries = reader.iter()?;

            assert_eq!(entries.len(), 50);

            // Verify sorted order is preserved through compression
            for i in 0..49 {
                assert!(
                    entries[i].0 < entries[i + 1].0,
                    "Order violation at index {}",
                    i
                );
            }
        }

        std::fs::remove_file(&path).ok();
        Ok(())
    }

    #[test]
    fn test_sstable_compressed_metadata() -> Result<()> {
        let dir = env::temp_dir();
        let path = dir.join("test_sstable_comp_metadata.sst");

        {
            let config = SSTableConfig {
                block_size: DEFAULT_BLOCK_SIZE,
                compression_type: CompressionType::Lz4,
            };
            let mut writer = SSTableWriter::new(&path, config)?;

            for i in 0..25 {
                let key = Key::from_str(&format!("key_{:06}", i));
                let value = CipherBlob::new(vec![i as u8; 50]);
                writer.add(key, value)?;
            }

            writer.finish()?;
        }

        {
            let reader = SSTableReader::open(&path)?;
            let (min_key, max_key, count) = reader.metadata()?;

            assert_eq!(min_key, Key::from_str("key_000000"));
            assert_eq!(max_key, Key::from_str("key_000024"));
            assert_eq!(count, 25);
        }

        std::fs::remove_file(&path).ok();
        Ok(())
    }

    #[test]
    fn test_sstable_compressed_bloom_filter() -> Result<()> {
        let dir = env::temp_dir();
        let path = dir.join("test_sstable_comp_bloom.sst");

        {
            let config = SSTableConfig {
                block_size: DEFAULT_BLOCK_SIZE,
                compression_type: CompressionType::Deflate,
            };
            let mut writer = SSTableWriter::new(&path, config)?;

            for i in 0..100 {
                let key = Key::from_str(&format!("existing_{:06}", i));
                let value = CipherBlob::new(vec![i as u8; 30]);
                writer.add(key, value)?;
            }

            writer.finish()?;
        }

        {
            let reader = SSTableReader::open(&path)?;

            // Existing keys should pass bloom filter
            for i in 0..100 {
                let key = Key::from_str(&format!("existing_{:06}", i));
                assert!(reader.may_contain(&key));
            }

            // Non-existent keys should mostly be rejected (bloom filter may have FPs)
            let mut rejected = 0;
            for i in 0..1000 {
                let key = Key::from_str(&format!("missing_{:06}", i));
                if !reader.may_contain(&key) {
                    rejected += 1;
                }
            }
            // With 1% FP rate, at least 900 of 1000 should be rejected
            assert!(
                rejected > 900,
                "Bloom filter rejected only {} of 1000 non-existent keys",
                rejected
            );
        }

        std::fs::remove_file(&path).ok();
        Ok(())
    }
}
