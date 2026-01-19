//! Compaction strategy for LSM-Tree
//!
//! Implements level-based compaction to:
//! - Merge SSTables from L0 to L1
//! - Merge overlapping SSTables within levels
//! - Remove tombstones (deleted keys)
//! - Maintain level size targets

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::{SSTableConfig, SSTableMetadata, SSTableReader, SSTableWriter};
use crate::types::{CipherBlob, Key};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            strategy: CompactionStrategy::LevelBased,
            l0_threshold: 4,
            level_multiplier: 10,
            base_level_size: 10 * 1024 * 1024,       // 10 MB
            max_compaction_bytes: 100 * 1024 * 1024, // 100 MB
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

/// Compaction statistics
#[derive(Debug, Clone, Default)]
pub struct CompactionStats {
    /// Total compactions performed
    pub total_compactions: u64,
    /// Total bytes read during compaction
    pub bytes_read: u64,
    /// Total bytes written during compaction
    pub bytes_written: u64,
    /// Total keys processed
    pub keys_processed: u64,
    /// Total tombstones removed
    pub tombstones_removed: u64,
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

    /// Plan a compaction task
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

    /// Select SSTables for compaction (L1+)
    fn select_sstables_for_compaction(
        &self,
        sstables: Vec<SSTableMetadata>,
    ) -> Vec<SSTableMetadata> {
        // Simple strategy: compact oldest SSTables first
        // In production, this should consider:
        // - SSTable overlap
        // - Size ratios
        // - Read frequency (hot/cold data)

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
    fn find_overlapping_sstables(
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

/// Compaction executor
pub struct CompactionExecutor {
    config: SSTableConfig,
    stats: CompactionStats,
}

impl CompactionExecutor {
    /// Create a new compaction executor
    pub fn new(config: SSTableConfig) -> Self {
        Self {
            config,
            stats: CompactionStats::default(),
        }
    }

    /// Execute a compaction task
    pub fn execute_compaction(
        &mut self,
        task: CompactionTask,
        output_dir: &Path,
        next_sstable_id: &mut u64,
    ) -> Result<Vec<SSTableMetadata>> {
        // Collect all entries from source and target SSTables
        let mut all_entries = BTreeMap::new();

        // Read from source SSTables
        for sstable in &task.source_sstables {
            self.read_sstable_entries(&sstable.path, &mut all_entries)?;
            self.stats.bytes_read += sstable.file_size;
        }

        // Read from target SSTables (overlapping)
        for sstable in &task.target_sstables {
            self.read_sstable_entries(&sstable.path, &mut all_entries)?;
            self.stats.bytes_read += sstable.file_size;
        }

        // Write merged SSTables to target level
        let output_sstables = self.write_compacted_sstables(
            all_entries,
            task.target_level,
            output_dir,
            next_sstable_id,
        )?;

        self.stats.total_compactions += 1;

        Ok(output_sstables)
    }

    /// Read entries from an SSTable
    fn read_sstable_entries(
        &mut self,
        path: &Path,
        entries: &mut BTreeMap<Key, Option<CipherBlob>>,
    ) -> Result<()> {
        let reader = SSTableReader::open(path)?;
        let sstable_entries = reader.iter()?;

        for (key, value) in sstable_entries {
            self.stats.keys_processed += 1;
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
            // Skip tombstones (None values)
            let value = match value_opt {
                Some(v) => v,
                None => {
                    self.stats.tombstones_removed += 1;
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

                        self.stats.bytes_written += file_size;

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

                self.stats.bytes_written += file_size;

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
}
