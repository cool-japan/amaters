//! FHE compiled-circuit cache.
//!
//! Memoises the result of [`amaters_core::compute::PredicateCompiler::compile`] keyed on the
//! normalised debug-formatted form of the predicate. Because circuit
//! compilation is a pure function of the predicate structure (no side-effects,
//! no I/O), a cache hit is always safe to use.
//!
//! # Design
//!
//! - **Key derivation**: the predicate is formatted via its `Debug` impl and
//!   the resulting UTF-8 bytes are hashed with blake3 to produce a 32-byte
//!   cache key.  This is deterministic as long as `Debug` output is stable,
//!   which it is for the `Predicate` type in this codebase.
//! - **LRU eviction**: the `CacheInner` maintains an insertion-order
//!   `VecDeque`; on every hit the accessed key is moved to the back (MRU
//!   position) so that the front always holds the least-recently-used entry.
//! - **Clone-able handle**: `CircuitCache` wraps its mutable interior in
//!   `Arc<Mutex<…>>` so that clones share the same backing store, enabling
//!   cheap per-request handles without re-initialising the cache.
//! - **Zero external dependencies**: all dependencies (`blake3`, `parking_lot`)
//!   are already listed in `amaters-net`'s `Cargo.toml`.

use amaters_core::compute::Circuit;
use amaters_core::types::Predicate;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// CircuitCacheKey
// ---------------------------------------------------------------------------

/// Opaque 32-byte cache key derived from a predicate's debug representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CircuitCacheKey([u8; 32]);

impl CircuitCacheKey {
    /// Derive a cache key from a predicate.
    ///
    /// The predicate is converted to a stable byte sequence via its `Debug`
    /// implementation and then hashed with blake3.
    pub fn from_predicate(predicate: &Predicate) -> Self {
        let repr = format!("{predicate:?}");
        let hash = blake3::hash(repr.as_bytes());
        Self(*hash.as_bytes())
    }

    /// Return the raw 32-byte hash.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// CircuitCacheStats
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of circuit cache statistics.
#[derive(Debug, Clone, Default)]
pub struct CircuitCacheStats {
    /// Total cache hits (key found and returned).
    pub hits: u64,
    /// Total cache misses (key not present; compilation was triggered).
    pub misses: u64,
    /// Total evictions due to the LRU capacity limit being reached.
    pub evictions: u64,
    /// Number of entries currently held in the cache.
    pub current_size: usize,
}

impl CircuitCacheStats {
    /// Cache hit rate as a value in `[0.0, 1.0]`.
    ///
    /// Returns `0.0` when no lookups have been performed yet.
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
// CircuitCacheConfig
// ---------------------------------------------------------------------------

/// Configuration for a [`CircuitCache`] instance.
#[derive(Debug, Clone)]
pub struct CircuitCacheConfig {
    /// Maximum number of compiled circuits to retain in memory.
    ///
    /// When the cache is full and a new circuit must be inserted, the
    /// least-recently-used entry is evicted.
    pub max_entries: usize,
}

impl Default for CircuitCacheConfig {
    fn default() -> Self {
        Self { max_entries: 256 }
    }
}

impl CircuitCacheConfig {
    /// Create a configuration with the given capacity limit.
    pub fn new(max_entries: usize) -> Self {
        Self { max_entries }
    }
}

// ---------------------------------------------------------------------------
// CacheInner (unsynchronised)
// ---------------------------------------------------------------------------

/// Unsynchronised interior state of the cache.
///
/// Protected externally by `parking_lot::Mutex`.
struct CacheInner {
    /// The compiled circuits, keyed by their hash.
    map: HashMap<CircuitCacheKey, Circuit>,
    /// LRU order: front = least recently used, back = most recently used.
    order: VecDeque<CircuitCacheKey>,
    /// Accumulated statistics.
    stats: CircuitCacheStats,
    /// Capacity limit and other config values.
    config: CircuitCacheConfig,
}

impl CacheInner {
    fn new(config: CircuitCacheConfig) -> Self {
        let capacity = config.max_entries;
        Self {
            map: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            stats: CircuitCacheStats::default(),
            config,
        }
    }

    /// Look up a key and return a clone of the stored circuit if present.
    ///
    /// On a hit the entry is promoted to the MRU (back) position and the hit
    /// counter is incremented.  On a miss the miss counter is incremented.
    fn get(&mut self, key: &CircuitCacheKey) -> Option<Circuit> {
        if let Some(circuit) = self.map.get(key).cloned() {
            self.stats.hits += 1;
            // Promote to MRU
            if let Some(pos) = self.order.iter().position(|k| k == key) {
                self.order.remove(pos);
                self.order.push_back(key.clone());
            }
            Some(circuit)
        } else {
            self.stats.misses += 1;
            None
        }
    }

    /// Insert a compiled circuit under the given key.
    ///
    /// If `key` already exists the call is a no-op (idempotent insert: the
    /// first compilation wins and redundant compilations from concurrent
    /// requests are discarded cheaply).  If the cache is at capacity the LRU
    /// entry is evicted first.
    fn insert(&mut self, key: CircuitCacheKey, circuit: Circuit) {
        if self.map.contains_key(&key) {
            return; // already cached; first write wins
        }
        // Evict until there is room
        while self.map.len() >= self.config.max_entries {
            if let Some(lru_key) = self.order.pop_front() {
                self.map.remove(&lru_key);
                self.stats.evictions += 1;
            } else {
                break; // empty order queue — should not happen
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, circuit);
        self.stats.current_size = self.map.len();
    }

    /// Return a snapshot of the current statistics (with `current_size` updated).
    fn stats_snapshot(&self) -> CircuitCacheStats {
        CircuitCacheStats {
            current_size: self.map.len(),
            ..self.stats.clone()
        }
    }
}

// ---------------------------------------------------------------------------
// CircuitCache (public, Clone-able handle)
// ---------------------------------------------------------------------------

/// Thread-safe LRU cache for compiled FHE circuits.
///
/// Cloning a `CircuitCache` produces a second handle that shares the same
/// underlying cache, making it cheap to hand out per-request copies.
///
/// # Example
///
/// ```rust,ignore
/// let cache = CircuitCache::new(CircuitCacheConfig::default());
/// let circuit = cache.get_or_compile(&predicate, || {
///     let mut compiler = PredicateCompiler::new();
///     compiler.compile(&predicate, EncryptedType::U8)
/// })?;
/// ```
#[derive(Clone)]
pub struct CircuitCache {
    inner: Arc<Mutex<CacheInner>>,
}

impl std::fmt::Debug for CircuitCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.inner.lock().stats_snapshot();
        f.debug_struct("CircuitCache")
            .field("current_size", &stats.current_size)
            .field("hits", &stats.hits)
            .field("misses", &stats.misses)
            .field("evictions", &stats.evictions)
            .finish()
    }
}

impl CircuitCache {
    /// Create a new cache with the given configuration.
    pub fn new(config: CircuitCacheConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CacheInner::new(config))),
        }
    }

    /// Look up a pre-computed cache key in the cache.
    ///
    /// Returns `Some(Circuit)` on a hit or `None` on a miss.
    pub fn get(&self, key: &CircuitCacheKey) -> Option<Circuit> {
        self.inner.lock().get(key)
    }

    /// Insert a compiled circuit into the cache.
    ///
    /// If the key is already present the call is a no-op.
    pub fn insert(&self, key: CircuitCacheKey, circuit: Circuit) {
        self.inner.lock().insert(key, circuit);
    }

    /// Get a cached circuit or compile it on a miss.
    ///
    /// On a cache miss `compile_fn` is called exactly once to produce the
    /// circuit, which is then stored before being returned.  On a cache hit
    /// `compile_fn` is never called.
    ///
    /// # Errors
    ///
    /// Returns the error produced by `compile_fn` on a miss if compilation
    /// fails.  Cache hits never return an error from this method.
    pub fn get_or_compile<F>(
        &self,
        predicate: &Predicate,
        compile_fn: F,
    ) -> amaters_core::error::Result<Circuit>
    where
        F: FnOnce() -> amaters_core::error::Result<Circuit>,
    {
        let key = CircuitCacheKey::from_predicate(predicate);

        // Fast path: cache hit — lock acquired and released before compile_fn
        if let Some(cached) = self.get(&key) {
            return Ok(cached);
        }

        // Slow path: compile and cache
        let circuit = compile_fn()?;
        self.insert(key, circuit.clone());
        Ok(circuit)
    }

    /// Return a point-in-time snapshot of the cache statistics.
    pub fn stats(&self) -> CircuitCacheStats {
        self.inner.lock().stats_snapshot()
    }

    /// Number of entries currently held in the cache.
    pub fn len(&self) -> usize {
        self.inner.lock().map.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use amaters_core::compute::{CircuitBuilder, EncryptedType};
    use amaters_core::types::{CipherBlob, ColumnRef, Predicate};

    /// Build the simplest valid circuit: a single Load("x") node with type U8.
    fn make_simple_circuit() -> Circuit {
        let mut builder = CircuitBuilder::new();
        builder.declare_variable("x", EncryptedType::U8);
        let x = builder.load("x");
        builder.build(x).expect("simple circuit should build")
    }

    /// Construct a simple deterministic predicate for use as a test fixture.
    fn make_predicate() -> Predicate {
        Predicate::Eq(
            ColumnRef::new("col".to_string()),
            CipherBlob::new(b"test".to_vec()),
        )
    }

    // 1. Initial lookup is a miss; subsequent lookup is a hit.
    #[test]
    fn test_cache_miss_then_hit() {
        let cache = CircuitCache::new(CircuitCacheConfig::default());
        let pred = make_predicate();
        let key = CircuitCacheKey::from_predicate(&pred);

        // First lookup: miss
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 0);

        // Insert the circuit
        cache.insert(key.clone(), make_simple_circuit());

        // Second lookup: hit
        assert!(cache.get(&key).is_some());
        assert_eq!(cache.stats().hits, 1);
    }

    // 2. Eviction fires when capacity is exceeded.
    #[test]
    fn test_eviction_at_capacity() {
        let cache = CircuitCache::new(CircuitCacheConfig::new(2));

        let preds: Vec<Predicate> = (0u8..3)
            .map(|i| Predicate::Eq(ColumnRef::new(format!("col{i}")), CipherBlob::new(vec![i])))
            .collect();

        // Fill to capacity
        for pred in &preds[..2] {
            let k = CircuitCacheKey::from_predicate(pred);
            cache.insert(k, make_simple_circuit());
        }
        assert_eq!(cache.len(), 2);

        // Insert a third entry — LRU (first inserted) must be evicted
        let key3 = CircuitCacheKey::from_predicate(&preds[2]);
        cache.insert(key3, make_simple_circuit());
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.stats().evictions, 1);

        // The first predicate's key should have been evicted
        let key0 = CircuitCacheKey::from_predicate(&preds[0]);
        assert!(
            cache.get(&key0).is_none(),
            "LRU entry should have been evicted"
        );
    }

    // 3. get_or_compile does NOT invoke the closure on a cache hit.
    #[test]
    fn test_get_or_compile_hit_skips_compile_fn() {
        let cache = CircuitCache::new(CircuitCacheConfig::default());
        let pred = make_predicate();
        let key = CircuitCacheKey::from_predicate(&pred);
        cache.insert(key, make_simple_circuit());

        let mut called = false;
        let result = cache.get_or_compile(&pred, || {
            called = true;
            Ok(make_simple_circuit())
        });
        assert!(result.is_ok(), "get_or_compile should succeed");
        assert!(!called, "compile_fn must not be called on a cache hit");
    }

    // 4. get_or_compile invokes the closure on a cache miss and caches the result.
    #[test]
    fn test_get_or_compile_miss_calls_compile_fn_and_caches() {
        let cache = CircuitCache::new(CircuitCacheConfig::default());
        let pred = make_predicate();

        let mut call_count = 0u32;
        let result = cache.get_or_compile(&pred, || {
            call_count += 1;
            Ok(make_simple_circuit())
        });
        assert!(result.is_ok());
        assert_eq!(call_count, 1, "compile_fn should be called exactly once");

        // Second call: should be served from cache
        let result2 = cache.get_or_compile(&pred, || {
            call_count += 1;
            Ok(make_simple_circuit())
        });
        assert!(result2.is_ok());
        assert_eq!(
            call_count, 1,
            "compile_fn must not be called again on a subsequent hit"
        );
    }

    // 5. Two structurally different predicates produce distinct keys.
    #[test]
    fn test_different_predicates_different_keys() {
        let p1 = Predicate::Eq(ColumnRef::new("a".to_string()), CipherBlob::new(vec![1]));
        let p2 = Predicate::Eq(ColumnRef::new("b".to_string()), CipherBlob::new(vec![2]));
        let k1 = CircuitCacheKey::from_predicate(&p1);
        let k2 = CircuitCacheKey::from_predicate(&p2);
        assert_ne!(k1, k2, "distinct predicates must map to distinct keys");
    }

    // 6. The same predicate always produces the same key (determinism).
    #[test]
    fn test_same_predicate_same_key() {
        let pred = make_predicate();
        let k1 = CircuitCacheKey::from_predicate(&pred);
        let k2 = CircuitCacheKey::from_predicate(&pred);
        assert_eq!(k1, k2, "same predicate must always map to the same key");
    }

    // 7. Clone shares the underlying cache.
    #[test]
    fn test_clone_shares_underlying_cache() {
        let cache1 = CircuitCache::new(CircuitCacheConfig::default());
        let cache2 = cache1.clone();
        let pred = make_predicate();
        let key = CircuitCacheKey::from_predicate(&pred);
        cache1.insert(key.clone(), make_simple_circuit());
        assert!(
            cache2.get(&key).is_some(),
            "cloned handle must see entries inserted via the original handle"
        );
    }

    // 8. hit_rate returns 0.0 before any lookups.
    #[test]
    fn test_hit_rate_zero_when_no_lookups() {
        let cache = CircuitCache::new(CircuitCacheConfig::default());
        let stats = cache.stats();
        assert!(
            (stats.hit_rate() - 0.0).abs() < f64::EPSILON,
            "hit_rate should be 0.0 with no lookups"
        );
    }

    // 9. hit_rate reflects accumulated hit/miss counts accurately.
    #[test]
    fn test_hit_rate_accuracy() {
        let cache = CircuitCache::new(CircuitCacheConfig::default());
        let pred = make_predicate();
        let key = CircuitCacheKey::from_predicate(&pred);
        cache.insert(key.clone(), make_simple_circuit());

        // 3 hits
        for _ in 0..3 {
            let _ = cache.get(&key);
        }
        // 1 miss (different key)
        let other = Predicate::Gt(ColumnRef::new("z".to_string()), CipherBlob::new(vec![9]));
        let _ = cache.get(&CircuitCacheKey::from_predicate(&other));

        let stats = cache.stats();
        // 3 / (3 + 1) == 0.75
        assert!(
            (stats.hit_rate() - 0.75).abs() < 1e-9,
            "expected hit_rate 0.75, got {}",
            stats.hit_rate()
        );
    }

    // 10. Idempotent insert: a second insert with the same key is a no-op.
    #[test]
    fn test_idempotent_insert() {
        let cache = CircuitCache::new(CircuitCacheConfig::default());
        let pred = make_predicate();
        let key = CircuitCacheKey::from_predicate(&pred);

        cache.insert(key.clone(), make_simple_circuit());
        cache.insert(key.clone(), make_simple_circuit()); // second insert same key
        assert_eq!(cache.len(), 1, "duplicate insert must not grow the cache");
    }

    // 11. LRU ordering: accessing an entry promotes it, protecting it from eviction.
    #[test]
    fn test_lru_promotion_protects_from_eviction() {
        let cache = CircuitCache::new(CircuitCacheConfig::new(2));

        let pred_a = Predicate::Eq(ColumnRef::new("a".to_string()), CipherBlob::new(vec![0]));
        let pred_b = Predicate::Gt(ColumnRef::new("b".to_string()), CipherBlob::new(vec![1]));
        let pred_c = Predicate::Lt(ColumnRef::new("c".to_string()), CipherBlob::new(vec![2]));

        let key_a = CircuitCacheKey::from_predicate(&pred_a);
        let key_b = CircuitCacheKey::from_predicate(&pred_b);
        let key_c = CircuitCacheKey::from_predicate(&pred_c);

        // Insert A and B (fills capacity)
        cache.insert(key_a.clone(), make_simple_circuit());
        cache.insert(key_b.clone(), make_simple_circuit());

        // Access A (promotes it to MRU; B becomes LRU)
        let _ = cache.get(&key_a);

        // Insert C — should evict B (LRU), not A
        cache.insert(key_c.clone(), make_simple_circuit());

        assert!(cache.get(&key_a).is_some(), "A should survive (promoted)");
        assert!(cache.get(&key_b).is_none(), "B should be evicted (LRU)");
        assert!(cache.get(&key_c).is_some(), "C was just inserted");
    }

    // 12. Debug formatting does not panic and contains expected fields.
    #[test]
    fn test_debug_format() {
        let cache = CircuitCache::new(CircuitCacheConfig::default());
        let dbg = format!("{:?}", cache);
        assert!(
            dbg.contains("CircuitCache"),
            "debug string should name the type"
        );
    }
}
