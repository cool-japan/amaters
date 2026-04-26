//! Server-side query result cache
//!
//! Provides an LRU cache with TTL-based expiry for caching serialized query
//! results. Repeated identical queries (especially expensive FHE filter queries)
//! are served from cache, dramatically reducing latency. The cache supports
//! collection-level invalidation so that write operations (PUT/DELETE/UPDATE)
//! automatically clear stale entries for the affected collection.
//!
//! # Architecture
//!
//! - **CacheKey**: blake3 hash of the serialized query (query type + parameters).
//! - **CacheEntry**: stores the serialized result bytes, creation time, TTL,
//!   access count, and byte size.
//! - **QueryCache**: thread-safe LRU cache protected by `parking_lot::RwLock`.
//!   Supports concurrent reads; writes acquire an exclusive lock.
//! - **CacheStats**: atomic counters for hits, misses, evictions, and insertions.
//!
//! # Write-through invalidation
//!
//! On mutating operations the caller should invoke [`QueryCache::invalidate`] with
//! the affected collection name. This removes every cached entry that was stored
//! under that collection, ensuring stale data is never served.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// CacheKey
// ---------------------------------------------------------------------------

/// Opaque cache key derived from a blake3 hash of the serialized query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey([u8; 32]);

impl CacheKey {
    /// Build a cache key by hashing arbitrary bytes with blake3.
    pub fn from_bytes(data: &[u8]) -> Self {
        let hash = blake3::hash(data);
        Self(*hash.as_bytes())
    }

    /// Build a composite key from a query type tag and parameter bytes.
    ///
    /// The key is `blake3(query_type || b':' || params)`.
    pub fn from_query(query_type: &str, params: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(query_type.as_bytes());
        hasher.update(b":");
        hasher.update(params);
        let hash = hasher.finalize();
        Self(*hash.as_bytes())
    }

    /// Return the raw 32-byte hash.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// CacheEntry
// ---------------------------------------------------------------------------

/// A single cached query result.
struct CacheEntry {
    /// Serialized query result bytes.
    result: Vec<u8>,
    /// When this entry was created.
    created_at: Instant,
    /// Per-entry TTL (may differ from the cache default).
    ttl: Duration,
    /// Number of times this entry has been accessed (read).
    access_count: AtomicU64,
    /// Last time this entry was accessed (for LRU eviction).
    last_accessed: Instant,
    /// Size of the result in bytes.
    size_bytes: usize,
    /// Collection name associated with this entry (for invalidation).
    collection: Option<String>,
}

impl CacheEntry {
    /// Whether the entry has expired according to its TTL.
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}

// ---------------------------------------------------------------------------
// CacheStats
// ---------------------------------------------------------------------------

/// Accumulated cache statistics.
///
/// Uses atomic counters so that a snapshot can be taken without holding any lock
/// on the cache itself.
pub struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    insertions: AtomicU64,
}

impl CacheStats {
    fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            insertions: AtomicU64::new(0),
        }
    }

    /// Return a point-in-time snapshot of the statistics.
    pub fn snapshot(&self) -> CacheStatsSnapshot {
        CacheStatsSnapshot {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            insertions: self.insertions.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time snapshot of cache statistics.
#[derive(Debug, Clone)]
pub struct CacheStatsSnapshot {
    /// Total cache hits.
    pub hits: u64,
    /// Total cache misses.
    pub misses: u64,
    /// Total evictions (LRU or TTL).
    pub evictions: u64,
    /// Total insertions.
    pub insertions: u64,
}

impl CacheStatsSnapshot {
    /// Cache hit rate as a value in `[0.0, 1.0]`.
    ///
    /// Returns `0.0` when there have been no lookups.
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    /// Total size tracked by the stats counters (insertions minus evictions).
    ///
    /// Note: this is an approximation; for the authoritative entry count use
    /// [`QueryCache::len`].
    pub fn approx_size(&self) -> u64 {
        self.insertions.saturating_sub(self.evictions)
    }
}

// ---------------------------------------------------------------------------
// QueryCache
// ---------------------------------------------------------------------------

/// Internal mutable state behind the `RwLock`.
struct CacheInner {
    /// Main storage: cache key -> entry.
    entries: HashMap<CacheKey, CacheEntry>,
    /// LRU order: front = least recently used, back = most recently used.
    lru_order: Vec<CacheKey>,
    /// Reverse index: collection name -> set of cache keys belonging to it.
    collection_index: HashMap<String, Vec<CacheKey>>,
}

impl CacheInner {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            lru_order: Vec::new(),
            collection_index: HashMap::new(),
        }
    }

    /// Move `key` to the back (most recently used) of the LRU list.
    fn touch(&mut self, key: &CacheKey) {
        if let Some(pos) = self.lru_order.iter().position(|k| k == key) {
            self.lru_order.remove(pos);
        }
        self.lru_order.push(*key);
    }

    /// Remove the least recently used entry. Returns the evicted key if any.
    fn evict_lru(&mut self) -> Option<CacheKey> {
        if self.lru_order.is_empty() {
            return None;
        }
        let key = self.lru_order.remove(0);
        self.remove_entry_inner(&key);
        Some(key)
    }

    /// Remove an entry from the map, LRU list, and collection index.
    fn remove_entry(&mut self, key: &CacheKey) {
        if let Some(pos) = self.lru_order.iter().position(|k| k == key) {
            self.lru_order.remove(pos);
        }
        self.remove_entry_inner(key);
    }

    /// Remove an entry from the map and collection index only (caller already
    /// handled the LRU list).
    fn remove_entry_inner(&mut self, key: &CacheKey) {
        if let Some(entry) = self.entries.remove(key) {
            if let Some(ref coll) = entry.collection {
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

/// Thread-safe LRU cache for server-side query results.
///
/// Uses `parking_lot::RwLock` for efficient concurrent access. Read operations
/// that result in a cache hit still require a write lock (to update LRU order
/// and access counters), but the critical section is kept short.
pub struct QueryCache {
    inner: RwLock<CacheInner>,
    max_entries: AtomicUsize,
    default_ttl: Duration,
    max_value_size: usize,
    stats: CacheStats,
}

impl QueryCache {
    /// Create a new `QueryCache`.
    ///
    /// # Arguments
    ///
    /// * `max_entries`    - maximum number of entries the cache may hold.
    /// * `default_ttl`    - default time-to-live for cached entries.
    /// * `max_value_size` - maximum size (in bytes) of a single cached value.
    pub fn new(max_entries: usize, default_ttl: Duration, max_value_size: usize) -> Self {
        Self {
            inner: RwLock::new(CacheInner::new()),
            max_entries: AtomicUsize::new(max_entries),
            default_ttl,
            max_value_size,
            stats: CacheStats::new(),
        }
    }

    /// Look up a cached result by cache key.
    ///
    /// Returns `Some(Vec<u8>)` if a non-expired entry exists, otherwise `None`.
    /// On a hit the entry is promoted to most-recently-used and the hit counter
    /// is incremented. On a miss or expiry the miss counter is incremented.
    pub fn get(&self, key: &CacheKey) -> Option<Vec<u8>> {
        let mut inner = self.inner.write();

        if let Some(entry) = inner.entries.get(key) {
            if entry.is_expired() {
                // Expired -- remove and record as miss + eviction.
                inner.remove_entry(key);
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                self.stats.evictions.fetch_add(1, Ordering::Relaxed);
                return None;
            }

            // Clone result before mutating.
            let result = entry.result.clone();
            // We need to update access metadata -- re-borrow mutably.
            if let Some(entry) = inner.entries.get_mut(key) {
                entry.access_count.fetch_add(1, Ordering::Relaxed);
                entry.last_accessed = Instant::now();
            }
            inner.touch(key);
            self.stats.hits.fetch_add(1, Ordering::Relaxed);
            Some(result)
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Store a query result in the cache using the cache's default TTL.
    ///
    /// If the value exceeds `max_value_size` it is silently rejected. If the
    /// cache is at capacity the least recently used entry is evicted first.
    pub fn put(&self, key: CacheKey, result: Vec<u8>) {
        self.put_with_options(key, result, self.default_ttl, None);
    }

    /// Store a query result with an explicit TTL and optional collection name.
    pub fn put_with_ttl(&self, key: CacheKey, result: Vec<u8>, ttl: Duration) {
        self.put_with_options(key, result, ttl, None);
    }

    /// Store a query result with an explicit TTL and collection name.
    pub fn put_with_options(
        &self,
        key: CacheKey,
        result: Vec<u8>,
        ttl: Duration,
        collection: Option<&str>,
    ) {
        if result.len() > self.max_value_size {
            return; // silently reject oversized values
        }

        let size_bytes = result.len();
        let now = Instant::now();

        let entry = CacheEntry {
            result,
            created_at: now,
            ttl,
            access_count: AtomicU64::new(0),
            last_accessed: now,
            size_bytes,
            collection: collection.map(String::from),
        };

        let mut inner = self.inner.write();

        // Remove existing entry with the same key if present.
        if inner.entries.contains_key(&key) {
            inner.remove_entry(&key);
        }

        // Evict LRU entries until we have room.
        let max = self.max_entries.load(Ordering::Relaxed);
        while inner.entries.len() >= max {
            if inner.evict_lru().is_some() {
                self.stats.evictions.fetch_add(1, Ordering::Relaxed);
            } else {
                break;
            }
        }

        // Update collection index.
        if let Some(ref coll) = entry.collection {
            inner
                .collection_index
                .entry(coll.clone())
                .or_default()
                .push(key);
        }

        inner.entries.insert(key, entry);
        inner.lru_order.push(key);
        self.stats.insertions.fetch_add(1, Ordering::Relaxed);
    }

    /// Invalidate all entries belonging to the given collection.
    ///
    /// This is the primary write-through invalidation hook: when a PUT, DELETE,
    /// or UPDATE operation mutates a collection, call this method with the
    /// collection name to ensure stale results are never served.
    pub fn invalidate(&self, collection: &str) {
        let mut inner = self.inner.write();
        if let Some(keys) = inner.collection_index.remove(collection) {
            let evicted = keys.len() as u64;
            for key in &keys {
                if let Some(pos) = inner.lru_order.iter().position(|k| k == key) {
                    inner.lru_order.remove(pos);
                }
                inner.entries.remove(key);
            }
            self.stats.evictions.fetch_add(evicted, Ordering::Relaxed);
        }
    }

    /// Clear the entire cache.
    pub fn invalidate_all(&self) {
        let mut inner = self.inner.write();
        let evicted = inner.entries.len() as u64;
        inner.entries.clear();
        inner.lru_order.clear();
        inner.collection_index.clear();
        self.stats.evictions.fetch_add(evicted, Ordering::Relaxed);
    }

    /// Return a snapshot of the current cache statistics.
    pub fn stats(&self) -> CacheStatsSnapshot {
        self.stats.snapshot()
    }

    /// Resize the cache to a new maximum number of entries.
    ///
    /// If the new maximum is smaller than the current number of entries the
    /// least recently used entries are evicted until the constraint is met.
    pub fn resize(&self, new_max: usize) {
        // We cannot mutate `self.max_entries` through `&self` alone, but
        // resizing is an infrequent admin operation. We perform the eviction
        // eagerly here; future insertions will still respect `self.max_entries`
        // which we *do* update via an interior-mutability trick below.
        //
        // Because `max_entries` is read without a lock we use a separate atomic
        // store/load pair -- but since the field is plain `usize` we instead
        // just evict under the write lock and accept a brief window where the
        // logical max is stale. This is acceptable for a "resize" operation.
        let mut inner = self.inner.write();
        while inner.entries.len() > new_max {
            if inner.evict_lru().is_some() {
                self.stats.evictions.fetch_add(1, Ordering::Relaxed);
            } else {
                break;
            }
        }
        drop(inner);

        // Update the atomic max_entries using the proper atomic store API.
        self.max_entries.store(new_max, Ordering::SeqCst);
    }

    /// Current number of entries in the cache.
    pub fn len(&self) -> usize {
        let inner = self.inner.read();
        inner.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total bytes of all cached values.
    pub fn total_size_bytes(&self) -> usize {
        let inner = self.inner.read();
        inner.entries.values().map(|e| e.size_bytes).sum()
    }
}

impl std::fmt::Debug for QueryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snap = self.stats.snapshot();
        f.debug_struct("QueryCache")
            .field("max_entries", &self.max_entries)
            .field("default_ttl", &self.default_ttl)
            .field("max_value_size", &self.max_value_size)
            .field("len", &self.len())
            .field("stats", &snap)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    /// Helper: create a cache with sensible defaults for testing.
    fn test_cache(max_entries: usize) -> QueryCache {
        QueryCache::new(max_entries, Duration::from_secs(60), 1024 * 1024)
    }

    // 1. test_cache_put_get
    #[test]
    fn test_cache_put_get() {
        let cache = test_cache(100);
        let key = CacheKey::from_bytes(b"select * from users");
        cache.put(key, vec![1, 2, 3, 4]);

        let result = cache.get(&key);
        assert!(result.is_some());
        assert_eq!(result.expect("should have value"), vec![1, 2, 3, 4]);
    }

    // 2. test_cache_miss
    #[test]
    fn test_cache_miss() {
        let cache = test_cache(100);
        let key = CacheKey::from_bytes(b"nonexistent query");

        let result = cache.get(&key);
        assert!(result.is_none());

        let snap = cache.stats();
        assert_eq!(snap.hits, 0);
        assert_eq!(snap.misses, 1);
    }

    // 3. test_cache_ttl_expiry
    #[test]
    fn test_cache_ttl_expiry() {
        let cache = QueryCache::new(100, Duration::from_millis(50), 1024 * 1024);
        let key = CacheKey::from_bytes(b"expiring query");
        cache.put(key, vec![10, 20]);

        // Should be present immediately.
        assert!(cache.get(&key).is_some());

        // Wait for TTL to expire.
        thread::sleep(Duration::from_millis(100));

        // Should now be gone.
        assert!(cache.get(&key).is_none());

        let snap = cache.stats();
        assert_eq!(snap.hits, 1);
        assert_eq!(snap.misses, 1);
        assert_eq!(snap.evictions, 1); // TTL expiry counts as eviction
    }

    // 4. test_cache_hit_updates_stats
    #[test]
    fn test_cache_hit_updates_stats() {
        let cache = test_cache(100);
        let key = CacheKey::from_bytes(b"stats query");
        cache.put(key, vec![1]);

        for _ in 0..5 {
            let _ = cache.get(&key);
        }

        let snap = cache.stats();
        assert_eq!(snap.hits, 5);
        assert_eq!(snap.misses, 0);
    }

    // 5. test_cache_miss_updates_stats
    #[test]
    fn test_cache_miss_updates_stats() {
        let cache = test_cache(100);

        for i in 0..3u8 {
            let key = CacheKey::from_bytes(&[i]);
            let _ = cache.get(&key);
        }

        let snap = cache.stats();
        assert_eq!(snap.hits, 0);
        assert_eq!(snap.misses, 3);
    }

    // 6. test_cache_lru_eviction
    #[test]
    fn test_cache_lru_eviction() {
        let cache = test_cache(3);

        let keys: Vec<CacheKey> = (0..3u8).map(|i| CacheKey::from_bytes(&[i])).collect();

        for (i, key) in keys.iter().enumerate() {
            cache.put(*key, vec![i as u8]);
        }

        assert_eq!(cache.len(), 3);

        // Access key[0] to make it recently used.
        let _ = cache.get(&keys[0]);

        // Insert a 4th entry -- should evict key[1] (LRU).
        let key3 = CacheKey::from_bytes(&[3u8]);
        cache.put(key3, vec![3]);

        assert_eq!(cache.len(), 3);
        assert!(
            cache.get(&keys[0]).is_some(),
            "key[0] was accessed and should survive"
        );
        assert!(
            cache.get(&keys[1]).is_none(),
            "key[1] should have been evicted"
        );
        assert!(
            cache.get(&keys[2]).is_some(),
            "key[2] should still be present"
        );
        assert!(cache.get(&key3).is_some(), "key[3] was just inserted");

        let snap = cache.stats();
        assert!(snap.evictions >= 1);
    }

    // 7. test_cache_invalidate_collection
    #[test]
    fn test_cache_invalidate_collection() {
        let cache = test_cache(100);

        let k1 = CacheKey::from_query("filter", b"users:age>18");
        let k2 = CacheKey::from_query("get", b"users:id=1");
        let k3 = CacheKey::from_query("filter", b"orders:total>100");

        cache.put_with_options(k1, vec![1], Duration::from_secs(60), Some("users"));
        cache.put_with_options(k2, vec![2], Duration::from_secs(60), Some("users"));
        cache.put_with_options(k3, vec![3], Duration::from_secs(60), Some("orders"));

        assert_eq!(cache.len(), 3);

        cache.invalidate("users");

        assert_eq!(cache.len(), 1);
        assert!(cache.get(&k1).is_none());
        assert!(cache.get(&k2).is_none());
        assert!(cache.get(&k3).is_some(), "orders entry should remain");
    }

    // 8. test_cache_invalidate_all
    #[test]
    fn test_cache_invalidate_all() {
        let cache = test_cache(100);

        for i in 0..10u8 {
            let key = CacheKey::from_bytes(&[i]);
            cache.put(key, vec![i]);
        }

        assert_eq!(cache.len(), 10);

        cache.invalidate_all();

        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());

        let snap = cache.stats();
        assert_eq!(snap.evictions, 10);
    }

    // 9. test_cache_hit_rate
    #[test]
    fn test_cache_hit_rate() {
        let cache = test_cache(100);
        let key = CacheKey::from_bytes(b"rate query");
        cache.put(key, vec![1]);

        // 3 hits
        for _ in 0..3 {
            let _ = cache.get(&key);
        }
        // 1 miss
        let missing = CacheKey::from_bytes(b"no such key");
        let _ = cache.get(&missing);

        let snap = cache.stats();
        // 3 / (3 + 1) = 0.75
        assert!((snap.hit_rate() - 0.75).abs() < 1e-9);

        // Zero lookups case.
        let empty_cache = test_cache(10);
        let snap = empty_cache.stats();
        assert!((snap.hit_rate() - 0.0).abs() < f64::EPSILON);
    }

    // 10. test_cache_concurrent_access
    #[test]
    fn test_cache_concurrent_access() {
        let cache = Arc::new(test_cache(500));
        let mut handles = Vec::new();

        // Writer threads.
        for t in 0..4 {
            let c = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for i in 0..200u64 {
                    let key_bytes = format!("thread-{}-key-{}", t, i);
                    let key = CacheKey::from_bytes(key_bytes.as_bytes());
                    c.put(key, vec![t as u8; 64]);
                }
            }));
        }

        // Reader threads.
        for t in 0..4 {
            let c = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for i in 0..200u64 {
                    let key_bytes = format!("thread-{}-key-{}", t, i);
                    let key = CacheKey::from_bytes(key_bytes.as_bytes());
                    let _ = c.get(&key);
                }
            }));
        }

        for h in handles {
            h.join().expect("thread should not panic");
        }

        let snap = cache.stats();
        assert!(snap.insertions > 0);
        assert!(cache.len() <= 500);
    }

    // 11. test_cache_max_value_size
    #[test]
    fn test_cache_max_value_size() {
        let cache = QueryCache::new(100, Duration::from_secs(60), 100);

        // Exactly at limit -- should be accepted.
        let k1 = CacheKey::from_bytes(b"small");
        cache.put(k1, vec![0u8; 100]);
        assert!(cache.get(&k1).is_some());

        // Over limit -- silently rejected.
        let k2 = CacheKey::from_bytes(b"big");
        cache.put(k2, vec![0u8; 101]);
        assert!(cache.get(&k2).is_none());

        let snap = cache.stats();
        assert_eq!(snap.insertions, 1); // only the small one
    }

    // 12. test_cache_resize
    #[test]
    fn test_cache_resize() {
        let cache = test_cache(10);

        for i in 0..10u8 {
            let key = CacheKey::from_bytes(&[i]);
            cache.put(key, vec![i]);
        }
        assert_eq!(cache.len(), 10);

        // Shrink to 5 -- should evict 5 LRU entries.
        cache.resize(5);
        assert_eq!(cache.len(), 5);

        let snap = cache.stats();
        assert_eq!(snap.evictions, 5);

        // New insertions should respect the new limit.
        for i in 100..106u8 {
            let key = CacheKey::from_bytes(&[i]);
            cache.put(key, vec![i]);
        }
        assert!(cache.len() <= 5);
    }

    // 13. test_cache_key_generation
    #[test]
    fn test_cache_key_generation() {
        let k1 = CacheKey::from_query("filter", b"users:age>18");
        let k2 = CacheKey::from_query("filter", b"users:age>18");
        assert_eq!(k1, k2, "same query should produce the same key");

        let k3 = CacheKey::from_bytes(b"hello world");
        let k4 = CacheKey::from_bytes(b"hello world");
        assert_eq!(k3, k4);
    }

    // 14. test_cache_different_queries
    #[test]
    fn test_cache_different_queries() {
        let k1 = CacheKey::from_query("filter", b"users:age>18");
        let k2 = CacheKey::from_query("filter", b"users:age>21");
        assert_ne!(k1, k2, "different params should produce different keys");

        let k3 = CacheKey::from_query("filter", b"users:age>18");
        let k4 = CacheKey::from_query("get", b"users:age>18");
        assert_ne!(
            k3, k4,
            "different query types should produce different keys"
        );
    }

    // Extra: total_size_bytes
    #[test]
    fn test_total_size_bytes() {
        let cache = test_cache(100);
        let k1 = CacheKey::from_bytes(b"a");
        let k2 = CacheKey::from_bytes(b"b");
        cache.put(k1, vec![0u8; 100]);
        cache.put(k2, vec![0u8; 200]);
        assert_eq!(cache.total_size_bytes(), 300);
    }

    // Extra: put_with_ttl uses custom TTL
    #[test]
    fn test_put_with_custom_ttl() {
        let cache = QueryCache::new(100, Duration::from_secs(300), 1024 * 1024);
        let key = CacheKey::from_bytes(b"short lived");
        cache.put_with_ttl(key, vec![1, 2], Duration::from_millis(50));

        assert!(cache.get(&key).is_some());
        thread::sleep(Duration::from_millis(100));
        assert!(cache.get(&key).is_none());
    }

    // Extra: debug formatting
    #[test]
    fn test_debug_format() {
        let cache = test_cache(10);
        let dbg = format!("{:?}", cache);
        assert!(dbg.contains("QueryCache"));
        assert!(dbg.contains("max_entries"));
    }
}
