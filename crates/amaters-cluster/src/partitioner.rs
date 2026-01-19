//! Key range partitioning and query routing
//!
//! This module provides partitioning strategies for distributing keys across shards
//! and routing queries to the correct shard(s).

use crate::error::{RaftError, RaftResult};
use crate::shard::{KeyRange, ShardId, ShardMetadata, ShardRegistry};
use crate::types::NodeId;
use amaters_core::Key;
use std::collections::{HashMap, HashSet};
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

/// Partitioner handles key-to-shard routing
#[derive(Clone)]
pub struct Partitioner {
    /// Shard registry
    registry: Arc<ShardRegistry>,
    /// Partitioning strategy
    strategy: PartitionStrategy,
    /// Number of virtual nodes for consistent hashing
    virtual_nodes: usize,
}

impl Partitioner {
    /// Create a new partitioner
    pub fn new(registry: Arc<ShardRegistry>, strategy: PartitionStrategy) -> Self {
        Self {
            registry,
            strategy,
            virtual_nodes: 100, // Default number of virtual nodes
        }
    }

    /// Set the number of virtual nodes for consistent hashing
    pub fn with_virtual_nodes(mut self, count: usize) -> Self {
        self.virtual_nodes = count;
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

    /// Route by consistent hashing
    fn route_by_consistent_hash(&self, key: &Key) -> RaftResult<ShardMetadata> {
        let shards = self.registry.get_all();
        if shards.is_empty() {
            return Err(RaftError::ConfigError {
                message: "No shards available".to_string(),
            });
        }

        // Build hash ring
        let mut ring: Vec<(u64, ShardId)> = Vec::new();
        for shard in &shards {
            for i in 0..self.virtual_nodes {
                let virtual_key = format!("{}:{}", shard.id, i);
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                virtual_key.hash(&mut hasher);
                let hash = hasher.finish();
                ring.push((hash, shard.id));
            }
        }
        ring.sort_by_key(|&(hash, _)| hash);

        // Find the shard for this key
        let key_hash = hash_key(key);
        let shard_id = ring
            .iter()
            .find(|&&(hash, _)| hash >= key_hash)
            .map(|&(_, id)| id)
            .unwrap_or_else(|| ring[0].1);

        self.registry.get(shard_id).ok_or_else(|| RaftError::ConfigError {
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
            targets
                .entry(shard.node_id)
                .or_insert_with(Vec::new)
                .push(shard.id);
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
            targets
                .entry(shard.node_id)
                .or_insert_with(Vec::new)
                .push(shard.id);
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
            QueryPlan::Scatter { targets, .. } => {
                targets.values().flatten().copied().collect()
            }
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

/// Result merger for scatter-gather queries
pub struct ResultMerger;

impl ResultMerger {
    /// Merge results from multiple shards (placeholder for future implementation)
    pub fn merge<T>(_results: Vec<Vec<T>>) -> Vec<T>
    where
        T: Clone,
    {
        // TODO: Implement proper merging logic based on query type
        // For now, just flatten all results
        _results.into_iter().flatten().collect()
    }

    /// Merge and sort results by key
    pub fn merge_sorted<T>(_results: Vec<Vec<T>>) -> Vec<T>
    where
        T: Clone + Ord,
    {
        // TODO: Implement efficient k-way merge for sorted results
        let mut merged: Vec<T> = _results.into_iter().flatten().collect();
        merged.sort();
        merged
    }

    /// Merge with deduplication
    pub fn merge_deduplicate<T>(_results: Vec<Vec<T>>) -> Vec<T>
    where
        T: Clone + Eq + Hash,
    {
        let set: HashSet<T> = _results.into_iter().flatten().collect();
        set.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_registry() -> Arc<ShardRegistry> {
        let registry = Arc::new(ShardRegistry::new());

        // Create 3 shards with non-overlapping ranges
        let range1 = KeyRange::new(Key::from_str("a"), Key::from_str("h"))
            .expect("valid range");
        let shard1 = ShardMetadata::new(1, range1, 100);
        registry.register(shard1).expect("register shard 1");

        let range2 = KeyRange::new(Key::from_str("h"), Key::from_str("p"))
            .expect("valid range");
        let shard2 = ShardMetadata::new(2, range2, 101);
        registry.register(shard2).expect("register shard 2");

        let range3 = KeyRange::new(Key::from_str("p"), Key::from_str("z"))
            .expect("valid range");
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
        let partitioner = Partitioner::new(registry, PartitionStrategy::ConsistentHash)
            .with_virtual_nodes(50);

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
            QueryPlan::Scatter { targets, merge_required } => {
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

    #[test]
    fn test_result_merger() {
        let results = vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 9]];
        let merged = ResultMerger::merge(results);
        assert_eq!(merged.len(), 9);
    }

    #[test]
    fn test_result_merger_sorted() {
        let results = vec![vec![1, 5, 9], vec![2, 6, 10], vec![3, 7, 11]];
        let merged = ResultMerger::merge_sorted(results);
        assert_eq!(merged, vec![1, 2, 3, 5, 6, 7, 9, 10, 11]);
    }

    #[test]
    fn test_result_merger_deduplicate() {
        let results = vec![vec![1, 2, 3], vec![2, 3, 4], vec![3, 4, 5]];
        let merged = ResultMerger::merge_deduplicate(results);
        assert_eq!(merged.len(), 5); // 1, 2, 3, 4, 5
    }
}
