//! Persistence example for AmateRS SDK
//!
//! This example demonstrates LSM-Tree persistence:
//! - Writing data to persistent storage
//! - Data survives server restarts
//! - Reading persisted data back
//! - Verifying data integrity
//!
//! ## How it works
//!
//! AmateRS uses an LSM-Tree (Log-Structured Merge-Tree) for persistence:
//! 1. Writes go to an in-memory MemTable
//! 2. When full, MemTable is flushed to disk as an SSTable
//! 3. Background compaction merges SSTables
//! 4. Data persists across restarts
//!
//! ## Prerequisites
//!
//! Before running this example, make sure the AmateRS server is running
//! with persistence enabled:
//! ```bash
//! cargo run --bin amaters-server -- --data-dir /tmp/amaters-data
//! ```
//!
//! Then run this example:
//! ```bash
//! cargo run --example persistence
//! ```
//!
//! ## Note
//!
//! This example demonstrates the persistence concept. In production:
//! - Configure durable storage paths
//! - Set appropriate flush intervals
//! - Monitor compaction performance
//! - Implement backup strategies

use amaters_core::{CipherBlob, Key};
use amaters_sdk_rust::AmateRSClient;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== AmateRS SDK Persistence Example ===\n");

    // Phase 1: Write data
    println!("Phase 1: Writing persistent data");
    println!("Connecting to server...");
    let client = AmateRSClient::connect("http://localhost:50051").await?;
    println!("Connected!\n");

    let collection = "persistent_store";

    // Write test data
    println!("  Writing test data...");
    let test_data: Vec<(&str, &[u8])> = vec![
        ("config:app_version", b"1.0.0".as_slice()),
        (
            "config:feature_flags",
            b"feature1,feature2,feature3".as_slice(),
        ),
        (
            "user:alice:profile",
            b"Alice Anderson, alice@example.com".as_slice(),
        ),
        (
            "user:bob:profile",
            b"Bob Builder, bob@example.com".as_slice(),
        ),
        ("metrics:request_count", b"12345".as_slice()),
        ("metrics:error_rate", b"0.001".as_slice()),
    ];

    for (key_str, data) in &test_data {
        let key = Key::from_str(key_str);
        let value = CipherBlob::new(data.to_vec());
        client.set(collection, &key, &value).await?;
        println!("    ✓ Wrote key: {}", key_str);
    }
    println!("  {} keys written", test_data.len());
    println!();

    // Verify immediate reads
    println!("  Verifying immediate reads...");
    for (key_str, expected_data) in &test_data {
        let key = Key::from_str(key_str);
        match client.get(collection, &key).await? {
            Some(value) => {
                if value.as_bytes() == *expected_data {
                    println!("    ✓ Verified: {}", key_str);
                } else {
                    println!("    ✗ Mismatch: {}", key_str);
                }
            }
            None => {
                println!("    ✗ Not found: {}", key_str);
            }
        }
    }
    println!();

    // Force flush to disk (if supported)
    println!("  Waiting for data to flush to disk...");
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("  ✓ Data should now be persisted\n");

    client.close();

    // Phase 2: Simulate restart
    println!("Phase 2: Simulating server restart");
    println!("  In production, you would:");
    println!("    1. Stop the server (Ctrl+C)");
    println!("    2. Restart the server");
    println!("    3. Reconnect the client");
    println!();
    println!("  For this example, we'll reconnect after a delay...");
    tokio::time::sleep(Duration::from_secs(1)).await;
    println!();

    // Phase 3: Read back data
    println!("Phase 3: Reading persisted data after 'restart'");
    println!("Reconnecting to server...");
    let client = AmateRSClient::connect("http://localhost:50051").await?;
    println!("Connected!\n");

    println!("  Reading back data...");
    let mut success_count = 0;
    let mut failure_count = 0;

    for (key_str, expected_data) in &test_data {
        let key = Key::from_str(key_str);
        match client.get(collection, &key).await? {
            Some(value) => {
                if value.as_bytes() == *expected_data {
                    println!(
                        "    ✓ Persisted: {} = {:?}",
                        key_str,
                        String::from_utf8_lossy(expected_data)
                    );
                    success_count += 1;
                } else {
                    println!("    ✗ Data mismatch: {}", key_str);
                    failure_count += 1;
                }
            }
            None => {
                println!("    ✗ Lost: {} (not found after restart)", key_str);
                failure_count += 1;
            }
        }
    }
    println!();

    // Summary
    println!("Persistence Summary:");
    println!("  Total keys written: {}", test_data.len());
    println!("  Successfully persisted: {}", success_count);
    println!("  Failed/Lost: {}", failure_count);

    if failure_count == 0 {
        println!("\n  ✓ All data persisted successfully!");
        println!("  LSM-Tree is working correctly.");
    } else {
        println!("\n  ✗ Some data was lost.");
        println!("  This might indicate:");
        println!("    - Server is using in-memory mode");
        println!("    - Data directory not configured");
        println!("    - Write-ahead log disabled");
    }
    println!();

    // Phase 4: Range query on persisted data
    println!("Phase 4: Range query on persisted data");
    println!("  Querying range: user:a to user:z");

    let start_key = Key::from_str("user:a");
    let end_key = Key::from_str("user:z");

    let results = client.range(collection, &start_key, &end_key).await?;
    println!("  ✓ Found {} user entries:", results.len());

    for (key, value) in results {
        println!("    {}: {}", key, String::from_utf8_lossy(value.as_bytes()));
    }
    println!();

    // Clean up
    println!("Closing client...");
    client.close();
    println!("Done!");
    println!();
    println!("Note: To verify true persistence, stop and restart the server,");
    println!("      then run this example again to see if data is still there.");

    Ok(())
}
