//! Raft persistent and volatile state

use crate::types::{LogIndex, NodeId, NodeState, Term};
use std::collections::HashMap;

/// Persistent state on all servers (must be persisted before responding to RPCs)
#[derive(Debug, Clone)]
pub struct PersistentState {
    /// Latest term server has seen (initialized to 0, increases monotonically)
    pub current_term: Term,
    /// Candidate ID that received vote in current term (None if none)
    pub voted_for: Option<NodeId>,
}

impl PersistentState {
    /// Create a new persistent state
    pub fn new() -> Self {
        Self {
            current_term: 0,
            voted_for: None,
        }
    }

    /// Update the current term (clears voted_for if term increases)
    pub fn update_term(&mut self, new_term: Term) {
        if new_term > self.current_term {
            self.current_term = new_term;
            self.voted_for = None;
        }
    }

    /// Grant a vote to a candidate
    pub fn grant_vote(&mut self, candidate_id: NodeId) {
        self.voted_for = Some(candidate_id);
    }
}

impl Default for PersistentState {
    fn default() -> Self {
        Self::new()
    }
}

/// Volatile state on all servers
#[derive(Debug, Clone)]
pub struct VolatileState {
    /// Current node state
    pub node_state: NodeState,
    /// Current known leader ID (None if unknown)
    pub leader_id: Option<NodeId>,
}

impl VolatileState {
    /// Create a new volatile state
    pub fn new() -> Self {
        Self {
            node_state: NodeState::Follower,
            leader_id: None,
        }
    }

    /// Transition to follower state
    pub fn become_follower(&mut self, leader_id: Option<NodeId>) {
        self.node_state = NodeState::Follower;
        self.leader_id = leader_id;
    }

    /// Transition to candidate state
    pub fn become_candidate(&mut self) {
        self.node_state = NodeState::Candidate;
        self.leader_id = None;
    }

    /// Transition to leader state
    pub fn become_leader(&mut self) {
        self.node_state = NodeState::Leader;
        self.leader_id = None;
    }

    /// Check if this node is the leader
    pub fn is_leader(&self) -> bool {
        self.node_state == NodeState::Leader
    }

    /// Check if this node is a candidate
    pub fn is_candidate(&self) -> bool {
        self.node_state == NodeState::Candidate
    }

    /// Check if this node is a follower
    pub fn is_follower(&self) -> bool {
        self.node_state == NodeState::Follower
    }
}

impl Default for VolatileState {
    fn default() -> Self {
        Self::new()
    }
}

/// Volatile state on leaders (reinitialized after election)
#[derive(Debug, Clone)]
pub struct LeaderState {
    /// For each server, index of the next log entry to send to that server
    pub next_index: HashMap<NodeId, LogIndex>,
    /// For each server, index of highest log entry known to be replicated on server
    pub match_index: HashMap<NodeId, LogIndex>,
}

impl LeaderState {
    /// Create a new leader state
    pub fn new(peers: &[NodeId], last_log_index: LogIndex) -> Self {
        let mut next_index = HashMap::new();
        let mut match_index = HashMap::new();

        for &peer in peers {
            next_index.insert(peer, last_log_index + 1);
            match_index.insert(peer, 0);
        }

        Self {
            next_index,
            match_index,
        }
    }

    /// Update next_index for a peer after successful replication
    pub fn update_success(&mut self, peer: NodeId, match_idx: LogIndex) {
        self.match_index.insert(peer, match_idx);
        self.next_index.insert(peer, match_idx + 1);
    }

    /// Update next_index for a peer after failed replication
    pub fn update_failure(&mut self, peer: NodeId) {
        if let Some(next_idx) = self.next_index.get_mut(&peer) {
            if *next_idx > 1 {
                *next_idx -= 1;
            }
        }
    }

    /// Get next_index for a peer
    pub fn get_next_index(&self, peer: NodeId) -> LogIndex {
        self.next_index.get(&peer).copied().unwrap_or(1)
    }

    /// Get match_index for a peer
    pub fn get_match_index(&self, peer: NodeId) -> LogIndex {
        self.match_index.get(&peer).copied().unwrap_or(0)
    }

    /// Calculate the commit index based on match_index values
    /// Returns the highest index that is replicated on a majority of servers
    pub fn calculate_commit_index(&self, current_index: LogIndex, quorum_size: usize) -> LogIndex {
        // Collect all match indices
        let mut indices: Vec<LogIndex> = self.match_index.values().copied().collect();
        indices.sort_unstable();
        indices.reverse();

        // Find the index at the quorum position
        // quorum_size includes the leader, so we need quorum_size - 1 followers
        if indices.len() + 1 >= quorum_size {
            let quorum_idx = quorum_size.saturating_sub(2);
            if quorum_idx < indices.len() {
                return indices[quorum_idx].min(current_index);
            }
        }

        0
    }
}

/// Volatile state for candidates (during election)
#[derive(Debug, Clone)]
pub struct CandidateState {
    /// Votes received from peers (including self)
    pub votes_received: Vec<NodeId>,
}

impl CandidateState {
    /// Create a new candidate state
    pub fn new(self_id: NodeId) -> Self {
        Self {
            votes_received: vec![self_id],
        }
    }

    /// Record a vote from a peer
    pub fn record_vote(&mut self, peer: NodeId) {
        if !self.votes_received.contains(&peer) {
            self.votes_received.push(peer);
        }
    }

    /// Check if we have a quorum of votes
    pub fn has_quorum(&self, quorum_size: usize) -> bool {
        self.votes_received.len() >= quorum_size
    }

    /// Get the number of votes received
    pub fn vote_count(&self) -> usize {
        self.votes_received.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_persistent_state_new() {
        let state = PersistentState::new();
        assert_eq!(state.current_term, 0);
        assert_eq!(state.voted_for, None);
    }

    #[test]
    fn test_persistent_state_update_term() {
        let mut state = PersistentState::new();
        state.voted_for = Some(1);
        state.update_term(5);

        assert_eq!(state.current_term, 5);
        assert_eq!(state.voted_for, None);
    }

    #[test]
    fn test_persistent_state_grant_vote() {
        let mut state = PersistentState::new();
        state.grant_vote(2);
        assert_eq!(state.voted_for, Some(2));
    }

    #[test]
    fn test_volatile_state_new() {
        let state = VolatileState::new();
        assert_eq!(state.node_state, NodeState::Follower);
        assert_eq!(state.leader_id, None);
    }

    #[test]
    fn test_volatile_state_transitions() {
        let mut state = VolatileState::new();

        state.become_candidate();
        assert!(state.is_candidate());
        assert_eq!(state.leader_id, None);

        state.become_leader();
        assert!(state.is_leader());
        assert_eq!(state.leader_id, None);

        state.become_follower(Some(5));
        assert!(state.is_follower());
        assert_eq!(state.leader_id, Some(5));
    }

    #[test]
    fn test_leader_state_new() {
        let peers = vec![1, 2, 3];
        let leader_state = LeaderState::new(&peers, 10);

        assert_eq!(leader_state.get_next_index(1), 11);
        assert_eq!(leader_state.get_match_index(1), 0);
    }

    #[test]
    fn test_leader_state_update_success() {
        let peers = vec![1, 2, 3];
        let mut leader_state = LeaderState::new(&peers, 10);

        leader_state.update_success(1, 12);
        assert_eq!(leader_state.get_next_index(1), 13);
        assert_eq!(leader_state.get_match_index(1), 12);
    }

    #[test]
    fn test_leader_state_update_failure() {
        let peers = vec![1, 2, 3];
        let mut leader_state = LeaderState::new(&peers, 10);

        leader_state.update_failure(1);
        assert_eq!(leader_state.get_next_index(1), 10);
    }

    #[test]
    fn test_leader_state_calculate_commit_index() {
        let peers = vec![2, 3, 4, 5];
        let mut leader_state = LeaderState::new(&peers, 10);

        // With 5 nodes total, quorum is 3
        leader_state.update_success(2, 8);
        leader_state.update_success(3, 9);
        leader_state.update_success(4, 7);
        leader_state.update_success(5, 6);

        // Sorted match indices: [9, 8, 7, 6]
        // At position 1 (quorum_size - 2 = 3 - 2 = 1): index 8
        let commit_idx = leader_state.calculate_commit_index(10, 3);
        assert_eq!(commit_idx, 8);
    }

    #[test]
    fn test_candidate_state_new() {
        let state = CandidateState::new(1);
        assert_eq!(state.vote_count(), 1);
        assert!(state.votes_received.contains(&1));
    }

    #[test]
    fn test_candidate_state_record_vote() {
        let mut state = CandidateState::new(1);
        state.record_vote(2);
        state.record_vote(3);

        assert_eq!(state.vote_count(), 3);
        assert!(state.has_quorum(2));
    }

    #[test]
    fn test_candidate_state_has_quorum() {
        let mut state = CandidateState::new(1);
        assert!(state.has_quorum(1));
        assert!(!state.has_quorum(2));

        state.record_vote(2);
        assert!(state.has_quorum(2));
    }
}
