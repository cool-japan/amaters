//! Core types for Raft consensus

use std::time::Duration;

/// Node identifier
pub type NodeId = u64;

/// Raft term number
pub type Term = u64;

/// Log entry index (1-indexed, 0 means no entry)
pub type LogIndex = u64;

/// Raft node state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    /// Follower state - passive, responds to RPCs
    Follower,
    /// Candidate state - requesting votes for leadership
    Candidate,
    /// Leader state - handles client requests and replicates log
    Leader,
}

impl NodeState {
    /// Get the state name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeState::Follower => "Follower",
            NodeState::Candidate => "Candidate",
            NodeState::Leader => "Leader",
        }
    }
}

/// Configuration for a Raft node
#[derive(Debug, Clone)]
pub struct RaftConfig {
    /// This node's ID
    pub node_id: NodeId,
    /// List of all peer node IDs (including this node)
    pub peers: Vec<NodeId>,
    /// Election timeout range (min, max) in milliseconds
    pub election_timeout_range: (u64, u64),
    /// Heartbeat interval in milliseconds
    pub heartbeat_interval: u64,
    /// Maximum number of entries to send in a single AppendEntries RPC
    pub max_entries_per_message: usize,
    /// Whether to enable log compaction
    pub enable_compaction: bool,
    /// Snapshot threshold (number of log entries before triggering snapshot)
    pub snapshot_threshold: usize,
}

impl RaftConfig {
    /// Create a new Raft configuration with sensible defaults
    pub fn new(node_id: NodeId, peers: Vec<NodeId>) -> Self {
        Self {
            node_id,
            peers,
            election_timeout_range: (150, 300),
            heartbeat_interval: 50,
            max_entries_per_message: 100,
            enable_compaction: true,
            snapshot_threshold: 10000,
        }
    }

    /// Get a random election timeout within the configured range
    pub fn random_election_timeout(&self) -> Duration {
        use std::collections::hash_map::RandomState;
        use std::hash::BuildHasher;

        let (min, max) = self.election_timeout_range;
        let range = max - min;

        // Use current time as seed for randomization
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);

        let random_value = RandomState::new().hash_one(now);

        let timeout_ms = min + (random_value % range);
        Duration::from_millis(timeout_ms)
    }

    /// Get the heartbeat interval
    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_millis(self.heartbeat_interval)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        // Check that node_id is in peers list
        if !self.peers.contains(&self.node_id) {
            return Err(format!("Node ID {} not found in peers list", self.node_id));
        }

        // Check for odd number of nodes (for quorum)
        if self.peers.len() % 2 == 0 {
            return Err(format!(
                "Raft requires odd number of nodes, got {}",
                self.peers.len()
            ));
        }

        // Check minimum nodes
        if self.peers.len() < 3 {
            return Err(format!(
                "Raft requires at least 3 nodes for fault tolerance, got {}",
                self.peers.len()
            ));
        }

        // Check election timeout range
        let (min, max) = self.election_timeout_range;
        if min >= max {
            return Err(format!(
                "Election timeout min ({}) must be less than max ({})",
                min, max
            ));
        }

        // Check heartbeat interval vs election timeout
        if self.heartbeat_interval >= min {
            return Err(format!(
                "Heartbeat interval ({}) must be less than election timeout min ({})",
                self.heartbeat_interval, min
            ));
        }

        Ok(())
    }

    /// Calculate the quorum size (majority)
    pub fn quorum_size(&self) -> usize {
        self.peers.len() / 2 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_state_as_str() {
        assert_eq!(NodeState::Follower.as_str(), "Follower");
        assert_eq!(NodeState::Candidate.as_str(), "Candidate");
        assert_eq!(NodeState::Leader.as_str(), "Leader");
    }

    #[test]
    fn test_raft_config_new() {
        let config = RaftConfig::new(1, vec![1, 2, 3]);
        assert_eq!(config.node_id, 1);
        assert_eq!(config.peers, vec![1, 2, 3]);
        assert_eq!(config.election_timeout_range, (150, 300));
        assert_eq!(config.heartbeat_interval, 50);
    }

    #[test]
    fn test_raft_config_validate_valid() {
        let config = RaftConfig::new(1, vec![1, 2, 3]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_raft_config_validate_node_not_in_peers() {
        let config = RaftConfig::new(4, vec![1, 2, 3]);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_raft_config_validate_even_number_of_nodes() {
        let config = RaftConfig::new(1, vec![1, 2, 3, 4]);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_raft_config_validate_too_few_nodes() {
        let config = RaftConfig::new(1, vec![1]);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_raft_config_quorum_size() {
        let config = RaftConfig::new(1, vec![1, 2, 3]);
        assert_eq!(config.quorum_size(), 2);

        let config = RaftConfig::new(1, vec![1, 2, 3, 4, 5]);
        assert_eq!(config.quorum_size(), 3);
    }

    #[test]
    fn test_random_election_timeout() {
        let config = RaftConfig::new(1, vec![1, 2, 3]);
        let timeout1 = config.random_election_timeout();
        let timeout2 = config.random_election_timeout();

        // Both should be within range
        assert!(timeout1.as_millis() >= 150);
        assert!(timeout1.as_millis() <= 300);
        assert!(timeout2.as_millis() >= 150);
        assert!(timeout2.as_millis() <= 300);
    }
}
