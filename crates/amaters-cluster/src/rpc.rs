//! RPC message types for Raft consensus

use crate::log::LogEntry;
use crate::types::{FencingToken, LogIndex, NodeId, Term};

/// Request for a vote from a candidate
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestVoteRequest {
    /// Candidate's term
    pub term: Term,
    /// Candidate requesting vote
    pub candidate_id: NodeId,
    /// Index of candidate's last log entry
    pub last_log_index: LogIndex,
    /// Term of candidate's last log entry
    pub last_log_term: Term,
}

impl RequestVoteRequest {
    /// Create a new vote request
    pub fn new(
        term: Term,
        candidate_id: NodeId,
        last_log_index: LogIndex,
        last_log_term: Term,
    ) -> Self {
        Self {
            term,
            candidate_id,
            last_log_index,
            last_log_term,
        }
    }
}

/// Response to a vote request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestVoteResponse {
    /// Current term, for candidate to update itself
    pub term: Term,
    /// True if candidate received vote
    pub vote_granted: bool,
    /// Hint about who the current leader is (for client redirection)
    pub leader_hint: Option<NodeId>,
}

impl RequestVoteResponse {
    /// Create a new vote response
    pub fn new(term: Term, vote_granted: bool) -> Self {
        Self {
            term,
            vote_granted,
            leader_hint: None,
        }
    }

    /// Create a new vote response with a leader hint
    pub fn with_leader_hint(term: Term, vote_granted: bool, leader_hint: Option<NodeId>) -> Self {
        Self {
            term,
            vote_granted,
            leader_hint,
        }
    }

    /// Create a granted response
    pub fn granted(term: Term) -> Self {
        Self::new(term, true)
    }

    /// Create a rejected response
    pub fn rejected(term: Term) -> Self {
        Self::new(term, false)
    }
}

/// Request to append entries (heartbeat or log replication)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendEntriesRequest {
    /// Leader's term
    pub term: Term,
    /// Leader's ID (for redirecting clients)
    pub leader_id: NodeId,
    /// Index of log entry immediately preceding new ones
    pub prev_log_index: LogIndex,
    /// Term of prev_log_index entry
    pub prev_log_term: Term,
    /// Log entries to store (empty for heartbeat)
    pub entries: Vec<LogEntry>,
    /// Leader's commit index
    pub leader_commit: LogIndex,
    /// Optional fencing token issued by the leader (None for legacy / non-leaders)
    pub fencing_token: Option<FencingToken>,
}

impl AppendEntriesRequest {
    /// Create a new append entries request
    pub fn new(
        term: Term,
        leader_id: NodeId,
        prev_log_index: LogIndex,
        prev_log_term: Term,
        entries: Vec<LogEntry>,
        leader_commit: LogIndex,
    ) -> Self {
        Self {
            term,
            leader_id,
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit,
            fencing_token: None,
        }
    }

    /// Create an append entries request with a fencing token attached
    pub fn with_fencing_token(
        term: Term,
        leader_id: NodeId,
        prev_log_index: LogIndex,
        prev_log_term: Term,
        entries: Vec<LogEntry>,
        leader_commit: LogIndex,
        token: FencingToken,
    ) -> Self {
        Self {
            term,
            leader_id,
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit,
            fencing_token: Some(token),
        }
    }

    /// Create a heartbeat (empty entries)
    pub fn heartbeat(
        term: Term,
        leader_id: NodeId,
        prev_log_index: LogIndex,
        prev_log_term: Term,
        leader_commit: LogIndex,
    ) -> Self {
        Self::new(
            term,
            leader_id,
            prev_log_index,
            prev_log_term,
            Vec::new(),
            leader_commit,
        )
    }

    /// Check if this is a heartbeat (no entries)
    pub fn is_heartbeat(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Response to an append entries request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendEntriesResponse {
    /// Current term, for leader to update itself
    pub term: Term,
    /// True if follower contained entry matching prev_log_index and prev_log_term
    pub success: bool,
    /// Follower's last log index (for optimization)
    pub last_log_index: LogIndex,
    /// Conflict index (for fast log backtracking)
    pub conflict_index: Option<LogIndex>,
    /// Conflict term (for fast log backtracking)
    pub conflict_term: Option<Term>,
    /// Hint about who the current leader is (for client redirection)
    pub leader_hint: Option<NodeId>,
    /// Optional fencing token echoed back from the follower
    pub fencing_token: Option<FencingToken>,
}

impl AppendEntriesResponse {
    /// Create a new append entries response
    pub fn new(
        term: Term,
        success: bool,
        last_log_index: LogIndex,
        conflict_index: Option<LogIndex>,
        conflict_term: Option<Term>,
    ) -> Self {
        Self {
            term,
            success,
            last_log_index,
            conflict_index,
            conflict_term,
            leader_hint: None,
            fencing_token: None,
        }
    }

    /// Create a success response with a fencing token echoed
    pub fn success_with_token(term: Term, last_log_index: LogIndex, token: FencingToken) -> Self {
        Self {
            term,
            success: true,
            last_log_index,
            conflict_index: None,
            conflict_term: None,
            leader_hint: None,
            fencing_token: Some(token),
        }
    }

    /// Create a success response
    pub fn success(term: Term, last_log_index: LogIndex) -> Self {
        Self::new(term, true, last_log_index, None, None)
    }

    /// Create a failure response
    pub fn failure(
        term: Term,
        last_log_index: LogIndex,
        conflict_index: LogIndex,
        conflict_term: Term,
    ) -> Self {
        Self::new(
            term,
            false,
            last_log_index,
            Some(conflict_index),
            Some(conflict_term),
        )
    }

    /// Create a rejected response (generic failure)
    pub fn rejected(term: Term) -> Self {
        Self::new(term, false, 0, None, None)
    }

    /// Attach a leader hint to this response.
    pub fn with_leader_hint(mut self, leader_hint: Option<NodeId>) -> Self {
        self.leader_hint = leader_hint;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Command;

    #[test]
    fn test_request_vote_request() {
        let req = RequestVoteRequest::new(5, 1, 10, 3);
        assert_eq!(req.term, 5);
        assert_eq!(req.candidate_id, 1);
        assert_eq!(req.last_log_index, 10);
        assert_eq!(req.last_log_term, 3);
    }

    #[test]
    fn test_request_vote_response() {
        let resp = RequestVoteResponse::granted(5);
        assert_eq!(resp.term, 5);
        assert!(resp.vote_granted);

        let resp = RequestVoteResponse::rejected(6);
        assert_eq!(resp.term, 6);
        assert!(!resp.vote_granted);
    }

    #[test]
    fn test_append_entries_request_heartbeat() {
        let req = AppendEntriesRequest::heartbeat(5, 1, 10, 3, 8);
        assert_eq!(req.term, 5);
        assert_eq!(req.leader_id, 1);
        assert_eq!(req.prev_log_index, 10);
        assert_eq!(req.prev_log_term, 3);
        assert!(req.entries.is_empty());
        assert_eq!(req.leader_commit, 8);
        assert!(req.is_heartbeat());
    }

    #[test]
    fn test_append_entries_request_with_entries() {
        let entry = LogEntry::new(5, 11, Command::from_str("test"));
        let req = AppendEntriesRequest::new(5, 1, 10, 3, vec![entry], 8);
        assert!(!req.is_heartbeat());
        assert_eq!(req.entries.len(), 1);
    }

    #[test]
    fn test_append_entries_response_success() {
        let resp = AppendEntriesResponse::success(5, 11);
        assert_eq!(resp.term, 5);
        assert!(resp.success);
        assert_eq!(resp.last_log_index, 11);
        assert!(resp.conflict_index.is_none());
        assert!(resp.conflict_term.is_none());
    }

    #[test]
    fn test_append_entries_response_failure() {
        let resp = AppendEntriesResponse::failure(5, 9, 8, 2);
        assert_eq!(resp.term, 5);
        assert!(!resp.success);
        assert_eq!(resp.last_log_index, 9);
        assert_eq!(resp.conflict_index, Some(8));
        assert_eq!(resp.conflict_term, Some(2));
    }

    #[test]
    fn test_request_vote_response_with_leader_hint() {
        let resp = RequestVoteResponse::with_leader_hint(5, false, Some(3));
        assert_eq!(resp.term, 5);
        assert!(!resp.vote_granted);
        assert_eq!(resp.leader_hint, Some(3));
    }

    #[test]
    fn test_append_entries_response_with_leader_hint() {
        let resp = AppendEntriesResponse::success(5, 11).with_leader_hint(Some(2));
        assert_eq!(resp.leader_hint, Some(2));
        assert!(resp.success);
    }

    #[test]
    fn test_leader_hint_none_by_default() {
        let resp = AppendEntriesResponse::success(5, 11);
        assert_eq!(resp.leader_hint, None);

        let vote_resp = RequestVoteResponse::granted(5);
        assert_eq!(vote_resp.leader_hint, None);
    }
}
