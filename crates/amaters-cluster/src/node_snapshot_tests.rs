//! Snapshot streaming tests for [`RaftNode::prepare_install_snapshot`].
//!
//! These tests live in a separate file to keep `node_tests.rs` under the
//! 2 000-line refactoring limit.

use super::*;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Create a leader node that has already taken a snapshot containing
/// `state_data`.  The snapshot covers log indices 1–6 (applied_index = 6,
/// term = 1).
///
/// Returns the node and the [`tempfile::TempDir`] that backs the snapshot
/// directory. The caller must keep `_dir` alive for the duration of the test.
fn create_leader_with_snapshot(state_data: Vec<u8>) -> (RaftNode, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("create temp dir");
    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.snapshot_dir = Some(dir.path().to_path_buf());
    config.snapshot_threshold = 5;
    let node = RaftNode::new(config).expect("create node");

    // Become leader.
    node.start_election();
    let resp = RequestVoteResponse::granted(1);
    node.handle_vote_response(2, resp);

    // Propose enough entries to exceed the snapshot threshold.
    for i in 0..6 {
        node.propose(Command::from_str(&format!("cmd{}", i)))
            .expect("propose");
    }
    {
        let mut log = node.log.write();
        log.set_commit_index(6).expect("set commit");
        log.set_applied_index(6).expect("set applied");
    }
    node.maybe_create_snapshot(state_data)
        .expect("create snapshot");

    (node, dir)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn test_prepare_install_snapshot_small_single_shot() -> RaftResult<()> {
    // The default chunk threshold is 4 MiB; our snapshot will be far smaller,
    // so prepare_install_snapshot must return a single done=true request with
    // no streamer state left behind.
    let (node, _dir) = create_leader_with_snapshot(b"small state".to_vec());

    // Simulate peer 2 being way behind (next_index = 1, snapshot is at 6).
    {
        let mut ls = node.leader_state.write();
        let state = ls.as_mut().expect("leader state");
        state.next_index.insert(2, 1);
    }

    let req = node
        .prepare_install_snapshot(2)?
        .expect("should produce a request for a lagging peer");

    // Small snapshot: must be delivered as a single complete transfer.
    assert!(req.done, "small snapshot must have done=true");
    assert_eq!(req.offset, 0, "complete transfer must start at offset 0");
    assert_eq!(req.last_included_index, 6);

    // No streamer state must be left over after a single-shot transfer.
    assert!(
        node.snapshot_streamers.read().is_empty(),
        "no streamers should remain after single-shot transfer"
    );

    Ok(())
}

#[test]
fn test_prepare_install_snapshot_large_streams_chunks() -> RaftResult<()> {
    // Build a snapshot larger than our tiny threshold so chunked streaming
    // is triggered.  We set the threshold to 16 bytes so any real state data
    // (> 16 bytes) will cause the chunked path to be taken.
    let dir = tempfile::TempDir::new().expect("create temp dir");

    // 1 KiB of state data — definitely over our 16-byte threshold.
    let state_data: Vec<u8> = (0..1024u16).map(|i| (i % 256) as u8).collect();

    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.snapshot_dir = Some(dir.path().to_path_buf());
    config.snapshot_threshold = 5;
    // Tiny threshold: any real snapshot will take the chunked path.
    config.snapshot_chunk_threshold_bytes = 16;
    // Small chunks: exercise multiple chunk iterations.
    config.snapshot_chunk_size_bytes = 64;

    let node = RaftNode::new(config).expect("create node");

    // Become leader.
    node.start_election();
    let resp = RequestVoteResponse::granted(1);
    node.handle_vote_response(2, resp);

    for i in 0..6 {
        node.propose(Command::from_str(&format!("cmd{}", i)))
            .expect("propose");
    }
    {
        let mut log = node.log.write();
        log.set_commit_index(6).expect("set commit");
        log.set_applied_index(6).expect("set applied");
    }
    node.maybe_create_snapshot(state_data.clone())
        .expect("create snapshot");

    // Peer 2 is far behind the snapshot point.
    {
        let mut ls = node.leader_state.write();
        let state = ls.as_mut().expect("leader state");
        state.next_index.insert(2, 1);
    }

    // Drive the chunk loop: feed each chunk into a SnapshotReceiver and
    // verify the assembled snapshot.
    //
    // The snapshot was created at index=6, term=1 (from the term-1 election).
    let mut receiver = crate::snapshot::SnapshotReceiver::new(6, 1);
    let mut chunk_count = 0usize;
    let assembled_snapshot = loop {
        let req = node
            .prepare_install_snapshot(2)?
            .expect("should produce a chunk while peer is still lagging");
        chunk_count += 1;

        let done = req.done;
        match receiver.receive_chunk(&req)? {
            Some(snap) => {
                assert!(done, "receive_chunk returned Some but done was false");
                break snap;
            }
            None => {
                assert!(!done, "chunk was not done but receiver returned None");
            }
        }
    };

    // More than one chunk must have been required for a 1 KiB snapshot with a
    // 64-byte chunk size.
    assert!(
        chunk_count > 1,
        "expected multiple chunks, got {}",
        chunk_count
    );

    // The reassembled bytes must pass the CRC32 integrity check.
    assert!(
        assembled_snapshot.verify_checksum(),
        "reassembled snapshot checksum must pass"
    );

    // After the final chunk the streamer must have been cleaned up automatically.
    assert!(
        node.snapshot_streamers.read().is_empty(),
        "streamer map must be empty after the final chunk is delivered"
    );

    Ok(())
}

#[test]
fn test_streamer_cleared_when_follower_catches_up() -> RaftResult<()> {
    // Start a chunked stream for peer 2, then advance its next_index past the
    // snapshot point. The next call to prepare_install_snapshot must return
    // Ok(None) and the in-progress streamer must be removed.
    let dir = tempfile::TempDir::new().expect("create temp dir");

    // Force chunked mode with a tiny threshold.
    let state_data: Vec<u8> = (0..512u16).map(|i| (i % 256) as u8).collect();

    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.snapshot_dir = Some(dir.path().to_path_buf());
    config.snapshot_threshold = 5;
    config.snapshot_chunk_threshold_bytes = 16;
    config.snapshot_chunk_size_bytes = 64;

    let node = RaftNode::new(config).expect("create node");

    node.start_election();
    let resp = RequestVoteResponse::granted(1);
    node.handle_vote_response(2, resp);

    for i in 0..6 {
        node.propose(Command::from_str(&format!("cmd{}", i)))
            .expect("propose");
    }
    {
        let mut log = node.log.write();
        log.set_commit_index(6).expect("set commit");
        log.set_applied_index(6).expect("set applied");
    }
    node.maybe_create_snapshot(state_data)
        .expect("create snapshot");

    // Peer 2 starts behind the snapshot point.
    {
        let mut ls = node.leader_state.write();
        let state = ls.as_mut().expect("leader state");
        state.next_index.insert(2, 1);
    }

    // Kick off the first chunk — this must register a streamer entry.
    let first_req = node.prepare_install_snapshot(2)?;
    assert!(first_req.is_some(), "first call must produce a chunk");
    assert!(
        !node.snapshot_streamers.read().is_empty(),
        "streamer must be registered after the first chunk"
    );

    // Simulate peer 2 catching up via an out-of-band mechanism: advance its
    // next_index well past the snapshot point (index 6).
    {
        let mut ls = node.leader_state.write();
        let state = ls.as_mut().expect("leader state");
        state.next_index.insert(2, 10);
    }

    // The next call must recognise the peer has caught up and return Ok(None).
    let no_req = node.prepare_install_snapshot(2)?;
    assert!(no_req.is_none(), "caught-up peer must receive Ok(None)");

    // The stale in-progress streamer must have been cleaned up.
    assert!(
        node.snapshot_streamers.read().is_empty(),
        "streamer must be removed after the peer catches up"
    );

    Ok(())
}

// ── W2.6b: Snapshot transfer to a newly joined node ──────────────────────────

/// Verify that when a new node joins a cluster that already has a snapshot,
/// the leader detects the lagging state, sends a snapshot, and the new
/// follower successfully installs it.
#[test]
fn test_snapshot_transfer_to_new_node() -> RaftResult<()> {
    let dir = tempfile::TempDir::new().expect("create temp dir");

    // Build a leader node (node 1) with snapshot support.
    let mut config = RaftConfig::new(1, vec![1, 2, 3]);
    config.snapshot_dir = Some(dir.path().to_path_buf());
    config.snapshot_threshold = 5;
    let leader = RaftNode::new(config).expect("create leader node");

    // Become leader at term 1.
    leader.start_election();
    let granted = RequestVoteResponse::granted(1);
    leader.handle_vote_response(2, granted);

    // Propose enough entries to exceed the snapshot threshold.
    let state_payload = b"cluster-state-v1".to_vec();
    for i in 0..6u64 {
        leader
            .propose(Command::from_str(&format!("SET k{} {}", i, i)))
            .expect("propose must succeed on leader");
    }

    // Advance applied_index so the snapshot can be taken.
    {
        let mut log = leader.log.write();
        log.set_commit_index(6).expect("set commit index");
        log.set_applied_index(6).expect("set applied index");
    }

    // Create the snapshot at index 6.
    let snapped = leader
        .maybe_create_snapshot(state_payload)
        .expect("snapshot creation must succeed");
    assert!(snapped, "snapshot must have been created");

    // Verify the log is now compacted up to index 6.
    {
        let log = leader.log.read();
        assert_eq!(
            log.snapshot_index(),
            6,
            "log must be compacted to snap index 6"
        );
    }

    // Simulate a 4th node joining: set its next_index to 1 (well below the
    // snapshot point) to trigger the snapshot-transfer path.
    {
        let mut ls_guard = leader.leader_state.write();
        let ls = ls_guard.as_mut().expect("leader state must exist");
        ls.next_index.insert(4, 1);
        ls.match_index.entry(4).or_insert(0);
    }

    // The leader must detect that node 4 needs a snapshot.
    assert!(
        leader.follower_needs_snapshot(4),
        "leader must identify node 4 as needing a snapshot"
    );

    // Prepare an InstallSnapshot request destined for node 4.
    let snap_req = leader
        .prepare_install_snapshot(4)
        .expect("prepare_install_snapshot must not error")
        .expect("must produce a request for a lagging peer");

    assert_eq!(
        snap_req.last_included_index, 6,
        "snapshot request must cover up to the applied index"
    );
    assert!(
        snap_req.done,
        "small snapshot must complete in a single request"
    );

    // Build a follower (node 4) and install the snapshot.
    // Use a 5-peer config so the odd-nodes validation passes.
    let follower_config = RaftConfig::new(4, vec![1, 2, 3, 4, 5]);
    let follower = RaftNode::new(follower_config).expect("create follower node 4");

    let install_resp = follower
        .handle_install_snapshot(snap_req)
        .expect("handle_install_snapshot must succeed");

    // The follower's term must match the leader's after the snapshot install.
    assert_eq!(
        install_resp.term,
        leader.current_term(),
        "follower term must match leader term post-install"
    );

    // Verify the follower's log reflects the installed snapshot.
    {
        let log = follower.log.read();
        assert_eq!(
            log.snapshot_index(),
            6,
            "follower log snapshot index must be 6 after install"
        );
    }

    Ok(())
}
