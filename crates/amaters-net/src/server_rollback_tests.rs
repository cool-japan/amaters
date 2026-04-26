// UPDATE rollback tests — included from server.rs `mod tests`
//
// These tests verify that UPDATE query rollback works correctly within
// batch transactions. They are `cfg(not(feature = "compute"))` because
// the failing-filter trick relies on the compute feature being absent.

/// Helper: build a proto Query from a core Query (for batch tests)
#[cfg(not(feature = "compute"))]
fn to_proto(q: &Query) -> crate::proto::query::Query {
    crate::convert::query_to_proto(q).expect("query_to_proto failed")
}

/// Helper: build a Filter query that always fails without compute feature
#[cfg(not(feature = "compute"))]
fn failing_filter_query() -> Query {
    Query::Filter {
        collection: "x".to_string(),
        predicate: dummy_predicate(),
    }
}

/// Helper: execute batch and return the response
#[cfg(not(feature = "compute"))]
async fn run_batch(
    service: &AqlServiceImpl<MemoryStorage>,
    queries: Vec<Query>,
) -> aql::BatchResponse {
    let proto_queries: Vec<_> = queries.iter().map(|q| to_proto(q)).collect();
    let request = aql::BatchRequest {
        queries: proto_queries,
        request_id: Some("test".to_string()),
        timeout_ms: None,
        isolation_level: 0,
        version: None,
    };
    service.execute_batch(request).await
}

/// Helper: check that batch response is an error
#[cfg(not(feature = "compute"))]
fn assert_batch_error(resp: &aql::BatchResponse) {
    assert!(
        matches!(
            &resp.response,
            Some(aql::batch_response::Response::Error(_))
        ),
        "Expected batch error, got: {:?}",
        resp.response
    );
}

/// Helper: check that batch response is success
#[cfg(not(feature = "compute"))]
fn assert_batch_ok(resp: &aql::BatchResponse) {
    assert!(
        matches!(
            &resp.response,
            Some(aql::batch_response::Response::Results(_))
        ),
        "Expected batch results, got: {:?}",
        resp.response
    );
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_rollback_on_batch_failure() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("k1");
    let orig = CipherBlob::new(vec![10, 20]);
    storage.put(&key, &orig).await.expect("put");
    let service = AqlServiceImpl::new(storage.clone());

    let update = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Set(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![99]),
        )],
    };
    let resp = run_batch(&service, vec![update, failing_filter_query()]).await;
    assert_batch_error(&resp);

    let restored = storage.get(&key).await.expect("get").expect("value");
    assert_eq!(restored.as_bytes(), &[10, 20]);
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_rollback_preserves_values() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("preserve");
    let orig = CipherBlob::new(vec![1, 2, 3, 4, 5]);
    storage.put(&key, &orig).await.expect("put");
    let service = AqlServiceImpl::new(storage.clone());

    let update = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Add(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![100]),
        )],
    };
    let resp = run_batch(&service, vec![update, failing_filter_query()]).await;
    assert_batch_error(&resp);

    let restored = storage.get(&key).await.expect("get").expect("value");
    assert_eq!(
        restored.as_bytes(),
        orig.as_bytes(),
        "exact original bytes must be restored"
    );
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_rollback_multiple_keys() {
    let storage = Arc::new(MemoryStorage::new());
    let originals: Vec<(Key, CipherBlob)> = (0u8..5)
        .map(|i| {
            (
                Key::from_str(&format!("mk_{}", i)),
                CipherBlob::new(vec![i, i + 10]),
            )
        })
        .collect();
    for (k, v) in &originals {
        storage.put(k, v).await.expect("put");
    }
    let service = AqlServiceImpl::new(storage.clone());

    let update = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Set(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![0]),
        )],
    };
    let resp = run_batch(&service, vec![update, failing_filter_query()]).await;
    assert_batch_error(&resp);

    for (k, v) in &originals {
        let restored = storage.get(k).await.expect("get").expect("value");
        assert_eq!(restored.as_bytes(), v.as_bytes());
    }
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_rollback_in_mixed_batch() {
    let storage = Arc::new(MemoryStorage::new());
    let k1 = Key::from_str("mix_set");
    let k2 = Key::from_str("mix_upd");
    let k3 = Key::from_str("mix_del");
    let v1 = CipherBlob::new(vec![1]);
    let v2 = CipherBlob::new(vec![2]);
    let v3 = CipherBlob::new(vec![3]);
    storage.put(&k1, &v1).await.expect("put");
    storage.put(&k2, &v2).await.expect("put");
    storage.put(&k3, &v3).await.expect("put");
    let service = AqlServiceImpl::new(storage.clone());

    let set_q = Query::Set {
        collection: "c".to_string(),
        key: k1.clone(),
        value: CipherBlob::new(vec![11]),
    };
    let upd_q = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Set(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![22]),
        )],
    };
    let del_q = Query::Delete {
        collection: "c".to_string(),
        key: k3.clone(),
    };

    let resp = run_batch(&service, vec![set_q, upd_q, del_q, failing_filter_query()]).await;
    assert_batch_error(&resp);

    assert_eq!(
        storage.get(&k1).await.expect("get").expect("v").as_bytes(),
        &[1]
    );
    assert_eq!(
        storage.get(&k2).await.expect("get").expect("v").as_bytes(),
        &[2]
    );
    assert_eq!(
        storage.get(&k3).await.expect("get").expect("v").as_bytes(),
        &[3]
    );
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_rollback_empty_collection() {
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    let update = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Set(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![1]),
        )],
    };
    let resp = run_batch(&service, vec![update, failing_filter_query()]).await;
    assert_batch_error(&resp);

    let keys = storage.keys().await.expect("keys");
    assert!(
        keys.is_empty(),
        "no keys should remain after rollback on empty collection"
    );
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_no_rollback_on_success() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("succ");
    let orig = CipherBlob::new(vec![5]);
    storage.put(&key, &orig).await.expect("put");
    let service = AqlServiceImpl::new(storage.clone());

    let update = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Set(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![99]),
        )],
    };
    let get = Query::Get {
        collection: "c".to_string(),
        key: key.clone(),
    };
    let resp = run_batch(&service, vec![update, get]).await;
    assert_batch_ok(&resp);

    let val = storage.get(&key).await.expect("get").expect("v");
    assert_eq!(val.as_bytes(), &[99]);
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_update_rollback_new_keys_removed() {
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    // Set a new key, then Update (snapshots that key), then fail
    let k = Key::from_str("new_key");
    let set_q = Query::Set {
        collection: "c".to_string(),
        key: k.clone(),
        value: CipherBlob::new(vec![50]),
    };
    let upd_q = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Add(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![1]),
        )],
    };
    let resp = run_batch(&service, vec![set_q, upd_q, failing_filter_query()]).await;
    assert_batch_error(&resp);

    let val = storage.get(&k).await.expect("get");
    assert!(val.is_none(), "new key should be removed by Set rollback");
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_batch_with_update_first() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("uf");
    let orig = CipherBlob::new(vec![7, 8]);
    storage.put(&key, &orig).await.expect("put");
    let service = AqlServiceImpl::new(storage.clone());

    let update = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Mul(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![2]),
        )],
    };
    let resp = run_batch(&service, vec![update, failing_filter_query()]).await;
    assert_batch_error(&resp);

    let restored = storage.get(&key).await.expect("get").expect("v");
    assert_eq!(restored.as_bytes(), &[7, 8]);
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_batch_with_update_last() {
    let storage = Arc::new(MemoryStorage::new());
    let key = Key::from_str("ul");
    let orig = CipherBlob::new(vec![42]);
    storage.put(&key, &orig).await.expect("put");
    let service = AqlServiceImpl::new(storage.clone());

    let set_q = Query::Set {
        collection: "c".to_string(),
        key: key.clone(),
        value: CipherBlob::new(vec![100]),
    };
    let resp = run_batch(&service, vec![set_q, failing_filter_query()]).await;
    assert_batch_error(&resp);

    let restored = storage.get(&key).await.expect("get").expect("v");
    assert_eq!(restored.as_bytes(), &[42]);
}

#[cfg(not(feature = "compute"))]
#[tokio::test]
async fn test_rollback_order_independence() {
    let storage = Arc::new(MemoryStorage::new());
    let k1 = Key::from_str("ord_a");
    let k2 = Key::from_str("ord_b");
    let v1 = CipherBlob::new(vec![1, 1]);
    let v2 = CipherBlob::new(vec![2, 2]);
    storage.put(&k1, &v1).await.expect("put");
    storage.put(&k2, &v2).await.expect("put");
    let service = AqlServiceImpl::new(storage.clone());

    let upd1 = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Add(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![10]),
        )],
    };
    let set_q = Query::Set {
        collection: "c".to_string(),
        key: k1.clone(),
        value: CipherBlob::new(vec![77, 77]),
    };
    let upd2 = Query::Update {
        collection: "c".to_string(),
        predicate: dummy_predicate(),
        updates: vec![amaters_core::Update::Mul(
            amaters_core::ColumnRef::new("v"),
            CipherBlob::new(vec![3]),
        )],
    };

    let resp = run_batch(&service, vec![upd1, set_q, upd2, failing_filter_query()]).await;
    assert_batch_error(&resp);

    assert_eq!(
        storage.get(&k1).await.expect("get").expect("v").as_bytes(),
        &[1, 1]
    );
    assert_eq!(
        storage.get(&k2).await.expect("get").expect("v").as_bytes(),
        &[2, 2]
    );
}
