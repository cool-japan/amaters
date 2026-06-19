use super::*;
use std::sync::Arc;
use tracing_test::traced_test;

// ── Shared test helpers (duplicated from node_tests.rs for module isolation) ──

fn create_test_node(node_id: NodeId) -> RaftNode {
    let config = RaftConfig::new(node_id, vec![1, 2, 3]);
    RaftNode::new(config).expect("Failed to create node")
}

fn create_test_node_with_snapshots(node_id: NodeId) -> (RaftNode, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let mut config = RaftConfig::new(node_id, vec![1, 2, 3]);
    config.snapshot_dir = Some(dir.path().to_path_buf());
    config.snapshot_threshold = 5;
    let node = RaftNode::new(config).expect("Failed to create node");
    (node, dir)
}

fn create_leader_node(node_id: NodeId) -> RaftNode {
    let node = create_test_node(node_id);
    node.start_election();
    let resp = RequestVoteResponse::granted(node.current_term());
    node.handle_vote_response(if node_id == 1 { 2 } else { 1 }, resp);
    assert_eq!(node.state(), NodeState::Leader);
    node
}

fn create_leader_5node(node_id: NodeId) -> RaftNode {
    let config = RaftConfig::new(node_id, vec![1, 2, 3, 4, 5]);
    let node = RaftNode::new(config).expect("Failed to create 5-node");
    node.start_election();
    let term = node.current_term();
    let peers: Vec<NodeId> = vec![1, 2, 3, 4, 5]
        .into_iter()
        .filter(|&p| p != node_id)
        .collect();
    node.handle_vote_response(peers[0], RequestVoteResponse::granted(term));
    node.handle_vote_response(peers[1], RequestVoteResponse::granted(term));
    assert_eq!(node.state(), NodeState::Leader);
    node
}

fn wal_replay_test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "amaters_wal_replay_adv_{name}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// ── Tests ──

#[test]
fn test_replicate_to_followers_returns_nothing_when_caught_up() {
    let leader = create_leader_node(1);

    // Don't propose any entries -- followers are already "caught up"
    let requests = leader.replicate_to_followers();
    assert!(
        requests.is_empty(),
        "no replication requests when all followers are caught up"
    );
}

#[test]
fn test_create_replication_request_for_specific_peer() {
    let leader = create_leader_node(1);

    // Propose an entry
    leader.propose(Command::from_str("cmd1")).expect("propose");

    // Should have a request for peer 2
    let req = leader.create_replication_request_for(2);
    assert!(req.is_some(), "should have request for peer 2");

    let req = req.expect("request for peer 2");
    assert_eq!(req.entries.len(), 1);
    assert_eq!(req.leader_id, 1);

    // Non-leader should return None
    let follower = create_test_node(2);
    assert!(follower.create_replication_request_for(3).is_none());
}

#[test]
fn test_leader_steps_down_on_higher_term_in_response() {
    let leader = create_leader_node(1);
    assert_eq!(leader.state(), NodeState::Leader);

    // Propose an entry so we can replicate
    leader.propose(Command::from_str("cmd1")).expect("propose");

    // Simulate a response with a higher term (follower has moved ahead)
    let resp = AppendEntriesResponse::rejected(leader.current_term() + 5);

    leader
        .handle_replication_response(2, resp)
        .expect("handle response");

    // Leader should step down
    assert_eq!(
        leader.state(),
        NodeState::Follower,
        "leader should step down on higher term"
    );
}

#[test]
fn test_candidate_steps_down_on_append_entries() {
    let candidate = create_test_node(1);
    candidate.start_election();
    assert_eq!(candidate.state(), NodeState::Candidate);

    let candidate_term = candidate.current_term();

    // Receive AppendEntries from a leader with equal or higher term
    let req = AppendEntriesRequest::heartbeat(candidate_term, 2, 0, 0, 0);
    let resp = candidate.handle_append_entries(req);

    assert!(resp.success);
    assert_eq!(
        candidate.state(),
        NodeState::Follower,
        "candidate should step down to follower"
    );
    assert_eq!(candidate.leader_id(), Some(2));
}

#[test]
fn test_replication_multiple_rounds() {
    let leader = create_leader_node(1);
    let follower = create_test_node(2);

    let term = leader.current_term();
    follower.persistent.write().current_term = term;

    // Round 1: propose and replicate 2 entries
    leader.propose(Command::from_str("cmd1")).expect("propose");
    leader.propose(Command::from_str("cmd2")).expect("propose");

    let requests = leader.replicate_to_followers();
    let (_, req) = requests
        .iter()
        .find(|(peer, _)| *peer == 2)
        .expect("request for peer 2");
    let resp = follower.handle_append_entries(req.clone());
    assert!(resp.success);
    leader
        .handle_replication_response(2, resp)
        .expect("handle response");

    // Round 2: propose 2 more and replicate
    leader.propose(Command::from_str("cmd3")).expect("propose");
    leader.propose(Command::from_str("cmd4")).expect("propose");

    let requests = leader.replicate_to_followers();
    let (_, req) = requests
        .iter()
        .find(|(peer, _)| *peer == 2)
        .expect("request for peer 2");

    // Should only have entries 3 and 4 (not 1 and 2 again)
    assert_eq!(
        req.entries.len(),
        2,
        "should only send new entries, not already replicated ones"
    );
    assert_eq!(req.entries[0].index, 3);
    assert_eq!(req.entries[1].index, 4);
    assert_eq!(
        req.prev_log_index, 2,
        "prev should point to last replicated"
    );

    let resp = follower.handle_append_entries(req.clone());
    assert!(resp.success);
    assert_eq!(resp.last_log_index, 4);
}

#[test]
fn test_commit_index_joint_consensus() {
    // Test that commit index advancement works during joint consensus
    let leader = create_leader_5node(1);
    let follower2 = create_test_node(2);
    // We need 5-node followers for this test
    let config3 = RaftConfig::new(3, vec![1, 2, 3, 4, 5]);
    let follower3 = RaftNode::new(config3).expect("create node 3");

    let term = leader.current_term();
    follower2.persistent.write().current_term = term;
    follower3.persistent.write().current_term = term;

    // Propose entries
    leader.propose(Command::from_str("cmd1")).expect("propose");

    // Replicate to follower 2
    let requests = leader.replicate_to_followers();
    if let Some((_, req)) = requests.iter().find(|(peer, _)| *peer == 2) {
        let resp = follower2.handle_append_entries(req.clone());
        assert!(resp.success);
        leader
            .handle_replication_response(2, resp)
            .expect("handle response");
    }

    // With 5-node cluster, quorum is 3.
    // Leader (1) + follower2 (2) = 2 nodes. Not enough for commit.
    // Need one more.

    // Replicate to follower 3
    let requests = leader.replicate_to_followers();
    if let Some((_, req)) = requests.iter().find(|(peer, _)| *peer == 3) {
        let resp = follower3.handle_append_entries(req.clone());
        assert!(resp.success);
        leader
            .handle_replication_response(3, resp)
            .expect("handle response");
    }

    // Now leader + 2 + 3 = 3 nodes => quorum in 5-node cluster
    assert_eq!(
        leader.commit_index(),
        1,
        "commit index should advance with 3/5 quorum"
    );
}

#[test]
fn test_append_entries_updates_follower_state_to_follower() {
    // A node in any state receiving a valid AppendEntries should
    // transition to follower if the term is equal or higher.
    let node = create_test_node(1);

    // Start as candidate
    node.start_election();
    assert_eq!(node.state(), NodeState::Candidate);

    let term = node.current_term();

    // Receive AppendEntries from legitimate leader with higher term
    let req = AppendEntriesRequest::heartbeat(term + 1, 2, 0, 0, 0);
    let resp = node.handle_append_entries(req);
    assert!(resp.success);
    assert_eq!(node.state(), NodeState::Follower);
    assert_eq!(node.leader_id(), Some(2));
    assert_eq!(node.current_term(), term + 1);
}

#[test]
fn test_auto_snapshot_below_threshold() {
    let (node, _dir) = create_test_node_with_snapshots(1);
    node.start_election();
    let resp = RequestVoteResponse::granted(1);
    node.handle_vote_response(2, resp);

    for i in 0..3 {
        node.propose(Command::from_str(&format!("cmd{}", i)))
            .expect("Propose should succeed");
    }
    {
        let mut log = node.log.write();
        log.set_commit_index(3).expect("ok");
        log.set_applied_index(3).expect("ok");
    }

    let policy = SnapshotPolicy::new(5);
    let created = node
        .auto_snapshot_if_needed(&policy, || Ok(b"state".to_vec()))
        .expect("ok");
    assert!(!created);
}

#[test]
fn test_auto_snapshot_above_threshold() {
    let (node, _dir) = create_test_node_with_snapshots(1);
    node.start_election();
    let resp = RequestVoteResponse::granted(1);
    node.handle_vote_response(2, resp);

    for i in 0..6 {
        node.propose(Command::from_str(&format!("cmd{}", i)))
            .expect("Propose should succeed");
    }
    {
        let mut log = node.log.write();
        log.set_commit_index(6).expect("ok");
        log.set_applied_index(6).expect("ok");
    }

    let policy = SnapshotPolicy::new(5);
    let created = node
        .auto_snapshot_if_needed(&policy, || Ok(b"state".to_vec()))
        .expect("ok");
    assert!(created);
}

#[test]
fn test_auto_snapshot_multiple_cycles() {
    let (node, _dir) = create_test_node_with_snapshots(1);
    node.start_election();
    let resp = RequestVoteResponse::granted(1);
    node.handle_vote_response(2, resp);

    let policy = SnapshotPolicy::new(5);

    // First batch
    for i in 0..6 {
        node.propose(Command::from_str(&format!("a{}", i)))
            .expect("ok");
    }
    {
        let mut log = node.log.write();
        log.set_commit_index(6).expect("ok");
        log.set_applied_index(6).expect("ok");
    }
    let created = node
        .auto_snapshot_if_needed(&policy, || Ok(b"state1".to_vec()))
        .expect("ok");
    assert!(created);

    // Second batch
    for i in 0..6 {
        node.propose(Command::from_str(&format!("b{}", i)))
            .expect("ok");
    }
    {
        let mut log = node.log.write();
        log.set_commit_index(12).expect("ok");
        log.set_applied_index(12).expect("ok");
    }
    let created = node
        .auto_snapshot_if_needed(&policy, || Ok(b"state2".to_vec()))
        .expect("ok");
    assert!(created);
}

#[traced_test]
#[test]
fn test_state_transitions_are_traced() {
    // Verify that a node can go through state transitions with tracing enabled
    // without panicking. This is a compilation + basic smoke test for the
    // structured tracing instrumentation added throughout node.rs.
    let node = create_test_node(1);

    // Follower → Candidate (start_election emits raft_election span + info)
    let _requests = node.start_election();
    assert_eq!(node.state(), NodeState::Candidate);

    // Candidate → Leader (handle_vote_response emits "Won election" + "Became leader")
    let resp = RequestVoteResponse::granted(node.current_term());
    let became_leader = node.handle_vote_response(2, resp);
    assert!(became_leader);
    assert_eq!(node.state(), NodeState::Leader);

    // Leader proposes an entry (info logged)
    let idx = node
        .propose(Command::from_str("traced_cmd"))
        .expect("propose ok");
    assert!(idx > 0);

    // Leader receives stale AppendEntries with higher term → steps down
    let higher_term_req = AppendEntriesRequest::heartbeat(3, 2, 0, 0, 0);
    let ae_resp = node.handle_append_entries(higher_term_req);
    assert!(ae_resp.success);
    assert_eq!(node.state(), NodeState::Follower);

    // Verify key tracing messages were emitted
    assert!(logs_contain("Started election"));
    assert!(logs_contain("Won election with quorum"));
    assert!(logs_contain("Became leader"));
    assert!(logs_contain("Stepped down to follower"));
}

// ===========================================================================
// WAL replay on startup
// ===========================================================================

#[test]
fn test_wal_replay_on_startup_basic() {
    // Write entries to WAL, then start a node with wal_dir pointing there.
    // The entries should appear in the RaftLog after construction.
    use crate::wal::{SyncMode, WalWriter};

    let base = wal_replay_test_dir("basic");
    let wal_dir = base.join("wal");
    let persist_dir = base.join("persist");

    // Write some WAL entries
    let mut writer =
        WalWriter::new(&wal_dir, SyncMode::EveryWrite, 64 * 1024 * 1024).expect("create writer");
    for i in 1..=5 {
        let entry = LogEntry::new(1, i, Command::from_str(&format!("cmd-{i}")));
        writer.append(&entry).expect("append");
    }
    drop(writer);

    // Create a node with both persistence dir and wal_dir
    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.persistence_dir = Some(persist_dir.clone());
    config.wal_dir = Some(wal_dir.clone());

    let node = RaftNode::new(config).expect("create node");

    // Verify all 5 WAL entries were replayed
    assert_eq!(node.last_log_index(), 5);

    // Verify entry content
    {
        let log = node.log.read();
        let entry = log.get(3).expect("entry at index 3");
        assert_eq!(entry.term, 1);
        assert_eq!(entry.command.data, b"cmd-3");
    }

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn test_wal_replay_merges_with_persistence() {
    // Persistence has entries 1..3, WAL has entries 1..5.
    // After replay, log should have all 5 entries.
    use crate::persistence::FilePersistence;
    use crate::wal::{SyncMode, WalWriter};

    let base = wal_replay_test_dir("merge");
    let wal_dir = base.join("wal");
    let persist_dir = base.join("persist");

    // Write entries 1..3 into persistence
    let fp = FilePersistence::new(&persist_dir, true).expect("create persistence");
    let persist_entries: Vec<LogEntry> = (1..=3)
        .map(|i| LogEntry::new(1, i, Command::from_str(&format!("cmd-{i}"))))
        .collect();
    fp.append_entries(&persist_entries)
        .expect("persist entries");
    fp.save_state(1, None).expect("save state");

    // Write entries 1..5 into WAL (superset)
    let mut writer =
        WalWriter::new(&wal_dir, SyncMode::EveryWrite, 64 * 1024 * 1024).expect("create writer");
    for i in 1..=5 {
        let entry = LogEntry::new(1, i, Command::from_str(&format!("cmd-{i}")));
        writer.append(&entry).expect("append");
    }
    drop(writer);

    // Create node
    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.persistence_dir = Some(persist_dir.clone());
    config.wal_dir = Some(wal_dir.clone());

    let node = RaftNode::new(config).expect("create node");

    // Should have all 5 entries: 3 from persistence + 2 from WAL replay
    assert_eq!(node.last_log_index(), 5);

    {
        let log = node.log.read();
        let e5 = log.get(5).expect("entry 5");
        assert_eq!(e5.command.data, b"cmd-5");
    }

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn test_wal_replay_with_applied_index_recovery() {
    // Persist some entries and an applied_index, write additional WAL entries.
    // On startup, the applied_index should be restored.
    use crate::persistence::FilePersistence;
    use crate::wal::{SyncMode, WalWriter};

    let base = wal_replay_test_dir("applied");
    let wal_dir = base.join("wal");
    let persist_dir = base.join("persist");

    // Persist entries 1..5 and applied_index = 3
    let fp = FilePersistence::new(&persist_dir, true).expect("create persistence");
    let entries: Vec<LogEntry> = (1..=5)
        .map(|i| LogEntry::new(1, i, Command::from_str(&format!("cmd-{i}"))))
        .collect();
    fp.append_entries(&entries).expect("persist");
    fp.save_state(1, None).expect("save state");
    fp.save_applied_index(3).expect("save applied");

    // WAL has entries 1..7
    let mut writer =
        WalWriter::new(&wal_dir, SyncMode::EveryWrite, 64 * 1024 * 1024).expect("writer");
    for i in 1..=7 {
        writer
            .append(&LogEntry::new(1, i, Command::from_str(&format!("wal-{i}"))))
            .expect("append");
    }
    drop(writer);

    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.persistence_dir = Some(persist_dir.clone());
    config.wal_dir = Some(wal_dir.clone());

    let node = RaftNode::new(config).expect("create node");

    // 5 from persistence + 2 from WAL replay = 7 total
    assert_eq!(node.last_log_index(), 7);

    // applied_index and commit_index should be restored
    {
        let log = node.log.read();
        assert_eq!(log.applied_index(), 3);
        assert_eq!(log.commit_index(), 3);
    }

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn test_wal_replay_crash_recovery_partial_entry() {
    // Write entries to WAL, then corrupt the tail (simulating a crash
    // mid-write). The node should recover all complete entries.
    use crate::wal::{SyncMode, WalWriter};

    let base = wal_replay_test_dir("crash_partial");
    let wal_dir = base.join("wal");

    // Write 5 entries
    let mut writer =
        WalWriter::new(&wal_dir, SyncMode::EveryWrite, 64 * 1024 * 1024).expect("writer");
    for i in 1..=5 {
        writer
            .append(&LogEntry::new(1, i, Command::from_str(&format!("cmd-{i}"))))
            .expect("append");
    }
    drop(writer);

    // Find the segment file and append garbage bytes (simulating partial write)
    let seg_files: Vec<_> = std::fs::read_dir(&wal_dir)
        .expect("readdir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".seg"))
        .collect();
    assert!(!seg_files.is_empty());

    // Append partial garbage to the last segment
    let last_seg = &seg_files[seg_files.len() - 1];
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(last_seg.path())
        .expect("open seg");
    use std::io::Write;
    f.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02])
        .expect("write garbage");
    drop(f);

    // Create node — should recover 5 valid entries despite partial tail
    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.wal_dir = Some(wal_dir.clone());

    let node = RaftNode::new(config).expect("create node");
    assert_eq!(node.last_log_index(), 5);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn test_wal_replay_empty_wal_dir() {
    // If wal_dir exists but has no segments, no entries should be replayed.
    let base = wal_replay_test_dir("empty_wal");
    let wal_dir = base.join("wal");
    std::fs::create_dir_all(&wal_dir).expect("create wal dir");

    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.wal_dir = Some(wal_dir.clone());

    let node = RaftNode::new(config).expect("create node");
    assert_eq!(node.last_log_index(), 0);
    assert_eq!(node.commit_index(), 0);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn test_wal_replay_with_persistence_backend() {
    // Test with_persistence() path for WAL replay.
    use crate::persistence::FilePersistence;
    use crate::wal::{SyncMode, WalWriter};

    let base = wal_replay_test_dir("with_persist");
    let wal_dir = base.join("wal");
    let persist_dir = base.join("persist");

    // Persist entries 1..3
    let fp = FilePersistence::new(&persist_dir, true).expect("create fp");
    let entries: Vec<LogEntry> = (1..=3)
        .map(|i| LogEntry::new(2, i, Command::from_str(&format!("p-{i}"))))
        .collect();
    fp.append_entries(&entries).expect("persist");
    fp.save_state(2, Some(1)).expect("save state");
    fp.save_applied_index(2).expect("save applied");

    // WAL has entries 1..6
    let mut writer =
        WalWriter::new(&wal_dir, SyncMode::EveryWrite, 64 * 1024 * 1024).expect("writer");
    for i in 1..=6 {
        writer
            .append(&LogEntry::new(2, i, Command::from_str(&format!("w-{i}"))))
            .expect("append");
    }
    drop(writer);

    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.wal_dir = Some(wal_dir.clone());

    let persistence: std::sync::Arc<dyn RaftPersistence> = std::sync::Arc::new(fp);
    let node = RaftNode::with_persistence(config, persistence).expect("create node");

    // 3 from persistence + 3 from WAL = 6
    assert_eq!(node.last_log_index(), 6);
    assert_eq!(node.current_term(), 2);

    {
        let log = node.log.read();
        assert_eq!(log.applied_index(), 2);
        assert_eq!(log.commit_index(), 2);
    }

    let _ = std::fs::remove_dir_all(&base);
}

// ── B1 spec-named WAL replay tests ──────────────────────────────────

/// Single-operation WAL replay: write one entry, restart, verify it survives.
#[test]
fn test_wal_replay_single_op() {
    use crate::wal::{SyncMode, WalWriter};

    let base = wal_replay_test_dir("b1_single_op");
    let wal_dir = base.join("wal");

    // Write a single entry to WAL
    {
        let mut writer =
            WalWriter::new(&wal_dir, SyncMode::EveryWrite, 64 * 1024 * 1024).expect("writer");
        writer
            .append(&LogEntry::new(1, 1, Command::from_str("single-op")))
            .expect("append");
    }

    // Restart node with wal_dir set
    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.wal_dir = Some(wal_dir.clone());

    let node = RaftNode::new(config).expect("create node");

    // Entry should be present in log after replay
    assert_eq!(node.last_log_index(), 1);
    {
        let log = node.log.read();
        let entry = log.get(1).expect("entry at index 1");
        assert_eq!(entry.command.data, b"single-op");
    }

    // Node must NOT be in recovering state after startup
    assert!(!node.is_recovering());

    let _ = std::fs::remove_dir_all(&base);
}

/// Multi-operation WAL replay: write N entries, restart, verify all survive.
#[test]
fn test_wal_replay_multi_op_restart() {
    use crate::wal::{SyncMode, WalWriter};

    let base = wal_replay_test_dir("b1_multi_op");
    let wal_dir = base.join("wal");

    const N: u64 = 10;

    // Write N entries to WAL
    {
        let mut writer =
            WalWriter::new(&wal_dir, SyncMode::EveryWrite, 64 * 1024 * 1024).expect("writer");
        for i in 1..=N {
            writer
                .append(&LogEntry::new(1, i, Command::from_str(&format!("op-{i}"))))
                .expect("append");
        }
    }

    // Restart node
    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.wal_dir = Some(wal_dir.clone());

    let node = RaftNode::new(config).expect("create node");

    assert_eq!(node.last_log_index(), N);
    {
        let log = node.log.read();
        for i in 1..=N {
            let entry = log.get(i).expect("entry exists");
            assert_eq!(entry.command.data, format!("op-{i}").as_bytes());
        }
    }

    // Recovery flag cleared
    assert!(!node.is_recovering());

    let _ = std::fs::remove_dir_all(&base);
}

/// WAL replay is skipped for entries superseded by a snapshot.
///
/// When applied_index (from persistence) is >= WAL's last_index, the node
/// considers the WAL entries as already applied and does not replay them
/// on top of the snapshot state.
#[test]
fn test_wal_replay_ignored_after_snapshot() {
    use crate::persistence::FilePersistence;
    use crate::wal::{SyncMode, WalWriter};

    let base = wal_replay_test_dir("b1_ignored_after_snap");
    let wal_dir = base.join("wal");
    let persist_dir = base.join("persist");

    // Simulate a node that took a snapshot covering indices 1..=5
    let fp = FilePersistence::new(&persist_dir, true).expect("create fp");
    let entries: Vec<LogEntry> = (1..=5)
        .map(|i| LogEntry::new(1, i, Command::from_str(&format!("snap-{i}"))))
        .collect();
    fp.append_entries(&entries).expect("persist");
    fp.save_state(1, None).expect("save state");
    fp.save_applied_index(5).expect("save applied");

    // WAL also has entries 1..=5 (duplicates, covered by snapshot)
    {
        let mut writer =
            WalWriter::new(&wal_dir, SyncMode::EveryWrite, 64 * 1024 * 1024).expect("writer");
        for i in 1..=5 {
            writer
                .append(&LogEntry::new(1, i, Command::from_str(&format!("wal-{i}"))))
                .expect("append");
        }
    }

    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.wal_dir = Some(wal_dir.clone());

    let persistence: std::sync::Arc<dyn RaftPersistence> = std::sync::Arc::new(fp);
    let node = RaftNode::with_persistence(config, persistence).expect("create node");

    // No duplicate entries; log should have exactly 5 entries
    assert_eq!(node.last_log_index(), 5);
    // applied_index remains at 5 (from persistence)
    {
        let log = node.log.read();
        assert_eq!(log.applied_index(), 5);
    }
    assert!(!node.is_recovering());

    let _ = std::fs::remove_dir_all(&base);
}

// ── Fencing token tests ─────────────────────────────────────────────

#[test]
fn test_fencing_token_new() {
    // packed: term=5, seq=0
    let token = FencingToken::new(5, 0);
    assert_eq!(token.term(), 5);
    assert_eq!(token.seq(), 0);
    assert_eq!(token.raw(), (5u64 << 32));
}

#[test]
fn test_fencing_token_bump_seq_increments_sequence() {
    let token = FencingToken::new(5, 0);
    let t1 = token.bump_seq();
    assert_eq!(t1.seq(), 1);
    assert_eq!(t1.term(), 5);

    let t2 = t1.bump_seq();
    assert_eq!(t2.seq(), 2);
}

#[test]
fn test_fencing_token_new_leader_term_resets_seq() {
    // Verify new_leader_term resets seq to 0
    let token = FencingToken::new_leader_term(3);
    assert_eq!(token.term(), 3);
    assert_eq!(token.seq(), 0);
}

// B3 spec-named packed representation roundtrip test
#[test]
fn test_fencing_packed_representation_roundtrip() {
    let original_term: u32 = 42;
    let original_seq: u32 = 1337;
    let token = FencingToken::new(original_term, original_seq);
    assert_eq!(token.term(), original_term);
    assert_eq!(token.seq(), original_seq);
    // Roundtrip via raw
    let raw = token.raw();
    let reconstructed = FencingToken(raw);
    assert_eq!(reconstructed.term(), original_term);
    assert_eq!(reconstructed.seq(), original_seq);
}

#[test]
fn test_fencing_token_state_issues_monotonic_tokens() {
    use crate::state::FencingTokenState;
    let state = FencingTokenState::new();
    state.bump_term_token(5);
    let t0 = state.issue_token();
    let t1 = state.issue_token();
    let t2 = state.issue_token();

    assert_eq!(t0.term(), 5);
    assert_eq!(t1.term(), 5);
    assert_eq!(t2.term(), 5);
    // seq increments: issue_token does fetch_add on the packed value, which
    // increments the low 32 bits (seq field).
    assert!(t1.seq() > t0.seq());
    assert!(t2.seq() > t1.seq());
}

#[test]
fn test_fencing_token_leader_issues_tokens() {
    let node = create_test_node(1);

    // Followers cannot issue fencing tokens
    assert!(node.issue_fencing_token().is_none());

    // Become leader
    node.start_election();
    let resp = RequestVoteResponse::granted(1);
    node.handle_vote_response(2, resp);
    assert_eq!(node.state(), NodeState::Leader);

    // Leader can issue fencing tokens
    let t0 = node.issue_fencing_token().expect("should issue token");
    assert_eq!(t0.term(), 1);

    let t1 = node.issue_fencing_token().expect("should issue token");
    assert!(t1.seq() > t0.seq());
}

#[test]
fn test_fencing_token_validate_current_term() {
    let node = create_test_node(1);
    node.start_election();
    let resp = RequestVoteResponse::granted(1);
    node.handle_vote_response(2, resp);

    let token = node.issue_fencing_token().expect("should issue token");
    // Token from current term is valid
    assert!(node.validate_fencing_token(&token).is_ok());
}

// B3 spec-named: rejects old term
#[test]
fn test_fencing_rejects_old_term() {
    let node = create_test_node(1);

    // First election at term 1
    node.start_election();
    let resp = RequestVoteResponse::granted(1);
    node.handle_vote_response(2, resp);

    let old_token = node.issue_fencing_token().expect("should issue token");
    assert_eq!(old_token.term(), 1);

    // Step down via a higher-term AppendEntries
    let higher_term = AppendEntriesRequest::heartbeat(5, 2, 0, 0, 0);
    node.handle_append_entries(higher_term);
    assert_eq!(node.state(), NodeState::Follower);
    assert_eq!(node.current_term(), 5);

    // The old token should now be stale
    let result = node.validate_fencing_token(&old_token);
    assert!(result.is_err());
    match result {
        Err(RaftError::StaleTerm { current, received }) => {
            assert_eq!(current, 5);
            assert_eq!(received, 1);
        }
        other => panic!("Expected StaleTerm, got {:?}", other),
    }
}

// B3 spec-named: accepts current term
#[test]
fn test_fencing_accepts_current_term() {
    let node = create_test_node(1);
    node.start_election();
    node.handle_vote_response(2, RequestVoteResponse::granted(1));
    let token = node.issue_fencing_token().expect("should issue token");
    assert!(node.validate_fencing_token(&token).is_ok());
}

// B3 spec-named: monotonic across leadership change
#[test]
fn test_fencing_monotonic_across_leadership_change() {
    let node = create_test_node(1);

    // Become leader at term 1
    node.start_election();
    node.handle_vote_response(2, RequestVoteResponse::granted(1));
    let token_term1 = node.issue_fencing_token().expect("should issue token");
    assert_eq!(token_term1.term(), 1);

    // Lose leadership via higher term, then win new election
    let higher_term = AppendEntriesRequest::heartbeat(5, 2, 0, 0, 0);
    node.handle_append_entries(higher_term);
    assert_eq!(node.state(), NodeState::Follower);

    // Start a new election at term 6
    node.start_election();
    assert_eq!(node.current_term(), 6);
    node.handle_vote_response(2, RequestVoteResponse::granted(6));
    assert_eq!(node.state(), NodeState::Leader);

    // New token should have the new term
    let token_term6 = node.issue_fencing_token().expect("should issue token");
    assert_eq!(token_term6.term(), 6);
    assert_eq!(token_term6.seq(), 0); // Reset sequence for new term

    // Old token from term 1 is stale at term 6
    assert!(node.validate_fencing_token(&token_term1).is_err());
    // New token from term 6 is valid
    assert!(node.validate_fencing_token(&token_term6).is_ok());
    // Monotonicity: term 6 > term 1
    assert!(token_term6 > token_term1);
}

#[test]
fn test_fencing_token_cleared_on_step_down() {
    let node = create_test_node(1);

    // Become leader
    node.start_election();
    node.handle_vote_response(2, RequestVoteResponse::granted(1));
    assert!(node.issue_fencing_token().is_some());

    // Step down via higher term
    let higher_term = AppendEntriesRequest::heartbeat(5, 2, 0, 0, 0);
    node.handle_append_entries(higher_term);
    assert_eq!(node.state(), NodeState::Follower);

    // No fencing tokens can be issued as follower
    assert!(node.issue_fencing_token().is_none());
}

#[test]
fn test_fencing_token_in_append_entries_request() {
    let token = FencingToken::new(5, 1);
    let req = AppendEntriesRequest::with_fencing_token(5, 1, 0, 0, Vec::new(), 0, token);
    assert_eq!(req.fencing_token, Some(token));
}

#[test]
fn test_fencing_token_in_append_entries_response() {
    let token = FencingToken::new(5, 1);
    let resp = AppendEntriesResponse::success_with_token(5, 10, token);
    assert_eq!(resp.fencing_token, Some(token));
    assert!(resp.success);
}

#[test]
fn test_fencing_token_default_none_in_messages() {
    let req = AppendEntriesRequest::new(5, 1, 0, 0, Vec::new(), 0);
    assert!(req.fencing_token.is_none());

    let resp = AppendEntriesResponse::success(5, 10);
    assert!(resp.fencing_token.is_none());

    let resp2 = AppendEntriesResponse::rejected(5);
    assert!(resp2.fencing_token.is_none());
}

/// Verify that updating `dynamic_config` takes effect immediately and that
/// the heartbeat interval reflects the new value on the next read.
#[test]
fn test_dynamic_reconfiguration_heartbeat_interval() {
    use crate::config::DynamicConfig;

    let node = create_test_node(1);

    // Check defaults set in RaftNode::new — derived from RaftConfig::heartbeat_interval
    // which defaults to 50 ms.
    let initial = node.get_dynamic_config();
    assert_eq!(
        initial.heartbeat_interval_ms, 50,
        "default heartbeat_interval_ms should match RaftConfig default (50 ms)"
    );
    assert_eq!(
        initial.compaction_threshold, 10_000,
        "default compaction_threshold should be 10_000"
    );

    // Hot-reload with new values (e.g. triggered by SIGHUP)
    let new_cfg = DynamicConfig {
        heartbeat_interval_ms: 75,
        compaction_threshold: 5_000,
    };
    node.update_dynamic_config(new_cfg);

    let updated = node.get_dynamic_config();
    assert_eq!(
        updated.heartbeat_interval_ms, 75,
        "heartbeat_interval_ms must be updated to 75"
    );
    assert_eq!(
        updated.compaction_threshold, 5_000,
        "compaction_threshold must be updated to 5_000"
    );

    // The Arc<RwLock<DynamicConfig>> is shared: a clone of the Arc must
    // also see the updated values (simulates the event loop reading it).
    let shared = Arc::clone(&node.dynamic_config);
    let read_back = shared.read();
    assert_eq!(
        read_back.heartbeat_interval_ms, 75,
        "shared Arc reader must see updated heartbeat_interval_ms"
    );
}

/// Verifies that DynamicConfig values extracted from a NodeConfig match the
/// source TOML fields.
#[test]
fn test_dynamic_config_from_node_config() {
    use crate::config::NodeConfig;

    let toml = r#"
bind_addr = "0.0.0.0:7001"
node_id = 1
heartbeat_interval_ms = 100
compaction_threshold = 8000
"#;
    let node_cfg = NodeConfig::from_toml(toml).expect("valid TOML");
    let dyn_cfg = node_cfg.dynamic();

    let node = create_test_node(1);
    node.update_dynamic_config(dyn_cfg);

    let got = node.get_dynamic_config();
    assert_eq!(got.heartbeat_interval_ms, 100);
    assert_eq!(got.compaction_threshold, 8000);
}
