//! Storage engine module (Iwato - The Rock Cave)
//!
//! This module provides persistent storage with LSM-Tree architecture.
//!
//! # Phase 1 Complete ✅
//! - [x] Implement in-memory storage for MVP
//!
//! # Phase 2 In Progress 🚧
//! - [x] Implement Memtable (in-memory sorted map)
//! - [x] Implement WAL (Write-Ahead Log)
//! - [x] Implement SSTable format and writer
//! - [x] Implement Block Cache (LRU)
//! - [x] Implement LSM-Tree basic structure
//! - [x] Implement compaction strategy
//! - [x] Implement Bloom filters for fast key lookups
//! - [x] Implement Manifest for metadata tracking
//! - [ ] Implement WiscKey value separation
//! - [ ] Add io_uring support for Linux

// Module declarations
pub mod backup;
pub mod block_cache;
pub mod bloom_filter;
pub mod buffer_pool;
pub mod compaction;
pub mod compression;
pub mod lsm_storage;
pub mod lsm_tree;
pub mod manifest;
pub mod memory;
pub mod memtable;
#[cfg(feature = "mmap")]
pub mod mmap_reader;
pub mod secondary_index;
pub mod sstable;
pub mod value_log;
pub mod value_log_gc;
pub mod value_log_gc_worker;
pub mod wal;

#[cfg(feature = "mmap")]
pub use mmap_reader::{MadviseHint, MmapPrefetcher, MmapReaderPool, MmapSstableReader};

pub use backup::{BackupManager, BackupMetadata, BackupType};
pub use block_cache::{BlockCache, BlockCacheConfig, BlockCacheKey, CacheStats, CachedBlock};
pub use bloom_filter::{BloomFilter, BloomFilterConfig, BloomFilterMetadata};
pub use buffer_pool::{BufferPool, BufferPoolStats, PooledBuffer};
pub use compaction::{
    CompactionConfig, CompactionExecutor, CompactionPlanner, CompactionStats,
    CompactionStatsSnapshot, CompactionStrategy, CompactionTask, CompactionThrottler, SizeTier,
    TombstoneEntry,
};
pub use compression::CompressionType;
pub use lsm_storage::LsmTreeStorage;
pub use lsm_tree::{LevelInfo, LsmTree, LsmTreeConfig, LsmTreeStats, SSTableMetadata};
pub use manifest::{Manifest, ManifestConfig, ManifestEntry, ManifestSnapshot, ManifestVersion};
pub use memory::MemoryStorage;
pub use memtable::{Memtable, MemtableConfig};
pub use secondary_index::{
    IndexConfig, IndexEntry, IndexManager, IndexStats, IndexType, SecondaryIndex,
};
pub use sstable::{SSTableConfig, SSTableReader, SSTableWriter};
pub use value_log::{GcStats, ValueLog, ValueLogConfig, ValuePointer};
pub use value_log_gc::{GcConfig, GcResult, SegmentStats};
pub use value_log_gc_worker::{
    BgGcStats, GcWorker, GcWorkerBuilder, GcWorkerHandle, spawn_gc_worker,
};
pub use wal::{Wal, WalConfig, WalEntry, WalEntryType, WalReader};
