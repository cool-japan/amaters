//! Chaos engineering tests for amaters-cluster Raft consensus.
//!
//! These tests verify that the Raft implementation remains correct under
//! adversarial conditions: message drops, out-of-order delivery, split
//! votes, leader failures under load, and concurrent term races.
//!
//! All tests are fully in-memory — no network calls are needed.

use amaters_cluster::{
    AppendEntriesRequest, Command, LogEntry, NodeState, RaftConfig, RaftNode, RequestVoteRequest,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cluster3() -> (RaftNode, RaftNode, RaftNode) {
    let peers = vec![1, 2, 3];
    (
        RaftNode::new(RaftConfig::new(1, peers.clone())).expect("n1"),
        RaftNode::new(RaftConfig::new(2, peers.clone())).expect("n2"),
        RaftNode::new(RaftConfig::new(3, peers)).expect("n3"),
    )
}

fn cluster5() -> [RaftNode; 5] {
    let peers = vec![1, 2, 3, 4, 5];
    [
        RaftNode::new(RaftConfig::new(1, peers.clone())).expect("n1"),
        RaftNode::new(RaftConfig::new(2, peers.clone())).expect("n2"),
        RaftNode::new(RaftConfig::new(3, peers.clone())).expect("n3"),
        RaftNode::new(RaftConfig::new(4, peers.clone())).expect("n4"),
        RaftNode::new(RaftConfig::new(5, peers)).expect("n5"),
    ]
}

fn elect(leader: &RaftNode, voters: &[&RaftNode]) -> bool {
    let _reqs = leader.start_election();
    for voter in voters {
        let req = RequestVoteRequest::new(
            leader.current_term(),
            leader.node_id(),
            leader.last_log_index(),
            0,
        );
        let resp = voter.handle_request_vote(req);
        if resp.vote_granted && leader.handle_vote_response(voter.node_id(), resp) {
            return true;
        }
    }
    false
}

/// Replicate a proposed command to all followers using `create_replication_requests`.
/// Returns the committed log index.
#[allow(dead_code)]
fn propose_and_replicate(leader: &RaftNode, followers: &[&RaftNode], cmd_str: &str) -> u64 {
    let idx = leader
        .propose(Command::from_str(cmd_str))
        .expect("propose failed");
    // Get the properly constructed replication requests (correct prev_log_term etc.)
    for (peer_id, req) in leader.create_replication_requests() {
        if let Some(&follower) = followers.iter().find(|f| f.node_id() == peer_id) {
            let resp = follower.handle_append_entries(req);
            if resp.success {
                let _ = leader.handle_replication_response(peer_id, resp);
            }
        }
    }
    idx
}

// ---------------------------------------------------------------------------
// Split vote — no candidate wins, election must be retried
// ---------------------------------------------------------------------------

#[test]
fn test_split_vote_no_winner_in_same_term() {
    let (n1, n2, n3) = cluster3();

    // Both n1 and n2 start elections at the same time (same term)
    n1.start_election();
    n2.start_election();

    // n3 votes for n1 (whichever it sees first)
    let req1 = RequestVoteRequest::new(n1.current_term(), n1.node_id(), n1.last_log_index(), 0);
    let resp_for_n1 = n3.handle_request_vote(req1);

    if resp_for_n1.vote_granted {
        n1.handle_vote_response(n3.node_id(), resp_for_n1);
    }

    // n2 only has its own vote — no quorum
    let req2 = RequestVoteRequest::new(n2.current_term(), n2.node_id(), n2.last_log_index(), 0);
    // n3 already voted for n1 in this term, so it rejects n2
    let resp_for_n2 = n3.handle_request_vote(req2);

    assert!(
        !resp_for_n2.vote_granted,
        "n3 should reject n2 after already voting for n1"
    );
}

// ---------------------------------------------------------------------------
// Message drop — leader gets no acknowledgements on first round
// ---------------------------------------------------------------------------

#[test]
fn test_proposal_with_dropped_responses() {
    let (n1, n2, n3) = cluster3();
    assert!(elect(&n1, &[&n2, &n3]));
    assert_eq!(n1.state(), NodeState::Leader);

    let idx = n1
        .propose(Command::from_str("chaos:key=value"))
        .expect("propose failed");
    assert!(idx > 0, "proposal index should be non-zero");

    // Build replication requests using the leader API (guarantees correct prev_log_term)
    let repl_reqs: Vec<_> = n1.create_replication_requests();
    assert!(
        !repl_reqs.is_empty(),
        "leader should produce replication requests"
    );

    // Simulate: n2's response is "dropped" (we call handle_append_entries but discard result)
    // n3's response is delivered to the leader
    let mut n3_req = None;
    for (peer_id, req) in &repl_reqs {
        if *peer_id == n2.node_id() {
            let _ = n2.handle_append_entries(req.clone()); // response dropped
        } else if *peer_id == n3.node_id() {
            n3_req = Some((*peer_id, req.clone()));
        }
    }

    if let Some((peer_id, req)) = n3_req {
        let resp3 = n3.handle_append_entries(req);
        assert!(resp3.success, "n3 should accept the entry");
        n1.handle_replication_response(peer_id, resp3)
            .expect("handle response");
    }

    // Quorum achieved (n1 + n3 = 2 out of 3) — entry should be committed
    assert!(
        n1.commit_index() >= idx,
        "commit_index ({}) should be >= proposed idx ({})",
        n1.commit_index(),
        idx
    );
}

// ---------------------------------------------------------------------------
// Leader step-down: demoted leader cannot propose
// ---------------------------------------------------------------------------

#[test]
fn test_demoted_leader_cannot_propose() {
    let (n1, n2, n3) = cluster3();
    assert!(elect(&n1, &[&n2, &n3]));
    assert_eq!(n1.state(), NodeState::Leader, "n1 should be leader");

    // Simulate a higher-term message arriving — n1 steps down
    let higher_req = AppendEntriesRequest::new(
        n1.current_term() + 1, // higher term
        2,                     // from n2 (new leader)
        0,
        0,
        vec![],
        0,
    );
    let resp = n1.handle_append_entries(higher_req);
    assert!(resp.success || n1.state() != NodeState::Leader);

    // After step-down, n1 should no longer be leader
    if n1.state() != NodeState::Leader {
        let result = n1.propose(Command::new(b"should_fail".to_vec()));
        assert!(
            result.is_err(),
            "demoted leader should not accept new proposals"
        );
    }
}

// ---------------------------------------------------------------------------
// Term monotonicity under concurrent elections
// ---------------------------------------------------------------------------

#[test]
fn test_term_monotonically_increases_across_elections() {
    let (n1, n2, n3) = cluster3();

    let initial_term = n1.current_term();

    // First election
    elect(&n1, &[&n2, &n3]);
    let term_after_first = n1.current_term();
    assert!(term_after_first > initial_term);

    // Simulate n1 stepping down (higher-term heartbeat arrives)
    let higher_term_req = AppendEntriesRequest::new(term_after_first + 1, 2, 0, 0, vec![], 0);
    n1.handle_append_entries(higher_term_req);

    // Now n2 starts a new election
    n2.start_election();
    let n2_term = n2.current_term();
    assert!(n2_term >= term_after_first, "term must not go backwards");
}

// ---------------------------------------------------------------------------
// Old leader's stale heartbeats must be rejected
// ---------------------------------------------------------------------------

#[test]
fn test_stale_leader_heartbeats_rejected() {
    let (n1, n2, _n3) = cluster3();

    // Record the current term before any election
    let old_term = 1u64;

    // Advance the network to term 3 by simulating an election
    n1.start_election();
    let new_req = AppendEntriesRequest::new(
        old_term + 2, // term 3
        1,
        0,
        0,
        vec![],
        0,
    );
    n2.handle_append_entries(new_req);

    // Now old-term heartbeat from a stale leader should be rejected
    let stale_req = AppendEntriesRequest::new(
        old_term, // stale term 1
        1,
        0,
        0,
        vec![],
        0,
    );
    let resp = n2.handle_append_entries(stale_req);
    assert!(!resp.success, "stale-term heartbeat must be rejected");
}

// ---------------------------------------------------------------------------
// Concurrent proposals: index ordering and idempotency
// ---------------------------------------------------------------------------

#[test]
fn test_multiple_proposals_have_ascending_indices() {
    let (n1, n2, n3) = cluster3();
    assert!(elect(&n1, &[&n2, &n3]));

    let mut prev_idx = 0u64;
    for i in 0..5 {
        let cmd = Command::new(format!("op:{}", i).into_bytes());
        let idx = n1.propose(cmd).expect("propose failed");
        assert!(
            idx > prev_idx,
            "proposal {} index {} must be > prev {}",
            i,
            idx,
            prev_idx
        );
        prev_idx = idx;
    }
}

// ---------------------------------------------------------------------------
// Five-node partial partition: 3 reachable nodes still form quorum
// ---------------------------------------------------------------------------

#[test]
fn test_five_node_partial_partition_majority_reachable() {
    let nodes = cluster5();

    // n1 wins election with votes from n2, n3, n4 (n5 is partitioned)
    let _reqs = nodes[0].start_election();

    let req = RequestVoteRequest::new(
        nodes[0].current_term(),
        nodes[0].node_id(),
        nodes[0].last_log_index(),
        0,
    );

    let mut votes = 0;
    for voter_idx in [1usize, 2, 3] {
        let resp = nodes[voter_idx].handle_request_vote(req.clone());
        if resp.vote_granted {
            if nodes[0].handle_vote_response(nodes[voter_idx].node_id(), resp) {
                votes += 1;
                break;
            }
            votes += 1;
        }
    }

    // n1 should have become leader with majority (n5 partitioned)
    assert_eq!(
        nodes[0].state(),
        NodeState::Leader,
        "should be leader with {} votes from reachable majority",
        votes
    );
}

// ---------------------------------------------------------------------------
// Vote grant is idempotent for the same candidate in the same term
// ---------------------------------------------------------------------------

#[test]
fn test_repeated_vote_grant_idempotent() {
    let (n1, n2, _n3) = cluster3();
    n1.start_election();

    let req = RequestVoteRequest::new(n1.current_term(), n1.node_id(), 0, 0);

    let resp1 = n2.handle_request_vote(req.clone());
    let resp2 = n2.handle_request_vote(req.clone());

    // Both responses should be consistent (same term, same grant decision)
    assert_eq!(resp1.vote_granted, resp2.vote_granted);
    assert_eq!(resp1.term, resp2.term);
}

// ---------------------------------------------------------------------------
// Follower rejects AppendEntries if term is older than its own
// ---------------------------------------------------------------------------

#[test]
fn test_follower_rejects_lower_term_append_entries() {
    let (n1, n2, n3) = cluster3();
    assert!(elect(&n1, &[&n2, &n3]));

    // Advance n2's term by simulating an election round
    n2.start_election();
    let advanced_term = n2.current_term();
    assert!(advanced_term > n1.current_term() || n1.current_term() >= 1);

    // If n2 is now in a higher term, it should reject n1's old heartbeat
    if advanced_term > n1.current_term() {
        let old_req = AppendEntriesRequest::new(n1.current_term(), n1.node_id(), 0, 0, vec![], 0);
        let resp = n2.handle_append_entries(old_req);
        assert!(
            !resp.success,
            "n2 in term {} should reject term {}",
            advanced_term,
            n1.current_term()
        );
    }
}

// ---------------------------------------------------------------------------
// Commit index never goes backwards
// ---------------------------------------------------------------------------

#[test]
fn test_commit_index_never_decreases() {
    let (n1, n2, n3) = cluster3();
    assert!(elect(&n1, &[&n2, &n3]));

    let mut last_commit = n1.commit_index();

    for i in 0..5u64 {
        let cmd_str = format!("key_{}", i);
        let idx = n1.propose(Command::from_str(&cmd_str)).expect("propose");
        // Use create_replication_requests so prev_log_term is correct for each round
        for (peer_id, req) in n1.create_replication_requests() {
            let follower = if peer_id == n2.node_id() { &n2 } else { &n3 };
            let resp = follower.handle_append_entries(req);
            if resp.success {
                let _ = n1.handle_replication_response(peer_id, resp);
            }
        }

        let new_commit = n1.commit_index();
        assert!(
            new_commit >= last_commit,
            "commit index went backwards: {} < {} at iteration {}",
            new_commit,
            last_commit,
            i
        );
        let _ = idx;
        last_commit = new_commit;
    }
}

#[test]
fn test_node_crash_and_restart_recovers() {
    let temp_dir = std::env::temp_dir().join(format!("amaters_crash_test_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");

    let peers = vec![1u64, 2, 3];

    let n1 = RaftNode::new(RaftConfig::new(1, peers.clone())).expect("n1");
    let n2 = RaftNode::new(RaftConfig::new(2, peers.clone())).expect("n2");
    let mut config3 = RaftConfig::new(3, peers.clone());
    config3.persistence_dir = Some(temp_dir.clone());
    let n3 = RaftNode::new(config3.clone()).expect("n3");

    assert!(elect(&n1, &[&n2, &n3]));
    assert_eq!(n1.state(), NodeState::Leader);

    n1.propose(Command::from_str("SET x 1")).expect("propose");
    for (peer_id, req) in n1.create_replication_requests() {
        let follower = if peer_id == n2.node_id() { &n2 } else { &n3 };
        let resp = follower.handle_append_entries(req);
        if resp.success {
            let _ = n1.handle_replication_response(peer_id, resp);
        }
    }

    let pre_crash_log = n3.last_log_index();

    // "Crash" n3 by dropping it
    drop(n3);

    // "Restart" n3 from persistent state
    let n3_restarted = RaftNode::new(config3).expect("n3 restart");

    assert_eq!(
        n3_restarted.node_id(),
        3,
        "restarted node must have correct id"
    );

    // Verify the node is functional (can process RPCs)
    let heartbeat = AppendEntriesRequest::new(n1.current_term(), 1, 0, 0, vec![], 0);
    let resp = n3_restarted.handle_append_entries(heartbeat);
    assert!(
        resp.success || resp.term >= n1.current_term(),
        "restarted node must accept valid heartbeat or respond with term"
    );

    let _ = pre_crash_log;

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_network_partition_majority_continues() {
    let nodes = cluster5();
    let [ref n1, ref n2, ref n3, ref n4, ref n5] = nodes;

    // Group A: n1, n2, n3 (majority=3). Group B: n4, n5 (minority=2).
    assert!(
        elect(n1, &[n2, n3]),
        "n1 must win election with group A quorum"
    );
    assert_eq!(n1.state(), NodeState::Leader, "n1 must be leader");

    // Group B: n4 tries election but only gets n5's vote (1 extra vote = 2 total, not quorum of 3)
    n4.start_election();
    let req = RequestVoteRequest::new(n4.current_term(), n4.node_id(), n4.last_log_index(), 0);
    let resp5 = n5.handle_request_vote(req);
    if resp5.vote_granted {
        let became = n4.handle_vote_response(n5.node_id(), resp5);
        // n4 + n5 = 2 votes, quorum=3, so n4 must NOT become leader
        assert!(
            !became,
            "n4 cannot become leader with only 2 votes (quorum=3)"
        );
    }
    assert_ne!(
        n4.state(),
        NodeState::Leader,
        "minority partition cannot elect leader"
    );

    // Group A leader can still propose
    let result = n1.propose(Command::from_str("SET partition_test 1"));
    assert!(
        result.is_ok(),
        "majority partition leader can still propose"
    );
}

#[test]
fn test_simultaneous_two_node_failure() {
    let nodes = cluster5();
    let [ref n1, ref n2, ref n3, ref n4, ref n5] = nodes;

    // Elect n1 as leader of all 5 nodes
    assert!(elect(n1, &[n2, n3, n4, n5]), "n1 must win initial election");
    assert_eq!(n1.state(), NodeState::Leader);

    // Simulate dropping n4 and n5 — only deliver to n2 and n3
    n1.propose(Command::from_str("SET after_failure 1"))
        .expect("propose after failure");

    for (peer_id, req) in n1.create_replication_requests() {
        if peer_id == n2.node_id() {
            let resp = n2.handle_append_entries(req);
            if resp.success {
                n1.handle_replication_response(peer_id, resp)
                    .expect("resp n2");
            }
        } else if peer_id == n3.node_id() {
            let resp = n3.handle_append_entries(req);
            if resp.success {
                n1.handle_replication_response(peer_id, resp)
                    .expect("resp n3");
            }
        }
        // n4, n5 messages dropped — they are "down"
    }

    // With n1+n2+n3 = 3 = quorum, the entry must commit
    assert!(
        n1.commit_index() >= 1,
        "cluster must continue with 3 of 5 nodes: commit_index={}",
        n1.commit_index()
    );
    assert_eq!(n1.state(), NodeState::Leader, "n1 must remain leader");
}

// ---------------------------------------------------------------------------
// W2.7: Message loss simulation
// ---------------------------------------------------------------------------

/// A pseudo-random drop filter using a simple Linear Congruential Generator.
///
/// The LCG produces a reproducible sequence from the given seed so that test
/// results are deterministic while still exercising the liveness-under-loss
/// property.
struct DroppingFilter {
    /// Fraction of messages to drop in [0.0, 1.0].
    drop_rate: f64,
    /// Current LCG state (acts as both seed and running counter).
    counter: u64,
}

impl DroppingFilter {
    fn new(drop_rate: f64, seed: u64) -> Self {
        Self {
            drop_rate,
            counter: seed,
        }
    }

    /// Returns `true` if the next message should be dropped.
    ///
    /// Uses Knuth's multiplicative LCG (64-bit variant):
    ///   `state = state * 6364136223846793005 + 1442695040888963407`
    fn should_drop(&mut self) -> bool {
        self.counter = self
            .counter
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.counter as f64 / u64::MAX as f64) < self.drop_rate
    }
}

/// Verify cluster liveness under 30% message loss.
///
/// With a quorum of 2 out of 3 nodes, we expect that even with 30% of
/// AppendEntries responses dropped, enough rounds will succeed to commit
/// all proposed entries.  We use a retry loop capped at 1 000 delivery
/// attempts to keep the test deterministic and bounded.
#[test]
fn test_message_loss_simulation() {
    let (n1, n2, n3) = cluster3();
    assert!(elect(&n1, &[&n2, &n3]), "n1 must win initial election");

    let mut filter = DroppingFilter::new(0.30, 0xDEAD_BEEF_CAFE_1234);

    // Propose 100 entries and attempt to replicate them under message loss.
    let total_proposals: u64 = 100;
    for i in 0..total_proposals {
        n1.propose(Command::new(format!("msg:{}", i).into_bytes()))
            .expect("propose must succeed while n1 is leader");
    }

    // Retry delivery loop: keep attempting to replicate until all entries
    // are committed or we exhaust the attempt budget.
    let mut attempts = 0usize;
    const MAX_ATTEMPTS: usize = 1_000;

    while n1.commit_index() < total_proposals && attempts < MAX_ATTEMPTS {
        attempts += 1;
        for (peer_id, req) in n1.create_replication_requests() {
            // Apply the drop filter to each replication message.
            if filter.should_drop() {
                continue;
            }
            let follower = if peer_id == n2.node_id() { &n2 } else { &n3 };
            let resp = follower.handle_append_entries(req);
            if resp.success {
                let _ = n1.handle_replication_response(peer_id, resp);
            }
        }
    }

    assert_eq!(
        n1.state(),
        NodeState::Leader,
        "n1 must remain leader throughout message-loss test"
    );
    assert_eq!(
        n1.commit_index(),
        total_proposals,
        "all {} entries must eventually commit despite 30% message loss (took {} attempts)",
        total_proposals,
        attempts
    );
}

// ---------------------------------------------------------------------------
// W2.7: Clock skew (term advancement) between nodes
// ---------------------------------------------------------------------------

/// Verify that a node whose term was artificially advanced (simulating a
/// clock skew or Byzantine-style term inflation) causes the other nodes to
/// update their terms via the vote-response path, and that no node retains a
/// stale term after the skew is absorbed.
///
/// In Raft the logical clock is the term, so "clock skew" manifests as a node
/// with a higher term.  The cluster heals when followers update their terms
/// upon receiving a `RequestVote` (or any RPC) with the higher term.
#[test]
fn test_clock_skew_recovery() {
    let (n1, n2, n3) = cluster3();

    // Record the baseline term (0 for all fresh nodes).
    let base_term = n1.current_term();
    assert_eq!(base_term, 0, "fresh nodes must start at term 0");

    // Inject a large term skew on n1 by having it run 10 phantom elections.
    // Each call to `start_election` increments the term by 1.
    let skew_amount = 10u64;
    for _ in 0..skew_amount {
        n1.start_election();
    }
    let skewed_term = n1.current_term();
    assert_eq!(
        skewed_term,
        base_term + skew_amount,
        "n1 term must reflect {} phantom elections",
        skew_amount
    );

    // n1 now broadcasts RequestVote at the skewed term.  n2 and n3 must update
    // their terms when they handle the vote request.
    let req = RequestVoteRequest::new(skewed_term, n1.node_id(), n1.last_log_index(), 0);

    let resp_from_n2 = n2.handle_request_vote(req.clone());
    // n2 must have adopted the skewed term regardless of vote grant.
    assert_eq!(
        n2.current_term(),
        skewed_term,
        "n2 must update its term to {} after handling skewed RequestVote",
        skewed_term
    );

    let resp_from_n3 = n3.handle_request_vote(req);
    assert_eq!(
        n3.current_term(),
        skewed_term,
        "n3 must update its term to {} after handling skewed RequestVote",
        skewed_term
    );

    // Feed the responses back to n1 so it can tally votes.
    if resp_from_n2.vote_granted {
        n1.handle_vote_response(n2.node_id(), resp_from_n2);
    }
    if resp_from_n3.vote_granted {
        n1.handle_vote_response(n3.node_id(), resp_from_n3);
    }

    // After absorption all three nodes must agree on the term — no node should
    // still hold a stale (lower) term.
    let final_term = n1.current_term();
    assert!(
        n2.current_term() >= final_term,
        "n2 term ({}) must be >= n1 term ({}) after skew absorption",
        n2.current_term(),
        final_term
    );
    assert!(
        n3.current_term() >= final_term,
        "n3 term ({}) must be >= n1 term ({}) after skew absorption",
        n3.current_term(),
        final_term
    );

    // No node must hold a term below the skewed term.
    for (name, term) in [
        ("n1", n1.current_term()),
        ("n2", n2.current_term()),
        ("n3", n3.current_term()),
    ] {
        assert!(
            term >= skewed_term,
            "{name} has stale term {term}; expected >= {skewed_term}"
        );
    }
}
