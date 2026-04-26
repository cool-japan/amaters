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

pub mod encryption;
pub mod error;
pub mod failover;
pub mod heartbeat;
pub mod log;
pub mod metrics;
pub mod node;
pub mod persistence;
pub mod rpc;
pub mod snapshot;
pub mod state;
pub mod types;
pub mod wal;
// Re-exports for convenience
pub use encryption::{EncryptedPayload, EntryEncryptor, LogEncryptionKey, LogIntegrityVerifier};
pub use error::{RaftError, RaftResult};
pub use failover::{FailoverConfig, FailoverCoordinator, FailoverEvent};
pub use heartbeat::FailureDetector;
pub use log::{ApplyResult, Command, LogEntry, RaftLog, SnapshotData, StateMachine};
pub use metrics::ClusterMetrics;
pub use node::RaftNode;
pub use persistence::{FilePersistence, MemoryPersistence, RaftPersistence};
pub use rpc::{
    AppendEntriesRequest, AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse,
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
