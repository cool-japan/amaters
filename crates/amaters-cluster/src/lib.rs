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

pub mod error;
pub mod log;
pub mod node;
pub mod rpc;
pub mod state;
pub mod types;

// Re-exports for convenience
pub use error::{RaftError, RaftResult};
pub use log::{Command, LogEntry, RaftLog};
pub use node::RaftNode;
pub use rpc::{
    AppendEntriesRequest, AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse,
};
pub use state::{CandidateState, LeaderState, PersistentState, VolatileState};
pub use types::{LogIndex, NodeId, NodeState, RaftConfig, Term};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Library name
pub const NAME: &str = env!("CARGO_PKG_NAME");
