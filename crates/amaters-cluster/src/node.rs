//! Main Raft node implementation

use crate::error::{RaftError, RaftResult};
use crate::log::{Command, LogEntry, RaftLog};
use crate::persistence::{FilePersistence, RaftPersistence};
use crate::rpc::{
    AppendEntriesRequest, AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse,
};
use crate::snapshot::{
    InstallSnapshotRequest, InstallSnapshotResponse, Snapshot, SnapshotConfig, SnapshotManager,
    SnapshotPolicy, SnapshotReceiver,
};
use crate::state::FencingTokenState;
use crate::state::{CandidateState, LeaderState, PersistentState, VolatileState};
use crate::types::{
    ClusterConfig, ConfigState, FencingToken, LogIndex, MembershipChange, NodeId, NodeState,
    RaftConfig, Term,
};
use crate::wal::{CorruptionPolicy, WalReader};
use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Snapshot manager for creating and loading snapshots
    snapshot_manager: Arc<RwLock<Option<SnapshotManager>>>,
    /// Receiver for chunked snapshot transfers from the leader
    snapshot_receiver: Arc<RwLock<Option<SnapshotReceiver>>>,
    /// Optional persistent storage backend
    persistence: Option<Arc<dyn RaftPersistence>>,
    /// Dynamic cluster membership state (joint consensus)
    config_state: Arc<RwLock<ConfigState>>,
    /// Whether this node has been removed and should step down
    stepping_down: Arc<RwLock<bool>>,
    /// Packed fencing token state (atomic, lock-free reads)
    fencing_token_state: Arc<FencingTokenState>,
    /// True while WAL replay is in progress; RPCs are rejected during this window
    is_recovering: Arc<AtomicBool>,
}

impl RaftNode {
    /// Create a new Raft node
    pub fn new(config: RaftConfig) -> RaftResult<Self> {
        // Validate configuration
        config
            .validate()
            .map_err(|msg| RaftError::ConfigError { message: msg })?;

        // Initialize snapshot manager if snapshot directory is configured
        let snapshot_manager = if let Some(ref dir) = config.snapshot_dir {
            let snap_config =
                SnapshotConfig::new(dir.clone(), config.max_snapshots, config.snapshot_threshold);
            Some(SnapshotManager::new(snap_config)?)
        } else {
            None
        };

        // Initialize persistence backend if persistence_dir is configured
        let persistence: Option<Arc<dyn RaftPersistence>> =
            if let Some(ref dir) = config.persistence_dir {
                Some(Arc::new(FilePersistence::new(dir, config.sync_on_write)?))
            } else {
                None
            };

        // If persistence is available, recover state from disk
        let (persistent_state, mut raft_log) = if let Some(ref p) = persistence {
            let (term, voted_for) = p.load_state()?;
            let mut ps = PersistentState::new();
            ps.current_term = term;
            ps.voted_for = voted_for;

            let entries = p.load_log()?;
            let mut log = RaftLog::new();
            if !entries.is_empty() {
                log.append_entries(entries)?;
            }

            // Restore applied_index from persistence
            let applied_idx = p.load_applied_index()?;
            if applied_idx > 0 && applied_idx <= log.last_index() {
                // Restore commit_index to applied_index (safe lower bound on recovery)
                if let Err(e) = log.set_commit_index(applied_idx) {
                    warn!(applied_idx, error = ?e, "Failed to restore commit index from applied_index");
                } else if let Err(e) = log.set_applied_index(applied_idx) {
                    warn!(applied_idx, error = ?e, "Failed to restore applied index");
                }
            }

            info!(
                node_id = config.node_id,
                term = term,
                voted_for = ?voted_for,
                last_log_index = log.last_index(),
                "Recovered state from persistence"
            );

            (ps, log)
        } else {
            (PersistentState::new(), RaftLog::new())
        };

        // Replay WAL entries if wal_dir is configured.
        // The `is_recovering` flag is set to true for the duration of replay
        // so that any concurrent RPC handlers can reject requests gracefully.
        let is_recovering = Arc::new(AtomicBool::new(false));
        if let Some(ref wal_dir) = config.wal_dir {
            is_recovering.store(true, Ordering::Release);
            let result = replay_wal_into_log(wal_dir, &mut raft_log);
            is_recovering.store(false, Ordering::Release);
            result?;
        }

        // Build initial stable config from peers (using empty addresses for now)
        let initial_members: Vec<(NodeId, String)> =
            config.peers.iter().map(|&id| (id, String::new())).collect();
        let config_state = ConfigState::new_stable(initial_members);

        Ok(Self {
            config: Arc::new(config),
            persistent: Arc::new(RwLock::new(persistent_state)),
            volatile: Arc::new(RwLock::new(VolatileState::new())),
            log: Arc::new(RwLock::new(raft_log)),
            leader_state: Arc::new(RwLock::new(None)),
            candidate_state: Arc::new(RwLock::new(None)),
            last_heartbeat: Arc::new(RwLock::new(Instant::now())),
            snapshot_manager: Arc::new(RwLock::new(snapshot_manager)),
            snapshot_receiver: Arc::new(RwLock::new(None)),
            persistence,
            config_state: Arc::new(RwLock::new(config_state)),
            stepping_down: Arc::new(RwLock::new(false)),
            fencing_token_state: Arc::new(FencingTokenState::new()),
            is_recovering,
        })
    }

    /// Create a new Raft node with an explicit persistence backend.
    ///
    /// Recovers state from the given persistence backend and uses it for all
    /// subsequent state and log mutations.
    pub fn with_persistence(
        config: RaftConfig,
        persistence: Arc<dyn RaftPersistence>,
    ) -> RaftResult<Self> {
        config
            .validate()
            .map_err(|msg| RaftError::ConfigError { message: msg })?;

        let snapshot_manager = if let Some(ref dir) = config.snapshot_dir {
            let snap_config =
                SnapshotConfig::new(dir.clone(), config.max_snapshots, config.snapshot_threshold);
            Some(SnapshotManager::new(snap_config)?)
        } else {
            None
        };

        let (term, voted_for) = persistence.load_state()?;
        let mut ps = PersistentState::new();
        ps.current_term = term;
        ps.voted_for = voted_for;

        let entries = persistence.load_log()?;
        let mut raft_log = RaftLog::new();
        if !entries.is_empty() {
            raft_log.append_entries(entries)?;
        }

        // Restore applied_index from persistence
        let applied_idx = persistence.load_applied_index()?;
        if applied_idx > 0 && applied_idx <= raft_log.last_index() {
            if let Err(e) = raft_log.set_commit_index(applied_idx) {
                warn!(applied_idx, error = ?e, "Failed to restore commit index from applied_index");
            } else if let Err(e) = raft_log.set_applied_index(applied_idx) {
                warn!(applied_idx, error = ?e, "Failed to restore applied index");
            }
        }

        info!(
            node_id = config.node_id,
            term = term,
            voted_for = ?voted_for,
            last_log_index = raft_log.last_index(),
            "Recovered state via explicit persistence"
        );

        // Replay WAL entries if wal_dir is configured.
        let is_recovering = Arc::new(AtomicBool::new(false));
        if let Some(ref wal_dir) = config.wal_dir {
            is_recovering.store(true, Ordering::Release);
            let result = replay_wal_into_log(wal_dir, &mut raft_log);
            is_recovering.store(false, Ordering::Release);
            result?;
        }

        let initial_members: Vec<(NodeId, String)> =
            config.peers.iter().map(|&id| (id, String::new())).collect();
        let config_state = ConfigState::new_stable(initial_members);

        Ok(Self {
            config: Arc::new(config),
            persistent: Arc::new(RwLock::new(ps)),
            volatile: Arc::new(RwLock::new(VolatileState::new())),
            log: Arc::new(RwLock::new(raft_log)),
            leader_state: Arc::new(RwLock::new(None)),
            candidate_state: Arc::new(RwLock::new(None)),
            last_heartbeat: Arc::new(RwLock::new(Instant::now())),
            snapshot_manager: Arc::new(RwLock::new(snapshot_manager)),
            snapshot_receiver: Arc::new(RwLock::new(None)),
            persistence: Some(persistence),
            config_state: Arc::new(RwLock::new(config_state)),
            stepping_down: Arc::new(RwLock::new(false)),
            fencing_token_state: Arc::new(FencingTokenState::new()),
            is_recovering,
        })
    }

    /// Persist current term and voted_for to the storage backend (no-op if
    /// persistence is not configured).
    fn persist_state(&self, term: Term, voted_for: Option<NodeId>) {
        if let Some(ref p) = self.persistence {
            if let Err(e) = p.save_state(term, voted_for) {
                warn!(node_id = self.node_id(), error = ?e, "Failed to persist state");
            }
        }
    }

    /// Persist log entries to the storage backend.
    fn persist_log_entries(&self, entries: &[LogEntry]) {
        if let Some(ref p) = self.persistence {
            if let Err(e) = p.append_entries(entries) {
                warn!(node_id = self.node_id(), error = ?e, "Failed to persist log entries");
            }
        }
    }

    /// Persist a log truncation to the storage backend.
    fn persist_log_truncation(&self, from_index: LogIndex) {
        if let Some(ref p) = self.persistence {
            if let Err(e) = p.truncate_log_from(from_index) {
                warn!(node_id = self.node_id(), error = ?e, "Failed to persist log truncation");
            }
        }
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
        let index = log.append(term, command.clone());

        // Persist the new entry
        let entry = LogEntry::new(term, index, command);
        self.persist_log_entries(&[entry]);

        info!(
            node_id = self.node_id(),
            index = index,
            term = term,
            "Proposed new entry"
        );

        Ok(index)
    }

    /// Return `true` if the node is currently replaying its WAL on startup.
    pub fn is_recovering(&self) -> bool {
        self.is_recovering.load(Ordering::Acquire)
    }

    /// Handle a RequestVote RPC
    pub fn handle_request_vote(&self, req: RequestVoteRequest) -> RequestVoteResponse {
        // Reject all RPCs during WAL replay to maintain safety.
        if self.is_recovering.load(Ordering::Acquire) {
            warn!(
                node_id = self.node_id(),
                candidate = req.candidate_id,
                event = "rpc_rejected_recovering",
                "Rejecting RequestVote: node is recovering from WAL"
            );
            return RequestVoteResponse::rejected(self.current_term());
        }

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
            let from_term = persistent.current_term;
            persistent.update_term(req.term);
            self.persist_state(persistent.current_term, persistent.voted_for);
            volatile.become_follower(None);
            *self.leader_state.write() = None;
            *self.candidate_state.write() = None;
            debug!(
                node_id = self.node_id(),
                from_term = from_term,
                to_term = persistent.current_term,
                "Stepped down to follower (higher term in RequestVote)"
            );
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
        self.persist_state(persistent.current_term, persistent.voted_for);
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
        // Reject all RPCs during WAL replay to maintain safety.
        if self.is_recovering.load(Ordering::Acquire) {
            warn!(
                node_id = self.node_id(),
                leader = req.leader_id,
                event = "rpc_rejected_recovering",
                "Rejecting AppendEntries: node is recovering from WAL"
            );
            return AppendEntriesResponse::rejected(self.current_term());
        }

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
            let from_term = persistent.current_term;
            persistent.update_term(req.term);
            self.persist_state(persistent.current_term, persistent.voted_for);
            volatile.become_follower(Some(req.leader_id));
            *self.leader_state.write() = None;
            *self.candidate_state.write() = None;
            debug!(
                node_id = self.node_id(),
                from_term = from_term,
                to_term = persistent.current_term,
                leader_id = req.leader_id,
                "Stepped down to follower (higher term in AppendEntries)"
            );
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
                self.persist_log_truncation(first_new_index);
            }

            // Persist before in-memory append
            self.persist_log_entries(&req.entries);

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
            let old_commit = log.commit_index();
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
                    old_commit_index = old_commit,
                    new_commit_index = new_commit,
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

        // Persist the new term and vote before responding
        self.persist_state(persistent.current_term, persistent.voted_for);

        // Transition to candidate
        volatile.become_candidate();

        // Initialize candidate state
        *self.candidate_state.write() = Some(CandidateState::new(self.node_id()));

        let term = persistent.current_term;
        let log = self.log.read();
        let last_log_index = log.last_index();
        let last_log_term = log.last_term();

        let _span =
            tracing::info_span!("raft_election", node_id = self.node_id(), term = term).entered();

        info!(
            node_id = self.node_id(),
            candidate_term = term,
            log_length = last_log_index,
            "Started election"
        );

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
                let from_term = persistent.current_term;
                persistent.update_term(resp.term);
                self.persist_state(persistent.current_term, persistent.voted_for);
                volatile.become_follower(None);
                *self.candidate_state.write() = None;
                debug!(
                    node_id = self.node_id(),
                    from_term = from_term,
                    to_term = persistent.current_term,
                    "Stepped down to follower (higher term in vote response)"
                );
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
            let votes = self
                .candidate_state
                .read()
                .as_ref()
                .map(|cs| cs.vote_count())
                .unwrap_or(0);
            info!(
                node_id = self.node_id(),
                term = self.current_term(),
                votes_received = votes,
                "Won election with quorum"
            );
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

        let persistent = self.persistent.read();
        let term = persistent.current_term;

        // Bump the packed fencing token to the new leader term (resets seq to 0).
        self.fencing_token_state.bump_term_token(term as u32);

        info!(
            node_id = self.node_id(),
            term,
            voted_for = ?persistent.voted_for,
            peer_count = self.config.peers.len(),
            "Became leader"
        );
    }

    /// Issue a new fencing token for the current leader term.
    ///
    /// Returns `None` if the node is not the current leader.
    pub fn issue_fencing_token(&self) -> Option<FencingToken> {
        if !self.volatile.read().is_leader() {
            return None;
        }
        Some(self.fencing_token_state.issue_token())
    }

    /// Validate that `token` is not stale relative to the current Raft term.
    ///
    /// Returns `Ok(())` if the token's term matches the current Raft term.
    /// Returns `Err(RaftError::StaleTerm)` if the token predates the current term.
    pub fn validate_fencing_token(&self, token: &FencingToken) -> RaftResult<()> {
        let current_term = self.current_term();
        if token.term() as u64 == current_term {
            Ok(())
        } else {
            Err(RaftError::StaleTerm {
                current: current_term,
                received: token.term() as u64,
            })
        }
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

                let prev_log_index = next_index.saturating_sub(1);
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

    /// Replicate log entries to all followers that need them.
    ///
    /// This is a convenience method that combines `create_replication_requests()`
    /// with the information the caller needs to actually send the RPCs.
    /// Returns a list of `(peer_id, request)` pairs. If a follower is fully
    /// caught up (its `next_index > last_log_index`), it is omitted -- use
    /// `create_heartbeats()` for idle keep-alive messages.
    ///
    /// Typical usage in a replication loop:
    /// ```rust,ignore
    /// let requests = leader.replicate_to_followers();
    /// for (peer, req) in requests {
    ///     let resp = rpc_send(peer, req);
    ///     leader.handle_replication_response(peer, resp)?;
    /// }
    /// ```
    pub fn replicate_to_followers(&self) -> Vec<(NodeId, AppendEntriesRequest)> {
        self.create_replication_requests()
    }

    /// Create an AppendEntries request for a specific follower.
    ///
    /// Returns `None` if this node is not the leader, if the follower is
    /// already fully caught up, or if leader state is unavailable.
    pub fn create_replication_request_for(&self, peer: NodeId) -> Option<AppendEntriesRequest> {
        let volatile = self.volatile.read();
        if !volatile.is_leader() {
            return None;
        }
        drop(volatile);

        let leader_state_guard = self.leader_state.read();
        let leader_state = leader_state_guard.as_ref()?;

        let term = self.current_term();
        let log = self.log.read();
        let leader_commit = log.commit_index();

        let next_index = leader_state.get_next_index(peer);

        if next_index > log.last_index() {
            // Peer is up-to-date; nothing to replicate
            return None;
        }

        let prev_log_index = next_index.saturating_sub(1);
        let prev_log_term = log.get_term(prev_log_index).unwrap_or(0);

        let entries = log.get_entries_from(next_index, self.config.max_entries_per_message);

        if entries.is_empty() {
            return None;
        }

        Some(AppendEntriesRequest::new(
            term,
            self.node_id(),
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit,
        ))
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
            let from_term = persistent.current_term;
            persistent.update_term(resp.term);
            self.persist_state(persistent.current_term, persistent.voted_for);
            volatile.become_follower(None);
            *self.leader_state.write() = None;
            info!(
                node_id = self.node_id(),
                from_term = from_term,
                to_term = persistent.current_term,
                "Stepped down: leader to follower (higher term in replication response)"
            );
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

            // Try to advance commit index, using joint-consensus-aware
            // calculation when a membership change is in progress.
            let config_state = self.config_state.read().clone();
            let new_commit = leader_state.calculate_commit_index_joint(
                self.node_id(),
                self.log.read().last_index(),
                &config_state,
            );

            let mut log = self.log.write();
            if new_commit > log.commit_index() {
                // Only commit entries from the current term (Raft safety)
                if let Some(term) = log.get_term(new_commit) {
                    if term == self.current_term() {
                        let old_commit = log.commit_index();
                        log.set_commit_index(new_commit)?;
                        info!(
                            node_id = self.node_id(),
                            old_commit_index = old_commit,
                            new_commit_index = new_commit,
                            "Advanced commit index"
                        );
                    }
                }
            }
        } else {
            // Replication failed -- use fast backup with conflict hints
            // when available, otherwise simple decrement.
            if resp.conflict_index.is_some() || resp.conflict_term.is_some() {
                leader_state.update_failure_with_hint(
                    from,
                    resp.conflict_index,
                    resp.conflict_term,
                    resp.last_log_index,
                );
            } else {
                leader_state.update_failure(from);
            }

            warn!(
                node_id = self.node_id(),
                peer = from,
                next_index = leader_state.get_next_index(from),
                conflict_index = ?resp.conflict_index,
                conflict_term = ?resp.conflict_term,
                "Replication failed, will retry with adjusted next_index"
            );
        }

        Ok(())
    }

    /// Attempt to create a snapshot if the log has grown past the threshold
    ///
    /// Call this after advancing the commit index. If a snapshot is created,
    /// the log is compacted up to the snapshot point.
    ///
    /// `state_machine_data` is the serialized state of the application state machine
    /// at the current applied index.
    pub fn maybe_create_snapshot(&self, state_machine_data: Vec<u8>) -> RaftResult<bool> {
        let mut snap_guard = self.snapshot_manager.write();
        let manager = match snap_guard.as_mut() {
            Some(m) => m,
            None => return Ok(false),
        };

        let log = self.log.read();
        let entries_since = log.entries_since_snapshot();

        if !manager.should_snapshot(entries_since) {
            return Ok(false);
        }

        let applied_index = log.applied_index();
        if applied_index == 0 {
            return Ok(false);
        }

        let applied_term = match log.get_term(applied_index) {
            Some(t) => t,
            None => {
                // The applied entry might already be compacted; use snapshot term
                let (snap_idx, snap_term) = log.get_snapshot_point();
                if applied_index == snap_idx {
                    snap_term
                } else {
                    return Err(RaftError::LogInconsistency {
                        reason: format!(
                            "Cannot determine term for applied index {}",
                            applied_index
                        ),
                    });
                }
            }
        };

        drop(log);

        manager.create_snapshot(state_machine_data, applied_index, applied_term)?;

        // Compact the log
        let mut log = self.log.write();
        log.compact_until(applied_index, applied_term)?;

        info!(
            node_id = self.node_id(),
            snapshot_index = applied_index,
            snapshot_term = applied_term,
            "Created snapshot and compacted log"
        );

        Ok(true)
    }

    /// Automatically create a snapshot if the log has grown past the configured threshold.
    ///
    /// Unlike `maybe_create_snapshot`, this method uses a `SnapshotPolicy` to decide
    /// whether to snapshot and takes a closure that produces state machine data on demand
    /// (avoiding the cost of serialization when no snapshot is needed).
    ///
    /// Call this after applying committed entries.
    pub fn auto_snapshot_if_needed<F>(
        &self,
        policy: &SnapshotPolicy,
        state_machine_data_fn: F,
    ) -> RaftResult<bool>
    where
        F: FnOnce() -> RaftResult<Vec<u8>>,
    {
        let log = self.log.read();
        let entries_since = log.entries_since_snapshot();
        let applied_index = log.applied_index();

        if !policy.should_snapshot(entries_since, applied_index) {
            return Ok(false);
        }

        if applied_index == 0 {
            return Ok(false);
        }

        let applied_term = match log.get_term(applied_index) {
            Some(t) => t,
            None => {
                let (snap_idx, snap_term) = log.get_snapshot_point();
                if applied_index == snap_idx {
                    snap_term
                } else {
                    return Err(RaftError::LogInconsistency {
                        reason: format!(
                            "Cannot determine term for applied index {}",
                            applied_index
                        ),
                    });
                }
            }
        };

        drop(log);

        let data = state_machine_data_fn()?;

        let mut snap_guard = self.snapshot_manager.write();
        let manager = match snap_guard.as_mut() {
            Some(m) => m,
            None => return Ok(false),
        };

        manager.create_snapshot(data, applied_index, applied_term)?;
        drop(snap_guard);

        let mut log = self.log.write();
        log.compact_until(applied_index, applied_term)?;

        info!(
            node_id = self.node_id(),
            snapshot_index = applied_index,
            snapshot_term = applied_term,
            entries_compacted = entries_since,
            "Auto-snapshot triggered and log compacted"
        );

        Ok(true)
    }

    /// Handle an InstallSnapshot RPC from the leader
    ///
    /// Used when a follower is too far behind and the leader sends a snapshot
    /// instead of individual log entries.
    pub fn handle_install_snapshot(
        &self,
        req: InstallSnapshotRequest,
    ) -> RaftResult<InstallSnapshotResponse> {
        let mut persistent = self.persistent.write();
        let mut volatile = self.volatile.write();

        debug!(
            node_id = self.node_id(),
            leader = req.leader_id,
            term = req.term,
            last_included_index = req.last_included_index,
            last_included_term = req.last_included_term,
            offset = req.offset,
            done = req.done,
            "Received InstallSnapshot"
        );

        // Update term if necessary
        if req.term > persistent.current_term {
            let from_term = persistent.current_term;
            persistent.update_term(req.term);
            self.persist_state(persistent.current_term, persistent.voted_for);
            volatile.become_follower(Some(req.leader_id));
            *self.leader_state.write() = None;
            *self.candidate_state.write() = None;
            debug!(
                node_id = self.node_id(),
                from_term = from_term,
                to_term = persistent.current_term,
                leader_id = req.leader_id,
                "Stepped down to follower (higher term in InstallSnapshot)"
            );
        }

        // Reject if term is stale
        if req.term < persistent.current_term {
            warn!(
                node_id = self.node_id(),
                leader = req.leader_id,
                current_term = persistent.current_term,
                request_term = req.term,
                "Rejecting InstallSnapshot: stale term"
            );
            return Ok(InstallSnapshotResponse::new(persistent.current_term));
        }

        // Update heartbeat and leader
        *self.last_heartbeat.write() = Instant::now();
        volatile.become_follower(Some(req.leader_id));
        *self.candidate_state.write() = None;

        let current_term = persistent.current_term;
        drop(persistent);
        drop(volatile);

        // Handle chunked snapshot transfer
        let mut receiver_guard = self.snapshot_receiver.write();

        // If this is a new snapshot transfer (offset 0), create a new receiver
        if req.offset == 0 {
            *receiver_guard = Some(SnapshotReceiver::new(
                req.last_included_index,
                req.last_included_term,
            ));
        }

        let receiver = match receiver_guard.as_mut() {
            Some(r) => r,
            None => {
                // No active receiver and offset != 0 - this is unexpected
                warn!(
                    node_id = self.node_id(),
                    offset = req.offset,
                    "Received non-initial snapshot chunk without active receiver"
                );
                return Ok(InstallSnapshotResponse::new(current_term));
            }
        };

        // Feed the chunk to the receiver
        let completed = receiver.receive_chunk(&req)?;

        if let Some(snapshot) = completed {
            // Clear receiver
            *receiver_guard = None;
            drop(receiver_guard);

            // Install the snapshot
            let mut snap_guard = self.snapshot_manager.write();
            if let Some(manager) = snap_guard.as_mut() {
                manager.install_snapshot(snapshot)?;
            }

            // Reset the log to match the snapshot
            let mut log = self.log.write();
            log.install_snapshot(req.last_included_index, req.last_included_term);

            info!(
                node_id = self.node_id(),
                last_included_index = req.last_included_index,
                last_included_term = req.last_included_term,
                "Installed snapshot from leader"
            );
        }

        Ok(InstallSnapshotResponse::new(current_term))
    }

    /// Prepare an InstallSnapshot request for a follower that is too far behind
    ///
    /// This is called by the leader when a follower's next_index falls behind
    /// the snapshot point and log entries are no longer available.
    pub fn prepare_install_snapshot(
        &self,
        target_peer: NodeId,
    ) -> RaftResult<Option<InstallSnapshotRequest>> {
        let volatile = self.volatile.read();
        if !volatile.is_leader() {
            return Ok(None);
        }
        drop(volatile);

        let snap_guard = self.snapshot_manager.read();
        let manager = match snap_guard.as_ref() {
            Some(m) => m,
            None => return Ok(None),
        };

        let snapshot = match manager.load_latest()? {
            Some(s) => s,
            None => return Ok(None),
        };

        // Check if the peer actually needs a snapshot
        let leader_state_guard = self.leader_state.read();
        if let Some(leader_state) = leader_state_guard.as_ref() {
            let next_index = leader_state.get_next_index(target_peer);
            let log = self.log.read();
            let (snap_idx, _) = log.get_snapshot_point();

            if next_index > snap_idx {
                // Peer doesn't need a snapshot, normal replication will work
                return Ok(None);
            }
        }

        let term = self.current_term();
        let req = InstallSnapshotRequest::new_complete(
            term,
            self.node_id(),
            snapshot.metadata.last_included_index,
            snapshot.metadata.last_included_term,
            snapshot.data,
        );

        Ok(Some(req))
    }

    /// Check if a follower needs a snapshot instead of normal log replication
    ///
    /// Returns true if the follower's next_index is at or before the snapshot point.
    pub fn follower_needs_snapshot(&self, peer: NodeId) -> bool {
        let leader_state_guard = self.leader_state.read();
        let leader_state = match leader_state_guard.as_ref() {
            Some(s) => s,
            None => return false,
        };

        let next_index = leader_state.get_next_index(peer);
        let log = self.log.read();
        let (snap_idx, _) = log.get_snapshot_point();

        snap_idx > 0 && next_index <= snap_idx
    }

    // ── Membership change (joint consensus) ──────────────────────────

    /// Add a node to the cluster via joint consensus.
    ///
    /// If this node is the leader the change is proposed immediately.
    /// Returns an error if a membership change is already in progress or
    /// the node is already a member.
    pub fn add_node(&self, node_id: NodeId, address: String) -> RaftResult<()> {
        self.propose_membership_change(MembershipChange::AddNode { node_id, address })
    }

    /// Remove a node from the cluster via joint consensus.
    ///
    /// If the removed node is this node, it will step down after the
    /// configuration change commits.
    pub fn remove_node(&self, node_id: NodeId) -> RaftResult<()> {
        self.propose_membership_change(MembershipChange::RemoveNode { node_id })
    }

    /// Get the current list of cluster members as `(node_id, address)` pairs.
    pub fn cluster_members(&self) -> Vec<(NodeId, String)> {
        self.config_state.read().all_members()
    }

    /// Check whether the cluster is currently in joint consensus.
    pub fn is_in_joint_consensus(&self) -> bool {
        self.config_state.read().is_joint()
    }

    /// Get the current membership configuration version.
    pub fn membership_version(&self) -> u64 {
        self.config_state.read().version()
    }

    /// Propose a membership change (leader only).
    ///
    /// Implements the simplified Raft joint consensus protocol (Section 6):
    /// 1. Leader creates joint config C_{old,new} and replicates it.
    /// 2. Once C_{old,new} is committed the leader creates C_{new}.
    /// 3. During the joint state, decisions require a majority of **both**
    ///    the old and new configurations.
    pub fn propose_membership_change(&self, change: MembershipChange) -> RaftResult<()> {
        // Must be leader
        let volatile = self.volatile.read();
        if !volatile.is_leader() {
            return Err(RaftError::NotLeader {
                leader_id: volatile.leader_id,
            });
        }
        drop(volatile);

        let mut cs = self.config_state.write();

        // Only one membership change at a time
        if cs.is_joint() {
            return Err(RaftError::MembershipChangeInProgress);
        }

        let current = match &*cs {
            ConfigState::Stable(c) => c.clone(),
            ConfigState::Joint { .. } => return Err(RaftError::MembershipChangeInProgress),
        };

        // Build the new config
        let new_config = match &change {
            MembershipChange::AddNode { node_id, address } => {
                if current.contains(*node_id) {
                    return Err(RaftError::NodeAlreadyMember { node_id: *node_id });
                }
                current.with_added_member(*node_id, address.clone())
            }
            MembershipChange::RemoveNode { node_id } => {
                if !current.contains(*node_id) {
                    return Err(RaftError::NodeNotMember { node_id: *node_id });
                }
                current.without_member(*node_id)
            }
        };

        // Enter joint consensus C_{old,new}
        *cs = ConfigState::Joint {
            old: current.clone(),
            new: new_config.clone(),
        };

        info!(
            node_id = self.node_id(),
            change = ?change,
            old_version = current.version(),
            new_version = new_config.version(),
            "Entered joint consensus"
        );

        // Update the peers list and leader state to include the union of
        // both configs so replication / heartbeats reach the new member.
        self.update_leader_state_for_config(&cs);

        // Append a joint-config marker to the log so followers replicate it.
        let term = self.current_term();
        let mut log = self.log.write();
        let _index = log.append(term, Command::from_str("__membership_joint__"));

        Ok(())
    }

    /// Commit the joint consensus transition: move from C_{old,new} to C_{new}.
    ///
    /// Call this once the joint config entry has been committed (i.e. acknowledged
    /// by a quorum of **both** old and new configs).
    pub fn commit_membership_change(&self) -> RaftResult<()> {
        let mut cs = self.config_state.write();
        let new_config = match &*cs {
            ConfigState::Joint { new, .. } => new.clone(),
            ConfigState::Stable(_) => {
                // Nothing to do -- already stable
                return Ok(());
            }
        };

        *cs = ConfigState::Stable(new_config.clone());

        info!(
            node_id = self.node_id(),
            version = new_config.version(),
            members = ?new_config.member_ids(),
            "Committed membership change, now stable"
        );

        // Update leader state to reflect the final config
        self.update_leader_state_for_config(&cs);

        // Append a stable-config marker so followers learn about it
        let term = self.current_term();
        let mut log = self.log.write();
        let _index = log.append(term, Command::from_str("__membership_stable__"));

        // If we (the leader) were removed, step down
        if !new_config.contains(self.node_id()) {
            drop(cs);
            drop(log);
            self.step_down();
        }

        Ok(())
    }

    /// Calculate whether a set of nodes forms a quorum under the current
    /// membership config (handles both stable and joint states).
    pub fn has_quorum(&self, responding_nodes: &HashSet<NodeId>) -> bool {
        self.config_state.read().has_quorum(responding_nodes)
    }

    /// Check if this node is stepping down (has been removed from the cluster).
    pub fn is_stepping_down(&self) -> bool {
        *self.stepping_down.read()
    }

    /// Gracefully step down: revert to follower and mark as stepping down.
    fn step_down(&self) {
        let mut volatile = self.volatile.write();
        volatile.become_follower(None);
        *self.leader_state.write() = None;
        *self.candidate_state.write() = None;
        *self.stepping_down.write() = true;

        info!(
            node_id = self.node_id(),
            "Stepping down -- removed from cluster"
        );
    }

    /// Synchronize leader replication state with the current config so
    /// new members receive heartbeats / log entries.
    fn update_leader_state_for_config(&self, cs: &ConfigState) {
        let mut ls_guard = self.leader_state.write();
        let ls = match ls_guard.as_mut() {
            Some(s) => s,
            None => return,
        };

        let all_ids = cs.all_member_ids();
        let log = self.log.read();
        let last_log_index = log.last_index();

        // Add entries for any new peers that are not yet tracked
        for &id in &all_ids {
            if id == self.node_id() {
                continue;
            }
            ls.next_index.entry(id).or_insert(last_log_index + 1);
            ls.match_index.entry(id).or_insert(0);
        }

        // Remove entries for peers that are no longer in any config
        ls.next_index
            .retain(|id, _| all_ids.contains(id) || *id == self.node_id());
        ls.match_index
            .retain(|id, _| all_ids.contains(id) || *id == self.node_id());
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

    /// Get a hint about the current leader (for client redirection)
    pub fn get_leader_hint(&self) -> Option<NodeId> {
        self.volatile.read().leader_id
    }

    /// Trigger a failover election if this node is a follower.
    /// Returns the vote requests to send to peers, or an empty vec
    /// if the node is not in follower state.
    pub fn trigger_failover_election(&self) -> Vec<RequestVoteRequest> {
        let state = self.volatile.read().node_state;
        if state != NodeState::Follower {
            return Vec::new();
        }
        self.start_election()
    }
}

// ---------------------------------------------------------------------------
// WAL replay helper
// ---------------------------------------------------------------------------

/// Replay WAL entries from `wal_dir` into `log`, merging with any entries
/// already present.
///
/// Strategy: WAL entries with indices greater than the current `log.last_index()`
/// are appended verbatim.  Entries at or below the current last index are
/// skipped (persistence already covers them, and WAL is treated as a
/// superset or equal set).  If the WAL has a higher-index entry that
/// conflicts in term, the WAL version wins (WAL is more recent).
///
/// Uses [`CorruptionPolicy::TruncateToLastGood`] for crash safety (partial
/// final entries are silently discarded).
fn replay_wal_into_log(wal_dir: &std::path::Path, log: &mut RaftLog) -> RaftResult<()> {
    let reader = WalReader::new(wal_dir);
    let (wal_entries, diag) = reader.recover_with_policy(CorruptionPolicy::TruncateToLastGood)?;

    if diag.corrupt_entries > 0 || diag.truncated_segments > 0 {
        warn!(
            corrupt_entries = diag.corrupt_entries,
            truncated_segments = diag.truncated_segments,
            valid_entries = diag.valid_entries,
            "WAL replay: corruption/truncation detected"
        );
    }

    if wal_entries.is_empty() {
        info!(wal_dir = %wal_dir.display(), "WAL replay: no entries to recover");
        return Ok(());
    }

    let current_last = log.last_index();
    let new_entries: Vec<LogEntry> = wal_entries
        .into_iter()
        .filter(|e| e.index > current_last)
        .collect();

    if new_entries.is_empty() {
        info!(
            wal_dir = %wal_dir.display(),
            current_last,
            "WAL replay: all WAL entries already present in log"
        );
        return Ok(());
    }

    let replayed_count = new_entries.len();
    let first_new = new_entries[0].index;
    let last_new = new_entries[new_entries.len() - 1].index;

    log.append_entries(new_entries)?;

    info!(
        wal_dir = %wal_dir.display(),
        replayed_count,
        first_new,
        last_new,
        new_last_index = log.last_index(),
        "WAL replay complete"
    );

    Ok(())
}

#[cfg(test)]
#[path = "node_tests.rs"]
mod tests;
