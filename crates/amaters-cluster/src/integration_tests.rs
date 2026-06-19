//! Cross-crate integration tests for the amaters-cluster layer.
//!
//! These tests exercise the cluster layer end-to-end — leadership, log
//! replication, snapshot round-trips, membership changes, fencing tokens,
//! the `PlacementScheduler` lifecycle, `ClusterCommand` serialisation, shard
//! routing, placement determinism and chunked snapshot streaming.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tempfile::TempDir;

use crate::{
    cluster_command::ClusterCommand,
    error::RaftError,
    log::Command,
    node::RaftNode,
    partitioner::{PartitionStrategy, Partitioner},
    placement::{PlacementCoordinator, PlacementPolicy},
    placement_scheduler::{PlacementScheduler, PlacementSchedulerConfig},
    rpc::{AppendEntriesRequest, RequestVoteResponse},
    shard::{KeyRange, ShardMetadata, ShardRegistry},
    snapshot::InstallSnapshotRequest,
    types::{NodeId, NodeState, RaftConfig},
};
use amaters_core::Key;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a minimal follower node with 3 peers.
fn make_follower(node_id: NodeId) -> RaftNode {
    let config = RaftConfig::new(node_id, vec![1, 2, 3]);
    RaftNode::new(config).expect("RaftNode::new must succeed for valid config")
}

/// Build a node that becomes leader immediately after construction.
///
/// Uses a 3-node config so quorum = 2.  After `start_election` the node has
/// 1 vote (self); granting one additional vote makes it the leader.
fn make_leader(node_id: NodeId) -> RaftNode {
    let node = make_follower(node_id);
    node.start_election();
    let resp = RequestVoteResponse::granted(node.current_term());
    let became_leader = node.handle_vote_response(2, resp);
    assert!(
        became_leader,
        "node {} must become leader after receiving a quorum vote",
        node_id
    );
    node
}

/// Create a leader node backed by a temporary snapshot directory.
///
/// The snapshot threshold is set to `5` so tests can trigger compaction by
/// proposing six entries and advancing `applied_index`.
fn make_leader_with_snapshot_dir() -> (RaftNode, TempDir) {
    let dir = TempDir::new().expect("create tempdir for snapshot");
    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.snapshot_dir = Some(dir.path().to_path_buf());
    config.snapshot_threshold = 5;
    let node = RaftNode::new(config).expect("create snapshot-aware node");
    node.start_election();
    let resp = RequestVoteResponse::granted(node.current_term());
    let became_leader = node.handle_vote_response(2, resp);
    assert!(became_leader, "node must become leader");
    (node, dir)
}

/// Build an empty shard registry wrapped in an Arc<RwLock>.
fn make_registry() -> Arc<RwLock<ShardRegistry>> {
    Arc::new(RwLock::new(ShardRegistry::new()))
}

// ── 1. Leader election ────────────────────────────────────────────────────────

#[test]
fn test_leader_election_single_node_becomes_candidate() {
    let node = make_follower(1);
    assert_eq!(
        node.state(),
        NodeState::Follower,
        "fresh node must start as follower"
    );
    let vote_reqs = node.start_election();
    assert_eq!(
        node.state(),
        NodeState::Candidate,
        "after start_election, node must be candidate"
    );
    assert_eq!(
        vote_reqs.len(),
        2,
        "a 3-node cluster must produce 2 vote requests (peers excl. self)"
    );
    assert_eq!(
        node.current_term(),
        1,
        "term must increment on election start"
    );
}

#[test]
fn test_leader_election_wins_on_quorum() {
    let node = make_follower(1);
    node.start_election();
    let term = node.current_term();

    // One granted vote is enough for quorum (self + 1 = 2 out of 3).
    let became_leader = node.handle_vote_response(2, RequestVoteResponse::granted(term));
    assert!(became_leader, "node must become leader after quorum vote");
    assert_eq!(
        node.state(),
        NodeState::Leader,
        "state must be Leader after winning election"
    );
    assert_eq!(
        node.current_term(),
        1,
        "term must not change after winning election"
    );
}

#[test]
fn test_leader_election_fails_without_quorum() {
    let node = make_follower(1);
    node.start_election();
    let term = node.current_term();

    // A denied vote does not make the node the leader.
    let became_leader = node.handle_vote_response(2, RequestVoteResponse::new(term, false));
    assert!(
        !became_leader,
        "node must NOT become leader on a denied vote"
    );
    assert_eq!(
        node.state(),
        NodeState::Candidate,
        "state must remain Candidate"
    );
}

// ── 2. Log replication ────────────────────────────────────────────────────────

#[test]
fn test_log_replication_leader_proposes_commit_advances() {
    let leader = make_leader(1);
    let initial_commit = leader.commit_index();

    let idx = leader
        .propose(Command::from_str("SET k v"))
        .expect("leader must accept proposals");
    assert_eq!(
        idx,
        initial_commit + 1,
        "proposed entry must be at commit+1"
    );

    // Simulate a follower ACK-ing the entry.
    let follower = make_follower(2);
    let ae_req = leader
        .create_replication_request_for(2)
        .expect("leader must produce replication request for peer 2");

    let ae_resp = follower.handle_append_entries(ae_req);
    assert!(ae_resp.success, "follower must accept valid append entries");

    // Feed the ACK back to the leader so it can advance commit_index.
    leader
        .handle_replication_response(2, ae_resp)
        .expect("leader must process replication response without error");

    assert!(
        leader.commit_index() >= idx,
        "commit_index must advance after receiving a quorum ACK"
    );
}

#[test]
fn test_follower_rejects_proposal() {
    let follower = make_follower(2);
    let result = follower.propose(Command::from_str("SET k v"));
    assert!(
        matches!(result, Err(RaftError::NotLeader { .. })),
        "follower must return NotLeader on propose, got {:?}",
        result
    );
}

#[test]
fn test_log_replication_follower_append_entries_updates_log() {
    let leader = make_leader(1);
    leader
        .propose(Command::from_str("cmd1"))
        .expect("propose cmd1");
    leader
        .propose(Command::from_str("cmd2"))
        .expect("propose cmd2");

    let follower = make_follower(2);
    let ae_req = leader
        .create_replication_request_for(2)
        .expect("replication request for peer 2");

    let follower_last_before = follower.last_log_index();
    let ae_resp = follower.handle_append_entries(ae_req);
    assert!(ae_resp.success, "follower must accept the entries");
    assert!(
        follower.last_log_index() > follower_last_before,
        "follower log must grow after AppendEntries"
    );
}

// ── 3. Snapshot round-trip ────────────────────────────────────────────────────

#[test]
fn test_snapshot_round_trip_install_on_follower() {
    let (leader, _dir) = make_leader_with_snapshot_dir();

    // Propose enough entries to exceed the threshold (5) and trigger a snapshot.
    for i in 0..6_u32 {
        leader
            .propose(Command::from_str(&format!("cmd{}", i)))
            .expect("propose");
    }
    {
        let mut log = leader.log.write();
        log.set_commit_index(6).expect("set commit");
        log.set_applied_index(6).expect("set applied");
    }
    leader
        .maybe_create_snapshot(b"state_v1".to_vec())
        .expect("create snapshot");

    // Simulate peer 2 lagging behind the snapshot point.
    {
        let mut ls_guard = leader.leader_state.write();
        let ls = ls_guard.as_mut().expect("leader state");
        ls.next_index.insert(2, 1);
    }

    let install_req = leader
        .prepare_install_snapshot(2)
        .expect("prepare_install_snapshot must succeed")
        .expect("must produce a request for a lagging peer");

    // The snapshot is small — must be single-shot.
    assert!(install_req.done, "small snapshot must be single-shot");
    assert_eq!(
        install_req.last_included_index, 6,
        "snapshot must cover index 6"
    );

    // Install on a fresh follower.
    let follower_dir = TempDir::new().expect("follower tempdir");
    let mut follower_config = RaftConfig::new(2, vec![1, 2, 3]);
    follower_config.snapshot_dir = Some(follower_dir.path().to_path_buf());
    let follower = RaftNode::new(follower_config).expect("follower node");

    let install_resp = follower
        .handle_install_snapshot(install_req)
        .expect("follower must accept snapshot");
    // Follower accepted the snapshot — term in response must be non-zero.
    assert_ne!(
        install_resp.term, 0,
        "response term must reflect the snapshot term"
    );

    // After installing the snapshot the follower's log pointer must reflect
    // the snapshot point (commit_index advances to last_included_index).
    assert_eq!(
        follower.commit_index(),
        6,
        "follower commit_index must match snapshot index after install"
    );
}

// ── 4. Membership change (joint consensus) ────────────────────────────────────

#[test]
fn test_membership_add_node_enters_joint_consensus() {
    let leader = make_leader(1);
    assert!(
        !leader.is_in_joint_consensus(),
        "must start in stable config"
    );

    leader
        .add_node(4, "127.0.0.1:9004".to_owned())
        .expect("add_node must succeed on leader");

    assert!(
        leader.is_in_joint_consensus(),
        "leader must be in joint consensus after add_node"
    );
}

#[test]
fn test_membership_commit_exits_joint_consensus() {
    let leader = make_leader(1);
    leader
        .add_node(4, "127.0.0.1:9004".to_owned())
        .expect("add node");
    assert!(leader.is_in_joint_consensus());

    leader
        .commit_membership_change()
        .expect("commit_membership_change must succeed");
    assert!(
        !leader.is_in_joint_consensus(),
        "must return to stable after committing the membership change"
    );
}

#[test]
fn test_membership_remove_node_enters_joint_consensus() {
    // Start with a 3-node cluster and remove one.
    let leader = make_leader(1);
    assert!(!leader.is_in_joint_consensus());

    leader
        .remove_node(3)
        .expect("remove_node must succeed on leader");
    assert!(
        leader.is_in_joint_consensus(),
        "must enter joint consensus after remove_node"
    );
}

// ── 5. Fencing tokens ────────────────────────────────────────────────────────

#[test]
fn test_fencing_token_leader_issues_token() {
    let leader = make_leader(1);
    let token = leader
        .issue_fencing_token()
        .expect("leader must be able to issue a fencing token");
    // First token in term 1 has term()=1, seq()=0; raw() > 0.
    assert_eq!(token.term(), 1, "first fencing token must have term 1");
    assert!(token.raw() > 0, "fencing token raw value must be non-zero");
}

#[test]
fn test_fencing_token_follower_returns_none() {
    let follower = make_follower(2);
    let token = follower.issue_fencing_token();
    assert!(
        token.is_none(),
        "follower must not issue fencing tokens, got {:?}",
        token
    );
}

#[test]
fn test_fencing_token_monotonically_increasing() {
    let leader = make_leader(1);

    let t1 = leader
        .issue_fencing_token()
        .expect("must issue first token");
    let t2 = leader
        .issue_fencing_token()
        .expect("must issue second token");
    let t3 = leader
        .issue_fencing_token()
        .expect("must issue third token");

    assert!(t2 > t1, "tokens must be strictly increasing (t2 > t1)");
    assert!(t3 > t2, "tokens must be strictly increasing (t3 > t2)");
}

#[test]
fn test_fencing_token_validation_succeeds() {
    let leader = make_leader(1);
    let token = leader.issue_fencing_token().expect("issue token");
    leader
        .validate_fencing_token(&token)
        .expect("valid token must pass validation");
}

// ── 6. PlacementScheduler lifecycle ──────────────────────────────────────────

#[tokio::test]
async fn test_placement_scheduler_attach_and_stop_on_step_down() {
    // Build a 3-node Arc node so attach_placement_scheduler compiles.
    let config = RaftConfig::new(1, vec![1, 2, 3]);
    let node = Arc::new(RaftNode::new(config).expect("create node"));

    let registry = make_registry();
    let scheduler = PlacementScheduler::new(
        Arc::clone(&node),
        Arc::clone(&registry),
        PlacementPolicy::default_policy(),
        PlacementSchedulerConfig {
            tick_interval: Duration::from_secs(3600),
            max_actions_per_tick: 5,
            imbalance_threshold: 0.2,
        },
    );

    // Attach the scheduler.
    node.attach_placement_scheduler(scheduler);

    // The handle must be stored.
    assert!(
        node.placement_scheduler_handle.read().is_some(),
        "placement_scheduler_handle must be Some after attach"
    );

    // Calling attach again must replace the old handle (stopping the old scheduler).
    let registry2 = make_registry();
    let scheduler2 = PlacementScheduler::new(
        Arc::clone(&node),
        Arc::clone(&registry2),
        PlacementPolicy::default_policy(),
        PlacementSchedulerConfig {
            tick_interval: Duration::from_secs(3600),
            max_actions_per_tick: 5,
            imbalance_threshold: 0.2,
        },
    );
    node.attach_placement_scheduler(scheduler2);
    assert!(
        node.placement_scheduler_handle.read().is_some(),
        "placement_scheduler_handle must still be Some after replacement"
    );
}

#[tokio::test]
async fn test_placement_scheduler_exits_promptly_on_stop() {
    let config = RaftConfig::new(1, vec![1, 2, 3]);
    let node = Arc::new(RaftNode::new(config).expect("create node"));
    let registry = make_registry();

    let scheduler = PlacementScheduler::new(
        Arc::clone(&node),
        Arc::clone(&registry),
        PlacementPolicy::default_policy(),
        PlacementSchedulerConfig {
            tick_interval: Duration::from_secs(3600),
            max_actions_per_tick: 5,
            imbalance_threshold: 0.2,
        },
    );

    let handle = scheduler.handle();
    let join = tokio::spawn(scheduler.run());

    handle.stop();
    tokio::time::timeout(Duration::from_secs(2), join)
        .await
        .expect("scheduler must exit within 2 s after handle.stop()")
        .expect("scheduler task must not panic");
}

// ── 7. ClusterCommand round-trip ──────────────────────────────────────────────

#[test]
fn test_cluster_command_round_trip_all_variants() {
    let variants: Vec<ClusterCommand> = vec![
        ClusterCommand::DataPut {
            key: b"hello".to_vec(),
            value: b"world".to_vec(),
        },
        ClusterCommand::DataDelete {
            key: b"goodbye".to_vec(),
        },
        ClusterCommand::PlaceSplit {
            shard_id: 42,
            split_key: vec![0x80, 0x00, 0xFF],
        },
        ClusterCommand::PlaceMerge {
            left_shard_id: 7,
            right_shard_id: 8,
        },
        ClusterCommand::PlaceTransfer {
            shard_id: 99,
            from_node: 1,
            to_node: 3,
        },
        ClusterCommand::MembershipAdd {
            node_id: 5,
            address: "192.168.1.10:7878".to_owned(),
        },
        ClusterCommand::MembershipRemove { node_id: 5 },
    ];

    assert_eq!(
        variants.len(),
        7,
        "must cover all 7 ClusterCommand variants"
    );

    for cmd in &variants {
        let encoded = cmd.encode();
        // Leading byte must be the tag.
        assert_eq!(
            encoded[0],
            cmd.tag(),
            "first byte must equal tag for {:?}",
            cmd
        );
        // Decode must reproduce the original.
        let decoded =
            ClusterCommand::decode(&encoded).expect("decode must succeed for freshly encoded cmd");
        assert_eq!(
            cmd, &decoded,
            "decoded command must equal original for {:?}",
            cmd
        );
        // TryFrom must also work.
        let via_try_from = ClusterCommand::try_from(encoded.as_slice())
            .expect("TryFrom must succeed for freshly encoded cmd");
        assert_eq!(cmd, &via_try_from, "TryFrom result must equal original");
    }
}

#[test]
fn test_cluster_command_decode_empty_is_error() {
    let result = ClusterCommand::decode(&[]);
    assert!(result.is_err(), "decoding empty slice must return an error");
}

#[test]
fn test_cluster_command_decode_unknown_tag_is_error() {
    let bytes = [0xFF, b'{', b'}'];
    let result = ClusterCommand::decode(&bytes);
    assert!(result.is_err(), "unknown tag byte must return an error");
}

// ── 8. Shard registry with partitioner ───────────────────────────────────────

#[test]
fn test_shard_registry_partitioner_routes_to_correct_node() {
    let registry = ShardRegistry::new();

    // Two non-overlapping ranges on different nodes.
    let range_a = KeyRange::new(Key::from_slice(&[0x00u8]), Key::from_slice(&[0x80u8]))
        .expect("valid range A");
    let range_b = KeyRange::new(Key::from_slice(&[0x80u8]), Key::from_slice(&[0xFFu8]))
        .expect("valid range B");

    let id_a = registry.allocate_shard_id();
    let shard_a = ShardMetadata::new(id_a, range_a, 1 /* node 1 */);
    registry.register(shard_a).expect("register shard A");

    let id_b = registry.allocate_shard_id();
    let shard_b = ShardMetadata::new(id_b, range_b, 2 /* node 2 */);
    registry.register(shard_b).expect("register shard B");

    let registry_arc = Arc::new(registry);
    let partitioner = Partitioner::new(Arc::clone(&registry_arc), PartitionStrategy::Range);

    // A key in range A must route to node 1.
    let key_a = Key::from_slice(&[0x40u8]);
    let routed_a = partitioner.route_key(&key_a).expect("route key A");
    assert_eq!(
        routed_a.node_id, 1,
        "key 0x40 must route to node 1 (range A)"
    );

    // A key in range B must route to node 2.
    let key_b = Key::from_slice(&[0xC0u8]);
    let routed_b = partitioner.route_key(&key_b).expect("route key B");
    assert_eq!(
        routed_b.node_id, 2,
        "key 0xC0 must route to node 2 (range B)"
    );
}

#[test]
fn test_shard_registry_count_and_get() {
    let registry = ShardRegistry::new();
    assert_eq!(registry.count(), 0, "fresh registry must be empty");

    let range =
        KeyRange::new(Key::from_slice(&[0u8]), Key::from_slice(&[128u8])).expect("valid range");
    let id = registry.allocate_shard_id();
    let shard = ShardMetadata::new(id, range, 1);
    registry.register(shard).expect("register shard");

    assert_eq!(
        registry.count(),
        1,
        "registry must have 1 shard after register"
    );
    assert!(
        registry.get(id).is_some(),
        "registered shard must be retrievable by id"
    );
}

// ── 9. Placement coordinator determinism ─────────────────────────────────────

#[test]
fn test_placement_coordinator_deterministic_empty_registry() {
    let registry = ShardRegistry::new();
    let coord = PlacementCoordinator::new(PlacementPolicy::default_policy());

    let plan1 = coord.plan(&registry).expect("plan1 on empty registry");
    let plan2 = coord.plan(&registry).expect("plan2 on empty registry");

    assert_eq!(
        plan1.len(),
        plan2.len(),
        "empty registry must produce identical (empty) plans on two calls"
    );
}

#[test]
fn test_placement_coordinator_deterministic_with_shards() {
    let registry = ShardRegistry::new();
    let policy = PlacementPolicy::new(100, 1000, 10, 100, 0.2);
    let coord = PlacementCoordinator::new(policy);

    // Hot shard on node 1 and cold shard on node 2.
    let r1 = KeyRange::new(Key::from_slice(&[0u8]), Key::from_slice(&[128u8])).expect("range1");
    let r2 = KeyRange::new(Key::from_slice(&[128u8]), Key::from_slice(&[255u8])).expect("range2");

    let id1 = registry.allocate_shard_id();
    let mut s1 = ShardMetadata::new(id1, r1, 1);
    s1.update_stats(200, 5000); // hot
    registry.register(s1).expect("register s1");

    let id2 = registry.allocate_shard_id();
    let mut s2 = ShardMetadata::new(id2, r2, 2);
    s2.update_stats(5, 50); // cold
    registry.register(s2).expect("register s2");

    let plan1 = coord.plan(&registry).expect("plan1");
    let plan2 = coord.plan(&registry).expect("plan2");

    assert_eq!(
        plan1.len(),
        plan2.len(),
        "plan length must be identical on two calls"
    );
    for (a1, a2) in plan1.actions.iter().zip(plan2.actions.iter()) {
        assert_eq!(
            a1, a2,
            "every action must be identical between the two plans"
        );
    }
}

#[test]
fn test_placement_coordinator_does_not_mutate_registry() {
    let registry = ShardRegistry::new();
    let range = KeyRange::new(Key::from_slice(&[0u8]), Key::from_slice(&[255u8])).expect("range");
    let id = registry.allocate_shard_id();
    let shard = ShardMetadata::new(id, range, 1);
    registry.register(shard).expect("register");

    let count_before = registry.count();
    let coord = PlacementCoordinator::new(PlacementPolicy::default_policy());
    coord.plan(&registry).expect("plan must succeed");
    let count_after = registry.count();

    assert_eq!(
        count_before, count_after,
        "plan() must not mutate the registry"
    );
}

// ── 10. Chunked snapshot streaming ────────────────────────────────────────────

#[test]
fn test_chunked_snapshot_small_is_single_shot() {
    let (leader, _dir) = make_leader_with_snapshot_dir();

    // Propose 6 entries and advance applied index past the threshold (5).
    for i in 0..6_u32 {
        leader
            .propose(Command::from_str(&format!("entry{}", i)))
            .expect("propose");
    }
    {
        let mut log = leader.log.write();
        log.set_commit_index(6).expect("set commit");
        log.set_applied_index(6).expect("set applied");
    }
    leader
        .maybe_create_snapshot(b"small-state-data".to_vec())
        .expect("create snapshot");

    // Simulate peer 2 lagging far behind.
    {
        let mut ls = leader.leader_state.write();
        let ls = ls.as_mut().expect("leader state");
        ls.next_index.insert(2, 1);
    }

    let req = leader
        .prepare_install_snapshot(2)
        .expect("prepare_install_snapshot")
        .expect("request for lagging peer");

    // Small snapshot (well below the 4 MiB chunk threshold) must be single-shot.
    assert!(
        req.done,
        "small snapshot must be delivered as a single chunk"
    );
    assert_eq!(req.offset, 0, "single-shot must have offset 0");
    assert_eq!(req.last_included_index, 6, "snapshot must cover index 6");
    // No streamer state must be left behind.
    assert!(
        leader.snapshot_streamers.read().is_empty(),
        "no streamers must remain after single-shot delivery"
    );
}

#[test]
fn test_chunked_snapshot_install_on_fresh_follower() {
    let (leader, _dir) = make_leader_with_snapshot_dir();

    for i in 0..6_u32 {
        leader
            .propose(Command::from_str(&format!("chunk_entry{}", i)))
            .expect("propose");
    }
    {
        let mut log = leader.log.write();
        log.set_commit_index(6).expect("set commit");
        log.set_applied_index(6).expect("set applied");
    }
    let state_data = b"chunked-snapshot-state".to_vec();
    leader
        .maybe_create_snapshot(state_data.clone())
        .expect("create snapshot");

    // Peer 2 is lagging.
    {
        let mut ls = leader.leader_state.write();
        let ls = ls.as_mut().expect("leader state");
        ls.next_index.insert(2, 1);
    }

    let req = leader
        .prepare_install_snapshot(2)
        .expect("prepare_install_snapshot")
        .expect("request for lagging peer");

    // Install on a fresh follower node.
    let follower_dir = TempDir::new().expect("follower tempdir");
    let mut fconfig = RaftConfig::new(2, vec![1, 2, 3]);
    fconfig.snapshot_dir = Some(follower_dir.path().to_path_buf());
    let follower = RaftNode::new(fconfig).expect("follower");

    let resp = follower
        .handle_install_snapshot(req)
        .expect("install snapshot must succeed");
    // Term in the response must be valid.
    assert!(
        resp.term == follower.current_term() || resp.term >= 1,
        "response term must be valid"
    );

    // Commit index on the follower must advance to the snapshot index.
    assert_eq!(
        follower.commit_index(),
        6,
        "follower commit_index must equal the snapshot's last_included_index"
    );
}

#[test]
fn test_snapshot_receiver_accumulates_chunks() {
    use crate::snapshot::{InstallSnapshotRequest, SnapshotReceiver};

    let data_part1: Vec<u8> = vec![0x01, 0x02, 0x03];
    let data_part2: Vec<u8> = vec![0x04, 0x05, 0x06];
    let mut expected = data_part1.clone();
    expected.extend_from_slice(&data_part2);

    let mut receiver = SnapshotReceiver::new(10, 1);

    let chunk1 = InstallSnapshotRequest::new_chunk(1, 1, 10, 1, 0, data_part1, false);
    let result1 = receiver
        .receive_chunk(&chunk1)
        .expect("first chunk must be accepted");
    assert!(
        result1.is_none(),
        "first chunk must not complete the snapshot"
    );

    let chunk2 = InstallSnapshotRequest::new_chunk(1, 1, 10, 1, 3, data_part2, true);
    let result2 = receiver
        .receive_chunk(&chunk2)
        .expect("second chunk must be accepted");

    let snapshot = result2.expect("second chunk (done=true) must complete the snapshot");
    assert_eq!(
        snapshot.data, expected,
        "reassembled snapshot data must equal the concatenated chunks"
    );
    assert_eq!(
        snapshot.metadata.last_included_index, 10,
        "snapshot metadata index must match"
    );
}
