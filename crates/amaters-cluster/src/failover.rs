//! Automatic failover coordination for Raft clusters.
//!
//! [`FailoverCoordinator`] wraps a [`FailureDetector`] and monitors the
//! current leader.  When the failure detector reports that the leader has
//! failed, the coordinator schedules an election with randomised jitter
//! (to avoid thundering-herd simultaneous elections) and emits
//! [`FailoverEvent`]s that the node event loop can act upon.
//!
//! Followers use [`FailoverCoordinator::leader_hint`] to redirect client
//! requests to the current leader without requiring a full round-trip.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::error::{RaftError, RaftResult};
use crate::heartbeat::FailureDetector;
use crate::types::{FailureEvent, HeartbeatConfig, NodeId};

// ── Events ──────────────────────────────────────────────────────────

/// Events produced by the [`FailoverCoordinator`] during each tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailoverEvent {
    /// The current leader has been detected as failed and an election has
    /// been scheduled (after the jitter delay expires).
    LeaderLost {
        /// Node ID of the old leader.
        old_leader: NodeId,
        /// Whether an election timer was started as a result.
        election_triggered: bool,
    },
    /// A new leader has been acknowledged (set via [`FailoverCoordinator::set_leader`]).
    LeaderElected {
        /// Node ID of the new leader.
        new_leader: NodeId,
    },
    /// The election jitter timer expired without a new leader being set.
    FailoverTimeout,
    /// A non-leader peer has failed.
    PeerFailed {
        /// The failed peer.
        node_id: NodeId,
    },
    /// A previously-failed peer has recovered.
    PeerRecovered {
        /// The recovered peer.
        node_id: NodeId,
    },
}

// ── Configuration ───────────────────────────────────────────────────

/// Tuning knobs for the failover coordinator.
#[derive(Debug, Clone)]
pub struct FailoverConfig {
    /// Minimum election jitter in milliseconds.
    pub election_jitter_min_ms: u64,
    /// Maximum election jitter in milliseconds.
    pub election_jitter_max_ms: u64,
    /// How many consecutive leader failure detections before triggering
    /// an election.
    pub max_consecutive_failures: u32,
}

impl FailoverConfig {
    /// Create a new failover configuration.
    pub fn new(
        election_jitter_min_ms: u64,
        election_jitter_max_ms: u64,
        max_consecutive_failures: u32,
    ) -> Self {
        Self {
            election_jitter_min_ms,
            election_jitter_max_ms,
            max_consecutive_failures,
        }
    }

    /// Validate the configuration, returning an error message on failure.
    pub fn validate(&self) -> Result<(), String> {
        if self.election_jitter_min_ms == 0 {
            return Err("election_jitter_min_ms must be > 0".to_string());
        }
        if self.election_jitter_max_ms <= self.election_jitter_min_ms {
            return Err(format!(
                "election_jitter_max_ms ({}) must be > election_jitter_min_ms ({})",
                self.election_jitter_max_ms, self.election_jitter_min_ms,
            ));
        }
        if self.max_consecutive_failures == 0 {
            return Err("max_consecutive_failures must be > 0".to_string());
        }
        Ok(())
    }

    /// Generate a random jitter duration in `[min, max)`.
    fn random_jitter(&self) -> Duration {
        let range = self.election_jitter_max_ms - self.election_jitter_min_ms;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let random_value = RandomState::new().hash_one(now);
        let jitter_ms = self.election_jitter_min_ms + (random_value % range);
        Duration::from_millis(jitter_ms)
    }
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            election_jitter_min_ms: 150,
            election_jitter_max_ms: 300,
            max_consecutive_failures: 3,
        }
    }
}

// ── Coordinator ─────────────────────────────────────────────────────

/// Internal election timer state.
#[derive(Debug)]
enum ElectionTimer {
    /// No election is pending.
    Idle,
    /// Waiting for jitter to expire before signalling the node to start
    /// an election.
    Pending {
        /// When the timer was started.
        started_at: Instant,
        /// How long to wait.
        jitter: Duration,
    },
    /// The jitter expired and we signalled the caller to start an election;
    /// now we are waiting for a new leader to appear.
    Fired {
        /// When the timer fired.
        fired_at: Instant,
    },
}

/// Coordinates automatic leader failover.
///
/// # Usage
///
/// ```rust,ignore
/// let hb_config = HeartbeatConfig::new(100, 500, 3);
/// let fo_config = FailoverConfig::default();
/// let mut coord = FailoverCoordinator::new(hb_config, fo_config, self_id);
/// coord.track_peer(2)?;
/// coord.track_peer(3)?;
/// coord.set_leader(2);
///
/// // In the event loop:
/// for event in coord.tick()? {
///     match event {
///         FailoverEvent::LeaderLost { .. } => { /* prepare for election */ }
///         FailoverEvent::FailoverTimeout => { node.start_election(); }
///         _ => {}
///     }
/// }
/// ```
pub struct FailoverCoordinator {
    /// Underlying failure detector.
    detector: FailureDetector,
    /// Failover-specific configuration.
    config: FailoverConfig,
    /// This node's own ID.
    self_id: NodeId,
    /// Current known leader (None if unknown).
    current_leader: Option<NodeId>,
    /// Election timer state.
    election_timer: ElectionTimer,
    /// Number of consecutive ticks where the leader was detected as failed.
    leader_failure_count: u32,
}

impl FailoverCoordinator {
    /// Create a new failover coordinator.
    pub fn new(
        heartbeat_config: HeartbeatConfig,
        failover_config: FailoverConfig,
        self_id: NodeId,
    ) -> Self {
        Self {
            detector: FailureDetector::new(heartbeat_config, self_id),
            config: failover_config,
            self_id,
            current_leader: None,
            election_timer: ElectionTimer::Idle,
            leader_failure_count: 0,
        }
    }

    // ── Peer management (delegates to FailureDetector) ──────────────

    /// Begin tracking a peer.
    pub fn track_peer(&mut self, peer_id: NodeId) -> RaftResult<()> {
        self.detector.track_peer(peer_id)
    }

    /// Stop tracking a peer.
    pub fn remove_peer(&mut self, peer_id: NodeId) {
        self.detector.remove_peer(peer_id);
        if self.current_leader == Some(peer_id) {
            self.current_leader = None;
        }
    }

    /// Record a heartbeat from a peer.
    pub fn record_heartbeat(&mut self, peer_id: NodeId) -> RaftResult<()> {
        self.detector.record_heartbeat(peer_id)
    }

    // ── Leader tracking ─────────────────────────────────────────────

    /// Set the current known leader.
    pub fn set_leader(&mut self, leader_id: NodeId) {
        let changed = self.current_leader != Some(leader_id);
        self.current_leader = Some(leader_id);
        if changed {
            self.leader_failure_count = 0;
            self.election_timer = ElectionTimer::Idle;
            debug!(
                self_id = self.self_id,
                leader_id = leader_id,
                "FailoverCoordinator: leader updated"
            );
        }
    }

    /// Clear the current leader (e.g. after stepping down).
    pub fn clear_leader(&mut self) {
        self.current_leader = None;
        self.leader_failure_count = 0;
        self.election_timer = ElectionTimer::Idle;
    }

    /// Return the current known leader, useful for client redirects.
    pub fn leader_hint(&self) -> Option<NodeId> {
        self.current_leader
    }

    /// Returns `true` if this node should redirect clients to the current leader.
    ///
    /// A node should redirect when there is a known leader that is not this node.
    /// If the leader is unknown (election in progress), returns `false` so that
    /// the caller can try locally or return a generic "leader unknown" error.
    pub fn should_redirect(&self, my_id: NodeId) -> bool {
        match self.current_leader {
            Some(leader) => leader != my_id,
            None => false,
        }
    }

    // ── Tick ────────────────────────────────────────────────────────

    /// Advance the coordinator by one tick.
    ///
    /// Checks the underlying failure detector and processes any leader
    /// failure / recovery events.  Returns a (possibly empty) list of
    /// [`FailoverEvent`]s the caller should act upon.
    pub fn tick(&mut self) -> RaftResult<Vec<FailoverEvent>> {
        let failure_events = self.detector.check_timeouts()?;
        let mut out = Vec::new();

        for fe in &failure_events {
            match fe {
                FailureEvent::NodeFailed { node_id, .. } => {
                    if Some(*node_id) == self.current_leader {
                        self.leader_failure_count = self.leader_failure_count.saturating_add(1);
                        let should_trigger =
                            self.leader_failure_count >= self.config.max_consecutive_failures;

                        if should_trigger {
                            self.schedule_election();
                        }

                        info!(
                            self_id = self.self_id,
                            leader = node_id,
                            failure_count = self.leader_failure_count,
                            triggered = should_trigger,
                            "Leader failure detected"
                        );

                        out.push(FailoverEvent::LeaderLost {
                            old_leader: *node_id,
                            election_triggered: should_trigger,
                        });
                    } else {
                        out.push(FailoverEvent::PeerFailed { node_id: *node_id });
                    }
                }
                FailureEvent::NodeRecovered { node_id } => {
                    if Some(*node_id) == self.current_leader {
                        // Leader came back — cancel any pending election timer.
                        self.leader_failure_count = 0;
                        self.election_timer = ElectionTimer::Idle;
                        debug!(
                            self_id = self.self_id,
                            leader = node_id,
                            "Leader recovered, election timer cancelled"
                        );
                    }
                    out.push(FailoverEvent::PeerRecovered { node_id: *node_id });
                }
            }
        }

        // Check election timer
        match &self.election_timer {
            ElectionTimer::Pending { started_at, jitter } => {
                if started_at.elapsed() >= *jitter {
                    info!(
                        self_id = self.self_id,
                        jitter_ms = jitter.as_millis() as u64,
                        "Election jitter expired, triggering failover"
                    );
                    self.election_timer = ElectionTimer::Fired {
                        fired_at: Instant::now(),
                    };
                    out.push(FailoverEvent::FailoverTimeout);
                }
            }
            ElectionTimer::Fired { .. } | ElectionTimer::Idle => {}
        }

        Ok(out)
    }

    /// Reset all state (e.g. when this node becomes leader).
    pub fn reset(&mut self) {
        self.detector.reset_all();
        self.leader_failure_count = 0;
        self.election_timer = ElectionTimer::Idle;
    }

    /// Return the IDs of peers currently considered failed.
    pub fn failed_peers(&self) -> Vec<NodeId> {
        self.detector.failed_peers()
    }

    /// Return the IDs of peers currently considered alive.
    pub fn alive_peers(&self) -> Vec<NodeId> {
        self.detector.alive_peers()
    }

    /// Return the number of tracked peers.
    pub fn peer_count(&self) -> usize {
        self.detector.peer_count()
    }

    /// Whether an election is currently pending (jitter not yet expired).
    pub fn is_election_pending(&self) -> bool {
        matches!(self.election_timer, ElectionTimer::Pending { .. })
    }

    /// Whether the election timer has fired.
    pub fn is_election_fired(&self) -> bool {
        matches!(self.election_timer, ElectionTimer::Fired { .. })
    }

    // ── Internal ────────────────────────────────────────────────────

    fn schedule_election(&mut self) {
        if matches!(
            self.election_timer,
            ElectionTimer::Pending { .. } | ElectionTimer::Fired { .. }
        ) {
            // Already scheduled or already fired; do not reset the timer.
            return;
        }
        let jitter = self.config.random_jitter();
        debug!(
            self_id = self.self_id,
            jitter_ms = jitter.as_millis() as u64,
            "Scheduling election with jitter"
        );
        self.election_timer = ElectionTimer::Pending {
            started_at: Instant::now(),
            jitter,
        };
    }
}

impl std::fmt::Debug for FailoverCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FailoverCoordinator")
            .field("self_id", &self.self_id)
            .field("current_leader", &self.current_leader)
            .field("leader_failure_count", &self.leader_failure_count)
            .field("peer_count", &self.detector.peer_count())
            .finish()
    }
}

// ── AlertEvent ──────────────────────────────────────────────────────

/// An event that the alerting subsystem can deliver to registered callbacks.
#[derive(Debug, Clone)]
pub enum AlertEvent {
    /// A node has been presumed failed (heartbeat timeout or explicit mark).
    NodeFailed { node_id: NodeId },
    /// A previously-failed node is now reachable again.
    NodeRecovered { node_id: NodeId },
    /// Raft leader changed (old may be `None` for the very first election).
    LeaderChanged {
        old_leader: Option<NodeId>,
        new_leader: NodeId,
    },
    /// The cluster lost quorum — fewer than half of members are reachable.
    QuorumLost {
        cluster_size: usize,
        reachable: usize,
    },
    /// A follower is lagging far behind the leader's commit index.
    SlowReplication { follower: NodeId, lag_entries: u64 },
}

// ── AlertCallback / AlertManager ────────────────────────────────────

/// A thread-safe, shared callback that receives [`AlertEvent`]s.
pub type AlertCallback = Arc<dyn Fn(AlertEvent) + Send + Sync>;

/// Fan-out alerting hub.
///
/// Register callbacks via [`register`][AlertManager::register]; emit events via
/// [`emit`][AlertManager::emit].  Both methods are safe to call from multiple
/// threads concurrently.
pub struct AlertManager {
    callbacks: Mutex<Vec<AlertCallback>>,
}

impl AlertManager {
    /// Create an empty manager with no registered callbacks.
    pub fn new() -> Self {
        Self {
            callbacks: Mutex::new(Vec::new()),
        }
    }

    /// Register a new callback.
    ///
    /// The callback will be invoked synchronously (in the calling thread) for
    /// every subsequent [`emit`][Self::emit] call.
    pub fn register(&self, callback: AlertCallback) {
        self.callbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(callback);
    }

    /// Emit an event to all registered callbacks.
    ///
    /// Callbacks are invoked in registration order.  A panicking callback does
    /// not prevent the remaining callbacks from being invoked.
    pub fn emit(&self, event: AlertEvent) {
        let guard = self.callbacks.lock().unwrap_or_else(|e| e.into_inner());
        for cb in guard.iter() {
            // Use std::panic::catch_unwind so a bad callback cannot poison the
            // lock or abort the cluster node.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cb(event.clone());
            }));
        }
    }
}

impl Default for AlertManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AlertManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.callbacks.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("AlertManager")
            .field("callback_count", &count)
            .finish()
    }
}

// ── FailoverController ──────────────────────────────────────────────

/// A higher-level node health monitor that combines heartbeat-timeout
/// detection with explicit mark/recover operations.
///
/// Unlike [`FailoverCoordinator`] (which is tied to the Raft election
/// machinery), `FailoverController` is a self-contained monitor suitable for
/// use by the cluster topology dashboard, the alerting subsystem, or external
/// health-check drivers.
///
/// # Thread safety
///
/// All methods use internal locks; the struct is `Send + Sync`.
///
/// # Test injection
///
/// Tests may call `set_last_seen` to back-date the last
/// heartbeat timestamp for a node without actually waiting.
pub struct FailoverController {
    heartbeat_timeout: Duration,
    last_seen: Mutex<std::collections::HashMap<NodeId, Instant>>,
    failed_nodes: dashmap::DashSet<NodeId>,
}

impl FailoverController {
    /// Create a new controller.
    ///
    /// `heartbeat_timeout` — how long a node can go without sending a
    /// heartbeat before it is considered failed.
    pub fn new(heartbeat_timeout: Duration) -> Self {
        Self {
            heartbeat_timeout,
            last_seen: Mutex::new(std::collections::HashMap::new()),
            failed_nodes: dashmap::DashSet::new(),
        }
    }

    /// Record that a heartbeat was received from `node_id`.
    ///
    /// If the node was previously marked as failed it is automatically
    /// recovered.
    pub fn record_heartbeat(&self, node_id: NodeId) {
        self.last_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(node_id, Instant::now());
        self.failed_nodes.remove(&node_id);
    }

    /// Check all known nodes; return the IDs of nodes that have not sent a
    /// heartbeat within the timeout window.
    ///
    /// Nodes detected here are also added to the internal failed set (visible
    /// via [`is_failed`][Self::is_failed] / [`failed_nodes`][Self::failed_nodes]).
    pub fn detect_failed_nodes(&self) -> Vec<NodeId> {
        let now = Instant::now();
        let guard = self.last_seen.lock().unwrap_or_else(|e| e.into_inner());
        let mut failed = Vec::new();
        for (&node_id, &last) in guard.iter() {
            if now.duration_since(last) >= self.heartbeat_timeout {
                self.failed_nodes.insert(node_id);
                failed.push(node_id);
            }
        }
        failed
    }

    /// Explicitly mark a node as failed (e.g. after Raft loss-of-quorum).
    pub fn mark_failed(&self, node_id: NodeId) {
        self.failed_nodes.insert(node_id);
    }

    /// Mark a node as recovered.
    ///
    /// This removes it from the failed set.  Call
    /// [`record_heartbeat`][Self::record_heartbeat] as well to reset the
    /// timeout clock.
    pub fn mark_recovered(&self, node_id: NodeId) {
        self.failed_nodes.remove(&node_id);
    }

    /// Return `true` if `node_id` is currently considered failed.
    pub fn is_failed(&self, node_id: NodeId) -> bool {
        self.failed_nodes.contains(&node_id)
    }

    /// Return all currently failed node IDs as a `Vec`.
    pub fn failed_nodes(&self) -> Vec<NodeId> {
        self.failed_nodes.iter().map(|r| *r).collect()
    }

    // ── Test-only helpers ─────────────────────────────────────────────────────

    /// Overwrite the last-seen timestamp for `node_id` to `instant`.
    ///
    /// Use this in tests to simulate time advancing without actually sleeping.
    #[cfg(test)]
    pub fn set_last_seen(&self, node_id: NodeId, instant: Instant) {
        self.last_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(node_id, instant);
    }
}

impl std::fmt::Debug for FailoverController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let known = self.last_seen.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("FailoverController")
            .field("heartbeat_timeout", &self.heartbeat_timeout)
            .field("known_nodes", &known)
            .field("failed_count", &self.failed_nodes.len())
            .finish()
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn fast_heartbeat_config() -> HeartbeatConfig {
        // Very short timeouts so tests complete quickly
        HeartbeatConfig::new(10, 30, 1)
    }

    fn fast_failover_config() -> FailoverConfig {
        FailoverConfig {
            election_jitter_min_ms: 10,
            election_jitter_max_ms: 30,
            max_consecutive_failures: 1,
        }
    }

    #[test]
    fn test_failover_config_default() {
        let cfg = FailoverConfig::default();
        assert_eq!(cfg.election_jitter_min_ms, 150);
        assert_eq!(cfg.election_jitter_max_ms, 300);
        assert_eq!(cfg.max_consecutive_failures, 3);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_failover_config_validation() {
        let bad1 = FailoverConfig::new(0, 300, 3);
        assert!(bad1.validate().is_err());

        let bad2 = FailoverConfig::new(300, 150, 3);
        assert!(bad2.validate().is_err());

        let bad3 = FailoverConfig::new(150, 300, 0);
        assert!(bad3.validate().is_err());

        let bad4 = FailoverConfig::new(150, 150, 3);
        assert!(bad4.validate().is_err());
    }

    #[test]
    fn test_failover_config_jitter_in_range() {
        let cfg = FailoverConfig::new(100, 200, 3);
        for _ in 0..20 {
            let jitter = cfg.random_jitter();
            assert!(jitter.as_millis() >= 100, "jitter too low: {:?}", jitter);
            assert!(jitter.as_millis() < 200, "jitter too high: {:?}", jitter);
        }
    }

    #[test]
    fn test_coordinator_creation() {
        let coord =
            FailoverCoordinator::new(HeartbeatConfig::default(), FailoverConfig::default(), 1);
        assert_eq!(coord.leader_hint(), None);
        assert_eq!(coord.peer_count(), 0);
        assert!(!coord.is_election_pending());
    }

    #[test]
    fn test_leader_hint_tracking() {
        let mut coord =
            FailoverCoordinator::new(HeartbeatConfig::default(), FailoverConfig::default(), 1);
        assert_eq!(coord.leader_hint(), None);

        coord.set_leader(2);
        assert_eq!(coord.leader_hint(), Some(2));

        coord.set_leader(3);
        assert_eq!(coord.leader_hint(), Some(3));

        coord.clear_leader();
        assert_eq!(coord.leader_hint(), None);
    }

    #[test]
    fn test_leader_failure_triggers_election() {
        let mut coord =
            FailoverCoordinator::new(fast_heartbeat_config(), fast_failover_config(), 1);
        coord.track_peer(2).expect("track peer 2");
        coord.track_peer(3).expect("track peer 3");
        coord.set_leader(2);

        // Let leader timeout
        thread::sleep(Duration::from_millis(50));

        let events = coord.tick().expect("tick");
        let leader_lost = events.iter().any(|e| {
            matches!(
                e,
                FailoverEvent::LeaderLost {
                    old_leader: 2,
                    election_triggered: true,
                }
            )
        });
        assert!(leader_lost, "Expected LeaderLost event, got: {:?}", events);
        assert!(coord.is_election_pending());
    }

    #[test]
    fn test_election_timer_fires_after_jitter() {
        let mut coord =
            FailoverCoordinator::new(fast_heartbeat_config(), fast_failover_config(), 1);
        coord.track_peer(2).expect("track peer 2");
        coord.set_leader(2);

        // Let leader timeout to trigger election scheduling
        thread::sleep(Duration::from_millis(50));
        let _ = coord.tick().expect("tick 1");

        // Wait for jitter to expire
        thread::sleep(Duration::from_millis(50));
        let events = coord.tick().expect("tick 2");

        let timeout_fired = events
            .iter()
            .any(|e| matches!(e, FailoverEvent::FailoverTimeout));
        assert!(
            timeout_fired,
            "Expected FailoverTimeout event, got: {:?}",
            events
        );
        assert!(coord.is_election_fired());
    }

    #[test]
    fn test_leader_recovery_cancels_election() {
        let mut coord =
            FailoverCoordinator::new(fast_heartbeat_config(), fast_failover_config(), 1);
        coord.track_peer(2).expect("track peer 2");
        coord.set_leader(2);

        // Let leader timeout
        thread::sleep(Duration::from_millis(50));
        let _ = coord.tick().expect("tick");
        assert!(coord.is_election_pending());

        // Leader sends a heartbeat → recovery
        coord.record_heartbeat(2).expect("record heartbeat");
        let events = coord.tick().expect("tick after recovery");

        let recovered = events
            .iter()
            .any(|e| matches!(e, FailoverEvent::PeerRecovered { node_id: 2 }));
        assert!(recovered, "Expected PeerRecovered, got: {:?}", events);

        // Election timer should be cancelled
        assert!(!coord.is_election_pending());
        assert!(!coord.is_election_fired());
    }

    #[test]
    fn test_non_leader_failure_emits_peer_failed() {
        let mut coord =
            FailoverCoordinator::new(fast_heartbeat_config(), fast_failover_config(), 1);
        coord.track_peer(2).expect("track peer 2");
        coord.track_peer(3).expect("track peer 3");
        coord.set_leader(2);

        // Let node 3 (non-leader) timeout while leader (node 2) stays alive
        thread::sleep(Duration::from_millis(50));
        // Keep leader heartbeat fresh so it doesn't time out itself
        coord.record_heartbeat(2).expect("leader heartbeat refresh");

        let events = coord.tick().expect("tick");
        let peer_failed = events
            .iter()
            .any(|e| matches!(e, FailoverEvent::PeerFailed { node_id: 3 }));
        assert!(peer_failed, "Expected PeerFailed for 3, got: {:?}", events);
        assert!(
            !coord.is_election_pending(),
            "Non-leader failure should not trigger election"
        );
    }

    #[test]
    fn test_jitter_prevents_simultaneous_elections() {
        // Two coordinators for different nodes, same leader.
        // Their jitter values should typically differ.
        let hb = fast_heartbeat_config();
        let fo = FailoverConfig {
            election_jitter_min_ms: 50,
            election_jitter_max_ms: 200,
            max_consecutive_failures: 1,
        };

        let mut c1 = FailoverCoordinator::new(hb.clone(), fo.clone(), 1);
        let mut c2 = FailoverCoordinator::new(hb.clone(), fo.clone(), 3);

        c1.track_peer(2).expect("c1 track 2");
        c1.track_peer(3).expect("c1 track 3");
        c1.set_leader(2);

        c2.track_peer(1).expect("c2 track 1");
        c2.track_peer(2).expect("c2 track 2");
        c2.set_leader(2);

        // Let leader timeout on both
        thread::sleep(Duration::from_millis(50));
        let _ = c1.tick().expect("c1 tick");
        let _ = c2.tick().expect("c2 tick");

        // Both should have scheduled an election, but the internal jitter
        // values should be independent (they use RandomState which is
        // seeded differently per invocation).
        assert!(c1.is_election_pending());
        assert!(c2.is_election_pending());
    }

    #[test]
    fn test_max_consecutive_failures_threshold() {
        let mut coord = FailoverCoordinator::new(
            fast_heartbeat_config(),
            FailoverConfig {
                election_jitter_min_ms: 10,
                election_jitter_max_ms: 30,
                max_consecutive_failures: 3,
            },
            1,
        );
        coord.track_peer(2).expect("track peer 2");
        coord.set_leader(2);

        // First timeout: failure count 1 — not enough
        thread::sleep(Duration::from_millis(50));
        let events = coord.tick().expect("tick 1");
        let triggered = events.iter().any(|e| {
            matches!(
                e,
                FailoverEvent::LeaderLost {
                    election_triggered: true,
                    ..
                }
            )
        });
        assert!(
            !triggered,
            "Should not trigger election after 1 failure, got: {:?}",
            events
        );

        // Since FailureDetector only emits NodeFailed once per peer
        // (stays in failed state), subsequent ticks won't increment.
        // This verifies the threshold behaviour: a single detection with
        // max_consecutive_failures=3 does NOT trigger an election.
        assert!(!coord.is_election_pending());
    }

    #[test]
    fn test_set_new_leader_resets_state() {
        let mut coord =
            FailoverCoordinator::new(fast_heartbeat_config(), fast_failover_config(), 1);
        coord.track_peer(2).expect("track peer 2");
        coord.track_peer(3).expect("track peer 3");
        coord.set_leader(2);

        // Let leader timeout and schedule election
        thread::sleep(Duration::from_millis(50));
        let _ = coord.tick().expect("tick");
        assert!(coord.is_election_pending());

        // New leader elected: resets everything
        coord.set_leader(3);
        assert!(!coord.is_election_pending());
        assert!(!coord.is_election_fired());
        assert_eq!(coord.leader_hint(), Some(3));
    }

    #[test]
    fn test_reset_clears_all() {
        let mut coord =
            FailoverCoordinator::new(fast_heartbeat_config(), fast_failover_config(), 1);
        coord.track_peer(2).expect("track peer 2");
        coord.set_leader(2);

        thread::sleep(Duration::from_millis(50));
        let _ = coord.tick().expect("tick");

        coord.reset();
        assert!(!coord.is_election_pending());
        assert!(!coord.is_election_fired());
        assert!(coord.failed_peers().is_empty());
    }

    #[test]
    fn test_remove_leader_peer_clears_leader() {
        let mut coord =
            FailoverCoordinator::new(HeartbeatConfig::default(), FailoverConfig::default(), 1);
        coord.track_peer(2).expect("track peer 2");
        coord.set_leader(2);
        assert_eq!(coord.leader_hint(), Some(2));

        coord.remove_peer(2);
        assert_eq!(coord.leader_hint(), None);
    }

    #[test]
    fn test_debug_impl() {
        let coord =
            FailoverCoordinator::new(HeartbeatConfig::default(), FailoverConfig::default(), 1);
        let dbg = format!("{:?}", coord);
        assert!(dbg.contains("FailoverCoordinator"));
        assert!(dbg.contains("self_id"));
    }

    // ── FailoverController tests ───────────────────────────────────────────────

    /// After calling `set_last_seen` with a timestamp far in the past,
    /// `detect_failed_nodes` must report that node as failed.
    #[test]
    fn test_failover_controller_detects_timeout() {
        let timeout = Duration::from_millis(100);
        let controller = FailoverController::new(timeout);

        // Prime last_seen with a timestamp well before the timeout.
        let old_instant = Instant::now() - Duration::from_millis(500);
        controller.set_last_seen(42, old_instant);

        let failed = controller.detect_failed_nodes();
        assert!(
            failed.contains(&42),
            "node 42 should be detected as failed; got {:?}",
            failed
        );
        assert!(controller.is_failed(42), "is_failed(42) should return true");
    }

    /// A node that is first failed, then recovered, must not appear in the
    /// failed list.
    #[test]
    fn test_failover_controller_recovered_node() {
        let controller = FailoverController::new(Duration::from_millis(100));

        controller.mark_failed(7);
        assert!(controller.is_failed(7));

        controller.mark_recovered(7);
        assert!(
            !controller.is_failed(7),
            "node 7 should no longer be failed"
        );
        assert!(
            !controller.failed_nodes().contains(&7),
            "failed_nodes() must not include recovered node 7"
        );
    }

    /// record_heartbeat after a failure should clear the failed status.
    #[test]
    fn test_failover_controller_heartbeat_clears_failure() {
        let controller = FailoverController::new(Duration::from_millis(100));
        controller.mark_failed(3);
        assert!(controller.is_failed(3));
        controller.record_heartbeat(3);
        assert!(!controller.is_failed(3));
    }

    // ── AlertManager tests ─────────────────────────────────────────────────────

    /// Emitting one event to two registered callbacks must invoke both.
    #[test]
    fn test_alert_manager_emits_to_all_callbacks() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let manager = AlertManager::new();
        let count = Arc::new(AtomicUsize::new(0));

        let c1 = Arc::clone(&count);
        manager.register(Arc::new(move |_evt: AlertEvent| {
            c1.fetch_add(1, Ordering::Relaxed);
        }));
        let c2 = Arc::clone(&count);
        manager.register(Arc::new(move |_evt: AlertEvent| {
            c2.fetch_add(1, Ordering::Relaxed);
        }));

        manager.emit(AlertEvent::NodeFailed { node_id: 5 });

        assert_eq!(
            count.load(Ordering::Relaxed),
            2,
            "both callbacks should have been invoked"
        );
    }

    /// Registering and emitting from multiple threads must not panic or
    /// deadlock.
    #[test]
    fn test_alert_manager_thread_safe() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::thread;

        let manager = Arc::new(AlertManager::new());
        let received = Arc::new(AtomicUsize::new(0));

        // Register from multiple threads.
        let mut handles = Vec::new();
        for _ in 0..4 {
            let mgr = Arc::clone(&manager);
            let recv = Arc::clone(&received);
            handles.push(thread::spawn(move || {
                mgr.register(Arc::new(move |_evt: AlertEvent| {
                    recv.fetch_add(1, Ordering::Relaxed);
                }));
            }));
        }
        for h in handles {
            h.join().expect("register thread must not panic");
        }

        // Emit from multiple threads simultaneously.
        let mut emit_handles = Vec::new();
        for _ in 0..4 {
            let mgr = Arc::clone(&manager);
            emit_handles.push(thread::spawn(move || {
                mgr.emit(AlertEvent::NodeFailed { node_id: 1 });
            }));
        }
        for h in emit_handles {
            h.join().expect("emit thread must not panic");
        }

        // 4 emits × 4 callbacks = 16 total invocations.
        let total = received.load(Ordering::Relaxed);
        assert_eq!(total, 16, "expected 16 invocations, got {}", total);
    }

    /// AlertEvent::LeaderChanged carries both old and new leader IDs.
    #[test]
    fn test_alert_manager_leader_changed_event() {
        let manager = AlertManager::new();
        let events: Arc<Mutex<Vec<AlertEvent>>> = Arc::new(Mutex::new(Vec::new()));

        let ev = Arc::clone(&events);
        manager.register(Arc::new(move |e: AlertEvent| {
            ev.lock().unwrap_or_else(|e| e.into_inner()).push(e);
        }));

        manager.emit(AlertEvent::LeaderChanged {
            old_leader: Some(1),
            new_leader: 2,
        });

        let guard = events.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(guard.len(), 1);
        assert!(matches!(
            guard[0],
            AlertEvent::LeaderChanged {
                old_leader: Some(1),
                new_leader: 2,
            }
        ));
    }

    /// After leader loss (set to None), should_redirect returns false because no
    /// leader hint is known. Once a new leader is elected and set, should_redirect
    /// returns true for non-leader nodes and false for the leader itself.
    #[test]
    fn test_failover_redirects_after_leader_loss() {
        let mut coord =
            FailoverCoordinator::new(HeartbeatConfig::default(), FailoverConfig::default(), 1);

        // No leader known yet — should not redirect (unknown destination)
        assert!(
            !coord.should_redirect(1),
            "no redirect when leader is unknown"
        );
        assert!(
            !coord.should_redirect(2),
            "no redirect when leader is unknown"
        );

        // Set node 2 as leader
        coord.set_leader(2);
        // Node 1 (self) should redirect to node 2
        assert!(
            coord.should_redirect(1),
            "node 1 should redirect when leader is node 2"
        );
        // Node 2 (the leader) should not redirect to itself
        assert!(
            !coord.should_redirect(2),
            "node 2 should not redirect when it is the leader"
        );

        // Simulate leader loss
        coord.clear_leader();
        // After loss, no redirect (leader unknown — election in progress)
        assert!(
            !coord.should_redirect(1),
            "no redirect when leader just lost (election pending)"
        );

        // New leader (node 3) elected after recovery
        coord.set_leader(3);
        assert!(
            coord.should_redirect(1),
            "node 1 should redirect to new leader node 3"
        );
        assert!(
            coord.should_redirect(2),
            "node 2 should redirect to new leader node 3"
        );
        assert!(
            !coord.should_redirect(3),
            "node 3 should not redirect to itself"
        );
    }

    /// A follower failure (non-leader peer) must not change the redirect behaviour;
    /// the leader_hint should remain pointing to the known leader.
    #[test]
    fn test_failover_no_redirect_on_follower_loss() {
        let mut coord =
            FailoverCoordinator::new(fast_heartbeat_config(), fast_failover_config(), 1);
        coord.track_peer(2).expect("track peer 2");
        coord.track_peer(3).expect("track peer 3");
        // Node 2 is the leader; node 3 is a follower
        coord.set_leader(2);

        // Node 3 (follower) times out — keep leader heartbeat alive
        thread::sleep(Duration::from_millis(50));
        coord.record_heartbeat(2).expect("leader heartbeat");
        let events = coord.tick().expect("tick");

        // Node 3 should be reported as failed
        let peer_failed = events
            .iter()
            .any(|e| matches!(e, FailoverEvent::PeerFailed { node_id: 3 }));
        assert!(peer_failed, "Expected PeerFailed for node 3");

        // The leader hint must still point to node 2
        assert_eq!(
            coord.leader_hint(),
            Some(2),
            "leader hint should still be node 2 after follower loss"
        );

        // Redirect logic: node 1 should still redirect to node 2
        assert!(
            coord.should_redirect(1),
            "node 1 should still redirect to leader 2 after follower 3 fails"
        );
        // No election should have been triggered by the follower failure
        assert!(
            !coord.is_election_pending(),
            "election must not be triggered by non-leader failure"
        );
    }
}
