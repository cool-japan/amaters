//! Alert rule engine for the AmateRS cluster layer.
//!
//! Provides a [`RuleEngine`] that evaluates [`AlertRule`]s against incoming
//! [`AlertEvent`]s, assigns [`AlertSeverity`], deduplicates within a
//! configurable time window, and fans out [`FiredAlert`]s to registered
//! [`AlertSink`]s.

use crate::failover::AlertEvent;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── Severity ────────────────────────────────────────────────────────────────

/// Severity level assigned to a fired alert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

// ── Rule ────────────────────────────────────────────────────────────────────

/// A predicate function that inspects an [`AlertEvent`] and optionally returns
/// the severity at which the rule should fire.  Returning `None` means the
/// rule does not match this event.
pub type RulePredicate = Arc<dyn Fn(&AlertEvent) -> Option<AlertSeverity> + Send + Sync>;

/// A named rule with its matching predicate.
pub struct AlertRule {
    /// Human-readable rule identifier (used as part of the dedup key).
    pub name: String,
    /// Predicate that maps an event to an optional severity.
    pub predicate: RulePredicate,
}

// ── FiredAlert ───────────────────────────────────────────────────────────────

/// A fully-resolved alert that has passed rule evaluation and dedup checks.
#[derive(Debug, Clone)]
pub struct FiredAlert {
    /// Name of the rule that produced this alert.
    pub rule_name: String,
    /// Severity assigned by the rule.
    pub severity: AlertSeverity,
    /// The original event that triggered the alert.
    pub event: AlertEvent,
    /// Stable dedup key derived from the event (see [`event_dedup_key`]).
    pub dedup_key: String,
}

// ── Sinks ────────────────────────────────────────────────────────────────────

/// Trait implemented by all alert receivers.
pub trait AlertSink: Send + Sync {
    fn on_alert(&self, alert: &FiredAlert);
}

/// A sink that emits alerts via the `tracing` crate.
pub struct LogSink;

impl AlertSink for LogSink {
    fn on_alert(&self, alert: &FiredAlert) {
        match alert.severity {
            AlertSeverity::Critical => {
                tracing::error!(
                    rule = %alert.rule_name,
                    dedup_key = %alert.dedup_key,
                    "CRITICAL alert fired"
                );
            }
            AlertSeverity::Warning => {
                tracing::warn!(
                    rule = %alert.rule_name,
                    dedup_key = %alert.dedup_key,
                    "WARNING alert fired"
                );
            }
            AlertSeverity::Info => {
                tracing::info!(
                    rule = %alert.rule_name,
                    dedup_key = %alert.dedup_key,
                    "INFO alert fired"
                );
            }
        }
    }
}

/// A sink that collects fired alerts into an in-memory vector; intended for
/// unit tests.
pub struct CollectingSink {
    alerts: Mutex<Vec<FiredAlert>>,
}

impl Default for CollectingSink {
    fn default() -> Self {
        Self::new()
    }
}

impl CollectingSink {
    /// Create a new, empty collecting sink.
    pub fn new() -> Self {
        Self {
            alerts: Mutex::new(Vec::new()),
        }
    }

    /// Return a clone of all collected alerts.
    pub fn collected(&self) -> Vec<FiredAlert> {
        self.alerts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Clear all collected alerts.
    pub fn clear(&self) {
        self.alerts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl AlertSink for CollectingSink {
    fn on_alert(&self, alert: &FiredAlert) {
        self.alerts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(alert.clone());
    }
}

// ── Dedup key ────────────────────────────────────────────────────────────────

/// Return a stable string that uniquely identifies the *entity* (node, cluster,
/// …) that an event concerns.  Used as the second component of the dedup key.
pub fn event_dedup_key(event: &AlertEvent) -> String {
    match event {
        AlertEvent::NodeFailed { node_id } => format!("node_failed:{node_id}"),
        AlertEvent::NodeRecovered { node_id } => format!("node_recovered:{node_id}"),
        AlertEvent::LeaderChanged { new_leader, .. } => format!("leader_changed:{new_leader}"),
        AlertEvent::QuorumLost { .. } => "quorum_lost".to_owned(),
        AlertEvent::SlowReplication { follower, .. } => format!("slow_replication:{follower}"),
    }
}

// ── RuleEngine ───────────────────────────────────────────────────────────────

/// Evaluates alert rules, deduplicates firings within a time window, and fans
/// out [`FiredAlert`]s to registered [`AlertSink`]s.
pub struct RuleEngine {
    rules: Vec<AlertRule>,
    sinks: Vec<Arc<dyn AlertSink>>,
    dedup_window: Duration,
    /// Maps `(rule_name, dedup_key)` → time of last firing.
    dedup: Mutex<HashMap<(String, String), Instant>>,
}

impl RuleEngine {
    /// Create a rule engine with the given dedup window.
    ///
    /// An event fired within `dedup_window` of a previous identical event
    /// (same rule + same entity) is suppressed.
    pub fn new_with_window(dedup_window: Duration) -> Self {
        Self {
            rules: Vec::new(),
            sinks: Vec::new(),
            dedup_window,
            dedup: Mutex::new(HashMap::new()),
        }
    }

    /// Register an alert rule.  Returns `&mut Self` for chaining.
    pub fn add_rule(&mut self, rule: AlertRule) -> &mut Self {
        self.rules.push(rule);
        self
    }

    /// Register an alert sink.  Returns `&mut Self` for chaining.
    pub fn add_sink(&mut self, sink: Arc<dyn AlertSink>) -> &mut Self {
        self.sinks.push(sink);
        self
    }

    /// Process one event against all registered rules.
    ///
    /// `now` is injected so that unit tests can advance time deterministically
    /// without sleeping.
    pub fn process_event(&self, event: &AlertEvent, now: Instant) {
        let dedup_key = event_dedup_key(event);

        for rule in &self.rules {
            let Some(severity) = (rule.predicate)(event) else {
                continue;
            };

            let dup_tuple = (rule.name.clone(), dedup_key.clone());

            // --- dedup check ---
            {
                let mut guard = self.dedup.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(last) = guard.get(&dup_tuple) {
                    if now.duration_since(*last) < self.dedup_window {
                        continue; // within window — suppress
                    }
                }
                guard.insert(dup_tuple, now);
            } // release lock before calling sinks

            let alert = FiredAlert {
                rule_name: rule.name.clone(),
                severity,
                event: event.clone(),
                dedup_key: dedup_key.clone(),
            };

            for sink in &self.sinks {
                sink.on_alert(&alert);
            }
        }
    }
}

// ── Default rule set ─────────────────────────────────────────────────────────

/// Build the standard set of alert rules used by the cluster layer.
///
/// `slow_repl_threshold` — minimum `lag_entries` value for a
/// [`AlertEvent::SlowReplication`] event to trigger a warning.
pub fn default_rules(slow_repl_threshold: u64) -> Vec<AlertRule> {
    vec![
        AlertRule {
            name: "node_failed".to_owned(),
            predicate: Arc::new(|event| match event {
                AlertEvent::NodeFailed { .. } => Some(AlertSeverity::Critical),
                _ => None,
            }),
        },
        AlertRule {
            name: "quorum_lost".to_owned(),
            predicate: Arc::new(|event| match event {
                AlertEvent::QuorumLost { .. } => Some(AlertSeverity::Critical),
                _ => None,
            }),
        },
        AlertRule {
            name: "leader_changed".to_owned(),
            predicate: Arc::new(|event| match event {
                AlertEvent::LeaderChanged { .. } => Some(AlertSeverity::Warning),
                _ => None,
            }),
        },
        AlertRule {
            name: "slow_replication".to_owned(),
            predicate: Arc::new(move |event| match event {
                AlertEvent::SlowReplication { lag_entries, .. }
                    if *lag_entries >= slow_repl_threshold =>
                {
                    Some(AlertSeverity::Warning)
                }
                _ => None,
            }),
        },
    ]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn build_engine(sink: Arc<CollectingSink>) -> RuleEngine {
        let mut engine = RuleEngine::new_with_window(Duration::from_secs(60));
        for rule in default_rules(100) {
            engine.add_rule(rule);
        }
        engine.add_sink(sink);
        engine
    }

    #[test]
    fn test_node_failed_fires_critical() {
        let sink = Arc::new(CollectingSink::new());
        let engine = build_engine(Arc::clone(&sink));
        engine.process_event(&AlertEvent::NodeFailed { node_id: 1 }, Instant::now());
        let alerts = sink.collected();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Critical);
        assert_eq!(alerts[0].rule_name, "node_failed");
    }

    #[test]
    fn test_quorum_lost_fires_critical() {
        let sink = Arc::new(CollectingSink::new());
        let engine = build_engine(Arc::clone(&sink));
        engine.process_event(
            &AlertEvent::QuorumLost {
                cluster_size: 5,
                reachable: 2,
            },
            Instant::now(),
        );
        let alerts = sink.collected();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Critical);
        assert_eq!(alerts[0].rule_name, "quorum_lost");
    }

    #[test]
    fn test_leader_changed_fires_warning() {
        let sink = Arc::new(CollectingSink::new());
        let engine = build_engine(Arc::clone(&sink));
        engine.process_event(
            &AlertEvent::LeaderChanged {
                old_leader: Some(1),
                new_leader: 2,
            },
            Instant::now(),
        );
        let alerts = sink.collected();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Warning);
        assert_eq!(alerts[0].rule_name, "leader_changed");
    }

    #[test]
    fn test_slow_replication_fires_warning_above_threshold() {
        let sink = Arc::new(CollectingSink::new());
        let engine = build_engine(Arc::clone(&sink));
        engine.process_event(
            &AlertEvent::SlowReplication {
                follower: 3,
                lag_entries: 200,
            },
            Instant::now(),
        );
        let alerts = sink.collected();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Warning);
        assert_eq!(alerts[0].rule_name, "slow_replication");
    }

    #[test]
    fn test_slow_replication_silent_below_threshold() {
        let sink = Arc::new(CollectingSink::new());
        let engine = build_engine(Arc::clone(&sink));
        engine.process_event(
            &AlertEvent::SlowReplication {
                follower: 3,
                lag_entries: 50, // below 100 threshold
            },
            Instant::now(),
        );
        assert!(sink.collected().is_empty());
    }

    #[test]
    fn test_dedup_suppresses_repeat_within_window() {
        let sink = Arc::new(CollectingSink::new());
        let engine = build_engine(Arc::clone(&sink));
        let t0 = Instant::now();
        let event = AlertEvent::NodeFailed { node_id: 7 };
        engine.process_event(&event, t0);
        // Second fire 1 second later — still within 60-second window
        engine.process_event(&event, t0 + Duration::from_secs(1));
        assert_eq!(
            sink.collected().len(),
            1,
            "second fire should be suppressed"
        );
    }

    #[test]
    fn test_dedup_refires_after_window() {
        let sink = Arc::new(CollectingSink::new());
        let engine = build_engine(Arc::clone(&sink));
        let t0 = Instant::now();
        let event = AlertEvent::NodeFailed { node_id: 8 };
        engine.process_event(&event, t0);
        // Second fire 61 seconds later — window has expired
        engine.process_event(&event, t0 + Duration::from_secs(61));
        assert_eq!(
            sink.collected().len(),
            2,
            "should refire after window expires"
        );
    }

    #[test]
    fn test_collecting_sink_captures_alerts() {
        let sink = Arc::new(CollectingSink::new());
        let engine = build_engine(Arc::clone(&sink));
        let t0 = Instant::now();
        engine.process_event(&AlertEvent::NodeFailed { node_id: 10 }, t0);
        engine.process_event(
            &AlertEvent::QuorumLost {
                cluster_size: 3,
                reachable: 1,
            },
            t0,
        );
        let alerts = sink.collected();
        assert_eq!(alerts.len(), 2);
        sink.clear();
        assert!(sink.collected().is_empty());
    }
}
