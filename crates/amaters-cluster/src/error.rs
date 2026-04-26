//! Error types for the Raft consensus module

use std::fmt;

/// Result type for Raft operations
pub type RaftResult<T> = Result<T, RaftError>;

/// Errors that can occur during Raft consensus operations
#[derive(Debug, Clone, PartialEq)]
pub enum RaftError {
    /// Node is not the leader
    NotLeader {
        /// Current leader ID if known
        leader_id: Option<u64>,
    },
    /// Invalid node state for this operation
    InvalidState {
        /// Expected state
        expected: String,
        /// Actual state
        actual: String,
    },
    /// Log inconsistency detected
    LogInconsistency {
        /// Description of the inconsistency
        reason: String,
    },
    /// Storage operation failed
    StorageError {
        /// Error message
        message: String,
    },
    /// Term is stale
    StaleTerm {
        /// Current term
        current: u64,
        /// Received term
        received: u64,
    },
    /// Vote already granted to another candidate
    VoteAlreadyGranted {
        /// Node that received the vote
        voted_for: u64,
    },
    /// Configuration error
    ConfigError {
        /// Error message
        message: String,
    },
    /// Network error
    NetworkError {
        /// Error message
        message: String,
    },
    /// Timeout occurred
    Timeout {
        /// Timeout description
        description: String,
    },
    /// A membership change is already in progress (joint consensus active)
    MembershipChangeInProgress,
    /// The target node is already a member of the cluster
    NodeAlreadyMember {
        /// The node ID that is already a member
        node_id: u64,
    },
    /// The target node is not a member of the cluster
    NodeNotMember {
        /// The node ID that was not found
        node_id: u64,
    },
    /// State machine application error
    StateMachineError {
        /// Error message
        message: String,
    },
    /// Node is currently replaying its WAL on startup and cannot serve requests
    Recovering,
    /// Generic error
    Other {
        /// Error message
        message: String,
    },
}

impl fmt::Display for RaftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RaftError::NotLeader { leader_id } => {
                write!(f, "Not leader")?;
                if let Some(id) = leader_id {
                    write!(f, " (current leader: {})", id)?;
                }
                Ok(())
            }
            RaftError::InvalidState { expected, actual } => {
                write!(f, "Invalid state: expected {}, got {}", expected, actual)
            }
            RaftError::LogInconsistency { reason } => {
                write!(f, "Log inconsistency: {}", reason)
            }
            RaftError::StorageError { message } => {
                write!(f, "Storage error: {}", message)
            }
            RaftError::StaleTerm { current, received } => {
                write!(f, "Stale term: current {}, received {}", current, received)
            }
            RaftError::VoteAlreadyGranted { voted_for } => {
                write!(f, "Vote already granted to node {}", voted_for)
            }
            RaftError::ConfigError { message } => {
                write!(f, "Configuration error: {}", message)
            }
            RaftError::NetworkError { message } => {
                write!(f, "Network error: {}", message)
            }
            RaftError::Timeout { description } => {
                write!(f, "Timeout: {}", description)
            }
            RaftError::MembershipChangeInProgress => {
                write!(
                    f,
                    "A membership change is already in progress (joint consensus active)"
                )
            }
            RaftError::NodeAlreadyMember { node_id } => {
                write!(f, "Node {} is already a member of the cluster", node_id)
            }
            RaftError::NodeNotMember { node_id } => {
                write!(f, "Node {} is not a member of the cluster", node_id)
            }
            RaftError::StateMachineError { message } => {
                write!(f, "State machine error: {}", message)
            }
            RaftError::Recovering => {
                write!(
                    f,
                    "Node is replaying WAL on startup and cannot serve requests"
                )
            }
            RaftError::Other { message } => {
                write!(f, "Error: {}", message)
            }
        }
    }
}

impl std::error::Error for RaftError {}
