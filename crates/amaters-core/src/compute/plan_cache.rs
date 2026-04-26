//! Plan cache for the query planner
//!
//! Provides an LRU cache with TTL-based expiry for caching physical execution
//! plans. This avoids re-planning identical queries when the same query is
//! submitted multiple times within the TTL window.

use crate::types::Query;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use super::planner::PhysicalPlan;

// ---------------------------------------------------------------------------
// Plan cache
// ---------------------------------------------------------------------------

/// Cache key: a blake3 hash of the normalized query representation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey([u8; 32]);

impl CacheKey {
    /// Create a cache key from a query by normalizing its debug representation
    /// and hashing it with blake3.
    pub(crate) fn from_query(query: &Query) -> Self {
        let raw = format!("{:?}", query);
        let normalized = Self::normalize(&raw);
        let hash = blake3::hash(normalized.as_bytes());
        Self(*hash.as_bytes())
    }

    /// Normalize a query string: trim whitespace, lowercase the operation type
    pub(crate) fn normalize(raw: &str) -> String {
        let trimmed = raw.trim();
        // Lowercase the first "word" (operation type like Filter, Get, etc.)
        // The debug format is e.g. "Filter { collection: ..., predicate: ... }"
        if let Some(idx) = trimmed.find(|c: char| !c.is_alphanumeric() && c != '_') {
            let (op, rest) = trimmed.split_at(idx);
            format!("{}{}", op.to_lowercase(), rest)
        } else {
            trimmed.to_lowercase()
        }
    }

    /// Create a cache key from a raw string (for prefix-based operations)
    #[allow(dead_code)]
    fn from_str(s: &str) -> Self {
        let hash = blake3::hash(s.as_bytes());
        Self(*hash.as_bytes())
    }
}

/// A cached physical plan with metadata
#[derive(Debug, Clone)]
pub struct CachedPlan {
    /// The cached physical plan
    pub plan: PhysicalPlan,
    /// When the plan was cached
    pub cached_at: Instant,
    /// Number of cache hits for this entry
    pub hit_count: u64,
    /// Normalized query string (for prefix matching during invalidation)
    pub normalized_query: String,
}

/// Configuration for the plan cache
#[derive(Debug, Clone)]
pub struct PlanCacheConfig {
    /// Maximum number of entries in the cache
    pub max_entries: usize,
    /// Time-to-live for cached plans
    pub ttl: Duration,
}

impl Default for PlanCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 1000,
            ttl: Duration::from_secs(300), // 5 minutes
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
    /// Number of evictions (LRU or TTL)
    pub evictions: u64,
    /// Current number of entries in the cache
    pub size: usize,
}

/// LRU plan cache with TTL-based expiry
///
/// Implements a manual LRU cache using a `HashMap` for O(1) lookups and a
/// `VecDeque` to track access order for eviction. Thread-safe via
/// `parking_lot::Mutex`.
pub struct PlanCache {
    /// Cache storage: key -> cached plan
    entries: Mutex<HashMap<CacheKey, CachedPlan>>,
    /// LRU order: front = least recently used, back = most recently used
    lru_order: Mutex<VecDeque<CacheKey>>,
    /// Configuration
    config: PlanCacheConfig,
    /// Running statistics
    stats: Mutex<CacheStats>,
}

impl PlanCache {
    /// Create a new plan cache with the given configuration
    pub fn new(config: PlanCacheConfig) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            lru_order: Mutex::new(VecDeque::new()),
            config,
            stats: Mutex::new(CacheStats::default()),
        }
    }

    /// Look up a cached plan by its cache key.
    ///
    /// Returns `Some(PhysicalPlan)` if a fresh (non-expired) entry exists,
    /// otherwise returns `None`. Updates LRU order and hit/miss statistics.
    pub fn get(&self, key: &CacheKey) -> Option<PhysicalPlan> {
        let mut entries = self.entries.lock();
        let mut stats = self.stats.lock();

        if let Some(entry) = entries.get_mut(key) {
            // Check TTL
            if entry.cached_at.elapsed() > self.config.ttl {
                // Expired: remove and count as miss + eviction
                entries.remove(key);
                let mut lru = self.lru_order.lock();
                lru.retain(|k| k != key);
                stats.misses += 1;
                stats.evictions += 1;
                stats.size = entries.len();
                return None;
            }

            // Cache hit
            entry.hit_count += 1;
            stats.hits += 1;

            // Move to back of LRU (most recently used)
            let mut lru = self.lru_order.lock();
            lru.retain(|k| k != key);
            lru.push_back(*key);

            Some(entry.plan.clone())
        } else {
            stats.misses += 1;
            None
        }
    }

    /// Insert a plan into the cache, evicting the LRU entry if at capacity.
    pub fn insert(&self, key: CacheKey, plan: PhysicalPlan, normalized_query: String) {
        let mut entries = self.entries.lock();
        let mut lru = self.lru_order.lock();
        let mut stats = self.stats.lock();

        // If the key already exists, update in place
        if entries.contains_key(&key) {
            lru.retain(|k| k != &key);
        }

        // Evict LRU entries if at capacity
        while entries.len() >= self.config.max_entries {
            if let Some(evicted_key) = lru.pop_front() {
                entries.remove(&evicted_key);
                stats.evictions += 1;
            } else {
                break;
            }
        }

        entries.insert(
            key,
            CachedPlan {
                plan,
                cached_at: Instant::now(),
                hit_count: 0,
                normalized_query,
            },
        );
        lru.push_back(key);
        stats.size = entries.len();
    }

    /// Invalidate all cached plans
    pub fn invalidate_all(&self) {
        let mut entries = self.entries.lock();
        let mut lru = self.lru_order.lock();
        let mut stats = self.stats.lock();

        let evicted = entries.len() as u64;
        entries.clear();
        lru.clear();
        stats.evictions += evicted;
        stats.size = 0;
    }

    /// Invalidate all cached plans whose normalized query starts with the
    /// given prefix (e.g., a collection name).
    pub fn invalidate_prefix(&self, prefix: &str) {
        let normalized_prefix = prefix.trim().to_lowercase();
        let mut entries = self.entries.lock();
        let mut lru = self.lru_order.lock();
        let mut stats = self.stats.lock();

        let keys_to_remove: Vec<CacheKey> = entries
            .iter()
            .filter(|(_, v)| v.normalized_query.contains(&normalized_prefix))
            .map(|(k, _)| *k)
            .collect();

        for key in &keys_to_remove {
            entries.remove(key);
            stats.evictions += 1;
        }

        lru.retain(|k| !keys_to_remove.contains(k));
        stats.size = entries.len();
    }

    /// Return a snapshot of the current cache statistics
    pub fn cache_stats(&self) -> CacheStats {
        let stats = self.stats.lock();
        stats.clone()
    }
}

impl std::fmt::Debug for PlanCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.cache_stats();
        f.debug_struct("PlanCache")
            .field("max_entries", &self.config.max_entries)
            .field("ttl", &self.config.ttl)
            .field("stats", &stats)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::super::planner::QueryPlanner;
    use super::*;
    use crate::error::Result;
    use crate::types::Key;

    // -- Plan cache tests ---------------------------------------------------

    #[test]
    fn test_cache_hit_for_same_query() -> Result<()> {
        let planner = QueryPlanner::new().with_cache(PlanCacheConfig::default());

        let query = Query::Get {
            collection: "users".to_string(),
            key: Key::from_str("user:1"),
        };

        // First call: cache miss, plan gets cached
        let plan1 = planner.plan(&query)?;
        let stats1 = planner.cache_stats();
        assert_eq!(stats1.misses, 1, "first call should be a miss");
        assert_eq!(stats1.hits, 0);
        assert_eq!(stats1.size, 1, "one entry should be cached");

        // Second call: cache hit
        let plan2 = planner.plan(&query)?;
        let stats2 = planner.cache_stats();
        assert_eq!(stats2.hits, 1, "second call should be a hit");
        assert_eq!(stats2.misses, 1, "miss count should not change");

        // Plans should be structurally equivalent
        assert_eq!(format!("{:?}", plan1), format!("{:?}", plan2));

        Ok(())
    }

    #[test]
    fn test_cache_miss_for_different_queries() -> Result<()> {
        let planner = QueryPlanner::new().with_cache(PlanCacheConfig::default());

        let query_a = Query::Get {
            collection: "users".to_string(),
            key: Key::from_str("user:1"),
        };
        let query_b = Query::Get {
            collection: "users".to_string(),
            key: Key::from_str("user:2"),
        };

        let _plan_a = planner.plan(&query_a)?;
        let _plan_b = planner.plan(&query_b)?;

        let stats = planner.cache_stats();
        assert_eq!(stats.misses, 2, "both queries should miss");
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.size, 2);

        Ok(())
    }

    #[test]
    fn test_cache_ttl_expiry() -> Result<()> {
        let config = PlanCacheConfig {
            max_entries: 100,
            ttl: Duration::from_millis(50), // very short TTL
        };
        let planner = QueryPlanner::new().with_cache(config);

        let query = Query::Get {
            collection: "items".to_string(),
            key: Key::from_str("item:1"),
        };

        // Cache the plan
        let _plan1 = planner.plan(&query)?;
        let stats1 = planner.cache_stats();
        assert_eq!(stats1.misses, 1);

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(100));

        // Should miss again because TTL expired
        let _plan2 = planner.plan(&query)?;
        let stats2 = planner.cache_stats();
        assert_eq!(stats2.misses, 2, "expired entry should cause a miss");
        assert_eq!(
            stats2.evictions, 1,
            "expired entry should count as eviction"
        );

        Ok(())
    }

    #[test]
    fn test_cache_lru_eviction() -> Result<()> {
        let config = PlanCacheConfig {
            max_entries: 3,
            ttl: Duration::from_secs(300),
        };
        let planner = QueryPlanner::new().with_cache(config);

        // Insert 3 entries (fills cache)
        for i in 0..3 {
            let query = Query::Get {
                collection: "data".to_string(),
                key: Key::from_str(&format!("key:{}", i)),
            };
            let _plan = planner.plan(&query)?;
        }

        let stats = planner.cache_stats();
        assert_eq!(stats.size, 3);
        assert_eq!(stats.evictions, 0);

        // Insert a 4th entry, should evict the LRU (key:0)
        let query_new = Query::Get {
            collection: "data".to_string(),
            key: Key::from_str("key:3"),
        };
        let _plan = planner.plan(&query_new)?;

        let stats = planner.cache_stats();
        assert_eq!(stats.size, 3, "size should remain at max_entries");
        assert_eq!(stats.evictions, 1, "one entry should have been evicted");

        // Access key:1 (was second oldest, but now should be in cache)
        let query_1 = Query::Get {
            collection: "data".to_string(),
            key: Key::from_str("key:1"),
        };
        let _plan = planner.plan(&query_1)?;
        let stats = planner.cache_stats();
        assert_eq!(stats.hits, 1, "key:1 should still be in cache");

        // The evicted key:0 should miss
        let query_0 = Query::Get {
            collection: "data".to_string(),
            key: Key::from_str("key:0"),
        };
        let _plan = planner.plan(&query_0)?;
        let stats = planner.cache_stats();
        // key:0 was evicted, so this is a miss; also evicts key:2 (now LRU)
        assert!(
            stats.misses >= 5,
            "key:0 should have been evicted and cause a miss"
        );

        Ok(())
    }

    #[test]
    fn test_cache_stats_accuracy() -> Result<()> {
        let config = PlanCacheConfig {
            max_entries: 10,
            ttl: Duration::from_secs(300),
        };
        let planner = QueryPlanner::new().with_cache(config);

        // Start with zero stats
        let stats = planner.cache_stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
        assert_eq!(stats.evictions, 0);
        assert_eq!(stats.size, 0);

        let query = Query::Get {
            collection: "stats_test".to_string(),
            key: Key::from_str("k1"),
        };

        // 1 miss
        let _p = planner.plan(&query)?;
        let stats = planner.cache_stats();
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.size, 1);

        // 5 hits
        for _ in 0..5 {
            let _p = planner.plan(&query)?;
        }
        let stats = planner.cache_stats();
        assert_eq!(stats.hits, 5);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.size, 1);

        Ok(())
    }

    #[test]
    fn test_cache_invalidate_all() -> Result<()> {
        let planner = QueryPlanner::new().with_cache(PlanCacheConfig::default());

        // Cache some entries
        for i in 0..5 {
            let query = Query::Get {
                collection: "inv_all".to_string(),
                key: Key::from_str(&format!("k:{}", i)),
            };
            let _p = planner.plan(&query)?;
        }

        let stats = planner.cache_stats();
        assert_eq!(stats.size, 5);

        // Invalidate all
        planner.invalidate_all();

        let stats = planner.cache_stats();
        assert_eq!(stats.size, 0, "all entries should be removed");
        assert_eq!(stats.evictions, 5, "all removed entries count as evictions");

        // Re-plan should miss
        let query = Query::Get {
            collection: "inv_all".to_string(),
            key: Key::from_str("k:0"),
        };
        let _p = planner.plan(&query)?;
        let stats = planner.cache_stats();
        assert_eq!(stats.misses, 6, "re-plan after invalidation should miss");

        Ok(())
    }

    #[test]
    fn test_cache_invalidate_prefix() -> Result<()> {
        let planner = QueryPlanner::new().with_cache(PlanCacheConfig::default());

        // Cache entries for two different collections
        for i in 0..3 {
            let query = Query::Get {
                collection: "orders".to_string(),
                key: Key::from_str(&format!("o:{}", i)),
            };
            let _p = planner.plan(&query)?;
        }
        for i in 0..2 {
            let query = Query::Get {
                collection: "products".to_string(),
                key: Key::from_str(&format!("p:{}", i)),
            };
            let _p = planner.plan(&query)?;
        }

        let stats = planner.cache_stats();
        assert_eq!(stats.size, 5);

        // Invalidate only "orders"
        planner.invalidate_prefix("orders");

        let stats = planner.cache_stats();
        assert_eq!(stats.size, 2, "only products should remain");
        assert_eq!(stats.evictions, 3, "3 orders entries evicted");

        // "products" entries should still hit
        let query = Query::Get {
            collection: "products".to_string(),
            key: Key::from_str("p:0"),
        };
        let _p = planner.plan(&query)?;
        let stats = planner.cache_stats();
        assert_eq!(stats.hits, 1, "products entry should still be cached");

        Ok(())
    }

    #[test]
    fn test_cache_concurrent_access() -> Result<()> {
        use std::sync::Arc;

        let config = PlanCacheConfig {
            max_entries: 100,
            ttl: Duration::from_secs(300),
        };
        let planner = Arc::new(QueryPlanner::new().with_cache(config));

        let mut handles = Vec::new();

        // Spawn 8 threads, each planning the same 10 queries
        for thread_id in 0..8 {
            let planner_clone = Arc::clone(&planner);
            let handle = std::thread::spawn(move || -> Result<()> {
                for i in 0..10 {
                    let query = Query::Get {
                        collection: "concurrent".to_string(),
                        key: Key::from_str(&format!("k:{}:{}", thread_id % 2, i)),
                    };
                    let _plan = planner_clone.plan(&query)?;
                }
                Ok(())
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("thread should not panic")?;
        }

        let stats = planner.cache_stats();

        // With 8 threads and 10 queries each (20 distinct keys: thread_id%2 x 10),
        // we should have at most 20 unique entries
        assert!(stats.size <= 20, "should have at most 20 entries");
        // Total operations = 80, misses <= 80, some should be hits
        let total = stats.hits + stats.misses;
        assert_eq!(total, 80, "total ops should be 80");
        // With 4 threads sharing the same keys as another 4, we expect some hits
        assert!(
            stats.hits > 0,
            "should have some cache hits from concurrent access"
        );

        Ok(())
    }

    #[test]
    fn test_cache_key_normalization() {
        // Same query should produce the same key regardless of whitespace variations
        let key_a = CacheKey::normalize("  Filter { collection: \"x\" }  ");
        let key_b = CacheKey::normalize("Filter { collection: \"x\" }");
        assert_eq!(key_a, key_b);

        // Operation type should be lowercased
        let normalized = CacheKey::normalize("FILTER { collection: \"x\" }");
        assert!(normalized.starts_with("filter"));
    }

    #[test]
    fn test_planner_without_cache() -> Result<()> {
        use super::super::planner::PhysicalPlan;

        // Verify that a planner without cache works normally
        let planner = QueryPlanner::new();
        assert!(planner.plan_cache().is_none());

        let query = Query::Get {
            collection: "no_cache".to_string(),
            key: Key::from_str("k1"),
        };

        let plan = planner.plan(&query)?;
        assert!(matches!(plan, PhysicalPlan::PointGet { .. }));

        // cache_stats should return defaults
        let stats = planner.cache_stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);

        // invalidate should be a no-op
        planner.invalidate_all();
        planner.invalidate_prefix("anything");

        Ok(())
    }

    #[test]
    fn test_cache_with_filter_queries() -> Result<()> {
        use super::super::planner::PhysicalPlan;
        use crate::types::{CipherBlob, Predicate, col};

        let planner = QueryPlanner::new().with_cache(PlanCacheConfig::default());

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate: Predicate::Gt(col("age"), CipherBlob::new(vec![18])),
        };

        // First plan: miss
        let plan1 = planner.plan(&query)?;
        let stats = planner.cache_stats();
        assert_eq!(stats.misses, 1);

        // Second plan: hit
        let plan2 = planner.plan(&query)?;
        let stats = planner.cache_stats();
        assert_eq!(stats.hits, 1);

        // Both plans should be FheFilter
        assert!(matches!(plan1, PhysicalPlan::FheFilter { .. }));
        assert!(matches!(plan2, PhysicalPlan::FheFilter { .. }));

        Ok(())
    }

    #[test]
    fn test_plan_cache_debug() {
        let cache = PlanCache::new(PlanCacheConfig::default());
        let debug_str = format!("{:?}", cache);
        assert!(debug_str.contains("PlanCache"));
        assert!(debug_str.contains("max_entries"));
    }

    #[test]
    fn test_cache_lru_order_updated_on_access() -> Result<()> {
        let config = PlanCacheConfig {
            max_entries: 3,
            ttl: Duration::from_secs(300),
        };
        let planner = QueryPlanner::new().with_cache(config);

        // Insert 3 entries: key:0, key:1, key:2
        // LRU order: key:0 (oldest) -> key:1 -> key:2 (newest)
        for i in 0..3 {
            let query = Query::Get {
                collection: "lru".to_string(),
                key: Key::from_str(&format!("key:{}", i)),
            };
            let _p = planner.plan(&query)?;
        }

        // Access key:0 to move it to the back
        let query_0 = Query::Get {
            collection: "lru".to_string(),
            key: Key::from_str("key:0"),
        };
        let _p = planner.plan(&query_0)?;

        // Now LRU order: key:1 (oldest) -> key:2 -> key:0 (newest)
        // Insert key:3 should evict key:1 (not key:0)
        let query_3 = Query::Get {
            collection: "lru".to_string(),
            key: Key::from_str("key:3"),
        };
        let _p = planner.plan(&query_3)?;

        // key:0 should still be cached (was accessed recently)
        let _p = planner.plan(&query_0)?;
        let stats = planner.cache_stats();
        // key:0 was accessed twice as hit (once to move, once now)
        assert!(
            stats.hits >= 2,
            "key:0 should still be in cache after LRU reorder"
        );

        // key:1 should have been evicted
        let query_1 = Query::Get {
            collection: "lru".to_string(),
            key: Key::from_str("key:1"),
        };
        let _p = planner.plan(&query_1)?;
        let stats = planner.cache_stats();
        // This causes another eviction (key:2 is now LRU) and a miss
        assert!(
            stats.evictions >= 2,
            "key:1 eviction + new eviction for reinsertion"
        );

        Ok(())
    }
}
