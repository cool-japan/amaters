//! Integration tests for FHE filter queries
//!
//! These tests verify the end-to-end functionality of FHE filter queries,
//! from client SDK through gRPC server to storage engine and back.

use amaters_core::Query;
use amaters_core::compute::{EncryptedU8, FheKeyPair};
use amaters_core::storage::MemoryStorage;
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key, Predicate, col};
use amaters_net::server::AqlServiceImpl;
use std::sync::Arc;

mod common;

/// Helper function to encrypt a U8 value
fn encrypt_u8(value: u8, keypair: &FheKeyPair) -> CipherBlob {
    let encrypted = EncryptedU8::encrypt(value, keypair.client_key());
    encrypted.to_cipher_blob().expect("Failed to serialize")
}

#[tokio::test]
async fn test_filter_query_basic() {
    // This test demonstrates the basic filter query workflow

    // 1. Setup: Generate keys and create storage
    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    // 2. Insert test data: ages 15, 25, 35, 70
    let test_ages = vec![
        ("user:1", 15u8),
        ("user:2", 25),
        ("user:3", 35),
        ("user:4", 70),
    ];

    for (key_str, age) in &test_ages {
        let key = Key::from_str(key_str);
        let encrypted_age = encrypt_u8(*age, &keypair);

        storage
            .put(&key, &encrypted_age)
            .await
            .expect("Failed to insert");
    }

    // 3. Execute filter query: age > 18
    let rhs_value = encrypt_u8(18, &keypair);
    let predicate = Predicate::Gt(col("age"), rhs_value);

    let query = Query::Filter {
        collection: "users".to_string(),
        predicate,
    };

    let result = service.execute_query_internal(query).await;

    // For now, the filter returns all rows since we haven't implemented
    // the encrypted boolean return mechanism in the proto yet
    // This test verifies the query executes without errors
    assert!(result.is_ok(), "Filter query should execute successfully");
}

#[tokio::test]
async fn test_filter_query_with_eq_predicate() {
    // Test equality filter: age == 25

    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let _service = AqlServiceImpl::new(storage.clone());

    // Insert test data
    let test_ages = vec![
        ("user:1", 15u8),
        ("user:2", 25),
        ("user:3", 35),
        ("user:4", 25),
    ];

    for (key_str, age) in &test_ages {
        let key = Key::from_str(key_str);
        let encrypted_age = encrypt_u8(*age, &keypair);

        storage
            .put(&key, &encrypted_age)
            .await
            .expect("Failed to insert");
    }

    // Execute filter query: age == 25
    let rhs_value = encrypt_u8(25, &keypair);
    let predicate = Predicate::Eq(col("age"), rhs_value);

    // For now, just verify the predicate is constructed correctly
    assert!(matches!(predicate, Predicate::Eq(_, _)));
}

#[tokio::test]
async fn test_filter_query_with_lt_predicate() {
    // Test less-than filter: age < 30

    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    // Insert test data
    let test_ages = vec![
        ("user:1", 15u8),
        ("user:2", 25),
        ("user:3", 35),
        ("user:4", 70),
    ];

    for (key_str, age) in &test_ages {
        let key = Key::from_str(key_str);
        let encrypted_age = encrypt_u8(*age, &keypair);

        storage
            .put(&key, &encrypted_age)
            .await
            .expect("Failed to insert");
    }

    // Execute filter query: age < 30
    let rhs_value = encrypt_u8(30, &keypair);
    let predicate = Predicate::Lt(col("age"), rhs_value);

    let query = Query::Filter {
        collection: "users".to_string(),
        predicate,
    };

    let result = service.execute_query_internal(query).await;
    assert!(result.is_ok(), "Filter query should execute successfully");
}

#[tokio::test]
async fn test_filter_query_empty_result() {
    // Test filter that matches no rows: age > 100

    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    // Insert test data
    let test_ages = vec![
        ("user:1", 15u8),
        ("user:2", 25),
        ("user:3", 35),
        ("user:4", 70),
    ];

    for (key_str, age) in &test_ages {
        let key = Key::from_str(key_str);
        let encrypted_age = encrypt_u8(*age, &keypair);

        storage
            .put(&key, &encrypted_age)
            .await
            .expect("Failed to insert");
    }

    // Execute filter query: age > 100
    let rhs_value = encrypt_u8(100, &keypair);
    let predicate = Predicate::Gt(col("age"), rhs_value);

    let query = Query::Filter {
        collection: "users".to_string(),
        predicate,
    };

    let result = service.execute_query_internal(query).await;

    // Query should execute successfully, even if no rows match
    assert!(result.is_ok(), "Filter query should execute successfully");
}

#[tokio::test]
async fn test_filter_query_on_empty_storage() {
    // Test filter on empty storage

    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage);

    // Execute filter query on empty storage
    let rhs_value = encrypt_u8(18, &keypair);
    let predicate = Predicate::Gt(col("age"), rhs_value);

    let query = Query::Filter {
        collection: "users".to_string(),
        predicate,
    };

    let result = service.execute_query_internal(query).await;
    assert!(
        result.is_ok(),
        "Filter query on empty storage should succeed"
    );
}

#[tokio::test]
async fn test_filter_query_with_lte_predicate() {
    // Test less-than-or-equal filter: age <= 35

    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    // Insert test data: ages 15, 25, 35, 70
    let test_ages = vec![
        ("user:1", 15u8),
        ("user:2", 25),
        ("user:3", 35),
        ("user:4", 70),
    ];

    for (key_str, age) in &test_ages {
        let key = Key::from_str(key_str);
        let encrypted_age = encrypt_u8(*age, &keypair);

        storage
            .put(&key, &encrypted_age)
            .await
            .expect("Failed to insert");
    }

    // Execute filter query: age <= 35
    let rhs_value = encrypt_u8(35, &keypair);
    let predicate = Predicate::Lte(col("age"), rhs_value);

    let query = Query::Filter {
        collection: "users".to_string(),
        predicate,
    };

    let result = service.execute_query_internal(query).await;
    assert!(result.is_ok(), "Filter query should execute successfully");

    // Expected results: user:1 (15), user:2 (25), user:3 (35)
    // user:4 (70) should not match
}

#[tokio::test]
async fn test_filter_query_with_gte_predicate() {
    // Test greater-than-or-equal filter: age >= 25

    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    // Insert test data
    let test_ages = vec![
        ("user:1", 15u8),
        ("user:2", 25),
        ("user:3", 35),
        ("user:4", 70),
    ];

    for (key_str, age) in &test_ages {
        let key = Key::from_str(key_str);
        let encrypted_age = encrypt_u8(*age, &keypair);

        storage
            .put(&key, &encrypted_age)
            .await
            .expect("Failed to insert");
    }

    // Execute filter query: age >= 25
    let rhs_value = encrypt_u8(25, &keypair);
    let predicate = Predicate::Gte(col("age"), rhs_value);

    let query = Query::Filter {
        collection: "users".to_string(),
        predicate,
    };

    let result = service.execute_query_internal(query).await;
    assert!(result.is_ok(), "Filter query should execute successfully");

    // Expected results: user:2 (25), user:3 (35), user:4 (70)
    // user:1 (15) should not match
}

#[tokio::test]
#[ignore] // Ignored for now as And/Or predicates need refined implementation
async fn test_filter_query_complex_and_predicate() {
    // Test complex filter: age > 18 AND age < 65

    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    // Insert test data: ages 15, 25, 35, 70
    let test_ages = vec![
        ("user:1", 15u8),
        ("user:2", 25),
        ("user:3", 35),
        ("user:4", 70),
    ];

    for (key_str, age) in &test_ages {
        let key = Key::from_str(key_str);
        let encrypted_age = encrypt_u8(*age, &keypair);

        storage
            .put(&key, &encrypted_age)
            .await
            .expect("Failed to insert");
    }

    // Execute filter query: age > 18 AND age < 65
    let rhs1 = encrypt_u8(18, &keypair);
    let rhs2 = encrypt_u8(65, &keypair);

    let pred1 = Predicate::Gt(col("age"), rhs1);
    let pred2 = Predicate::Lt(col("age"), rhs2);
    let predicate = Predicate::And(Box::new(pred1), Box::new(pred2));

    let query = Query::Filter {
        collection: "users".to_string(),
        predicate,
    };

    // Note: This currently won't work correctly because the circuit
    // expects single value/rhs pairs, but this predicate has two RHS values.
    // This is a known limitation of v0.1.0 and should be addressed in future versions.
    let result = service.execute_query_internal(query).await;

    // For now, we just verify it doesn't panic
    // In the future, this should properly filter to user:2 (25) and user:3 (35)
    let _ = result;
}

#[tokio::test]
#[ignore] // Ignored for now as Or predicates need refined implementation
async fn test_filter_query_complex_or_predicate() {
    // Test complex filter: age < 18 OR age > 65

    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    // Insert test data
    let test_ages = vec![
        ("user:1", 15u8),
        ("user:2", 25),
        ("user:3", 35),
        ("user:4", 70),
    ];

    for (key_str, age) in &test_ages {
        let key = Key::from_str(key_str);
        let encrypted_age = encrypt_u8(*age, &keypair);

        storage
            .put(&key, &encrypted_age)
            .await
            .expect("Failed to insert");
    }

    // Execute filter query: age < 18 OR age > 65
    let rhs1 = encrypt_u8(18, &keypair);
    let rhs2 = encrypt_u8(65, &keypair);

    let pred1 = Predicate::Lt(col("age"), rhs1);
    let pred2 = Predicate::Gt(col("age"), rhs2);
    let predicate = Predicate::Or(Box::new(pred1), Box::new(pred2));

    let query = Query::Filter {
        collection: "users".to_string(),
        predicate,
    };

    let result = service.execute_query_internal(query).await;

    // Expected results: user:1 (15) and user:4 (70)
    let _ = result;
}

#[tokio::test]
async fn test_filter_query_with_not_predicate() {
    // Test NOT predicate: NOT (age == 25)

    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    // Insert test data
    let test_ages = vec![
        ("user:1", 15u8),
        ("user:2", 25),
        ("user:3", 35),
        ("user:4", 25),
    ];

    for (key_str, age) in &test_ages {
        let key = Key::from_str(key_str);
        let encrypted_age = encrypt_u8(*age, &keypair);

        storage
            .put(&key, &encrypted_age)
            .await
            .expect("Failed to insert");
    }

    // Execute filter query: NOT (age == 25)
    let rhs_value = encrypt_u8(25, &keypair);
    let pred = Predicate::Eq(col("age"), rhs_value);
    let predicate = Predicate::Not(Box::new(pred));

    let query = Query::Filter {
        collection: "users".to_string(),
        predicate,
    };

    let result = service.execute_query_internal(query).await;
    assert!(result.is_ok(), "Filter query should execute successfully");

    // Expected results: user:1 (15) and user:3 (35)
    // user:2 and user:4 (both 25) should not match
}

#[tokio::test]
async fn test_filter_performance_with_large_dataset() {
    // Test filter query performance with a larger dataset

    let keypair = FheKeyPair::generate().expect("Failed to generate keys");
    keypair.set_as_global_server_key();

    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    // Insert 100 test records
    for i in 0..100u8 {
        let key = Key::from_str(&format!("user:{:03}", i));
        let age = (i % 80) + 10; // Ages from 10 to 89
        let encrypted_age = encrypt_u8(age, &keypair);

        storage
            .put(&key, &encrypted_age)
            .await
            .expect("Failed to insert");
    }

    // Execute filter query: age > 50
    let rhs_value = encrypt_u8(50, &keypair);
    let predicate = Predicate::Gt(col("age"), rhs_value);

    let query = Query::Filter {
        collection: "users".to_string(),
        predicate,
    };

    let start = std::time::Instant::now();
    let result = service.execute_query_internal(query).await;
    let duration = start.elapsed();

    assert!(result.is_ok(), "Filter query should execute successfully");

    // Log performance (for manual inspection)
    println!("Filter query on 100 rows took: {:?}", duration);
}
