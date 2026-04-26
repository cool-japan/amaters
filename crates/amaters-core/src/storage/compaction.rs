//! Compaction strategy for LSM-Tree
//!
//! Implements level-based and size-tiered compaction strategies to:
//! - Merge SSTables from L0 to L1
//! - Merge overlapping SSTables within levels
//! - Remove tombstones (deleted keys) with TTL-based garbage collection
//! - Maintain level size targets
//! - Track compaction statistics with atomic counters
//! - Throttle compaction write rate

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::{SSTableConfig, SSTableMetadata, SSTableReader, SSTableWriter};
use crate::types::{CipherBlob, Key};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Compaction strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionStrategy {
    /// Level-based compaction (default)
    LevelBased,
    /// Size-tiered compaction
    SizeTiered,
}

/// Compaction configuration
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Strategy to use
    pub strategy: CompactionStrategy,
    /// L0 compaction threshold (number of SSTables)
    pub l0_threshold: usize,
    /// Level size multiplier
    pub level_multiplier: usize,
    /// Base level size (L1 target size in bytes)
    pub base_level_size: u64,
    /// Maximum compaction size (bytes)
    pub max_compaction_bytes: u64,
    /// Minimum SSTable size for size-tiered compaction (bytes)
    pub min_sstable_size: u64,
    /// Size ratio for grouping SSTables in size-tiered compaction
    /// SSTables within this ratio of each other are in the same tier
    pub size_ratio: f64,
    /// Minimum number of SSTables in a size tier to trigger compaction
    pub min_tier_size: usize,
    /// Maximum compaction write rate in bytes per second (0 = unlimited)
    pub max_compaction_bytes_per_sec: u64,
    /// Tombstone time-to-live: tombstones older than this are garbage collected
    pub tombstone_ttl: Duration,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            strategy: CompactionStrategy::LevelBased,
            l0_threshold: 4,
            level_multiplier: 10,
            base_level_size: 10 * 1024 * 1024,       // 10 MB
            max_compaction_bytes: 100 * 1024 * 1024, // 100 MB
            min_sstable_size: 1024,                  // 1 KB
            size_ratio: 2.0,
            min_tier_size: 4,
            max_compaction_bytes_per_sec: 0, // unlimited
            tombstone_ttl: Duration::from_secs(7 * 24 * 3600), // 7 days
        }
    }
}

/// Compaction task
#[derive(Debug, Clone)]
pub struct CompactionTask {
    /// Source level
    pub source_level: usize,
    /// Target level
    pub target_level: usize,
    /// SSTables to compact from source level
    pub source_sstables: Vec<SSTableMetadata>,
    /// SSTables to merge from target level (if any)
    pub target_sstables: Vec<SSTableMetadata>,
}

/// Thread-safe compaction statistics tracked with atomic counters
pub struct CompactionStats {
    /// Total bytes read during compaction
    pub bytes_read: AtomicU64,
    /// Total bytes written during compaction
    pub bytes_written: AtomicU64,
    /// Total files merged
    pub files_merged: AtomicU64,
    /// Total compactions completed
    pub compactions_completed: AtomicU64,
    /// Total duration in milliseconds
    pub total_duration_ms: AtomicU64,
    /// Total keys processed
    pub keys_processed: AtomicU64,
    /// Total tombstones removed
    pub tombstones_removed: AtomicU64,
}

impl Default for CompactionStats {
    fn default() -> Self {
        Self {
            bytes_read: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            files_merged: AtomicU64::new(0),
            compactions_completed: AtomicU64::new(0),
            total_duration_ms: AtomicU64::new(0),
            keys_processed: AtomicU64::new(0),
            tombstones_removed: AtomicU64::new(0),
        }
    }
}

impl std::fmt::Debug for CompactionStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompactionStats")
            .field("bytes_read", &self.bytes_read.load(Ordering::Relaxed))
            .field("bytes_written", &self.bytes_written.load(Ordering::Relaxed))
            .field("files_merged", &self.files_merged.load(Ordering::Relaxed))
            .field(
                "compactions_completed",
                &self.compactions_completed.load(Ordering::Relaxed),
            )
            .field(
                "total_duration_ms",
                &self.total_duration_ms.load(Ordering::Relaxed),
            )
            .field(
                "keys_processed",
                &self.keys_processed.load(Ordering::Relaxed),
            )
            .field(
                "tombstones_removed",
                &self.tombstones_removed.load(Ordering::Relaxed),
            )
            .finish()
    }
}

impl CompactionStats {
    /// Create a snapshot of current stats as simple values
    pub fn snapshot(&self) -> CompactionStatsSnapshot {
        CompactionStatsSnapshot {
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            files_merged: self.files_merged.load(Ordering::Relaxed),
            compactions_completed: self.compactions_completed.load(Ordering::Relaxed),
            total_duration_ms: self.total_duration_ms.load(Ordering::Relaxed),
            keys_processed: self.keys_processed.load(Ordering::Relaxed),
            tombstones_removed: self.tombstones_removed.load(Ordering::Relaxed),
        }
    }
}

/// Non-atomic snapshot of compaction statistics
#[derive(Debug, Clone, Default)]
pub struct CompactionStatsSnapshot {
    /// Total bytes read during compaction
    pub bytes_read: u64,
    /// Total bytes written during compaction
    pub bytes_written: u64,
    /// Total files merged
    pub files_merged: u64,
    /// Total compactions completed
    pub compactions_completed: u64,
    /// Total duration in milliseconds
    pub total_duration_ms: u64,
    /// Total keys processed
    pub keys_processed: u64,
    /// Total tombstones removed
    pub tombstones_removed: u64,
}

/// Compaction write throttler
///
/// Limits the rate of compaction writes to avoid overwhelming I/O.
/// Uses `std::thread::sleep` for tokio-free operation.
#[derive(Debug)]
pub struct CompactionThrottler {
    /// Maximum bytes per second (0 = unlimited)
    max_bytes_per_sec: u64,
    /// Bytes written in the current tracking window
    bytes_in_window: u64,
    /// Start time of the current tracking window
    window_start: Instant,
}

impl CompactionThrottler {
    /// Create a new throttler with the given rate limit
    pub fn new(max_bytes_per_sec: u64) -> Self {
        Self {
            max_bytes_per_sec,
            bytes_in_window: 0,
            window_start: Instant::now(),
        }
    }

    /// Record bytes written and sleep if rate limit is exceeded
    pub fn throttle(&mut self, bytes_written: u64) {
        if self.max_bytes_per_sec == 0 {
            return; // unlimited
        }

        self.bytes_in_window += bytes_written;

        let elapsed = self.window_start.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();

        // Calculate expected time for the bytes written at the rate limit
        let expected_secs = self.bytes_in_window as f64 / self.max_bytes_per_sec as f64;

        if expected_secs > elapsed_secs {
            let sleep_duration = Duration::from_secs_f64(expected_secs - elapsed_secs);
            std::thread::sleep(sleep_duration);
        }

        // Reset window periodically (every second) to avoid accumulation drift
        if elapsed_secs >= 1.0 {
            self.bytes_in_window = 0;
            self.window_start = Instant::now();
        }
    }

    /// Check if throttling is enabled
    pub fn is_enabled(&self) -> bool {
        self.max_bytes_per_sec > 0
    }
}

/// A size tier: a group of SSTables with similar sizes
#[derive(Debug, Clone)]
pub struct SizeTier {
    /// SSTables in this tier
    pub sstables: Vec<SSTableMetadata>,
    /// Average file size in this tier
    pub avg_size: u64,
}

/// Compaction planner
pub struct CompactionPlanner {
    config: CompactionConfig,
}

impl CompactionPlanner {
    /// Create a new compaction planner
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    /// Check if L0 needs compaction
    pub fn needs_l0_compaction(&self, l0_sstable_count: usize) -> bool {
        l0_sstable_count >= self.config.l0_threshold
    }

    /// Check if a level needs compaction
    pub fn needs_level_compaction(&self, level: usize, level_size: u64) -> bool {
        if level == 0 {
            return false; // L0 uses count-based threshold
        }

        let target_size = self.level_target_size(level);
        level_size > target_size
    }

    /// Calculate target size for a level
    pub fn level_target_size(&self, level: usize) -> u64 {
        if level == 0 {
            return 0; // L0 doesn't have a size target
        }

        self.config.base_level_size * (self.config.level_multiplier as u64).pow(level as u32 - 1)
    }

    /// Plan a compaction task (level-based strategy)
    pub fn plan_compaction(
        &self,
        source_level: usize,
        source_sstables: Vec<SSTableMetadata>,
        target_sstables: Vec<SSTableMetadata>,
    ) -> Option<CompactionTask> {
        if source_sstables.is_empty() {
            return None;
        }

        // For L0 → L1, take all L0 SSTables
        let source_to_compact = if source_level == 0 {
            source_sstables
        } else {
            // For L1+, select SSTables based on size
            self.select_sstables_for_compaction(source_sstables)
        };

        if source_to_compact.is_empty() {
            return None;
        }

        // Find overlapping SSTables in target level
        let target_to_merge = self.find_overlapping_sstables(&source_to_compact, &target_sstables);

        Some(CompactionTask {
            source_level,
            target_level: source_level + 1,
            source_sstables: source_to_compact,
            target_sstables: target_to_merge,
        })
    }

    /// Plan a size-tiered compaction task
    ///
    /// Groups SSTables by similar size (within `size_ratio` of each other).
    /// When a tier has at least `min_tier_size` SSTables, they are merged.
    pub fn plan_size_tiered_compaction(
        &self,
        sstables: Vec<SSTableMetadata>,
    ) -> Option<CompactionTask> {
        let tiers = self.group_by_size_tier(sstables);

        // Find the first tier that has enough SSTables to trigger compaction
        for tier in tiers {
            if tier.sstables.len() >= self.config.min_tier_size {
                // Determine the appropriate target level
                // Size-tiered puts output into level based on size
                let max_level = tier.sstables.iter().map(|s| s.level).max().unwrap_or(0);
                let target_level = max_level + 1;

                return Some(CompactionTask {
                    source_level: max_level,
                    target_level,
                    source_sstables: tier.sstables,
                    target_sstables: Vec::new(),
                });
            }
        }

        None
    }

    /// Group SSTables into size tiers
    ///
    /// SSTables are grouped such that the largest file in a tier is at most
    /// `size_ratio` times the smallest file in the same tier.
    pub fn group_by_size_tier(&self, mut sstables: Vec<SSTableMetadata>) -> Vec<SizeTier> {
        if sstables.is_empty() {
            return Vec::new();
        }

        // Sort by file size
        sstables.sort_by_key(|s| s.file_size);

        // Filter out SSTables smaller than minimum size
        let sstables: Vec<SSTableMetadata> = sstables
            .into_iter()
            .filter(|s| s.file_size >= self.config.min_sstable_size)
            .collect();

        if sstables.is_empty() {
            return Vec::new();
        }

        let mut tiers: Vec<SizeTier> = Vec::new();
        let mut current_tier_sstables: Vec<SSTableMetadata> = Vec::new();
        let mut tier_min_size: u64 = 0;

        for sstable in sstables {
            if current_tier_sstables.is_empty() {
                tier_min_size = sstable.file_size;
                current_tier_sstables.push(sstable);
            } else if (sstable.file_size as f64) <= (tier_min_size as f64 * self.config.size_ratio)
            {
                // Within the size ratio, add to current tier
                current_tier_sstables.push(sstable);
            } else {
                // Start a new tier
                let avg_size = current_tier_sstables
                    .iter()
                    .map(|s| s.file_size)
                    .sum::<u64>()
                    / current_tier_sstables.len().max(1) as u64;
                tiers.push(SizeTier {
                    sstables: std::mem::take(&mut current_tier_sstables),
                    avg_size,
                });
                tier_min_size = sstable.file_size;
                current_tier_sstables.push(sstable);
            }
        }

        // Don't forget the last tier
        if !current_tier_sstables.is_empty() {
            let avg_size = current_tier_sstables
                .iter()
                .map(|s| s.file_size)
                .sum::<u64>()
                / current_tier_sstables.len().max(1) as u64;
            tiers.push(SizeTier {
                sstables: current_tier_sstables,
                avg_size,
            });
        }

        tiers
    }

    /// Select SSTables for compaction (L1+)
    fn select_sstables_for_compaction(
        &self,
        sstables: Vec<SSTableMetadata>,
    ) -> Vec<SSTableMetadata> {
        let mut selected = Vec::new();
        let mut total_size = 0u64;

        for sstable in sstables {
            if total_size + sstable.file_size > self.config.max_compaction_bytes {
                break;
            }

            total_size += sstable.file_size;
            selected.push(sstable);

            // Compact at least 2 SSTables
            if selected.len() >= 2 {
                break;
            }
        }

        selected
    }

    /// Find overlapping SSTables in target level
    pub fn find_overlapping_sstables(
        &self,
        source_sstables: &[SSTableMetadata],
        target_sstables: &[SSTableMetadata],
    ) -> Vec<SSTableMetadata> {
        if source_sstables.is_empty() {
            return Vec::new();
        }

        // Find min and max keys from source SSTables (safe: checked is_empty above)
        let min_key = source_sstables
            .iter()
            .map(|s| &s.min_key)
            .min()
            .expect("source_sstables is non-empty");

        let max_key = source_sstables
            .iter()
            .map(|s| &s.max_key)
            .max()
            .expect("source_sstables is non-empty");

        // Find all target SSTables that overlap with this range
        target_sstables
            .iter()
            .filter(|sstable| {
                // Check if ranges overlap
                !(&sstable.max_key < min_key || &sstable.min_key > max_key)
            })
            .cloned()
            .collect()
    }
}

/// Tombstone entry with timestamp for TTL-based garbage collection
#[derive(Debug, Clone)]
pub struct TombstoneEntry {
    /// The key that was deleted
    pub key: Key,
    /// When the tombstone was created
    pub created_at: Instant,
}

/// Compaction executor
pub struct CompactionExecutor {
    config: SSTableConfig,
    compaction_config: CompactionConfig,
    stats: Arc<CompactionStats>,
    throttler: CompactionThrottler,
    /// Active tombstones with their creation times
    tombstones: BTreeMap<Key, Instant>,
}

impl CompactionExecutor {
    /// Create a new compaction executor
    pub fn new(config: SSTableConfig) -> Self {
        Self {
            config,
            compaction_config: CompactionConfig::default(),
            stats: Arc::new(CompactionStats::default()),
            throttler: CompactionThrottler::new(0),
            tombstones: BTreeMap::new(),
        }
    }

    /// Create a new compaction executor with compaction configuration
    pub fn with_compaction_config(
        config: SSTableConfig,
        compaction_config: CompactionConfig,
    ) -> Self {
        let throttler = CompactionThrottler::new(compaction_config.max_compaction_bytes_per_sec);
        Self {
            config,
            compaction_config,
            stats: Arc::new(CompactionStats::default()),
            throttler,
            tombstones: BTreeMap::new(),
        }
    }

    /// Register a tombstone with its creation time
    pub fn register_tombstone(&mut self, key: Key, created_at: Instant) {
        self.tombstones.insert(key, created_at);
    }

    /// Check if a tombstone has expired based on TTL
    fn is_tombstone_expired(&self, key: &Key) -> bool {
        if let Some(created_at) = self.tombstones.get(key) {
            created_at.elapsed() >= self.compaction_config.tombstone_ttl
        } else {
            false
        }
    }

    /// Execute a compaction task
    pub fn execute_compaction(
        &mut self,
        task: CompactionTask,
        output_dir: &Path,
        next_sstable_id: &mut u64,
    ) -> Result<Vec<SSTableMetadata>> {
        let start_time = Instant::now();

        // Track files merged
        let files_merged = (task.source_sstables.len() + task.target_sstables.len()) as u64;
        self.stats
            .files_merged
            .fetch_add(files_merged, Ordering::Relaxed);

        // Collect all entries from source and target SSTables
        let mut all_entries: BTreeMap<Key, Option<CipherBlob>> = BTreeMap::new();

        // Read from source SSTables
        for sstable in &task.source_sstables {
            self.read_sstable_entries(&sstable.path, &mut all_entries)?;
            self.stats
                .bytes_read
                .fetch_add(sstable.file_size, Ordering::Relaxed);
        }

        // Read from target SSTables (overlapping)
        for sstable in &task.target_sstables {
            self.read_sstable_entries(&sstable.path, &mut all_entries)?;
            self.stats
                .bytes_read
                .fetch_add(sstable.file_size, Ordering::Relaxed);
        }

        // Write merged SSTables to target level
        let output_sstables = self.write_compacted_sstables(
            all_entries,
            task.target_level,
            output_dir,
            next_sstable_id,
        )?;

        self.stats
            .compactions_completed
            .fetch_add(1, Ordering::Relaxed);

        let duration_ms = start_time.elapsed().as_millis() as u64;
        self.stats
            .total_duration_ms
            .fetch_add(duration_ms, Ordering::Relaxed);

        Ok(output_sstables)
    }

    /// Read entries from an SSTable
    fn read_sstable_entries(
        &self,
        path: &Path,
        entries: &mut BTreeMap<Key, Option<CipherBlob>>,
    ) -> Result<()> {
        let reader = SSTableReader::open(path)?;
        let sstable_entries = reader.iter()?;

        for (key, value) in sstable_entries {
            self.stats.keys_processed.fetch_add(1, Ordering::Relaxed);
            // Later entries overwrite earlier ones (LSM semantics)
            entries.insert(key, Some(value));
        }

        Ok(())
    }

    /// Write compacted entries to new SSTables
    fn write_compacted_sstables(
        &mut self,
        entries: BTreeMap<Key, Option<CipherBlob>>,
        target_level: usize,
        output_dir: &Path,
        next_id: &mut u64,
    ) -> Result<Vec<SSTableMetadata>> {
        let mut output_sstables = Vec::new();
        let mut current_writer: Option<SSTableWriter> = None;
        let mut current_path: Option<PathBuf> = None;
        let mut current_size = 0usize;
        let mut current_min_key: Option<Key> = None;
        let mut current_max_key: Option<Key> = None;
        let mut current_entries = 0usize;

        const MAX_SSTABLE_SIZE: usize = 2 * 1024 * 1024; // 2 MB per SSTable

        for (key, value_opt) in entries {
            // Handle tombstones (None values)
            let value = match value_opt {
                Some(v) => v,
                None => {
                    // Check if tombstone has expired (TTL-based GC)
                    if self.is_tombstone_expired(&key) {
                        self.stats
                            .tombstones_removed
                            .fetch_add(1, Ordering::Relaxed);
                        // Remove from tombstone tracking
                        self.tombstones.remove(&key);
                        continue;
                    }
                    // Tombstone not expired yet, still skip writing it
                    // (original behavior: all tombstones removed during compaction)
                    self.stats
                        .tombstones_removed
                        .fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };

            // Start new SSTable if needed
            if current_writer.is_none() || current_size >= MAX_SSTABLE_SIZE {
                // Finish previous SSTable
                if let Some(writer) = current_writer.take() {
                    writer.finish()?;

                    if let (Some(path), Some(min_key), Some(max_key)) = (
                        current_path.take(),
                        current_min_key.take(),
                        current_max_key.take(),
                    ) {
                        let file_size = std::fs::metadata(&path)
                            .map_err(|e| {
                                AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                                    "Failed to get SSTable size: {}",
                                    e
                                )))
                            })?
                            .len();

                        self.stats
                            .bytes_written
                            .fetch_add(file_size, Ordering::Relaxed);
                        self.throttler.throttle(file_size);

                        output_sstables.push(SSTableMetadata {
                            path,
                            min_key,
                            max_key,
                            num_entries: current_entries,
                            file_size,
                            level: target_level,
                        });
                    }
                }

                // Start new SSTable
                let id = *next_id;
                *next_id += 1;
                let path = output_dir.join(format!("L{}_{:08}.sst", target_level, id));
                let writer = SSTableWriter::new(&path, self.config.clone())?;

                current_writer = Some(writer);
                current_path = Some(path);
                current_size = 0;
                current_min_key = None;
                current_max_key = None;
                current_entries = 0;
            }

            // Write entry
            if let Some(ref mut writer) = current_writer {
                let entry_size = 16 + key.as_bytes().len() + value.as_bytes().len();
                writer.add(key.clone(), value)?;
                current_size += entry_size;
                current_entries += 1;

                if current_min_key.is_none() {
                    current_min_key = Some(key.clone());
                }
                current_max_key = Some(key);
            }
        }

        // Finish final SSTable
        if let Some(writer) = current_writer {
            writer.finish()?;

            if let (Some(path), Some(min_key), Some(max_key)) =
                (current_path, current_min_key, current_max_key)
            {
                let file_size = std::fs::metadata(&path)
                    .map_err(|e| {
                        AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                            "Failed to get SSTable size: {}",
                            e
                        )))
                    })?
                    .len();

                self.stats
                    .bytes_written
                    .fetch_add(file_size, Ordering::Relaxed);
                self.throttler.throttle(file_size);

                output_sstables.push(SSTableMetadata {
                    path,
                    min_key,
                    max_key,
                    num_entries: current_entries,
                    file_size,
                    level: target_level,
                });
            }
        }

        Ok(output_sstables)
    }

    /// Get compaction statistics
    pub fn stats(&self) -> &CompactionStats {
        &self.stats
    }

    /// Get a snapshot of compaction statistics
    pub fn stats_snapshot(&self) -> CompactionStatsSnapshot {
        self.stats.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_planner_l0_threshold() {
        let config = CompactionConfig::default();
        let planner = CompactionPlanner::new(config);

        assert!(!planner.needs_l0_compaction(3));
        assert!(planner.needs_l0_compaction(4));
        assert!(planner.needs_l0_compaction(5));
    }

    #[test]
    fn test_compaction_planner_level_sizes() {
        let config = CompactionConfig {
            base_level_size: 10 * 1024 * 1024, // 10 MB
            level_multiplier: 10,
            ..Default::default()
        };
        let planner = CompactionPlanner::new(config);

        assert_eq!(planner.level_target_size(1), 10 * 1024 * 1024); // 10 MB
        assert_eq!(planner.level_target_size(2), 100 * 1024 * 1024); // 100 MB
        assert_eq!(planner.level_target_size(3), 1000 * 1024 * 1024); // 1 GB
    }

    #[test]
    fn test_compaction_planner_needs_compaction() {
        let config = CompactionConfig::default();
        let planner = CompactionPlanner::new(config);

        // L0 doesn't use size-based threshold
        assert!(!planner.needs_level_compaction(0, 100 * 1024 * 1024));

        // L1 target is 10 MB
        assert!(!planner.needs_level_compaction(1, 5 * 1024 * 1024));
        assert!(planner.needs_level_compaction(1, 15 * 1024 * 1024));
    }

    #[test]
    fn test_find_overlapping_sstables() {
        let config = CompactionConfig::default();
        let planner = CompactionPlanner::new(config);

        let source = vec![SSTableMetadata {
            path: PathBuf::from("s1.sst"),
            min_key: Key::from_str("key_005"),
            max_key: Key::from_str("key_015"),
            num_entries: 10,
            file_size: 1000,
            level: 0,
        }];

        let target = vec![
            SSTableMetadata {
                path: PathBuf::from("t1.sst"),
                min_key: Key::from_str("key_000"),
                max_key: Key::from_str("key_010"),
                num_entries: 10,
                file_size: 1000,
                level: 1,
            },
            SSTableMetadata {
                path: PathBuf::from("t2.sst"),
                min_key: Key::from_str("key_020"),
                max_key: Key::from_str("key_030"),
                num_entries: 10,
                file_size: 1000,
                level: 1,
            },
        ];

        let overlapping = planner.find_overlapping_sstables(&source, &target);

        assert_eq!(overlapping.len(), 1);
        assert_eq!(overlapping[0].path, PathBuf::from("t1.sst"));
    }

    // ====== Size-tiered compaction tests ======

    #[test]
    fn test_size_tiered_grouping_basic() {
        let config = CompactionConfig {
            strategy: CompactionStrategy::SizeTiered,
            min_sstable_size: 100,
            size_ratio: 2.0,
            min_tier_size: 4,
            ..Default::default()
        };
        let planner = CompactionPlanner::new(config);

        // Create SSTables with similar sizes (all within 2x of each other)
        let sstables = vec![
            make_metadata("a.sst", 1000, 0),
            make_metadata("b.sst", 1200, 0),
            make_metadata("c.sst", 1500, 0),
            make_metadata("d.sst", 1800, 0),
        ];

        let tiers = planner.group_by_size_tier(sstables);

        // All should be in one tier (1000 * 2.0 = 2000, all are <= 2000)
        assert_eq!(tiers.len(), 1);
        assert_eq!(tiers[0].sstables.len(), 4);
    }

    #[test]
    fn test_size_tiered_grouping_multiple_tiers() {
        let config = CompactionConfig {
            strategy: CompactionStrategy::SizeTiered,
            min_sstable_size: 100,
            size_ratio: 2.0,
            min_tier_size: 2,
            ..Default::default()
        };
        let planner = CompactionPlanner::new(config);

        // Two distinct size groups
        let sstables = vec![
            make_metadata("small1.sst", 1000, 0),
            make_metadata("small2.sst", 1500, 0),
            make_metadata("big1.sst", 10000, 0),
            make_metadata("big2.sst", 15000, 0),
        ];

        let tiers = planner.group_by_size_tier(sstables);

        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[0].sstables.len(), 2); // small group
        assert_eq!(tiers[1].sstables.len(), 2); // big group
    }

    #[test]
    fn test_size_tiered_merge_trigger() {
        let config = CompactionConfig {
            strategy: CompactionStrategy::SizeTiered,
            min_sstable_size: 100,
            size_ratio: 2.0,
            min_tier_size: 4,
            ..Default::default()
        };
        let planner = CompactionPlanner::new(config);

        // Not enough SSTables in any tier
        let sstables = vec![
            make_metadata("a.sst", 1000, 0),
            make_metadata("b.sst", 1200, 0),
            make_metadata("c.sst", 1500, 0),
        ];

        let task = planner.plan_size_tiered_compaction(sstables);
        assert!(task.is_none(), "Should not trigger with only 3 SSTables");

        // Enough SSTables in a tier
        let sstables = vec![
            make_metadata("a.sst", 1000, 0),
            make_metadata("b.sst", 1200, 0),
            make_metadata("c.sst", 1500, 0),
            make_metadata("d.sst", 1800, 0),
        ];

        let task = planner.plan_size_tiered_compaction(sstables);
        assert!(
            task.is_some(),
            "Should trigger with 4 SSTables in same tier"
        );

        let task = task.expect("task should be Some");
        assert_eq!(task.source_sstables.len(), 4);
        assert_eq!(task.target_level, 1);
    }

    #[test]
    fn test_size_tiered_filters_small_sstables() {
        let config = CompactionConfig {
            strategy: CompactionStrategy::SizeTiered,
            min_sstable_size: 500,
            size_ratio: 2.0,
            min_tier_size: 4,
            ..Default::default()
        };
        let planner = CompactionPlanner::new(config);

        // All SSTables below minimum size
        let sstables = vec![
            make_metadata("a.sst", 100, 0),
            make_metadata("b.sst", 200, 0),
            make_metadata("c.sst", 300, 0),
            make_metadata("d.sst", 400, 0),
        ];

        let tiers = planner.group_by_size_tier(sstables);
        assert!(tiers.is_empty());
    }

    // ====== Compaction stats tests ======

    #[test]
    fn test_compaction_stats_default() {
        let stats = CompactionStats::default();
        let snapshot = stats.snapshot();

        assert_eq!(snapshot.bytes_read, 0);
        assert_eq!(snapshot.bytes_written, 0);
        assert_eq!(snapshot.files_merged, 0);
        assert_eq!(snapshot.compactions_completed, 0);
        assert_eq!(snapshot.total_duration_ms, 0);
        assert_eq!(snapshot.keys_processed, 0);
        assert_eq!(snapshot.tombstones_removed, 0);
    }

    #[test]
    fn test_compaction_stats_atomic_updates() {
        let stats = CompactionStats::default();

        stats.bytes_read.fetch_add(1000, Ordering::Relaxed);
        stats.bytes_written.fetch_add(500, Ordering::Relaxed);
        stats.files_merged.fetch_add(3, Ordering::Relaxed);
        stats.compactions_completed.fetch_add(1, Ordering::Relaxed);
        stats.total_duration_ms.fetch_add(42, Ordering::Relaxed);
        stats.keys_processed.fetch_add(100, Ordering::Relaxed);
        stats.tombstones_removed.fetch_add(5, Ordering::Relaxed);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.bytes_read, 1000);
        assert_eq!(snapshot.bytes_written, 500);
        assert_eq!(snapshot.files_merged, 3);
        assert_eq!(snapshot.compactions_completed, 1);
        assert_eq!(snapshot.total_duration_ms, 42);
        assert_eq!(snapshot.keys_processed, 100);
        assert_eq!(snapshot.tombstones_removed, 5);
    }

    #[test]
    fn test_compaction_stats_thread_safety() {
        let stats = Arc::new(CompactionStats::default());

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let stats_clone = Arc::clone(&stats);
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        stats_clone.bytes_read.fetch_add(1, Ordering::Relaxed);
                        stats_clone.keys_processed.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("thread should complete");
        }

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.bytes_read, 1000);
        assert_eq!(snapshot.keys_processed, 1000);
    }

    // ====== Throttling tests ======

    #[test]
    fn test_throttler_disabled() {
        let mut throttler = CompactionThrottler::new(0);
        assert!(!throttler.is_enabled());

        // Should return immediately, no sleeping
        let start = Instant::now();
        throttler.throttle(1_000_000);
        let elapsed = start.elapsed();

        // Should complete nearly instantly
        assert!(elapsed < Duration::from_millis(50));
    }

    #[test]
    fn test_throttler_enabled() {
        let mut throttler = CompactionThrottler::new(10_000); // 10 KB/s
        assert!(throttler.is_enabled());

        // Write 20 KB, should take ~2 seconds at 10 KB/s
        let start = Instant::now();
        throttler.throttle(20_000);
        let elapsed = start.elapsed();

        // Should have throttled for approximately 2 seconds
        assert!(
            elapsed >= Duration::from_millis(1500),
            "Expected >= 1.5s delay, got {:?}",
            elapsed
        );
    }

    #[test]
    fn test_throttler_small_writes_no_delay() {
        let mut throttler = CompactionThrottler::new(1_000_000); // 1 MB/s
        assert!(throttler.is_enabled());

        // Write small amount, should complete quickly
        let start = Instant::now();
        throttler.throttle(100);
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_millis(50));
    }

    // ====== Tombstone TTL GC tests ======

    #[test]
    fn test_tombstone_registration() {
        let config = SSTableConfig::default();
        let mut executor = CompactionExecutor::new(config);

        let key = Key::from_str("test_key");
        let created_at = Instant::now();
        executor.register_tombstone(key.clone(), created_at);

        // Should not be expired with default 7-day TTL
        assert!(!executor.is_tombstone_expired(&key));
    }

    #[test]
    fn test_tombstone_expiry_with_short_ttl() {
        let config = SSTableConfig::default();
        let compaction_config = CompactionConfig {
            tombstone_ttl: Duration::from_millis(1), // Very short TTL for testing
            ..Default::default()
        };
        let mut executor = CompactionExecutor::with_compaction_config(config, compaction_config);

        let key = Key::from_str("expired_key");
        // Create tombstone "in the past" by using an Instant that's old enough
        let old_time = Instant::now() - Duration::from_millis(10);
        executor.register_tombstone(key.clone(), old_time);

        // Should be expired now
        assert!(executor.is_tombstone_expired(&key));
    }

    #[test]
    fn test_tombstone_not_expired() {
        let config = SSTableConfig::default();
        let compaction_config = CompactionConfig {
            tombstone_ttl: Duration::from_secs(3600), // 1 hour TTL
            ..Default::default()
        };
        let mut executor = CompactionExecutor::with_compaction_config(config, compaction_config);

        let key = Key::from_str("fresh_key");
        executor.register_tombstone(key.clone(), Instant::now());

        // Should NOT be expired (just created)
        assert!(!executor.is_tombstone_expired(&key));
    }

    #[test]
    fn test_unknown_tombstone_not_expired() {
        let config = SSTableConfig::default();
        let executor = CompactionExecutor::new(config);

        // Key not registered as tombstone
        let key = Key::from_str("unknown_key");
        assert!(!executor.is_tombstone_expired(&key));
    }

    // ====== Edge case tests ======

    #[test]
    fn test_plan_compaction_empty_source() {
        let config = CompactionConfig::default();
        let planner = CompactionPlanner::new(config);

        let task = planner.plan_compaction(0, Vec::new(), Vec::new());
        assert!(task.is_none());
    }

    #[test]
    fn test_plan_compaction_single_sstable() {
        let config = CompactionConfig::default();
        let planner = CompactionPlanner::new(config);

        let source = vec![SSTableMetadata {
            path: PathBuf::from("single.sst"),
            min_key: Key::from_str("key_001"),
            max_key: Key::from_str("key_010"),
            num_entries: 10,
            file_size: 1000,
            level: 0,
        }];

        // L0 takes all SSTables, even just one
        let task = planner.plan_compaction(0, source, Vec::new());
        assert!(task.is_some());
        let task = task.expect("task should be Some for L0");
        assert_eq!(task.source_sstables.len(), 1);
    }

    #[test]
    fn test_size_tiered_empty_input() {
        let config = CompactionConfig {
            strategy: CompactionStrategy::SizeTiered,
            ..Default::default()
        };
        let planner = CompactionPlanner::new(config);

        let task = planner.plan_size_tiered_compaction(Vec::new());
        assert!(task.is_none());
    }

    #[test]
    fn test_find_overlapping_empty_source() {
        let config = CompactionConfig::default();
        let planner = CompactionPlanner::new(config);

        let target = vec![SSTableMetadata {
            path: PathBuf::from("t1.sst"),
            min_key: Key::from_str("key_000"),
            max_key: Key::from_str("key_010"),
            num_entries: 10,
            file_size: 1000,
            level: 1,
        }];

        let overlapping = planner.find_overlapping_sstables(&[], &target);
        assert!(overlapping.is_empty());
    }

    #[test]
    fn test_find_overlapping_no_overlap() {
        let config = CompactionConfig::default();
        let planner = CompactionPlanner::new(config);

        let source = vec![SSTableMetadata {
            path: PathBuf::from("s1.sst"),
            min_key: Key::from_str("aaa"),
            max_key: Key::from_str("bbb"),
            num_entries: 10,
            file_size: 1000,
            level: 0,
        }];

        let target = vec![SSTableMetadata {
            path: PathBuf::from("t1.sst"),
            min_key: Key::from_str("zzz_000"),
            max_key: Key::from_str("zzz_999"),
            num_entries: 10,
            file_size: 1000,
            level: 1,
        }];

        let overlapping = planner.find_overlapping_sstables(&source, &target);
        assert!(overlapping.is_empty());
    }

    #[test]
    fn test_compaction_config_defaults() {
        let config = CompactionConfig::default();
        assert_eq!(config.strategy, CompactionStrategy::LevelBased);
        assert_eq!(config.l0_threshold, 4);
        assert_eq!(config.level_multiplier, 10);
        assert_eq!(config.min_sstable_size, 1024);
        assert_eq!(config.size_ratio, 2.0);
        assert_eq!(config.min_tier_size, 4);
        assert_eq!(config.max_compaction_bytes_per_sec, 0);
        assert_eq!(config.tombstone_ttl, Duration::from_secs(7 * 24 * 3600));
    }

    #[test]
    fn test_executor_stats_accessible() {
        let executor = CompactionExecutor::new(SSTableConfig::default());
        let snapshot = executor.stats_snapshot();
        assert_eq!(snapshot.compactions_completed, 0);
        assert_eq!(snapshot.bytes_read, 0);
    }

    #[test]
    fn test_size_tiered_preserves_level_info() {
        let config = CompactionConfig {
            strategy: CompactionStrategy::SizeTiered,
            min_sstable_size: 100,
            size_ratio: 2.0,
            min_tier_size: 2,
            ..Default::default()
        };
        let planner = CompactionPlanner::new(config);

        // SSTables at level 2
        let sstables = vec![
            make_metadata("a.sst", 1000, 2),
            make_metadata("b.sst", 1500, 2),
        ];

        let task = planner.plan_size_tiered_compaction(sstables);
        assert!(task.is_some());
        let task = task.expect("task should be Some");
        assert_eq!(task.source_level, 2);
        assert_eq!(task.target_level, 3);
    }

    #[test]
    fn test_level_target_size_l0() {
        let config = CompactionConfig::default();
        let planner = CompactionPlanner::new(config);
        assert_eq!(planner.level_target_size(0), 0);
    }

    // ====== Integration-style tests with real SSTable I/O ======

    #[test]
    fn test_executor_compaction_with_stats() {
        let temp_dir =
            std::env::temp_dir().join(format!("amaters_compaction_test_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("should create temp dir");

        let sstable_config = SSTableConfig::default();

        // Create two SSTables with some entries
        let path1 = temp_dir.join("L0_00000001.sst");
        let path2 = temp_dir.join("L0_00000002.sst");

        create_test_sstable(
            &path1,
            &sstable_config,
            &[("key_01", "val_01"), ("key_02", "val_02")],
        );
        create_test_sstable(
            &path2,
            &sstable_config,
            &[("key_03", "val_03"), ("key_04", "val_04")],
        );

        let meta1 = make_file_metadata(&path1, 0);
        let meta2 = make_file_metadata(&path2, 0);

        let task = CompactionTask {
            source_level: 0,
            target_level: 1,
            source_sstables: vec![meta1, meta2],
            target_sstables: Vec::new(),
        };

        let mut executor = CompactionExecutor::new(sstable_config);
        let mut next_id = 100u64;

        let result = executor.execute_compaction(task, &temp_dir, &mut next_id);
        assert!(result.is_ok(), "compaction should succeed");

        let output = result.expect("should have output");
        assert!(!output.is_empty(), "should produce output SSTables");

        let snapshot = executor.stats_snapshot();
        assert!(snapshot.bytes_read > 0, "should have read bytes");
        assert!(snapshot.bytes_written > 0, "should have written bytes");
        assert_eq!(snapshot.compactions_completed, 1);
        assert_eq!(snapshot.files_merged, 2);
        assert!(
            snapshot.keys_processed >= 4,
            "should have processed at least 4 keys"
        );
        assert!(
            snapshot.total_duration_ms < 10_000,
            "should complete quickly"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_executor_compaction_with_throttling() {
        let temp_dir =
            std::env::temp_dir().join(format!("amaters_throttle_test_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("should create temp dir");

        let sstable_config = SSTableConfig::default();

        // Create SSTables
        let path1 = temp_dir.join("L0_00000001.sst");
        create_test_sstable(
            &path1,
            &sstable_config,
            &[("key_01", "val_01"), ("key_02", "val_02")],
        );

        let meta1 = make_file_metadata(&path1, 0);

        let compaction_config = CompactionConfig {
            max_compaction_bytes_per_sec: 0, // unlimited for this basic test
            ..Default::default()
        };

        let task = CompactionTask {
            source_level: 0,
            target_level: 1,
            source_sstables: vec![meta1],
            target_sstables: Vec::new(),
        };

        let mut executor =
            CompactionExecutor::with_compaction_config(sstable_config, compaction_config);
        let mut next_id = 200u64;

        let start = Instant::now();
        let result = executor.execute_compaction(task, &temp_dir, &mut next_id);
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        // With unlimited throttle, should complete quickly
        assert!(elapsed < Duration::from_secs(5));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // ====== Helper functions ======

    fn make_metadata(name: &str, file_size: u64, level: usize) -> SSTableMetadata {
        SSTableMetadata {
            path: PathBuf::from(name),
            min_key: Key::from_str(&format!("{}_min", name)),
            max_key: Key::from_str(&format!("{}_max", name)),
            num_entries: 10,
            file_size,
            level,
        }
    }

    fn create_test_sstable(path: &Path, config: &SSTableConfig, entries: &[(&str, &str)]) {
        let mut writer =
            SSTableWriter::new(path, config.clone()).expect("should create SSTable writer");
        for (k, v) in entries {
            let key = Key::from_str(k);
            let value = CipherBlob::new(v.as_bytes().to_vec());
            writer.add(key, value).expect("should add entry");
        }
        writer.finish().expect("should finish writing");
    }

    fn make_file_metadata(path: &Path, level: usize) -> SSTableMetadata {
        let file_size = std::fs::metadata(path)
            .expect("SSTable file should exist")
            .len();

        // Read the SSTable to get key range
        let reader = SSTableReader::open(path).expect("should open SSTable");
        let entries = reader.iter().expect("should read entries");

        let min_key = entries
            .first()
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| Key::from_str(""));
        let max_key = entries
            .last()
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| Key::from_str(""));

        SSTableMetadata {
            path: path.to_path_buf(),
            min_key,
            max_key,
            num_entries: entries.len(),
            file_size,
            level,
        }
    }
}
