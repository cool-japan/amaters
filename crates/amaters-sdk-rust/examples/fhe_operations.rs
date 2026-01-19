//! FHE operations example for AmateRS SDK
//!
//! This example demonstrates FHE (Fully Homomorphic Encryption) capabilities:
//! - Client-side encryption and decryption
//! - Key management
//! - Encrypted queries and filters
//! - Server-side computation on encrypted data
//! - Encrypted filtering without server seeing plaintext
//!
//! ## Note on Implementation
//!
//! This example uses stub FHE implementation (data is not actually encrypted).
//! When the `fhe` feature is enabled, this will use actual TFHE encryption.
//! The workflow and API remain the same regardless.
//!
//! ## Prerequisites
//!
//! Before running this example, make sure the AmateRS server is running:
//! ```bash
//! cargo run --bin amaters-server
//! ```
//!
//! To run this example:
//! ```bash
//! cargo run --example fhe_operations
//! ```
//!
//! To run with real TFHE encryption (when implemented):
//! ```bash
//! cargo run --example fhe_operations --features fhe
//! ```

use amaters_core::{CipherBlob, Key, Predicate, col};
use amaters_sdk_rust::{AmateRSClient, FheEncryptor, query};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== AmateRS SDK FHE Operations Example ===\n");

    #[cfg(not(feature = "fhe"))]
    println!("Note: Using stub FHE implementation (not secure!)");
    #[cfg(not(feature = "fhe"))]
    println!("Enable the 'fhe' feature for real encryption.\n");

    // Example 1: Create FHE encryptor
    println!("Example 1: Creating FHE encryptor");
    println!("  Generating FHE keys...");
    let encryptor = FheEncryptor::new()?;
    println!("  ✓ Keys generated\n");

    // Example 2: Encrypt data
    println!("Example 2: Encrypting data");
    let plaintext = b"Sensitive medical record: Patient 123, Blood Type O+";
    println!("  Plaintext: {} bytes", plaintext.len());

    let ciphertext = encryptor.encrypt(plaintext)?;
    println!("  ✓ Encrypted: {} bytes", ciphertext.len());
    println!();

    // Example 3: Decrypt data
    println!("Example 3: Decrypting data");
    let decrypted = encryptor.decrypt(&ciphertext)?;
    println!("  ✓ Decrypted: {} bytes", decrypted.len());
    println!("  Data matches original: {}", decrypted == plaintext);
    println!();

    // Example 4: Connect with encryptor
    println!("Example 4: Using client with FHE encryptor");
    let client = AmateRSClient::connect("http://localhost:50051")
        .await?
        .with_encryptor(encryptor);

    println!("  ✓ Client configured with FHE encryptor");
    println!("  Encryptor available: {}", client.encryptor().is_some());
    println!();

    // Example 5: Store encrypted data
    println!("Example 5: Storing encrypted data");
    let records = vec![
        ("patient:001", "Medical record for patient 001"),
        ("patient:002", "Medical record for patient 002"),
        ("patient:003", "Medical record for patient 003"),
    ];

    // Get encryptor reference
    let encryptor = client
        .encryptor()
        .expect("encryptor should be set")
        .as_ref();

    for (id, record) in &records {
        let key = Key::from_str(id);
        let encrypted = encryptor.encrypt(record.as_bytes())?;

        client.set("medical_records", &key, &encrypted).await?;
        println!("  ✓ Stored encrypted record: {}", id);
    }
    println!();

    // Example 6: Retrieve and decrypt data
    println!("Example 6: Retrieving and decrypting data");
    let key = Key::from_str("patient:001");

    if let Some(encrypted) = client.get("medical_records", &key).await? {
        println!("  Retrieved encrypted record: {} bytes", encrypted.len());

        let decrypted = encryptor.decrypt(&encrypted)?;
        let record = String::from_utf8_lossy(&decrypted);

        println!("  ✓ Decrypted record: {}", record);
    } else {
        println!("  Record not found");
    }
    println!();

    // Example 7: Batch encryption
    println!("Example 7: Batch encryption");
    let data: [&[u8]; 5] = [
        b"Record 1",
        b"Record 2",
        b"Record 3",
        b"Record 4",
        b"Record 5",
    ];

    println!("  Encrypting {} records in batch...", data.len());
    let encrypted_batch = encryptor.encrypt_batch(&data)?;
    println!("  ✓ Batch encrypted: {} items", encrypted_batch.len());

    // Verify decryption
    for (i, cipher) in encrypted_batch.iter().enumerate() {
        let decrypted = encryptor.decrypt(cipher)?;
        assert_eq!(decrypted, data[i], "Decryption mismatch at index {}", i);
    }
    println!("  ✓ All items decrypt correctly");
    println!();

    // Example 8: Encrypted queries
    println!("Example 8: Building encrypted queries");

    // Encrypt query values
    let status_value = encryptor.encrypt(b"active")?;
    let age_value = encryptor.encrypt(b"18")?;

    println!("  Creating filter with encrypted predicates...");
    let q = query("users").filter(amaters_core::Predicate::And(
        Box::new(amaters_core::Predicate::Eq(
            amaters_core::col("status"),
            status_value,
        )),
        Box::new(amaters_core::Predicate::Gt(
            amaters_core::col("age"),
            age_value,
        )),
    ));

    println!("  ✓ Query built with encrypted values");
    println!("  Query: SELECT * FROM users WHERE status = <encrypted> AND age > <encrypted>");

    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed");
    println!();

    // Example 9: Key management (stub)
    println!("Example 9: Key management");
    let keys = encryptor.keys();

    // Save keys (stub implementation)
    let temp_dir = std::env::temp_dir();
    let key_file = temp_dir.join("amaters_test_keys.bin");
    println!("  Saving keys to {:?}...", key_file);

    match keys.save_to_file(&key_file) {
        Ok(()) => println!("  ✓ Keys saved"),
        Err(e) => println!("  Note: {}", e),
    }
    println!();

    // Example 10: FHE Filter Query - The Key Feature
    println!("Example 10: FHE Filter Query - Server-side filtering on encrypted data");
    println!("  This demonstrates the core FHE capability: filtering without seeing data\n");

    // Store encrypted age values for multiple users
    println!("  Storing encrypted age values...");
    let users_ages = vec![
        ("user:alice", 25u8),
        ("user:bob", 17u8),
        ("user:charlie", 35u8),
        ("user:dave", 70u8),
        ("user:eve", 22u8),
    ];

    for (user_id, age) in &users_ages {
        let key = Key::from_str(user_id);
        // In production, this would be encrypted with FHE
        let encrypted_age = encryptor.encrypt(&[*age])?;
        client.set("users_with_ages", &key, &encrypted_age).await?;
        println!("    Stored encrypted age for {}", user_id);
    }
    println!();

    // Build a filter query: age > 18 AND age < 65
    println!("  Building filter: age > 18 AND age < 65");
    let encrypted_18 = encryptor.encrypt(&[18u8])?;
    let encrypted_65 = encryptor.encrypt(&[65u8])?;

    let predicate = Predicate::And(
        Box::new(Predicate::Gt(col("age"), encrypted_18)),
        Box::new(Predicate::Lt(col("age"), encrypted_65)),
    );

    let filter_query = query("users_with_ages").filter(predicate);

    println!("  Executing filter on server (server never sees ages)...");
    let result = client.execute_query(&filter_query).await?;

    match result {
        amaters_sdk_rust::QueryResult::Multi(values) => {
            println!("  ✓ Filter executed: {} results", values.len());
            println!("  Results (after client-side decryption):");
            for (key, encrypted_value) in values {
                let age_bytes = encryptor.decrypt(&encrypted_value)?;
                if !age_bytes.is_empty() {
                    let age = age_bytes[0];
                    println!("    {} - age: {}", key, age);
                }
            }
            println!("\n  Expected: alice (25), charlie (35), eve (22)");
            println!("  Filtered out: bob (17 - too young), dave (70 - too old)");
        }
        _ => println!("  Unexpected result type"),
    }
    println!();

    // Example 11: Encryption properties and security model
    println!("Example 11: FHE Properties and Security Model");
    println!("  FHE allows computation on encrypted data:");
    println!("    - Server can compare encrypted values (>, <, =)");
    println!("    - Server can perform arithmetic on ciphertext (+, *, -)");
    println!("    - Server NEVER sees plaintext");
    println!("    - All results remain encrypted");
    println!();

    println!("  Security model:");
    println!("    - Client holds private keys");
    println!("    - Server stores only ciphertext");
    println!("    - Zero-knowledge proofs possible");
    println!("    - Post-quantum secure (TFHE)");
    println!();

    println!("  Use cases:");
    println!("    - Healthcare: Query patient records without exposing data");
    println!("    - Finance: Analyze credit scores while encrypted");
    println!("    - Supply chain: Track sensitive logistics data");
    println!("    - Genomics: Search DNA databases privately");
    println!();

    // Clean up
    client.close();
    println!("Done!");

    Ok(())
}
