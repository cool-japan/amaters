//! Background placement scheduling task.
//!
//! [`PlacementScheduler`] runs as an async task on the Raft leader.  On each
//! tick it calls [`PlacementCoordinator::plan`] against the current
//! [`ShardRegistry`] and proposes each resulting [`crate::placement::PlacementAction`] as a
//! Raft log entry via [`RaftNode::propose`].
//!
//! # State machine boundary
//!
//! The scheduler is **purely additive**: it reads the registry, generates a
//! placement plan, and writes `PlaceSplit` / `PlaceMerge` / `PlaceTransfer`
//! entries into the Raft log.  It does **not** execute the plan — the state
//! machine that applies committed placement entries (calling
//! [`crate::shard::ShardSplit::create_shards`],
//! [`crate::shard::ShardMerge::create_merged_shard`], updating the registry,
//! and migrating data) is a separate future-phase concern.
//!
//! # Lifecycle
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use parking_lot::RwLock;
//! use amaters_cluster::{
//!     PlacementScheduler, PlacementSchedulerConfig,
//!     placement::PlacementPolicy,
//!     shard::ShardRegistry,
//!     node::RaftNode,
//! };
//!
//! let node = Arc::new(RaftNode::new(config)?);
//! let registry = Arc::new(RwLock::new(ShardRegistry::new()));
//! let scheduler = PlacementScheduler::new(
//!     node.clone(),
//!     registry.clone(),
//!     PlacementPolicy::default_policy(),
//!     PlacementSchedulerConfig::default(),
//! );
//! let handle = scheduler.handle();
//! tokio::spawn(scheduler.run());
//! // ... later, to stop:
//! handle.stop();
//! ```

use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::cluster_command::ClusterCommand;
use crate::log::Command;
use crate::node::RaftNode;
use crate::placement::{PlacementCoordinator, PlacementPolicy};
use crate::shard::ShardRegistry;

// ── PlacementSchedulerConfig ──────────────────────────────────────────────────

/// Tuning parameters for the placement scheduler loop.
#[derive(Debug, Clone)]
pub struct PlacementSchedulerConfig {
    /// How often to run the placement evaluation loop.
    ///
    /// Shorter intervals increase cluster responsiveness to load changes but
    /// may generate bursts of Raft log entries.  The default of 30 seconds is
    /// appropriate for most production clusters.
    pub tick_interval: Duration,

    /// Maximum number of placement actions to propose per tick.
    ///
    /// Capping per-tick proposals prevents flooding the Raft log during
    /// rebalancing storms (e.g., after adding many nodes at once).  Outstanding
    /// actions are proposed on subsequent ticks until the plan is drained.
    pub max_actions_per_tick: usize,

    /// Minimum imbalance ratio required before proposing rebalancing actions.
    ///
    /// Computed as `(max_shards_per_node - min_shards_per_node) / mean_shards_per_node`.
    /// If the current imbalance is below this value, the rebalancing step is skipped.
    /// Default: 0.2 (20% deviation from mean triggers rebalancing).
    pub imbalance_threshold: f64,
}

impl Default for PlacementSchedulerConfig {
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_secs(30),
            max_actions_per_tick: 5,
            imbalance_threshold: 0.2,
        }
    }
}

/// Compute the cluster imbalance ratio.
///
/// Returns `(max_shards_per_node - min_shards_per_node) / mean_shards_per_node`.
/// Returns `0.0` if there are fewer than 2 nodes or no shards.
fn compute_imbalance(registry: &ShardRegistry) -> f64 {
    use std::collections::HashMap;

    let shards = registry.get_all();
    if shards.is_empty() {
        return 0.0;
    }

    let mut counts: HashMap<crate::types::NodeId, usize> = HashMap::new();
    for shard in &shards {
        *counts.entry(shard.node_id).or_insert(0) += 1;
    }

    if counts.len() < 2 {
        return 0.0;
    }

    let max = counts.values().copied().max().unwrap_or(0) as f64;
    let min = counts.values().copied().min().unwrap_or(0) as f64;
    let mean = shards.len() as f64 / counts.len() as f64;

    if mean == 0.0 {
        return 0.0;
    }

    (max - min) / mean
}

// ── PlacementSchedulerHandle ──────────────────────────────────────────────────

/// A cheaply-cloneable handle to a running [`PlacementScheduler`].
///
/// Calling [`stop`][PlacementSchedulerHandle::stop] causes the scheduler's
/// [`run`][PlacementScheduler::run] loop to return after the current tick
/// finishes — it does not interrupt mid-tick execution.
#[derive(Debug, Clone)]
pub struct PlacementSchedulerHandle {
    stop: Arc<Notify>,
}

impl PlacementSchedulerHandle {
    /// Signal the scheduler to stop after the current tick completes.
    ///
    /// This method is idempotent: calling it multiple times is safe.
    pub fn stop(&self) {
        self.stop.notify_one();
    }
}

// ── PlacementScheduler ────────────────────────────────────────────────────────

/// Async background task that drives shard placement on the Raft leader.
///
/// On each tick the scheduler:
/// 1. Checks whether *this* node is currently the Raft leader; if not, it skips
///    the tick silently.  This makes it safe to run a scheduler on every node
///    in the cluster — only the leader will actually write to the log.
/// 2. Acquires a read lock on the [`ShardRegistry`] and calls
///    [`PlacementCoordinator::plan`] to compute the required placement actions.
/// 3. Proposes up to [`PlacementSchedulerConfig::max_actions_per_tick`] actions
///    to the Raft log via [`RaftNode::propose`].  If `propose` returns
///    [`crate::error::RaftError::NotLeader`] the scheduler stops proposing for
///    this tick (we stepped down mid-tick).
pub struct PlacementScheduler {
    node: Arc<RaftNode>,
    registry: Arc<RwLock<ShardRegistry>>,
    coordinator: PlacementCoordinator,
    config: PlacementSchedulerConfig,
    stop: Arc<Notify>,
}

impl PlacementScheduler {
    /// Construct a new scheduler.
    ///
    /// The scheduler does not start running until [`run`][Self::run] is called
    /// (typically via `tokio::spawn`).
    pub fn new(
        node: Arc<RaftNode>,
        registry: Arc<RwLock<ShardRegistry>>,
        policy: PlacementPolicy,
        config: PlacementSchedulerConfig,
    ) -> Self {
        Self {
            node,
            registry,
            coordinator: PlacementCoordinator::new(policy),
            config,
            stop: Arc::new(Notify::new()),
        }
    }

    /// Return a [`PlacementSchedulerHandle`] that can be used to stop the loop.
    ///
    /// The handle is cheap to clone and can be shared across threads.
    pub fn handle(&self) -> PlacementSchedulerHandle {
        PlacementSchedulerHandle {
            stop: Arc::clone(&self.stop),
        }
    }

    /// Return a clone of the internal stop [`Notify`].
    ///
    /// Calling `notify_one()` on the returned handle causes [`run`][Self::run]
    /// to return after the current tick finishes.  Prefer
    /// [`PlacementSchedulerHandle::stop`] via [`Self::handle`] for a more
    /// ergonomic API.
    pub fn stop_signal(&self) -> Arc<Notify> {
        Arc::clone(&self.stop)
    }

    /// Run the scheduler loop until the stop signal fires.
    ///
    /// Normally invoked via `tokio::spawn(scheduler.run())`.  The future
    /// resolves when either:
    /// - The stop signal is triggered (via [`PlacementSchedulerHandle::stop`]
    ///   or the raw [`Notify`] returned by [`stop_signal`][Self::stop_signal]).
    /// - The enclosing task is cancelled.
    pub async fn run(self) {
        info!(
            tick_interval_secs = self.config.tick_interval.as_secs_f64(),
            max_actions_per_tick = self.config.max_actions_per_tick,
            "PlacementScheduler: started",
        );

        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.config.tick_interval) => {
                    self.tick().await;
                }
                _ = self.stop.notified() => {
                    info!("PlacementScheduler: stop signal received, exiting");
                    break;
                }
            }
        }
    }

    /// Execute one placement evaluation and proposal cycle.
    ///
    /// This method is intentionally `pub(crate)` so unit tests can drive it
    /// directly without waiting for a real timer tick.
    pub(crate) async fn tick(&self) {
        if !self.node.is_leader() {
            debug!("PlacementScheduler: skipping tick — not leader");
            return;
        }

        // Compute current imbalance to decide whether rebalancing is needed.
        let imbalance = {
            let registry = self.registry.read();
            compute_imbalance(&registry)
        };

        if imbalance < self.config.imbalance_threshold {
            debug!(
                imbalance,
                threshold = self.config.imbalance_threshold,
                "PlacementScheduler: imbalance below threshold, skipping tick",
            );
            return;
        }

        // Compute the placement plan under a short-lived read lock so writers
        // are not starved.  The coordinator is pure-functional: no mutations.
        let plan = {
            let registry = self.registry.read();
            match self.coordinator.plan(&registry) {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = ?e, "PlacementScheduler: plan() failed; skipping tick");
                    return;
                }
            }
        };

        if plan.is_empty() {
            debug!("PlacementScheduler: no placement actions needed");
            return;
        }

        info!(
            action_count = plan.len(),
            "PlacementScheduler: proposing placement actions",
        );

        for action in plan.actions.iter().take(self.config.max_actions_per_tick) {
            let cmd = ClusterCommand::from_placement_action(action);
            let encoded = cmd.encode();
            match self.node.propose(Command::new(encoded)) {
                Ok(index) => {
                    debug!(
                        log_index = index,
                        variant = ?cmd.tag(),
                        "PlacementScheduler: proposed action",
                    );
                }
                Err(e) => {
                    warn!(
                        error = ?e,
                        "PlacementScheduler: failed to propose action \
                         (likely stepped down from leadership); aborting this tick",
                    );
                    // We are no longer the leader — stop proposing for this tick.
                    // The next tick will re-check leadership before proceeding.
                    break;
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster_command::ClusterCommand;
    use crate::types::RaftConfig;
    use amaters_core::Key;
    use std::time::Duration;

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Build a minimal RaftNode that starts as a follower (not leader).
    ///
    /// Uses a 3-node configuration so `validate()` passes (odd, ≥ 3 peers).
    fn make_follower_node() -> Arc<RaftNode> {
        let config = RaftConfig::new(1, vec![1, 2, 3]);
        Arc::new(RaftNode::new(config).expect("RaftNode::new must succeed for valid config"))
    }

    /// Build an empty shared shard registry.
    fn make_registry() -> Arc<RwLock<ShardRegistry>> {
        Arc::new(RwLock::new(ShardRegistry::new()))
    }

    // ── config tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_scheduler_config_defaults() {
        let cfg = PlacementSchedulerConfig::default();
        assert_eq!(
            cfg.tick_interval,
            Duration::from_secs(30),
            "default tick_interval must be 30 seconds",
        );
        assert_eq!(
            cfg.max_actions_per_tick, 5,
            "default max_actions_per_tick must be 5",
        );
    }

    // ── construction tests ────────────────────────────────────────────────────

    #[test]
    fn test_placement_scheduler_new() {
        let node = make_follower_node();
        let registry = make_registry();
        let policy = PlacementPolicy::default_policy();
        let config = PlacementSchedulerConfig::default();

        let scheduler =
            PlacementScheduler::new(Arc::clone(&node), Arc::clone(&registry), policy, config);

        // stop_signal() must return a cloneable Arc<Notify>.
        let sig1 = scheduler.stop_signal();
        let sig2 = Arc::clone(&sig1);
        // Both point to the same allocation.
        assert!(
            Arc::ptr_eq(&sig1, &sig2),
            "cloned stop signals must share the same Arc",
        );

        // handle() must also refer to the same underlying Notify.
        let handle = scheduler.handle();
        assert!(
            Arc::ptr_eq(&sig1, &handle.stop),
            "handle stop must share the same Arc as stop_signal()",
        );
    }

    // ── tick with non-leader node ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_scheduler_skips_when_not_leader() {
        // A freshly constructed RaftNode starts as a Follower.
        // tick() must return without calling propose() and without panicking.
        let node = make_follower_node();
        let registry = make_registry();
        let policy = PlacementPolicy::default_policy();
        let config = PlacementSchedulerConfig {
            tick_interval: Duration::from_secs(3600),
            max_actions_per_tick: 5,
            imbalance_threshold: 0.2,
        };

        let scheduler =
            PlacementScheduler::new(Arc::clone(&node), Arc::clone(&registry), policy, config);

        // The node is a follower; tick() must short-circuit immediately.
        // We verify no panic occurs and that the log stays empty.
        let log_len_before = node.last_log_index();
        scheduler.tick().await;
        let log_len_after = node.last_log_index();

        assert_eq!(
            log_len_before, log_len_after,
            "tick() must not append any entries when node is not leader",
        );
    }

    // ── stop signal exits the run loop promptly ───────────────────────────────

    #[tokio::test]
    async fn test_scheduler_exits_on_stop_signal() {
        let node = make_follower_node();
        let registry = make_registry();
        let policy = PlacementPolicy::default_policy();
        // Very long tick so the select! arm is driven by the stop signal, not
        // the timer.
        let config = PlacementSchedulerConfig {
            tick_interval: Duration::from_secs(3600),
            max_actions_per_tick: 5,
            imbalance_threshold: 0.2,
        };

        let scheduler =
            PlacementScheduler::new(Arc::clone(&node), Arc::clone(&registry), policy, config);

        let stop = scheduler.stop_signal();
        let join = tokio::spawn(scheduler.run());

        // Signal stop and wait; the task should finish within 1 second.
        stop.notify_one();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("scheduler must exit within 1 second after stop signal")
            .expect("scheduler task must not panic");
    }

    // ── PlacementSchedulerHandle::stop works ─────────────────────────────────

    #[tokio::test]
    async fn test_handle_stop_exits_run_loop() {
        let node = make_follower_node();
        let registry = make_registry();
        let policy = PlacementPolicy::default_policy();
        let config = PlacementSchedulerConfig {
            tick_interval: Duration::from_secs(3600),
            max_actions_per_tick: 5,
            imbalance_threshold: 0.2,
        };

        let scheduler =
            PlacementScheduler::new(Arc::clone(&node), Arc::clone(&registry), policy, config);

        let handle = scheduler.handle();
        let join = tokio::spawn(scheduler.run());

        handle.stop();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("scheduler must exit within 1 second after handle.stop()")
            .expect("scheduler task must not panic");
    }

    // ── cluster_command round-trip exercised from within the scheduler module ─

    #[test]
    fn test_cluster_command_round_trip_in_scheduler() {
        // Verify that the ClusterCommand encoding used in tick() is correct by
        // manually exercising the encode→decode path for all placement variants.
        let split_cmd = ClusterCommand::PlaceSplit {
            shard_id: 10,
            split_key: Key::from_slice(&[0x80u8]).as_bytes().to_vec(),
        };
        let encoded = split_cmd.encode();
        let decoded = ClusterCommand::decode(&encoded).expect("PlaceSplit round-trip must succeed");
        assert_eq!(split_cmd, decoded, "PlaceSplit round-trip must be lossless");

        let merge_cmd = ClusterCommand::PlaceMerge {
            left_shard_id: 3,
            right_shard_id: 4,
        };
        let encoded = merge_cmd.encode();
        let decoded = ClusterCommand::decode(&encoded).expect("PlaceMerge round-trip must succeed");
        assert_eq!(merge_cmd, decoded, "PlaceMerge round-trip must be lossless");

        let transfer_cmd = ClusterCommand::PlaceTransfer {
            shard_id: 99,
            from_node: 1,
            to_node: 2,
        };
        let encoded = transfer_cmd.encode();
        let decoded =
            ClusterCommand::decode(&encoded).expect("PlaceTransfer round-trip must succeed");
        assert_eq!(
            transfer_cmd, decoded,
            "PlaceTransfer round-trip must be lossless"
        );
    }

    // ── stop is idempotent ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_handle_stop_is_idempotent() {
        let node = make_follower_node();
        let registry = make_registry();
        let policy = PlacementPolicy::default_policy();
        let config = PlacementSchedulerConfig {
            tick_interval: Duration::from_secs(3600),
            max_actions_per_tick: 5,
            imbalance_threshold: 0.2,
        };

        let scheduler =
            PlacementScheduler::new(Arc::clone(&node), Arc::clone(&registry), policy, config);

        let handle = scheduler.handle();
        let join = tokio::spawn(scheduler.run());

        // Call stop() more than once — must not panic or deadlock.
        handle.stop();
        handle.stop();
        handle.stop();

        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("scheduler must still exit cleanly after multiple stop() calls")
            .expect("scheduler task must not panic");
    }

    // ── imbalance threshold tests ─────────────────────────────────────────────

    #[test]
    fn test_placement_scheduler_config_imbalance_threshold_default() {
        let cfg = PlacementSchedulerConfig::default();
        assert!(
            (cfg.imbalance_threshold - 0.2).abs() < 1e-9,
            "default imbalance threshold must be 0.2, got {}",
            cfg.imbalance_threshold
        );
    }

    #[tokio::test]
    async fn test_placement_skipped_below_threshold() {
        use crate::shard::ShardRegistry;
        // An empty registry yields imbalance 0.0 which is below default threshold 0.2.
        let registry = ShardRegistry::new();
        let imbalance = super::compute_imbalance(&registry);
        assert_eq!(imbalance, 0.0, "empty registry imbalance must be 0.0");
    }

    #[test]
    fn test_imbalance_computed_correctly() {
        use crate::shard::{KeyRange, ShardMetadata, ShardRegistry};
        use amaters_core::Key;

        let registry = ShardRegistry::new();

        // Use clearly non-overlapping ranges with zero-padded prefixes so
        // lexicographic order matches intended order.
        // Node 1: 3 shards — "a00".."a10", "a10".."a20", "a20".."a30"
        // Node 2: 1 shard  — "b00".."b10"
        // max=3, min=1, mean=4/2=2.0 → (3-1)/2 = 1.0
        let ranges: &[(&str, &str, u64, u64)] = &[
            ("a00", "a10", 1, 1),
            ("a10", "a20", 2, 1),
            ("a20", "a30", 3, 1),
            ("b00", "b10", 4, 2),
        ];
        for &(start_s, end_s, shard_id, node_id) in ranges {
            let start = Key::from_str(start_s);
            let end = Key::from_str(end_s);
            let range = KeyRange::new(start, end).expect("valid range");
            let shard = ShardMetadata::new(shard_id, range, node_id);
            registry.register(shard).expect("register must succeed");
        }

        let imbalance = super::compute_imbalance(&registry);
        // max=3, min=1, mean=4/2=2.0 → (3-1)/2 = 1.0
        assert!(
            (imbalance - 1.0).abs() < 1e-9,
            "imbalance should be 1.0, got {}",
            imbalance
        );
    }
}
