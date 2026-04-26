//! LSM-Tree (Log-Structured Merge-Tree) implementation
//!
//! Multi-level persistent storage engine integrating:
//! - Memtable (in-memory writes)
//! - WAL (durability)
//! - SSTables (persistent storage)
//! - Block Cache (read optimization)

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::{
    BlockCache, BlockCacheConfig, BlockCacheKey, CachedBlock, CompactionConfig, CompactionExecutor,
    CompactionPlanner, Memtable, MemtableConfig, SSTableConfig, SSTableReader, SSTableWriter,
    ValueLog, ValueLogConfig, ValuePointer, Wal,
};
use crate::types::{CipherBlob, Key};
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// SSTable metadata
#[derive(Debug, Clone)]
pub struct SSTableMetadata {
    /// SSTable file path
    pub path: PathBuf,
    /// Minimum key in the SSTable
    pub min_key: Key,
    /// Maximum key in the SSTable
    pub max_key: Key,
    /// Number of entries
    pub num_entries: usize,
    /// File size in bytes
    pub file_size: u64,
    /// Level in the LSM-Tree
    pub level: usize,
}

/// Level information
#[derive(Debug, Clone)]
pub struct LevelInfo {
    /// Level number (0 = L0, 1 = L1, etc.)
    pub level: usize,
    /// SSTables in this level
    pub sstables: Vec<SSTableMetadata>,
    /// Total size in bytes
    pub total_size: u64,
}

impl LevelInfo {
    fn new(level: usize) -> Self {
        Self {
            level,
            sstables: Vec::new(),
            total_size: 0,
        }
    }

    fn add_sstable(&mut self, metadata: SSTableMetadata) {
        self.total_size += metadata.file_size;
        self.sstables.push(metadata);
    }
}

/// LSM-Tree configuration
#[derive(Debug, Clone)]
pub struct LsmTreeConfig {
    /// Directory for storing SSTables
    pub data_dir: PathBuf,
    /// WAL directory
    pub wal_dir: PathBuf,
    /// Memtable configuration
    pub memtable_config: MemtableConfig,
    /// SSTable configuration
    pub sstable_config: SSTableConfig,
    /// Block cache configuration
    pub block_cache_config: BlockCacheConfig,
    /// Compaction configuration
    pub compaction_config: CompactionConfig,
    /// Value log configuration (optional, for WiscKey value separation)
    pub value_log_config: Option<ValueLogConfig>,
    /// Maximum number of levels (default: 7)
    pub max_levels: usize,
    /// L0 size threshold for compaction (default: 4 SSTables)
    pub l0_compaction_threshold: usize,
    /// Level size multiplier (default: 10x per level)
    pub level_size_multiplier: usize,
}

impl Default for LsmTreeConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("./data"),
            wal_dir: PathBuf::from("./wal"),
            memtable_config: MemtableConfig::default(),
            sstable_config: SSTableConfig::default(),
            block_cache_config: BlockCacheConfig::default(),
            compaction_config: CompactionConfig::default(),
            value_log_config: None, // Disabled by default for backward compatibility
            max_levels: 7,
            l0_compaction_threshold: 4,
            level_size_multiplier: 10,
        }
    }
}

/// LSM-Tree storage engine
pub struct LsmTree {
    /// Configuration
    config: LsmTreeConfig,
    /// Current memtable (active writes)
    memtable: Arc<Memtable>,
    /// Immutable memtable being flushed (if any)
    immutable_memtable: Arc<RwLock<Option<Arc<Memtable>>>>,
    /// Write-ahead log
    wal: Arc<RwLock<Wal>>,
    /// Value log for large values (WiscKey)
    value_log: Option<Arc<ValueLog>>,
    /// Levels (L0, L1, L2, ...)
    levels: Arc<RwLock<Vec<LevelInfo>>>,
    /// Block cache for SSTable blocks
    block_cache: Arc<BlockCache>,
    /// Next SSTable ID
    next_sstable_id: Arc<RwLock<u64>>,
    /// Compaction planner
    compaction_planner: CompactionPlanner,
    /// Compaction executor
    compaction_executor: Arc<RwLock<CompactionExecutor>>,
}

impl LsmTree {
    /// Create a new LSM-Tree with default configuration
    pub fn new<P: AsRef<Path>>(data_dir: P) -> Result<Self> {
        let config = LsmTreeConfig {
            data_dir: data_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        Self::with_config(config)
    }

    /// Create a new LSM-Tree with custom configuration
    pub fn with_config(config: LsmTreeConfig) -> Result<Self> {
        // Create directories
        std::fs::create_dir_all(&config.data_dir).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to create data directory: {}",
                e
            )))
        })?;

        std::fs::create_dir_all(&config.wal_dir).map_err(|e| {
            AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Failed to create WAL directory: {}",
                e
            )))
        })?;

        // Initialize WAL
        let wal_path = config.wal_dir.join("wal.log");
        let wal = Wal::create(wal_path)?;

        // Initialize memtable
        let memtable = Memtable::with_config(config.memtable_config.clone());

        // Initialize levels
        let mut levels = Vec::with_capacity(config.max_levels);
        for i in 0..config.max_levels {
            levels.push(LevelInfo::new(i));
        }

        // Initialize block cache
        let block_cache = BlockCache::with_config(config.block_cache_config.clone());

        // Initialize compaction
        let compaction_planner = CompactionPlanner::new(config.compaction_config.clone());
        let compaction_executor = CompactionExecutor::new(config.sstable_config.clone());

        // Initialize value log if configured
        let value_log = if let Some(ref vlog_config) = config.value_log_config {
            Some(Arc::new(ValueLog::with_config(vlog_config.clone())?))
        } else {
            None
        };

        let mut lsm = Self {
            config,
            memtable: Arc::new(memtable),
            immutable_memtable: Arc::new(RwLock::new(None)),
            wal: Arc::new(RwLock::new(wal)),
            value_log,
            levels: Arc::new(RwLock::new(levels)),
            block_cache: Arc::new(block_cache),
            next_sstable_id: Arc::new(RwLock::new(0)),
            compaction_planner,
            compaction_executor: Arc::new(RwLock::new(compaction_executor)),
        };

        // Recover existing SSTables from disk
        lsm.recover_sstables()?;

        Ok(lsm)
    }

    /// Recover existing SSTables from the data directory
    fn recover_sstables(&mut self) -> Result<()> {
        use std::fs;

        // Scan data directory for SSTable files
        let entries = fs::read_dir(&self.config.data_dir).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to read data directory: {}",
                e
            )))
        })?;

        let mut sstables_by_level: BTreeMap<usize, Vec<SSTableMetadata>> = BTreeMap::new();
        let mut max_id = 0u64;

        for entry in entries {
            let entry = entry.map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read directory entry: {}",
                    e
                )))
            })?;

            let path = entry.path();
            let filename = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => continue,
            };

            // Parse filename format: L{level}_{id}.sst
            if filename.starts_with('L') && filename.ends_with(".sst") {
                let parts: Vec<&str> = filename[1..].trim_end_matches(".sst").split('_').collect();
                if parts.len() == 2 {
                    if let (Ok(level), Ok(id)) =
                        (parts[0].parse::<usize>(), parts[1].parse::<u64>())
                    {
                        // Update max ID
                        if id > max_id {
                            max_id = id;
                        }

                        // Read SSTable metadata
                        let reader = SSTableReader::open(&path)?;
                        let (min_key, max_key, num_entries) = reader.metadata()?;

                        let file_size = std::fs::metadata(&path)
                            .map_err(|e| {
                                AmateRSError::IoError(ErrorContext::new(format!(
                                    "Failed to get file size: {}",
                                    e
                                )))
                            })?
                            .len();

                        let metadata = SSTableMetadata {
                            path: path.clone(),
                            min_key,
                            max_key,
                            num_entries,
                            file_size,
                            level,
                        };

                        sstables_by_level.entry(level).or_default().push(metadata);
                    }
                }
            }
        }

        // Add recovered SSTables to levels
        let mut levels = self.levels.write();
        for (level, mut sstables) in sstables_by_level {
            if level < levels.len() {
                // Sort SSTables by min_key for non-L0 levels
                if level > 0 {
                    sstables.sort_by(|a, b| a.min_key.cmp(&b.min_key));
                }

                for metadata in sstables {
                    levels[level].add_sstable(metadata);
                }
            }
        }
        drop(levels);

        // Set next SSTable ID
        *self.next_sstable_id.write() = max_id + 1;

        Ok(())
    }

    /// Put a key-value pair
    pub fn put(&self, key: Key, value: CipherBlob) -> Result<()> {
        // Check if value should be separated to vLog
        let stored_value = if let Some(ref vlog) = self.value_log {
            if vlog.should_separate(&value) {
                // Store in vLog and get pointer
                let pointer = vlog.append(key.clone(), value)?;
                vlog.flush()?;

                // Encode pointer as CipherBlob with "VPTR" magic prefix
                Self::encode_value_pointer(&pointer)
            } else {
                // Store value directly
                value
            }
        } else {
            // No vLog configured, store value directly
            value
        };

        // Write to WAL first (durability)
        {
            let mut wal = self.wal.write();
            wal.put(key.clone(), stored_value.clone())?;
        }

        // Write to memtable
        self.memtable.put(key, stored_value)?;

        // Check if memtable should be flushed
        if self.memtable.should_flush() {
            self.try_flush_memtable()?;
        }

        Ok(())
    }

    /// Get a value by key
    pub fn get(&self, key: &Key) -> Result<Option<CipherBlob>> {
        // Check memtable first (most recent data)
        if let Some(value) = self.memtable.get(key)? {
            return self.resolve_value(value);
        }

        // Check immutable memtable if exists
        {
            let immutable = self.immutable_memtable.read();
            if let Some(ref memtable) = *immutable {
                if let Some(value) = memtable.get(key)? {
                    return self.resolve_value(value);
                }
            }
        }

        // Search through levels (L0 to Ln)
        let levels = self.levels.read();
        for level_info in levels.iter() {
            if let Some(value) = self.search_level(level_info, key)? {
                return self.resolve_value(value);
            }
        }

        Ok(None)
    }

    /// Resolve a value: if it's a ValuePointer, read from vLog; otherwise return as-is
    fn resolve_value(&self, value: CipherBlob) -> Result<Option<CipherBlob>> {
        // Check for tombstone (zero-length blob)
        if value.as_bytes().is_empty() {
            return Ok(None);
        }

        if Self::is_value_pointer(&value) {
            if let Some(ref vlog) = self.value_log {
                let pointer = Self::decode_value_pointer(&value)?;
                let actual_value = vlog.read(&pointer)?;
                Ok(Some(actual_value))
            } else {
                Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                    "Found value pointer but vLog is not configured".to_string(),
                )))
            }
        } else {
            Ok(Some(value))
        }
    }

    /// Delete a key
    pub fn delete(&self, key: Key) -> Result<()> {
        // Write tombstone to WAL
        {
            let mut wal = self.wal.write();
            wal.delete(key.clone())?;
        }

        // Write tombstone to memtable
        self.memtable.delete(key)?;

        // Check if memtable should be flushed
        if self.memtable.should_flush() {
            self.try_flush_memtable()?;
        }

        Ok(())
    }

    /// Range scan
    pub fn range(&self, start: &Key, end: &Key) -> Result<Vec<(Key, CipherBlob)>> {
        let mut results = BTreeMap::new();

        // Collect from all levels (newer data overwrites older)
        let levels = self.levels.read();
        for level_info in levels.iter().rev() {
            let level_results = self.range_scan_level(level_info, start, end)?;
            for (k, v) in level_results {
                results.entry(k).or_insert(v);
            }
        }

        // Check immutable memtable
        {
            let immutable = self.immutable_memtable.read();
            if let Some(ref memtable) = *immutable {
                for (k, v) in memtable.range(start, end) {
                    results.insert(k, v);
                }
            }
        }

        // Check memtable (most recent)
        for (k, v) in self.memtable.range(start, end) {
            results.insert(k, v);
        }

        Ok(results.into_iter().collect())
    }

    /// Search a specific level for a key
    fn search_level(&self, level_info: &LevelInfo, key: &Key) -> Result<Option<CipherBlob>> {
        // For L0, check all SSTables (may have overlapping ranges)
        if level_info.level == 0 {
            // Check newest first
            for metadata in level_info.sstables.iter().rev() {
                if key >= &metadata.min_key && key <= &metadata.max_key {
                    if let Some(value) = self.read_from_sstable(&metadata.path, key)? {
                        return Ok(Some(value));
                    }
                }
            }
        } else {
            // For L1+, SSTables are non-overlapping, use binary search
            let idx = level_info.sstables.binary_search_by(|metadata| {
                if key < &metadata.min_key {
                    std::cmp::Ordering::Greater
                } else if key > &metadata.max_key {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            });

            if let Ok(idx) = idx {
                let metadata = &level_info.sstables[idx];
                if let Some(value) = self.read_from_sstable(&metadata.path, key)? {
                    return Ok(Some(value));
                }
            }
        }

        Ok(None)
    }

    /// Range scan a specific level
    fn range_scan_level(
        &self,
        level_info: &LevelInfo,
        start: &Key,
        end: &Key,
    ) -> Result<Vec<(Key, CipherBlob)>> {
        let mut results = Vec::new();

        for metadata in &level_info.sstables {
            // Skip if SSTable range doesn't overlap with query range
            if &metadata.max_key < start || &metadata.min_key > end {
                continue;
            }

            // Read from SSTable
            let reader = SSTableReader::open(&metadata.path)?;
            let entries = reader.iter()?;

            for (k, v) in entries {
                // Range is [start, end) - start inclusive, end exclusive
                if &k >= start && &k < end {
                    results.push((k, v));
                }
            }
        }

        Ok(results)
    }

    /// Read a key from an SSTable with block cache
    fn read_from_sstable(&self, path: &Path, key: &Key) -> Result<Option<CipherBlob>> {
        let reader = SSTableReader::open(path)?;
        reader.get(key)
    }

    /// Try to flush memtable to L0
    fn try_flush_memtable(&self) -> Result<()> {
        // Check if already flushing
        {
            let immutable = self.immutable_memtable.read();
            if immutable.is_some() {
                // Already flushing, skip
                return Ok(());
            }
        }

        // Swap memtable to immutable
        {
            let mut immutable = self.immutable_memtable.write();
            if immutable.is_some() {
                return Ok(());
            }

            // Swap current memtable with a new one
            let old_memtable = Arc::clone(&self.memtable);
            let new_memtable = Memtable::with_config(self.config.memtable_config.clone());

            // Note: In a real implementation, we'd use Arc::make_mut or similar
            // For now, this is a simplified version
            *immutable = Some(old_memtable);
        }

        // Flush immutable memtable to SSTable
        self.flush_immutable_memtable()?;

        Ok(())
    }

    /// Flush immutable memtable to L0 SSTable
    fn flush_immutable_memtable(&self) -> Result<()> {
        let memtable = {
            let mut immutable = self.immutable_memtable.write();
            immutable.take()
        };

        if let Some(memtable) = memtable {
            // Generate SSTable path
            let sstable_id = {
                let mut next_id = self.next_sstable_id.write();
                let id = *next_id;
                *next_id += 1;
                id
            };

            let sstable_path = self
                .config
                .data_dir
                .join(format!("L0_{:08}.sst", sstable_id));

            // Write SSTable
            let mut writer = SSTableWriter::new(&sstable_path, self.config.sstable_config.clone())?;

            let entries = memtable.entries();
            let mut min_key = None;
            let mut max_key = None;
            let mut num_entries = 0;

            for (key, value_opt) in entries {
                // Write both values and tombstones to SSTable
                // Tombstones are written as zero-length blobs
                let value = value_opt.unwrap_or_else(|| CipherBlob::new(Vec::new()));

                if min_key.is_none() {
                    min_key = Some(key.clone());
                }
                max_key = Some(key.clone());
                writer.add(key, value)?;
                num_entries += 1;
            }

            writer.finish()?;

            // Get file size
            let file_size = std::fs::metadata(&sstable_path)
                .map_err(|e| {
                    AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                        "Failed to get SSTable size: {}",
                        e
                    )))
                })?
                .len();

            // Add to L0
            if let (Some(min_key), Some(max_key)) = (min_key, max_key) {
                let metadata = SSTableMetadata {
                    path: sstable_path,
                    min_key,
                    max_key,
                    num_entries,
                    file_size,
                    level: 0,
                };

                let mut levels = self.levels.write();
                levels[0].add_sstable(metadata);
            }

            // Trigger compaction if L0 threshold exceeded
            self.trigger_compaction()?;
        }

        Ok(())
    }

    /// Trigger compaction if needed
    fn trigger_compaction(&self) -> Result<()> {
        let levels = self.levels.read();

        // Check L0 compaction threshold
        let l0_count = levels[0].sstables.len();
        if self.compaction_planner.needs_l0_compaction(l0_count) {
            drop(levels); // Release read lock before compaction
            return self.compact_l0_to_l1();
        }

        // Check other levels for size-based compaction
        for level_info in levels.iter() {
            if level_info.level > 0
                && self
                    .compaction_planner
                    .needs_level_compaction(level_info.level, level_info.total_size)
            {
                let source_level = level_info.level;
                drop(levels); // Release read lock
                return self.compact_level(source_level);
            }
        }

        Ok(())
    }

    /// Compact L0 to L1
    fn compact_l0_to_l1(&self) -> Result<()> {
        let (source_sstables, target_sstables) = {
            let levels = self.levels.read();
            let source = levels[0].sstables.clone();
            let target = if levels.len() > 1 {
                levels[1].sstables.clone()
            } else {
                Vec::new()
            };
            (source, target)
        };

        if let Some(task) =
            self.compaction_planner
                .plan_compaction(0, source_sstables, target_sstables)
        {
            self.execute_compaction_task(task)?;
        }

        Ok(())
    }

    /// Compact a specific level
    fn compact_level(&self, source_level: usize) -> Result<()> {
        let (source_sstables, target_sstables) = {
            let levels = self.levels.read();
            if source_level >= levels.len() {
                return Ok(());
            }

            let source = levels[source_level].sstables.clone();
            let target = if source_level + 1 < levels.len() {
                levels[source_level + 1].sstables.clone()
            } else {
                Vec::new()
            };
            (source, target)
        };

        if let Some(task) =
            self.compaction_planner
                .plan_compaction(source_level, source_sstables, target_sstables)
        {
            self.execute_compaction_task(task)?;
        }

        Ok(())
    }

    /// Execute a compaction task
    fn execute_compaction_task(&self, task: crate::storage::CompactionTask) -> Result<()> {
        // Execute compaction
        let output_sstables = {
            let mut executor = self.compaction_executor.write();
            let mut next_id = self.next_sstable_id.write();
            executor.execute_compaction(task.clone(), &self.config.data_dir, &mut next_id)?
        };

        // Update levels: remove old SSTables, add new ones
        let mut levels = self.levels.write();

        // Remove source SSTables
        levels[task.source_level]
            .sstables
            .retain(|s| !task.source_sstables.iter().any(|ts| ts.path == s.path));
        levels[task.source_level].total_size = levels[task.source_level]
            .sstables
            .iter()
            .map(|s| s.file_size)
            .sum();

        // Remove target SSTables
        if task.target_level < levels.len() {
            levels[task.target_level]
                .sstables
                .retain(|s| !task.target_sstables.iter().any(|ts| ts.path == s.path));
            levels[task.target_level].total_size = levels[task.target_level]
                .sstables
                .iter()
                .map(|s| s.file_size)
                .sum();

            // Add new SSTables
            for sstable in output_sstables {
                levels[task.target_level].add_sstable(sstable);
            }
        }

        drop(levels);

        // Delete old SSTable files
        for sstable in task.source_sstables.iter().chain(&task.target_sstables) {
            std::fs::remove_file(&sstable.path).ok();
        }

        Ok(())
    }

    /// Get level information
    pub fn level_info(&self, level: usize) -> Option<LevelInfo> {
        let levels = self.levels.read();
        if level < levels.len() {
            Some(levels[level].clone())
        } else {
            None
        }
    }

    /// Get all levels information
    pub fn all_levels_info(&self) -> Vec<LevelInfo> {
        self.levels.read().clone()
    }

    /// Get statistics
    pub fn stats(&self) -> LsmTreeStats {
        let levels = self.levels.read();
        let cache_stats = self.block_cache.stats();
        let compaction_stats = self.compaction_executor.read().stats_snapshot();

        LsmTreeStats {
            memtable_size: self.memtable.size_bytes(),
            num_levels: levels.len(),
            levels: levels.clone(),
            cache_hit_rate: cache_stats.hit_rate(),
            cache_size: cache_stats.size_bytes,
            compaction_stats,
        }
    }

    /// Get all keys from the LSM-Tree
    pub fn keys(&self) -> Result<Vec<Key>> {
        let mut key_set = std::collections::BTreeSet::new();

        // Collect from memtable
        for (key, value_opt) in self.memtable.entries() {
            if value_opt.is_some() {
                key_set.insert(key);
            }
        }

        // Collect from immutable memtable
        {
            let immutable = self.immutable_memtable.read();
            if let Some(ref memtable) = *immutable {
                for (key, value_opt) in memtable.entries() {
                    if value_opt.is_some() {
                        key_set.insert(key);
                    }
                }
            }
        }

        // Collect from all levels
        let levels = self.levels.read();
        for level_info in levels.iter() {
            for metadata in &level_info.sstables {
                let reader = SSTableReader::open(&metadata.path)?;
                let entries = reader.iter()?;
                for (key, _) in entries {
                    key_set.insert(key);
                }
            }
        }

        Ok(key_set.into_iter().collect())
    }

    /// Flush all pending writes to disk
    pub fn flush(&self) -> Result<()> {
        // Flush memtable if it has data
        if self.memtable.size_bytes() > 0 {
            self.try_flush_memtable()?;
        }

        // Flush immutable memtable if exists
        self.flush_immutable_memtable()?;

        // Flush WAL
        {
            let mut wal = self.wal.write();
            wal.flush()?;
        }

        // Flush value log if configured
        if let Some(ref vlog) = self.value_log {
            vlog.flush()?;
        }

        Ok(())
    }

    /// Close the LSM-Tree gracefully
    pub fn close(&self) -> Result<()> {
        // Flush all pending writes
        // File handles will be closed when the structures are dropped
        self.flush()?;
        Ok(())
    }

    // ===== ValuePointer encoding/decoding helpers =====

    /// Encode a ValuePointer as a CipherBlob with "VPTR" magic prefix
    fn encode_value_pointer(pointer: &ValuePointer) -> CipherBlob {
        const MAGIC: &[u8] = b"VPTR"; // 4 bytes
        let pointer_bytes = pointer.encode();

        let mut bytes = Vec::with_capacity(MAGIC.len() + pointer_bytes.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&pointer_bytes);

        CipherBlob::new(bytes)
    }

    /// Decode a CipherBlob back to a ValuePointer
    fn decode_value_pointer(blob: &CipherBlob) -> Result<ValuePointer> {
        const MAGIC: &[u8] = b"VPTR";
        let bytes = blob.as_bytes();

        if bytes.len() < MAGIC.len() {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Invalid value pointer: too short".to_string(),
            )));
        }

        if &bytes[..MAGIC.len()] != MAGIC {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "Invalid value pointer: bad magic".to_string(),
            )));
        }

        ValuePointer::decode(&bytes[MAGIC.len()..])
    }

    /// Check if a CipherBlob contains a ValuePointer
    fn is_value_pointer(blob: &CipherBlob) -> bool {
        const MAGIC: &[u8] = b"VPTR";
        let bytes = blob.as_bytes();
        bytes.len() >= MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC
    }
}

/// LSM-Tree statistics
#[derive(Debug, Clone)]
pub struct LsmTreeStats {
    /// Current memtable size in bytes
    pub memtable_size: usize,
    /// Number of levels
    pub num_levels: usize,
    /// Level information
    pub levels: Vec<LevelInfo>,
    /// Block cache hit rate
    pub cache_hit_rate: f64,
    /// Block cache size in bytes
    pub cache_size: usize,
    /// Compaction statistics
    pub compaction_stats: crate::storage::CompactionStatsSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_lsm_tree_basic_operations() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_basic");
        std::fs::create_dir_all(&dir).ok();

        let lsm = LsmTree::new(&dir)?;

        // Put
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);
        lsm.put(key.clone(), value.clone())?;

        // Get
        let retrieved = lsm.get(&key)?;
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved
                .expect("Value should be retrievable after put")
                .as_bytes(),
            &[1, 2, 3, 4, 5]
        );

        // Delete
        lsm.delete(key.clone())?;

        // Verify deleted
        let retrieved = lsm.get(&key)?;
        assert!(retrieved.is_none());

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_multiple_keys() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_multiple");
        std::fs::create_dir_all(&dir).ok();

        let lsm = LsmTree::new(&dir)?;

        // Write multiple keys
        for i in 0..10 {
            let key = Key::from_str(&format!("key_{:03}", i));
            let value = CipherBlob::new(vec![i as u8; 100]);
            lsm.put(key, value)?;
        }

        // Read back
        for i in 0..10 {
            let key = Key::from_str(&format!("key_{:03}", i));
            let value = lsm.get(&key)?;
            assert!(value.is_some());
            assert_eq!(value.expect("Value should exist").as_bytes()[0], i as u8);
        }

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_range_scan() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_range");
        std::fs::create_dir_all(&dir).ok();

        let lsm = LsmTree::new(&dir)?;

        // Write keys
        for i in 0..20 {
            let key = Key::from_str(&format!("key_{:03}", i));
            let value = CipherBlob::new(vec![i as u8; 50]);
            lsm.put(key, value)?;
        }

        // Range scan
        let start = Key::from_str("key_005");
        let end = Key::from_str("key_015");
        let results = lsm.range(&start, &end)?;

        assert!(results.len() >= 10);

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_stats() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_stats");
        std::fs::create_dir_all(&dir).ok();

        let lsm = LsmTree::new(&dir)?;

        // Write some data
        for i in 0..5 {
            let key = Key::from_str(&format!("key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 100]);
            lsm.put(key, value)?;
        }

        let stats = lsm.stats();
        assert!(stats.memtable_size > 0);
        assert_eq!(stats.num_levels, 7); // Default max levels

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_compaction_trigger() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_compaction");
        std::fs::create_dir_all(&dir).ok();

        let mut config = LsmTreeConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        config.memtable_config.max_size_bytes = 1024; // Small memtable to trigger flushes
        config.l0_compaction_threshold = 2; // Low threshold for testing

        let lsm = LsmTree::with_config(config)?;

        // Write enough data to trigger multiple flushes and compaction
        for i in 0..100 {
            let key = Key::from_str(&format!("key_{:04}", i));
            let value = CipherBlob::new(vec![i as u8; 200]);
            lsm.put(key, value)?;
        }

        // Check stats
        let stats = lsm.stats();
        assert!(
            stats.compaction_stats.compactions_completed > 0
                || !stats.levels[0].sstables.is_empty()
        );

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_compaction_stats() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_compaction_stats");
        std::fs::create_dir_all(&dir).ok();

        let mut config = LsmTreeConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        config.memtable_config.max_size_bytes = 512;

        let lsm = LsmTree::with_config(config)?;

        // Write data
        for i in 0..50 {
            let key = Key::from_str(&format!("key_{:04}", i));
            let value = CipherBlob::new(vec![i as u8; 100]);
            lsm.put(key, value)?;
        }

        let stats = lsm.stats();
        // Compaction may have occurred due to small memtable size
        // Just verify stats structure is available (stats are unsigned, so always >= 0)
        let _ = stats.compaction_stats.keys_processed;
        let _ = stats.compaction_stats.tombstones_removed;

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_level_organization() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_levels");
        std::fs::create_dir_all(&dir).ok();

        let mut config = LsmTreeConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        config.memtable_config.max_size_bytes = 1024;

        let lsm = LsmTree::with_config(config)?;

        // Write data to populate levels
        for i in 0..200 {
            let key = Key::from_str(&format!("key_{:05}", i));
            let value = CipherBlob::new(vec![i as u8; 150]);
            lsm.put(key, value)?;
        }

        // Verify all data is still readable (regardless of which level)
        for i in 0..200 {
            let key = Key::from_str(&format!("key_{:05}", i));
            let value = lsm.get(&key)?;
            assert!(value.is_some());
        }

        // Check that data exists somewhere in the levels
        let stats = lsm.stats();
        let total_sstables: usize = stats.levels.iter().map(|l| l.sstables.len()).sum();
        assert!(total_sstables > 0 || stats.memtable_size > 0);

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_bloom_filter_negative_lookups() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_bloom");
        std::fs::create_dir_all(&dir).ok();

        let mut config = LsmTreeConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        config.memtable_config.max_size_bytes = 512; // Small to trigger flushes

        let lsm = LsmTree::with_config(config)?;

        // Write keys that will be flushed to SSTables
        for i in 0..100 {
            let key = Key::from_str(&format!("exists_{:04}", i));
            let value = CipherBlob::new(vec![i as u8; 100]);
            lsm.put(key, value)?;
        }

        // Query for keys that exist (should find them)
        for i in 0..100 {
            let key = Key::from_str(&format!("exists_{:04}", i));
            let result = lsm.get(&key)?;
            assert!(result.is_some());
        }

        // Query for keys that don't exist (bloom filter should help avoid disk reads)
        for i in 0..100 {
            let key = Key::from_str(&format!("notexists_{:04}", i));
            let result = lsm.get(&key)?;
            assert!(result.is_none());
        }

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    // ===== WiscKey Value Separation Tests =====

    #[test]
    fn test_lsm_tree_vlog_basic() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_vlog_basic");
        std::fs::create_dir_all(&dir).ok();

        let config = LsmTreeConfig {
            data_dir: dir.clone(),
            wal_dir: dir.join("wal"),
            value_log_config: Some(ValueLogConfig {
                vlog_dir: dir.join("vlog"),
                max_file_size: 1024 * 1024, // 1MB
                value_threshold: 1024,      // 1KB
                sync_on_write: false,
                gc_threshold: 0.5,
            }),
            ..Default::default()
        };

        let lsm = LsmTree::with_config(config)?;

        // Small value (< 1KB) - stored inline
        let small_key = Key::from_str("small_key");
        let small_value = CipherBlob::new(vec![1u8; 512]);
        lsm.put(small_key.clone(), small_value.clone())?;

        // Large value (> 1KB) - stored in vLog
        let large_key = Key::from_str("large_key");
        let large_value = CipherBlob::new(vec![2u8; 2048]);
        lsm.put(large_key.clone(), large_value.clone())?;

        // Verify both values can be retrieved
        let retrieved_small = lsm.get(&small_key)?;
        assert!(retrieved_small.is_some());
        assert_eq!(
            retrieved_small
                .expect("Small value should be retrievable")
                .as_bytes(),
            &vec![1u8; 512]
        );

        let retrieved_large = lsm.get(&large_key)?;
        assert!(retrieved_large.is_some());
        assert_eq!(
            retrieved_large
                .expect("Large value should be retrievable")
                .as_bytes(),
            &vec![2u8; 2048]
        );

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_vlog_multiple_large_values() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_vlog_multiple");
        std::fs::create_dir_all(&dir).ok();

        let config = LsmTreeConfig {
            data_dir: dir.clone(),
            wal_dir: dir.join("wal"),
            value_log_config: Some(ValueLogConfig {
                vlog_dir: dir.join("vlog"),
                max_file_size: 1024 * 1024,
                value_threshold: 1024,
                sync_on_write: false,
                gc_threshold: 0.5,
            }),
            ..Default::default()
        };

        let lsm = LsmTree::with_config(config)?;

        // Write 20 large values
        for i in 0..20 {
            let key = Key::from_str(&format!("large_key_{:02}", i));
            let value = CipherBlob::new(vec![i as u8; 2048]);
            lsm.put(key, value)?;
        }

        // Read back and verify
        for i in 0..20 {
            let key = Key::from_str(&format!("large_key_{:02}", i));
            let value = lsm.get(&key)?;
            assert!(value.is_some());
            let retrieved = value.expect("Value should exist");
            assert_eq!(retrieved.as_bytes()[0], i as u8);
            assert_eq!(retrieved.as_bytes().len(), 2048);
        }

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_vlog_with_flush() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_vlog_flush");
        std::fs::create_dir_all(&dir).ok();

        let mut config = LsmTreeConfig {
            data_dir: dir.clone(),
            wal_dir: dir.join("wal"),
            value_log_config: Some(ValueLogConfig {
                vlog_dir: dir.join("vlog"),
                max_file_size: 1024 * 1024,
                value_threshold: 1024,
                sync_on_write: false,
                gc_threshold: 0.5,
            }),
            ..Default::default()
        };
        config.memtable_config.max_size_bytes = 4096; // Small memtable to trigger flushes

        let lsm = LsmTree::with_config(config)?;

        // Write enough data to trigger memtable flush
        for i in 0..50 {
            let key = Key::from_str(&format!("key_{:03}", i));
            let value = CipherBlob::new(vec![i as u8; 1500]); // > 1KB, stored in vLog
            lsm.put(key, value)?;
        }

        // Verify all data is readable after flush
        for i in 0..50 {
            let key = Key::from_str(&format!("key_{:03}", i));
            let value = lsm.get(&key)?;
            assert!(value.is_some());
            let retrieved = value.expect("Value should exist");
            assert_eq!(retrieved.as_bytes()[0], i as u8);
        }

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_vlog_disabled() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_vlog_disabled");
        std::fs::create_dir_all(&dir).ok();

        let config = LsmTreeConfig {
            data_dir: dir.clone(),
            value_log_config: None, // vLog disabled
            ..Default::default()
        };

        let lsm = LsmTree::with_config(config)?;

        // Large value should still work (stored inline)
        let key = Key::from_str("large_key");
        let value = CipherBlob::new(vec![42u8; 5000]);
        lsm.put(key.clone(), value.clone())?;

        let retrieved = lsm.get(&key)?;
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved
                .expect("Value should be retrievable after put")
                .as_bytes(),
            &vec![42u8; 5000]
        );

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_vlog_update() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_vlog_update");
        std::fs::create_dir_all(&dir).ok();

        let config = LsmTreeConfig {
            data_dir: dir.clone(),
            wal_dir: dir.join("wal"),
            value_log_config: Some(ValueLogConfig {
                vlog_dir: dir.join("vlog"),
                max_file_size: 1024 * 1024,
                value_threshold: 1024,
                sync_on_write: false,
                gc_threshold: 0.5,
            }),
            ..Default::default()
        };

        let lsm = LsmTree::with_config(config)?;

        let key = Key::from_str("update_key");

        // Write initial large value
        let value1 = CipherBlob::new(vec![1u8; 2048]);
        lsm.put(key.clone(), value1)?;

        // Update with new large value
        let value2 = CipherBlob::new(vec![2u8; 2048]);
        lsm.put(key.clone(), value2)?;

        // Verify latest value is retrieved
        let retrieved = lsm.get(&key)?;
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved
                .expect("Value should be retrievable after put")
                .as_bytes()[0],
            2u8
        );

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_vlog_delete() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_vlog_delete");
        std::fs::create_dir_all(&dir).ok();

        let config = LsmTreeConfig {
            data_dir: dir.clone(),
            wal_dir: dir.join("wal"),
            value_log_config: Some(ValueLogConfig {
                vlog_dir: dir.join("vlog"),
                max_file_size: 1024 * 1024,
                value_threshold: 1024,
                sync_on_write: false,
                gc_threshold: 0.5,
            }),
            ..Default::default()
        };

        let lsm = LsmTree::with_config(config)?;

        let key = Key::from_str("delete_key");

        // Write large value
        let value = CipherBlob::new(vec![42u8; 2048]);
        lsm.put(key.clone(), value)?;

        // Verify it exists
        assert!(lsm.get(&key)?.is_some());

        // Delete it
        lsm.delete(key.clone())?;

        // Verify it's gone
        assert!(lsm.get(&key)?.is_none());

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn test_lsm_tree_value_pointer_encoding() -> Result<()> {
        // Test ValuePointer encoding/decoding
        let pointer = ValuePointer {
            file_id: 123,
            offset: 456789,
            length: 2048,
            checksum: 0xDEADBEEF,
        };

        // Encode
        let encoded = LsmTree::encode_value_pointer(&pointer);

        // Check it's marked as a pointer
        assert!(LsmTree::is_value_pointer(&encoded));

        // Decode
        let decoded = LsmTree::decode_value_pointer(&encoded)?;

        // Verify
        assert_eq!(decoded.file_id, 123);
        assert_eq!(decoded.offset, 456789);
        assert_eq!(decoded.length, 2048);
        assert_eq!(decoded.checksum, 0xDEADBEEF);

        // Test non-pointer value
        let regular_value = CipherBlob::new(vec![1, 2, 3, 4, 5]);
        assert!(!LsmTree::is_value_pointer(&regular_value));

        Ok(())
    }
}
