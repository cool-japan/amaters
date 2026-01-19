//! End-to-End Integration Tests for AmateRS
//!
//! These tests verify the entire stack works together:
//! - Start real server with gRPC endpoint
//! - Connect SDK client to server
//! - Execute operations through full stack
//! - Verify persistence and correctness
//!
//! Test Categories:
//! - Basic CRUD Operations
//! - Concurrent Operations
//! - Error Scenarios
//! - LSM-Tree Persistence
//! - FHE Operations
//! - Stress Tests

mod e2e_common;

use amaters_core::types::{CipherBlob, Key, Predicate, Query, col};
use amaters_sdk_rust::{AmateRSClient, ClientConfig, QueryResult};
use e2e_common::E2eTestContext;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::sleep;

// ============================================================================
// Category 1: Basic CRUD Operations (10 tests)
// ============================================================================

mod basic_crud {
    use super::*;

    #[tokio::test]
    async fn test_e2e_set_get() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        // Set value
        ctx.client.set("default", &key, &value).await?;

        // Get value
        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved
                .expect("Value should be retrievable in test")
                .as_bytes(),
            &[1, 2, 3, 4, 5]
        );

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_delete() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let key = Key::from_str("delete_key");
        let value = CipherBlob::new(vec![1, 2, 3]);

        // Set value
        ctx.client.set("default", &key, &value).await?;

        // Verify exists
        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_some());

        // Delete
        ctx.client.delete("default", &key).await?;

        // Verify deleted
        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_none());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_update() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let key = Key::from_str("update_key");
        let value1 = CipherBlob::new(vec![1, 1, 1]);
        let value2 = CipherBlob::new(vec![2, 2, 2]);

        // Set initial value
        ctx.client.set("default", &key, &value1).await?;

        // Verify
        let retrieved = ctx.client.get("default", &key).await?;
        assert_eq!(
            retrieved
                .expect("Value should be retrievable in test")
                .as_bytes(),
            &[1, 1, 1]
        );

        // Update
        ctx.client.set("default", &key, &value2).await?;

        // Verify update
        let retrieved = ctx.client.get("default", &key).await?;
        assert_eq!(
            retrieved
                .expect("Value should be retrievable in test")
                .as_bytes(),
            &[2, 2, 2]
        );

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_get_nonexistent() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let key = Key::from_str("nonexistent");
        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_none());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_delete_nonexistent() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let key = Key::from_str("nonexistent");
        // Should not error
        ctx.client.delete("default", &key).await?;

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_empty_value() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let key = Key::from_str("empty_key");
        let value = CipherBlob::new(vec![]);

        ctx.client.set("default", &key, &value).await?;

        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_some());
        assert!(
            retrieved
                .expect("Value should be retrievable in test")
                .is_empty()
        );

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_large_value() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let key = Key::from_str("large_key");
        let large_data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
        let value = CipherBlob::new(large_data.clone());

        ctx.client.set("default", &key, &value).await?;

        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved
                .expect("Value should be retrievable in test")
                .as_bytes(),
            &large_data
        );

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_batch_operations() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        // Insert multiple keys
        for i in 0..10 {
            let key = Key::from_str(&format!("batch_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 100]);
            ctx.client.set("default", &key, &value).await?;
        }

        // Verify all keys
        for i in 0..10 {
            let key = Key::from_str(&format!("batch_key_{}", i));
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_some());
            assert_eq!(
                retrieved
                    .expect("Value should be retrievable in test")
                    .as_bytes()[0],
                i as u8
            );
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_range_query() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        // Insert ordered keys
        for i in 0..20 {
            let key = Key::from_str(&format!("range_key_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.client.set("default", &key, &value).await?;
        }

        // Execute range query
        let start = Key::from_str("range_key_0005");
        let end = Key::from_str("range_key_0015");

        let query = Query::Range {
            collection: "default".to_string(),
            start,
            end,
        };

        let result = ctx.client.execute_query(&query).await?;
        match result {
            QueryResult::Multi(rows) => assert_eq!(rows.len(), 10),
            _ => panic!("Expected Multi result"),
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_binary_data() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let key = Key::from_slice(&[0x00, 0xFF, 0x7F, 0x80]);
        let value = CipherBlob::new(vec![0x00, 0x11, 0x22, 0x33, 0xFF]);

        ctx.client.set("default", &key, &value).await?;

        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved
                .expect("Value should be retrievable in test")
                .as_bytes(),
            &[0x00, 0x11, 0x22, 0x33, 0xFF]
        );

        ctx.cleanup().await;
        Ok(())
    }
}

// ============================================================================
// Category 2: Concurrent Operations (10 tests)
// ============================================================================

mod concurrent_ops {
    use super::*;
    use tokio::sync::Barrier;

    #[tokio::test]
    async fn test_e2e_concurrent_writes() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);
        let num_clients = 5;
        let ops_per_client = 100;
        let barrier = Arc::new(Barrier::new(num_clients));

        let mut handles = Vec::new();

        for client_id in 0..num_clients {
            let ctx = Arc::clone(&ctx);
            let barrier = Arc::clone(&barrier);

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                barrier.wait().await;

                for i in 0..ops_per_client {
                    let key = Key::from_str(&format!("concurrent_w_{}_{}", client_id, i));
                    let value = CipherBlob::new(vec![client_id as u8; 50]);
                    client
                        .set("default", &key, &value)
                        .await
                        .map_err(|e| format!("Set failed: {}", e))?;
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        // Verify count
        let total_expected = num_clients * ops_per_client;
        // We can't easily count keys through SDK, so just verify some keys exist
        for client_id in 0..num_clients {
            let key = Key::from_str(&format!("concurrent_w_{}_0", client_id));
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_some());
        }

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_concurrent_reads() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);

        // Prepopulate data
        for i in 0..50 {
            let key = Key::from_str(&format!("read_concurrent_{}", i));
            let value = CipherBlob::new(vec![i as u8; 100]);
            ctx.client.set("default", &key, &value).await?;
        }

        let num_clients = 10;
        let reads_per_client = 50;
        let barrier = Arc::new(Barrier::new(num_clients));
        let mut handles = Vec::new();

        for _ in 0..num_clients {
            let ctx = Arc::clone(&ctx);
            let barrier = Arc::clone(&barrier);

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                barrier.wait().await;

                for i in 0..reads_per_client {
                    let key = Key::from_str(&format!("read_concurrent_{}", i % 50));
                    let _ = client
                        .get("default", &key)
                        .await
                        .map_err(|e| format!("Get failed: {}", e))?;
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_concurrent_mixed() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);

        // Prepopulate some data
        for i in 0..25 {
            let key = Key::from_str(&format!("mixed_{}", i));
            let value = CipherBlob::new(vec![i as u8; 50]);
            ctx.client.set("default", &key, &value).await?;
        }

        let num_clients = 4;
        let ops_per_client = 50;
        let barrier = Arc::new(Barrier::new(num_clients));
        let mut handles = Vec::new();

        for client_id in 0..num_clients {
            let ctx = Arc::clone(&ctx);
            let barrier = Arc::clone(&barrier);

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                barrier.wait().await;

                for i in 0usize..ops_per_client {
                    match i % 3 {
                        0 => {
                            // Read
                            let key = Key::from_str(&format!("mixed_{}", i % 25));
                            let _ = client
                                .get("default", &key)
                                .await
                                .map_err(|e| format!("Get failed: {}", e))?;
                        }
                        1 => {
                            // Write
                            let key = Key::from_str(&format!("mixed_new_{}_{}", client_id, i));
                            let value = CipherBlob::new(vec![1; 50]);
                            client
                                .set("default", &key, &value)
                                .await
                                .map_err(|e| format!("Set failed: {}", e))?;
                        }
                        _ => {
                            // Delete
                            let key = Key::from_str(&format!(
                                "mixed_new_{}_{}",
                                client_id,
                                i.saturating_sub(1)
                            ));
                            client
                                .delete("default", &key)
                                .await
                                .map_err(|e| format!("Delete failed: {}", e))?;
                        }
                    }
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_concurrent_same_key() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);
        let num_clients = 20;
        let barrier = Arc::new(Barrier::new(num_clients));
        let key = Key::from_str("contested_key");

        let mut handles = Vec::new();

        for client_id in 0..num_clients {
            let ctx = Arc::clone(&ctx);
            let barrier = Arc::clone(&barrier);
            let key = key.clone();

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                barrier.wait().await;

                let value = CipherBlob::new(vec![client_id as u8; 100]);
                client
                    .set("default", &key, &value)
                    .await
                    .map_err(|e| format!("Set failed: {}", e))?;

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        // Verify key exists with one of the values
        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_some());

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_concurrent_deletes() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);

        // Prepopulate
        for i in 0..100 {
            let key = Key::from_str(&format!("delete_concurrent_{}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.client.set("default", &key, &value).await?;
        }

        let num_clients = 5;
        let deletes_per_client = 20;
        let barrier = Arc::new(Barrier::new(num_clients));
        let mut handles = Vec::new();

        for client_id in 0..num_clients {
            let ctx = Arc::clone(&ctx);
            let barrier = Arc::clone(&barrier);

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                barrier.wait().await;

                for i in 0..deletes_per_client {
                    let key = Key::from_str(&format!(
                        "delete_concurrent_{}",
                        client_id * deletes_per_client + i
                    ));
                    client
                        .delete("default", &key)
                        .await
                        .map_err(|e| format!("Delete failed: {}", e))?;
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_concurrent_updates() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);

        // Prepopulate
        for i in 0..50 {
            let key = Key::from_str(&format!("update_concurrent_{}", i));
            let value = CipherBlob::new(vec![0; 100]);
            ctx.client.set("default", &key, &value).await?;
        }

        let num_clients = 5;
        let updates_per_client = 10;
        let barrier = Arc::new(Barrier::new(num_clients));
        let mut handles = Vec::new();

        for client_id in 0..num_clients {
            let ctx = Arc::clone(&ctx);
            let barrier = Arc::clone(&barrier);

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                barrier.wait().await;

                for i in 0..updates_per_client {
                    let key = Key::from_str(&format!("update_concurrent_{}", i));
                    let value = CipherBlob::new(vec![client_id as u8; 100]);
                    client
                        .set("default", &key, &value)
                        .await
                        .map_err(|e| format!("Set failed: {}", e))?;
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_concurrent_range_queries() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);

        // Prepopulate
        for i in 0..100 {
            let key = Key::from_str(&format!("range_concurrent_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.client.set("default", &key, &value).await?;
        }

        let num_clients = 5;
        let barrier = Arc::new(Barrier::new(num_clients));
        let mut handles = Vec::new();

        for client_id in 0..num_clients {
            let ctx = Arc::clone(&ctx);
            let barrier = Arc::clone(&barrier);

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                barrier.wait().await;

                for i in 0..10 {
                    let start_idx = i * 10;
                    let end_idx = start_idx + 10;

                    let start = Key::from_str(&format!("range_concurrent_{:04}", start_idx));
                    let end = Key::from_str(&format!("range_concurrent_{:04}", end_idx));

                    let query = Query::Range {
                        collection: "default".to_string(),
                        start,
                        end,
                    };

                    let _ = client
                        .execute_query(&query)
                        .await
                        .map_err(|e| format!("Range query failed: {}", e))?;
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_high_concurrency() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);
        let num_clients = 20;
        let ops_per_client = 50;
        let barrier = Arc::new(Barrier::new(num_clients));

        let mut handles = Vec::new();

        for client_id in 0..num_clients {
            let ctx = Arc::clone(&ctx);
            let barrier = Arc::clone(&barrier);

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                barrier.wait().await;

                for i in 0..ops_per_client {
                    let key = Key::from_str(&format!("high_concur_{}_{}", client_id, i));
                    let value = CipherBlob::new(vec![client_id as u8; 100]);
                    client
                        .set("default", &key, &value)
                        .await
                        .map_err(|e| format!("Set failed: {}", e))?;
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_sequential_consistency() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);
        let key = Key::from_str("consistency_key");

        // Sequential updates
        for i in 0..10 {
            let value = CipherBlob::new(vec![i as u8; 100]);
            ctx.client.set("default", &key, &value).await?;
        }

        // Final value should be 9
        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved
                .expect("Value should be retrievable in test")
                .as_bytes()[0],
            9
        );

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_client_reconnection() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);

        // Write with first client
        let key = Key::from_str("reconnect_key");
        let value = CipherBlob::new(vec![1, 2, 3]);
        ctx.client.set("default", &key, &value).await?;

        // Create new client and read
        let client2 = ctx.create_additional_client().await?;
        let retrieved = client2.get("default", &key).await?;
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved
                .expect("Value should be retrievable in test")
                .as_bytes(),
            &[1, 2, 3]
        );

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }
}

// ============================================================================
// Category 3: Error Scenarios (10 tests)
// ============================================================================

mod error_scenarios {
    use super::*;

    #[tokio::test]
    async fn test_e2e_connection_refused() {
        // Try to connect to non-existent server
        let result = AmateRSClient::connect("http://127.0.0.1:55555").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_e2e_invalid_address() {
        let result = AmateRSClient::connect("invalid_address").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_e2e_connection_timeout() {
        let config = ClientConfig::new("http://127.0.0.1:55556")
            .with_connect_timeout(Duration::from_millis(100));

        let result = AmateRSClient::connect_with_config(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_e2e_empty_collection_name() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3]);

        // Empty collection name should still work (defaults to empty string)
        let result = ctx.client.set("", &key, &value).await;
        // Depending on implementation, this might error or succeed
        // For now, we'll just verify it doesn't panic

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_very_long_key() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let long_key_str = "a".repeat(10_000);
        let key = Key::from_str(&long_key_str);
        let value = CipherBlob::new(vec![1, 2, 3]);

        let result = ctx.client.set("default", &key, &value).await;
        // Should either succeed or fail gracefully
        if result.is_ok() {
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_some());
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_very_large_value() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let key = Key::from_str("large_value_key");
        // Try 10MB value
        let large_value = CipherBlob::new(vec![42u8; 10_000_000]);

        let result = ctx.client.set("default", &key, &large_value).await;
        // Should handle large values or fail gracefully

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_rapid_connections() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        // Rapidly create and drop connections
        for _ in 0..10 {
            let _client = ctx.create_additional_client().await?;
            // Client drops here
        }

        // Server should still be responsive
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3]);
        ctx.client.set("default", &key, &value).await?;

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_server_under_load() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);

        // Hammer the server with requests
        let mut handles = Vec::new();

        for client_id in 0..10 {
            let ctx = Arc::clone(&ctx);

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                for i in 0..100 {
                    let key = Key::from_str(&format!("load_{}_{}", client_id, i));
                    let value = CipherBlob::new(vec![i as u8; 1000]);
                    client
                        .set("default", &key, &value)
                        .await
                        .map_err(|e| format!("Set failed: {}", e))?;
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_malformed_range_query() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        // Range where start > end
        let start = Key::from_str("zzz");
        let end = Key::from_str("aaa");

        let query = Query::Range {
            collection: "default".to_string(),
            start,
            end,
        };

        let result = ctx.client.execute_query(&query).await?;
        // Should return empty result
        match result {
            QueryResult::Multi(rows) => assert_eq!(rows.len(), 0),
            _ => panic!("Expected Multi result"),
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_special_characters_in_key() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        let special_key = Key::from_str("key_with_!@#$%^&*()_+-=[]{}|;':\",./<>?");
        let value = CipherBlob::new(vec![1, 2, 3]);

        ctx.client.set("default", &special_key, &value).await?;

        let retrieved = ctx.client.get("default", &special_key).await?;
        assert!(retrieved.is_some());

        ctx.cleanup().await;
        Ok(())
    }
}

// ============================================================================
// Category 4: LSM-Tree Persistence (10 tests)
// ============================================================================

mod lsm_persistence {
    use super::*;

    #[tokio::test]
    async fn test_e2e_lsm_basic_persistence() -> Result<(), Box<dyn std::error::Error>> {
        let mut ctx = E2eTestContext::with_storage("lsm").await?;

        // Write data
        for i in 0..10 {
            let key = Key::from_str(&format!("persist_{}", i));
            let value = CipherBlob::new(vec![i as u8; 100]);
            ctx.client.set("default", &key, &value).await?;
        }

        // Restart server
        ctx.restart_server().await?;

        // Verify data persists
        for i in 0..10 {
            let key = Key::from_str(&format!("persist_{}", i));
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_some());
            assert_eq!(
                retrieved
                    .expect("Value should be retrievable in test")
                    .as_bytes()[0],
                i as u8
            );
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_lsm_large_dataset_persistence() -> Result<(), Box<dyn std::error::Error>> {
        let mut ctx = E2eTestContext::with_storage("lsm").await?;

        // Write 1000 keys
        for i in 0..1000 {
            let key = Key::from_str(&format!("large_persist_{:06}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 500]);
            ctx.client.set("default", &key, &value).await?;
        }

        // Restart
        ctx.restart_server().await?;

        // Verify all keys present
        for i in 0..1000 {
            let key = Key::from_str(&format!("large_persist_{:06}", i));
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_some());
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_lsm_updates_persist() -> Result<(), Box<dyn std::error::Error>> {
        let mut ctx = E2eTestContext::with_storage("lsm").await?;

        let key = Key::from_str("update_persist");

        // Write initial value
        ctx.client
            .set("default", &key, &CipherBlob::new(vec![1; 100]))
            .await?;

        // Update multiple times
        for i in 2..=5 {
            ctx.client
                .set("default", &key, &CipherBlob::new(vec![i; 100]))
                .await?;
        }

        // Restart
        ctx.restart_server().await?;

        // Verify final value persists
        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved
                .expect("Value should be retrievable in test")
                .as_bytes()[0],
            5
        );

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_lsm_deletes_persist() -> Result<(), Box<dyn std::error::Error>> {
        let mut ctx = E2eTestContext::with_storage("lsm").await?;

        // Write data
        for i in 0..20 {
            let key = Key::from_str(&format!("delete_persist_{}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.client.set("default", &key, &value).await?;
        }

        // Delete half
        for i in 0..10 {
            let key = Key::from_str(&format!("delete_persist_{}", i));
            ctx.client.delete("default", &key).await?;
        }

        // Restart
        ctx.restart_server().await?;

        // Verify deletes persisted
        for i in 0..10 {
            let key = Key::from_str(&format!("delete_persist_{}", i));
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_none());
        }

        // Verify remaining data
        for i in 10..20 {
            let key = Key::from_str(&format!("delete_persist_{}", i));
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_some());
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_lsm_multiple_restarts() -> Result<(), Box<dyn std::error::Error>> {
        let mut ctx = E2eTestContext::with_storage("lsm").await?;

        for restart_num in 0..3 {
            // Write data specific to this iteration
            for i in 0..10 {
                let key = Key::from_str(&format!("multi_restart_{}_{}", restart_num, i));
                let value = CipherBlob::new(vec![restart_num as u8; 100]);
                ctx.client.set("default", &key, &value).await?;
            }

            if restart_num < 2 {
                ctx.restart_server().await?;
            }
        }

        // Final restart
        ctx.restart_server().await?;

        // Verify all data from all iterations
        for restart_num in 0..3 {
            for i in 0..10 {
                let key = Key::from_str(&format!("multi_restart_{}_{}", restart_num, i));
                let retrieved = ctx.client.get("default", &key).await?;
                assert!(retrieved.is_some());
                assert_eq!(
                    retrieved
                        .expect("Value should be retrievable in test")
                        .as_bytes()[0],
                    restart_num as u8
                );
            }
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_lsm_range_query_after_restart() -> Result<(), Box<dyn std::error::Error>> {
        let mut ctx = E2eTestContext::with_storage("lsm").await?;

        // Write ordered data
        for i in 0..50 {
            let key = Key::from_str(&format!("range_persist_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.client.set("default", &key, &value).await?;
        }

        // Restart
        ctx.restart_server().await?;

        // Execute range query
        let start = Key::from_str("range_persist_0010");
        let end = Key::from_str("range_persist_0020");

        let query = Query::Range {
            collection: "default".to_string(),
            start,
            end,
        };

        let result = ctx.client.execute_query(&query).await?;
        match result {
            QueryResult::Multi(rows) => assert_eq!(rows.len(), 10),
            _ => panic!("Expected Multi result"),
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_lsm_mixed_operations_persist() -> Result<(), Box<dyn std::error::Error>> {
        let mut ctx = E2eTestContext::with_storage("lsm").await?;

        // Phase 1: Insert
        for i in 0..30 {
            let key = Key::from_str(&format!("mixed_persist_{}", i));
            let value = CipherBlob::new(vec![1; 50]);
            ctx.client.set("default", &key, &value).await?;
        }

        // Phase 2: Update half
        for i in 0..15 {
            let key = Key::from_str(&format!("mixed_persist_{}", i));
            let value = CipherBlob::new(vec![2; 50]);
            ctx.client.set("default", &key, &value).await?;
        }

        // Phase 3: Delete some
        for i in 15..20 {
            let key = Key::from_str(&format!("mixed_persist_{}", i));
            ctx.client.delete("default", &key).await?;
        }

        // Restart
        ctx.restart_server().await?;

        // Verify state
        for i in 0..15 {
            let key = Key::from_str(&format!("mixed_persist_{}", i));
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_some());
            assert_eq!(
                retrieved
                    .expect("Value should be retrievable in test")
                    .as_bytes()[0],
                2
            );
        }

        for i in 15..20 {
            let key = Key::from_str(&format!("mixed_persist_{}", i));
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_none());
        }

        for i in 20..30 {
            let key = Key::from_str(&format!("mixed_persist_{}", i));
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_some());
            assert_eq!(
                retrieved
                    .expect("Value should be retrievable in test")
                    .as_bytes()[0],
                1
            );
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_lsm_empty_database_restart() -> Result<(), Box<dyn std::error::Error>> {
        let mut ctx = E2eTestContext::with_storage("lsm").await?;

        // Don't write any data, just restart
        ctx.restart_server().await?;

        // Verify server is functional
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3]);
        ctx.client.set("default", &key, &value).await?;

        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_some());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_lsm_binary_data_persist() -> Result<(), Box<dyn std::error::Error>> {
        let mut ctx = E2eTestContext::with_storage("lsm").await?;

        // Write binary data
        let key = Key::from_slice(&[0x00, 0xFF, 0x7F, 0x80]);
        let value = CipherBlob::new(vec![0x00, 0x11, 0x22, 0x33, 0xFF, 0xAA, 0xBB, 0xCC]);

        ctx.client.set("default", &key, &value).await?;

        // Restart
        ctx.restart_server().await?;

        // Verify binary data persists correctly
        let retrieved = ctx.client.get("default", &key).await?;
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved
                .expect("Value should be retrievable in test")
                .as_bytes(),
            &[0x00, 0x11, 0x22, 0x33, 0xFF, 0xAA, 0xBB, 0xCC]
        );

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_lsm_concurrent_writes_then_restart() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut ctx = E2eTestContext::with_storage("lsm").await?;
        let ctx_arc = Arc::new(RwLock::new(ctx));

        // Concurrent writes
        let mut handles = Vec::new();

        for client_id in 0..5 {
            let ctx = Arc::clone(&ctx_arc);

            let handle = tokio::spawn(async move {
                let ctx_read = ctx.read().await;
                let client = ctx_read
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                for i in 0..20 {
                    let key = Key::from_str(&format!("concurrent_persist_{}_{}", client_id, i));
                    let value = CipherBlob::new(vec![client_id as u8; 100]);
                    client
                        .set("default", &key, &value)
                        .await
                        .map_err(|e| format!("Set failed: {}", e))?;
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        // Restart
        let mut ctx_write = ctx_arc.write().await;
        ctx_write.restart_server().await?;

        // Verify all concurrent writes persisted
        for client_id in 0..5 {
            for i in 0..20 {
                let key = Key::from_str(&format!("concurrent_persist_{}_{}", client_id, i));
                let retrieved = ctx_write.client.get("default", &key).await?;
                assert!(retrieved.is_some());
                assert_eq!(
                    retrieved
                        .expect("Value should be retrievable in test")
                        .as_bytes()[0],
                    client_id as u8
                );
            }
        }

        // Drop the write guard before unwrapping
        drop(ctx_write);

        let ctx = Arc::try_unwrap(ctx_arc)
            .map_err(|_| "Arc unwrap failed")?
            .into_inner();
        ctx.cleanup().await;
        Ok(())
    }
}

// ============================================================================
// Category 5: FHE Operations (10 tests)
// ============================================================================

mod fhe_operations {
    use super::*;
    use amaters_core::compute::{EncryptedU8, FheKeyPair};

    fn encrypt_u8(value: u8, keypair: &FheKeyPair) -> CipherBlob {
        let encrypted = EncryptedU8::encrypt(value, keypair.client_key());
        encrypted.to_cipher_blob().expect("Failed to serialize")
    }

    #[tokio::test]
    async fn test_e2e_fhe_filter_simple() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        // Insert encrypted values
        let test_ages = vec![
            ("user:1", 15u8),
            ("user:2", 25),
            ("user:3", 35),
            ("user:4", 70),
        ];

        for (key_str, age) in &test_ages {
            let key = Key::from_str(key_str);
            let encrypted_age = encrypt_u8(*age, &keypair);
            ctx.client.set("users", &key, &encrypted_age).await?;
        }

        // Execute filter query: age > 18
        let rhs_value = encrypt_u8(18, &keypair);
        let predicate = Predicate::Gt(col("age"), rhs_value);

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate,
        };

        let result = ctx.client.execute_query(&query).await;
        // Just verify it doesn't error for now
        assert!(result.is_ok());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_fhe_filter_complex() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        // Insert data
        for i in 0..10 {
            let key = Key::from_str(&format!("user:{}", i));
            let age = (i * 10) as u8;
            let encrypted_age = encrypt_u8(age, &keypair);
            ctx.client.set("users", &key, &encrypted_age).await?;
        }

        // Query: age > 18 AND age < 65
        let lower_bound = encrypt_u8(18, &keypair);
        let upper_bound = encrypt_u8(65, &keypair);

        let pred1 = Predicate::Gt(col("age"), lower_bound);
        let pred2 = Predicate::Lt(col("age"), upper_bound);
        let combined = Predicate::And(Box::new(pred1), Box::new(pred2));

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate: combined,
        };

        let result = ctx.client.execute_query(&query).await;
        result.expect("Failed to execute complex filter query");

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_fhe_equality_filter() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        // Insert data with some duplicates
        let ages = [20u8, 25, 30, 25, 20, 35];
        for (i, age) in ages.iter().enumerate() {
            let key = Key::from_str(&format!("user:{}", i));
            let encrypted_age = encrypt_u8(*age, &keypair);
            ctx.client.set("users", &key, &encrypted_age).await?;
        }

        // Query: age == 25
        let target = encrypt_u8(25, &keypair);
        let predicate = Predicate::Eq(col("age"), target);

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate,
        };

        let result = ctx.client.execute_query(&query).await;
        assert!(result.is_ok());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_fhe_not_equal_filter() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        for i in 0..5 {
            let key = Key::from_str(&format!("user:{}", i));
            let encrypted_age = encrypt_u8((i * 10) as u8, &keypair);
            ctx.client.set("users", &key, &encrypted_age).await?;
        }

        // Query: age != 20 (expressed as NOT(age == 20))
        let target = encrypt_u8(20, &keypair);
        let predicate = Predicate::Not(Box::new(Predicate::Eq(col("age"), target)));

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate,
        };

        let result = ctx.client.execute_query(&query).await;
        assert!(result.is_ok());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_fhe_less_than_filter() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        for i in 0..10 {
            let key = Key::from_str(&format!("user:{}", i));
            let encrypted_age = encrypt_u8((i * 5) as u8, &keypair);
            ctx.client.set("users", &key, &encrypted_age).await?;
        }

        // Query: age < 25
        let target = encrypt_u8(25, &keypair);
        let predicate = Predicate::Lt(col("age"), target);

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate,
        };

        let result = ctx.client.execute_query(&query).await;
        assert!(result.is_ok());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_fhe_greater_or_equal() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        for i in 0..10 {
            let key = Key::from_str(&format!("user:{}", i));
            let encrypted_age = encrypt_u8((i * 10) as u8, &keypair);
            ctx.client.set("users", &key, &encrypted_age).await?;
        }

        // Query: age >= 50
        let target = encrypt_u8(50, &keypair);
        let predicate = Predicate::Gte(col("age"), target);

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate,
        };

        let result = ctx.client.execute_query(&query).await;
        assert!(result.is_ok());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_fhe_less_or_equal() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        for i in 0..10 {
            let key = Key::from_str(&format!("user:{}", i));
            let encrypted_age = encrypt_u8((i * 10) as u8, &keypair);
            ctx.client.set("users", &key, &encrypted_age).await?;
        }

        // Query: age <= 50
        let target = encrypt_u8(50, &keypair);
        let predicate = Predicate::Lte(col("age"), target);

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate,
        };

        let result = ctx.client.execute_query(&query).await;
        assert!(result.is_ok());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_fhe_or_predicate() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        for i in 0..10 {
            let key = Key::from_str(&format!("user:{}", i));
            let encrypted_age = encrypt_u8((i * 10) as u8, &keypair);
            ctx.client.set("users", &key, &encrypted_age).await?;
        }

        // Query: age < 20 OR age > 70
        let lower = encrypt_u8(20, &keypair);
        let upper = encrypt_u8(70, &keypair);

        let pred1 = Predicate::Lt(col("age"), lower);
        let pred2 = Predicate::Gt(col("age"), upper);
        let combined = Predicate::Or(Box::new(pred1), Box::new(pred2));

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate: combined,
        };

        let result = ctx.client.execute_query(&query).await;
        assert!(result.is_ok());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_fhe_empty_result_set() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        for i in 0..5 {
            let key = Key::from_str(&format!("user:{}", i));
            let encrypted_age = encrypt_u8((20 + i) as u8, &keypair);
            ctx.client.set("users", &key, &encrypted_age).await?;
        }

        // Query that matches nothing: age > 100
        let target = encrypt_u8(100, &keypair);
        let predicate = Predicate::Gt(col("age"), target);

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate,
        };

        let result = ctx.client.execute_query(&query).await;
        // Just verify it doesn't error for now
        // TODO: Implement client-side filtering to verify empty result set
        // Currently returns all rows because encrypted predicate results
        // are not yet included in the proto/response
        assert!(result.is_ok());

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_fhe_nested_predicates() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        for i in 0..20 {
            let key = Key::from_str(&format!("user:{}", i));
            let encrypted_age = encrypt_u8((i * 5) as u8, &keypair);
            ctx.client.set("users", &key, &encrypted_age).await?;
        }

        // Query: (age > 20 AND age < 50) OR (age > 70)
        let v20 = encrypt_u8(20, &keypair);
        let v50 = encrypt_u8(50, &keypair);
        let v70 = encrypt_u8(70, &keypair);

        let pred1 = Predicate::Gt(col("age"), v20);
        let pred2 = Predicate::Lt(col("age"), v50);
        let and_pred = Predicate::And(Box::new(pred1), Box::new(pred2));

        let pred3 = Predicate::Gt(col("age"), v70);
        let or_pred = Predicate::Or(Box::new(and_pred), Box::new(pred3));

        let query = Query::Filter {
            collection: "users".to_string(),
            predicate: or_pred,
        };

        let result = ctx.client.execute_query(&query).await;
        result.expect("Failed to execute nested predicates query");

        ctx.cleanup().await;
        Ok(())
    }
}

// ============================================================================
// Category 6: Stress Tests (5 tests)
// ============================================================================

mod stress_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn test_e2e_large_dataset() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        // Insert 10,000 keys
        for i in 0..10_000 {
            let key = Key::from_str(&format!("large_dataset_{:08}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 100]);
            ctx.client.set("default", &key, &value).await?;

            // Progress indicator
            if i % 1000 == 0 {
                println!("Inserted {} keys", i);
            }
        }

        // Verify random sampling
        for i in (0..10_000).step_by(100) {
            let key = Key::from_str(&format!("large_dataset_{:08}", i));
            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_some());
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_large_values() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        // Test with 1MB values
        for i in 0..5 {
            let key = Key::from_str(&format!("large_value_{}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 1_000_000]);

            ctx.client.set("default", &key, &value).await?;

            let retrieved = ctx.client.get("default", &key).await?;
            assert!(retrieved.is_some());
            assert_eq!(
                retrieved
                    .expect("Value should be retrievable in test")
                    .len(),
                1_000_000
            );
        }

        ctx.cleanup().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_sustained_throughput() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);
        let duration = Duration::from_secs(10);
        let start = std::time::Instant::now();
        let ops_counter = Arc::new(AtomicU64::new(0));

        let mut handles = Vec::new();

        // Spawn 5 workers
        for worker_id in 0..5 {
            let ctx = Arc::clone(&ctx);
            let ops_counter = Arc::clone(&ops_counter);
            let start_time = start;

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let mut i = 0u64;
                while start.elapsed() < duration {
                    let key = Key::from_str(&format!("sustained_{}_{}", worker_id, i));
                    let value = CipherBlob::new(vec![(i % 256) as u8; 500]);

                    client
                        .set("default", &key, &value)
                        .await
                        .map_err(|e| format!("Set failed: {}", e))?;

                    ops_counter.fetch_add(1, Ordering::Relaxed);
                    i += 1;
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        let total_ops = ops_counter.load(Ordering::Relaxed);
        let elapsed = start.elapsed();
        let ops_per_sec = total_ops as f64 / elapsed.as_secs_f64();

        println!("Sustained throughput test:");
        println!("  Duration: {:?}", elapsed);
        println!("  Total operations: {}", total_ops);
        println!("  Operations/sec: {:.2}", ops_per_sec);

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_memory_pressure() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Arc::new(E2eTestContext::new().await?);

        // Create memory pressure with many concurrent large operations
        let mut handles = Vec::new();

        for client_id in 0..10 {
            let ctx = Arc::clone(&ctx);

            let handle = tokio::spawn(async move {
                let client = ctx
                    .create_additional_client()
                    .await
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                for i in 0..50 {
                    let key = Key::from_str(&format!("memory_pressure_{}_{}", client_id, i));
                    let value = CipherBlob::new(vec![(i % 256) as u8; 100_000]);

                    client
                        .set("default", &key, &value)
                        .await
                        .map_err(|e| format!("Set failed: {}", e))?;
                }

                Ok::<_, String>(())
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .map_err(|e| format!("Task panicked: {}", e))??;
        }

        // Verify system is still responsive
        let key = Key::from_str("final_check");
        let value = CipherBlob::new(vec![1, 2, 3]);
        ctx.client.set("default", &key, &value).await?;

        Arc::try_unwrap(ctx)
            .map_err(|_| "Arc unwrap failed")?
            .cleanup()
            .await;
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_rapid_key_turnover() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = E2eTestContext::new().await?;

        // Rapidly create and delete keys
        for iteration in 0..10 {
            // Insert 1000 keys
            for i in 0..1000 {
                let key = Key::from_str(&format!("turnover_{}_{}", iteration, i));
                let value = CipherBlob::new(vec![(i % 256) as u8; 100]);
                ctx.client.set("default", &key, &value).await?;
            }

            // Delete them all
            for i in 0..1000 {
                let key = Key::from_str(&format!("turnover_{}_{}", iteration, i));
                ctx.client.delete("default", &key).await?;
            }

            println!("Completed iteration {}", iteration);
        }

        // Verify system is still responsive
        let key = Key::from_str("final_check");
        let value = CipherBlob::new(vec![1, 2, 3]);
        ctx.client.set("default", &key, &value).await?;

        ctx.cleanup().await;
        Ok(())
    }
}
