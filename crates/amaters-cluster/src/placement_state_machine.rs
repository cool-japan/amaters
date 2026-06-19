//! State machine for the placement layer.
//!
//! [`PlacementStateMachine`] applies committed [`ClusterCommand`] log entries
//! that affect shard placement (split / merge / transfer) to the shared
//! [`ShardRegistry`].

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use amaters_core::Key;

use crate::{
    cluster_command::ClusterCommand,
    error::{RaftError, RaftResult},
    log::{LogEntry, StateMachine},
    shard::{
        KeyRange, ShardId, ShardMerge, ShardMetadata, ShardRegistry, ShardSplit, ShardState,
        ShardTransfer,
    },
    types::NodeId,
};

// ── Snapshot DTO ─────────────────────────────────────────────────────────────

/// Portable snapshot representation for [`ShardMetadata`].
///
/// [`ShardMetadata`] contains [`SystemTime`] fields which lack a stable serde
/// representation; this DTO converts them to milliseconds since the Unix epoch.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ShardMetadataDto {
    id: ShardId,
    range_start: Vec<u8>,
    range_end: Vec<u8>,
    /// Encoded as a `u8` discriminant (0=Active, 1=Splitting, 2=Merging,
    /// 3=Transferring, 4=Offline) to keep the snapshot format self-contained.
    state: u8,
    node_id: NodeId,
    replicas: Vec<NodeId>,
    estimated_keys: u64,
    estimated_size_bytes: u64,
    last_updated_ms: u64,
    created_at_ms: u64,
    version: u64,
}

fn state_to_u8(s: &ShardState) -> u8 {
    match s {
        ShardState::Active => 0,
        ShardState::Splitting => 1,
        ShardState::Merging => 2,
        ShardState::Transferring => 3,
        ShardState::Offline => 4,
    }
}

fn u8_to_state(v: u8) -> RaftResult<ShardState> {
    match v {
        0 => Ok(ShardState::Active),
        1 => Ok(ShardState::Splitting),
        2 => Ok(ShardState::Merging),
        3 => Ok(ShardState::Transferring),
        4 => Ok(ShardState::Offline),
        other => Err(RaftError::StateMachineError {
            message: format!("unknown ShardState discriminant {}", other),
        }),
    }
}

fn system_time_to_ms(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

fn ms_to_system_time(ms: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(ms)
}

impl ShardMetadataDto {
    fn from_meta(m: &ShardMetadata) -> Self {
        Self {
            id: m.id,
            range_start: m.range.start.as_bytes().to_vec(),
            range_end: m.range.end.as_bytes().to_vec(),
            state: state_to_u8(&m.state),
            node_id: m.node_id,
            replicas: m.replicas.clone(),
            estimated_keys: m.estimated_keys,
            estimated_size_bytes: m.estimated_size_bytes,
            last_updated_ms: system_time_to_ms(m.last_updated),
            created_at_ms: system_time_to_ms(m.created_at),
            version: m.version,
        }
    }

    fn into_meta(self) -> RaftResult<ShardMetadata> {
        let start = Key::from_slice(&self.range_start);
        let end = Key::from_slice(&self.range_end);
        let range = KeyRange::new(start, end)?;
        let mut meta = ShardMetadata::new(self.id, range, self.node_id);
        meta.state = u8_to_state(self.state)?;
        meta.replicas = self.replicas;
        meta.estimated_keys = self.estimated_keys;
        meta.estimated_size_bytes = self.estimated_size_bytes;
        meta.last_updated = ms_to_system_time(self.last_updated_ms);
        meta.created_at = ms_to_system_time(self.created_at_ms);
        meta.version = self.version;
        Ok(meta)
    }
}

// ── PlacementStateMachine ─────────────────────────────────────────────────────

/// A [`StateMachine`] that applies placement-layer Raft log entries to the
/// cluster's [`ShardRegistry`].
///
/// Only [`ClusterCommand::PlaceSplit`], [`ClusterCommand::PlaceMerge`], and
/// [`ClusterCommand::PlaceTransfer`] entries mutate the registry; all other
/// command types are treated as no-ops so that the same log can carry data- and
/// membership-layer entries without needing separate partitioning.
pub struct PlacementStateMachine {
    registry: Arc<ShardRegistry>,
}

impl PlacementStateMachine {
    /// Construct a new state machine backed by the given registry.
    pub fn new(registry: Arc<ShardRegistry>) -> Self {
        Self { registry }
    }
}

impl StateMachine for PlacementStateMachine {
    fn apply(&mut self, entry: &LogEntry) -> RaftResult<Vec<u8>> {
        let cmd = match ClusterCommand::decode(&entry.command.data) {
            Ok(c) => c,
            Err(_) => {
                // Not a recognised ClusterCommand (e.g. raw membership markers
                // written as plain ASCII during bootstrap).  Treat as a no-op.
                return Ok(Vec::new());
            }
        };

        match cmd {
            ClusterCommand::PlaceSplit {
                shard_id,
                split_key,
            } => {
                let left_id = self.registry.allocate_shard_id();
                let right_id = self.registry.allocate_shard_id();
                let key = Key::from_slice(&split_key);
                let split = ShardSplit::new(shard_id, left_id, right_id, key);
                self.registry.execute_split(&split)?;
                Ok(Vec::new())
            }
            ClusterCommand::PlaceMerge {
                left_shard_id,
                right_shard_id,
            } => {
                let target_id = self.registry.allocate_shard_id();
                let merge = ShardMerge::new(left_shard_id, right_shard_id, target_id);
                self.registry.execute_merge(&merge)?;
                Ok(Vec::new())
            }
            ClusterCommand::PlaceTransfer {
                shard_id,
                from_node,
                to_node,
            } => {
                let transfer = ShardTransfer::new(shard_id, from_node, to_node);
                self.registry.execute_transfer(&transfer)?;
                Ok(Vec::new())
            }
            // Data-plane and membership changes are handled by other state
            // machines; silently ignore them here.
            _ => Ok(Vec::new()),
        }
    }

    fn snapshot(&self) -> RaftResult<Vec<u8>> {
        let shards = self.registry.get_all();
        let dtos: Vec<ShardMetadataDto> = shards.iter().map(ShardMetadataDto::from_meta).collect();
        oxicode::serde::encode_serde(&dtos).map_err(|e| RaftError::StateMachineError {
            message: format!(
                "PlacementStateMachine::snapshot: serialisation failed: {}",
                e
            ),
        })
    }

    fn restore(&mut self, snapshot: &[u8]) -> RaftResult<()> {
        let dtos: Vec<ShardMetadataDto> =
            oxicode::serde::decode_serde(snapshot).map_err(|e| RaftError::StateMachineError {
                message: format!(
                    "PlacementStateMachine::restore: deserialisation failed: {}",
                    e
                ),
            })?;

        // Remove all existing shards before inserting the snapshot state.
        for shard in self.registry.get_all() {
            // Ignore "not found" errors — the registry may already have been
            // partially cleared by a prior (failed) restore attempt.
            let _ = self.registry.remove(shard.id);
        }

        for dto in dtos {
            let meta = dto.into_meta()?;
            // Use `update` (upsert) rather than `register` so that range-overlap
            // checks do not fire for shards whose ranges abut one another.
            self.registry.update(meta)?;
        }

        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        log::{Command, LogEntry},
        shard::{KeyRange, ShardMetadata, ShardRegistry},
    };
    use amaters_core::Key;

    fn make_entry(data: Vec<u8>) -> LogEntry {
        LogEntry::new(1, 1, Command::new(data))
    }

    fn make_registry_with_shard(
        start: &str,
        end: &str,
        node_id: NodeId,
    ) -> (Arc<ShardRegistry>, ShardId) {
        let registry = Arc::new(ShardRegistry::new());
        let shard_id = registry.allocate_shard_id();
        let range = KeyRange::new(Key::from_str(start), Key::from_str(end)).expect("valid range");
        let shard = ShardMetadata::new(shard_id, range, node_id);
        registry.register(shard).expect("register");
        (registry, shard_id)
    }

    #[test]
    fn test_placement_state_machine_applies_split() {
        let (registry, shard_id) = make_registry_with_shard("a", "z", 1);
        let mut sm = PlacementStateMachine::new(Arc::clone(&registry));

        let cmd = ClusterCommand::PlaceSplit {
            shard_id,
            split_key: Key::from_str("m").as_bytes().to_vec(),
        };
        let entry = make_entry(cmd.encode());
        sm.apply(&entry).expect("apply split");

        assert!(registry.get(shard_id).is_none(), "parent should be removed");
        let mut all = registry.get_all();
        assert_eq!(all.len(), 2, "should have two children");

        for shard in &all {
            assert_eq!(shard.state, ShardState::Active);
        }

        all.sort_by(|a, b| a.range.start.cmp(&b.range.start));
        assert_eq!(all[0].range.start, Key::from_str("a"));
        assert_eq!(all[0].range.end, Key::from_str("m"));
        assert_eq!(all[1].range.start, Key::from_str("m"));
        assert_eq!(all[1].range.end, Key::from_str("z"));
    }

    #[test]
    fn test_placement_state_machine_applies_merge() {
        let registry = Arc::new(ShardRegistry::new());
        let left_id = registry.allocate_shard_id();
        let right_id = registry.allocate_shard_id();
        let left_range = KeyRange::new(Key::from_str("a"), Key::from_str("m")).expect("range");
        let right_range = KeyRange::new(Key::from_str("m"), Key::from_str("z")).expect("range");
        registry
            .register(ShardMetadata::new(left_id, left_range, 1))
            .expect("register left");
        registry
            .register(ShardMetadata::new(right_id, right_range, 1))
            .expect("register right");

        let mut sm = PlacementStateMachine::new(Arc::clone(&registry));

        let cmd = ClusterCommand::PlaceMerge {
            left_shard_id: left_id,
            right_shard_id: right_id,
        };
        let entry = make_entry(cmd.encode());
        sm.apply(&entry).expect("apply merge");

        assert!(registry.get(left_id).is_none(), "left should be removed");
        assert!(registry.get(right_id).is_none(), "right should be removed");
        let all = registry.get_all();
        assert_eq!(all.len(), 1, "should have one merged shard");
        assert_eq!(all[0].range.start, Key::from_str("a"));
        assert_eq!(all[0].range.end, Key::from_str("z"));
        assert_eq!(all[0].state, ShardState::Active);
    }

    #[test]
    fn test_placement_snapshot_round_trip() {
        let (registry, _) = make_registry_with_shard("a", "z", 42);
        let sm = PlacementStateMachine::new(Arc::clone(&registry));

        let snap = sm.snapshot().expect("snapshot");
        assert!(!snap.is_empty(), "snapshot must not be empty");

        let new_registry = Arc::new(ShardRegistry::new());
        let mut sm2 = PlacementStateMachine::new(Arc::clone(&new_registry));
        sm2.restore(&snap).expect("restore");

        let shards = new_registry.get_all();
        assert_eq!(shards.len(), 1, "restored registry should have one shard");
        assert_eq!(shards[0].range.start, Key::from_str("a"));
        assert_eq!(shards[0].range.end, Key::from_str("z"));
        assert_eq!(shards[0].node_id, 42);
    }

    #[test]
    fn test_apply_non_placement_command_is_noop() {
        let registry = Arc::new(ShardRegistry::new());
        let mut sm = PlacementStateMachine::new(Arc::clone(&registry));

        let cmd = ClusterCommand::MembershipAdd {
            node_id: 5,
            address: "127.0.0.1:7878".into(),
        };
        let entry = make_entry(cmd.encode());
        let result = sm.apply(&entry).expect("apply membership add");
        assert!(result.is_empty());
        assert_eq!(registry.count(), 0, "registry should be unchanged");
    }
}
