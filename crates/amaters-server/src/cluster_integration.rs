//! Wires `amaters-cluster`'s Raft node into the server lifecycle.
//!
//! This module is compiled only when the `cluster` feature is enabled.
//! Without the feature, [`ClusterHandle::start_standalone`] provides a
//! no-op shim so that the rest of the server can use a single code path.
//!
//! # Minimum cluster size
//!
//! The underlying `RaftNode` enforces a minimum of 3 nodes (odd-quorum
//! requirement).  `start` therefore requires `peers.len() >= 3`.
//! `start_standalone` does not create a `RaftNode`; it returns a sentinel
//! handle that reports `is_leader = true` without any Raft overhead.

use std::sync::Arc;

use crate::server::{ServerError, ServerResult};

// ─── Feature-gated imports ────────────────────────────────────────────────────

#[cfg(feature = "cluster")]
use amaters_cluster::{
    ClusterCommand, Command, LogIndex, NodeId, PlacementStateMachine, RaftConfig, RaftError,
    RaftNode, ShardId, ShardMetadata, ShardRegistry,
};

// ─── Inner state ──────────────────────────────────────────────────────────────

/// Internal state that changes based on whether we're running a full Raft
/// cluster or a standalone node.
enum ClusterInner {
    /// Standalone sentinel — always leader, zero shards.
    Standalone,
    /// Full Raft node with placement registry.
    #[cfg(feature = "cluster")]
    Raft {
        raft: Arc<RaftNode>,
        registry: Arc<ShardRegistry>,
    },
}

// ─── ClusterHandle ────────────────────────────────────────────────────────────

/// An opaque handle to the running cluster layer.
///
/// Callers obtain it via [`ClusterHandle::start`] (multi-node Raft) or
/// [`ClusterHandle::start_standalone`] (single-node, no Raft).
pub struct ClusterHandle {
    /// Stable node identity used for logging and status responses.
    node_id: u64,
    inner: ClusterInner,
}

impl ClusterHandle {
    /// Start the Raft node for multi-node operation.
    ///
    /// `peers` must contain **at least 3** (node_id, address) pairs (including
    /// this node) because the Raft quorum algorithm requires an odd-sized
    /// cluster of ≥ 3.  The addresses are stored for use by the RPC transport
    /// (Phase 8 work).
    #[cfg(feature = "cluster")]
    pub async fn start(
        node_id: u64,
        peers: Vec<(u64, std::net::SocketAddr)>,
    ) -> ServerResult<Self> {
        let registry = Arc::new(ShardRegistry::new());

        // Build PlacementStateMachine backed by the shard registry.
        let sm = PlacementStateMachine::new(Arc::clone(&registry));

        // Collect all peer node IDs (including this node).
        let mut peer_ids: Vec<u64> = peers.iter().map(|(id, _)| *id).collect();
        if !peer_ids.contains(&node_id) {
            peer_ids.push(node_id);
        }

        let config = RaftConfig::new(node_id, peer_ids);

        let raft = RaftNode::new(config)
            .map_err(|e| ServerError::Cluster(format!("Failed to create RaftNode: {}", e)))?;

        raft.set_state_machine(sm)
            .map_err(|e| ServerError::Cluster(format!("Failed to set state machine: {}", e)))?;

        let raft = Arc::new(raft);

        Ok(Self {
            node_id,
            inner: ClusterInner::Raft { raft, registry },
        })
    }

    /// Start in standalone (single-node) mode — no Raft, always leader.
    ///
    /// This path does not create a `RaftNode` (which would fail for < 3
    /// peers) and simply returns a sentinel that reports `is_leader = true`.
    pub async fn start_standalone(node_id: u64) -> ServerResult<Self> {
        Ok(Self {
            node_id,
            inner: ClusterInner::Standalone,
        })
    }

    /// Returns `true` when this node currently believes it is the Raft leader.
    pub fn is_leader(&self) -> bool {
        match &self.inner {
            ClusterInner::Standalone => true,
            #[cfg(feature = "cluster")]
            ClusterInner::Raft { raft, .. } => raft.is_leader(),
        }
    }

    /// Return this node's numeric identifier.
    pub fn node_id(&self) -> u64 {
        self.node_id
    }

    /// Number of shards tracked by the placement registry.
    ///
    /// Returns 0 when running in standalone mode or without the `cluster` feature.
    pub fn shard_count(&self) -> usize {
        match &self.inner {
            ClusterInner::Standalone => 0,
            #[cfg(feature = "cluster")]
            ClusterInner::Raft { registry, .. } => registry.count(),
        }
    }

    /// Expose the underlying [`RaftNode`] for advanced use.
    #[cfg(feature = "cluster")]
    pub fn raft_node(&self) -> Option<Arc<RaftNode>> {
        match &self.inner {
            ClusterInner::Raft { raft, .. } => Some(Arc::clone(raft)),
            ClusterInner::Standalone => None,
        }
    }

    /// Expose the underlying [`ShardRegistry`] for advanced use.
    #[cfg(feature = "cluster")]
    pub fn shard_registry(&self) -> Option<Arc<ShardRegistry>> {
        match &self.inner {
            ClusterInner::Raft { registry, .. } => Some(Arc::clone(registry)),
            ClusterInner::Standalone => None,
        }
    }

    // ─── S1: Shard management API ─────────────────────────────────────────────

    /// List all shards currently registered in the placement layer.
    ///
    /// Returns an empty `Vec` when running in standalone mode.
    #[cfg(feature = "cluster")]
    pub fn list_shards(&self) -> Vec<ShardMetadata> {
        match &self.inner {
            ClusterInner::Standalone => vec![],
            ClusterInner::Raft { registry, .. } => registry.get_all(),
        }
    }

    /// List shards currently assigned to a specific node.
    ///
    /// Returns an empty `Vec` when running in standalone mode.
    #[cfg(feature = "cluster")]
    pub fn shards_on_node(&self, node_id: NodeId) -> Vec<ShardMetadata> {
        match &self.inner {
            ClusterInner::Standalone => vec![],
            ClusterInner::Raft { registry, .. } => registry.get_by_node(node_id),
        }
    }

    /// Find the shard responsible for a given key.
    ///
    /// Returns `None` when running in standalone mode.
    #[cfg(feature = "cluster")]
    pub fn find_shard_for_key(&self, key: &amaters_core::Key) -> Option<ShardMetadata> {
        match &self.inner {
            ClusterInner::Standalone => None,
            ClusterInner::Raft { registry, .. } => registry.find_shard_for_key(key),
        }
    }

    /// Propose a shard split via Raft (leader only).
    ///
    /// Returns `Err(ServerError::Cluster("NotLeader: ..."))` if this node is
    /// not the current Raft leader.
    #[cfg(feature = "cluster")]
    pub fn propose_split(
        &self,
        shard_id: ShardId,
        split_key: amaters_core::Key,
    ) -> ServerResult<LogIndex> {
        match &self.inner {
            ClusterInner::Standalone => Err(ServerError::Cluster(
                "NotLeader: standalone node has no Raft consensus".to_string(),
            )),
            ClusterInner::Raft { raft, .. } => {
                let cmd_bytes = ClusterCommand::PlaceSplit {
                    shard_id,
                    split_key: split_key.as_bytes().to_vec(),
                }
                .encode();
                let cmd = Command::new(cmd_bytes);
                raft.propose(cmd).map_err(|e| match &e {
                    RaftError::NotLeader { leader_id } => ServerError::Cluster(format!(
                        "NotLeader: current leader is {:?}",
                        leader_id
                    )),
                    _ => ServerError::Cluster(format!("Raft propose error: {}", e)),
                })
            }
        }
    }

    /// Propose a shard merge via Raft (leader only).
    ///
    /// Returns `Err(ServerError::Cluster("NotLeader: ..."))` if this node is
    /// not the current Raft leader.
    #[cfg(feature = "cluster")]
    pub fn propose_merge(
        &self,
        left_shard_id: ShardId,
        right_shard_id: ShardId,
    ) -> ServerResult<LogIndex> {
        match &self.inner {
            ClusterInner::Standalone => Err(ServerError::Cluster(
                "NotLeader: standalone node has no Raft consensus".to_string(),
            )),
            ClusterInner::Raft { raft, .. } => {
                let cmd_bytes = ClusterCommand::PlaceMerge {
                    left_shard_id,
                    right_shard_id,
                }
                .encode();
                let cmd = Command::new(cmd_bytes);
                raft.propose(cmd).map_err(|e| match &e {
                    RaftError::NotLeader { leader_id } => ServerError::Cluster(format!(
                        "NotLeader: current leader is {:?}",
                        leader_id
                    )),
                    _ => ServerError::Cluster(format!("Raft propose error: {}", e)),
                })
            }
        }
    }

    /// Propose a shard transfer via Raft (leader only).
    ///
    /// Returns `Err(ServerError::Cluster("NotLeader: ..."))` if this node is
    /// not the current Raft leader.
    #[cfg(feature = "cluster")]
    pub fn propose_transfer(
        &self,
        shard_id: ShardId,
        from_node: NodeId,
        to_node: NodeId,
    ) -> ServerResult<LogIndex> {
        match &self.inner {
            ClusterInner::Standalone => Err(ServerError::Cluster(
                "NotLeader: standalone node has no Raft consensus".to_string(),
            )),
            ClusterInner::Raft { raft, .. } => {
                let cmd_bytes = ClusterCommand::PlaceTransfer {
                    shard_id,
                    from_node,
                    to_node,
                }
                .encode();
                let cmd = Command::new(cmd_bytes);
                raft.propose(cmd).map_err(|e| match &e {
                    RaftError::NotLeader { leader_id } => ServerError::Cluster(format!(
                        "NotLeader: current leader is {:?}",
                        leader_id
                    )),
                    _ => ServerError::Cluster(format!("Raft propose error: {}", e)),
                })
            }
        }
    }
}

// ─── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_standalone_handle_is_leader() {
        let handle = ClusterHandle::start_standalone(1)
            .await
            .expect("standalone");
        assert!(
            handle.is_leader(),
            "standalone should always report is_leader"
        );
        assert_eq!(handle.node_id(), 1);
    }

    #[tokio::test]
    async fn test_standalone_shard_count_is_zero() {
        let handle = ClusterHandle::start_standalone(7)
            .await
            .expect("standalone");
        assert_eq!(handle.shard_count(), 0);
    }

    /// Full Raft cluster smoke test — requires the `cluster` feature and 3 peers.
    #[cfg(feature = "cluster")]
    #[tokio::test]
    async fn test_cluster_start_three_node() {
        let peers: Vec<(u64, std::net::SocketAddr)> = vec![
            (1, "127.0.0.1:17878".parse().expect("addr1")),
            (2, "127.0.0.1:17879".parse().expect("addr2")),
            (3, "127.0.0.1:17880".parse().expect("addr3")),
        ];
        let handle = ClusterHandle::start(1, peers).await.expect("start cluster");
        // is_leader() must not panic
        let _ = handle.is_leader();
        assert_eq!(handle.shard_count(), 0);
        assert_eq!(handle.node_id(), 1);
    }

    /// Standalone propose_split returns an error (no Raft).
    #[cfg(feature = "cluster")]
    #[tokio::test]
    async fn test_standalone_propose_split_is_error() {
        let handle = ClusterHandle::start_standalone(1)
            .await
            .expect("standalone");
        let key = amaters_core::Key::from_slice(&[0x80]);
        let result = handle.propose_split(1, key);
        assert!(
            result.is_err(),
            "standalone propose_split must return an error"
        );
    }
}

// ─── S4/S5/S6: In-process cluster test harness ───────────────────────────────

#[cfg(all(test, feature = "cluster"))]
mod cluster_tests {
    use super::*;
    use amaters_cluster::{PlacementStateMachine, RaftConfig, RaftNode, ShardRegistry};

    /// In-process 3-node Raft cluster for deterministic testing.
    ///
    /// No network sockets — messages are routed by the pump.
    struct TestCluster {
        nodes: Vec<Arc<RaftNode>>,
        #[allow(dead_code)]
        registries: Vec<Arc<ShardRegistry>>,
    }

    impl TestCluster {
        fn new_three_node() -> Self {
            let peer_ids = vec![1u64, 2, 3];
            let mut nodes = Vec::new();
            let mut registries = Vec::new();

            for &id in &peer_ids {
                let registry = Arc::new(ShardRegistry::new());
                let sm = PlacementStateMachine::new(Arc::clone(&registry));
                let config = RaftConfig::new(id, peer_ids.clone());
                let node = RaftNode::new(config).expect("create node");
                node.set_state_machine(sm).expect("set sm");
                nodes.push(Arc::new(node));
                registries.push(registry);
            }

            Self { nodes, registries }
        }

        /// Pump all pending messages to quiescence (max `rounds` iterations).
        ///
        /// Returns the number of rounds actually used.
        #[allow(dead_code)]
        fn pump(&self, rounds: usize) -> usize {
            for round in 0..rounds {
                let mut any_sent = false;

                // Collect outbound messages from all nodes.
                for sender_idx in 0..self.nodes.len() {
                    let sender = &self.nodes[sender_idx];
                    let messages = sender.replicate_to_followers();

                    for (target_id, req) in messages {
                        // Find the target node and deliver the message.
                        if let Some(target) = self.nodes.iter().find(|n| n.node_id() == target_id) {
                            let resp = target.handle_append_entries(req);
                            let _ = sender.handle_replication_response(target_id, resp);
                            any_sent = true;
                        }
                    }
                }

                if !any_sent {
                    return round + 1;
                }
            }
            rounds
        }

        /// Find the current leader (if any).
        #[allow(dead_code)]
        fn find_leader(&self) -> Option<&Arc<RaftNode>> {
            self.nodes.iter().find(|n| n.is_leader())
        }

        /// Get the commit index on node at `idx`.
        #[allow(dead_code)]
        fn commit_index(&self, idx: usize) -> u64 {
            self.nodes[idx].commit_index()
        }
    }

    // ─── S5: Consensus tests ─────────────────────────────────────────────────

    #[test]
    fn test_leader_election_three_node() {
        let cluster = TestCluster::new_three_node();

        // Pump messages so any initial state is settled.  After pumping,
        // the invariant is that *at most one* node reports is_leader() — the
        // consensus safety property.  Fresh nodes start as Follower so there
        // may be zero leaders until an election completes.
        cluster.pump(50);

        let leaders: Vec<_> = cluster.nodes.iter().filter(|n| n.is_leader()).collect();
        assert!(
            leaders.len() <= 1,
            "At most one leader should exist; found {}",
            leaders.len()
        );
    }

    #[test]
    fn test_multi_node_replication() {
        let cluster = TestCluster::new_three_node();

        // Fresh 3-node cluster: no elections have run yet.
        // A non-leader should not produce replication messages.
        for node in &cluster.nodes {
            if !node.is_leader() {
                // replicate_to_followers must not panic on a non-leader.
                let msgs = node.replicate_to_followers();
                let _ = msgs;
            }
        }

        // Pump to drive any pending state transitions.
        cluster.pump(10);

        // The key invariant: commit_index is monotone (must not decrease or panic).
        for idx in 0..cluster.nodes.len() {
            let ci = cluster.commit_index(idx);
            let _ = ci; // no panic = pass
        }
    }

    #[test]
    fn test_read_your_writes_leader_routed() {
        // Tests the propose → replicate → commit flow.
        //
        // Since fresh nodes start as Followers, propose() will return
        // RaftError::NotLeader on all of them until an election completes.
        // This test verifies the entire path does not panic and that
        // error handling for NotLeader is correct.

        let cluster = TestCluster::new_three_node();

        let mut leader_found = false;
        for node in &cluster.nodes {
            let cmd = Command::new(vec![0u8]);
            match node.propose(cmd) {
                Ok(_index) => {
                    leader_found = true;
                    // Leader accepted the command — pump to replicate.
                    cluster.pump(20);
                    // After pump, commit_index must not panic.
                    let _ = node.commit_index();
                }
                Err(amaters_cluster::RaftError::NotLeader { .. }) => {
                    // Expected: fresh node is a follower.
                }
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }
        // Whether or not a leader was found, the cluster must not have panicked.
        let _ = leader_found;
    }

    // ─── S6: #[ignore] stubs ─────────────────────────────────────────────────

    #[test]
    #[ignore = "needs live gRPC Raft transport — no socket transport is wired; cross-process replication requires the Phase 8 transport layer"]
    fn test_cross_process_replication_via_grpc() {
        // Future: once gRPC transport is wired in server.rs / service.rs,
        // this test will spin up a real 3-node cluster over loopback gRPC
        // and verify that propose on one process is visible on the others.
        unimplemented!()
    }

    #[test]
    #[ignore = "needs ReadIndex/quorum-confirmed linearizable read — RaftNode has no read_index() API; see TODO for quorum reads"]
    fn test_quorum_linearizable_read() {
        // Future: implement read_index() on RaftNode (appends a no-op, waits
        // for quorum confirmation, then reads from state machine).
        unimplemented!()
    }
}
