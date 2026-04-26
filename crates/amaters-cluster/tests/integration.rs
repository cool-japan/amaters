//! Integration tests for amaters-cluster Raft consensus
//!
//! Tests multi-node election, log replication, and term advancement scenarios.

use amaters_cluster::{
    AppendEntriesRequest, AppendEntriesResponse, Command, LogEntry, NodeState, RaftConfig,
    RaftNode, RequestVoteRequest, RequestVoteResponse,
};

/// Helper: create a 3-node cluster (node IDs 1, 2, 3)
fn create_three_node_cluster() -> (RaftNode, RaftNode, RaftNode) {
    let peers = vec![1, 2, 3];
    let n1 = RaftNode::new(RaftConfig::new(1, peers.clone())).expect("node 1 creation failed");
    let n2 = RaftNode::new(RaftConfig::new(2, peers.clone())).expect("node 2 creation failed");
    let n3 = RaftNode::new(RaftConfig::new(3, peers)).expect("node 3 creation failed");
    (n1, n2, n3)
}

/// Helper: make node become leader through election
fn elect_leader(leader: &RaftNode, voters: &[&RaftNode]) {
    let vote_requests = leader.start_election();
    assert!(
        !vote_requests.is_empty(),
        "start_election should produce vote requests"
    );

    // Each voter handles the vote request and returns a response
    for voter in voters {
        let req = RequestVoteRequest::new(
            leader.current_term(),
            leader.node_id(),
            leader.last_log_index(),
            0, // last_log_term = 0 for empty log
        );
        let resp = voter.handle_request_vote(req);

        if resp.vote_granted {
            let became_leader = leader.handle_vote_response(voter.node_id(), resp);
            if became_leader {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Election Tests
// ---------------------------------------------------------------------------

#[test]
fn test_three_node_election_produces_exactly_one_leader() {
    let (n1, n2, n3) = create_three_node_cluster();

    // All nodes start as followers
    assert_eq!(n1.state(), NodeState::Follower);
    assert_eq!(n2.state(), NodeState::Follower);
    assert_eq!(n3.state(), NodeState::Follower);

    // Node 1 starts election
    elect_leader(&n1, &[&n2, &n3]);

    // Exactly one leader
    assert_eq!(n1.state(), NodeState::Leader);
    assert_eq!(n1.current_term(), 1);

    // Others remain followers (they voted but did not become candidates)
    assert_eq!(n2.state(), NodeState::Follower);
    assert_eq!(n3.state(), NodeState::Follower);
}

#[test]
fn test_election_requires_quorum() {
    let (n1, _n2, _n3) = create_three_node_cluster();

    // Node 1 starts election but gets no votes from peers
    let _vote_requests = n1.start_election();
    assert_eq!(n1.state(), NodeState::Candidate);

    // Send a rejected vote -- node should remain candidate
    let rejected = RequestVoteResponse::rejected(1);
    let became_leader = n1.handle_vote_response(2, rejected);
    assert!(!became_leader);
    assert_eq!(n1.state(), NodeState::Candidate);
}

#[test]
fn test_election_with_five_nodes() {
    let peers = vec![1, 2, 3, 4, 5];
    let n1 = RaftNode::new(RaftConfig::new(1, peers.clone())).expect("n1");
    let n2 = RaftNode::new(RaftConfig::new(2, peers.clone())).expect("n2");
    let n3 = RaftNode::new(RaftConfig::new(3, peers.clone())).expect("n3");
    let n4 = RaftNode::new(RaftConfig::new(4, peers.clone())).expect("n4");
    let _n5 = RaftNode::new(RaftConfig::new(5, peers)).expect("n5");

    // With 5 nodes, quorum = 3 (self + 2 votes)
    let _vote_requests = n1.start_election();

    // Get votes from n2 and n3 (enough for quorum)
    let req = RequestVoteRequest::new(n1.current_term(), n1.node_id(), 0, 0);
    let resp2 = n2.handle_request_vote(req.clone());
    assert!(resp2.vote_granted);
    let became_leader = n1.handle_vote_response(2, resp2);
    // self + n2 = 2, not enough
    assert!(!became_leader);

    let req = RequestVoteRequest::new(n1.current_term(), n1.node_id(), 0, 0);
    let resp3 = n3.handle_request_vote(req);
    assert!(resp3.vote_granted);
    let became_leader = n1.handle_vote_response(3, resp3);
    // self + n2 + n3 = 3 = quorum
    assert!(became_leader);
    assert_eq!(n1.state(), NodeState::Leader);

    // n4 should NOT have voted yet, but the election is already won
    assert_eq!(n4.state(), NodeState::Follower);
}

// ---------------------------------------------------------------------------
// Log Replication Tests
// ---------------------------------------------------------------------------

#[test]
fn test_log_replication_via_append_entries() {
    let (n1, n2, _n3) = create_three_node_cluster();

    // Make n1 the leader
    elect_leader(&n1, &[&n2, &_n3]);
    assert_eq!(n1.state(), NodeState::Leader);

    // Propose entries on the leader
    let idx1 = n1
        .propose(Command::from_str("SET x 1"))
        .expect("propose 1 failed");
    let idx2 = n1
        .propose(Command::from_str("SET y 2"))
        .expect("propose 2 failed");

    assert_eq!(idx1, 1);
    assert_eq!(idx2, 2);

    // Create replication requests
    let repl_requests = n1.create_replication_requests();
    assert!(
        !repl_requests.is_empty(),
        "leader should create replication requests"
    );

    // Find the request destined for node 2
    let (_, req_for_n2) = repl_requests
        .iter()
        .find(|(peer, _)| *peer == 2)
        .expect("should have request for node 2");

    // Follower handles AppendEntries
    let resp = n2.handle_append_entries(req_for_n2.clone());
    assert!(resp.success, "follower should accept valid entries");
    assert_eq!(resp.last_log_index, 2);

    // Follower's log should now match leader's
    assert_eq!(n2.last_log_index(), n1.last_log_index());
}

#[test]
fn test_heartbeat_does_not_change_log() {
    let (n1, n2, n3) = create_three_node_cluster();
    elect_leader(&n1, &[&n2, &n3]);

    let heartbeats = n1.create_heartbeats();
    assert!(!heartbeats.is_empty(), "leader should send heartbeats");

    for (peer_id, hb) in &heartbeats {
        assert!(hb.is_heartbeat(), "heartbeat entries must be empty");

        let target = if *peer_id == 2 { &n2 } else { &n3 };
        let resp = target.handle_append_entries(hb.clone());
        assert!(resp.success, "heartbeat should be accepted by follower");
    }

    // Log should still be empty on all nodes
    assert_eq!(n1.last_log_index(), 0);
    assert_eq!(n2.last_log_index(), 0);
    assert_eq!(n3.last_log_index(), 0);
}

#[test]
fn test_propose_as_follower_fails() {
    let (n1, _n2, _n3) = create_three_node_cluster();
    assert_eq!(n1.state(), NodeState::Follower);

    let result = n1.propose(Command::from_str("SET x 1"));
    assert!(result.is_err(), "follower should reject proposals");
}

// ---------------------------------------------------------------------------
// Term Advancement Tests
// ---------------------------------------------------------------------------

#[test]
fn test_term_advancement_on_higher_term_vote_request() {
    let (n1, n2, _n3) = create_three_node_cluster();

    // n1 is at term 0
    assert_eq!(n1.current_term(), 0);

    // n2 starts election, advancing to term 1
    n2.start_election();
    assert_eq!(n2.current_term(), 1);

    // n1 receives a vote request from n2 at term 1
    let req = RequestVoteRequest::new(1, 2, 0, 0);
    let resp = n1.handle_request_vote(req);

    // n1 should update its term and grant the vote
    assert!(resp.vote_granted);
    assert_eq!(n1.current_term(), 1);
    assert_eq!(n1.state(), NodeState::Follower);
}

#[test]
fn test_leader_steps_down_on_higher_term() {
    let (n1, n2, n3) = create_three_node_cluster();

    // Make n1 leader at term 1
    elect_leader(&n1, &[&n2, &n3]);
    assert_eq!(n1.state(), NodeState::Leader);
    assert_eq!(n1.current_term(), 1);

    // Simulate n2 starting a new election at term 2
    // by sending an AppendEntries with higher term (as if n2 became leader at term 2)
    let higher_term_req = AppendEntriesRequest::heartbeat(2, 2, 0, 0, 0);
    let resp = n1.handle_append_entries(higher_term_req);

    // n1 should step down to follower and update its term
    assert!(resp.success);
    assert_eq!(n1.current_term(), 2);
    assert_eq!(n1.state(), NodeState::Follower);
}

#[test]
fn test_candidate_steps_down_on_higher_term_vote_response() {
    let (n1, _n2, _n3) = create_three_node_cluster();

    // n1 becomes candidate at term 1
    n1.start_election();
    assert_eq!(n1.state(), NodeState::Candidate);
    assert_eq!(n1.current_term(), 1);

    // Receive a vote response with a higher term (e.g., term 5)
    let resp = RequestVoteResponse::rejected(5);
    let became_leader = n1.handle_vote_response(2, resp);

    assert!(!became_leader);
    assert_eq!(n1.current_term(), 5);
    assert_eq!(n1.state(), NodeState::Follower);
}

#[test]
fn test_stale_vote_request_rejected() {
    let (n1, n2, _n3) = create_three_node_cluster();

    // Advance n1 to term 3 by starting elections
    n1.start_election(); // term 1
    // Simulate stepping down and starting again
    // We can just directly start another election
    n1.start_election(); // term 2
    n1.start_election(); // term 3
    assert_eq!(n1.current_term(), 3);

    // n2 sends a vote request at term 1 (stale)
    let stale_req = RequestVoteRequest::new(1, 2, 0, 0);
    let resp = n1.handle_request_vote(stale_req);

    assert!(!resp.vote_granted, "stale term vote should be rejected");
    assert_eq!(resp.term, 3, "response should contain current term");
}

#[test]
fn test_stale_append_entries_rejected() {
    let (n1, _n2, _n3) = create_three_node_cluster();

    // Advance n1 to term 2
    n1.start_election(); // term 1
    n1.start_election(); // term 2

    // Receive AppendEntries from stale term 1
    let stale_req = AppendEntriesRequest::heartbeat(1, 2, 0, 0, 0);
    let resp = n1.handle_append_entries(stale_req);

    assert!(!resp.success, "stale term AppendEntries should be rejected");
    assert_eq!(resp.term, 2);
}

// ---------------------------------------------------------------------------
// Replication Response Tests
// ---------------------------------------------------------------------------

#[test]
fn test_replication_response_updates_leader_state() {
    let (n1, n2, n3) = create_three_node_cluster();
    elect_leader(&n1, &[&n2, &n3]);

    // Propose an entry
    n1.propose(Command::from_str("SET a 1"))
        .expect("propose failed");

    // Get replication requests
    let repl = n1.create_replication_requests();
    let (_, req_for_n2) = repl.iter().find(|(p, _)| *p == 2).expect("request for n2");

    // Follower handles it
    let resp = n2.handle_append_entries(req_for_n2.clone());
    assert!(resp.success);

    // Leader processes the response
    n1.handle_replication_response(2, resp)
        .expect("handle response failed");

    // After getting responses from a quorum, commit index should advance
    // (self + n2 = 2 = quorum for 3 nodes)
    assert_eq!(n1.commit_index(), 1);
}

#[test]
fn test_leader_steps_down_on_higher_term_replication_response() {
    let (n1, n2, n3) = create_three_node_cluster();
    elect_leader(&n1, &[&n2, &n3]);
    assert_eq!(n1.state(), NodeState::Leader);

    // Simulate a replication response with a higher term
    let resp = AppendEntriesResponse::new(10, false, 0, None, None);
    n1.handle_replication_response(2, resp)
        .expect("handle response failed");

    assert_eq!(n1.state(), NodeState::Follower);
    assert_eq!(n1.current_term(), 10);
}

// ---------------------------------------------------------------------------
// Multi-round Election Tests
// ---------------------------------------------------------------------------

#[test]
fn test_successive_elections_increment_term() {
    let (n1, n2, n3) = create_three_node_cluster();

    // First election: n1 becomes leader at term 1
    elect_leader(&n1, &[&n2, &n3]);
    assert_eq!(n1.current_term(), 1);
    assert_eq!(n1.state(), NodeState::Leader);

    // n2 starts a new election at term 2
    // First, n2 needs to have term >= n1's term. It already has term 1 from voting.
    let _vote_requests = n2.start_election();
    assert_eq!(n2.current_term(), 2);
    assert_eq!(n2.state(), NodeState::Candidate);

    // n3 votes for n2
    let req = RequestVoteRequest::new(2, 2, 0, 0);
    let resp = n3.handle_request_vote(req);
    assert!(resp.vote_granted);
    let became_leader = n2.handle_vote_response(3, resp);
    assert!(became_leader);
    assert_eq!(n2.state(), NodeState::Leader);
    assert_eq!(n2.current_term(), 2);

    // When n1 receives a heartbeat from n2 at term 2, it steps down
    let hb = AppendEntriesRequest::heartbeat(2, 2, 0, 0, 0);
    let resp = n1.handle_append_entries(hb);
    assert!(resp.success);
    assert_eq!(n1.state(), NodeState::Follower);
    assert_eq!(n1.current_term(), 2);
}

#[test]
fn test_duplicate_vote_for_same_candidate() {
    let (n1, n2, _n3) = create_three_node_cluster();

    n1.start_election();

    // n2 votes for n1
    let req = RequestVoteRequest::new(1, 1, 0, 0);
    let resp1 = n2.handle_request_vote(req.clone());
    assert!(resp1.vote_granted);

    // n2 receives another vote request from n1 in the same term
    let resp2 = n2.handle_request_vote(req);
    // Should still grant because it already voted for this candidate
    assert!(resp2.vote_granted);
}

#[test]
fn test_vote_rejected_when_already_voted_for_different_candidate() {
    let (n1, n2, n3) = create_three_node_cluster();

    // n1 starts election at term 1
    n1.start_election();

    // n3 votes for n1
    let req_from_n1 = RequestVoteRequest::new(1, 1, 0, 0);
    let resp = n3.handle_request_vote(req_from_n1);
    assert!(resp.vote_granted);

    // n2 also starts election at term 1 (this would only happen with network delays)
    // But n3 already voted for n1 in term 1, so should reject n2
    let req_from_n2 = RequestVoteRequest::new(1, 2, 0, 0);
    let resp = n3.handle_request_vote(req_from_n2);
    assert!(
        !resp.vote_granted,
        "should reject vote for different candidate in same term"
    );
}
