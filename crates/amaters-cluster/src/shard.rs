//! Shard metadata and operations
//!
//! This module defines the core types for distributed sharding in AmateRS.
//! It handles shard metadata, split/merge operations, and data migration.

use crate::error::{RaftError, RaftResult};
use crate::types::NodeId;
use amaters_core::Key;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Unique identifier for a shard
pub type ShardId = u64;

/// Shard state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardState {
    /// Shard is active and serving requests
    Active,
    /// Shard is being split into two shards
    Splitting,
    /// Shard is being merged with another shard
    Merging,
    /// Shard is being transferred to another node
    Transferring,
    /// Shard is offline (node failure or maintenance)
    Offline,
}

impl ShardState {
    /// Check if the shard can serve read requests
    pub fn can_read(&self) -> bool {
        matches!(self, ShardState::Active | ShardState::Splitting | ShardState::Transferring)
    }

    /// Check if the shard can serve write requests
    pub fn can_write(&self) -> bool {
        matches!(self, ShardState::Active)
    }

    /// Get the state name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            ShardState::Active => "Active",
            ShardState::Splitting => "Splitting",
            ShardState::Merging => "Merging",
            ShardState::Transferring => "Transferring",
            ShardState::Offline => "Offline",
        }
    }
}

/// Key range for a shard
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRange {
    /// Start key (inclusive)
    pub start: Key,
    /// End key (exclusive)
    pub end: Key,
}

impl KeyRange {
    /// Create a new key range
    pub fn new(start: Key, end: Key) -> RaftResult<Self> {
        if start >= end {
            return Err(RaftError::ConfigError {
                message: format!("Invalid key range: start {:?} >= end {:?}", start, end),
            });
        }
        Ok(Self { start, end })
    }

    /// Check if a key is within this range
    pub fn contains(&self, key: &Key) -> bool {
        key >= &self.start && key < &self.end
    }

    /// Check if this range overlaps with another range
    pub fn overlaps(&self, other: &KeyRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Calculate the midpoint key for splitting
    pub fn midpoint(&self) -> Key {
        // Simple midpoint calculation based on byte comparison
        let start_bytes = self.start.as_bytes();
        let end_bytes = self.end.as_bytes();

        let min_len = start_bytes.len().min(end_bytes.len());
        let mut mid_bytes = Vec::with_capacity(min_len);

        let mut carry = false;
        for i in 0..min_len {
            let avg = (start_bytes[i] as u16 + end_bytes[i] as u16 + carry as u16) / 2;
            mid_bytes.push(avg as u8);
            carry = (start_bytes[i] as u16 + end_bytes[i] as u16 + carry as u16) % 2 == 1;
        }

        // Ensure midpoint is different from start
        if mid_bytes == start_bytes[..min_len] {
            if let Some(last) = mid_bytes.last_mut() {
                *last = last.saturating_add(1);
            }
        }

        Key::from_slice(&mid_bytes)
    }

    /// Create a range that covers all possible keys
    pub fn full() -> Self {
        Self {
            start: Key::from_slice(&[0u8]),
            end: Key::from_slice(&[0xFFu8; 32]),
        }
    }
}

/// Shard metadata tracking
#[derive(Debug, Clone)]
pub struct ShardMetadata {
    /// Unique shard identifier
    pub id: ShardId,
    /// Key range this shard is responsible for
    pub range: KeyRange,
    /// Current state of the shard
    pub state: ShardState,
    /// Node ID where this shard is located
    pub node_id: NodeId,
    /// Replica node IDs (for fault tolerance)
    pub replicas: Vec<NodeId>,
    /// Estimated number of keys in this shard
    pub estimated_keys: u64,
    /// Estimated size in bytes
    pub estimated_size_bytes: u64,
    /// Last update timestamp
    pub last_updated: SystemTime,
    /// Creation timestamp
    pub created_at: SystemTime,
    /// Version number for optimistic concurrency control
    pub version: u64,
}

impl ShardMetadata {
    /// Create new shard metadata
    pub fn new(id: ShardId, range: KeyRange, node_id: NodeId) -> Self {
        let now = SystemTime::now();
        Self {
            id,
            range,
            state: ShardState::Active,
            node_id,
            replicas: Vec::new(),
            estimated_keys: 0,
            estimated_size_bytes: 0,
            last_updated: now,
            created_at: now,
            version: 1,
        }
    }

    /// Update shard state
    pub fn set_state(&mut self, state: ShardState) {
        self.state = state;
        self.last_updated = SystemTime::now();
        self.version += 1;
    }

    /// Update shard statistics
    pub fn update_stats(&mut self, estimated_keys: u64, estimated_size_bytes: u64) {
        self.estimated_keys = estimated_keys;
        self.estimated_size_bytes = estimated_size_bytes;
        self.last_updated = SystemTime::now();
        self.version += 1;
    }

    /// Add a replica
    pub fn add_replica(&mut self, node_id: NodeId) -> RaftResult<()> {
        if self.replicas.contains(&node_id) {
            return Err(RaftError::ConfigError {
                message: format!("Replica {} already exists for shard {}", node_id, self.id),
            });
        }
        self.replicas.push(node_id);
        self.last_updated = SystemTime::now();
        self.version += 1;
        Ok(())
    }

    /// Remove a replica
    pub fn remove_replica(&mut self, node_id: NodeId) -> RaftResult<()> {
        let initial_len = self.replicas.len();
        self.replicas.retain(|&id| id != node_id);
        if self.replicas.len() == initial_len {
            return Err(RaftError::ConfigError {
                message: format!("Replica {} not found for shard {}", node_id, self.id),
            });
        }
        self.last_updated = SystemTime::now();
        self.version += 1;
        Ok(())
    }

    /// Check if this shard is hot (exceeds threshold)
    pub fn is_hot(&self, key_threshold: u64, size_threshold: u64) -> bool {
        self.estimated_keys > key_threshold || self.estimated_size_bytes > size_threshold
    }

    /// Check if this shard is cold (below threshold)
    pub fn is_cold(&self, key_threshold: u64, size_threshold: u64) -> bool {
        self.estimated_keys < key_threshold && self.estimated_size_bytes < size_threshold
    }

    /// Check if the shard metadata is stale
    pub fn is_stale(&self, max_age: Duration) -> bool {
        self.last_updated
            .elapsed()
            .map(|elapsed| elapsed > max_age)
            .unwrap_or(false)
    }
}

/// Shard split operation descriptor
#[derive(Debug, Clone)]
pub struct ShardSplit {
    /// Source shard ID
    pub source_shard_id: ShardId,
    /// First new shard ID (left range)
    pub left_shard_id: ShardId,
    /// Second new shard ID (right range)
    pub right_shard_id: ShardId,
    /// Split point key
    pub split_key: Key,
    /// Timestamp when split was initiated
    pub initiated_at: SystemTime,
}

impl ShardSplit {
    /// Create a new shard split descriptor
    pub fn new(
        source_shard_id: ShardId,
        left_shard_id: ShardId,
        right_shard_id: ShardId,
        split_key: Key,
    ) -> Self {
        Self {
            source_shard_id,
            left_shard_id,
            right_shard_id,
            split_key,
            initiated_at: SystemTime::now(),
        }
    }

    /// Create left and right shard metadata from source
    pub fn create_shards(
        &self,
        source: &ShardMetadata,
    ) -> RaftResult<(ShardMetadata, ShardMetadata)> {
        // Create left shard (start to split_key)
        let left_range = KeyRange::new(source.range.start.clone(), self.split_key.clone())?;
        let mut left_shard = ShardMetadata::new(
            self.left_shard_id,
            left_range,
            source.node_id,
        );
        left_shard.replicas = source.replicas.clone();

        // Create right shard (split_key to end)
        let right_range = KeyRange::new(self.split_key.clone(), source.range.end.clone())?;
        let mut right_shard = ShardMetadata::new(
            self.right_shard_id,
            right_range,
            source.node_id,
        );
        right_shard.replicas = source.replicas.clone();

        // Estimate stats (simple equal split assumption)
        left_shard.estimated_keys = source.estimated_keys / 2;
        left_shard.estimated_size_bytes = source.estimated_size_bytes / 2;
        right_shard.estimated_keys = source.estimated_keys / 2;
        right_shard.estimated_size_bytes = source.estimated_size_bytes / 2;

        Ok((left_shard, right_shard))
    }
}

/// Shard merge operation descriptor
#[derive(Debug, Clone)]
pub struct ShardMerge {
    /// First source shard ID (should have lower key range)
    pub left_shard_id: ShardId,
    /// Second source shard ID (should have higher key range)
    pub right_shard_id: ShardId,
    /// Target merged shard ID
    pub target_shard_id: ShardId,
    /// Timestamp when merge was initiated
    pub initiated_at: SystemTime,
}

impl ShardMerge {
    /// Create a new shard merge descriptor
    pub fn new(
        left_shard_id: ShardId,
        right_shard_id: ShardId,
        target_shard_id: ShardId,
    ) -> Self {
        Self {
            left_shard_id,
            right_shard_id,
            target_shard_id,
            initiated_at: SystemTime::now(),
        }
    }

    /// Validate that two shards can be merged
    pub fn validate(&self, left: &ShardMetadata, right: &ShardMetadata) -> RaftResult<()> {
        // Check that key ranges are adjacent
        if left.range.end != right.range.start {
            return Err(RaftError::ConfigError {
                message: format!(
                    "Shards {} and {} are not adjacent (left.end={:?}, right.start={:?})",
                    left.id, right.id, left.range.end, right.range.start
                ),
            });
        }

        // Check that shards are on the same node
        if left.node_id != right.node_id {
            return Err(RaftError::ConfigError {
                message: format!(
                    "Shards {} and {} are on different nodes ({} vs {})",
                    left.id, right.id, left.node_id, right.node_id
                ),
            });
        }

        Ok(())
    }

    /// Create merged shard metadata
    pub fn create_merged_shard(
        &self,
        left: &ShardMetadata,
        right: &ShardMetadata,
    ) -> RaftResult<ShardMetadata> {
        self.validate(left, right)?;

        let merged_range = KeyRange::new(
            left.range.start.clone(),
            right.range.end.clone(),
        )?;

        let mut merged = ShardMetadata::new(
            self.target_shard_id,
            merged_range,
            left.node_id,
        );

        // Combine statistics
        merged.estimated_keys = left.estimated_keys + right.estimated_keys;
        merged.estimated_size_bytes = left.estimated_size_bytes + right.estimated_size_bytes;

        // Use replicas from left shard (should be same as right)
        merged.replicas = left.replicas.clone();

        Ok(merged)
    }
}

/// Shard transfer operation descriptor
#[derive(Debug, Clone)]
pub struct ShardTransfer {
    /// Shard ID being transferred
    pub shard_id: ShardId,
    /// Source node ID
    pub from_node: NodeId,
    /// Destination node ID
    pub to_node: NodeId,
    /// Transfer progress (0.0 to 1.0)
    pub progress: f64,
    /// Timestamp when transfer was initiated
    pub initiated_at: SystemTime,
    /// Estimated completion time
    pub estimated_completion: Option<SystemTime>,
}

impl ShardTransfer {
    /// Create a new shard transfer descriptor
    pub fn new(shard_id: ShardId, from_node: NodeId, to_node: NodeId) -> Self {
        Self {
            shard_id,
            from_node,
            to_node,
            progress: 0.0,
            initiated_at: SystemTime::now(),
            estimated_completion: None,
        }
    }

    /// Update transfer progress
    pub fn update_progress(&mut self, progress: f64) {
        self.progress = progress.clamp(0.0, 1.0);

        // Estimate completion time based on progress
        if progress > 0.0 && progress < 1.0 {
            if let Ok(elapsed) = self.initiated_at.elapsed() {
                let total_time = elapsed.as_secs_f64() / progress;
                let remaining_time = total_time * (1.0 - progress);
                self.estimated_completion = Some(
                    SystemTime::now() + Duration::from_secs_f64(remaining_time)
                );
            }
        }
    }

    /// Check if transfer is complete
    pub fn is_complete(&self) -> bool {
        self.progress >= 1.0
    }
}

/// Shard registry for tracking all shards in the cluster
#[derive(Debug, Clone)]
pub struct ShardRegistry {
    /// Map from shard ID to shard metadata
    shards: Arc<parking_lot::RwLock<BTreeMap<ShardId, ShardMetadata>>>,
    /// Next available shard ID
    next_shard_id: Arc<parking_lot::Mutex<ShardId>>,
}

impl ShardRegistry {
    /// Create a new shard registry
    pub fn new() -> Self {
        Self {
            shards: Arc::new(parking_lot::RwLock::new(BTreeMap::new())),
            next_shard_id: Arc::new(parking_lot::Mutex::new(1)),
        }
    }

    /// Allocate a new shard ID
    pub fn allocate_shard_id(&self) -> ShardId {
        let mut next_id = self.next_shard_id.lock();
        let id = *next_id;
        *next_id += 1;
        id
    }

    /// Register a new shard
    pub fn register(&self, shard: ShardMetadata) -> RaftResult<()> {
        let mut shards = self.shards.write();

        // Check for overlapping ranges
        for existing in shards.values() {
            if existing.range.overlaps(&shard.range) {
                return Err(RaftError::ConfigError {
                    message: format!(
                        "Shard {} range overlaps with existing shard {} range",
                        shard.id, existing.id
                    ),
                });
            }
        }

        shards.insert(shard.id, shard);
        Ok(())
    }

    /// Get shard metadata by ID
    pub fn get(&self, shard_id: ShardId) -> Option<ShardMetadata> {
        self.shards.read().get(&shard_id).cloned()
    }

    /// Update shard metadata
    pub fn update(&self, shard: ShardMetadata) -> RaftResult<()> {
        let mut shards = self.shards.write();
        shards.insert(shard.id, shard);
        Ok(())
    }

    /// Remove a shard
    pub fn remove(&self, shard_id: ShardId) -> RaftResult<()> {
        let mut shards = self.shards.write();
        shards.remove(&shard_id).ok_or_else(|| RaftError::ConfigError {
            message: format!("Shard {} not found", shard_id),
        })?;
        Ok(())
    }

    /// Get all shards
    pub fn get_all(&self) -> Vec<ShardMetadata> {
        self.shards.read().values().cloned().collect()
    }

    /// Get shards on a specific node
    pub fn get_by_node(&self, node_id: NodeId) -> Vec<ShardMetadata> {
        self.shards
            .read()
            .values()
            .filter(|shard| shard.node_id == node_id)
            .cloned()
            .collect()
    }

    /// Find shard responsible for a key
    pub fn find_shard_for_key(&self, key: &Key) -> Option<ShardMetadata> {
        self.shards
            .read()
            .values()
            .find(|shard| shard.range.contains(key))
            .cloned()
    }

    /// Get total number of shards
    pub fn count(&self) -> usize {
        self.shards.read().len()
    }
}

impl Default for ShardRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_state() {
        assert!(ShardState::Active.can_read());
        assert!(ShardState::Active.can_write());
        assert!(ShardState::Splitting.can_read());
        assert!(!ShardState::Splitting.can_write());
        assert!(!ShardState::Offline.can_read());
        assert!(!ShardState::Offline.can_write());
    }

    #[test]
    fn test_key_range_contains() -> RaftResult<()> {
        let range = KeyRange::new(
            Key::from_str("a"),
            Key::from_str("z"),
        )?;

        assert!(range.contains(&Key::from_str("m")));
        assert!(range.contains(&Key::from_str("a")));
        assert!(!range.contains(&Key::from_str("z")));
        assert!(!range.contains(&Key::from_str("aa")));

        Ok(())
    }

    #[test]
    fn test_key_range_overlaps() -> RaftResult<()> {
        let range1 = KeyRange::new(Key::from_str("a"), Key::from_str("m"))?;
        let range2 = KeyRange::new(Key::from_str("g"), Key::from_str("z"))?;
        let range3 = KeyRange::new(Key::from_str("m"), Key::from_str("z"))?;

        assert!(range1.overlaps(&range2));
        assert!(range2.overlaps(&range1));
        assert!(!range1.overlaps(&range3));

        Ok(())
    }

    #[test]
    fn test_key_range_midpoint() -> RaftResult<()> {
        let range = KeyRange::new(
            Key::from_str("a"),
            Key::from_str("z"),
        )?;

        let mid = range.midpoint();
        assert!(mid > range.start);
        assert!(mid < range.end);

        Ok(())
    }

    #[test]
    fn test_shard_metadata_creation() {
        let range = KeyRange::new(Key::from_str("a"), Key::from_str("z"))
            .expect("valid range");
        let shard = ShardMetadata::new(1, range, 100);

        assert_eq!(shard.id, 1);
        assert_eq!(shard.node_id, 100);
        assert_eq!(shard.state, ShardState::Active);
        assert_eq!(shard.version, 1);
    }

    #[test]
    fn test_shard_metadata_update_stats() {
        let range = KeyRange::new(Key::from_str("a"), Key::from_str("z"))
            .expect("valid range");
        let mut shard = ShardMetadata::new(1, range, 100);

        let initial_version = shard.version;
        shard.update_stats(1000, 50000);

        assert_eq!(shard.estimated_keys, 1000);
        assert_eq!(shard.estimated_size_bytes, 50000);
        assert_eq!(shard.version, initial_version + 1);
    }

    #[test]
    fn test_shard_metadata_replicas() -> RaftResult<()> {
        let range = KeyRange::new(Key::from_str("a"), Key::from_str("z"))?;
        let mut shard = ShardMetadata::new(1, range, 100);

        shard.add_replica(101)?;
        shard.add_replica(102)?;
        assert_eq!(shard.replicas.len(), 2);

        assert!(shard.add_replica(101).is_err());

        shard.remove_replica(101)?;
        assert_eq!(shard.replicas.len(), 1);
        assert!(shard.replicas.contains(&102));

        Ok(())
    }

    #[test]
    fn test_shard_split() -> RaftResult<()> {
        let range = KeyRange::new(Key::from_str("a"), Key::from_str("z"))?;
        let mut source = ShardMetadata::new(1, range, 100);
        source.update_stats(1000, 100000);

        let split = ShardSplit::new(1, 2, 3, Key::from_str("m"));
        let (left, right) = split.create_shards(&source)?;

        assert_eq!(left.id, 2);
        assert_eq!(right.id, 3);
        assert_eq!(left.range.end, Key::from_str("m"));
        assert_eq!(right.range.start, Key::from_str("m"));
        assert_eq!(left.estimated_keys, 500);
        assert_eq!(right.estimated_keys, 500);

        Ok(())
    }

    #[test]
    fn test_shard_merge() -> RaftResult<()> {
        let left_range = KeyRange::new(Key::from_str("a"), Key::from_str("m"))?;
        let right_range = KeyRange::new(Key::from_str("m"), Key::from_str("z"))?;

        let mut left = ShardMetadata::new(1, left_range, 100);
        let mut right = ShardMetadata::new(2, right_range, 100);

        left.update_stats(500, 50000);
        right.update_stats(500, 50000);

        let merge = ShardMerge::new(1, 2, 3);
        let merged = merge.create_merged_shard(&left, &right)?;

        assert_eq!(merged.id, 3);
        assert_eq!(merged.range.start, Key::from_str("a"));
        assert_eq!(merged.range.end, Key::from_str("z"));
        assert_eq!(merged.estimated_keys, 1000);
        assert_eq!(merged.estimated_size_bytes, 100000);

        Ok(())
    }

    #[test]
    fn test_shard_transfer() {
        let mut transfer = ShardTransfer::new(1, 100, 101);
        assert_eq!(transfer.progress, 0.0);
        assert!(!transfer.is_complete());

        transfer.update_progress(0.5);
        assert_eq!(transfer.progress, 0.5);
        assert!(!transfer.is_complete());

        transfer.update_progress(1.0);
        assert!(transfer.is_complete());
    }

    #[test]
    fn test_shard_registry() -> RaftResult<()> {
        let registry = ShardRegistry::new();

        let id1 = registry.allocate_shard_id();
        let id2 = registry.allocate_shard_id();
        assert_ne!(id1, id2);

        let range1 = KeyRange::new(Key::from_str("a"), Key::from_str("m"))?;
        let shard1 = ShardMetadata::new(id1, range1, 100);
        registry.register(shard1.clone())?;

        let retrieved = registry.get(id1);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.expect("Shard should be retrieved from registry").id, id1);

        let found = registry.find_shard_for_key(&Key::from_str("g"));
        assert!(found.is_some());
        assert_eq!(found.expect("Shard should be found for key").id, id1);

        assert_eq!(registry.count(), 1);

        Ok(())
    }

    #[test]
    fn test_shard_registry_overlapping_ranges() -> RaftResult<()> {
        let registry = ShardRegistry::new();

        let range1 = KeyRange::new(Key::from_str("a"), Key::from_str("m"))?;
        let shard1 = ShardMetadata::new(1, range1, 100);
        registry.register(shard1)?;

        let range2 = KeyRange::new(Key::from_str("g"), Key::from_str("z"))?;
        let shard2 = ShardMetadata::new(2, range2, 100);
        let result = registry.register(shard2);

        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_hot_cold_shards() {
        let range = KeyRange::new(Key::from_str("a"), Key::from_str("z"))
            .expect("valid range");
        let mut shard = ShardMetadata::new(1, range, 100);

        shard.update_stats(1000, 50000);
        assert!(shard.is_hot(500, 25000));
        assert!(!shard.is_cold(500, 25000));

        shard.update_stats(100, 5000);
        assert!(!shard.is_hot(500, 25000));
        assert!(shard.is_cold(500, 25000));
    }
}
