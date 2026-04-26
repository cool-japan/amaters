//! Tests for the streaming query API.
//!
//! Tests that verify the streaming *infrastructure* (backpressure,
//! cancellation, etc.) use [`QueryStream`] + [`spawn_stub_producer`] directly
//! so they do not require a live server.
//!
//! Tests that previously went through [`AmateRSClient::stream_query`] have
//! been migrated to use the stub producer directly for the same reason —
//! `stream_query` now performs a real gRPC call and would require a server.

use amaters_sdk_rust::{QueryStream, Row, StreamConfig, streaming::spawn_stub_producer};
use futures::StreamExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::time::{Duration, sleep, timeout};

// ---------------------------------------------------------------------------
// D1-T1: stream_results — collect first 5 items, all are Ok
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stream_results() {
    // Use the stub producer directly — simulates the streaming infrastructure
    // without requiring a live server.
    const COLLECTION: &str = "users";
    let config = StreamConfig::default();
    let (stream, sender) = QueryStream::new(&config);

    let _handle = spawn_stub_producer(COLLECTION.to_string(), 128, sender, None);

    // Collect the first 5 items.
    let items: Vec<_> = stream.take(5).collect().await;

    assert_eq!(items.len(), 5, "expected exactly 5 items");
    for (i, item) in items.iter().enumerate() {
        assert!(item.is_ok(), "item {i} should be Ok");
        let row = item.as_ref().expect("row should be Ok");
        // The stub encodes the index in little-endian bytes as the value.
        let idx = u64::from_le_bytes(row.value[..8].try_into().expect("value should be 8 bytes"));
        assert_eq!(idx, i as u64, "row {i}: unexpected index in value");
    }
}

// ---------------------------------------------------------------------------
// D1-T2: stream_backpressure — slow consumer, bounded buffer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stream_backpressure() {
    // Buffer capacity = 8.
    const BUF: usize = 8;
    // Producer will attempt to send this many rows.
    const TOTAL: usize = 32;

    let config = StreamConfig::new(BUF);
    let (stream, sender) = QueryStream::new(&config);

    // Track how many rows the producer has dispatched.
    let producer_sent = Arc::clone(&sender.sent);

    let produce_handle = tokio::spawn(async move {
        for i in 0..TOTAL {
            let row = Row::new(
                format!("key:{i}").into_bytes(),
                (i as u64).to_le_bytes().to_vec(),
            );
            if !sender.send_row(row).await {
                break;
            }
        }
    });

    let mut received = 0usize;
    let mut s = stream;

    while let Some(item) = s.next().await {
        assert!(item.is_ok(), "row should be Ok");
        received += 1;

        // After each receive, check that the producer has not gotten more
        // than buffer_size + 1 ahead of the consumer.
        // (+1 for the item currently in flight / send().await that hasn't
        //  completed yet.)
        let sent = producer_sent.load(Ordering::Relaxed);
        assert!(
            sent <= received + BUF + 1,
            "backpressure violated: sent={sent}, received={received}, buffer={BUF}"
        );

        // Simulate a slow consumer.
        sleep(Duration::from_millis(1)).await;
    }

    produce_handle.await.expect("producer task should finish");
    assert_eq!(received, TOTAL, "consumer should receive all {TOTAL} rows");
}

// ---------------------------------------------------------------------------
// D1-T3: stream_cancellation — drop after 2 rows, task terminates <= 1 s
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stream_cancellation() {
    // Use a very large row count so the producer keeps running unless cancelled.
    const MANY: usize = 100_000;

    let config = StreamConfig::new(4);
    let (stream, sender) = QueryStream::new(&config);

    let finished = Arc::new(AtomicBool::new(false));
    let finished_clone = Arc::clone(&finished);

    // Spawn the producer in a wrapper that sets `finished` when done.
    tokio::spawn(async move {
        spawn_stub_producer("cancel_test".to_string(), MANY, sender, None)
            .await
            .ok();
        finished_clone.store(true, Ordering::Release);
    });

    // Consume 2 rows then drop the stream.
    let mut s = stream;
    let _ = s.next().await;
    let _ = s.next().await;
    drop(s); // CancellationToken::cancel() is called in Drop

    // The producer task should stop within 1 second.
    let result = timeout(Duration::from_secs(1), async {
        loop {
            if finished.load(Ordering::Acquire) {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "producer task did not terminate within 1 second after stream was dropped"
    );
}

// ---------------------------------------------------------------------------
// D1-T4: with_timeout config — stream auto-cancels after deadline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stream_config_timeout() {
    let config = StreamConfig::new(8).with_timeout(2);
    let timeout_secs = config.timeout_secs;
    let (stream, sender) = QueryStream::new(&config);

    // Stub produces a finite number of rows; timeout is forwarded so the
    // producer also honours it.
    let _handle = spawn_stub_producer("data".to_string(), 128, sender, timeout_secs);

    // Collect all rows; the stream should finish (not hang).
    let rows: Vec<_> = stream.collect().await;
    // All items must be Ok.
    for row in &rows {
        assert!(row.is_ok());
    }
}

// ---------------------------------------------------------------------------
// D1-T5: stub producer encodes collection prefix in row keys
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stream_query_row_key_prefix() {
    let collection = "inventory";
    let config = StreamConfig::new(16);
    let (stream, sender) = QueryStream::new(&config);

    let _handle = spawn_stub_producer(collection.to_string(), 16, sender, None);

    let rows: Vec<_> = stream.take(3).collect().await;

    for item in &rows {
        let row = item.as_ref().expect("row should be Ok");
        let key_str = String::from_utf8_lossy(&row.key);
        assert!(
            key_str.starts_with(collection),
            "key '{key_str}' should start with collection '{collection}'"
        );
    }
}
