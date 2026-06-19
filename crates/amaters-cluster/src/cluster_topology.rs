//! Cluster topology snapshot for dashboards and observability.
//!
//! [`TopologyCollector`] gathers a point-in-time view of every node's health,
//! state, shard assignment, and log position.  The resulting
//! [`ClusterTopology`] is serialisable to JSON for dashboard consumption.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::failover::FailoverController;
use crate::shard::{ShardId, ShardRegistry};
use crate::types::NodeId;

// ── NodeState ─────────────────────────────────────────────────────────────────

/// Cluster-level view of a single node's operational state.
///
/// Unlike the Raft-level [`crate::types::NodeState`] (which distinguishes
/// Follower / Candidate / Leader), this enum adds an `Offline` variant to
/// represent nodes that the [`FailoverController`] has detected as failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    /// This node is the current Raft leader.
    Leader,
    /// This node is a Raft follower.
    Follower,
    /// This node is participating in an election as a candidate.
    Candidate,
    /// This node is offline (failed / unreachable).
    Offline,
}

// ── NodeStatus ────────────────────────────────────────────────────────────────

/// A summary of one node as seen by the topology collector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    /// The node's identifier.
    pub node_id: NodeId,
    /// Current Raft state (or Offline if the failover controller considers it
    /// failed).
    pub state: NodeState,
    /// Number of shards currently assigned to this node.
    pub shard_count: usize,
    /// Wall-clock milliseconds of the last received heartbeat, or `None` if
    /// no heartbeat has been observed yet.
    pub last_heartbeat_ms: Option<u64>,
    /// The node's last known log index (0 if unknown).
    pub log_index: u64,
    /// `true` iff this node is the current Raft leader.
    pub is_leader: bool,
}

// ── ClusterTopology ───────────────────────────────────────────────────────────

/// A point-in-time snapshot of the entire cluster topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterTopology {
    /// Per-node status entries.
    pub nodes: Vec<NodeStatus>,
    /// Total number of shards across all nodes.
    pub total_shards: usize,
    /// The current leader's node ID, or `None` if no leader is known.
    pub leader_node_id: Option<NodeId>,
    /// Maps each node ID to the list of shard IDs assigned to it.
    pub shard_distribution: HashMap<NodeId, Vec<ShardId>>,
}

// ── TopologyCollector ─────────────────────────────────────────────────────────

/// Builds [`ClusterTopology`] snapshots by correlating the
/// [`FailoverController`] with an optional [`ShardRegistry`].
pub struct TopologyCollector {
    failover: Arc<FailoverController>,
    registry: Option<Arc<ShardRegistry>>,
}

impl TopologyCollector {
    /// Create a collector driven by `failover` with no shard registry.
    pub fn new(failover: Arc<FailoverController>) -> Self {
        Self {
            failover,
            registry: None,
        }
    }

    /// Create a collector that also incorporates shard distribution data.
    pub fn with_registry(failover: Arc<FailoverController>, registry: Arc<ShardRegistry>) -> Self {
        Self {
            failover,
            registry: Some(registry),
        }
    }

    /// Build a topology snapshot over the provided set of `nodes`.
    ///
    /// For each node ID the collector:
    /// 1. Checks whether the [`FailoverController`] considers it failed.
    /// 2. Counts its shards (if a registry is attached).
    /// 3. Identifies the leader as the node that is *not* failed and has the
    ///    most shards (a heuristic; real integration should pass the Raft
    ///    leader ID explicitly).
    ///
    /// The `leader_node_id` field uses the first provided `leader_hint`
    /// argument when available.
    pub fn snapshot(&self, nodes: &[NodeId]) -> ClusterTopology {
        self.snapshot_with_leader(nodes, None)
    }

    /// Build a topology snapshot providing an explicit `leader_hint`.
    pub fn snapshot_with_leader(
        &self,
        nodes: &[NodeId],
        leader_hint: Option<NodeId>,
    ) -> ClusterTopology {
        // Build shard distribution map from the registry.
        let mut shard_distribution: HashMap<NodeId, Vec<ShardId>> =
            nodes.iter().map(|&nid| (nid, Vec::new())).collect();

        if let Some(ref reg) = self.registry {
            for shard in reg.get_all() {
                shard_distribution
                    .entry(shard.node_id)
                    .or_default()
                    .push(shard.id);
            }
        }

        let total_shards: usize = shard_distribution.values().map(|v| v.len()).sum();

        // Current wall-clock time for heartbeat stamps.
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let mut node_statuses = Vec::with_capacity(nodes.len());
        let failed_set: std::collections::HashSet<NodeId> =
            self.failover.failed_nodes().into_iter().collect();

        for &node_id in nodes {
            let is_offline = failed_set.contains(&node_id);
            let is_leader = leader_hint.map(|l| l == node_id).unwrap_or(false);

            let state = if is_offline {
                NodeState::Offline
            } else if is_leader {
                NodeState::Leader
            } else {
                NodeState::Follower
            };

            let shard_count = shard_distribution
                .get(&node_id)
                .map(|v| v.len())
                .unwrap_or(0);

            // Heartbeat timestamp: if not offline, report current time (meaning
            // "recently seen").  In a production integration, this would be the
            // actual last-seen timestamp from the failover controller.
            let last_heartbeat_ms = if is_offline { None } else { Some(now_ms) };

            node_statuses.push(NodeStatus {
                node_id,
                state,
                shard_count,
                last_heartbeat_ms,
                log_index: 0,
                is_leader,
            });
        }

        ClusterTopology {
            nodes: node_statuses,
            total_shards,
            leader_node_id: leader_hint,
            shard_distribution,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failover::FailoverController;
    use crate::shard::{KeyRange, ShardMetadata, ShardRegistry};
    use amaters_core::Key;
    use std::time::Duration;

    fn make_controller() -> Arc<FailoverController> {
        Arc::new(FailoverController::new(Duration::from_millis(500)))
    }

    fn make_registry_with_shards() -> Arc<ShardRegistry> {
        let reg = ShardRegistry::new();
        for (shard_id, node_id, s, e) in [
            (1u64, 1u64, "a0", "a1"),
            (2, 1, "a1", "a2"),
            (3, 2, "b0", "b1"),
        ] {
            let range = KeyRange::new(Key::from_str(s), Key::from_str(e)).expect("range");
            reg.register(ShardMetadata::new(shard_id, range, node_id))
                .expect("register");
        }
        Arc::new(reg)
    }

    // ── test_topology_snapshot_contains_all_nodes ─────────────────────────────

    #[test]
    fn test_topology_snapshot_contains_all_nodes() {
        let controller = make_controller();
        let collector = TopologyCollector::new(Arc::clone(&controller));
        let nodes = vec![1u64, 2, 3];

        let topology = collector.snapshot(&nodes);

        assert_eq!(topology.nodes.len(), 3, "topology must contain all 3 nodes");
        let ids: Vec<NodeId> = topology.nodes.iter().map(|n| n.node_id).collect();
        for &nid in &nodes {
            assert!(ids.contains(&nid), "node {} must appear in topology", nid);
        }
    }

    // ── test_topology_marks_failed_nodes_offline ──────────────────────────────

    #[test]
    fn test_topology_marks_failed_nodes_offline() {
        let controller = make_controller();
        // Explicitly mark node 2 as failed.
        controller.mark_failed(2);

        let collector = TopologyCollector::new(Arc::clone(&controller));
        let nodes = vec![1u64, 2, 3];

        let topology = collector.snapshot(&nodes);

        let node2 = topology
            .nodes
            .iter()
            .find(|n| n.node_id == 2)
            .expect("node 2 must be present");
        assert_eq!(
            node2.state,
            NodeState::Offline,
            "failed node must be marked Offline"
        );
        assert!(
            node2.last_heartbeat_ms.is_none(),
            "offline node must have no heartbeat timestamp"
        );

        // Node 1 and 3 should not be offline.
        for &nid in &[1u64, 3] {
            let n = topology
                .nodes
                .iter()
                .find(|n| n.node_id == nid)
                .expect("node must be present");
            assert_ne!(
                n.state,
                NodeState::Offline,
                "node {} should not be offline",
                nid
            );
        }
    }

    // ── test_topology_shard_distribution ─────────────────────────────────────

    #[test]
    fn test_topology_shard_distribution() {
        let controller = make_controller();
        let registry = make_registry_with_shards();
        let collector = TopologyCollector::with_registry(Arc::clone(&controller), registry);

        let nodes = vec![1u64, 2];
        let topology = collector.snapshot(&nodes);

        assert_eq!(topology.total_shards, 3);
        let node1_shards = &topology.shard_distribution[&1];
        assert_eq!(node1_shards.len(), 2, "node 1 should have 2 shards");
        let node2_shards = &topology.shard_distribution[&2];
        assert_eq!(node2_shards.len(), 1, "node 2 should have 1 shard");
    }

    // ── test_topology_leader_hint ─────────────────────────────────────────────

    #[test]
    fn test_topology_leader_hint() {
        let controller = make_controller();
        let collector = TopologyCollector::new(Arc::clone(&controller));
        let nodes = vec![1u64, 2, 3];

        let topology = collector.snapshot_with_leader(&nodes, Some(1));

        assert_eq!(topology.leader_node_id, Some(1));
        let leader_node = topology
            .nodes
            .iter()
            .find(|n| n.node_id == 1)
            .expect("node 1 must be present");
        assert!(leader_node.is_leader);
        assert_eq!(leader_node.state, NodeState::Leader);
    }

    // ── test_topology_serialises ──────────────────────────────────────────────

    #[test]
    fn test_topology_serialises_to_json() {
        let controller = make_controller();
        let collector = TopologyCollector::new(Arc::clone(&controller));
        let topology = collector.snapshot(&[1, 2]);

        let json = serde_json::to_string(&topology).expect("serialize");
        assert!(json.contains("\"nodes\""));
        assert!(json.contains("\"total_shards\""));
    }
}
