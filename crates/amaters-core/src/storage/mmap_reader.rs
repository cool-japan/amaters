//! Memory-mapped SSTable reader for read-heavy workloads
//!
//! Uses `memmap2::Mmap` for zero-copy reads instead of explicit `read()` calls.
//! Provides point lookups via binary search over the SSTable index and efficient
//! range scans with OS-level `madvise` hints on Unix systems.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │           MmapReaderPool                 │
//! │  DashMap<PathBuf, Arc<MmapSstableReader>>│
//! └──────────┬───────────────────────────────┘
//!            │ get_or_open()
//!            ▼
//! ┌──────────────────────────────────────────┐
//! │         MmapSstableReader                │
//! │  memmap2::Mmap  ─────► raw bytes         │
//! │  index: Vec<MmapIndexEntry>              │
//! │  bloom_filter: BloomFilter               │
//! └──────────────────────────────────────────┘
//! ```

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::compression::{self, CompressionType};
use crate::storage::{BloomFilter, BloomFilterMetadata};
use crate::types::{CipherBlob, Key};
use crate::utils::{calculate_checksum, verify_checksum};
use dashmap::DashMap;
use memmap2::Mmap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// SSTable magic number: "SSTA" (0x53535441)
const SSTABLE_MAGIC: u32 = 0x53535441;

/// SSTable format version (must match sstable.rs)
const SSTABLE_VERSION: u32 = 3;

/// Footer size in bytes (must match sstable.rs)
const FOOTER_SIZE: usize = 37;

/// Bloom filter metadata size in bytes
const BLOOM_METADATA_SIZE: usize = 24;

/// Parsed footer from the end of an SSTable file.
#[derive(Debug, Clone)]
struct MmapFooter {
    index_offset: u64,
    bloom_filter_offset: u64,
    _block_size: u32,
    num_blocks: u32,
    compression_type: CompressionType,
}

/// Index entry pointing to a data block, parsed from the mmap region.
#[derive(Debug, Clone)]
struct MmapIndexEntry {
    /// First key in the block
    key: Key,
    /// Byte offset of the block in the file
    offset: u64,
}

/// madvise hint for the OS kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MadviseHint {
    /// Random access pattern (point lookups)
    Random,
    /// Sequential access pattern (range scans)
    Sequential,
}

/// Prefetcher that issues `madvise` hints on Unix systems.
///
/// On non-Unix platforms this is a no-op.
pub struct MmapPrefetcher;

impl MmapPrefetcher {
    /// Advise the OS kernel about the expected access pattern for the mmap region.
    #[cfg(unix)]
    pub fn advise(mmap: &Mmap, hint: MadviseHint) -> Result<()> {
        use memmap2::Advice;

        let advice = match hint {
            MadviseHint::Random => Advice::Random,
            MadviseHint::Sequential => Advice::Sequential,
        };
        mmap.advise(advice).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!("madvise failed: {}", e)))
        })
    }

    /// No-op on non-Unix platforms.
    #[cfg(not(unix))]
    pub fn advise(_mmap: &Mmap, _hint: MadviseHint) -> Result<()> {
        Ok(())
    }
}

/// Memory-mapped SSTable reader.
///
/// Thread-safe (`Mmap` is `Send + Sync`) and designed for read-heavy workloads
/// where avoiding explicit `read()` syscalls matters.
pub struct MmapSstableReader {
    /// The memory-mapped file region (entire SSTable)
    mmap: Mmap,
    /// Parsed footer metadata
    footer: MmapFooter,
    /// Sorted index entries (one per data block)
    index: Vec<MmapIndexEntry>,
    /// Bloom filter for fast negative lookups
    bloom_filter: BloomFilter,
    /// Source file path (for diagnostics)
    _path: PathBuf,
}

// Compile-time assertion that `MmapSstableReader` is Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MmapSstableReader>();
};

impl MmapSstableReader {
    /// Open an SSTable file via memory mapping.
    ///
    /// Parses the footer, index, and bloom filter eagerly so that subsequent
    /// `get` / `range` calls only need to touch data blocks.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to open SSTable for mmap: {}",
                e
            )))
        })?;

        let file_len = file
            .metadata()
            .map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to stat SSTable: {}",
                    e
                )))
            })?
            .len() as usize;

        if file_len < FOOTER_SIZE {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "SSTable file too small for footer".to_string(),
            )));
        }

        // SAFETY: the file is opened read-only and we never modify it.
        let mmap = unsafe {
            Mmap::map(&file).map_err(|e| {
                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                    "Failed to mmap SSTable: {}",
                    e
                )))
            })?
        };

        // Default to random-access hint (point lookups are more common).
        MmapPrefetcher::advise(&mmap, MadviseHint::Random)?;

        // --- Parse footer ---
        let footer = Self::parse_footer(&mmap, file_len)?;

        // --- Parse index ---
        let index = Self::parse_index(&mmap, &footer)?;

        // --- Parse bloom filter ---
        let bloom_filter = Self::parse_bloom_filter(&mmap, &footer, file_len)?;

        Ok(Self {
            mmap,
            footer,
            index,
            bloom_filter,
            _path: path_ref.to_path_buf(),
        })
    }

    /// Returns the total number of bytes currently mapped.
    pub fn mapped_bytes(&self) -> usize {
        self.mmap.len()
    }

    // ------------------------------------------------------------------ //
    //  Point lookup
    // ------------------------------------------------------------------ //

    /// Look up a single key, returning its value if present.
    ///
    /// Uses the bloom filter for fast negative answers and binary search
    /// over the index to locate the correct data block.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let key_obj = Key::from_slice(key);

        // Bloom filter short-circuit
        if !self.bloom_filter.may_contain(&key_obj) {
            return Ok(None);
        }

        let block_idx = match self.find_block_index(&key_obj) {
            Some(idx) => idx,
            None => return Ok(None),
        };

        let entries = self.read_block(block_idx)?;
        for (k, v) in &entries {
            if k.as_bytes() == key {
                return Ok(Some(v.clone()));
            }
        }

        Ok(None)
    }

    // ------------------------------------------------------------------ //
    //  Range scan
    // ------------------------------------------------------------------ //

    /// Return all key-value pairs whose keys fall in `[start, end)`.
    ///
    /// Issues a `MADV_SEQUENTIAL` hint before the scan and restores
    /// `MADV_RANDOM` afterwards.
    pub fn range(&self, start: &[u8], end: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        // Hint for sequential access during the scan
        MmapPrefetcher::advise(&self.mmap, MadviseHint::Sequential)?;

        let result = self.range_inner(start, end);

        // Restore random-access hint regardless of success/failure
        let _ = MmapPrefetcher::advise(&self.mmap, MadviseHint::Random);

        result
    }

    fn range_inner(&self, start: &[u8], end: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let start_key = Key::from_slice(start);
        let end_key = Key::from_slice(end);

        // Find the first block that could contain `start`
        let first_block = match self.find_block_index(&start_key) {
            Some(idx) => idx,
            None => {
                // start is before the first key in the SSTable; scan from block 0
                if self.index.is_empty() {
                    return Ok(Vec::new());
                }
                0
            }
        };

        let mut result = Vec::new();

        for block_idx in first_block..self.index.len() {
            // If this block's first key is >= end, we are done
            if self.index[block_idx].key >= end_key {
                // The block's first key is already past the range -- but we
                // still need to check because earlier blocks may have keys
                // within range. However the *first* key of this block is >=
                // end, so all subsequent blocks' first keys are even larger.
                break;
            }

            let entries = self.read_block(block_idx)?;
            for (k, v) in entries {
                if k >= start_key && k < end_key {
                    result.push((k.to_vec(), v));
                } else if k >= end_key {
                    return Ok(result);
                }
            }
        }

        Ok(result)
    }

    // ------------------------------------------------------------------ //
    //  Internal helpers
    // ------------------------------------------------------------------ //

    /// Parse the footer from the last `FOOTER_SIZE` bytes of the mmap.
    fn parse_footer(mmap: &[u8], file_len: usize) -> Result<MmapFooter> {
        let footer_start = file_len - FOOTER_SIZE;
        let bytes = &mmap[footer_start..];

        if bytes.len() < FOOTER_SIZE {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Footer slice too small".to_string(),
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
                "Invalid SSTable magic: expected {:#x}, got {:#x}",
                SSTABLE_MAGIC, magic
            ))));
        }

        if version != SSTABLE_VERSION {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Unsupported SSTable version: {}",
                version
            ))));
        }

        // Verify footer checksum (same algorithm as sstable.rs Footer::compute_checksum)
        let mut cksum_input = Vec::new();
        cksum_input.extend_from_slice(&magic.to_le_bytes());
        cksum_input.extend_from_slice(&version.to_le_bytes());
        cksum_input.extend_from_slice(&index_offset.to_le_bytes());
        cksum_input.extend_from_slice(&bloom_filter_offset.to_le_bytes());
        cksum_input.extend_from_slice(&block_size.to_le_bytes());
        cksum_input.extend_from_slice(&num_blocks.to_le_bytes());
        cksum_input.push(compression_type.to_byte());
        let expected = calculate_checksum(&cksum_input);

        if checksum != expected {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Footer checksum mismatch: expected {}, got {}",
                expected, checksum
            ))));
        }

        Ok(MmapFooter {
            index_offset,
            bloom_filter_offset,
            _block_size: block_size,
            num_blocks,
            compression_type,
        })
    }

    /// Parse the index block from the mmap region.
    fn parse_index(mmap: &[u8], footer: &MmapFooter) -> Result<Vec<MmapIndexEntry>> {
        let start = footer.index_offset as usize;
        let end = footer.bloom_filter_offset as usize;

        if start > mmap.len() || end > mmap.len() || start >= end {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Invalid index region bounds".to_string(),
            )));
        }

        let index_bytes = &mmap[start..end];

        // Verify index checksum (last 4 bytes)
        if index_bytes.len() < 4 {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Index block too small for checksum".to_string(),
            )));
        }
        let data_len = index_bytes.len() - 4;
        let expected_checksum = u32::from_le_bytes([
            index_bytes[data_len],
            index_bytes[data_len + 1],
            index_bytes[data_len + 2],
            index_bytes[data_len + 3],
        ]);
        verify_checksum(&index_bytes[..data_len], expected_checksum)?;

        // Parse entries
        let mut cursor = 0usize;
        if cursor + 4 > data_len {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Index block too small for entry count".to_string(),
            )));
        }
        let num_entries = u32::from_le_bytes([
            index_bytes[cursor],
            index_bytes[cursor + 1],
            index_bytes[cursor + 2],
            index_bytes[cursor + 3],
        ]) as usize;
        cursor += 4;

        if num_entries as u32 != footer.num_blocks {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Index entry count {} does not match footer num_blocks {}",
                num_entries, footer.num_blocks
            ))));
        }

        let mut index = Vec::with_capacity(num_entries);

        for _ in 0..num_entries {
            if cursor + 4 > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Truncated index entry (key length)".to_string(),
                )));
            }
            let key_len = u32::from_le_bytes([
                index_bytes[cursor],
                index_bytes[cursor + 1],
                index_bytes[cursor + 2],
                index_bytes[cursor + 3],
            ]) as usize;
            cursor += 4;

            if cursor + key_len > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Truncated index entry (key data)".to_string(),
                )));
            }
            let key = Key::from_slice(&index_bytes[cursor..cursor + key_len]);
            cursor += key_len;

            if cursor + 8 > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Truncated index entry (offset)".to_string(),
                )));
            }
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

            index.push(MmapIndexEntry { key, offset });
        }

        Ok(index)
    }

    /// Parse the bloom filter from the mmap region.
    fn parse_bloom_filter(
        mmap: &[u8],
        footer: &MmapFooter,
        file_len: usize,
    ) -> Result<BloomFilter> {
        let bf_start = footer.bloom_filter_offset as usize;
        let bf_end = file_len - FOOTER_SIZE;

        if bf_start + BLOOM_METADATA_SIZE > bf_end {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Bloom filter region too small for metadata".to_string(),
            )));
        }

        let metadata_bytes = &mmap[bf_start..bf_start + BLOOM_METADATA_SIZE];
        let bloom_metadata = BloomFilterMetadata::from_bytes(metadata_bytes)?;

        let bloom_data_start = bf_start + BLOOM_METADATA_SIZE;
        let bloom_size = bloom_metadata.num_bits.div_ceil(8);

        if bloom_data_start + bloom_size > bf_end {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Bloom filter data extends past footer".to_string(),
            )));
        }

        let bloom_data = mmap[bloom_data_start..bloom_data_start + bloom_size].to_vec();

        BloomFilter::from_bytes(
            bloom_data,
            bloom_metadata.num_bits,
            bloom_metadata.num_hash_functions,
            bloom_metadata.num_elements,
        )
    }

    /// Binary search the index to find which block could contain `key`.
    fn find_block_index(&self, key: &Key) -> Option<usize> {
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

    /// Read and decompress a single data block from the mmap slice.
    ///
    /// Block on-disk format:
    /// ```text
    /// [original_size: 4 bytes LE][compressed_size: 4 bytes LE][compressed_data: N bytes]
    /// ```
    fn read_block(&self, block_index: usize) -> Result<Vec<(Key, Vec<u8>)>> {
        if block_index >= self.index.len() {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Block index out of bounds".to_string(),
            )));
        }

        let offset = self.index[block_index].offset as usize;
        let data = &self.mmap[..];

        if offset + 8 > data.len() {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Block header extends past mmap region".to_string(),
            )));
        }

        let original_size = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;

        let compressed_size = u32::from_le_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]) as usize;

        let block_data_start = offset + 8;
        let block_data_end = block_data_start + compressed_size;

        if block_data_end > data.len() {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Compressed block data extends past mmap region".to_string(),
            )));
        }

        let compressed_data = &data[block_data_start..block_data_end];

        // Decompress
        let block_bytes = compression::decompress_block(
            compressed_data,
            self.footer.compression_type,
            original_size,
        )?;

        // Parse the data block entries
        Self::parse_data_block_entries(&block_bytes)
    }

    /// Parse key-value entries from a raw (decompressed) data block.
    ///
    /// Format:
    /// ```text
    /// [num_entries: 4 bytes LE]
    /// for each entry:
    ///   [key_len: 4 bytes LE][key: key_len bytes]
    ///   [val_len: 4 bytes LE][val: val_len bytes]
    /// [checksum: 4 bytes LE]
    /// ```
    fn parse_data_block_entries(block_bytes: &[u8]) -> Result<Vec<(Key, Vec<u8>)>> {
        if block_bytes.len() < 8 {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Data block too small".to_string(),
            )));
        }

        // Verify checksum
        let data_len = block_bytes.len() - 4;
        let expected_checksum = u32::from_le_bytes([
            block_bytes[data_len],
            block_bytes[data_len + 1],
            block_bytes[data_len + 2],
            block_bytes[data_len + 3],
        ]);
        verify_checksum(&block_bytes[..data_len], expected_checksum)?;

        let mut cursor = 0usize;
        let num_entries = u32::from_le_bytes([
            block_bytes[cursor],
            block_bytes[cursor + 1],
            block_bytes[cursor + 2],
            block_bytes[cursor + 3],
        ]) as usize;
        cursor += 4;

        let mut entries = Vec::with_capacity(num_entries);

        for _ in 0..num_entries {
            // Key length
            if cursor + 4 > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Truncated key length in data block".to_string(),
                )));
            }
            let key_len = u32::from_le_bytes([
                block_bytes[cursor],
                block_bytes[cursor + 1],
                block_bytes[cursor + 2],
                block_bytes[cursor + 3],
            ]) as usize;
            cursor += 4;

            if cursor + key_len > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Truncated key data in data block".to_string(),
                )));
            }
            let key = Key::from_slice(&block_bytes[cursor..cursor + key_len]);
            cursor += key_len;

            // Value length
            if cursor + 4 > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Truncated value length in data block".to_string(),
                )));
            }
            let val_len = u32::from_le_bytes([
                block_bytes[cursor],
                block_bytes[cursor + 1],
                block_bytes[cursor + 2],
                block_bytes[cursor + 3],
            ]) as usize;
            cursor += 4;

            if cursor + val_len > data_len {
                return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Truncated value data in data block".to_string(),
                )));
            }
            let value = block_bytes[cursor..cursor + val_len].to_vec();
            cursor += val_len;

            entries.push((key, value));
        }

        Ok(entries)
    }
}

// -------------------------------------------------------------------------- //
//  Reader Pool
// -------------------------------------------------------------------------- //

/// Pool of open `MmapSstableReader` instances keyed by file path.
///
/// Avoids re-opening and re-mapping the same SSTable file when multiple
/// components need concurrent read access.
pub struct MmapReaderPool {
    readers: DashMap<PathBuf, Arc<MmapSstableReader>>,
}

impl MmapReaderPool {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self {
            readers: DashMap::new(),
        }
    }

    /// Get an existing reader or open (and cache) a new one.
    pub fn get_or_open<P: AsRef<Path>>(&self, path: P) -> Result<Arc<MmapSstableReader>> {
        let canonical = path.as_ref().to_path_buf();

        // Fast path: reader already cached
        if let Some(entry) = self.readers.get(&canonical) {
            return Ok(Arc::clone(entry.value()));
        }

        // Slow path: open a new reader and insert
        let reader = Arc::new(MmapSstableReader::open(&canonical)?);
        self.readers
            .entry(canonical)
            .or_insert_with(|| Arc::clone(&reader));
        Ok(reader)
    }

    /// Evict (release) a cached reader for the given path.
    ///
    /// Returns `true` if a reader was actually evicted.
    pub fn evict<P: AsRef<Path>>(&self, path: P) -> bool {
        self.readers.remove(&path.as_ref().to_path_buf()).is_some()
    }

    /// Total bytes currently memory-mapped across all cached readers.
    pub fn total_mapped_bytes(&self) -> usize {
        self.readers
            .iter()
            .map(|entry| entry.value().mapped_bytes())
            .sum()
    }

    /// Number of readers currently in the pool.
    pub fn len(&self) -> usize {
        self.readers.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.readers.is_empty()
    }
}

impl Default for MmapReaderPool {
    fn default() -> Self {
        Self::new()
    }
}

// -------------------------------------------------------------------------- //
//  Tests
// -------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sstable::{SSTableConfig, SSTableWriter};
    use std::sync::Barrier;

    /// Helper: write an SSTable with `n` entries at the given path.
    fn write_test_sstable(path: &Path, n: usize) -> Result<()> {
        let config = SSTableConfig::default();
        let mut writer = SSTableWriter::new(path, config)?;

        for i in 0..n {
            let key = Key::from_str(&format!("key_{:06}", i));
            let value = CipherBlob::new(format!("value_{:06}", i).into_bytes());
            writer.add(key, value)?;
        }

        writer.finish()
    }

    // -------------------------------------------------------------- //
    //  Basic round-trip
    // -------------------------------------------------------------- //

    #[test]
    fn test_mmap_basic_roundtrip() -> Result<()> {
        let dir = std::env::temp_dir();
        let path = dir.join("mmap_basic_roundtrip.sst");
        let _cleanup = FileCleanup(&path);

        write_test_sstable(&path, 10)?;
        let reader = MmapSstableReader::open(&path)?;

        for i in 0..10 {
            let key = format!("key_{:06}", i);
            let val = reader.get(key.as_bytes())?;
            assert!(val.is_some(), "key {} should exist", key);
            let expected = format!("value_{:06}", i);
            assert_eq!(
                val.as_deref(),
                Some(expected.as_bytes()),
                "value mismatch for {}",
                key
            );
        }

        Ok(())
    }

    // -------------------------------------------------------------- //
    //  Missing key returns None
    // -------------------------------------------------------------- //

    #[test]
    fn test_mmap_missing_key() -> Result<()> {
        let dir = std::env::temp_dir();
        let path = dir.join("mmap_missing_key.sst");
        let _cleanup = FileCleanup(&path);

        write_test_sstable(&path, 10)?;
        let reader = MmapSstableReader::open(&path)?;

        assert!(reader.get(b"nonexistent_key")?.is_none());
        assert!(reader.get(b"zzz_after_all")?.is_none());
        assert!(reader.get(b"aaa_before_all")?.is_none());

        Ok(())
    }

    // -------------------------------------------------------------- //
    //  Range scan
    // -------------------------------------------------------------- //

    #[test]
    fn test_mmap_range_scan() -> Result<()> {
        let dir = std::env::temp_dir();
        let path = dir.join("mmap_range_scan.sst");
        let _cleanup = FileCleanup(&path);

        write_test_sstable(&path, 100)?;
        let reader = MmapSstableReader::open(&path)?;

        // Range [key_000010, key_000020)
        let start = "key_000010";
        let end = "key_000020";
        let results = reader.range(start.as_bytes(), end.as_bytes())?;

        assert_eq!(results.len(), 10, "expected 10 entries in range");

        for (i, (k, v)) in results.iter().enumerate() {
            let expected_key = format!("key_{:06}", 10 + i);
            let expected_val = format!("value_{:06}", 10 + i);
            assert_eq!(k, expected_key.as_bytes());
            assert_eq!(v, expected_val.as_bytes());
        }

        Ok(())
    }

    #[test]
    fn test_mmap_range_empty() -> Result<()> {
        let dir = std::env::temp_dir();
        let path = dir.join("mmap_range_empty.sst");
        let _cleanup = FileCleanup(&path);

        write_test_sstable(&path, 10)?;
        let reader = MmapSstableReader::open(&path)?;

        // Range that does not overlap any keys
        let results = reader.range(b"zzz_start", b"zzz_end")?;
        assert!(results.is_empty());

        Ok(())
    }

    // -------------------------------------------------------------- //
    //  Large SSTable (1000+ entries)
    // -------------------------------------------------------------- //

    #[test]
    fn test_mmap_large_sstable() -> Result<()> {
        let dir = std::env::temp_dir();
        let path = dir.join("mmap_large_sstable.sst");
        let _cleanup = FileCleanup(&path);

        let count = 1500;
        write_test_sstable(&path, count)?;
        let reader = MmapSstableReader::open(&path)?;

        // Spot-check first, middle, last
        for i in [0, count / 2, count - 1] {
            let key = format!("key_{:06}", i);
            let val = reader.get(key.as_bytes())?;
            assert!(val.is_some(), "key {} should exist", key);
        }

        // Full range scan
        let all = reader.range(b"key_000000", b"key_999999")?;
        assert_eq!(all.len(), count);

        Ok(())
    }

    // -------------------------------------------------------------- //
    //  Reader pool: get_or_open reuses, evict releases
    // -------------------------------------------------------------- //

    #[test]
    fn test_reader_pool_reuse() -> Result<()> {
        let dir = std::env::temp_dir();
        let path = dir.join("mmap_pool_reuse.sst");
        let _cleanup = FileCleanup(&path);

        write_test_sstable(&path, 10)?;

        let pool = MmapReaderPool::new();
        assert!(pool.is_empty());

        let r1 = pool.get_or_open(&path)?;
        assert_eq!(pool.len(), 1);

        let r2 = pool.get_or_open(&path)?;
        assert_eq!(pool.len(), 1);

        // Both Arcs point to the same allocation
        assert!(Arc::ptr_eq(&r1, &r2));

        Ok(())
    }

    #[test]
    fn test_reader_pool_evict() -> Result<()> {
        let dir = std::env::temp_dir();
        let path = dir.join("mmap_pool_evict.sst");
        let _cleanup = FileCleanup(&path);

        write_test_sstable(&path, 10)?;

        let pool = MmapReaderPool::new();
        let _ = pool.get_or_open(&path)?;
        assert_eq!(pool.len(), 1);
        assert!(pool.total_mapped_bytes() > 0);

        assert!(pool.evict(&path));
        assert!(pool.is_empty());
        assert_eq!(pool.total_mapped_bytes(), 0);

        // Evicting again returns false
        assert!(!pool.evict(&path));

        Ok(())
    }

    // -------------------------------------------------------------- //
    //  Concurrent reads from multiple threads
    // -------------------------------------------------------------- //

    #[test]
    fn test_mmap_concurrent_reads() -> Result<()> {
        let dir = std::env::temp_dir();
        let path = dir.join("mmap_concurrent.sst");
        let _cleanup = FileCleanup(&path);

        let count = 200;
        write_test_sstable(&path, count)?;
        let reader = Arc::new(MmapSstableReader::open(&path)?);

        let num_threads = 8;
        let barrier = Arc::new(Barrier::new(num_threads));
        let mut handles = Vec::with_capacity(num_threads);

        for t in 0..num_threads {
            let reader = Arc::clone(&reader);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || -> Result<()> {
                barrier.wait();
                // Each thread reads a different subset
                let start = (t * count) / num_threads;
                let end = ((t + 1) * count) / num_threads;
                for i in start..end {
                    let key = format!("key_{:06}", i);
                    let val = reader.get(key.as_bytes())?;
                    assert!(val.is_some(), "thread {} missing key {}", t, key);
                }
                Ok(())
            }));
        }

        for h in handles {
            h.join()
                .map_err(|_| {
                    AmateRSError::StorageIntegrity(ErrorContext::new(
                        "Thread panicked during concurrent read test".to_string(),
                    ))
                })?
                .map_err(|e| {
                    AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                        "Concurrent read error: {}",
                        e
                    )))
                })?;
        }

        Ok(())
    }

    // -------------------------------------------------------------- //
    //  Concurrent pool access
    // -------------------------------------------------------------- //

    #[test]
    fn test_reader_pool_concurrent() -> Result<()> {
        let dir = std::env::temp_dir();
        let path = dir.join("mmap_pool_concurrent.sst");
        let _cleanup = FileCleanup(&path);

        write_test_sstable(&path, 50)?;
        let pool = Arc::new(MmapReaderPool::new());

        let num_threads = 4;
        let barrier = Arc::new(Barrier::new(num_threads));
        let mut handles = Vec::with_capacity(num_threads);

        for _ in 0..num_threads {
            let pool = Arc::clone(&pool);
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || -> Result<()> {
                barrier.wait();
                let reader = pool.get_or_open(&path)?;
                let val = reader.get(b"key_000000")?;
                assert!(val.is_some());
                Ok(())
            }));
        }

        for h in handles {
            h.join()
                .map_err(|_| {
                    AmateRSError::StorageIntegrity(ErrorContext::new(
                        "Thread panicked in pool concurrent test".to_string(),
                    ))
                })?
                .map_err(|e| {
                    AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                        "Pool concurrent error: {}",
                        e
                    )))
                })?;
        }

        // Only one reader should have been created
        assert_eq!(pool.len(), 1);

        Ok(())
    }

    // -------------------------------------------------------------- //
    //  Cleanup helper
    // -------------------------------------------------------------- //

    /// RAII guard that removes a file when dropped (best-effort).
    struct FileCleanup<'a>(&'a Path);

    impl<'a> Drop for FileCleanup<'a> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
}
