//! Core types for Raft consensus

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

/// Node identifier
pub type NodeId = u64;

/// Raft term number
pub type Term = u64;

/// Log entry index (1-indexed, 0 means no entry)
pub type LogIndex = u64;

/// A membership change request for dynamic cluster reconfiguration
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipChange {
    /// Add a new node to the cluster
    AddNode {
        /// The node ID to add
        node_id: NodeId,
        /// The network address of the node
        address: String,
    },
    /// Remove an existing node from the cluster
    RemoveNode {
        /// The node ID to remove
        node_id: NodeId,
    },
}

/// Tracks cluster members with their addresses and a monotonically increasing version
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterConfig {
    /// Map of node IDs to their network addresses
    members: Vec<(NodeId, String)>,
    /// Monotonically increasing version number for this configuration
    version: u64,
}

impl ClusterConfig {
    /// Create a new cluster config with the given members and version
    pub fn new(members: Vec<(NodeId, String)>, version: u64) -> Self {
        Self { members, version }
    }

    /// Get the list of member node IDs
    pub fn member_ids(&self) -> HashSet<NodeId> {
        self.members.iter().map(|(id, _)| *id).collect()
    }

    /// Get all members as (node_id, address) pairs
    pub fn members(&self) -> &[(NodeId, String)] {
        &self.members
    }

    /// Get the version of this configuration
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Check if a node is a member
    pub fn contains(&self, node_id: NodeId) -> bool {
        self.members.iter().any(|(id, _)| *id == node_id)
    }

    /// Get the majority quorum size for this config
    pub fn quorum_size(&self) -> usize {
        self.members.len() / 2 + 1
    }

    /// Get the number of members
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Check if the config has no members
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Add a member to the config, returning a new config with incremented version
    pub fn with_added_member(&self, node_id: NodeId, address: String) -> Self {
        let mut members = self.members.clone();
        if !self.contains(node_id) {
            members.push((node_id, address));
        }
        Self {
            members,
            version: self.version + 1,
        }
    }

    /// Remove a member from the config, returning a new config with incremented version
    pub fn without_member(&self, node_id: NodeId) -> Self {
        let members: Vec<_> = self
            .members
            .iter()
            .filter(|(id, _)| *id != node_id)
            .cloned()
            .collect();
        Self {
            members,
            version: self.version + 1,
        }
    }
}

/// The state of cluster configuration during membership changes.
///
/// Implements the Raft joint consensus protocol (Section 6):
/// - `Stable`: Normal operation with a single configuration
/// - `Joint`: Transitional state requiring majority from BOTH old and new configs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigState {
    /// Normal operation with a single configuration
    Stable(ClusterConfig),
    /// Joint consensus: decisions require majority of both old and new configs
    Joint {
        /// The old (current) configuration
        old: ClusterConfig,
        /// The new (target) configuration
        new: ClusterConfig,
    },
}

impl ConfigState {
    /// Create a new stable config state
    pub fn new_stable(members: Vec<(NodeId, String)>) -> Self {
        ConfigState::Stable(ClusterConfig::new(members, 0))
    }

    /// Get all unique member node IDs across both configs (if joint)
    pub fn all_member_ids(&self) -> HashSet<NodeId> {
        match self {
            ConfigState::Stable(config) => config.member_ids(),
            ConfigState::Joint { old, new } => {
                let mut ids = old.member_ids();
                ids.extend(new.member_ids());
                ids
            }
        }
    }

    /// Check if we are in joint consensus
    pub fn is_joint(&self) -> bool {
        matches!(self, ConfigState::Joint { .. })
    }

    /// Get the current version (max of both configs if joint)
    pub fn version(&self) -> u64 {
        match self {
            ConfigState::Stable(config) => config.version(),
            ConfigState::Joint { old, new } => old.version().max(new.version()),
        }
    }

    /// Check if a given set of responding nodes forms a quorum.
    ///
    /// During joint consensus, a quorum requires majority in BOTH the old
    /// and new configurations independently.
    pub fn has_quorum(&self, responding_nodes: &HashSet<NodeId>) -> bool {
        match self {
            ConfigState::Stable(config) => {
                let count = config.member_ids().intersection(responding_nodes).count();
                count >= config.quorum_size()
            }
            ConfigState::Joint { old, new } => {
                let old_count = old.member_ids().intersection(responding_nodes).count();
                let new_count = new.member_ids().intersection(responding_nodes).count();
                old_count >= old.quorum_size() && new_count >= new.quorum_size()
            }
        }
    }

    /// Get the stable config (only valid if not in joint state)
    pub fn stable_config(&self) -> Option<&ClusterConfig> {
        match self {
            ConfigState::Stable(config) => Some(config),
            ConfigState::Joint { .. } => None,
        }
    }

    /// Get all members as (node_id, address) pairs
    pub fn all_members(&self) -> Vec<(NodeId, String)> {
        match self {
            ConfigState::Stable(config) => config.members().to_vec(),
            ConfigState::Joint { old, new } => {
                let mut seen = HashSet::new();
                let mut result = Vec::new();
                for (id, addr) in old.members().iter().chain(new.members().iter()) {
                    if seen.insert(*id) {
                        result.push((*id, addr.clone()));
                    }
                }
                result
            }
        }
    }
}

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
    pub snapshot_threshold: u64,
    /// Maximum number of snapshots to retain on disk
    pub max_snapshots: usize,
    /// Directory for storing snapshots (None = snapshots disabled on disk)
    pub snapshot_dir: Option<PathBuf>,
    /// Directory for Raft persistent state and log (None = in-memory only)
    pub persistence_dir: Option<PathBuf>,
    /// Directory for segment-based WAL replay on startup (None = WAL replay disabled)
    pub wal_dir: Option<PathBuf>,
    /// Whether to fsync after every persistent write (default: true)
    pub sync_on_write: bool,
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
            max_snapshots: 3,
            snapshot_dir: None,
            persistence_dir: None,
            wal_dir: None,
            sync_on_write: true,
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

/// Configuration for heartbeat-based failure detection
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// Interval between heartbeat sends in milliseconds
    pub interval_ms: u64,
    /// Time in milliseconds after which a peer is considered potentially failed
    pub timeout_ms: u64,
    /// Number of consecutive missed heartbeats before declaring failure
    pub max_missed: u32,
}

impl HeartbeatConfig {
    /// Create a new heartbeat configuration
    pub fn new(interval_ms: u64, timeout_ms: u64, max_missed: u32) -> Self {
        Self {
            interval_ms,
            timeout_ms,
            max_missed,
        }
    }

    /// Create a default heartbeat configuration
    /// Default: 100ms interval, 500ms timeout, 3 missed max
    pub fn default_config() -> Self {
        Self {
            interval_ms: 100,
            timeout_ms: 500,
            max_missed: 3,
        }
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.interval_ms == 0 {
            return Err("Heartbeat interval must be > 0".to_string());
        }
        if self.timeout_ms == 0 {
            return Err("Heartbeat timeout must be > 0".to_string());
        }
        if self.timeout_ms <= self.interval_ms {
            return Err(format!(
                "Heartbeat timeout ({}) must be greater than interval ({})",
                self.timeout_ms, self.interval_ms
            ));
        }
        if self.max_missed == 0 {
            return Err("max_missed must be > 0".to_string());
        }
        Ok(())
    }
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self::default_config()
    }
}

/// A packed monotonically increasing fencing token that uniquely identifies a write epoch.
///
/// Encoded as a single `u64`:
/// - High 32 bits = Raft term (capped at `u32::MAX` for compactness; terms exceeding `u32::MAX`
///   are exceedingly unlikely in any realistic deployment).
/// - Low 32 bits  = per-term monotonic sequence number.
///
/// Each time a node becomes leader it resets the sequence to zero and bumps the term
/// component via [`FencingToken::new_leader_term`].  Storage backends and followers use
/// the token to reject stale writes from former leaders: a write is stale when its
/// token's term is less than the current term, or the term matches but the sequence
/// has been superseded by a higher-sequence write in the same term.
///
/// # Format
///
/// ```text
/// [term: 32 bits][seq: 32 bits]
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FencingToken(pub u64);

impl FencingToken {
    /// Pack `term` and `seq` into a new fencing token.
    pub fn new(term: u32, seq: u32) -> Self {
        Self(((term as u64) << 32) | (seq as u64))
    }

    /// Extract the term component (high 32 bits).
    pub fn term(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// Extract the sequence component (low 32 bits).
    pub fn seq(self) -> u32 {
        self.0 as u32
    }

    /// Return the raw `u64` representation.
    pub fn raw(self) -> u64 {
        self.0
    }

    /// Return a new token with the sequence number incremented by one,
    /// keeping the term unchanged.
    ///
    /// Wraps on `u32` overflow (extremely unlikely in practice).
    pub fn bump_seq(self) -> Self {
        let term = self.term();
        let seq = self.seq().wrapping_add(1);
        Self::new(term, seq)
    }

    /// Return a fresh token for a new leader term; the sequence number resets to 0.
    pub fn new_leader_term(term: u32) -> Self {
        Self::new(term, 0)
    }
}

/// Events emitted by the failure detector
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureEvent {
    /// A node has been detected as failed (missed too many heartbeats)
    NodeFailed {
        /// The node that failed
        node_id: NodeId,
        /// Number of consecutive missed heartbeats
        missed_count: u32,
        /// Duration since last successful heartbeat
        last_seen_ago_ms: u64,
    },
    /// A previously failed node has recovered (heartbeat received again)
    NodeRecovered {
        /// The node that recovered
        node_id: NodeId,
    },
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

    // ── ClusterConfig tests ─────────────────────────────────────────

    #[test]
    fn test_cluster_config_new() {
        let members = vec![(1, "addr1".to_string()), (2, "addr2".to_string())];
        let cfg = ClusterConfig::new(members.clone(), 0);
        assert_eq!(cfg.len(), 2);
        assert_eq!(cfg.version(), 0);
        assert!(cfg.contains(1));
        assert!(cfg.contains(2));
        assert!(!cfg.contains(3));
    }

    #[test]
    fn test_cluster_config_quorum() {
        let members = vec![(1, "a".into()), (2, "b".into()), (3, "c".into())];
        let cfg = ClusterConfig::new(members, 0);
        assert_eq!(cfg.quorum_size(), 2); // 3/2 + 1 = 2
    }

    #[test]
    fn test_cluster_config_add_remove() {
        let members = vec![(1, "a".into()), (2, "b".into()), (3, "c".into())];
        let cfg = ClusterConfig::new(members, 0);

        let cfg2 = cfg.with_added_member(4, "d".into());
        assert_eq!(cfg2.len(), 4);
        assert!(cfg2.contains(4));
        assert_eq!(cfg2.version(), 1);

        let cfg3 = cfg2.without_member(2);
        assert_eq!(cfg3.len(), 3);
        assert!(!cfg3.contains(2));
        assert_eq!(cfg3.version(), 2);
    }

    #[test]
    fn test_cluster_config_add_existing_is_noop() {
        let members = vec![(1, "a".into()), (2, "b".into())];
        let cfg = ClusterConfig::new(members, 0);
        let cfg2 = cfg.with_added_member(1, "a2".into());
        // Should still have 2 members (not duplicated)
        assert_eq!(cfg2.len(), 2);
    }

    // ── ConfigState tests ───────────────────────────────────────────

    #[test]
    fn test_config_state_stable_quorum() {
        let members = vec![(1, "a".into()), (2, "b".into()), (3, "c".into())];
        let cs = ConfigState::new_stable(members);
        assert!(!cs.is_joint());

        let mut responding = HashSet::new();
        responding.insert(1);
        assert!(!cs.has_quorum(&responding)); // 1 of 3 -- no quorum

        responding.insert(2);
        assert!(cs.has_quorum(&responding)); // 2 of 3 -- quorum
    }

    #[test]
    fn test_config_state_joint_quorum() {
        let old = ClusterConfig::new(vec![(1, "a".into()), (2, "b".into()), (3, "c".into())], 0);
        let new = ClusterConfig::new(
            vec![
                (1, "a".into()),
                (2, "b".into()),
                (3, "c".into()),
                (4, "d".into()),
            ],
            1,
        );
        let cs = ConfigState::Joint {
            old: old.clone(),
            new: new.clone(),
        };
        assert!(cs.is_joint());

        // Need majority of old (2/3) AND new (3/4)
        let mut r = HashSet::new();
        r.insert(1);
        r.insert(2);
        // old: 2/3 ok, new: 2/4 not ok
        assert!(!cs.has_quorum(&r));

        r.insert(3);
        // old: 3/3 ok, new: 3/4 ok
        assert!(cs.has_quorum(&r));
    }

    #[test]
    fn test_config_state_all_members() {
        let old = ClusterConfig::new(vec![(1, "a".into()), (2, "b".into()), (3, "c".into())], 0);
        let new = ClusterConfig::new(vec![(1, "a".into()), (2, "b".into()), (4, "d".into())], 1);
        let cs = ConfigState::Joint { old, new };
        let members = cs.all_members();
        let ids: HashSet<NodeId> = members.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids.len(), 4); // 1, 2, 3, 4
        assert!(ids.contains(&3));
        assert!(ids.contains(&4));
    }

    #[test]
    fn test_config_state_version() {
        let cs = ConfigState::new_stable(vec![(1, "a".into())]);
        assert_eq!(cs.version(), 0);
    }

    // ── HeartbeatConfig tests ───────────────────────────────────────

    #[test]
    fn test_heartbeat_config_new() {
        let config = HeartbeatConfig::new(100, 500, 3);
        assert_eq!(config.interval_ms, 100);
        assert_eq!(config.timeout_ms, 500);
        assert_eq!(config.max_missed, 3);
    }

    #[test]
    fn test_heartbeat_config_default() {
        let config = HeartbeatConfig::default();
        assert_eq!(config.interval_ms, 100);
        assert_eq!(config.timeout_ms, 500);
        assert_eq!(config.max_missed, 3);
    }

    #[test]
    fn test_heartbeat_config_validate_ok() {
        let config = HeartbeatConfig::new(100, 500, 3);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_heartbeat_config_validate_zero_interval() {
        let config = HeartbeatConfig::new(0, 500, 3);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_heartbeat_config_validate_zero_timeout() {
        let config = HeartbeatConfig::new(100, 0, 3);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_heartbeat_config_validate_timeout_less_than_interval() {
        let config = HeartbeatConfig::new(100, 50, 3);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_heartbeat_config_validate_timeout_equal_interval() {
        let config = HeartbeatConfig::new(100, 100, 3);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_heartbeat_config_validate_zero_max_missed() {
        let config = HeartbeatConfig::new(100, 500, 0);
        assert!(config.validate().is_err());
    }

    // ── FailureEvent tests ──────────────────────────────────────────

    #[test]
    fn test_failure_event_node_failed_eq() {
        let a = FailureEvent::NodeFailed {
            node_id: 2,
            missed_count: 3,
            last_seen_ago_ms: 500,
        };
        let b = FailureEvent::NodeFailed {
            node_id: 2,
            missed_count: 3,
            last_seen_ago_ms: 500,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_failure_event_node_recovered_eq() {
        let a = FailureEvent::NodeRecovered { node_id: 2 };
        let b = FailureEvent::NodeRecovered { node_id: 2 };
        assert_eq!(a, b);
    }

    #[test]
    fn test_failure_event_ne() {
        let a = FailureEvent::NodeFailed {
            node_id: 2,
            missed_count: 3,
            last_seen_ago_ms: 500,
        };
        let b = FailureEvent::NodeRecovered { node_id: 2 };
        assert_ne!(a, b);
    }
}
