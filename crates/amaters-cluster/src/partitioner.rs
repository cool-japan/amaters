//! Key range partitioning and query routing
//!
//! This module provides partitioning strategies for distributing keys across shards
//! and routing queries to the correct shard(s).

use crate::error::{RaftError, RaftResult};
use crate::shard::{KeyRange, ShardId, ShardMetadata, ShardRegistry};
use crate::types::NodeId;
use amaters_core::Key;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Partitioning strategy for key distribution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionStrategy {
    /// Range-based partitioning (keys sorted by range)
    Range,
    /// Hash-based partitioning (keys distributed by hash)
    Hash,
    /// Consistent hashing (virtual nodes for better distribution)
    ConsistentHash,
}

/// Hash function for key partitioning
fn hash_key(key: &Key) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// A maintained consistent hash ring backed by a `BTreeMap`.
///
/// Virtual nodes for each shard are inserted at construction and maintained
/// incrementally via `add_shard`/`remove_shard`, avoiding the O(S·V·log(S·V))
/// rebuild on every lookup.
#[derive(Clone)]
pub struct HashRing {
    /// Sorted map from virtual-node hash → ShardId.
    ring: std::collections::BTreeMap<u64, ShardId>,
    /// Number of virtual nodes per shard.
    virtual_nodes: usize,
}

impl HashRing {
    /// Create an empty ring with the given number of virtual nodes per shard.
    pub fn new(virtual_nodes: usize) -> Self {
        Self {
            ring: std::collections::BTreeMap::new(),
            virtual_nodes,
        }
    }

    /// Insert `virtual_nodes` entries for the given shard.
    pub fn add_shard(&mut self, id: ShardId) {
        for i in 0..self.virtual_nodes {
            let hash = Self::virtual_node_hash(id, i);
            self.ring.insert(hash, id);
        }
    }

    /// Remove all virtual-node entries for the given shard.
    pub fn remove_shard(&mut self, id: ShardId) {
        for i in 0..self.virtual_nodes {
            let hash = Self::virtual_node_hash(id, i);
            // Only remove if this slot still maps to our shard (no hash collision replacement).
            if self.ring.get(&hash) == Some(&id) {
                self.ring.remove(&hash);
            }
        }
    }

    /// Route a key to the responsible shard.
    ///
    /// Returns `None` if the ring is empty.
    pub fn get_shard_for_key(&self, key: &Key) -> Option<ShardId> {
        if self.ring.is_empty() {
            return None;
        }
        let key_hash = hash_key(key);
        // Find the first virtual node with hash >= key_hash (successor).
        self.ring
            .range(key_hash..)
            .next()
            .or_else(|| self.ring.iter().next()) // wrap-around to first entry
            .map(|(_, &id)| id)
    }

    /// Hash function for virtual node `(shard_id, i)`.
    fn virtual_node_hash(shard_id: ShardId, i: usize) -> u64 {
        use std::hash::{Hash, Hasher};
        let virtual_key = format!("{}:{}", shard_id, i);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        virtual_key.hash(&mut hasher);
        hasher.finish()
    }
}

/// Partitioner handles key-to-shard routing
#[derive(Clone)]
pub struct Partitioner {
    /// Shard registry
    registry: Arc<ShardRegistry>,
    /// Partitioning strategy
    strategy: PartitionStrategy,
    /// Number of virtual nodes for consistent hashing
    virtual_nodes: usize,
    /// Maintained consistent hash ring (initialized from registry at construction).
    hash_ring: HashRing,
}

impl Partitioner {
    /// Create a new partitioner
    pub fn new(registry: Arc<ShardRegistry>, strategy: PartitionStrategy) -> Self {
        let virtual_nodes = 100;
        let mut hash_ring = HashRing::new(virtual_nodes);
        for shard in registry.get_all() {
            hash_ring.add_shard(shard.id);
        }
        Self {
            registry,
            strategy,
            virtual_nodes,
            hash_ring,
        }
    }

    /// Set the number of virtual nodes for consistent hashing
    pub fn with_virtual_nodes(mut self, count: usize) -> Self {
        self.virtual_nodes = count;
        // Rebuild the hash ring with the new virtual node count.
        let mut new_ring = HashRing::new(count);
        for shard in self.registry.get_all() {
            new_ring.add_shard(shard.id);
        }
        self.hash_ring = new_ring;
        self
    }

    /// Route a key to the responsible shard
    pub fn route_key(&self, key: &Key) -> RaftResult<ShardMetadata> {
        match self.strategy {
            PartitionStrategy::Range => self.route_by_range(key),
            PartitionStrategy::Hash => self.route_by_hash(key),
            PartitionStrategy::ConsistentHash => self.route_by_consistent_hash(key),
        }
    }

    /// Route by range partitioning
    fn route_by_range(&self, key: &Key) -> RaftResult<ShardMetadata> {
        self.registry
            .find_shard_for_key(key)
            .ok_or_else(|| RaftError::ConfigError {
                message: format!("No shard found for key: {:?}", key),
            })
    }

    /// Route by hash partitioning
    fn route_by_hash(&self, key: &Key) -> RaftResult<ShardMetadata> {
        let shards = self.registry.get_all();
        if shards.is_empty() {
            return Err(RaftError::ConfigError {
                message: "No shards available".to_string(),
            });
        }

        let hash = hash_key(key);
        let index = (hash % shards.len() as u64) as usize;
        Ok(shards[index].clone())
    }

    /// Route by consistent hashing using the maintained hash ring.
    fn route_by_consistent_hash(&self, key: &Key) -> RaftResult<ShardMetadata> {
        let shard_id =
            self.hash_ring
                .get_shard_for_key(key)
                .ok_or_else(|| RaftError::ConfigError {
                    message: "Consistent hash ring is empty — no shards registered".to_string(),
                })?;

        self.registry
            .get(shard_id)
            .ok_or_else(|| RaftError::ConfigError {
                message: format!("Shard {} not found in registry", shard_id),
            })
    }

    /// Route a key range query to all relevant shards
    pub fn route_range(&self, start: &Key, end: &Key) -> RaftResult<Vec<ShardMetadata>> {
        let query_range = KeyRange::new(start.clone(), end.clone())?;
        let shards = self.registry.get_all();

        let relevant_shards: Vec<ShardMetadata> = shards
            .into_iter()
            .filter(|shard| shard.range.overlaps(&query_range))
            .collect();

        if relevant_shards.is_empty() {
            return Err(RaftError::ConfigError {
                message: format!("No shards found for range {:?} to {:?}", start, end),
            });
        }

        Ok(relevant_shards)
    }

    /// Get all shards on a specific node
    pub fn get_shards_on_node(&self, node_id: NodeId) -> Vec<ShardMetadata> {
        self.registry.get_by_node(node_id)
    }

    /// Get all shards in the cluster
    pub fn get_all_shards(&self) -> Vec<ShardMetadata> {
        self.registry.get_all()
    }
}

/// Query router for distributed queries
pub struct QueryRouter {
    partitioner: Partitioner,
}

impl QueryRouter {
    /// Create a new query router
    pub fn new(partitioner: Partitioner) -> Self {
        Self { partitioner }
    }

    /// Route a point query (single key lookup)
    pub fn route_point_query(&self, key: &Key) -> RaftResult<QueryPlan> {
        let shard = self.partitioner.route_key(key)?;
        Ok(QueryPlan::Single {
            shard_id: shard.id,
            node_id: shard.node_id,
        })
    }

    /// Route a range query (multiple keys)
    pub fn route_range_query(&self, start: &Key, end: &Key) -> RaftResult<QueryPlan> {
        let shards = self.partitioner.route_range(start, end)?;

        let mut targets: HashMap<NodeId, Vec<ShardId>> = HashMap::new();
        for shard in shards {
            targets.entry(shard.node_id).or_default().push(shard.id);
        }

        Ok(QueryPlan::Scatter {
            targets,
            merge_required: true,
        })
    }

    /// Route a full scan query (all shards)
    pub fn route_scan_query(&self) -> RaftResult<QueryPlan> {
        let shards = self.partitioner.get_all_shards();
        if shards.is_empty() {
            return Err(RaftError::ConfigError {
                message: "No shards available for scan".to_string(),
            });
        }

        let mut targets: HashMap<NodeId, Vec<ShardId>> = HashMap::new();
        for shard in shards {
            targets.entry(shard.node_id).or_default().push(shard.id);
        }

        Ok(QueryPlan::Scatter {
            targets,
            merge_required: true,
        })
    }

    /// Get statistics for query planning
    pub fn get_query_stats(&self) -> QueryStats {
        let shards = self.partitioner.get_all_shards();
        let total_shards = shards.len();
        let nodes: HashSet<NodeId> = shards.iter().map(|s| s.node_id).collect();
        let total_nodes = nodes.len();

        let total_keys: u64 = shards.iter().map(|s| s.estimated_keys).sum();
        let total_size: u64 = shards.iter().map(|s| s.estimated_size_bytes).sum();

        QueryStats {
            total_shards,
            total_nodes,
            total_keys,
            total_size_bytes: total_size,
        }
    }
}

/// Query execution plan
#[derive(Debug, Clone)]
pub enum QueryPlan {
    /// Single shard query
    Single {
        /// Target shard ID
        shard_id: ShardId,
        /// Target node ID
        node_id: NodeId,
    },
    /// Multi-shard scatter-gather query
    Scatter {
        /// Map of node ID to shard IDs
        targets: HashMap<NodeId, Vec<ShardId>>,
        /// Whether results need to be merged
        merge_required: bool,
    },
}

impl QueryPlan {
    /// Get all nodes involved in the query
    pub fn get_nodes(&self) -> Vec<NodeId> {
        match self {
            QueryPlan::Single { node_id, .. } => vec![*node_id],
            QueryPlan::Scatter { targets, .. } => targets.keys().copied().collect(),
        }
    }

    /// Get all shards involved in the query
    pub fn get_shards(&self) -> Vec<ShardId> {
        match self {
            QueryPlan::Single { shard_id, .. } => vec![*shard_id],
            QueryPlan::Scatter { targets, .. } => targets.values().flatten().copied().collect(),
        }
    }

    /// Check if the query requires result merging
    pub fn requires_merge(&self) -> bool {
        match self {
            QueryPlan::Single { .. } => false,
            QueryPlan::Scatter { merge_required, .. } => *merge_required,
        }
    }
}

/// Query statistics for optimization
#[derive(Debug, Clone)]
pub struct QueryStats {
    /// Total number of shards
    pub total_shards: usize,
    /// Total number of nodes
    pub total_nodes: usize,
    /// Total estimated keys across all shards
    pub total_keys: u64,
    /// Total estimated size in bytes
    pub total_size_bytes: u64,
}

impl QueryStats {
    /// Get average keys per shard
    pub fn avg_keys_per_shard(&self) -> u64 {
        if self.total_shards == 0 {
            0
        } else {
            self.total_keys / self.total_shards as u64
        }
    }

    /// Get average size per shard
    pub fn avg_size_per_shard(&self) -> u64 {
        if self.total_shards == 0 {
            0
        } else {
            self.total_size_bytes / self.total_shards as u64
        }
    }

    /// Get average shards per node
    pub fn avg_shards_per_node(&self) -> f64 {
        if self.total_nodes == 0 {
            0.0
        } else {
            self.total_shards as f64 / self.total_nodes as f64
        }
    }
}

/// Range-based partitioner backed by a sorted `BTreeMap`.
///
/// Maps key-space ranges to shards via range-start bytes.  Each shard owns
/// the half-open interval `[start, next_start)`.  The shard whose range start
/// is the largest value ≤ the lookup key is selected.
pub struct RangePartitioner {
    /// BTreeMap from range-start bytes (inclusive) → ShardId.
    /// The owning shard covers [start, next_start).
    ranges: std::collections::BTreeMap<Vec<u8>, ShardId>,
}

impl RangePartitioner {
    /// Create an empty range partitioner.
    pub fn new() -> Self {
        Self {
            ranges: std::collections::BTreeMap::new(),
        }
    }

    /// Register a shard that owns the range starting at `start` (inclusive).
    pub fn add_range(&mut self, start: Vec<u8>, shard_id: ShardId) {
        self.ranges.insert(start, shard_id);
    }

    /// Remove the range entry whose start equals `start`.
    pub fn remove_range(&mut self, start: &[u8]) {
        self.ranges.remove(start);
    }

    /// Route a key to its responsible shard.
    ///
    /// Finds the shard whose range start is the greatest value ≤ the key.
    /// Returns `None` if no range covers the key (i.e., key is before all ranges).
    pub fn get_shard_for_key(&self, key: &Key) -> Option<ShardId> {
        let key_bytes = key.as_bytes().to_vec();
        self.ranges
            .range(..=key_bytes)
            .next_back()
            .map(|(_, &id)| id)
    }

    /// Build a `RangePartitioner` from a `ShardRegistry`, using each shard's
    /// range start as the routing key.
    pub fn from_registry(registry: &ShardRegistry) -> Self {
        let mut rp = Self::new();
        for shard in registry.get_all() {
            rp.add_range(shard.range.start.as_bytes().to_vec(), shard.id);
        }
        rp
    }
}

impl Default for RangePartitioner {
    fn default() -> Self {
        Self::new()
    }
}

/// Item tracked in the k-way merge heap.
///
/// Stores the value alongside its shard and position indices so that
/// the next element from the same shard can be pushed after a pop.
struct MergeItem<T> {
    value: T,
    shard_idx: usize,
    item_idx: usize,
}

// We want a *min*-heap but `BinaryHeap` is a max-heap, so we reverse
// the ordering.  Two items from different shards that compare equal are
// tie-broken by shard index to give a stable, deterministic output.
impl<T: Ord> PartialEq for MergeItem<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.shard_idx == other.shard_idx
    }
}

impl<T: Ord> Eq for MergeItem<T> {}

impl<T: Ord> PartialOrd for MergeItem<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Ord> Ord for MergeItem<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse so that BinaryHeap (max-heap) behaves as a min-heap.
        // Tie-break on shard_idx for determinism.
        other
            .value
            .cmp(&self.value)
            .then_with(|| other.shard_idx.cmp(&self.shard_idx))
    }
}

/// Wrapper used for `merge_sorted_by_key` where the sort key is extracted
/// via a closure and may differ from the value's natural ordering.
struct MergeItemByKey<T, K> {
    value: T,
    key: K,
    shard_idx: usize,
    item_idx: usize,
}

impl<T, K: Ord> PartialEq for MergeItemByKey<T, K> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.shard_idx == other.shard_idx
    }
}

impl<T, K: Ord> Eq for MergeItemByKey<T, K> {}

impl<T, K: Ord> PartialOrd for MergeItemByKey<T, K> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T, K: Ord> Ord for MergeItemByKey<T, K> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .key
            .cmp(&self.key)
            .then_with(|| other.shard_idx.cmp(&self.shard_idx))
    }
}

/// Result merger for scatter-gather queries.
///
/// Provides several strategies for combining results returned from
/// multiple shards:
///
/// - **`merge`** – simple concatenation (no ordering guarantees).
/// - **`merge_sorted`** – efficient O(N log K) k-way merge that
///   assumes each input `Vec` is already sorted.
/// - **`merge_sorted_by_key`** – same algorithm but sorts by a
///   caller-supplied key extractor, useful for `(Key, Value)` tuples.
/// - **`merge_deduplicate`** – unordered merge with duplicate removal.
/// - **`merge_sorted_deduplicate`** – ordered merge with duplicate removal.
pub struct ResultMerger;

impl ResultMerger {
    /// Merge results from multiple shards by simple concatenation.
    ///
    /// No ordering guarantees.  O(N) where N is the total number of items.
    pub fn merge<T>(results: Vec<Vec<T>>) -> Vec<T> {
        let total_len: usize = results.iter().map(|v| v.len()).sum();
        let mut merged = Vec::with_capacity(total_len);
        for batch in results {
            merged.extend(batch);
        }
        merged
    }

    /// Merge pre-sorted shard results using an efficient k-way merge.
    ///
    /// Each input `Vec` **must** be sorted in ascending order.  The output
    /// is a single sorted `Vec`.
    ///
    /// Complexity: O(N log K) where N = total items, K = number of shards.
    pub fn merge_sorted<T>(results: Vec<Vec<T>>) -> Vec<T>
    where
        T: Ord,
    {
        let total_len: usize = results.iter().map(|v| v.len()).sum();
        if total_len == 0 {
            return Vec::new();
        }

        // Convert each Vec into an owning iterator so we can pull items
        // one-by-one without cloning.
        let mut iterators: Vec<std::vec::IntoIter<T>> =
            results.into_iter().map(|v| v.into_iter()).collect();

        let mut heap: BinaryHeap<MergeItem<T>> = BinaryHeap::with_capacity(iterators.len());

        // Seed the heap with the first element from each non-empty shard.
        for (shard_idx, iter) in iterators.iter_mut().enumerate() {
            if let Some(value) = iter.next() {
                heap.push(MergeItem {
                    value,
                    shard_idx,
                    item_idx: 0,
                });
            }
        }

        let mut merged = Vec::with_capacity(total_len);

        while let Some(item) = heap.pop() {
            let next_item_idx = item.item_idx + 1;
            let shard_idx = item.shard_idx;
            merged.push(item.value);

            // Push the next element from the same shard, if available.
            if let Some(value) = iterators[shard_idx].next() {
                heap.push(MergeItem {
                    value,
                    shard_idx,
                    item_idx: next_item_idx,
                });
            }
        }

        merged
    }

    /// Merge pre-sorted shard results by a key extracted via `key_fn`.
    ///
    /// Each input `Vec` **must** be sorted in ascending order of the key.
    /// Useful for merging `(Key, CipherBlob)` tuples where sorting is
    /// done on the `Key` component.
    ///
    /// Complexity: O(N log K) where N = total items, K = number of shards.
    pub fn merge_sorted_by_key<T, K, F>(results: Vec<Vec<T>>, key_fn: F) -> Vec<T>
    where
        K: Ord,
        F: Fn(&T) -> K,
    {
        let total_len: usize = results.iter().map(|v| v.len()).sum();
        if total_len == 0 {
            return Vec::new();
        }

        let mut iterators: Vec<std::vec::IntoIter<T>> =
            results.into_iter().map(|v| v.into_iter()).collect();

        let mut heap: BinaryHeap<MergeItemByKey<T, K>> = BinaryHeap::with_capacity(iterators.len());

        for (shard_idx, iter) in iterators.iter_mut().enumerate() {
            if let Some(value) = iter.next() {
                let key = key_fn(&value);
                heap.push(MergeItemByKey {
                    value,
                    key,
                    shard_idx,
                    item_idx: 0,
                });
            }
        }

        let mut merged = Vec::with_capacity(total_len);

        while let Some(item) = heap.pop() {
            let next_item_idx = item.item_idx + 1;
            let shard_idx = item.shard_idx;
            merged.push(item.value);

            if let Some(value) = iterators[shard_idx].next() {
                let key = key_fn(&value);
                heap.push(MergeItemByKey {
                    value,
                    key,
                    shard_idx,
                    item_idx: next_item_idx,
                });
            }
        }

        merged
    }

    /// Merge with deduplication (unordered).
    pub fn merge_deduplicate<T>(results: Vec<Vec<T>>) -> Vec<T>
    where
        T: Eq + Hash,
    {
        let mut set: HashSet<T> = HashSet::new();
        for batch in results {
            set.extend(batch);
        }
        set.into_iter().collect()
    }

    /// Merge pre-sorted shard results with deduplication.
    ///
    /// Uses the k-way merge algorithm and skips consecutive duplicates.
    /// Each input `Vec` **must** be sorted and should not contain
    /// duplicates within a single shard for best results.
    ///
    /// Complexity: O(N log K) where N = total items, K = number of shards.
    pub fn merge_sorted_deduplicate<T>(results: Vec<Vec<T>>) -> Vec<T>
    where
        T: Ord,
    {
        let total_len: usize = results.iter().map(|v| v.len()).sum();
        if total_len == 0 {
            return Vec::new();
        }

        let mut iterators: Vec<std::vec::IntoIter<T>> =
            results.into_iter().map(|v| v.into_iter()).collect();

        let mut heap: BinaryHeap<MergeItem<T>> = BinaryHeap::with_capacity(iterators.len());

        for (shard_idx, iter) in iterators.iter_mut().enumerate() {
            if let Some(value) = iter.next() {
                heap.push(MergeItem {
                    value,
                    shard_idx,
                    item_idx: 0,
                });
            }
        }

        let mut merged = Vec::with_capacity(total_len);

        while let Some(item) = heap.pop() {
            let next_item_idx = item.item_idx + 1;
            let shard_idx = item.shard_idx;

            // Skip duplicate if the last pushed element is equal.
            let is_dup = merged.last().is_some_and(|last: &T| last == &item.value);
            if !is_dup {
                merged.push(item.value);
            }

            if let Some(value) = iterators[shard_idx].next() {
                heap.push(MergeItem {
                    value,
                    shard_idx,
                    item_idx: next_item_idx,
                });
            }
        }

        merged.shrink_to_fit();
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_registry() -> Arc<ShardRegistry> {
        let registry = Arc::new(ShardRegistry::new());

        // Create 3 shards with non-overlapping ranges
        let range1 = KeyRange::new(Key::from_str("a"), Key::from_str("h")).expect("valid range");
        let shard1 = ShardMetadata::new(1, range1, 100);
        registry.register(shard1).expect("register shard 1");

        let range2 = KeyRange::new(Key::from_str("h"), Key::from_str("p")).expect("valid range");
        let shard2 = ShardMetadata::new(2, range2, 101);
        registry.register(shard2).expect("register shard 2");

        let range3 = KeyRange::new(Key::from_str("p"), Key::from_str("z")).expect("valid range");
        let shard3 = ShardMetadata::new(3, range3, 102);
        registry.register(shard3).expect("register shard 3");

        registry
    }

    #[test]
    fn test_partitioner_range_routing() -> RaftResult<()> {
        let registry = create_test_registry();
        let partitioner = Partitioner::new(registry, PartitionStrategy::Range);

        let shard = partitioner.route_key(&Key::from_str("d"))?;
        assert_eq!(shard.id, 1);

        let shard = partitioner.route_key(&Key::from_str("m"))?;
        assert_eq!(shard.id, 2);

        let shard = partitioner.route_key(&Key::from_str("x"))?;
        assert_eq!(shard.id, 3);

        Ok(())
    }

    #[test]
    fn test_partitioner_hash_routing() -> RaftResult<()> {
        let registry = create_test_registry();
        let partitioner = Partitioner::new(registry, PartitionStrategy::Hash);

        // Hash routing should be deterministic
        let shard1 = partitioner.route_key(&Key::from_str("test_key"))?;
        let shard2 = partitioner.route_key(&Key::from_str("test_key"))?;
        assert_eq!(shard1.id, shard2.id);

        Ok(())
    }

    #[test]
    fn test_partitioner_consistent_hash_routing() -> RaftResult<()> {
        let registry = create_test_registry();
        let partitioner =
            Partitioner::new(registry, PartitionStrategy::ConsistentHash).with_virtual_nodes(50);

        // Consistent hashing should be deterministic
        let shard1 = partitioner.route_key(&Key::from_str("test_key"))?;
        let shard2 = partitioner.route_key(&Key::from_str("test_key"))?;
        assert_eq!(shard1.id, shard2.id);

        Ok(())
    }

    #[test]
    fn test_partitioner_range_query() -> RaftResult<()> {
        let registry = create_test_registry();
        let partitioner = Partitioner::new(registry, PartitionStrategy::Range);

        // Query spanning two shards
        let shards = partitioner.route_range(&Key::from_str("d"), &Key::from_str("m"))?;
        assert_eq!(shards.len(), 2);

        // Query spanning all shards
        let shards = partitioner.route_range(&Key::from_str("a"), &Key::from_str("z"))?;
        assert_eq!(shards.len(), 3);

        Ok(())
    }

    #[test]
    fn test_query_router_point_query() -> RaftResult<()> {
        let registry = create_test_registry();
        let partitioner = Partitioner::new(registry, PartitionStrategy::Range);
        let router = QueryRouter::new(partitioner);

        let plan = router.route_point_query(&Key::from_str("d"))?;
        match plan {
            QueryPlan::Single { shard_id, node_id } => {
                assert_eq!(shard_id, 1);
                assert_eq!(node_id, 100);
            }
            _ => panic!("Expected single query plan"),
        }

        Ok(())
    }

    #[test]
    fn test_query_router_range_query() -> RaftResult<()> {
        let registry = create_test_registry();
        let partitioner = Partitioner::new(registry, PartitionStrategy::Range);
        let router = QueryRouter::new(partitioner);

        let plan = router.route_range_query(&Key::from_str("d"), &Key::from_str("m"))?;
        match plan {
            QueryPlan::Scatter {
                targets,
                merge_required,
            } => {
                assert!(merge_required);
                assert_eq!(targets.len(), 2); // Two nodes involved
            }
            _ => panic!("Expected scatter query plan"),
        }

        Ok(())
    }

    #[test]
    fn test_query_router_scan_query() -> RaftResult<()> {
        let registry = create_test_registry();
        let partitioner = Partitioner::new(registry, PartitionStrategy::Range);
        let router = QueryRouter::new(partitioner);

        let plan = router.route_scan_query()?;
        match plan {
            QueryPlan::Scatter { targets, .. } => {
                assert_eq!(targets.len(), 3); // All nodes involved
            }
            _ => panic!("Expected scatter query plan"),
        }

        Ok(())
    }

    #[test]
    fn test_query_stats() -> RaftResult<()> {
        let registry = create_test_registry();
        let partitioner = Partitioner::new(registry, PartitionStrategy::Range);
        let router = QueryRouter::new(partitioner);

        let stats = router.get_query_stats();
        assert_eq!(stats.total_shards, 3);
        assert_eq!(stats.total_nodes, 3);

        Ok(())
    }

    #[test]
    fn test_query_plan_methods() -> RaftResult<()> {
        let mut targets = HashMap::new();
        targets.insert(100, vec![1, 2]);
        targets.insert(101, vec![3]);

        let plan = QueryPlan::Scatter {
            targets,
            merge_required: true,
        };

        let nodes = plan.get_nodes();
        assert_eq!(nodes.len(), 2);

        let shards = plan.get_shards();
        assert_eq!(shards.len(), 3);

        assert!(plan.requires_merge());

        Ok(())
    }

    // ---------------------------------------------------------------
    //  ResultMerger tests
    // ---------------------------------------------------------------

    #[test]
    fn test_result_merger_merge_concatenates() {
        let results = vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 9]];
        let merged = ResultMerger::merge(results);
        assert_eq!(merged, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn test_result_merger_merge_empty_inputs() {
        let results: Vec<Vec<i32>> = vec![];
        let merged = ResultMerger::merge(results);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_result_merger_merge_some_empty_vecs() {
        let results: Vec<Vec<i32>> = vec![vec![], vec![1, 2], vec![], vec![3]];
        let merged = ResultMerger::merge(results);
        assert_eq!(merged, vec![1, 2, 3]);
    }

    #[test]
    fn test_result_merger_merge_all_empty_vecs() {
        let results: Vec<Vec<i32>> = vec![vec![], vec![], vec![]];
        let merged = ResultMerger::merge(results);
        assert!(merged.is_empty());
    }

    // -- merge_sorted (k-way merge) ------------------------------------

    #[test]
    fn test_merge_sorted_basic() {
        let results = vec![vec![1, 5, 9], vec![2, 6, 10], vec![3, 7, 11]];
        let merged = ResultMerger::merge_sorted(results);
        assert_eq!(merged, vec![1, 2, 3, 5, 6, 7, 9, 10, 11]);
    }

    #[test]
    fn test_merge_sorted_empty_input() {
        let results: Vec<Vec<i32>> = vec![];
        let merged = ResultMerger::merge_sorted(results);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_merge_sorted_single_shard() {
        let results = vec![vec![10, 20, 30]];
        let merged = ResultMerger::merge_sorted(results);
        assert_eq!(merged, vec![10, 20, 30]);
    }

    #[test]
    fn test_merge_sorted_single_element_shards() {
        let results = vec![vec![5], vec![1], vec![3]];
        let merged = ResultMerger::merge_sorted(results);
        assert_eq!(merged, vec![1, 3, 5]);
    }

    #[test]
    fn test_merge_sorted_with_empty_shards() {
        let results = vec![vec![], vec![1, 3, 5], vec![], vec![2, 4], vec![]];
        let merged = ResultMerger::merge_sorted(results);
        assert_eq!(merged, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_merge_sorted_all_empty_shards() {
        let results: Vec<Vec<i32>> = vec![vec![], vec![], vec![]];
        let merged = ResultMerger::merge_sorted(results);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_merge_sorted_with_duplicates() {
        let results = vec![vec![1, 3, 5], vec![1, 3, 5], vec![2, 4, 6]];
        let merged = ResultMerger::merge_sorted(results);
        assert_eq!(merged, vec![1, 1, 2, 3, 3, 4, 5, 5, 6]);
    }

    #[test]
    fn test_merge_sorted_unequal_lengths() {
        let results = vec![vec![1], vec![2, 4, 6, 8, 10], vec![3, 5]];
        let merged = ResultMerger::merge_sorted(results);
        assert_eq!(merged, vec![1, 2, 3, 4, 5, 6, 8, 10]);
    }

    #[test]
    fn test_merge_sorted_negative_numbers() {
        let results = vec![vec![-10, -5, 0], vec![-8, -3, 2], vec![-20, 1]];
        let merged = ResultMerger::merge_sorted(results);
        assert_eq!(merged, vec![-20, -10, -8, -5, -3, 0, 1, 2]);
    }

    #[test]
    fn test_merge_sorted_strings() {
        let results = vec![
            vec!["apple".to_string(), "cherry".to_string()],
            vec!["banana".to_string(), "date".to_string()],
        ];
        let merged = ResultMerger::merge_sorted(results);
        assert_eq!(
            merged,
            vec![
                "apple".to_string(),
                "banana".to_string(),
                "cherry".to_string(),
                "date".to_string()
            ]
        );
    }

    #[test]
    fn test_merge_sorted_large_scale() {
        // 100 shards x 100 items each
        let num_shards = 100;
        let items_per_shard = 100;
        let mut results: Vec<Vec<i64>> = Vec::with_capacity(num_shards);

        for shard_idx in 0..num_shards {
            let shard: Vec<i64> = (0..items_per_shard)
                .map(|i| (shard_idx as i64) + (i as i64) * (num_shards as i64))
                .collect();
            results.push(shard);
        }

        let merged = ResultMerger::merge_sorted(results);

        // Verify length
        assert_eq!(merged.len(), num_shards * items_per_shard);

        // Verify sorted
        for window in merged.windows(2) {
            assert!(
                window[0] <= window[1],
                "Output not sorted: {} > {}",
                window[0],
                window[1]
            );
        }
    }

    #[test]
    fn test_merge_sorted_deterministic_tie_breaking() {
        // When values are equal, shard ordering should be deterministic
        let results = vec![vec![1, 2, 3], vec![1, 2, 3], vec![1, 2, 3]];
        let merged1 = ResultMerger::merge_sorted(results.clone());
        let merged2 = ResultMerger::merge_sorted(results);
        assert_eq!(merged1, merged2);
        assert_eq!(merged1, vec![1, 1, 1, 2, 2, 2, 3, 3, 3]);
    }

    // -- merge_sorted_by_key -------------------------------------------

    #[test]
    fn test_merge_sorted_by_key_basic() {
        // Simulate (key, value) tuples sorted by key
        let results = vec![
            vec![(1, "a"), (3, "c"), (5, "e")],
            vec![(2, "b"), (4, "d"), (6, "f")],
        ];
        let merged = ResultMerger::merge_sorted_by_key(results, |item| item.0);
        let keys: Vec<i32> = merged.iter().map(|item| item.0).collect();
        assert_eq!(keys, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_merge_sorted_by_key_empty() {
        let results: Vec<Vec<(i32, &str)>> = vec![];
        let merged = ResultMerger::merge_sorted_by_key(results, |item: &(i32, &str)| item.0);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_merge_sorted_by_key_with_string_keys() {
        let results = vec![
            vec![("apple", 10), ("cherry", 30)],
            vec![("banana", 20), ("date", 40)],
        ];
        let merged = ResultMerger::merge_sorted_by_key(results, |item| item.0);
        let keys: Vec<&str> = merged.iter().map(|item| item.0).collect();
        assert_eq!(keys, vec!["apple", "banana", "cherry", "date"]);
    }

    #[test]
    fn test_merge_sorted_by_key_reverse_field() {
        // Sort by a secondary field (the value, not the first element)
        let results = vec![
            vec![("x", 1), ("y", 3), ("z", 5)],
            vec![("a", 2), ("b", 4), ("c", 6)],
        ];
        let merged = ResultMerger::merge_sorted_by_key(results, |item| item.1);
        let values: Vec<i32> = merged.iter().map(|item| item.1).collect();
        assert_eq!(values, vec![1, 2, 3, 4, 5, 6]);
    }

    // -- merge_deduplicate ---------------------------------------------

    #[test]
    fn test_result_merger_deduplicate() {
        let results = vec![vec![1, 2, 3], vec![2, 3, 4], vec![3, 4, 5]];
        let mut merged = ResultMerger::merge_deduplicate(results);
        merged.sort();
        assert_eq!(merged, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_result_merger_deduplicate_empty() {
        let results: Vec<Vec<i32>> = vec![];
        let merged = ResultMerger::merge_deduplicate(results);
        assert!(merged.is_empty());
    }

    // -- merge_sorted_deduplicate --------------------------------------

    #[test]
    fn test_merge_sorted_deduplicate_basic() {
        let results = vec![vec![1, 3, 5], vec![1, 3, 5], vec![2, 4, 6]];
        let merged = ResultMerger::merge_sorted_deduplicate(results);
        assert_eq!(merged, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_merge_sorted_deduplicate_no_dups() {
        let results = vec![vec![1, 4, 7], vec![2, 5, 8], vec![3, 6, 9]];
        let merged = ResultMerger::merge_sorted_deduplicate(results);
        assert_eq!(merged, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn test_merge_sorted_deduplicate_all_same() {
        let results = vec![vec![1, 1, 1], vec![1, 1], vec![1]];
        let merged = ResultMerger::merge_sorted_deduplicate(results);
        assert_eq!(merged, vec![1]);
    }

    #[test]
    fn test_merge_sorted_deduplicate_empty() {
        let results: Vec<Vec<i32>> = vec![];
        let merged = ResultMerger::merge_sorted_deduplicate(results);
        assert!(merged.is_empty());
    }

    // -- property-style randomized test --------------------------------

    #[test]
    fn test_merge_sorted_random_property() {
        // Generate pseudo-random sorted vectors and verify the merge is sorted.
        // Uses a simple LCG to avoid depending on `rand`.
        let num_shards = 20;
        let max_items = 50;
        let mut seed: u64 = 0xDEAD_BEEF_CAFE;

        let mut results: Vec<Vec<i64>> = Vec::with_capacity(num_shards);
        for _ in 0..num_shards {
            // Determine shard length
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = (seed % (max_items as u64 + 1)) as usize;

            let mut shard = Vec::with_capacity(len);
            for _ in 0..len {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                shard.push((seed >> 33) as i64); // use upper bits for quality
            }
            shard.sort();
            results.push(shard);
        }

        let expected_len: usize = results.iter().map(|v| v.len()).sum();
        let merged = ResultMerger::merge_sorted(results);
        assert_eq!(merged.len(), expected_len);

        // Verify sorted
        for window in merged.windows(2) {
            assert!(
                window[0] <= window[1],
                "Property violation: {} > {}",
                window[0],
                window[1]
            );
        }
    }

    #[test]
    fn test_hash_ring_maintained() {
        let mut ring = HashRing::new(10);

        // Add two shards and verify keys route to one of them.
        ring.add_shard(1u64);
        ring.add_shard(2u64);

        let key = Key::from_str("hello");
        let shard = ring.get_shard_for_key(&key);
        assert!(shard.is_some(), "key must route to some shard");
        let shard_id = shard.expect("key must route to some shard");
        assert!(shard_id == 1 || shard_id == 2);

        // Remove shard 1 — all keys must now route to shard 2.
        ring.remove_shard(1u64);
        let shard_after = ring.get_shard_for_key(&key);
        assert_eq!(shard_after, Some(2u64));
    }

    #[test]
    fn test_range_partitioner_routing() {
        let mut rp = RangePartitioner::new();
        rp.add_range(b"a".to_vec(), 1u64);
        rp.add_range(b"m".to_vec(), 2u64);
        rp.add_range(b"z".to_vec(), 3u64);

        // "apple" >= "a" and < "m" → shard 1
        let k_apple = Key::from_str("apple");
        assert_eq!(rp.get_shard_for_key(&k_apple), Some(1u64));

        // "moon" >= "m" and < "z" → shard 2
        let k_moon = Key::from_str("moon");
        assert_eq!(rp.get_shard_for_key(&k_moon), Some(2u64));

        // "zebra" >= "z" → shard 3
        let k_zebra = Key::from_str("zebra");
        assert_eq!(rp.get_shard_for_key(&k_zebra), Some(3u64));

        // Key before any range → None
        let k_zero = Key::from_slice(&[0u8]);
        assert_eq!(rp.get_shard_for_key(&k_zero), None);
    }

    #[test]
    fn test_partitioner_consistent_hash_uses_ring() {
        // Build a registry with two shards.
        let registry = Arc::new(ShardRegistry::new());
        let range1 = KeyRange::new(Key::from_str("a"), Key::from_str("m")).expect("valid range");
        let range2 = KeyRange::new(Key::from_str("m"), Key::from_str("z")).expect("valid range");
        let shard1 = ShardMetadata::new(1, range1, 100);
        let shard2 = ShardMetadata::new(2, range2, 101);
        registry.register(shard1).expect("register shard 1");
        registry.register(shard2).expect("register shard 2");

        let partitioner = Partitioner::new(registry, PartitionStrategy::ConsistentHash);

        // Routing must succeed for a key in the hash space.
        let key = Key::from_str("hello");
        let result = partitioner.route_key(&key);
        assert!(
            result.is_ok(),
            "consistent hash routing must succeed: {:?}",
            result
        );
    }
}
