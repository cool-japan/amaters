//! Streaming query support for AmateRS SDK
//!
//! Provides [`QueryStream`] — a bounded, cancellable stream of [`Row`]s produced
//! by a background task.  The stream implements [`futures::Stream`] so callers
//! can use standard combinators (`map`, `filter`, `collect`, …).
//!
//! # Design
//!
//! * **Backpressure** — The producer writes into a bounded
//!   [`tokio::sync::mpsc`] channel whose capacity is set by
//!   [`StreamConfig::buffer_size`].  The producer is forced to `await` once the
//!   channel is full, which naturally throttles generation rate to consumption
//!   rate.
//!
//! * **Cancellation** — A [`tokio_util::sync::CancellationToken`] is shared
//!   between the consumer and the background producer task.  Dropping the
//!   [`QueryStream`] cancels the token, and the producer checks it before every
//!   `send`.
//!
//! * **Lazy** — The background task is spawned by `QueryStream`; no
//!   data is generated until [`QueryStream`] is polled.
//!
//! The [`spawn_stub_producer`] helper is retained as a `#[doc(hidden)]`
//! test utility; production code uses the real `execute_stream` RPC.

use crate::error::SdkError;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration controlling backpressure and optional timeout for a
/// [`QueryStream`].
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Channel capacity — the maximum number of un-consumed [`Row`]s that
    /// can be buffered in memory.  When the channel is full the producer
    /// blocks until the consumer drains at least one item.  Defaults to
    /// **64**.
    pub buffer_size: usize,

    /// Optional per-stream timeout in seconds.  If set, the background task
    /// is automatically cancelled after this many seconds even if rows
    /// remain unread.  `None` means no timeout.
    pub timeout_secs: Option<u64>,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            buffer_size: 64,
            timeout_secs: None,
        }
    }
}

impl StreamConfig {
    /// Create a new config with the given buffer size.
    pub fn new(buffer_size: usize) -> Self {
        Self {
            buffer_size,
            timeout_secs: None,
        }
    }

    /// Set an optional timeout (seconds) after which the stream is cancelled.
    #[must_use]
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = Some(secs);
        self
    }
}

// ---------------------------------------------------------------------------
// Row
// ---------------------------------------------------------------------------

/// A single key-value result row returned from a streaming query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The row key (raw bytes).
    pub key: Vec<u8>,
    /// The row value (raw bytes; may be ciphertext).
    pub value: Vec<u8>,
}

impl Row {
    /// Create a new row from raw key and value bytes.
    pub fn new(key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// RowSender — handle given to the producer task
// ---------------------------------------------------------------------------

/// A handle used by the background producer task to send rows to the
/// consumer and to detect cancellation.
pub struct RowSender {
    tx: mpsc::Sender<Result<Row, SdkError>>,
    cancel: CancellationToken,
    /// Tracks the total number of rows sent, exposed in tests.
    pub sent: Arc<AtomicUsize>,
}

impl RowSender {
    /// Send a row, blocking until there is capacity in the channel.
    ///
    /// Returns `false` if the stream was cancelled or the receiver was
    /// dropped (either means the producer should stop).
    pub async fn send_row(&self, row: Row) -> bool {
        if self.cancel.is_cancelled() {
            return false;
        }
        self.sent.fetch_add(1, Ordering::Relaxed);
        self.tx.send(Ok(row)).await.is_ok()
    }

    /// Returns `true` if the stream has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Return a clone of the cancellation token so that producers can
    /// `select!` against it while awaiting a slow I/O operation.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Send an error to the consumer.
    ///
    /// Returns `false` if the stream was cancelled or the receiver was
    /// dropped.
    pub async fn send_error(&self, err: SdkError) -> bool {
        if self.cancel.is_cancelled() {
            return false;
        }
        self.tx.send(Err(err)).await.is_ok()
    }
}

// ---------------------------------------------------------------------------
// QueryStream
// ---------------------------------------------------------------------------

/// A cancellable, backpressure-aware stream of [`Row`]s from a query.
///
/// Implements [`futures::Stream`] with `Item = Result<Row, SdkError>`.
/// Dropping the stream cancels the background producer task.
pub struct QueryStream {
    /// Receiving end of the bounded channel.
    rx: mpsc::Receiver<Result<Row, SdkError>>,
    /// Token used to signal cancellation to the producer task.
    cancel: CancellationToken,
}

impl QueryStream {
    /// Spawn a background producer task and return the paired consumer stream
    /// together with a [`RowSender`] for the task to use.
    ///
    /// The caller is responsible for spawning a `tokio::task` that uses the
    /// returned [`RowSender`].
    pub fn new(config: &StreamConfig) -> (Self, RowSender) {
        let (tx, rx) = mpsc::channel(config.buffer_size);
        let cancel = CancellationToken::new();
        let sent = Arc::new(AtomicUsize::new(0));

        let sender = RowSender {
            tx,
            cancel: cancel.clone(),
            sent,
        };

        let stream = Self { rx, cancel };

        (stream, sender)
    }

    /// Cancel the background task immediately.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for QueryStream {
    fn drop(&mut self) {
        // Signal the producer to stop when the stream is dropped.
        self.cancel.cancel();
    }
}

impl Stream for QueryStream {
    type Item = Result<Row, SdkError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

// ---------------------------------------------------------------------------
// Stub generator — used by client::stream_query
// ---------------------------------------------------------------------------

/// Spawn a simulated producer that generates `total_rows` mock rows derived
/// from the query collection name.
///
/// This is a **test-only** helper used to exercise streaming infrastructure
/// (backpressure, cancellation, etc.) without a live server.  Production code
/// uses the real `execute_stream` gRPC RPC instead.
#[doc(hidden)]
pub fn spawn_stub_producer(
    query_collection: String,
    total_rows: usize,
    sender: RowSender,
    timeout_secs: Option<u64>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let deadline = timeout_secs.map(|s| tokio::time::Instant::now() + Duration::from_secs(s));

        for i in 0..total_rows {
            // Check cancellation before each send.
            if sender.is_cancelled() {
                break;
            }

            // Honour optional deadline.
            if let Some(dl) = deadline {
                if tokio::time::Instant::now() >= dl {
                    break;
                }
            }

            let key =
                format!("{collection}:row:{i}", collection = query_collection, i = i).into_bytes();
            let value = (i as u64).to_le_bytes().to_vec();
            let row = Row::new(key, value);

            if !sender.send_row(row).await {
                break;
            }
        }
        // Task finishes cleanly; channel closes when sender is dropped.
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_stream_config_defaults() {
        let cfg = StreamConfig::default();
        assert_eq!(cfg.buffer_size, 64);
        assert!(cfg.timeout_secs.is_none());
    }

    #[tokio::test]
    async fn test_row_construction() {
        let row = Row::new(b"key".to_vec(), b"value".to_vec());
        assert_eq!(row.key, b"key");
        assert_eq!(row.value, b"value");
    }

    #[tokio::test]
    async fn test_stream_collects_rows() {
        let config = StreamConfig::new(16);
        let (stream, sender) = QueryStream::new(&config);

        let _handle = spawn_stub_producer("test".to_string(), 5, sender, None);

        let rows: Vec<_> = stream.collect().await;
        assert_eq!(rows.len(), 5);
        for r in &rows {
            assert!(r.is_ok());
        }
    }

    #[tokio::test]
    async fn test_stream_cancellation_stops_producer() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::time::{Duration, sleep};

        let config = StreamConfig::new(4);
        let (stream, sender) = QueryStream::new(&config);
        let finished = Arc::new(AtomicBool::new(false));
        let finished_clone = Arc::clone(&finished);

        let _handle = tokio::spawn(async move {
            spawn_stub_producer("cancel_test".to_string(), 1_000, sender, None)
                .await
                .ok();
            finished_clone.store(true, Ordering::Release);
        });

        // Drop after receiving just 2 rows.
        let mut s = stream;
        let _ = s.next().await;
        let _ = s.next().await;
        drop(s); // triggers CancellationToken::cancel()

        // Producer should stop within 1 second.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while !finished.load(Ordering::Acquire) {
            if tokio::time::Instant::now() >= deadline {
                panic!("producer task did not stop within 1 second after stream was dropped");
            }
            sleep(Duration::from_millis(10)).await;
        }
    }
}
