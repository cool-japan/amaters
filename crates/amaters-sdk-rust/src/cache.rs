//! Client-side query result cache for the AmateRS Rust SDK
//!
//! Provides an LRU (Least Recently Used) cache for query results, reducing
//! round-trips to the server for frequently accessed data. The cache is
//! thread-safe and supports TTL-based expiration, collection-level invalidation,
//! and configurable size limits.
//!
//! # Example
//!
//! ```no_run
//! use amaters_sdk_rust::cache::{QueryCache, QueryCacheConfig, InvalidationPolicy};
//! use std::time::Duration;
//!
//! let config = QueryCacheConfig::default()
//!     .with_max_entries(500)
//!     .with_ttl(Duration::from_secs(120))
//!     .with_max_value_size(512 * 1024);
//!
//! let cache = QueryCache::new(config);
//!
//! // Put and get
//! cache.put(b"key1", vec![1, 2, 3]);
//! if let Some(data) = cache.get(b"key1") {
//!     assert_eq!(data, vec![1, 2, 3]);
//! }
//! ```

use parking_lot::RwLock;
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the query cache.
#[derive(Debug, Clone)]
pub struct QueryCacheConfig {
    /// Maximum number of entries the cache will hold.
    pub max_entries: usize,
    /// Time-to-live for each cached entry.
    pub ttl: Duration,
    /// Maximum size (in bytes) of a single cached value.
    pub max_value_size: usize,
    /// Cache invalidation policy on write operations.
    pub invalidation_policy: InvalidationPolicy,
}

impl Default for QueryCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 1000,
            ttl: Duration::from_secs(60),
            max_value_size: 1024 * 1024, // 1 MB
            invalidation_policy: InvalidationPolicy::OnWrite,
        }
    }
}

impl QueryCacheConfig {
    /// Set the maximum number of cache entries.
    #[must_use]
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// Set the TTL for cached entries.
    #[must_use]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Set the maximum size of a single cached value in bytes.
    #[must_use]
    pub fn with_max_value_size(mut self, max_value_size: usize) -> Self {
        self.max_value_size = max_value_size;
        self
    }

    /// Set the invalidation policy.
    #[must_use]
    pub fn with_invalidation_policy(mut self, policy: InvalidationPolicy) -> Self {
        self.invalidation_policy = policy;
        self
    }
}

/// Policy for cache invalidation when a write operation occurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidationPolicy {
    /// Automatically invalidate affected cache entries on write (default).
    OnWrite,
    /// Cache entries are only invalidated manually by the caller.
    Manual,
    /// No invalidation — entries live until TTL expiry or LRU eviction.
    None,
}

// ---------------------------------------------------------------------------
// CacheStats
// ---------------------------------------------------------------------------

/// Statistics about cache usage.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of cache hits.
    pub hits: u64,
    /// Number of cache misses.
    pub misses: u64,
    /// Number of entries evicted due to capacity constraints.
    pub evictions: u64,
    /// Current number of entries in the cache.
    pub size: usize,
    /// Total number of entries that have been inserted.
    pub total_inserts: u64,
    /// Total number of explicit invalidations.
    pub invalidations: u64,
}

impl CacheStats {
    /// Cache hit rate as a value in `[0.0, 1.0]`.
    /// Returns `0.0` if there have been no lookups.
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

// ---------------------------------------------------------------------------
// Internal structures
// ---------------------------------------------------------------------------

/// A cached result entry.
#[derive(Debug, Clone)]
struct CachedResult {
    /// The raw cached data.
    data: Vec<u8>,
    /// When the entry was inserted / last refreshed.
    inserted_at: Instant,
    /// How many times this entry has been hit.
    hit_count: u64,
    /// The collection this entry belongs to (for collection-level invalidation).
    collection: Option<String>,
}

/// Node in the doubly-linked LRU list.
///
/// We store prev/next as `Option<CacheKey>` indices into the `HashMap` so that
/// the entire data structure lives inside a single allocation-friendly map.
#[derive(Debug, Clone)]
struct LruNode {
    prev: Option<CacheKey>,
    next: Option<CacheKey>,
}

/// Opaque cache key (blake3 hash).
type CacheKey = [u8; 32];

/// Internal mutable state protected by a `RwLock`.
struct CacheInner {
    /// The main storage: key -> (cached result, LRU node).
    entries: HashMap<CacheKey, (CachedResult, LruNode)>,
    /// Head of the LRU list (most recently used).
    head: Option<CacheKey>,
    /// Tail of the LRU list (least recently used — eviction candidate).
    tail: Option<CacheKey>,
    /// Reverse index: collection name -> set of cache keys belonging to it.
    collection_index: HashMap<String, Vec<CacheKey>>,
    /// Accumulated statistics.
    stats: CacheStats,
}

impl CacheInner {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            head: None,
            tail: None,
            collection_index: HashMap::new(),
            stats: CacheStats {
                hits: 0,
                misses: 0,
                evictions: 0,
                size: 0,
                total_inserts: 0,
                invalidations: 0,
            },
        }
    }

    // ---- LRU helpers -------------------------------------------------------

    /// Detach `key` from the doubly-linked list without removing it from the map.
    fn detach(&mut self, key: &CacheKey) {
        let node = if let Some((_, node)) = self.entries.get(key) {
            node.clone()
        } else {
            return;
        };

        // Fix previous node's next pointer
        if let Some(prev_key) = &node.prev {
            if let Some((_, prev_node)) = self.entries.get_mut(prev_key) {
                prev_node.next = node.next;
            }
        } else {
            // This node was the head
            self.head = node.next;
        }

        // Fix next node's prev pointer
        if let Some(next_key) = &node.next {
            if let Some((_, next_node)) = self.entries.get_mut(next_key) {
                next_node.prev = node.prev;
            }
        } else {
            // This node was the tail
            self.tail = node.prev;
        }

        // Clear this node's pointers
        if let Some((_, n)) = self.entries.get_mut(key) {
            n.prev = None;
            n.next = None;
        }
    }

    /// Push `key` to the front (head) of the LRU list. Assumes the key is
    /// already detached (or freshly inserted).
    fn push_front(&mut self, key: CacheKey) {
        if let Some(old_head) = self.head {
            if old_head == key {
                return; // already at head
            }
            // Point old head's prev to this key
            if let Some((_, node)) = self.entries.get_mut(&old_head) {
                node.prev = Some(key);
            }
        }

        // Set this node's pointers
        if let Some((_, node)) = self.entries.get_mut(&key) {
            node.prev = None;
            node.next = self.head;
        }

        self.head = Some(key);

        if self.tail.is_none() {
            self.tail = Some(key);
        }
    }

    /// Move an existing key to the front (most recently used).
    fn touch(&mut self, key: &CacheKey) {
        let k = *key;
        self.detach(&k);
        self.push_front(k);
    }

    /// Evict the least recently used entry (tail). Returns the evicted key.
    fn evict_lru(&mut self) -> Option<CacheKey> {
        let tail_key = self.tail?;
        self.remove_entry(&tail_key);
        self.stats.evictions += 1;
        Some(tail_key)
    }

    /// Remove an entry entirely (map + LRU list + collection index).
    fn remove_entry(&mut self, key: &CacheKey) {
        self.detach(key);
        if let Some((result, _)) = self.entries.remove(key) {
            self.stats.size = self.stats.size.saturating_sub(1);
            // Remove from collection index
            if let Some(ref coll) = result.collection {
                if let Some(keys) = self.collection_index.get_mut(coll) {
                    keys.retain(|k| k != key);
                    if keys.is_empty() {
                        self.collection_index.remove(coll);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// QueryCache (public API)
// ---------------------------------------------------------------------------

/// Thread-safe LRU cache for query results.
///
/// Uses `parking_lot::RwLock` for efficient concurrent access. Read operations
/// (cache hits) only require a read lock; mutations (inserts, evictions,
/// invalidations) acquire a write lock.
pub struct QueryCache {
    inner: RwLock<CacheInner>,
    config: QueryCacheConfig,
}

impl QueryCache {
    /// Create a new cache with the given configuration.
    pub fn new(config: QueryCacheConfig) -> Self {
        Self {
            inner: RwLock::new(CacheInner::new()),
            config,
        }
    }

    /// Look up a cached value by its raw key bytes.
    ///
    /// Returns `None` if the entry does not exist or has expired.
    /// A successful lookup increments the hit counter and moves the entry
    /// to the most-recently-used position.
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let cache_key = Self::hash_key(key);

        // First try with a read lock to check existence and expiry
        {
            let inner = self.inner.read();
            match inner.entries.get(&cache_key) {
                Some((result, _)) => {
                    if result.inserted_at.elapsed() > self.config.ttl {
                        // Expired — we'll remove it below with a write lock
                        drop(inner);
                        let mut inner = self.inner.write();
                        inner.remove_entry(&cache_key);
                        inner.stats.misses += 1;
                        return None;
                    }
                }
                None => {
                    drop(inner);
                    let mut inner = self.inner.write();
                    inner.stats.misses += 1;
                    return None;
                }
            }
        }

        // Entry exists and is not expired — promote & bump hit count.
        let mut inner = self.inner.write();
        // Re-check under write lock (another thread may have evicted it).
        if let Some((result, _)) = inner.entries.get_mut(&cache_key) {
            if result.inserted_at.elapsed() > self.config.ttl {
                inner.remove_entry(&cache_key);
                inner.stats.misses += 1;
                return None;
            }
            result.hit_count += 1;
            let data = result.data.clone();
            inner.stats.hits += 1;
            inner.touch(&cache_key);
            Some(data)
        } else {
            inner.stats.misses += 1;
            None
        }
    }

    /// Insert a value into the cache.
    ///
    /// If the value exceeds `max_value_size` it is silently dropped. If the
    /// cache is at capacity, the least recently used entry is evicted first.
    pub fn put(&self, key: &[u8], value: Vec<u8>) {
        self.put_with_collection(key, value, None);
    }

    /// Insert a value into the cache with an associated collection name.
    ///
    /// The collection name is used for `invalidate_collection`.
    pub fn put_with_collection(&self, key: &[u8], value: Vec<u8>, collection: Option<&str>) {
        if value.len() > self.config.max_value_size {
            return; // silently reject oversized values
        }

        let cache_key = Self::hash_key(key);

        let mut inner = self.inner.write();

        // If key already exists, remove it first so we can re-insert cleanly.
        if inner.entries.contains_key(&cache_key) {
            inner.remove_entry(&cache_key);
        }

        // Evict if at capacity
        while inner.entries.len() >= self.config.max_entries {
            inner.evict_lru();
        }

        let coll_string = collection.map(String::from);

        // Insert into collection index
        if let Some(ref coll) = coll_string {
            inner
                .collection_index
                .entry(coll.clone())
                .or_default()
                .push(cache_key);
        }

        let result = CachedResult {
            data: value,
            inserted_at: Instant::now(),
            hit_count: 0,
            collection: coll_string,
        };

        let node = LruNode {
            prev: None,
            next: None,
        };

        inner.entries.insert(cache_key, (result, node));
        inner.stats.size += 1;
        inner.stats.total_inserts += 1;
        inner.push_front(cache_key);
    }

    /// Remove a specific entry from the cache.
    pub fn invalidate(&self, key: &[u8]) {
        let cache_key = Self::hash_key(key);
        let mut inner = self.inner.write();
        if inner.entries.contains_key(&cache_key) {
            inner.remove_entry(&cache_key);
            inner.stats.invalidations += 1;
        }
    }

    /// Remove all cache entries belonging to a given collection.
    pub fn invalidate_collection(&self, collection: &str) {
        let mut inner = self.inner.write();
        if let Some(keys) = inner.collection_index.remove(collection) {
            for key in &keys {
                inner.detach(key);
                inner.entries.remove(key);
                inner.stats.size = inner.stats.size.saturating_sub(1);
                inner.stats.invalidations += 1;
            }
        }
    }

    /// Remove all entries from the cache.
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        let prev_size = inner.entries.len();
        inner.entries.clear();
        inner.head = None;
        inner.tail = None;
        inner.collection_index.clear();
        inner.stats.size = 0;
        inner.stats.invalidations += prev_size as u64;
    }

    /// Return a snapshot of the current cache statistics.
    pub fn stats(&self) -> CacheStats {
        let inner = self.inner.read();
        inner.stats.clone()
    }

    /// Return the current number of entries in the cache.
    pub fn len(&self) -> usize {
        let inner = self.inner.read();
        inner.entries.len()
    }

    /// Check whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get a reference to the cache configuration.
    pub fn config(&self) -> &QueryCacheConfig {
        &self.config
    }

    /// Return the invalidation policy.
    pub fn invalidation_policy(&self) -> InvalidationPolicy {
        self.config.invalidation_policy
    }

    // ---- key helpers -------------------------------------------------------

    /// Build a composite cache key from collection + query key and hash it.
    pub fn make_key(collection: &str, query_key: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(collection.len() + 1 + query_key.len());
        buf.extend_from_slice(collection.as_bytes());
        buf.push(b':');
        buf.extend_from_slice(query_key);
        buf
    }

    /// Compute a blake3 hash of the raw key bytes.
    fn hash_key(key: &[u8]) -> CacheKey {
        let hash = blake3::hash(key);
        *hash.as_bytes()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    fn default_cache() -> QueryCache {
        QueryCache::new(QueryCacheConfig::default())
    }

    // --- basic hit / miss ---------------------------------------------------

    #[test]
    fn test_cache_hit() {
        let cache = default_cache();
        cache.put(b"key1", vec![10, 20, 30]);

        let result = cache.get(b"key1");
        assert!(result.is_some());
        assert_eq!(result.expect("should have value"), vec![10, 20, 30]);

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 0);
    }

    #[test]
    fn test_cache_miss() {
        let cache = default_cache();

        let result = cache.get(b"nonexistent");
        assert!(result.is_none());

        let stats = cache.stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 1);
    }

    // --- TTL expiry ---------------------------------------------------------

    #[test]
    fn test_ttl_expiry() {
        let config = QueryCacheConfig::default().with_ttl(Duration::from_millis(50));
        let cache = QueryCache::new(config);

        cache.put(b"key1", vec![1, 2, 3]);
        assert!(cache.get(b"key1").is_some());

        // Wait for TTL to expire
        thread::sleep(Duration::from_millis(80));

        assert!(cache.get(b"key1").is_none());

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1); // the expired lookup
    }

    // --- LRU eviction -------------------------------------------------------

    #[test]
    fn test_lru_eviction() {
        let config = QueryCacheConfig::default().with_max_entries(3);
        let cache = QueryCache::new(config);

        cache.put(b"a", vec![1]);
        cache.put(b"b", vec![2]);
        cache.put(b"c", vec![3]);

        // Cache is full. Insert one more — should evict "a" (LRU).
        cache.put(b"d", vec![4]);

        assert!(cache.get(b"a").is_none(), "a should have been evicted");
        assert!(cache.get(b"b").is_some());
        assert!(cache.get(b"c").is_some());
        assert!(cache.get(b"d").is_some());

        let stats = cache.stats();
        assert_eq!(stats.evictions, 1);
    }

    #[test]
    fn test_lru_access_order() {
        let config = QueryCacheConfig::default().with_max_entries(3);
        let cache = QueryCache::new(config);

        cache.put(b"a", vec![1]);
        cache.put(b"b", vec![2]);
        cache.put(b"c", vec![3]);

        // Access "a" to make it most recently used
        let _ = cache.get(b"a");

        // Insert "d" — should evict "b" (now LRU), not "a"
        cache.put(b"d", vec![4]);

        assert!(
            cache.get(b"a").is_some(),
            "a was accessed and should not be evicted"
        );
        assert!(cache.get(b"b").is_none(), "b should have been evicted");
        assert!(cache.get(b"c").is_some());
        assert!(cache.get(b"d").is_some());
    }

    // --- write invalidation -------------------------------------------------

    #[test]
    fn test_invalidate_key() {
        let cache = default_cache();

        cache.put(b"key1", vec![1]);
        cache.put(b"key2", vec![2]);

        cache.invalidate(b"key1");

        assert!(cache.get(b"key1").is_none());
        assert!(cache.get(b"key2").is_some());

        let stats = cache.stats();
        assert_eq!(stats.invalidations, 1);
    }

    // --- collection invalidation --------------------------------------------

    #[test]
    fn test_invalidate_collection() {
        let cache = default_cache();

        let key1 = QueryCache::make_key("users", b"u1");
        let key2 = QueryCache::make_key("users", b"u2");
        let key3 = QueryCache::make_key("orders", b"o1");

        cache.put_with_collection(&key1, vec![1], Some("users"));
        cache.put_with_collection(&key2, vec![2], Some("users"));
        cache.put_with_collection(&key3, vec![3], Some("orders"));

        cache.invalidate_collection("users");

        assert!(cache.get(&key1).is_none());
        assert!(cache.get(&key2).is_none());
        assert!(cache.get(&key3).is_some(), "orders entry should remain");

        let stats = cache.stats();
        assert_eq!(stats.invalidations, 2);
    }

    // --- stats accuracy -----------------------------------------------------

    #[test]
    fn test_stats_accuracy() {
        let cache = default_cache();

        // 3 inserts
        cache.put(b"a", vec![1]);
        cache.put(b"b", vec![2]);
        cache.put(b"c", vec![3]);

        // 2 hits
        let _ = cache.get(b"a");
        let _ = cache.get(b"b");

        // 1 miss
        let _ = cache.get(b"z");

        // 1 invalidation
        cache.invalidate(b"c");

        let stats = cache.stats();
        assert_eq!(stats.total_inserts, 3);
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.invalidations, 1);
        assert_eq!(stats.size, 2);

        let rate = stats.hit_rate();
        // 2 hits / (2 hits + 1 miss) = 0.666...
        assert!((rate - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn test_hit_rate_no_lookups() {
        let cache = default_cache();
        let stats = cache.stats();
        assert!((stats.hit_rate() - 0.0).abs() < f64::EPSILON);
    }

    // --- concurrent access --------------------------------------------------

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;

        let cache = Arc::new(QueryCache::new(
            QueryCacheConfig::default().with_max_entries(500),
        ));

        let mut handles = Vec::new();

        // Spawn writer threads
        for t in 0..4 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for i in 0..200 {
                    let key = format!("thread-{}-key-{}", t, i);
                    cache.put(key.as_bytes(), vec![t as u8; 64]);
                }
            }));
        }

        // Spawn reader threads
        for t in 0..4 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for i in 0..200 {
                    let key = format!("thread-{}-key-{}", t, i);
                    let _ = cache.get(key.as_bytes());
                }
            }));
        }

        for h in handles {
            h.join().expect("thread should not panic");
        }

        let stats = cache.stats();
        // Just verify no panics occurred and stats are sane
        assert!(stats.total_inserts > 0);
        assert!(stats.size <= 500);
    }

    // --- max value size enforcement -----------------------------------------

    #[test]
    fn test_max_value_size_enforcement() {
        let config = QueryCacheConfig::default().with_max_value_size(100);
        let cache = QueryCache::new(config);

        // This should be accepted (100 bytes exactly)
        cache.put(b"small", vec![0u8; 100]);
        assert!(cache.get(b"small").is_some());

        // This should be silently rejected (101 bytes)
        cache.put(b"big", vec![0u8; 101]);
        assert!(cache.get(b"big").is_none());

        let stats = cache.stats();
        assert_eq!(stats.total_inserts, 1); // only the small one
    }

    // --- clear --------------------------------------------------------------

    #[test]
    fn test_clear() {
        let cache = default_cache();

        cache.put(b"a", vec![1]);
        cache.put(b"b", vec![2]);
        cache.put(b"c", vec![3]);

        assert_eq!(cache.len(), 3);

        cache.clear();

        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
        assert!(cache.get(b"a").is_none());

        let stats = cache.stats();
        assert_eq!(stats.size, 0);
        assert_eq!(stats.invalidations, 3);
    }

    // --- make_key helper ----------------------------------------------------

    #[test]
    fn test_make_key() {
        let key = QueryCache::make_key("users", b"abc");
        assert_eq!(key, b"users:abc");
    }

    // --- overwrite existing key ---------------------------------------------

    #[test]
    fn test_overwrite_existing_key() {
        let cache = default_cache();

        cache.put(b"key", vec![1, 2, 3]);
        assert_eq!(cache.get(b"key").expect("should exist"), vec![1, 2, 3]);

        cache.put(b"key", vec![4, 5, 6]);
        assert_eq!(cache.get(b"key").expect("should exist"), vec![4, 5, 6]);

        assert_eq!(cache.len(), 1);

        let stats = cache.stats();
        assert_eq!(stats.total_inserts, 2);
    }

    // --- invalidation policy ------------------------------------------------

    #[test]
    fn test_invalidation_policy_config() {
        let config =
            QueryCacheConfig::default().with_invalidation_policy(InvalidationPolicy::Manual);
        let cache = QueryCache::new(config);
        assert_eq!(cache.invalidation_policy(), InvalidationPolicy::Manual);
    }

    // --- single entry cache edge case ---------------------------------------

    #[test]
    fn test_single_entry_cache() {
        let config = QueryCacheConfig::default().with_max_entries(1);
        let cache = QueryCache::new(config);

        cache.put(b"a", vec![1]);
        assert!(cache.get(b"a").is_some());

        cache.put(b"b", vec![2]);
        assert!(cache.get(b"a").is_none());
        assert!(cache.get(b"b").is_some());

        let stats = cache.stats();
        assert_eq!(stats.evictions, 1);
    }
}
