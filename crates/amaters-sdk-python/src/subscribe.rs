//! Subscribe / watch API for AmateRS Python SDK.
//!
//! Provides [`PyChangeEvent`] and [`PyWatchHandle`] — a lightweight
//! poll-based change-notification mechanism that does not require an async
//! event loop on the Python side.

use pyo3::prelude::*;
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// A single change notification delivered to a watch handle.
///
/// Attributes:
///     collection (str): Name of the collection that changed.
///     event_type (str): One of ``"insert"``, ``"update"``, or ``"delete"``.
///     record_id (int | None): Identifier of the changed record, if available.
#[pyclass(name = "ChangeEvent")]
pub struct PyChangeEvent {
    /// Name of the collection that changed.
    #[pyo3(get)]
    pub collection: String,
    /// Event type: "insert", "update", or "delete".
    #[pyo3(get)]
    pub event_type: String,
    /// Optional record identifier.
    #[pyo3(get)]
    pub record_id: Option<u64>,
}

#[pymethods]
impl PyChangeEvent {
    fn __repr__(&self) -> String {
        format!(
            "ChangeEvent(collection={:?}, type={:?}, id={:?})",
            self.collection, self.event_type, self.record_id
        )
    }

    fn __str__(&self) -> String {
        self.__repr__()
    }
}

/// Shared event queue for one watch subscription.
///
/// Both the Rust-side emitter (inside the client) and the Python-side
/// poll consumer share this type via `Arc<Mutex<...>>`.
pub type EventQueue = Arc<Mutex<VecDeque<PyChangeEvent>>>;

/// A watch handle returned by ``AmateRSClient.watch()``.
///
/// Calling :meth:`poll_events` returns any events that have accumulated
/// since the last poll without blocking. Call :meth:`close` to unsubscribe.
///
/// Example:
///     >>> handle = client.watch("my_collection")
///     >>> events = handle.poll_events(10)
///     >>> handle.close()
#[pyclass(name = "WatchHandle")]
pub struct PyWatchHandle {
    pub collection: String,
    pub queue: EventQueue,
    pub closed: Arc<AtomicBool>,
}

#[pymethods]
impl PyWatchHandle {
    /// Return up to ``limit`` pending events (non-blocking).
    ///
    /// Args:
    ///     limit (int): Maximum number of events to return.
    ///
    /// Returns:
    ///     list[ChangeEvent]: Pending change events (may be empty).
    pub fn poll_events(&self, limit: usize) -> Vec<PyChangeEvent> {
        if self.closed.load(Ordering::Acquire) {
            return Vec::new();
        }
        let effective_limit = if limit == 0 { usize::MAX } else { limit };
        match self.queue.lock() {
            Ok(mut guard) => {
                let take = effective_limit.min(guard.len());
                guard.drain(..take).collect()
            }
            Err(_) => Vec::new(),
        }
    }

    /// Unsubscribe and close the watch handle.
    ///
    /// After calling this method, :meth:`poll_events` will always return an
    /// empty list.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Check whether the handle is still active.
    ///
    /// Returns:
    ///     bool: ``True`` if the handle has not been closed.
    pub fn is_active(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }

    fn __repr__(&self) -> String {
        format!(
            "WatchHandle(collection={:?}, active={})",
            self.collection,
            self.is_active()
        )
    }
}

/// Internal registry that maps collection names to a list of subscriber queues.
///
/// Stored inside `PyAmateRSClient` so that `watch()` and `emit_change()` share
/// the same registry.
#[derive(Default)]
pub struct WatchRegistry {
    /// collection name → list of subscriber queues (one per active watch handle)
    subscribers: std::collections::HashMap<String, Vec<EventQueue>>,
}

impl WatchRegistry {
    /// Register a new subscriber for `collection`.  Returns the queue that the
    /// new `PyWatchHandle` will poll.
    pub fn subscribe(&mut self, collection: &str) -> EventQueue {
        let queue: EventQueue = Arc::new(Mutex::new(VecDeque::new()));
        self.subscribers
            .entry(collection.to_string())
            .or_default()
            .push(Arc::clone(&queue));
        queue
    }

    /// Broadcast a change event to all subscribers of `collection`.
    ///
    /// Queues that have been closed (i.e. whose `Arc` strong count has dropped
    /// to 1, meaning only the registry itself holds a reference) are pruned.
    pub fn emit(&mut self, collection: &str, event_type: String, record_id: Option<u64>) {
        let Some(queues) = self.subscribers.get_mut(collection) else {
            return;
        };

        queues.retain(|q| {
            // Prune queues where the WatchHandle was dropped (Arc strong count == 1)
            if Arc::strong_count(q) <= 1 {
                return false;
            }
            let event = PyChangeEvent {
                collection: collection.to_string(),
                event_type: event_type.clone(),
                record_id,
            };
            if let Ok(mut guard) = q.lock() {
                guard.push_back(event);
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watch_handle_receives_events() {
        let queue: EventQueue = Arc::new(Mutex::new(VecDeque::new()));
        // Push a test event directly into the queue
        queue.lock().expect("lock").push_back(PyChangeEvent {
            collection: "test".into(),
            event_type: "insert".into(),
            record_id: Some(42),
        });

        let handle = PyWatchHandle {
            collection: "test".into(),
            queue: Arc::clone(&queue),
            closed: Arc::new(AtomicBool::new(false)),
        };

        let events = handle.poll_events(10);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "insert");
        assert_eq!(events[0].record_id, Some(42));
        assert_eq!(events[0].collection, "test");
    }

    #[test]
    fn test_watch_handle_respects_limit() {
        let queue: EventQueue = Arc::new(Mutex::new(VecDeque::new()));
        for i in 0..5 {
            queue.lock().expect("lock").push_back(PyChangeEvent {
                collection: "col".into(),
                event_type: "update".into(),
                record_id: Some(i),
            });
        }

        let handle = PyWatchHandle {
            collection: "col".into(),
            queue,
            closed: Arc::new(AtomicBool::new(false)),
        };

        let events = handle.poll_events(3);
        assert_eq!(events.len(), 3, "should return at most limit events");
    }

    #[test]
    fn test_watch_handle_closed() {
        let queue: EventQueue = Arc::new(Mutex::new(VecDeque::new()));
        let handle = PyWatchHandle {
            collection: "test".into(),
            queue,
            closed: Arc::new(AtomicBool::new(false)),
        };

        assert!(handle.is_active());
        handle.close();
        assert!(!handle.is_active());

        // Closed handle returns empty even if queue has events
        // (handled by poll_events checking closed flag)
    }

    #[test]
    fn test_poll_events_on_closed_handle_returns_empty() {
        let queue: EventQueue = Arc::new(Mutex::new(VecDeque::new()));
        queue.lock().expect("lock").push_back(PyChangeEvent {
            collection: "c".into(),
            event_type: "delete".into(),
            record_id: None,
        });

        let handle = PyWatchHandle {
            collection: "c".into(),
            queue,
            closed: Arc::new(AtomicBool::new(true)), // already closed
        };

        let events = handle.poll_events(10);
        assert!(events.is_empty(), "closed handle should return no events");
    }

    #[test]
    fn test_registry_emit_delivers_to_subscriber() {
        let mut registry = WatchRegistry::default();
        let queue = registry.subscribe("orders");

        // Simulate external reference keeping the WatchHandle alive
        let _handle_ref = Arc::clone(&queue);

        registry.emit("orders", "insert".into(), Some(1));
        registry.emit("orders", "update".into(), Some(2));

        let guard = queue.lock().expect("lock");
        assert_eq!(guard.len(), 2);
        assert_eq!(guard[0].event_type, "insert");
        assert_eq!(guard[1].event_type, "update");
    }

    #[test]
    fn test_registry_emit_no_subscribers_is_noop() {
        let mut registry = WatchRegistry::default();
        // Should not panic when no subscribers exist
        registry.emit("unknown_collection", "insert".into(), None);
    }

    #[test]
    fn test_registry_prunes_dropped_handles() {
        let mut registry = WatchRegistry::default();
        let queue = registry.subscribe("items");

        // Drop the extra reference so only registry holds it (strong count == 1)
        drop(queue);

        // emit should prune the dead subscriber without panicking
        registry.emit("items", "insert".into(), Some(99));

        // The subscriber list for "items" should now be empty
        let subs = registry
            .subscribers
            .get("items")
            .map(|v| v.len())
            .unwrap_or(0);
        assert_eq!(subs, 0, "dead subscriber should be pruned");
    }

    #[test]
    #[ignore = "requires live AmateRS server at localhost:50051"]
    fn test_live_server_insert_and_watch() {
        todo!()
    }
}
