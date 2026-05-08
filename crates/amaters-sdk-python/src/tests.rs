//! Tests for the Python SDK bindings

use crate::config::{PyClientConfig, PyRetryConfig};
use crate::helpers::classify_sdk_error;
use crate::streaming::{PyBatchStreamIterator, PyStreamIterator};
use crate::types::{
    BatchResultItem, PyBatchResult, PyKey, PyScanResult, SendableQueryResult,
    query_result_to_sendable,
};
use crate::{SATURATED_END_KEY_LEN, prefix_upper_bound, saturated_end_key};
use amaters_core::{CipherBlob, Key};
use amaters_sdk_rust::QueryResult;

#[test]
fn test_client_config() {
    let config = PyClientConfig::new("http://localhost:50051".to_string(), 10, 30, 10);
    assert_eq!(config.server_addr, "http://localhost:50051");
    assert_eq!(config.connect_timeout_secs, 10);
    assert_eq!(config.request_timeout_secs, 30);
    assert_eq!(config.max_connections, 10);
}

#[test]
fn test_client_config_repr() {
    let config = PyClientConfig::new("http://localhost:50051".to_string(), 5, 15, 20);
    let repr = config.__repr__();
    assert!(repr.contains("localhost:50051"));
    assert!(repr.contains("5s"));
    assert!(repr.contains("15s"));
    assert!(repr.contains("20"));
}

#[test]
fn test_client_config_str() {
    let config = PyClientConfig::new("http://localhost:50051".to_string(), 5, 15, 20);
    let s = config.__str__();
    assert!(s.contains("localhost:50051"));
}

#[test]
fn test_client_config_getters() {
    let config = PyClientConfig::new("http://localhost:50051".to_string(), 7, 25, 15);
    assert_eq!(config.connect_timeout(), 7);
    assert_eq!(config.request_timeout(), 25);
    assert_eq!(config.max_connections(), 15);
}

#[test]
fn test_retry_config() {
    let config = PyRetryConfig::new(5, 200);
    assert_eq!(config.max_retries, 5);
    assert_eq!(config.initial_backoff_ms, 200);
}

#[test]
fn test_retry_config_repr() {
    let config = PyRetryConfig::new(5, 200);
    let repr = config.__repr__();
    assert!(repr.contains("5"));
    assert!(repr.contains("200ms"));
}

#[test]
fn test_retry_config_str() {
    let config = PyRetryConfig::new(5, 200);
    let s = config.__str__();
    assert!(s.contains("5 retries"));
    assert!(s.contains("200ms"));
}

#[test]
fn test_retry_config_str_no_retry() {
    let config = PyRetryConfig::no_retry();
    let s = config.__str__();
    assert!(s.contains("no retries"));
}

#[test]
fn test_no_retry() {
    let config = PyRetryConfig::no_retry();
    assert_eq!(config.max_retries, 0);
    assert_eq!(config.initial_backoff_ms, 0);
}

#[test]
fn test_retry_config_getters() {
    let config = PyRetryConfig::new(7, 500);
    assert_eq!(config.get_max_retries(), 7);
    assert_eq!(config.get_initial_backoff_ms(), 500);
}

#[test]
fn test_retry_into_rust() {
    let config = PyRetryConfig::new(5, 200);
    let rust = config.into_rust();
    assert_eq!(rust.max_retries, 5);
}

#[test]
fn test_config_into_rust() {
    let config = PyClientConfig::new("http://localhost:50051".to_string(), 10, 30, 10);
    let rust = config.into_rust();
    assert_eq!(rust.server_addr, "http://localhost:50051");
}

#[test]
fn test_config_with_retry_into_rust() {
    let mut config = PyClientConfig::new("http://localhost:50051".to_string(), 10, 30, 10);
    config.retry_config = Some(PyRetryConfig::new(5, 200));
    let rust = config.into_rust();
    assert_eq!(rust.server_addr, "http://localhost:50051");
    assert_eq!(rust.retry_config.max_retries, 5);
}

#[test]
fn test_batch_result_item_success() {
    let item = BatchResultItem::Success;
    assert!(matches!(item, BatchResultItem::Success));
}

#[test]
fn test_batch_result_item_value() {
    let data = vec![1, 2, 3, 4];
    let item = BatchResultItem::Value(data.clone());
    match item {
        BatchResultItem::Value(v) => assert_eq!(v, data),
        _ => panic!("expected Value variant"),
    }
}

#[test]
fn test_batch_result_item_not_found() {
    let item = BatchResultItem::NotFound;
    assert!(matches!(item, BatchResultItem::NotFound));
}

#[test]
fn test_batch_result_item_affected_rows() {
    let item = BatchResultItem::AffectedRows(42);
    match item {
        BatchResultItem::AffectedRows(n) => assert_eq!(n, 42),
        _ => panic!("expected AffectedRows variant"),
    }
}

#[test]
fn test_batch_result_len() {
    let result = PyBatchResult {
        results: vec![
            BatchResultItem::Success,
            BatchResultItem::Value(vec![1, 2]),
            BatchResultItem::NotFound,
        ],
        index: std::sync::atomic::AtomicUsize::new(0),
    };
    assert_eq!(result.__len__(), 3);
}

#[test]
fn test_batch_result_repr() {
    let result = PyBatchResult {
        results: vec![BatchResultItem::Success, BatchResultItem::Value(vec![1, 2])],
        index: std::sync::atomic::AtomicUsize::new(0),
    };
    let repr = result.__repr__();
    assert!(repr.contains("2 operations"));
}

#[test]
fn test_batch_result_empty() {
    let result = PyBatchResult {
        results: vec![],
        index: std::sync::atomic::AtomicUsize::new(0),
    };
    assert_eq!(result.__len__(), 0);
    let repr = result.__repr__();
    assert!(repr.contains("0 operations"));
}

#[test]
fn test_key_len() {
    let key = PyKey {
        inner: Key::from_str("hello"),
    };
    assert_eq!(key.__len__(), 5);
}

#[test]
fn test_key_repr() {
    let key = PyKey {
        inner: Key::from_str("test_key"),
    };
    let repr = key.__repr__();
    assert!(repr.contains("test_key"));
    assert!(repr.starts_with("Key('"));
}

#[test]
fn test_key_str() {
    let key = PyKey {
        inner: Key::from_str("test_key"),
    };
    assert_eq!(key.__str__(), "test_key");
}

#[test]
fn test_key_equality() {
    let key1 = PyKey {
        inner: Key::from_str("same"),
    };
    let key2 = PyKey {
        inner: Key::from_str("same"),
    };
    let key3 = PyKey {
        inner: Key::from_str("different"),
    };
    assert!(key1.__eq__(&key2));
    assert!(!key1.__eq__(&key3));
}

#[test]
fn test_key_hash() {
    let key1 = PyKey {
        inner: Key::from_str("same"),
    };
    let key2 = PyKey {
        inner: Key::from_str("same"),
    };
    assert_eq!(key1.__hash__(), key2.__hash__());
}

#[test]
fn test_key_hash_different() {
    let key1 = PyKey {
        inner: Key::from_str("alpha"),
    };
    let key2 = PyKey {
        inner: Key::from_str("beta"),
    };
    assert_ne!(key1.__hash__(), key2.__hash__());
}

#[test]
fn test_classify_sdk_error_connection() {
    let err = amaters_sdk_rust::SdkError::Connection("refused".to_string());
    let (kind, msg) = classify_sdk_error(&err);
    assert_eq!(kind, "ConnectionError");
    assert!(msg.contains("Connection error"));
    assert!(msg.contains("refused"));
}

#[test]
fn test_classify_sdk_error_timeout() {
    let err = amaters_sdk_rust::SdkError::Timeout("30s exceeded".to_string());
    let (kind, msg) = classify_sdk_error(&err);
    assert_eq!(kind, "TimeoutError");
    assert!(msg.contains("Timeout"));
    assert!(msg.contains("30s exceeded"));
}

#[test]
fn test_classify_sdk_error_invalid_argument() {
    let err = amaters_sdk_rust::SdkError::InvalidArgument("bad key".to_string());
    let (kind, msg) = classify_sdk_error(&err);
    assert_eq!(kind, "ValueError");
    assert!(msg.contains("Invalid argument"));
    assert!(msg.contains("bad key"));
}

#[test]
fn test_classify_sdk_error_other() {
    let err = amaters_sdk_rust::SdkError::OperationFailed("something went wrong".to_string());
    let (kind, msg) = classify_sdk_error(&err);
    assert_eq!(kind, "RuntimeError");
    assert!(msg.contains("something went wrong"));
}

#[test]
fn test_classify_sdk_error_connection_contains_detail() {
    let err = amaters_sdk_rust::SdkError::Connection("tcp connect failed".to_string());
    let (kind, msg) = classify_sdk_error(&err);
    assert_eq!(kind, "ConnectionError");
    assert!(msg.contains("tcp connect failed"));
}

#[test]
fn test_client_repr_format() {
    let addr = "http://localhost:50051";
    let repr = format!("AmateRSClient(server_addr='{}')", addr);
    assert!(repr.contains("localhost:50051"));
}

#[test]
fn test_client_str_format() {
    let addr = "http://localhost:50051";
    let s = format!("AmateRSClient connected to {}", addr);
    assert!(s.contains("connected to"));
    assert!(s.contains("localhost:50051"));
}

#[test]
fn test_key_from_str() {
    let key = PyKey::from_str("user:123".to_string());
    assert_eq!(key.__str__(), "user:123");
    assert_eq!(key.__len__(), 8);
}

#[test]
fn test_key_to_string() {
    let key = PyKey {
        inner: Key::from_str("my_key"),
    };
    assert_eq!(key.to_string(), "my_key");
}

#[test]
fn test_query_result_conversions() {
    let success = QueryResult::Success { affected_rows: 5 };
    match &success {
        QueryResult::Success { affected_rows } => assert_eq!(*affected_rows, 5),
        _ => panic!("expected Success"),
    }

    let single_none = QueryResult::Single(None);
    assert!(matches!(&single_none, QueryResult::Single(None)));

    let blob = CipherBlob::new(vec![10, 20, 30]);
    let single_some = QueryResult::Single(Some(blob));
    match &single_some {
        QueryResult::Single(Some(b)) => assert_eq!(b.as_bytes(), &[10, 20, 30]),
        _ => panic!("expected Single(Some)"),
    }
}

#[test]
fn test_batch_result_multiple_types() {
    let result = PyBatchResult {
        results: vec![
            BatchResultItem::Success,
            BatchResultItem::Value(vec![10, 20]),
            BatchResultItem::NotFound,
            BatchResultItem::AffectedRows(3),
        ],
        index: std::sync::atomic::AtomicUsize::new(0),
    };
    assert_eq!(result.__len__(), 4);
}

#[test]
fn test_sendable_query_result_value() {
    let r = query_result_to_sendable(&QueryResult::Single(Some(CipherBlob::new(vec![1, 2, 3]))));
    match r {
        SendableQueryResult::Value(v) => assert_eq!(v, vec![1, 2, 3]),
        _ => panic!("expected Value"),
    }
}

#[test]
fn test_sendable_query_result_empty() {
    let r = query_result_to_sendable(&QueryResult::Single(None));
    assert!(matches!(r, SendableQueryResult::Empty));
}

#[test]
fn test_sendable_query_result_affected() {
    let r = query_result_to_sendable(&QueryResult::Success { affected_rows: 10 });
    match r {
        SendableQueryResult::AffectedRows(n) => assert_eq!(n, 10),
        _ => panic!("expected AffectedRows"),
    }
}

#[test]
fn test_sendable_query_result_multi() {
    let pairs = vec![
        (Key::from_str("k1"), CipherBlob::new(vec![1])),
        (Key::from_str("k2"), CipherBlob::new(vec![2])),
    ];
    let r = query_result_to_sendable(&QueryResult::Multi(pairs));
    match r {
        SendableQueryResult::Multi(p) => {
            assert_eq!(p.len(), 2);
            assert_eq!(p[0].0, b"k1");
            assert_eq!(p[1].1, vec![2]);
        }
        _ => panic!("expected Multi"),
    }
}

// ======== StreamIterator tests ========

fn make_test_items(n: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    (0..n)
        .map(|i| {
            (
                format!("key:{i}").into_bytes(),
                format!("val:{i}").into_bytes(),
            )
        })
        .collect()
}

#[test]
fn test_stream_iterator_yields_correct_chunks() {
    let items = make_test_items(10);
    let iter = PyStreamIterator {
        items: items.clone(),
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 3,
    };

    // First chunk: 3 items
    let chunk1 = iter
        .__next__()
        .expect("should not error")
        .expect("should have items");
    assert_eq!(chunk1.len(), 3);
    assert_eq!(chunk1[0].0, b"key:0");
    assert_eq!(chunk1[2].0, b"key:2");

    // Second chunk: 3 items
    let chunk2 = iter
        .__next__()
        .expect("should not error")
        .expect("should have items");
    assert_eq!(chunk2.len(), 3);
    assert_eq!(chunk2[0].0, b"key:3");

    // Third chunk: 3 items
    let chunk3 = iter
        .__next__()
        .expect("should not error")
        .expect("should have items");
    assert_eq!(chunk3.len(), 3);
    assert_eq!(chunk3[0].0, b"key:6");

    // Fourth chunk: 1 remaining item
    let chunk4 = iter
        .__next__()
        .expect("should not error")
        .expect("should have items");
    assert_eq!(chunk4.len(), 1);
    assert_eq!(chunk4[0].0, b"key:9");
}

#[test]
fn test_stream_iterator_raises_stop_iteration() {
    let items = make_test_items(2);
    let iter = PyStreamIterator {
        items,
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 10,
    };

    // First chunk gets all items
    let chunk = iter
        .__next__()
        .expect("should not error")
        .expect("should have items");
    assert_eq!(chunk.len(), 2);

    // Next call should return None (StopIteration in Python)
    let result = iter.__next__().expect("should not error");
    assert!(result.is_none());

    // Repeated calls should still return None
    let result2 = iter.__next__().expect("should not error");
    assert!(result2.is_none());
}

#[test]
fn test_stream_iterator_collect_returns_all_items() {
    let items = make_test_items(5);
    let iter = PyStreamIterator {
        items: items.clone(),
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 2,
    };

    let all = iter.collect();
    assert_eq!(all.len(), 5);
    assert_eq!(all[0].0, b"key:0");
    assert_eq!(all[4].0, b"key:4");

    // After collect, iterator should be exhausted
    let result = iter.__next__().expect("should not error");
    assert!(result.is_none());
}

#[test]
fn test_stream_iterator_collect_after_partial_iteration() {
    let items = make_test_items(6);
    let iter = PyStreamIterator {
        items,
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 2,
    };

    // Consume first chunk
    let chunk = iter
        .__next__()
        .expect("should not error")
        .expect("should have items");
    assert_eq!(chunk.len(), 2);

    // Collect remaining
    let remaining = iter.collect();
    assert_eq!(remaining.len(), 4);
    assert_eq!(remaining[0].0, b"key:2");
    assert_eq!(remaining[3].0, b"key:5");
}

#[test]
fn test_stream_iterator_empty_range() {
    let iter = PyStreamIterator {
        items: Vec::new(),
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 10,
    };

    assert_eq!(iter.__len__(), 0);
    let result = iter.__next__().expect("should not error");
    assert!(result.is_none());

    let collected = iter.collect();
    assert!(collected.is_empty());
}

#[test]
fn test_stream_iterator_single_item() {
    let items = make_test_items(1);
    let iter = PyStreamIterator {
        items,
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 10,
    };

    assert_eq!(iter.__len__(), 1);

    let chunk = iter
        .__next__()
        .expect("should not error")
        .expect("should have items");
    assert_eq!(chunk.len(), 1);
    assert_eq!(chunk[0].0, b"key:0");

    // Should be exhausted
    let result = iter.__next__().expect("should not error");
    assert!(result.is_none());
}

#[test]
fn test_stream_iterator_chunk_size_larger_than_result_set() {
    let items = make_test_items(3);
    let iter = PyStreamIterator {
        items,
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 100,
    };

    // Should yield all items in a single chunk
    let chunk = iter
        .__next__()
        .expect("should not error")
        .expect("should have items");
    assert_eq!(chunk.len(), 3);

    // Next call returns None
    let result = iter.__next__().expect("should not error");
    assert!(result.is_none());
}

#[test]
fn test_stream_iterator_remaining() {
    let items = make_test_items(5);
    let iter = PyStreamIterator {
        items,
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 2,
    };

    assert_eq!(iter.remaining(), 5);

    let _ = iter.__next__();
    assert_eq!(iter.remaining(), 3);

    let _ = iter.__next__();
    assert_eq!(iter.remaining(), 1);

    let _ = iter.__next__();
    assert_eq!(iter.remaining(), 0);
}

#[test]
fn test_stream_iterator_repr() {
    let items = make_test_items(10);
    let iter = PyStreamIterator {
        items,
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 3,
    };

    let repr = iter.__repr__();
    assert!(repr.contains("total=10"));
    assert!(repr.contains("position=0"));
    assert!(repr.contains("chunk_size=3"));

    let _ = iter.__next__();
    let repr2 = iter.__repr__();
    assert!(repr2.contains("position=3"));
}

// ======== BatchStreamIterator tests ========

#[test]
fn test_batch_stream_iterator_yields_chunks() {
    let results = vec![
        SendableQueryResult::Empty,
        SendableQueryResult::Value(vec![1, 2]),
        SendableQueryResult::AffectedRows(5),
        SendableQueryResult::Value(vec![3, 4]),
    ];
    let iter = PyBatchStreamIterator {
        results,
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 2,
    };

    let chunk1 = iter.__next__().expect("should have items");
    assert_eq!(chunk1.len(), 2);
    assert!(matches!(chunk1[0], SendableQueryResult::Empty));

    let chunk2 = iter.__next__().expect("should have items");
    assert_eq!(chunk2.len(), 2);

    // Exhausted
    assert!(iter.__next__().is_none());
}

#[test]
fn test_batch_stream_iterator_collect() {
    let results = vec![
        SendableQueryResult::Value(vec![1]),
        SendableQueryResult::Value(vec![2]),
        SendableQueryResult::Value(vec![3]),
    ];
    let iter = PyBatchStreamIterator {
        results,
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 1,
    };

    // Consume one chunk
    let _ = iter.__next__();
    assert_eq!(iter.remaining(), 2);

    // Collect remaining
    let remaining = iter.collect();
    assert_eq!(remaining.len(), 2);
    assert_eq!(iter.remaining(), 0);
}

#[test]
fn test_batch_stream_iterator_repr() {
    let iter = PyBatchStreamIterator {
        results: vec![SendableQueryResult::Empty; 5],
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 2,
    };
    let repr = iter.__repr__();
    assert!(repr.contains("total=5"));
    assert!(repr.contains("chunk_size=2"));
}

// ======== ScanResult / cursor-based pagination tests ========

#[test]
fn test_scan_result_properties() {
    let result = PyScanResult {
        results: vec![
            (b"key:0".to_vec(), b"val:0".to_vec()),
            (b"key:1".to_vec(), b"val:1".to_vec()),
        ],
        next_cursor: Some("2".to_string()),
    };

    assert_eq!(result.__len__(), 2);
    assert!(result.has_more());
    assert_eq!(result.next_cursor(), Some("2".to_string()));

    let results = result.results();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, b"key:0");
}

#[test]
fn test_scan_result_no_more_results() {
    let result = PyScanResult {
        results: vec![(b"key:0".to_vec(), b"val:0".to_vec())],
        next_cursor: None,
    };

    assert!(!result.has_more());
    assert!(result.next_cursor().is_none());
}

#[test]
fn test_scan_result_empty() {
    let result = PyScanResult {
        results: Vec::new(),
        next_cursor: None,
    };

    assert_eq!(result.__len__(), 0);
    assert!(!result.has_more());
}

#[test]
fn test_scan_result_repr() {
    let result = PyScanResult {
        results: vec![(b"a".to_vec(), b"b".to_vec())],
        next_cursor: Some("1".to_string()),
    };
    let repr = result.__repr__();
    assert!(repr.contains("count=1"));
    assert!(repr.contains("has_more=true"));

    let result_done = PyScanResult {
        results: Vec::new(),
        next_cursor: None,
    };
    let repr_done = result_done.__repr__();
    assert!(repr_done.contains("has_more=false"));
}

#[test]
fn test_scan_cursor_pagination_simulation() {
    // Simulate what scan() does internally for pagination
    let all_items: Vec<(Vec<u8>, Vec<u8>)> = (0..7)
        .map(|i| (format!("k{i}").into_bytes(), format!("v{i}").into_bytes()))
        .collect();
    let limit = 3;

    // Page 1: offset=0
    let offset = 0;
    let page_end = std::cmp::min(offset + limit, all_items.len());
    let page1 = all_items[offset..page_end].to_vec();
    let cursor1 = Some(page_end.to_string());
    assert_eq!(page1.len(), 3);
    assert_eq!(cursor1, Some("3".to_string()));

    // Page 2: offset=3
    let offset = 3;
    let page_end = std::cmp::min(offset + limit, all_items.len());
    let page2 = all_items[offset..page_end].to_vec();
    let cursor2 = Some(page_end.to_string());
    assert_eq!(page2.len(), 3);
    assert_eq!(cursor2, Some("6".to_string()));

    // Page 3: offset=6
    let offset = 6;
    let page_end = std::cmp::min(offset + limit, all_items.len());
    let page3 = all_items[offset..page_end].to_vec();
    let cursor3 = if page_end < all_items.len() {
        Some(page_end.to_string())
    } else {
        None
    };
    assert_eq!(page3.len(), 1);
    assert!(cursor3.is_none());
}

#[test]
fn test_stream_iterator_chunk_size_getter() {
    let iter = PyStreamIterator {
        items: Vec::new(),
        position: std::sync::atomic::AtomicUsize::new(0),
        chunk_size: 42,
    };
    assert_eq!(iter.chunk_size(), 42);
}

// ======== prefix_upper_bound / saturated_end_key tests ========

#[test]
fn test_prefix_upper_bound_empty_returns_none() {
    // An empty prefix matches every key, so there is no representable
    // upper bound.
    assert!(prefix_upper_bound(b"").is_none());
}

#[test]
fn test_prefix_upper_bound_single_byte_increments() {
    // ``[0x05]`` → ``[0x06]``.
    let result = prefix_upper_bound(&[0x05]).expect("should return upper bound");
    assert_eq!(result, vec![0x06]);
}

#[test]
fn test_prefix_upper_bound_saturated_bytes_returns_none() {
    // All-0xFF prefix has no successor in lexicographic order.
    assert!(prefix_upper_bound(&[0xFF]).is_none());
    assert!(prefix_upper_bound(&[0xFF, 0xFF]).is_none());
    assert!(prefix_upper_bound(&[0xFF, 0xFF, 0xFF, 0xFF]).is_none());
}

#[test]
fn test_prefix_upper_bound_truncates_trailing_ff() {
    // ``[0x10, 0x20, 0xFF]`` → ``[0x10, 0x21]``.
    let result = prefix_upper_bound(&[0x10, 0x20, 0xFF]).expect("should return upper bound");
    assert_eq!(result, vec![0x10, 0x21]);

    // ``[0x10, 0x20, 0xFF, 0xFF]`` → ``[0x10, 0x21]``.
    let result =
        prefix_upper_bound(&[0x10, 0x20, 0xFF, 0xFF]).expect("should return upper bound");
    assert_eq!(result, vec![0x10, 0x21]);
}

#[test]
fn test_prefix_upper_bound_middle_byte_increments() {
    // ``b"abc"`` → ``b"abd"``.
    let result = prefix_upper_bound(b"abc").expect("should return upper bound");
    assert_eq!(result, b"abd".to_vec());
}

#[test]
fn test_prefix_upper_bound_ascii_user_prefix() {
    // ``b"user:"`` → ``b"user;"`` (`':'` = 0x3A → ``';'`` = 0x3B).
    let result = prefix_upper_bound(b"user:").expect("should return upper bound");
    assert_eq!(result, b"user;".to_vec());
}

#[test]
fn test_prefix_upper_bound_zero_byte_increments() {
    // ``[0x00]`` → ``[0x01]``.
    let result = prefix_upper_bound(&[0x00]).expect("should return upper bound");
    assert_eq!(result, vec![0x01]);
}

#[test]
fn test_prefix_upper_bound_all_intermediate_zero() {
    // ``[0x00, 0x00, 0x05, 0xFF, 0xFF]`` → ``[0x00, 0x00, 0x06]``.
    let result =
        prefix_upper_bound(&[0x00, 0x00, 0x05, 0xFF, 0xFF]).expect("should return upper bound");
    assert_eq!(result, vec![0x00, 0x00, 0x06]);
}

#[test]
fn test_saturated_end_key_length() {
    // The sentinel must always be ``SATURATED_END_KEY_LEN`` bytes.
    let key = saturated_end_key();
    assert_eq!(key.len(), SATURATED_END_KEY_LEN);
    assert!(key.iter().all(|b| *b == 0xFF));
}

#[test]
fn test_saturated_end_key_is_strictly_greater_than_any_prefix_match() {
    // The sentinel must compare strictly greater than every plausible
    // key sharing a prefix shorter than `SATURATED_END_KEY_LEN`.
    let sentinel = saturated_end_key();

    let candidate1 = b"user:9999999999".to_vec();
    let candidate2 = vec![0xFF; SATURATED_END_KEY_LEN - 1];
    let mut candidate3 = vec![0xFF; SATURATED_END_KEY_LEN - 1];
    candidate3.push(0xFE);

    assert!(candidate1 < sentinel);
    assert!(candidate2 < sentinel);
    assert!(candidate3 < sentinel);
}

// ======== prefix_query semantic tests against the in-process MockServer ========

#[tokio::test(flavor = "multi_thread")]
async fn test_prefix_query_returns_matching_keys() -> anyhow::Result<()> {
    use amaters_sdk_rust::AmateRSClient;
    use amaters_sdk_rust::mock::MockServerBuilder;

    let mock = MockServerBuilder::new()
        .with_value("user:1", CipherBlob::new(b"alice".to_vec()))
        .with_value("user:2", CipherBlob::new(b"bob".to_vec()))
        .with_value("user:3", CipherBlob::new(b"carol".to_vec()))
        .with_value("admin:1", CipherBlob::new(b"root".to_vec()))
        .with_value("session:1", CipherBlob::new(b"s1".to_vec()))
        .start()
        .await?;

    let client = AmateRSClient::connect(&mock.endpoint()).await?;

    // Mirror what `prefix_query` does internally.
    let prefix = b"user:".to_vec();
    let end = prefix_upper_bound(&prefix).unwrap_or_else(saturated_end_key);

    let start_key = Key::new(prefix);
    let end_key = Key::new(end);

    let results = client.range("default", &start_key, &end_key).await?;
    let keys: Vec<Vec<u8>> = results.iter().map(|(k, _)| k.as_bytes().to_vec()).collect();

    assert_eq!(keys.len(), 3, "expected only the 3 user:* keys");
    assert!(keys.contains(&b"user:1".to_vec()));
    assert!(keys.contains(&b"user:2".to_vec()));
    assert!(keys.contains(&b"user:3".to_vec()));
    assert!(!keys.contains(&b"admin:1".to_vec()));
    assert!(!keys.contains(&b"session:1".to_vec()));

    mock.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_prefix_query_empty_prefix_returns_all() -> anyhow::Result<()> {
    use amaters_sdk_rust::AmateRSClient;
    use amaters_sdk_rust::mock::MockServerBuilder;

    let mock = MockServerBuilder::new()
        .with_value("a", CipherBlob::new(b"1".to_vec()))
        .with_value("m", CipherBlob::new(b"2".to_vec()))
        .with_value("z", CipherBlob::new(b"3".to_vec()))
        .start()
        .await?;

    let client = AmateRSClient::connect(&mock.endpoint()).await?;

    // Empty prefix → upper bound is None → falls back to saturated sentinel.
    let prefix: Vec<u8> = Vec::new();
    let end = prefix_upper_bound(&prefix).unwrap_or_else(saturated_end_key);
    assert_eq!(end.len(), SATURATED_END_KEY_LEN);

    let start_key = Key::new(prefix);
    let end_key = Key::new(end);

    let results = client.range("default", &start_key, &end_key).await?;
    assert_eq!(results.len(), 3, "empty prefix should match every key");

    mock.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_prefix_query_no_matches_returns_empty() -> anyhow::Result<()> {
    use amaters_sdk_rust::AmateRSClient;
    use amaters_sdk_rust::mock::MockServerBuilder;

    let mock = MockServerBuilder::new()
        .with_value("user:1", CipherBlob::new(b"alice".to_vec()))
        .with_value("user:2", CipherBlob::new(b"bob".to_vec()))
        .start()
        .await?;

    let client = AmateRSClient::connect(&mock.endpoint()).await?;

    let prefix = b"missing:".to_vec();
    let end = prefix_upper_bound(&prefix).unwrap_or_else(saturated_end_key);

    let start_key = Key::new(prefix);
    let end_key = Key::new(end);

    let results = client.range("default", &start_key, &end_key).await?;
    assert!(results.is_empty(), "no keys should match a non-existent prefix");

    mock.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_prefix_query_saturated_prefix_uses_sentinel() -> anyhow::Result<()> {
    use amaters_sdk_rust::AmateRSClient;
    use amaters_sdk_rust::mock::MockServerBuilder;

    // Pre-populate two keys whose first byte is 0xFF and one that is not.
    let key_a = Key::new(vec![0xFF, 0x01]);
    let key_b = Key::new(vec![0xFF, 0x02]);
    let key_c = Key::new(vec![0x10, 0x01]);

    let mock = MockServerBuilder::new()
        .with_value(key_a.clone(), CipherBlob::new(b"a".to_vec()))
        .with_value(key_b.clone(), CipherBlob::new(b"b".to_vec()))
        .with_value(key_c.clone(), CipherBlob::new(b"c".to_vec()))
        .start()
        .await?;

    let client = AmateRSClient::connect(&mock.endpoint()).await?;

    // Prefix [0xFF] saturates → upper bound is None → sentinel kicks in.
    let prefix = vec![0xFFu8];
    let end = prefix_upper_bound(&prefix).unwrap_or_else(saturated_end_key);
    assert_eq!(end.len(), SATURATED_END_KEY_LEN);

    let start_key = Key::new(prefix);
    let end_key = Key::new(end);

    let results = client.range("default", &start_key, &end_key).await?;
    let keys: Vec<Vec<u8>> = results.iter().map(|(k, _)| k.as_bytes().to_vec()).collect();

    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&vec![0xFF, 0x01]));
    assert!(keys.contains(&vec![0xFF, 0x02]));
    assert!(!keys.contains(&vec![0x10, 0x01]));

    mock.shutdown().await;
    Ok(())
}
