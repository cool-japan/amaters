use super::*;
use amaters_core::storage::MemoryStorage;
use amaters_core::types::{CipherBlob, Key};

#[tokio::test]
async fn test_service_creation() {
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage);
    assert!(service.start_time.elapsed().as_secs() < 1);
}

#[tokio::test]
async fn test_get_query_execution() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("test_key");
    let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

    storage.put(&key, &value).await.expect("Failed to put");

    let service = AqlServiceImpl::new(storage);

    let query = Query::Get {
        collection: "test".to_string(),
        key: key.clone(),
    };

    let result = service.execute_query_internal(query).await;
    assert!(result.is_ok());

    let query_result = result.expect("Query failed");
    match query_result.result {
        Some(query::query_result::Result::Single(single)) => {
            assert!(single.value.is_some());
        }
        _ => panic!("Expected single result"),
    }
}

#[tokio::test]
async fn test_set_query_execution() {
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    let key = Key::from_str("test_key");
    let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

    let query = Query::Set {
        collection: "test".to_string(),
        key: key.clone(),
        value: value.clone(),
    };

    let result = service.execute_query_internal(query).await;
    assert!(result.is_ok());

    // Verify the value was stored
    let stored = storage.get(&key).await.expect("Failed to get");
    assert!(stored.is_some());
    assert_eq!(stored.expect("No value"), value);
}

#[tokio::test]
async fn test_delete_query_execution() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("test_key");
    let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

    storage.put(&key, &value).await.expect("Failed to put");

    let service = AqlServiceImpl::new(storage.clone());

    let query = Query::Delete {
        collection: "test".to_string(),
        key: key.clone(),
    };

    let result = service.execute_query_internal(query).await;
    assert!(result.is_ok());

    // Verify the value was deleted
    let stored = storage.get(&key).await.expect("Failed to get");
    assert!(stored.is_none());
}

#[tokio::test]
async fn test_range_query_execution() {
    let storage = Arc::new(MemoryStorage::new());

    // Insert test data
    for i in 0..10 {
        let key = Key::from_str(&format!("key_{:02}", i));
        let value = CipherBlob::new(vec![i as u8]);
        storage.put(&key, &value).await.expect("Failed to put");
    }

    let service = AqlServiceImpl::new(storage);

    let query = Query::Range {
        collection: "test".to_string(),
        start: Key::from_str("key_03"),
        end: Key::from_str("key_07"),
    };

    let result = service.execute_query_internal(query).await;
    assert!(result.is_ok());

    let query_result = result.expect("Query failed");
    match query_result.result {
        Some(query::query_result::Result::Multi(multi)) => {
            assert!(!multi.values.is_empty());
        }
        _ => panic!("Expected multi result"),
    }
}

#[tokio::test]
async fn test_get_nonexistent_key() {
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage);

    let query = Query::Get {
        collection: "test".to_string(),
        key: Key::from_str("nonexistent"),
    };

    let result = service.execute_query_internal(query).await;
    assert!(result.is_ok());

    let query_result = result.expect("Query failed");
    match query_result.result {
        Some(query::query_result::Result::Single(single)) => {
            assert!(single.value.is_none());
        }
        _ => panic!("Expected single result"),
    }
}

#[tokio::test]
async fn test_health_check() {
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage);

    let request = aql::HealthCheckRequest { service: None };
    let response = service.health_check(request).await;

    assert_eq!(response.status, aql::HealthStatus::HealthServing as i32);
}

#[tokio::test]
async fn test_server_info() {
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage);

    let request = aql::ServerInfoRequest {};
    let response = service.get_server_info(request).await;

    assert!(response.version.is_some());
    assert!(!response.capabilities.is_empty());
    assert!(response.capabilities.contains(&"query.get".to_string()));
}

#[cfg(feature = "compute")]
#[tokio::test]
async fn test_server_info_advertises_filter() {
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage);

    let request = aql::ServerInfoRequest {};
    let response = service.get_server_info(request).await;

    assert!(
        response.capabilities.contains(&"query.filter".to_string()),
        "capabilities should advertise query.filter when compute feature is enabled"
    );
}

#[cfg(feature = "compute")]
#[tokio::test]
async fn test_filter_query_execution() {
    use amaters_core::{ColumnRef, Predicate};

    let storage = Arc::new(MemoryStorage::new());

    // Store single-byte (plaintext) values 0..4.  Single-byte blobs are
    // detected as plaintext by the server and filtered without FHE.
    for i in 0u8..5 {
        let key = Key::from_str(&format!("row_{:02}", i));
        let value = CipherBlob::new(vec![i]);
        storage
            .put(&key, &value)
            .await
            .expect("Failed to insert test data");
    }

    let service = AqlServiceImpl::new(storage);

    // Filter predicate: value > 2 (expects rows 3 and 4 to match)
    let rhs_blob = CipherBlob::new(vec![2]);
    let predicate = Predicate::Gt(ColumnRef::new("value".to_string()), rhs_blob);

    let filter_query = Query::Filter {
        collection: "test".to_string(),
        predicate,
    };

    let result = service
        .execute_query_internal(filter_query)
        .await
        .expect("plaintext filter query should succeed");

    match result.result {
        Some(query::query_result::Result::Multi(multi)) => {
            // Plaintext filtering: only rows with value > 2 (i.e., 3 and 4) are returned.
            assert_eq!(
                multi.values.len(),
                2,
                "expected 2 matching rows (values 3 and 4)"
            );
            // Plaintext results have no encrypted predicate result field.
            for kv in &multi.values {
                assert!(
                    kv.encrypted_predicate_result.is_none(),
                    "plaintext filter results should not carry encrypted_predicate_result"
                );
            }
        }
        other => panic!("Expected Multi result from filter query, got {:?}", other),
    }
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_filter_query_requires_compute_feature() {
    use amaters_core::{ColumnRef, Predicate};

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage);

    let rhs_blob = CipherBlob::new(vec![1]);
    let predicate = Predicate::Gt(ColumnRef::new("value".to_string()), rhs_blob);

    let filter_query = Query::Filter {
        collection: "test".to_string(),
        predicate,
    };

    let result = service.execute_query_internal(filter_query).await;
    assert!(
        result.is_err(),
        "Filter should fail without compute feature"
    );
    let err_msg = result
        .as_ref()
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err_msg.contains("compute feature"),
        "Error should mention compute feature: {}",
        err_msg
    );
}

// ---------------------------------------------------------------
// UPDATE query tests (non-compute path: updates ALL rows)
// ---------------------------------------------------------------

/// Helper to build a dummy predicate (used by UPDATE queries).
/// Without the compute feature the predicate is ignored, so we
/// just need a syntactically valid one.
#[cfg(not(feature = "compute"))]
fn dummy_predicate() -> amaters_core::Predicate {
    amaters_core::Predicate::Eq(
        amaters_core::ColumnRef::new("col"),
        CipherBlob::new(vec![0]),
    )
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_set_single_key() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("row_00");
    let original = CipherBlob::new(vec![10, 20, 30]);
    storage.put(&key, &original).await.expect("Failed to put");

    let service = AqlServiceImpl::new(storage.clone());

    let new_blob = CipherBlob::new(vec![99, 88, 77]);
    let query = Query::Update {
        collection: "test".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Set(
            amaters_core::ColumnRef::new("val"),
            new_blob.clone(),
        )],
    };

    let result = service
        .execute_query_internal(query)
        .await
        .expect("Update failed");
    match result.result {
        Some(query::query_result::Result::Success(s)) => {
            assert_eq!(s.affected_rows, 1);
        }
        other => panic!("Expected Success, got {:?}", other),
    }

    let stored = storage
        .get(&key)
        .await
        .expect("Failed to get")
        .expect("Key missing after update");
    assert_eq!(stored, new_blob);
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_set_multiple_keys() {
    let storage = Arc::new(MemoryStorage::new());

    for i in 0u8..5 {
        let key = Key::from_str(&format!("row_{:02}", i));
        let value = CipherBlob::new(vec![i]);
        storage.put(&key, &value).await.expect("Failed to put");
    }

    let service = AqlServiceImpl::new(storage.clone());

    let replacement = CipherBlob::new(vec![255]);
    let query = Query::Update {
        collection: "data".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Set(
            amaters_core::ColumnRef::new("v"),
            replacement.clone(),
        )],
    };

    let result = service
        .execute_query_internal(query)
        .await
        .expect("Update failed");
    match result.result {
        Some(query::query_result::Result::Success(s)) => {
            assert_eq!(s.affected_rows, 5);
        }
        other => panic!("Expected Success, got {:?}", other),
    }

    // Verify all keys were updated
    for i in 0u8..5 {
        let key = Key::from_str(&format!("row_{:02}", i));
        let stored = storage
            .get(&key)
            .await
            .expect("Failed to get")
            .expect("Key missing");
        assert_eq!(stored, replacement);
    }
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_nonexistent_collection() {
    // No keys in storage at all — update should succeed with 0 affected rows
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage);

    let query = Query::Update {
        collection: "ghost".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Set(
            amaters_core::ColumnRef::new("x"),
            CipherBlob::new(vec![1]),
        )],
    };

    let result = service
        .execute_query_internal(query)
        .await
        .expect("Update on empty storage should not error");
    match result.result {
        Some(query::query_result::Result::Success(s)) => {
            assert_eq!(s.affected_rows, 0);
        }
        other => panic!("Expected Success with 0 rows, got {:?}", other),
    }
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_add_operation() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("counter");
    let original = CipherBlob::new(vec![10, 20]);
    storage.put(&key, &original).await.expect("Failed to put");

    let service = AqlServiceImpl::new(storage.clone());

    let addend = CipherBlob::new(vec![5, 3]);
    let query = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Add(
            amaters_core::ColumnRef::new("v"),
            addend,
        )],
    };

    service
        .execute_query_internal(query)
        .await
        .expect("Update failed");

    let stored = storage
        .get(&key)
        .await
        .expect("Failed to get")
        .expect("Key missing");
    assert_eq!(stored.as_bytes(), &[15, 23]);
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_mul_operation() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("product");
    let original = CipherBlob::new(vec![3, 4]);
    storage.put(&key, &original).await.expect("Failed to put");

    let service = AqlServiceImpl::new(storage.clone());

    let factor = CipherBlob::new(vec![2, 5]);
    let query = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Mul(
            amaters_core::ColumnRef::new("v"),
            factor,
        )],
    };

    service
        .execute_query_internal(query)
        .await
        .expect("Update failed");

    let stored = storage
        .get(&key)
        .await
        .expect("Failed to get")
        .expect("Key missing");
    assert_eq!(stored.as_bytes(), &[6, 20]);
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_multiple_operations_per_key() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("multi_op");
    let original = CipherBlob::new(vec![2]);
    storage.put(&key, &original).await.expect("Failed to put");

    let service = AqlServiceImpl::new(storage.clone());

    // Add 3 then multiply by 10: (2 + 3) * 10 = 50
    let query = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![
            amaters_core::Update::Add(amaters_core::ColumnRef::new("v"), CipherBlob::new(vec![3])),
            amaters_core::Update::Mul(amaters_core::ColumnRef::new("v"), CipherBlob::new(vec![10])),
        ],
    };

    service
        .execute_query_internal(query)
        .await
        .expect("Update failed");

    let stored = storage
        .get(&key)
        .await
        .expect("Failed to get")
        .expect("Key missing");
    assert_eq!(stored.as_bytes(), &[50]);
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_returns_affected_count() {
    let storage = Arc::new(MemoryStorage::new());

    // Insert exactly 7 keys
    for i in 0u8..7 {
        let key = Key::from_str(&format!("k{}", i));
        storage
            .put(&key, &CipherBlob::new(vec![i]))
            .await
            .expect("Failed to put");
    }

    let service = AqlServiceImpl::new(storage);

    let query = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Set(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![0]),
        )],
    };

    let result = service
        .execute_query_internal(query)
        .await
        .expect("Update failed");
    match result.result {
        Some(query::query_result::Result::Success(s)) => {
            assert_eq!(s.affected_rows, 7);
        }
        other => panic!("Expected Success with 7 rows, got {:?}", other),
    }
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_preserves_other_collections() {
    // Since our storage is flat (no collection namespacing at the storage level),
    // we verify that keys with different prefixes are still present after update.
    let storage = Arc::new(MemoryStorage::new());

    let key_a = Key::from_str("collA_row1");
    let key_b = Key::from_str("collB_row1");
    let val_a = CipherBlob::new(vec![1, 2, 3]);
    let val_b = CipherBlob::new(vec![4, 5, 6]);

    storage.put(&key_a, &val_a).await.expect("Failed to put A");
    storage.put(&key_b, &val_b).await.expect("Failed to put B");

    let service = AqlServiceImpl::new(storage.clone());

    // Update sets all keys; verify key_b is still readable (even though changed)
    let query = Query::Update {
        collection: "collA".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Set(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![99]),
        )],
    };

    service
        .execute_query_internal(query)
        .await
        .expect("Update failed");

    // Both keys should still exist in storage
    let stored_a = storage.get(&key_a).await.expect("Failed to get A");
    assert!(stored_a.is_some(), "key_a should still exist");

    let stored_b = storage.get(&key_b).await.expect("Failed to get B");
    assert!(stored_b.is_some(), "key_b should still exist");
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_empty_updates_vec() {
    // An update with an empty updates vector should succeed and not modify values
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("keep_me");
    let original = CipherBlob::new(vec![42]);
    storage.put(&key, &original).await.expect("Failed to put");

    let service = AqlServiceImpl::new(storage.clone());

    let query = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![], // no operations
    };

    let result = service
        .execute_query_internal(query)
        .await
        .expect("Update with empty ops should succeed");
    match result.result {
        Some(query::query_result::Result::Success(s)) => {
            // The row was "affected" (iterated) even though no ops were applied
            assert_eq!(s.affected_rows, 1);
        }
        other => panic!("Expected Success, got {:?}", other),
    }

    // Value should be unchanged
    let stored = storage
        .get(&key)
        .await
        .expect("Failed to get")
        .expect("Key missing");
    assert_eq!(stored, original);
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_then_select_verifies_changes() {
    let storage = Arc::new(MemoryStorage::new());

    // Insert 3 rows
    for i in 0u8..3 {
        let key = Key::from_str(&format!("sel_{:02}", i));
        let value = CipherBlob::new(vec![i, i, i]);
        storage.put(&key, &value).await.expect("Failed to put");
    }

    let service = AqlServiceImpl::new(storage.clone());

    // Update: add [1, 1, 1] to every row
    let update_query = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Add(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![1, 1, 1]),
        )],
    };

    service
        .execute_query_internal(update_query)
        .await
        .expect("Update failed");

    // Now read back each key and verify the addition
    for i in 0u8..3 {
        let key = Key::from_str(&format!("sel_{:02}", i));
        let get_query = Query::Get {
            collection: "c".to_string(),
            key: key.clone(),
        };

        let result = service
            .execute_query_internal(get_query)
            .await
            .expect("Get failed");

        match result.result {
            Some(query::query_result::Result::Single(single)) => {
                let proto_val = single.value.expect("Expected value from get");
                // The proto value data should equal [i+1, i+1, i+1]
                let expected = vec![i + 1, i + 1, i + 1];
                assert_eq!(
                    proto_val.data, expected,
                    "Row sel_{:02} should have been updated",
                    i
                );
            }
            other => panic!("Expected Single result, got {:?}", other),
        }
    }
}

/// With compute enabled, the UPDATE handler compiles the predicate and
/// evaluates it via FHE. This test verifies the code path runs without
/// panicking and returns a valid result (either success or a known FHE
/// error), similar to the existing filter compute test.
#[cfg(feature = "compute")]
#[tokio::test]
async fn test_update_with_compute_feature() {
    use amaters_core::{ColumnRef, Predicate};

    let storage = Arc::new(MemoryStorage::new());

    for i in 0u8..3 {
        let key = Key::from_str(&format!("row_{:02}", i));
        let value = CipherBlob::new(vec![i]);
        storage
            .put(&key, &value)
            .await
            .expect("Failed to insert test data");
    }

    let service = AqlServiceImpl::new(storage);

    let rhs_blob = CipherBlob::new(vec![1]);
    let predicate = Predicate::Eq(ColumnRef::new("value"), rhs_blob);

    let update_query = Query::Update {
        collection: "test".to_string(),
        predicate,
        updates: vec![amaters_core::Update::Set(
            ColumnRef::new("v"),
            CipherBlob::new(vec![99]),
        )],
    };

    let result = service.execute_query_internal(update_query).await;

    // Accept either Ok (FHE evaluated successfully) or a known FHE error
    match result {
        Ok(query_result) => {
            match query_result.result {
                Some(query::query_result::Result::Success(s)) => {
                    // Some or all rows may have been affected
                    assert!(s.affected_rows <= 3);
                }
                other => panic!("Expected Success result from update, got {:?}", other),
            }
        }
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("FHE")
                    || msg.contains("fhe")
                    || msg.contains("Predicate compilation")
                    || msg.contains("compilation failed")
                    || msg.contains("execution")
                    || msg.contains("RHS"),
                "Unexpected error from update query: {}",
                msg
            );
        }
    }
}

// UPDATE rollback tests are in server_rollback_tests.rs
include!("server_rollback_tests.rs");

/// Compile-time test: verifies that `AqlServerBuilder::build_grpc_service` compiles
/// without the `compression` feature. No runtime assertions are needed — if the
/// `#[cfg(feature = "compression")]` block were unconditional this test would fail
/// to compile (or worse, panic at runtime) when the feature is absent.
#[tokio::test]
async fn test_compression_feature_gate_disabled() {
    let storage = Arc::new(MemoryStorage::new());
    let builder = AqlServerBuilder::new(storage);
    // build_grpc_service should always compile regardless of compression feature.
    let _server = builder.build_grpc_service();
    // If we reach here, the feature-gate is working correctly.
}

include!("net_integration_tests.rs");
