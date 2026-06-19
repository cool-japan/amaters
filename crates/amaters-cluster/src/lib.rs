//! Consensus layer for AmateRS (Ukehi - The Sacred Pledge)
//!
//! This crate implements Raft consensus with support for encrypted logs
//! and distributed cluster management.
//!
//! ## Architecture
//!
//! The consensus layer consists of:
//!
//! - **Raft Node**: Core consensus implementation with leader election and log replication
//! - **Log Management**: Persistent log with in-memory cache and compaction
//! - **State Management**: Persistent and volatile state tracking
//! - **RPC Layer**: Request/response messages for inter-node communication
//!
//! ## Example
//!
//! ```rust,ignore
//! use amaters_cluster::{RaftNode, RaftConfig, Command};
//!
//! // Create a 3-node cluster
//! let config = RaftConfig::new(1, vec![1, 2, 3]);
//! let node = RaftNode::new(config)?;
//!
//! // Propose a command (as leader)
//! let cmd = Command::from_str("SET key value");
//! let index = node.propose(cmd)?;
//! ```
//!
//! ## Consensus Flow
//!
//! ```text
//!  propose(cmd) → [Leader]
//!                     │ replicate_to_followers()
//!                     ├──→ [Follower 1] handle_append_entries()
//!                     └──→ [Follower 2] handle_append_entries()
//!                               ↓
//!                     handle_replication_response() (on leader)
//!                               ↓
//!                     commit_index advances → StateMachine::apply()
//! ```

pub mod alert_rules;
pub mod cluster_command;
pub mod cluster_topology;
pub mod config;
pub mod encryption;
pub mod error;
pub mod failover;
pub mod heartbeat;
pub mod key_rotation;
pub mod log;
pub mod merkle;
pub mod metrics;
pub mod migration;
pub mod node;
pub mod partitioner;
pub mod persistence;
pub mod placement;
pub mod placement_scheduler;
pub mod placement_state_machine;
pub mod rpc;
pub mod shard;
pub mod snapshot;
pub mod state;
pub mod types;
pub mod wal;
// Re-exports for convenience
pub use cluster_command::ClusterCommand;
pub use cluster_topology::{
    ClusterTopology, NodeState as ClusterNodeState, NodeStatus, TopologyCollector,
};
pub use encryption::{EncryptedPayload, EntryEncryptor, LogEncryptionKey, LogIntegrityVerifier};
pub use error::{RaftError, RaftResult};
pub use failover::{
    AlertCallback, AlertEvent, AlertManager, FailoverConfig, FailoverController,
    FailoverCoordinator, FailoverEvent,
};

pub use alert_rules::{
    AlertRule, AlertSeverity, AlertSink, CollectingSink, FiredAlert, LogSink, RuleEngine,
    default_rules, event_dedup_key,
};
pub use heartbeat::FailureDetector;
pub use key_rotation::{KeyManager, KeyVersion, LEGACY_KEY_VERSION};
pub use log::{ApplyResult, Command, LogEntry, RaftLog, SnapshotData, StateMachine};
pub use merkle::{MerkleProof, MerkleTree};
pub use metrics::ClusterMetrics;
pub use migration::{Migration, MigrationStatus, MigrationTracker, compute_rebalance_plan};
pub use node::RaftNode;
pub use partitioner::{
    PartitionStrategy, Partitioner, QueryPlan, QueryRouter, QueryStats, ResultMerger,
};
pub use persistence::{FilePersistence, MemoryPersistence, RaftPersistence};
pub use placement::{PlacementAction, PlacementCoordinator, PlacementPlan, PlacementPolicy};
pub use placement_scheduler::{
    PlacementScheduler, PlacementSchedulerConfig, PlacementSchedulerHandle,
};
pub use placement_state_machine::PlacementStateMachine;
pub use rpc::{
    AppendEntriesRequest, AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse,
};
pub use shard::{
    KeyRange, ShardId, ShardMerge, ShardMetadata, ShardRegistry, ShardSplit, ShardState,
    ShardTransfer,
};
pub use snapshot::{
    DiskSnapshotStore, InstallSnapshotRequest, InstallSnapshotResponse, Snapshot, SnapshotConfig,
    SnapshotManager, SnapshotMetadata, SnapshotPolicy, SnapshotReceiver, SnapshotStore,
};
pub use state::{CandidateState, FencingTokenState, LeaderState, PersistentState, VolatileState};
pub use types::{
    ClusterConfig, ConfigState, FailureEvent, FencingToken, HeartbeatConfig, LogIndex,
    MembershipChange, NodeId, NodeState, RaftConfig, Term,
};
pub use wal::{CorruptionPolicy, SyncMode, WalDiagnostics, WalReader, WalWriter};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Library name
pub const NAME: &str = env!("CARGO_PKG_NAME");
