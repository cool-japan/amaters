//! Integration tests for AmateRS SDK

use amaters_core::{CipherBlob, Key, col};
use amaters_sdk_rust::{
    AmateRSClient, ClientConfig, FheEncryptor, MockServerBuilder, RetryConfig, SdkError, query,
};
use std::time::Duration;

#[tokio::test]
async fn test_client_connection_config() {
    let config = ClientConfig::new("http://localhost:50051")
        .with_connect_timeout(Duration::from_secs(5))
        .with_request_timeout(Duration::from_secs(10))
        .with_max_connections(5);

    assert_eq!(config.server_addr, "http://localhost:50051");
    assert_eq!(config.connect_timeout, Duration::from_secs(5));
    assert_eq!(config.max_connections, 5);
}

#[tokio::test]
async fn test_retry_config() {
    let retry = RetryConfig::new()
        .with_max_retries(5)
        .with_initial_backoff(Duration::from_millis(50));

    assert_eq!(retry.max_retries, 5);

    let backoff1 = retry.backoff_duration(1);
    let backoff2 = retry.backoff_duration(2);
    assert!(backoff2 > backoff1, "Backoff should increase");
}

#[tokio::test]
async fn test_no_retry_config() {
    let retry = RetryConfig::no_retry();
    assert_eq!(retry.max_retries, 0);
}

#[tokio::test]
async fn test_fhe_encryptor_stub() {
    // This tests the stub implementation (without fhe feature)
    #[cfg(not(feature = "fhe"))]
    {
        let encryptor = FheEncryptor::new().expect("create encryptor");
        let plaintext = b"test data";

        let ciphertext = encryptor.encrypt(plaintext).expect("encrypt");
        let decrypted = encryptor.decrypt(&ciphertext).expect("decrypt");

        assert_eq!(decrypted, plaintext);
    }
}

#[tokio::test]
async fn test_fhe_batch_encrypt() {
    #[cfg(not(feature = "fhe"))]
    {
        let encryptor = FheEncryptor::new().expect("create encryptor");
        let data: Vec<&[u8]> = vec![b"one", b"two", b"three"];

        let encrypted = encryptor.encrypt_batch(&data).expect("batch encrypt");
        assert_eq!(encrypted.len(), 3);

        // Decrypt and verify
        for (i, cipher) in encrypted.iter().enumerate() {
            let decrypted = encryptor.decrypt(cipher).expect("decrypt");
            assert_eq!(decrypted, data[i]);
        }
    }
}

#[tokio::test]
async fn test_query_builder() {
    let key = Key::from_str("test:1");
    let value = CipherBlob::new(vec![1, 2, 3]);

    // Test simple queries
    let q = query("users").get(key.clone());
    assert!(matches!(
        q,
        amaters_core::Query::Get { collection, .. } if collection == "users"
    ));

    let q = query("users").set(key.clone(), value.clone());
    assert!(matches!(
        q,
        amaters_core::Query::Set { collection, .. } if collection == "users"
    ));

    let q = query("users").delete(key.clone());
    assert!(matches!(
        q,
        amaters_core::Query::Delete { collection, .. } if collection == "users"
    ));
}

#[tokio::test]
async fn test_query_builder_filter() {
    let q = query("users")
        .where_clause()
        .eq(col("status"), CipherBlob::new(vec![1]))
        .build();

    match q {
        amaters_core::Query::Filter {
            collection,
            predicate,
        } => {
            assert_eq!(collection, "users");
            assert!(matches!(predicate, amaters_core::Predicate::Eq(_, _)));
        }
        _ => panic!("expected Filter query"),
    }
}

#[tokio::test]
async fn test_query_builder_complex_filter() {
    use amaters_core::Predicate;

    let q = query("users")
        .where_clause()
        .eq(col("active"), CipherBlob::new(vec![1]))
        .and(Predicate::Gt(col("age"), CipherBlob::new(vec![18])))
        .or(Predicate::Eq(col("role"), CipherBlob::new(vec![2])))
        .build();

    match q {
        amaters_core::Query::Filter {
            collection,
            predicate,
        } => {
            assert_eq!(collection, "users");
            // Predicate structure: ((active = 1 AND age > 18) OR role = 2)
            assert!(matches!(predicate, Predicate::Or(_, _)));
        }
        _ => panic!("expected Filter query"),
    }
}

#[tokio::test]
async fn test_query_builder_range() {
    let q = query("data").range(Key::from_str("a"), Key::from_str("z"));

    match q {
        amaters_core::Query::Range {
            collection,
            start,
            end,
        } => {
            assert_eq!(collection, "data");
            assert_eq!(start.to_string_lossy(), "a");
            assert_eq!(end.to_string_lossy(), "z");
        }
        _ => panic!("expected Range query"),
    }
}

#[tokio::test]
async fn test_error_is_retryable() {
    let err = SdkError::Connection("test".to_string());
    assert!(err.is_retryable());

    let err = SdkError::Timeout("test".to_string());
    assert!(err.is_retryable());

    let err = SdkError::InvalidArgument("test".to_string());
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn test_connection_pool_stats() {
    let config = ClientConfig::default();
    let pool = amaters_sdk_rust::connection::ConnectionPool::new(config);

    let stats = pool.stats();
    assert_eq!(stats.total_connections, 0);
    assert_eq!(stats.max_connections, 10);
}

// ---------------------------------------------------------------------------
// Tests that previously required a running server — now use MockServerBuilder
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_client_basic_operations() {
    let mock = MockServerBuilder::new()
        .start()
        .await
        .expect("start mock server");

    let client = AmateRSClient::connect(&mock.endpoint())
        .await
        .expect("connect to mock server");

    let collection = "test_collection";
    let key = Key::from_str("test_key");
    let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

    // Set
    client
        .set(collection, &key, &value)
        .await
        .expect("set value");

    // Get
    let retrieved = client
        .get(collection, &key)
        .await
        .expect("get value")
        .expect("value should exist");
    assert_eq!(retrieved, value);

    // Contains
    assert!(
        client.contains(collection, &key).await.expect("contains"),
        "key should exist"
    );

    // Delete
    client.delete(collection, &key).await.expect("delete");

    // Verify deletion
    let retrieved = client.get(collection, &key).await.expect("get value");
    assert!(retrieved.is_none(), "value should not exist after delete");

    mock.shutdown().await;
}

#[tokio::test]
async fn test_client_with_encryptor() {
    let mock = MockServerBuilder::new()
        .start()
        .await
        .expect("start mock server");

    let encryptor = FheEncryptor::new().expect("create encryptor");
    let client = AmateRSClient::connect(&mock.endpoint())
        .await
        .expect("connect")
        .with_encryptor(encryptor);

    assert!(client.encryptor().is_some());

    mock.shutdown().await;
}

#[tokio::test]
async fn test_client_health_check() {
    let mock = MockServerBuilder::new()
        .start()
        .await
        .expect("start mock server");

    let client = AmateRSClient::connect(&mock.endpoint())
        .await
        .expect("connect");

    client.health_check().await.expect("health check");

    mock.shutdown().await;
}

#[tokio::test]
async fn test_client_batch_operations() {
    let mock = MockServerBuilder::new()
        .start()
        .await
        .expect("start mock server");

    let client = AmateRSClient::connect(&mock.endpoint())
        .await
        .expect("connect");

    let queries = vec![
        query("users").set(Key::from_str("u1"), CipherBlob::new(vec![1])),
        query("users").set(Key::from_str("u2"), CipherBlob::new(vec![2])),
        query("users").set(Key::from_str("u3"), CipherBlob::new(vec![3])),
    ];

    let results = client.execute_batch(queries).await.expect("execute batch");
    assert_eq!(results.len(), 3);

    mock.shutdown().await;
}
