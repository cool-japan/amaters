//! Block cache for SSTable blocks
//!
//! LRU (Least Recently Used) cache for caching SSTable data blocks in memory.
//! Reduces disk I/O by keeping frequently accessed blocks in memory.

use crate::error::{AmateRSError, ErrorContext, Result};
use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Cache key identifying a specific block in a specific SSTable
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockCacheKey {
    /// SSTable file path
    pub sstable_path: String,
    /// Block index within the SSTable
    pub block_index: usize,
}

impl BlockCacheKey {
    /// Create a new cache key
    pub fn new(sstable_path: String, block_index: usize) -> Self {
        Self {
            sstable_path,
            block_index,
        }
    }
}

/// Cached block data
#[derive(Debug, Clone)]
pub struct CachedBlock {
    /// The block data
    pub data: Arc<Vec<u8>>,
    /// Size in bytes
    pub size: usize,
}

impl CachedBlock {
    /// Create a new cached block
    pub fn new(data: Vec<u8>) -> Self {
        let size = data.len();
        Self {
            data: Arc::new(data),
            size,
        }
    }

    /// Get the block data as a slice
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }
}

/// LRU block cache configuration
#[derive(Debug, Clone)]
pub struct BlockCacheConfig {
    /// Maximum cache size in bytes
    pub max_size_bytes: usize,
    /// Whether to track cache statistics
    pub enable_stats: bool,
}

impl Default for BlockCacheConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 128 * 1024 * 1024, // 128 MB default
            enable_stats: true,
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of cache hits
    pub hits: u64,
    /// Number of cache misses
    pub misses: u64,
    /// Number of evictions
    pub evictions: u64,
    /// Current number of blocks in cache
    pub block_count: usize,
    /// Current cache size in bytes
    pub size_bytes: usize,
}

impl CacheStats {
    /// Calculate hit rate (0.0 to 1.0)
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    /// Calculate miss rate (0.0 to 1.0)
    pub fn miss_rate(&self) -> f64 {
        1.0 - self.hit_rate()
    }
}

/// LRU cache entry
struct CacheEntry {
    key: BlockCacheKey,
    block: CachedBlock,
}

/// LRU (Least Recently Used) block cache
///
/// Thread-safe cache using RwLock for concurrent read access.
/// Evicts least recently used blocks when cache is full.
pub struct BlockCache {
    /// Configuration
    config: BlockCacheConfig,
    /// Cache entries (HashMap for O(1) lookups)
    cache: Arc<RwLock<HashMap<BlockCacheKey, CachedBlock>>>,
    /// LRU order (most recent at back)
    lru_order: Arc<RwLock<VecDeque<BlockCacheKey>>>,
    /// Current cache size in bytes
    current_size: Arc<RwLock<usize>>,
    /// Cache statistics
    stats: Arc<RwLock<CacheStats>>,
}

impl BlockCache {
    /// Create a new block cache with default configuration
    pub fn new() -> Self {
        Self::with_config(BlockCacheConfig::default())
    }

    /// Create a new block cache with custom configuration
    pub fn with_config(config: BlockCacheConfig) -> Self {
        Self {
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            lru_order: Arc::new(RwLock::new(VecDeque::new())),
            current_size: Arc::new(RwLock::new(0)),
            stats: Arc::new(RwLock::new(CacheStats::default())),
        }
    }

    /// Get a block from the cache
    pub fn get(&self, key: &BlockCacheKey) -> Option<CachedBlock> {
        // First, check if block exists and clone it
        let block = {
            let cache = self.cache.read();
            cache.get(key).cloned()
        };

        // Update LRU and stats after releasing cache lock
        if let Some(ref block) = block {
            // Update LRU order (move to back)
            self.touch(key);

            // Update statistics
            if self.config.enable_stats {
                let mut stats = self.stats.write();
                stats.hits += 1;
            }

            Some(block.clone())
        } else {
            // Update statistics
            if self.config.enable_stats {
                let mut stats = self.stats.write();
                stats.misses += 1;
            }

            None
        }
    }

    /// Put a block into the cache
    pub fn put(&self, key: BlockCacheKey, block: CachedBlock) -> Result<()> {
        let block_size = block.size;

        // Check if we need to evict blocks to make room
        self.evict_if_needed(block_size)?;

        // Insert into cache
        let (new_block_count, new_size_bytes) = {
            let mut cache = self.cache.write();
            let mut lru_order = self.lru_order.write();
            let mut current_size = self.current_size.write();

            // Remove old entry if exists
            if let Some(old_block) = cache.remove(&key) {
                *current_size -= old_block.size;
                // Remove from LRU order
                lru_order.retain(|k| k != &key);
            }

            // Insert new entry
            cache.insert(key.clone(), block);
            lru_order.push_back(key);
            *current_size += block_size;

            // Return stats while we have the locks
            (cache.len(), *current_size)
        };

        // Update statistics after releasing locks
        if self.config.enable_stats {
            let mut stats = self.stats.write();
            stats.block_count = new_block_count;
            stats.size_bytes = new_size_bytes;
        }

        Ok(())
    }

    /// Touch a key (move to most recent position)
    fn touch(&self, key: &BlockCacheKey) {
        let mut lru_order = self.lru_order.write();

        // Remove from current position
        lru_order.retain(|k| k != key);

        // Add to back (most recent)
        lru_order.push_back(key.clone());
    }

    /// Evict blocks if needed to make room for new block
    fn evict_if_needed(&self, new_block_size: usize) -> Result<()> {
        if new_block_size > self.config.max_size_bytes {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Block size {} exceeds cache size {}",
                new_block_size, self.config.max_size_bytes
            ))));
        }

        let current_size = *self.current_size.read();
        let mut size_to_free =
            (current_size + new_block_size).saturating_sub(self.config.max_size_bytes);

        while size_to_free > 0 {
            // Get least recently used key (front of queue) and evict it atomically
            let (evicted_size, should_update_stats) = {
                let mut cache = self.cache.write();
                let mut lru_order = self.lru_order.write();
                let mut current_size = self.current_size.write();

                // Get front key
                if let Some(key) = lru_order.front().cloned() {
                    if let Some(block) = cache.remove(&key) {
                        lru_order.pop_front();
                        *current_size -= block.size;
                        (block.size, self.config.enable_stats)
                    } else {
                        (0, false)
                    }
                } else {
                    // No more blocks to evict
                    (0, false)
                }
            };

            if evicted_size == 0 {
                // No more blocks to evict
                break;
            }

            // Update statistics after releasing locks
            if should_update_stats {
                let mut stats = self.stats.write();
                stats.evictions += 1;
            }

            if evicted_size >= size_to_free {
                size_to_free = 0;
            } else {
                size_to_free -= evicted_size;
            }
        }

        Ok(())
    }

    /// Clear all blocks from the cache
    pub fn clear(&self) {
        let mut cache = self.cache.write();
        let mut lru_order = self.lru_order.write();
        let mut current_size = self.current_size.write();

        cache.clear();
        lru_order.clear();
        *current_size = 0;

        if self.config.enable_stats {
            let mut stats = self.stats.write();
            stats.block_count = 0;
            stats.size_bytes = 0;
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        self.stats.read().clone()
    }

    /// Get current cache size in bytes
    pub fn current_size(&self) -> usize {
        *self.current_size.read()
    }

    /// Get number of blocks in cache
    pub fn block_count(&self) -> usize {
        self.cache.read().len()
    }

    /// Check if cache contains a key
    pub fn contains(&self, key: &BlockCacheKey) -> bool {
        self.cache.read().contains_key(key)
    }

    /// Remove a specific block from cache
    pub fn remove(&self, key: &BlockCacheKey) -> Option<CachedBlock> {
        let mut cache = self.cache.write();
        let mut lru_order = self.lru_order.write();
        let mut current_size = self.current_size.write();

        if let Some(block) = cache.remove(key) {
            lru_order.retain(|k| k != key);
            *current_size -= block.size;

            if self.config.enable_stats {
                let mut stats = self.stats.write();
                stats.block_count = cache.len();
                stats.size_bytes = *current_size;
            }

            Some(block)
        } else {
            None
        }
    }

    /// Invalidate all blocks for a specific SSTable
    pub fn invalidate_sstable(&self, sstable_path: &str) {
        let mut cache = self.cache.write();
        let mut lru_order = self.lru_order.write();
        let mut current_size = self.current_size.write();

        // Collect keys to remove
        let keys_to_remove: Vec<BlockCacheKey> = cache
            .keys()
            .filter(|k| k.sstable_path == sstable_path)
            .cloned()
            .collect();

        // Remove blocks
        for key in keys_to_remove {
            if let Some(block) = cache.remove(&key) {
                *current_size -= block.size;
                lru_order.retain(|k| k != &key);
            }
        }

        if self.config.enable_stats {
            let mut stats = self.stats.write();
            stats.block_count = cache.len();
            stats.size_bytes = *current_size;
        }
    }
}

impl Default for BlockCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_cache_basic() -> Result<()> {
        let cache = BlockCache::new();

        let key = BlockCacheKey::new("test.sst".to_string(), 0);
        let block = CachedBlock::new(vec![1, 2, 3, 4, 5]);

        // Initially not in cache
        assert!(cache.get(&key).is_none());

        // Put in cache
        cache.put(key.clone(), block.clone())?;

        // Now should be in cache
        let retrieved = cache.get(&key).expect("Block should be in cache after put");
        assert_eq!(retrieved.as_slice(), &[1, 2, 3, 4, 5]);

        Ok(())
    }

    #[test]
    fn test_block_cache_lru_eviction() -> Result<()> {
        let config = BlockCacheConfig {
            max_size_bytes: 100,
            enable_stats: true,
        };
        let cache = BlockCache::with_config(config);

        // Add blocks that exceed cache size
        for i in 0..5 {
            let key = BlockCacheKey::new("test.sst".to_string(), i);
            let block = CachedBlock::new(vec![0u8; 30]); // 30 bytes each
            cache.put(key, block)?;
        }

        // Cache should have evicted oldest blocks
        assert!(cache.current_size() <= 100);

        // First blocks should be evicted
        let key0 = BlockCacheKey::new("test.sst".to_string(), 0);
        let key1 = BlockCacheKey::new("test.sst".to_string(), 1);
        assert!(cache.get(&key0).is_none());
        assert!(cache.get(&key1).is_none());

        // Recent blocks should still be present
        let key4 = BlockCacheKey::new("test.sst".to_string(), 4);
        assert!(cache.get(&key4).is_some());

        Ok(())
    }

    #[test]
    fn test_block_cache_touch() -> Result<()> {
        let config = BlockCacheConfig {
            max_size_bytes: 100,
            enable_stats: true,
        };
        let cache = BlockCache::with_config(config);

        // Add 3 blocks
        for i in 0..3 {
            let key = BlockCacheKey::new("test.sst".to_string(), i);
            let block = CachedBlock::new(vec![0u8; 30]);
            cache.put(key, block)?;
        }

        // Touch block 0 (make it most recent)
        let key0 = BlockCacheKey::new("test.sst".to_string(), 0);
        cache.get(&key0);

        // Add a new block (should evict block 1, not block 0)
        let key3 = BlockCacheKey::new("test.sst".to_string(), 3);
        let block3 = CachedBlock::new(vec![0u8; 30]);
        cache.put(key3, block3)?;

        // Block 0 should still be present (touched)
        assert!(cache.get(&key0).is_some());

        // Block 1 should be evicted (oldest untouched)
        let key1 = BlockCacheKey::new("test.sst".to_string(), 1);
        assert!(cache.get(&key1).is_none());

        Ok(())
    }

    #[test]
    fn test_block_cache_stats() -> Result<()> {
        let cache = BlockCache::new();

        let key = BlockCacheKey::new("test.sst".to_string(), 0);
        let block = CachedBlock::new(vec![1, 2, 3]);

        // Miss
        cache.get(&key);

        // Put
        cache.put(key.clone(), block)?;

        // Hit
        cache.get(&key);
        cache.get(&key);

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hit_rate(), 2.0 / 3.0);

        Ok(())
    }

    #[test]
    fn test_block_cache_clear() -> Result<()> {
        let cache = BlockCache::new();

        for i in 0..5 {
            let key = BlockCacheKey::new("test.sst".to_string(), i);
            let block = CachedBlock::new(vec![0u8; 100]);
            cache.put(key, block)?;
        }

        assert!(cache.block_count() > 0);
        assert!(cache.current_size() > 0);

        cache.clear();

        assert_eq!(cache.block_count(), 0);
        assert_eq!(cache.current_size(), 0);

        Ok(())
    }

    #[test]
    fn test_block_cache_remove() -> Result<()> {
        let cache = BlockCache::new();

        let key = BlockCacheKey::new("test.sst".to_string(), 0);
        let block = CachedBlock::new(vec![1, 2, 3]);

        cache.put(key.clone(), block)?;
        assert!(cache.contains(&key));

        cache.remove(&key);
        assert!(!cache.contains(&key));

        Ok(())
    }

    #[test]
    fn test_block_cache_invalidate_sstable() -> Result<()> {
        let cache = BlockCache::new();

        // Add blocks from two SSTables
        for i in 0..3 {
            let key = BlockCacheKey::new("test1.sst".to_string(), i);
            let block = CachedBlock::new(vec![0u8; 100]);
            cache.put(key, block)?;
        }

        for i in 0..3 {
            let key = BlockCacheKey::new("test2.sst".to_string(), i);
            let block = CachedBlock::new(vec![0u8; 100]);
            cache.put(key, block)?;
        }

        assert_eq!(cache.block_count(), 6);

        // Invalidate one SSTable
        cache.invalidate_sstable("test1.sst");

        assert_eq!(cache.block_count(), 3);

        // test1.sst blocks should be gone
        let key1 = BlockCacheKey::new("test1.sst".to_string(), 0);
        assert!(!cache.contains(&key1));

        // test2.sst blocks should still be present
        let key2 = BlockCacheKey::new("test2.sst".to_string(), 0);
        assert!(cache.contains(&key2));

        Ok(())
    }

    #[test]
    fn test_block_cache_concurrent() -> Result<()> {
        use std::sync::Arc;
        use std::thread;

        let cache = Arc::new(BlockCache::new());
        let mut handles = vec![];

        // Spawn multiple threads doing cache operations
        for thread_id in 0..4 {
            let cache = Arc::clone(&cache);
            let handle = thread::spawn(move || {
                for i in 0..100 {
                    let key = BlockCacheKey::new(format!("test_{}.sst", thread_id), i);
                    let block = CachedBlock::new(vec![thread_id as u8; 100]);
                    cache
                        .put(key.clone(), block)
                        .expect("Cache put should succeed in concurrent test");
                    cache.get(&key);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("Thread should complete successfully");
        }

        // Cache should have blocks from all threads
        assert!(cache.block_count() > 0);
        let stats = cache.stats();
        assert!(stats.hits > 0);

        Ok(())
    }
}
