//! Raft persistent and volatile state

use crate::types::{FencingToken, LogIndex, NodeId, NodeState, Term};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::debug;

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
            debug!(
                old_term = self.current_term,
                new_term = new_term,
                "Persistent state: term updated, cleared voted_for"
            );
            self.current_term = new_term;
            self.voted_for = None;
        }
    }

    /// Grant a vote to a candidate
    pub fn grant_vote(&mut self, candidate_id: NodeId) {
        debug!(
            candidate_id = candidate_id,
            term = self.current_term,
            "Persistent state: vote granted"
        );
        self.voted_for = Some(candidate_id);
    }
}

impl Default for PersistentState {
    fn default() -> Self {
        Self::new()
    }
}

/// Volatile fencing-token state shared across the cluster node.
///
/// Stores the current packed fencing token as an `AtomicU64` so that concurrent
/// readers (e.g. storage guards) can check staleness without taking a lock.
/// The high 32 bits encode the Raft term; the low 32 bits encode the monotonic
/// write sequence within that term.
pub struct FencingTokenState {
    current_token: AtomicU64,
}

impl FencingTokenState {
    /// Create a new state with token = 0 (no leader epoch yet).
    pub fn new() -> Self {
        Self {
            current_token: AtomicU64::new(0),
        }
    }

    /// Atomically issue the next fencing token by incrementing the sequence.
    ///
    /// Intended to be called on every write from the current leader.
    pub fn issue_token(&self) -> FencingToken {
        let raw = self.current_token.fetch_add(1, Ordering::SeqCst);
        FencingToken(raw)
    }

    /// Bump the token to a new leader term, resetting the sequence to zero.
    ///
    /// This must be called atomically when a node wins an election.
    pub fn bump_term_token(&self, new_term: u32) {
        let token = FencingToken::new_leader_term(new_term);
        self.current_token.store(token.raw(), Ordering::SeqCst);
    }

    /// Read the current raw token value (for serialisation / inspection).
    pub fn current_raw(&self) -> u64 {
        self.current_token.load(Ordering::SeqCst)
    }
}

impl Default for FencingTokenState {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FencingTokenState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let raw = self.current_token.load(Ordering::Relaxed);
        let token = FencingToken(raw);
        f.debug_struct("FencingTokenState")
            .field("term", &token.term())
            .field("seq", &token.seq())
            .finish()
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

    /// Update next_index for a peer after failed replication using conflict hints.
    ///
    /// This implements the "fast backup" optimization from the Raft paper:
    /// instead of decrementing next_index one at a time, we jump back to the
    /// conflict point reported by the follower.
    ///
    /// - `conflict_index`: the first index of the conflicting term on the follower
    /// - `conflict_term`: the term of the conflicting entry
    /// - `follower_last_index`: the follower's last log index
    ///
    /// If the leader has entries with `conflict_term`, it sets `next_index` to
    /// the index after its last entry of that term. Otherwise, it sets
    /// `next_index` to `conflict_index`.
    pub fn update_failure_with_hint(
        &mut self,
        peer: NodeId,
        conflict_index: Option<LogIndex>,
        _conflict_term: Option<Term>,
        follower_last_index: LogIndex,
    ) {
        let new_next = match conflict_index {
            Some(ci) if ci > 0 => {
                // Jump back to the conflict index
                ci
            }
            _ => {
                // No conflict hint; fall back to follower's last index + 1
                // (but at least 1)
                (follower_last_index + 1).max(1)
            }
        };

        // Ensure we never go backwards past 1
        let clamped = new_next.max(1);

        // Only update if this actually moves next_index backwards (or stays)
        if let Some(next_idx) = self.next_index.get_mut(&peer) {
            if clamped < *next_idx {
                *next_idx = clamped;
            } else {
                // Fall back to simple decrement if hint doesn't help
                if *next_idx > 1 {
                    *next_idx -= 1;
                }
            }
        } else {
            self.next_index.insert(peer, clamped);
        }
    }

    /// Calculate the commit index considering joint consensus.
    ///
    /// During joint consensus, an entry must be replicated to a majority of
    /// **both** the old and new configurations. The leader itself counts toward
    /// both configs.
    pub fn calculate_commit_index_joint(
        &self,
        leader_id: NodeId,
        current_last_index: LogIndex,
        config_state: &crate::types::ConfigState,
    ) -> LogIndex {
        match config_state {
            crate::types::ConfigState::Stable(config) => {
                let quorum = config.quorum_size();
                self.calculate_commit_index(current_last_index, quorum)
            }
            crate::types::ConfigState::Joint { old, new } => {
                // For each config, count how many members have replicated
                // each index. The leader is always up-to-date.
                let old_commit = Self::quorum_index_for_config(
                    old,
                    leader_id,
                    current_last_index,
                    &self.match_index,
                );
                let new_commit = Self::quorum_index_for_config(
                    new,
                    leader_id,
                    current_last_index,
                    &self.match_index,
                );

                // Must be committed in both configs
                old_commit.min(new_commit)
            }
        }
    }

    /// Find the highest index replicated to a majority of a single config.
    fn quorum_index_for_config(
        config: &crate::types::ClusterConfig,
        leader_id: NodeId,
        leader_last_index: LogIndex,
        match_index: &HashMap<NodeId, LogIndex>,
    ) -> LogIndex {
        let member_ids = config.member_ids();
        let quorum = config.quorum_size();

        // Collect match indices for members of this config
        let mut indices: Vec<LogIndex> = member_ids
            .iter()
            .map(|&id| {
                if id == leader_id {
                    leader_last_index
                } else {
                    match_index.get(&id).copied().unwrap_or(0)
                }
            })
            .collect();

        indices.sort_unstable();
        indices.reverse();

        // The index at position (quorum-1) is the highest index
        // replicated to at least `quorum` members.
        if indices.len() >= quorum && quorum > 0 {
            indices[quorum - 1]
        } else {
            0
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

    // ── Fast backup / conflict hint tests ─────────────────────────────

    #[test]
    fn test_update_failure_with_hint_jumps_to_conflict() {
        let peers = vec![2, 3, 4];
        let mut ls = LeaderState::new(&peers, 10);
        // next_index for peer 2 starts at 11

        ls.update_failure_with_hint(2, Some(5), Some(2), 8);
        assert_eq!(
            ls.get_next_index(2),
            5,
            "should jump back to conflict_index"
        );
    }

    #[test]
    fn test_update_failure_with_hint_no_hint_uses_last_index() {
        let peers = vec![2, 3];
        let mut ls = LeaderState::new(&peers, 10);

        ls.update_failure_with_hint(2, None, None, 3);
        assert_eq!(
            ls.get_next_index(2),
            4,
            "should use follower_last_index + 1"
        );
    }

    #[test]
    fn test_update_failure_with_hint_does_not_go_forward() {
        let peers = vec![2, 3];
        let mut ls = LeaderState::new(&peers, 5);
        // next_index for peer 2 = 6

        // Conflict hint at index 10 -- should not advance next_index
        ls.update_failure_with_hint(2, Some(10), Some(1), 9);
        // Should fall back to simple decrement since hint is not helpful
        assert_eq!(ls.get_next_index(2), 5);
    }

    // ── Joint consensus commit index tests ────────────────────────────

    #[test]
    fn test_calculate_commit_index_joint_stable() {
        use crate::types::{ClusterConfig, ConfigState};

        let peers = vec![2, 3, 4];
        let mut ls = LeaderState::new(&peers, 10);
        ls.update_success(2, 8);
        ls.update_success(3, 7);
        ls.update_success(4, 6);

        // Stable config: {1, 2, 3, 4, 5} -- but we only track 2, 3, 4
        // Leader (1) has last_index 10
        let config = ConfigState::Stable(ClusterConfig::new(
            vec![
                (1, "a".into()),
                (2, "b".into()),
                (3, "c".into()),
                (4, "d".into()),
                (5, "e".into()),
            ],
            0,
        ));

        // Quorum = 3 out of 5
        // Indices: leader=10, 2=8, 3=7, 4=6, 5=0
        // Sorted desc: [10, 8, 7, 6, 0]
        // quorum-1 = 2 => index 7
        let commit = ls.calculate_commit_index_joint(1, 10, &config);
        assert_eq!(commit, 7);
    }

    #[test]
    fn test_calculate_commit_index_joint_consensus() {
        use crate::types::{ClusterConfig, ConfigState};

        let peers = vec![2, 3, 4];
        let mut ls = LeaderState::new(&peers, 10);
        ls.update_success(2, 8);
        ls.update_success(3, 7);
        ls.update_success(4, 9);

        // old: {1, 2, 3} quorum = 2
        // new: {1, 2, 3, 4} quorum = 3
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

        let config = ConfigState::Joint { old, new };

        // old: leader=10, 2=8, 3=7 => sorted [10, 8, 7] => quorum(2)-1=1 => 8
        // new: leader=10, 2=8, 3=7, 4=9 => sorted [10, 9, 8, 7] => quorum(3)-1=2 => 8
        // min(8, 8) = 8
        let commit = ls.calculate_commit_index_joint(1, 10, &config);
        assert_eq!(commit, 8);
    }

    #[test]
    fn test_calculate_commit_index_joint_limited_by_old() {
        use crate::types::{ClusterConfig, ConfigState};

        let peers = vec![2, 3, 4, 5];
        let mut ls = LeaderState::new(&peers, 10);
        ls.update_success(2, 3); // in old config, low match
        ls.update_success(3, 9); // in both
        ls.update_success(4, 9); // only in new
        ls.update_success(5, 9); // only in new

        // old: {1, 2, 3} quorum = 2
        // new: {1, 3, 4, 5} quorum = 3
        let old = ClusterConfig::new(vec![(1, "a".into()), (2, "b".into()), (3, "c".into())], 0);
        let new = ClusterConfig::new(
            vec![
                (1, "a".into()),
                (3, "c".into()),
                (4, "d".into()),
                (5, "e".into()),
            ],
            1,
        );

        let config = ConfigState::Joint { old, new };

        // old: leader=10, 2=3, 3=9 => sorted [10, 9, 3] => quorum(2)-1=1 => 9
        // new: leader=10, 3=9, 4=9, 5=9 => sorted [10, 9, 9, 9] => quorum(3)-1=2 => 9
        // min(9, 9) = 9
        let commit = ls.calculate_commit_index_joint(1, 10, &config);
        assert_eq!(commit, 9);
    }
}
