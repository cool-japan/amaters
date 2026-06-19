//! Placement driver — deterministic shard placement planning.
//!
//! This module analyses a [`ShardRegistry`] snapshot and produces a
//! [`PlacementPlan`] describing required splits, merges and transfers.
//! No I/O, no mutation — it is a pure function over the registry state.
//! Data movement (executing the plan) is outside this module's scope.

use crate::error::RaftResult;
use crate::shard::{ShardId, ShardMetadata, ShardRegistry, ShardState};
use crate::types::NodeId;
use amaters_core::Key;
use std::collections::BTreeMap;

/// A single placement action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementAction {
    /// Split a hot shard at `split_key`.
    Split {
        /// The shard to split.
        shard_id: ShardId,
        /// The key at which to split the shard.
        split_key: Key,
    },
    /// Merge two adjacent cold shards on the same node.
    Merge {
        /// The left (lower-range) shard.
        left_shard_id: ShardId,
        /// The right (higher-range) shard.
        right_shard_id: ShardId,
    },
    /// Transfer a shard from one node to another to rebalance load.
    Transfer {
        /// The shard to move.
        shard_id: ShardId,
        /// The node currently hosting the shard.
        from_node: NodeId,
        /// The node that should receive the shard.
        to_node: NodeId,
    },
}

/// Thresholds driving placement decisions.
#[derive(Debug, Clone)]
pub struct PlacementPolicy {
    /// Key count above which a shard is "hot" and should be split.
    pub hot_key_threshold: u64,
    /// Size in bytes above which a shard is "hot" and should be split.
    pub hot_size_threshold: u64,
    /// Key count below which a shard is "cold" and may be merged.
    pub cold_key_threshold: u64,
    /// Size in bytes below which a shard is "cold" and may be merged.
    pub cold_size_threshold: u64,
    /// Allowed fractional deviation of a node's shard count from the cluster
    /// mean before a transfer is proposed.  E.g. 0.20 = 20 %.
    pub imbalance_tolerance: f64,
}

impl PlacementPolicy {
    /// Create a new placement policy with explicit thresholds.
    pub fn new(
        hot_key_threshold: u64,
        hot_size_threshold: u64,
        cold_key_threshold: u64,
        cold_size_threshold: u64,
        imbalance_tolerance: f64,
    ) -> Self {
        Self {
            hot_key_threshold,
            hot_size_threshold,
            cold_key_threshold,
            cold_size_threshold,
            imbalance_tolerance,
        }
    }

    /// Sensible defaults for a small cluster.
    pub fn default_policy() -> Self {
        Self::new(100_000, 512 * 1024 * 1024, 1_000, 10 * 1024 * 1024, 0.20)
    }
}

/// An ordered list of placement actions with a no-mutation guarantee.
#[derive(Debug, Clone, Default)]
pub struct PlacementPlan {
    /// The ordered list of actions to execute.
    pub actions: Vec<PlacementAction>,
}

impl PlacementPlan {
    /// Return `true` if the plan contains no actions.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Return the total number of actions in the plan.
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Iterate over split actions only.
    pub fn splits(&self) -> impl Iterator<Item = &PlacementAction> {
        self.actions
            .iter()
            .filter(|a| matches!(a, PlacementAction::Split { .. }))
    }

    /// Iterate over merge actions only.
    pub fn merges(&self) -> impl Iterator<Item = &PlacementAction> {
        self.actions
            .iter()
            .filter(|a| matches!(a, PlacementAction::Merge { .. }))
    }

    /// Iterate over transfer actions only.
    pub fn transfers(&self) -> impl Iterator<Item = &PlacementAction> {
        self.actions
            .iter()
            .filter(|a| matches!(a, PlacementAction::Transfer { .. }))
    }
}

/// Stateless placement coordinator.
///
/// All methods are pure functions over their arguments; no state is mutated.
pub struct PlacementCoordinator {
    policy: PlacementPolicy,
}

impl PlacementCoordinator {
    /// Create a new coordinator with the given policy.
    pub fn new(policy: PlacementPolicy) -> Self {
        Self { policy }
    }

    /// Detect hot shards that need splitting.
    ///
    /// Only [`ShardState::Active`] shards are considered.  The output is
    /// deterministic: shards are examined in ascending `shard_id` order.
    pub fn detect_splits(&self, shards: &[ShardMetadata]) -> Vec<PlacementAction> {
        let mut sorted: Vec<&ShardMetadata> = shards
            .iter()
            .filter(|s| s.state == ShardState::Active)
            .collect();
        sorted.sort_by_key(|s| s.id);

        let mut actions = Vec::new();
        for s in sorted {
            if s.is_hot(
                self.policy.hot_key_threshold,
                self.policy.hot_size_threshold,
            ) {
                let split_key = s.range.midpoint();
                actions.push(PlacementAction::Split {
                    shard_id: s.id,
                    split_key,
                });
            }
        }
        actions
    }

    /// Detect adjacent cold shard pairs that can be merged.
    ///
    /// Two shards are candidates for merging when:
    /// - Both are [`ShardState::Active`].
    /// - Both are cold (below the cold thresholds).
    /// - They reside on the **same** node.
    /// - Their key ranges are adjacent: `left.range.end == right.range.start`.
    ///
    /// Each shard is used in at most one merge action (consume-on-use).
    /// Output order is deterministic: pairs are found by sorting shards by
    /// `(node_id, range.start bytes)` and walking adjacent pairs.
    pub fn detect_merges(&self, shards: &[ShardMetadata]) -> Vec<PlacementAction> {
        // Collect only active, cold shards.
        let mut candidates: Vec<&ShardMetadata> = shards
            .iter()
            .filter(|s| {
                s.state == ShardState::Active
                    && s.is_cold(
                        self.policy.cold_key_threshold,
                        self.policy.cold_size_threshold,
                    )
            })
            .collect();

        // Sort by node_id first, then by range start bytes for adjacency detection.
        candidates.sort_by(|a, b| {
            a.node_id
                .cmp(&b.node_id)
                .then_with(|| a.range.start.as_bytes().cmp(b.range.start.as_bytes()))
        });

        let mut actions = Vec::new();
        let mut consumed = vec![false; candidates.len()];

        for i in 0..candidates.len().saturating_sub(1) {
            if consumed[i] {
                continue;
            }
            let left = candidates[i];
            let right = candidates[i + 1];

            // Adjacent ranges: left.end bytes must equal right.start bytes.
            let adjacent = left.range.end.as_bytes() == right.range.start.as_bytes();
            // Same node.
            let same_node = left.node_id == right.node_id;

            if adjacent && same_node {
                actions.push(PlacementAction::Merge {
                    left_shard_id: left.id,
                    right_shard_id: right.id,
                });
                // Mark both as consumed so they cannot participate in another merge.
                consumed[i] = true;
                consumed[i + 1] = true;
            }
        }

        actions
    }

    /// Produce rebalance transfers using a deterministic greedy algorithm.
    ///
    /// The algorithm:
    /// 1. Build a [`BTreeMap`]`<NodeId, Vec<ShardMetadata>>` for deterministic
    ///    iteration order.
    /// 2. Compute `mean = total_active_shards / node_count`.
    /// 3. Greedily pick the shard with the *smallest* `estimated_keys` from the
    ///    most-over-loaded node and propose a transfer to the most-under-loaded
    ///    node. Tie-break by `node_id` (ascending) for stability.
    /// 4. Stop when no node exceeds `mean * (1 + imbalance_tolerance)`.
    pub fn detect_rebalance(&self, shards: &[ShardMetadata]) -> Vec<PlacementAction> {
        // Only Active shards participate in rebalancing.
        let active: Vec<&ShardMetadata> = shards
            .iter()
            .filter(|s| s.state == ShardState::Active)
            .collect();

        if active.is_empty() {
            return Vec::new();
        }

        // Build node → shard list using BTreeMap for deterministic iteration.
        // We work with owned clones so we can mutate the per-node shard lists.
        let mut by_node: BTreeMap<NodeId, Vec<ShardMetadata>> = BTreeMap::new();
        for s in &active {
            by_node.entry(s.node_id).or_default().push((*s).clone());
        }

        let node_count = by_node.len();
        if node_count < 2 {
            // Cannot rebalance a single-node cluster.
            return Vec::new();
        }

        // Sort each node's shards by estimated_keys ascending for determinism
        // (we always pick the smallest-key-count shard when offloading).
        for shards_on_node in by_node.values_mut() {
            shards_on_node.sort_by_key(|s| (s.estimated_keys, s.id));
        }

        let total_shards = active.len();
        let mean = total_shards as f64 / node_count as f64;
        let over_threshold = mean * (1.0 + self.policy.imbalance_tolerance);

        let mut actions = Vec::new();

        // Bound the loop: at most `total_shards` transfers can improve balance.
        // When n_shards < n_nodes, perfect balance is impossible and the greedy
        // algorithm would oscillate forever without this guard.
        let max_transfers = total_shards + 1;
        let mut transfer_count = 0;

        // Iterate until no node is over-loaded or we reach the transfer budget.
        loop {
            if transfer_count >= max_transfers {
                break;
            }

            // Find the most-over-loaded node (count > over_threshold).
            // Tie-break by node_id (BTreeMap iteration is already sorted ascending,
            // so the first qualifying node is the stable, deterministic choice).
            let over_node = by_node
                .iter()
                .filter(|(_, v)| v.len() as f64 > over_threshold)
                .max_by(|(id_a, v_a), (id_b, v_b)| {
                    v_a.len().cmp(&v_b.len()).then_with(|| id_b.cmp(id_a)) // smaller id wins ties
                })
                .map(|(id, _)| *id);

            let from_node = match over_node {
                Some(id) => id,
                None => break,
            };

            // Find the most-under-loaded node (fewest shards, tie-break smallest id).
            let to_node = by_node
                .iter()
                .filter(|&(&id, _)| id != from_node)
                .min_by(|(id_a, v_a), (id_b, v_b)| {
                    v_a.len().cmp(&v_b.len()).then_with(|| id_a.cmp(id_b)) // smaller id wins ties
                })
                .map(|(id, _)| *id);

            let to_node = match to_node {
                Some(id) => id,
                None => break,
            };

            // Don't transfer if to_node would become over-loaded (no net improvement).
            let to_node_new_len = by_node.get(&to_node).map(|v| v.len() + 1).unwrap_or(1);
            if to_node_new_len as f64 > over_threshold {
                break;
            }

            // Pick the shard with the smallest estimated_keys on the over-loaded node
            // (already sorted ascending, so take the first element).
            let shard_to_move = {
                let v = by_node
                    .get_mut(&from_node)
                    .expect("from_node must be in map");
                if v.is_empty() {
                    break;
                }
                v.remove(0)
            };

            let shard_id = shard_to_move.id;
            by_node
                .get_mut(&to_node)
                .expect("to_node must be in map")
                .push(shard_to_move);

            actions.push(PlacementAction::Transfer {
                shard_id,
                from_node,
                to_node,
            });

            transfer_count += 1;
        }

        actions
    }

    /// Produce a full placement plan.
    ///
    /// Order of actions: splits first (they change shard count), then merges,
    /// then rebalance transfers.
    ///
    /// # Data movement boundary
    ///
    /// This function produces *descriptions only*. Executing the plan —
    /// calling [`crate::shard::ShardSplit::create_shards`],
    /// [`crate::shard::ShardMerge::create_merged_shard`], updating the
    /// registry, and migrating data — is outside this module's scope.
    pub fn plan(&self, registry: &ShardRegistry) -> RaftResult<PlacementPlan> {
        let shards = registry.get_all();
        let mut actions = self.detect_splits(&shards);
        actions.extend(self.detect_merges(&shards));
        actions.extend(self.detect_rebalance(&shards));
        Ok(PlacementPlan { actions })
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use crate::shard::{KeyRange, ShardMetadata, ShardRegistry};
    use crate::types::NodeId;
    use proptest::prelude::*;

    /// Construct a registry with `n_shards` shards, each with 1-byte disjoint ranges.
    /// Each shard is placed on node `(index % n_nodes) + 1` for a round-robin distribution.
    /// The shard's stats come from the `key_counts` and `sizes` slices (modulo length).
    fn build_registry_from_slices(
        n_shards: usize,
        n_nodes: usize,
        key_counts: &[u64],
        sizes: &[u64],
    ) -> ShardRegistry {
        let registry = ShardRegistry::new();
        let max_shards = n_shards.min(254); // keep byte ranges valid
        for i in 0..max_shards {
            let start_byte = i as u8;
            let end_byte = start_byte + 1;
            let range =
                match KeyRange::new(Key::from_slice(&[start_byte]), Key::from_slice(&[end_byte])) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
            let shard_id = registry.allocate_shard_id();
            let node_id: NodeId = (i % n_nodes.max(1)) as NodeId + 1;
            let mut shard = ShardMetadata::new(shard_id, range, node_id);
            let keys = if key_counts.is_empty() {
                0
            } else {
                key_counts[i % key_counts.len()]
            };
            let sz = if sizes.is_empty() {
                0
            } else {
                sizes[i % sizes.len()]
            };
            shard.update_stats(keys, sz);
            let _ = registry.register(shard);
        }
        registry
    }

    proptest! {
        // Reduced case count + smaller input ranges: PlacementCoordinator::plan()
        // runs in unoptimized debug mode where proptest overhead is ~2-5s/case.
        // Ranges are narrowed to what's needed to exercise hot/cold/rebalance thresholds.
        #![proptest_config(ProptestConfig::with_cases(10))]

        #[test]
        fn prop_plan_deterministic(
            n_shards in 0usize..=4usize,
            n_nodes in 1usize..=3usize,
            key_count in 0u64..=2000u64,
            size in 0u64..=2_000_000u64,
        ) {
            // Property: plan() called twice on identical registry state
            // produces identical action lists.
            let registry = build_registry_from_slices(
                n_shards,
                n_nodes,
                &[key_count],
                &[size],
            );
            let policy = PlacementPolicy::default_policy();
            let coord = PlacementCoordinator::new(policy);

            let plan1 = coord.plan(&registry).expect("plan1");
            let plan2 = coord.plan(&registry).expect("plan2");

            prop_assert_eq!(
                plan1.len(),
                plan2.len(),
                "plan() must be deterministic: lengths differ"
            );
            for (a1, a2) in plan1.actions.iter().zip(plan2.actions.iter()) {
                prop_assert_eq!(a1, a2, "plan() actions must be identical on repeated calls");
            }
        }

        #[test]
        fn prop_empty_registry_no_actions(_seed in any::<u64>()) {
            // Property: empty registry always yields an empty plan.
            let registry = ShardRegistry::new();
            let coord = PlacementCoordinator::new(PlacementPolicy::default_policy());
            let plan = coord.plan(&registry).expect("plan on empty registry");
            prop_assert!(
                plan.is_empty(),
                "empty registry must produce no placement actions"
            );
        }

        #[test]
        fn prop_plan_does_not_mutate_registry(
            n_shards in 1usize..=4usize,
            n_nodes in 1usize..=3usize,
        ) {
            // Property: calling plan() must not change the registry's shard count.
            let registry = build_registry_from_slices(n_shards, n_nodes, &[0], &[0]);
            let count_before = registry.count();
            let coord = PlacementCoordinator::new(PlacementPolicy::default_policy());
            coord.plan(&registry).expect("plan must succeed");
            let count_after = registry.count();
            prop_assert_eq!(
                count_before,
                count_after,
                "plan() must not mutate the registry"
            );
        }

        #[test]
        fn prop_no_split_below_hot_threshold(
            n_shards in 1usize..=4usize,
            key_count in 0u64..=99u64,
            size in 0u64..=999u64,
        ) {
            // Property: shards below the hot thresholds generate no Split actions.
            let registry = build_registry_from_slices(
                n_shards,
                1,
                &[key_count],
                &[size],
            );
            // hot_key = 100, hot_size = 1000: all generated shards are below threshold.
            let policy = PlacementPolicy::new(100, 1000, 0, 0, 0.5);
            let coord = PlacementCoordinator::new(policy);
            let plan = coord.plan(&registry).expect("plan");
            let split_count = plan.splits().count();
            prop_assert_eq!(
                split_count,
                0,
                "shards below hot threshold must not produce Split actions"
            );
        }

        #[test]
        fn prop_all_actions_have_valid_shard_ids(
            n_shards in 1usize..=4usize,
            n_nodes in 1usize..=3usize,
            key_count in 0u64..=2000u64,
            size in 0u64..=2_000_000u64,
        ) {
            // Property: every action in the plan references shard IDs that exist
            // in the registry at the time of planning.
            let registry = build_registry_from_slices(
                n_shards,
                n_nodes,
                &[key_count],
                &[size],
            );
            let existing_ids: std::collections::HashSet<u64> = registry
                .get_all()
                .into_iter()
                .map(|s| s.id)
                .collect();
            let policy = PlacementPolicy::default_policy();
            let coord = PlacementCoordinator::new(policy);
            let plan = coord.plan(&registry).expect("plan");

            for action in &plan.actions {
                match action {
                    PlacementAction::Split { shard_id, .. } => {
                        prop_assert!(
                            existing_ids.contains(shard_id),
                            "Split references unknown shard_id {}",
                            shard_id
                        );
                    }
                    PlacementAction::Merge { left_shard_id, right_shard_id } => {
                        prop_assert!(
                            existing_ids.contains(left_shard_id),
                            "Merge left references unknown shard_id {}",
                            left_shard_id
                        );
                        prop_assert!(
                            existing_ids.contains(right_shard_id),
                            "Merge right references unknown shard_id {}",
                            right_shard_id
                        );
                    }
                    PlacementAction::Transfer { shard_id, .. } => {
                        prop_assert!(
                            existing_ids.contains(shard_id),
                            "Transfer references unknown shard_id {}",
                            shard_id
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard::{KeyRange, ShardMetadata, ShardRegistry, ShardState};
    use crate::types::NodeId;

    /// Build a shard with explicit stats.
    ///
    /// Note: [`ShardMetadata::new`] has signature `(id, range, node_id)`.
    fn make_shard(
        id: ShardId,
        node: NodeId,
        range: KeyRange,
        keys: u64,
        size: u64,
    ) -> ShardMetadata {
        let mut s = ShardMetadata::new(id, range, node);
        s.update_stats(keys, size);
        s
    }

    #[test]
    fn test_detect_splits_hot_shard() {
        let policy = PlacementPolicy::new(100, 1000, 10, 100, 0.2);
        let coord = PlacementCoordinator::new(policy);
        let range =
            KeyRange::new(Key::from_slice(&[0u8]), Key::from_slice(&[255u8])).expect("valid range");
        let shard = make_shard(1, 0, range, 200, 2000); // hot on both axes
        let actions = coord.detect_splits(&[shard]);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], PlacementAction::Split { shard_id: 1, .. }),
            "expected split action for shard 1, got {:?}",
            actions[0]
        );
    }

    #[test]
    fn test_detect_no_split_cold_shard() {
        let policy = PlacementPolicy::new(100, 1000, 10, 100, 0.2);
        let coord = PlacementCoordinator::new(policy);
        let range =
            KeyRange::new(Key::from_slice(&[0u8]), Key::from_slice(&[255u8])).expect("valid range");
        let shard = make_shard(1, 0, range, 5, 50); // cold
        let actions = coord.detect_splits(&[shard]);
        assert!(actions.is_empty(), "cold shard must not be split");
    }

    #[test]
    fn test_detect_splits_only_active_shards() {
        // A splitting-state shard above the threshold must be ignored.
        let policy = PlacementPolicy::new(100, 1000, 10, 100, 0.2);
        let coord = PlacementCoordinator::new(policy);
        let range =
            KeyRange::new(Key::from_slice(&[0u8]), Key::from_slice(&[255u8])).expect("valid range");
        let mut shard = make_shard(1, 0, range, 200, 2000);
        shard.state = ShardState::Splitting;
        let actions = coord.detect_splits(&[shard]);
        assert!(actions.is_empty(), "non-active shard must not be split");
    }

    #[test]
    fn test_detect_merges_adjacent_cold_same_node() {
        let policy = PlacementPolicy::new(100, 1000, 50, 500, 0.2);
        let coord = PlacementCoordinator::new(policy);

        // Two adjacent cold shards on the same node.
        let left_range = KeyRange::new(Key::from_slice(&[10u8]), Key::from_slice(&[128u8]))
            .expect("valid range");
        let right_range = KeyRange::new(Key::from_slice(&[128u8]), Key::from_slice(&[200u8]))
            .expect("valid range");

        let left = make_shard(1, 5, left_range, 10, 50);
        let right = make_shard(2, 5, right_range, 10, 50);

        let actions = coord.detect_merges(&[left, right]);
        assert_eq!(actions.len(), 1, "expected one merge action");
        assert!(
            matches!(
                &actions[0],
                PlacementAction::Merge {
                    left_shard_id: 1,
                    right_shard_id: 2,
                }
            ),
            "unexpected merge action: {:?}",
            actions[0]
        );
    }

    #[test]
    fn test_no_merge_different_nodes() {
        // Two adjacent cold shards on *different* nodes must not be merged.
        let policy = PlacementPolicy::new(100, 1000, 50, 500, 0.2);
        let coord = PlacementCoordinator::new(policy);

        let left_range = KeyRange::new(Key::from_slice(&[10u8]), Key::from_slice(&[128u8]))
            .expect("valid range");
        let right_range = KeyRange::new(Key::from_slice(&[128u8]), Key::from_slice(&[200u8]))
            .expect("valid range");

        let left = make_shard(1, 5, left_range, 10, 50);
        let right = make_shard(2, 6, right_range, 10, 50); // different node

        let actions = coord.detect_merges(&[left, right]);
        assert!(actions.is_empty(), "cross-node merge must not be proposed");
    }

    #[test]
    fn test_no_merge_non_adjacent_cold_shards() {
        // Cold shards on the same node but with a gap between them must not
        // be merged.
        let policy = PlacementPolicy::new(100, 1000, 50, 500, 0.2);
        let coord = PlacementCoordinator::new(policy);

        let left_range = KeyRange::new(Key::from_slice(&[10u8]), Key::from_slice(&[100u8]))
            .expect("valid range");
        // gap: [100, 128) is missing
        let right_range = KeyRange::new(Key::from_slice(&[128u8]), Key::from_slice(&[200u8]))
            .expect("valid range");

        let left = make_shard(1, 5, left_range, 10, 50);
        let right = make_shard(2, 5, right_range, 10, 50);

        let actions = coord.detect_merges(&[left, right]);
        assert!(
            actions.is_empty(),
            "non-adjacent cold shards must not be merged"
        );
    }

    #[test]
    fn test_no_merge_hot_adjacent_shards() {
        // Adjacent hot shards on the same node must NOT be merged.
        let policy = PlacementPolicy::new(100, 1000, 50, 500, 0.2);
        let coord = PlacementCoordinator::new(policy);

        let left_range = KeyRange::new(Key::from_slice(&[10u8]), Key::from_slice(&[128u8]))
            .expect("valid range");
        let right_range = KeyRange::new(Key::from_slice(&[128u8]), Key::from_slice(&[200u8]))
            .expect("valid range");

        // Hot shards.
        let left = make_shard(1, 5, left_range, 200, 2000);
        let right = make_shard(2, 5, right_range, 200, 2000);

        let actions = coord.detect_merges(&[left, right]);
        assert!(actions.is_empty(), "hot shards must not be merged");
    }

    #[test]
    fn test_detect_rebalance_imbalanced() {
        // 3 nodes with 7/1/1 distribution -> transfers must be proposed.
        let policy = PlacementPolicy::new(u64::MAX, u64::MAX, 0, 0, 0.2);
        let coord = PlacementCoordinator::new(policy);

        // Create 9 shards: 7 on node 1, 1 on node 2, 1 on node 3.
        // Use non-overlapping ranges of 1 byte each for simplicity.
        let mut shards = Vec::new();
        for i in 0u8..9 {
            let range = KeyRange::new(Key::from_slice(&[i]), Key::from_slice(&[i + 1]))
                .expect("valid range");
            // Distribute 7/1/1: nodes 1, 2, 3 with 7, 1, 1 shards respectively.
            let node: NodeId = if i < 7 {
                1
            } else if i == 7 {
                2
            } else {
                3
            };
            shards.push(make_shard(i as ShardId + 1, node, range, 0, 0));
        }

        let actions = coord.detect_rebalance(&shards);
        assert!(
            !actions.is_empty(),
            "imbalanced cluster must produce transfer actions"
        );
        // All actions must be transfers.
        for a in &actions {
            assert!(
                matches!(a, PlacementAction::Transfer { .. }),
                "unexpected action type: {:?}",
                a
            );
        }
    }

    #[test]
    fn test_detect_rebalance_already_balanced() {
        // 3 nodes, 3 shards each (3/3/3) -> no transfers needed.
        let policy = PlacementPolicy::new(u64::MAX, u64::MAX, 0, 0, 0.2);
        let coord = PlacementCoordinator::new(policy);

        let mut shards = Vec::new();
        for i in 0u8..9 {
            let range = KeyRange::new(Key::from_slice(&[i]), Key::from_slice(&[i + 1]))
                .expect("valid range");
            let node = (i % 3) as u64 + 1;
            shards.push(make_shard(i as ShardId + 1, node, range, 0, 0));
        }

        let actions = coord.detect_rebalance(&shards);
        assert!(
            actions.is_empty(),
            "balanced cluster must not produce transfers, got {:?}",
            actions
        );
    }

    #[test]
    fn test_plan_does_not_mutate_registry() {
        let policy = PlacementPolicy::default_policy();
        let coord = PlacementCoordinator::new(policy);
        let registry = ShardRegistry::new();
        let count_before = registry.count();
        let _ = coord.plan(&registry).expect("plan must succeed");
        assert_eq!(
            registry.count(),
            count_before,
            "registry must not be mutated by plan()"
        );
    }

    #[test]
    fn test_plan_deterministic() {
        // Calling plan() twice on the same registry must produce identical results.
        let policy = PlacementPolicy::new(100, 1000, 10, 100, 0.2);
        let coord = PlacementCoordinator::new(policy);

        let registry = ShardRegistry::new();

        // Register a few shards with different sizes.
        let r1 =
            KeyRange::new(Key::from_slice(&[0u8]), Key::from_slice(&[85u8])).expect("valid range");
        let r2 = KeyRange::new(Key::from_slice(&[85u8]), Key::from_slice(&[170u8]))
            .expect("valid range");
        let r3 = KeyRange::new(Key::from_slice(&[170u8]), Key::from_slice(&[255u8]))
            .expect("valid range");

        let s1 = make_shard(1, 1, r1, 200, 5000); // hot
        let s2 = make_shard(2, 2, r2, 5, 10); // cold
        let s3 = make_shard(3, 3, r3, 5, 10); // cold

        registry.register(s1).expect("register s1");
        registry.register(s2).expect("register s2");
        registry.register(s3).expect("register s3");

        let plan1 = coord.plan(&registry).expect("plan 1");
        let plan2 = coord.plan(&registry).expect("plan 2");

        assert_eq!(
            plan1.len(),
            plan2.len(),
            "plan length must be deterministic"
        );

        // Compare each action structurally.
        for (a1, a2) in plan1.actions.iter().zip(plan2.actions.iter()) {
            assert_eq!(a1, a2, "action mismatch between plan1 and plan2");
        }
    }

    #[test]
    fn test_plan_empty_registry() {
        let policy = PlacementPolicy::default_policy();
        let coord = PlacementCoordinator::new(policy);
        let registry = ShardRegistry::new();
        let plan = coord
            .plan(&registry)
            .expect("plan must succeed on empty registry");
        assert!(plan.is_empty(), "empty registry must produce an empty plan");
    }

    #[test]
    fn test_placement_plan_iterators() {
        // Verify the splits/merges/transfers helpers filter correctly.
        let plan = PlacementPlan {
            actions: vec![
                PlacementAction::Split {
                    shard_id: 1,
                    split_key: Key::from_slice(&[128u8]),
                },
                PlacementAction::Merge {
                    left_shard_id: 2,
                    right_shard_id: 3,
                },
                PlacementAction::Transfer {
                    shard_id: 4,
                    from_node: 1,
                    to_node: 2,
                },
            ],
        };

        assert_eq!(plan.splits().count(), 1);
        assert_eq!(plan.merges().count(), 1);
        assert_eq!(plan.transfers().count(), 1);
        assert_eq!(plan.len(), 3);
        assert!(!plan.is_empty());
    }
}
