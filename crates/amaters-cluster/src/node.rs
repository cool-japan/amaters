//! Main Raft node implementation

use crate::error::{RaftError, RaftResult};
use crate::log::{Command, RaftLog};
use crate::rpc::{
    AppendEntriesRequest, AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse,
};
use crate::state::{CandidateState, LeaderState, PersistentState, VolatileState};
use crate::types::{LogIndex, NodeId, NodeState, RaftConfig, Term};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// A Raft consensus node
pub struct RaftNode {
    /// Node configuration
    config: Arc<RaftConfig>,
    /// Persistent state
    persistent: Arc<RwLock<PersistentState>>,
    /// Volatile state
    volatile: Arc<RwLock<VolatileState>>,
    /// Raft log
    log: Arc<RwLock<RaftLog>>,
    /// Leader-specific state
    leader_state: Arc<RwLock<Option<LeaderState>>>,
    /// Candidate-specific state
    candidate_state: Arc<RwLock<Option<CandidateState>>>,
    /// Last time we received a message from the leader
    last_heartbeat: Arc<RwLock<Instant>>,
}

impl RaftNode {
    /// Create a new Raft node
    pub fn new(config: RaftConfig) -> RaftResult<Self> {
        // Validate configuration
        config
            .validate()
            .map_err(|msg| RaftError::ConfigError { message: msg })?;

        Ok(Self {
            config: Arc::new(config),
            persistent: Arc::new(RwLock::new(PersistentState::new())),
            volatile: Arc::new(RwLock::new(VolatileState::new())),
            log: Arc::new(RwLock::new(RaftLog::new())),
            leader_state: Arc::new(RwLock::new(None)),
            candidate_state: Arc::new(RwLock::new(None)),
            last_heartbeat: Arc::new(RwLock::new(Instant::now())),
        })
    }

    /// Get the current node ID
    pub fn node_id(&self) -> NodeId {
        self.config.node_id
    }

    /// Get the current term
    pub fn current_term(&self) -> Term {
        self.persistent.read().current_term
    }

    /// Get the current state
    pub fn state(&self) -> NodeState {
        self.volatile.read().node_state
    }

    /// Get the current leader ID
    pub fn leader_id(&self) -> Option<NodeId> {
        self.volatile.read().leader_id
    }

    /// Check if this node is the leader
    pub fn is_leader(&self) -> bool {
        self.volatile.read().is_leader()
    }

    /// Get the commit index
    pub fn commit_index(&self) -> LogIndex {
        self.log.read().commit_index()
    }

    /// Get the last log index
    pub fn last_log_index(&self) -> LogIndex {
        self.log.read().last_index()
    }

    /// Append a command to the log (leader only)
    pub fn propose(&self, command: Command) -> RaftResult<LogIndex> {
        let volatile = self.volatile.read();
        if !volatile.is_leader() {
            return Err(RaftError::NotLeader {
                leader_id: volatile.leader_id,
            });
        }
        drop(volatile);

        let term = self.current_term();
        let mut log = self.log.write();
        let index = log.append(term, command);

        info!(
            node_id = self.node_id(),
            index = index,
            term = term,
            "Proposed new entry"
        );

        Ok(index)
    }

    /// Handle a RequestVote RPC
    pub fn handle_request_vote(&self, req: RequestVoteRequest) -> RequestVoteResponse {
        let mut persistent = self.persistent.write();
        let mut volatile = self.volatile.write();

        debug!(
            node_id = self.node_id(),
            candidate = req.candidate_id,
            term = req.term,
            "Received RequestVote"
        );

        // Update term if necessary
        if req.term > persistent.current_term {
            persistent.update_term(req.term);
            volatile.become_follower(None);
            *self.leader_state.write() = None;
            *self.candidate_state.write() = None;
        }

        // Reject if term is stale
        if req.term < persistent.current_term {
            warn!(
                node_id = self.node_id(),
                candidate = req.candidate_id,
                current_term = persistent.current_term,
                request_term = req.term,
                "Rejecting vote: stale term"
            );
            return RequestVoteResponse::rejected(persistent.current_term);
        }

        // Check if we've already voted
        if let Some(voted_for) = persistent.voted_for {
            if voted_for != req.candidate_id {
                warn!(
                    node_id = self.node_id(),
                    candidate = req.candidate_id,
                    voted_for = voted_for,
                    "Rejecting vote: already voted"
                );
                return RequestVoteResponse::rejected(persistent.current_term);
            }
        }

        // Check if candidate's log is at least as up-to-date as ours
        let log = self.log.read();
        let our_last_index = log.last_index();
        let our_last_term = log.last_term();

        let log_ok = req.last_log_term > our_last_term
            || (req.last_log_term == our_last_term && req.last_log_index >= our_last_index);

        if !log_ok {
            warn!(
                node_id = self.node_id(),
                candidate = req.candidate_id,
                our_last_index = our_last_index,
                our_last_term = our_last_term,
                candidate_last_index = req.last_log_index,
                candidate_last_term = req.last_log_term,
                "Rejecting vote: candidate log not up-to-date"
            );
            return RequestVoteResponse::rejected(persistent.current_term);
        }

        // Grant vote
        persistent.grant_vote(req.candidate_id);
        *self.last_heartbeat.write() = Instant::now();

        info!(
            node_id = self.node_id(),
            candidate = req.candidate_id,
            term = req.term,
            "Granted vote"
        );

        RequestVoteResponse::granted(persistent.current_term)
    }

    /// Handle an AppendEntries RPC
    pub fn handle_append_entries(&self, req: AppendEntriesRequest) -> AppendEntriesResponse {
        let mut persistent = self.persistent.write();
        let mut volatile = self.volatile.write();

        debug!(
            node_id = self.node_id(),
            leader = req.leader_id,
            term = req.term,
            entries = req.entries.len(),
            "Received AppendEntries"
        );

        // Update term if necessary
        if req.term > persistent.current_term {
            persistent.update_term(req.term);
            volatile.become_follower(Some(req.leader_id));
            *self.leader_state.write() = None;
            *self.candidate_state.write() = None;
        }

        // Reject if term is stale
        if req.term < persistent.current_term {
            warn!(
                node_id = self.node_id(),
                leader = req.leader_id,
                current_term = persistent.current_term,
                request_term = req.term,
                "Rejecting AppendEntries: stale term"
            );
            return AppendEntriesResponse::rejected(persistent.current_term);
        }

        // Update heartbeat and leader
        *self.last_heartbeat.write() = Instant::now();
        volatile.become_follower(Some(req.leader_id));
        *self.candidate_state.write() = None;

        drop(persistent);
        drop(volatile);

        // Handle the entries
        let mut log = self.log.write();
        let our_last_index = log.last_index();

        // Check if we have the previous log entry
        if req.prev_log_index > 0 && !log.matches(req.prev_log_index, req.prev_log_term) {
            // Find conflict index and term
            let conflict_index = req.prev_log_index.min(our_last_index);
            let conflict_term = log.get_term(conflict_index).unwrap_or(0);

            warn!(
                node_id = self.node_id(),
                prev_log_index = req.prev_log_index,
                prev_log_term = req.prev_log_term,
                conflict_index = conflict_index,
                conflict_term = conflict_term,
                "Rejecting AppendEntries: log inconsistency"
            );

            return AppendEntriesResponse::failure(
                self.current_term(),
                our_last_index,
                conflict_index,
                conflict_term,
            );
        }

        // Append entries if any
        if !req.entries.is_empty() {
            // Delete conflicting entries
            let first_new_index = req.entries[0].index;
            if first_new_index <= our_last_index {
                if let Err(e) = log.truncate_from(first_new_index) {
                    warn!(
                        node_id = self.node_id(),
                        error = ?e,
                        "Failed to truncate log"
                    );
                    return AppendEntriesResponse::rejected(self.current_term());
                }
            }

            // Append new entries
            if let Err(e) = log.append_entries(req.entries) {
                warn!(
                    node_id = self.node_id(),
                    error = ?e,
                    "Failed to append entries"
                );
                return AppendEntriesResponse::rejected(self.current_term());
            }
        }

        // Update commit index
        if req.leader_commit > log.commit_index() {
            let new_commit = req.leader_commit.min(log.last_index());
            if let Err(e) = log.set_commit_index(new_commit) {
                warn!(
                    node_id = self.node_id(),
                    error = ?e,
                    "Failed to update commit index"
                );
            } else {
                debug!(
                    node_id = self.node_id(),
                    commit_index = new_commit,
                    "Updated commit index"
                );
            }
        }

        AppendEntriesResponse::success(self.current_term(), log.last_index())
    }

    /// Start an election (transition to candidate)
    pub fn start_election(&self) -> Vec<RequestVoteRequest> {
        let mut persistent = self.persistent.write();
        let mut volatile = self.volatile.write();

        // Increment term and vote for self
        persistent.current_term += 1;
        persistent.grant_vote(self.node_id());

        // Transition to candidate
        volatile.become_candidate();

        // Initialize candidate state
        *self.candidate_state.write() = Some(CandidateState::new(self.node_id()));

        let term = persistent.current_term;
        let log = self.log.read();
        let last_log_index = log.last_index();
        let last_log_term = log.last_term();

        info!(node_id = self.node_id(), term = term, "Started election");

        // Send RequestVote to all peers
        self.config
            .peers
            .iter()
            .filter(|&&peer| peer != self.node_id())
            .map(|&peer| {
                RequestVoteRequest::new(term, self.node_id(), last_log_index, last_log_term)
            })
            .collect()
    }

    /// Handle a vote response during election
    pub fn handle_vote_response(&self, from: NodeId, resp: RequestVoteResponse) -> bool {
        let should_become_leader = {
            let mut persistent = self.persistent.write();
            let mut volatile = self.volatile.write();

            // Check if we're still a candidate
            if !volatile.is_candidate() {
                return false;
            }

            // Update term if necessary
            if resp.term > persistent.current_term {
                persistent.update_term(resp.term);
                volatile.become_follower(None);
                *self.candidate_state.write() = None;
                return false;
            }

            // Ignore stale responses
            if resp.term < persistent.current_term {
                return false;
            }

            // Record vote if granted
            if resp.vote_granted {
                let mut candidate_state_guard = self.candidate_state.write();
                if let Some(candidate_state) = candidate_state_guard.as_mut() {
                    candidate_state.record_vote(from);

                    info!(
                        node_id = self.node_id(),
                        from = from,
                        votes = candidate_state.vote_count(),
                        quorum = self.config.quorum_size(),
                        "Received vote"
                    );

                    // Check if we have a quorum
                    candidate_state.has_quorum(self.config.quorum_size())
                } else {
                    false
                }
            } else {
                false
            }
        };

        // Become leader outside of locks to prevent deadlock
        if should_become_leader {
            self.become_leader();
            return true;
        }

        false
    }

    /// Transition to leader
    fn become_leader(&self) {
        let mut volatile = self.volatile.write();
        volatile.become_leader();

        let log = self.log.read();
        let last_log_index = log.last_index();

        // Initialize leader state
        *self.leader_state.write() = Some(LeaderState::new(&self.config.peers, last_log_index));
        *self.candidate_state.write() = None;

        info!(
            node_id = self.node_id(),
            term = self.current_term(),
            "Became leader"
        );
    }

    /// Create heartbeat messages for all peers
    pub fn create_heartbeats(&self) -> Vec<(NodeId, AppendEntriesRequest)> {
        let volatile = self.volatile.read();
        if !volatile.is_leader() {
            return Vec::new();
        }
        drop(volatile);

        let term = self.current_term();
        let log = self.log.read();
        let leader_commit = log.commit_index();

        self.config
            .peers
            .iter()
            .filter(|&&peer| peer != self.node_id())
            .map(|&peer| {
                let prev_log_index = log.last_index();
                let prev_log_term = log.last_term();

                let req = AppendEntriesRequest::heartbeat(
                    term,
                    self.node_id(),
                    prev_log_index,
                    prev_log_term,
                    leader_commit,
                );

                (peer, req)
            })
            .collect()
    }

    /// Create replication messages for all peers
    pub fn create_replication_requests(&self) -> Vec<(NodeId, AppendEntriesRequest)> {
        let volatile = self.volatile.read();
        if !volatile.is_leader() {
            return Vec::new();
        }
        drop(volatile);

        let leader_state_guard = self.leader_state.read();
        let leader_state = match leader_state_guard.as_ref() {
            Some(state) => state,
            None => return Vec::new(),
        };

        let term = self.current_term();
        let log = self.log.read();
        let leader_commit = log.commit_index();

        self.config
            .peers
            .iter()
            .filter(|&&peer| peer != self.node_id())
            .filter_map(|&peer| {
                let next_index = leader_state.get_next_index(peer);

                if next_index > log.last_index() {
                    return None;
                }

                let prev_log_index = if next_index > 1 { next_index - 1 } else { 0 };
                let prev_log_term = log.get_term(prev_log_index).unwrap_or(0);

                let entries = log.get_entries_from(next_index, self.config.max_entries_per_message);

                if entries.is_empty() {
                    return None;
                }

                let req = AppendEntriesRequest::new(
                    term,
                    self.node_id(),
                    prev_log_index,
                    prev_log_term,
                    entries,
                    leader_commit,
                );

                Some((peer, req))
            })
            .collect()
    }

    /// Handle a replication response
    pub fn handle_replication_response(
        &self,
        from: NodeId,
        resp: AppendEntriesResponse,
    ) -> RaftResult<()> {
        let mut persistent = self.persistent.write();
        let mut volatile = self.volatile.write();

        // Check if we're still the leader
        if !volatile.is_leader() {
            return Ok(());
        }

        // Update term if necessary
        if resp.term > persistent.current_term {
            persistent.update_term(resp.term);
            volatile.become_follower(None);
            *self.leader_state.write() = None;
            return Ok(());
        }

        drop(persistent);
        drop(volatile);

        let mut leader_state_guard = self.leader_state.write();
        let leader_state = match leader_state_guard.as_mut() {
            Some(state) => state,
            None => return Ok(()),
        };

        if resp.success {
            // Update match_index and next_index
            leader_state.update_success(from, resp.last_log_index);

            debug!(
                node_id = self.node_id(),
                peer = from,
                match_index = resp.last_log_index,
                "Replication successful"
            );

            // Try to advance commit index
            let new_commit = leader_state
                .calculate_commit_index(self.log.read().last_index(), self.config.quorum_size());

            let mut log = self.log.write();
            if new_commit > log.commit_index() {
                // Verify that the entry at new_commit has the current term
                if let Some(term) = log.get_term(new_commit) {
                    if term == self.current_term() {
                        log.set_commit_index(new_commit)?;
                        info!(
                            node_id = self.node_id(),
                            commit_index = new_commit,
                            "Advanced commit index"
                        );
                    }
                }
            }
        } else {
            // Replication failed, decrement next_index
            leader_state.update_failure(from);

            warn!(
                node_id = self.node_id(),
                peer = from,
                next_index = leader_state.get_next_index(from),
                "Replication failed, will retry"
            );
        }

        Ok(())
    }

    /// Check if election timeout has elapsed
    pub fn election_timeout_elapsed(&self) -> bool {
        let last_heartbeat = *self.last_heartbeat.read();
        let timeout = self.config.random_election_timeout();
        last_heartbeat.elapsed() >= timeout
    }

    /// Reset election timer
    pub fn reset_election_timer(&self) {
        *self.last_heartbeat.write() = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_node(node_id: NodeId) -> RaftNode {
        let config = RaftConfig::new(node_id, vec![1, 2, 3]);
        RaftNode::new(config).expect("Failed to create node")
    }

    #[test]
    fn test_new_node() {
        let node = create_test_node(1);
        assert_eq!(node.node_id(), 1);
        assert_eq!(node.current_term(), 0);
        assert_eq!(node.state(), NodeState::Follower);
        assert_eq!(node.leader_id(), None);
    }

    #[test]
    fn test_start_election() {
        let node = create_test_node(1);
        let requests = node.start_election();

        assert_eq!(node.state(), NodeState::Candidate);
        assert_eq!(node.current_term(), 1);
        assert_eq!(requests.len(), 2); // 3 peers - self
    }

    #[test]
    fn test_handle_vote_granted() {
        let node = create_test_node(1);
        node.start_election();

        // With 3 nodes, quorum is 2 (self + 1 vote)
        // After start_election, node has 1 vote (self)
        // After first granted vote, node has 2 votes = quorum
        let resp = RequestVoteResponse::granted(1);
        let became_leader = node.handle_vote_response(2, resp);
        assert!(became_leader);
        assert_eq!(node.state(), NodeState::Leader);
    }

    #[test]
    fn test_propose_as_follower() {
        let node = create_test_node(1);
        let result = node.propose(Command::from_str("test"));
        assert!(result.is_err());
    }

    #[test]
    fn test_propose_as_leader() {
        let node = create_test_node(1);
        node.start_election();

        // Become leader
        let resp = RequestVoteResponse::granted(1);
        node.handle_vote_response(2, resp);

        // Now we can propose
        let result = node.propose(Command::from_str("test"));
        assert!(result.is_ok());
    }
}
