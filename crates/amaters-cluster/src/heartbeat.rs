//! Heartbeat-based failure detection for cluster peers.
//!
//! [`FailureDetector`] tracks the last heartbeat timestamp for each peer and
//! emits [`FailureEvent`]s when peers time out or recover.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::error::{RaftError, RaftResult};
use crate::types::{FailureEvent, HeartbeatConfig, NodeId};

/// Per-peer tracking state maintained by the failure detector.
#[derive(Debug)]
struct PeerState {
    /// When the most recent heartbeat was received from this peer.
    last_heartbeat: Instant,
    /// Monotonically increasing count of consecutive missed heartbeat rounds.
    missed_count: u32,
    /// Whether this peer is currently considered to have failed.
    is_failed: bool,
}

impl PeerState {
    fn new(now: Instant) -> Self {
        Self {
            last_heartbeat: now,
            missed_count: 0,
            is_failed: false,
        }
    }
}

/// Heartbeat-based peer failure detector.
///
/// Call [`FailureDetector::record_heartbeat`] each time a heartbeat (or any
/// message) is received from a peer.  Call
/// [`FailureDetector::check_timeouts`] periodically (e.g., once per
/// heartbeat interval) to receive a list of [`FailureEvent`]s for peers that
/// have timed out or recovered since the last check.
#[derive(Debug)]
pub struct FailureDetector {
    /// Configuration for heartbeat intervals and timeouts.
    config: HeartbeatConfig,
    /// This node's own ID (never tracked as a peer).
    self_id: NodeId,
    /// Per-peer tracking state.
    peers: HashMap<NodeId, PeerState>,
}

impl FailureDetector {
    /// Create a new failure detector.
    ///
    /// # Arguments
    /// * `config` — heartbeat timing configuration
    /// * `self_id` — this node's ID; it will never be added to peer tracking
    pub fn new(config: HeartbeatConfig, self_id: NodeId) -> Self {
        Self {
            config,
            self_id,
            peers: HashMap::new(),
        }
    }

    /// Begin tracking a new peer.
    ///
    /// Initialises the peer with `last_heartbeat = now` so it does not
    /// immediately time out.
    ///
    /// Returns an error if `peer_id == self_id`.
    pub fn track_peer(&mut self, peer_id: NodeId) -> RaftResult<()> {
        if peer_id == self.self_id {
            return Err(RaftError::StorageError {
                message: format!(
                    "Cannot track self (node {}) as a peer in FailureDetector",
                    peer_id
                ),
            });
        }
        self.peers
            .entry(peer_id)
            .or_insert_with(|| PeerState::new(Instant::now()));
        Ok(())
    }

    /// Stop tracking a peer and remove its state.
    pub fn remove_peer(&mut self, peer_id: NodeId) {
        self.peers.remove(&peer_id);
    }

    /// Record that a heartbeat (or any live message) was received from `peer_id`.
    ///
    /// Resets the missed counter and marks the peer alive.  If the peer was
    /// not previously tracked it is added automatically.
    ///
    /// Returns an error if `peer_id == self_id`.
    pub fn record_heartbeat(&mut self, peer_id: NodeId) -> RaftResult<()> {
        if peer_id == self.self_id {
            return Err(RaftError::StorageError {
                message: format!("Cannot record heartbeat from self (node {})", peer_id),
            });
        }
        let state = self
            .peers
            .entry(peer_id)
            .or_insert_with(|| PeerState::new(Instant::now()));
        state.last_heartbeat = Instant::now();
        state.missed_count = 0;
        // Note: is_failed is intentionally NOT cleared here.
        // Recovery is detected by check_timeouts() when it sees
        // elapsed < timeout with is_failed still set.
        Ok(())
    }

    /// Inspect all tracked peers and return any failure / recovery events.
    ///
    /// A peer is considered failed when the elapsed time since its last
    /// heartbeat exceeds `config.timeout_ms` **and** its `missed_count`
    /// reaches `config.max_missed`.  A previously-failed peer that now
    /// satisfies the aliveness condition emits a [`FailureEvent::NodeRecovered`].
    ///
    /// This method updates internal state (missed counters, failed flags) in
    /// place.
    pub fn check_timeouts(&mut self) -> RaftResult<Vec<FailureEvent>> {
        let now = Instant::now();
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let max_missed = self.config.max_missed;

        let mut events = Vec::new();

        for (&peer_id, state) in self.peers.iter_mut() {
            let elapsed = now.duration_since(state.last_heartbeat);

            if elapsed >= timeout {
                // Peer has not sent a heartbeat within the timeout window
                state.missed_count = state.missed_count.saturating_add(1);
                if state.missed_count >= max_missed && !state.is_failed {
                    state.is_failed = true;
                    events.push(FailureEvent::NodeFailed {
                        node_id: peer_id,
                        missed_count: state.missed_count,
                        last_seen_ago_ms: elapsed.as_millis() as u64,
                    });
                }
            } else if state.is_failed {
                // Peer has recovered
                state.is_failed = false;
                state.missed_count = 0;
                events.push(FailureEvent::NodeRecovered { node_id: peer_id });
            }
        }

        Ok(events)
    }

    /// Return the IDs of all peers currently considered failed.
    pub fn failed_peers(&self) -> Vec<NodeId> {
        self.peers
            .iter()
            .filter(|(_, s)| s.is_failed)
            .map(|(&id, _)| id)
            .collect()
    }

    /// Return the IDs of all peers currently considered alive.
    pub fn alive_peers(&self) -> Vec<NodeId> {
        self.peers
            .iter()
            .filter(|(_, s)| !s.is_failed)
            .map(|(&id, _)| id)
            .collect()
    }

    /// Reset all peer states (missed counters cleared, all marked alive).
    ///
    /// Useful when this node becomes a new leader and stale failure data
    /// should be discarded.
    pub fn reset_all(&mut self) {
        let now = Instant::now();
        for state in self.peers.values_mut() {
            state.last_heartbeat = now;
            state.missed_count = 0;
            state.is_failed = false;
        }
    }

    /// Return the number of currently tracked peers.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    fn make_detector() -> FailureDetector {
        let config = HeartbeatConfig {
            interval_ms: 10,
            timeout_ms: 50,
            max_missed: 2,
        };
        FailureDetector::new(config, 1)
    }

    #[test]
    fn test_track_peer_and_record_heartbeat() {
        let mut d = make_detector();
        d.track_peer(2).expect("track ok");
        d.record_heartbeat(2).expect("heartbeat ok");
        assert_eq!(d.peer_count(), 1);
        assert!(d.alive_peers().contains(&2));
    }

    #[test]
    fn test_track_self_is_error() {
        let mut d = make_detector();
        let r = d.track_peer(1); // self_id is 1
        assert!(r.is_err());
    }

    #[test]
    fn test_failure_detection_on_timeout() {
        let mut d = make_detector(); // timeout_ms=50, max_missed=2
        d.track_peer(2).expect("track ok");

        // Sleep beyond timeout
        thread::sleep(Duration::from_millis(60));
        // First check: missed_count=1 (< max_missed=2), no failure yet
        let events = d.check_timeouts().expect("check ok");
        assert!(
            events.is_empty(),
            "First check should not declare failure yet"
        );

        // Sleep again and check
        thread::sleep(Duration::from_millis(60));
        let events = d.check_timeouts().expect("check ok");
        let failed: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, FailureEvent::NodeFailed { .. }))
            .collect();
        assert!(
            !failed.is_empty(),
            "Should declare failure after max_missed exceeded"
        );
        assert!(d.failed_peers().contains(&2));
    }

    #[test]
    fn test_recovery_after_failure() {
        let mut d = make_detector();
        d.track_peer(3).expect("track ok");

        // Force the peer into failed state by sleeping past timeout twice
        thread::sleep(Duration::from_millis(60));
        d.check_timeouts().expect("check 1 ok");
        thread::sleep(Duration::from_millis(60));
        d.check_timeouts().expect("check 2 ok");

        assert!(d.failed_peers().contains(&3), "Peer 3 should be failed");

        // Send a heartbeat — peer recovers
        d.record_heartbeat(3).expect("heartbeat ok");
        let events = d.check_timeouts().expect("check ok");
        let recovered: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, FailureEvent::NodeRecovered { node_id: 3 }))
            .collect();
        assert!(!recovered.is_empty(), "Should emit NodeRecovered event");
        assert!(d.alive_peers().contains(&3));
    }

    #[test]
    fn test_reset_all_clears_failure_state() {
        let mut d = make_detector();
        d.track_peer(2).expect("track ok");
        d.track_peer(3).expect("track ok");

        thread::sleep(Duration::from_millis(60));
        d.check_timeouts().expect("ok");
        thread::sleep(Duration::from_millis(60));
        d.check_timeouts().expect("ok");

        assert!(!d.failed_peers().is_empty(), "Some peers should be failed");

        d.reset_all();
        assert!(
            d.failed_peers().is_empty(),
            "reset_all should clear all failures"
        );
        assert_eq!(d.alive_peers().len(), 2);
    }

    #[test]
    fn test_auto_track_via_heartbeat() {
        let mut d = make_detector();
        // Peer not tracked yet, record_heartbeat should auto-track it
        d.record_heartbeat(5).expect("auto-track ok");
        assert_eq!(d.peer_count(), 1);
        assert!(d.alive_peers().contains(&5));
    }

    #[test]
    fn test_remove_peer() {
        let mut d = make_detector();
        d.track_peer(2).expect("track ok");
        d.remove_peer(2);
        assert_eq!(d.peer_count(), 0);
    }
}
