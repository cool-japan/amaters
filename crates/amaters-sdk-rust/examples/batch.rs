//! Batch operations example for AmateRS SDK
//!
//! This example demonstrates batch processing:
//! - Executing multiple queries in a single batch
//! - Atomic batch operations (all succeed or all fail)
//! - Performance benefits of batching
//! - Mixed operations (set, get, delete)
//! - Performance comparison: sequential vs batch
//!
//! ## Prerequisites
//!
//! Before running this example, make sure the AmateRS server is running:
//! ```bash
//! cargo run --bin amaters-server
//! ```
//!
//! Then run this example:
//! ```bash
//! cargo run --example batch
//! ```

use amaters_core::{CipherBlob, Key};
use amaters_sdk_rust::{AmateRSClient, query};
use std::time::Instant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== AmateRS SDK Batch Operations Example ===\n");

    // Connect to server
    println!("Connecting to server...");
    let client = AmateRSClient::connect("http://localhost:50051").await?;
    println!("Connected!\n");

    // Example 1: Simple batch insert
    println!("Example 1: Batch insert");
    let users = vec![
        ("alice", "Alice's data"),
        ("bob", "Bob's data"),
        ("charlie", "Charlie's data"),
        ("dave", "Dave's data"),
        ("eve", "Eve's data"),
    ];

    let mut queries = Vec::new();
    for (username, data) in &users {
        let key = Key::from_str(&format!("user:{}", username));
        let value = CipherBlob::new(data.as_bytes().to_vec());
        queries.push(query("users").set(key, value));
    }

    println!("  Inserting {} users in batch...", users.len());
    let start = Instant::now();
    let results = client.execute_batch(queries).await?;
    let elapsed = start.elapsed();

    println!("  ✓ Batch completed: {} operations", results.len());
    println!("  Time elapsed: {:?}", elapsed);
    println!();

    // Example 2: Mixed batch operations
    println!("Example 2: Mixed batch operations");
    let queries = vec![
        query("users").set(
            Key::from_str("user:frank"),
            CipherBlob::new(b"Frank's data".to_vec()),
        ),
        query("users").get(Key::from_str("user:alice")),
        query("users").delete(Key::from_str("user:bob")),
        query("users").set(
            Key::from_str("user:grace"),
            CipherBlob::new(b"Grace's data".to_vec()),
        ),
    ];

    println!("  Executing mixed batch (set, get, delete, set)...");
    let start = Instant::now();
    let results = client.execute_batch(queries).await?;
    let elapsed = start.elapsed();

    println!("  ✓ Batch completed: {} operations", results.len());
    println!("  Time elapsed: {:?}", elapsed);
    println!();

    // Example 3: Large batch
    println!("Example 3: Large batch insert");
    let batch_size = 100;
    let mut queries = Vec::new();

    for i in 0..batch_size {
        let key = Key::from_str(&format!("data:{:04}", i));
        let value = CipherBlob::new(format!("Data item {}", i).into_bytes());
        queries.push(query("data").set(key, value));
    }

    println!("  Inserting {} items in batch...", batch_size);
    let start = Instant::now();
    let results = client.execute_batch(queries).await?;
    let elapsed = start.elapsed();

    println!("  ✓ Batch completed: {} operations", results.len());
    println!("  Time elapsed: {:?}", elapsed);
    println!(
        "  Average time per operation: {:?}",
        elapsed / batch_size as u32
    );
    println!();

    // Example 4: Batch with pre-encrypted data
    println!("Example 4: Batch insert with encrypted data");
    let data_items: [(usize, &[u8]); 5] = [
        (0, b"sensitive data 1"),
        (1, b"sensitive data 2"),
        (2, b"sensitive data 3"),
        (3, b"sensitive data 4"),
        (4, b"sensitive data 5"),
    ];

    println!("  Preparing {} encrypted items...", data_items.len());

    // Build batch queries
    let mut queries = Vec::new();
    for (i, data) in &data_items {
        let key = Key::from_str(&format!("encrypted:{}", i));
        // In production, you would encrypt this data with your FHE keys
        let cipher = CipherBlob::new(data.to_vec());
        queries.push(query("encrypted").set(key, cipher));
    }

    println!("  Inserting {} encrypted items in batch...", queries.len());
    let start = Instant::now();
    let results = client.execute_batch(queries).await?;
    let batch_elapsed = start.elapsed();

    println!("  ✓ Batch completed: {} operations", results.len());
    println!("  Batch insert time: {:?}", batch_elapsed);
    println!();

    // Example 5: Range delete (using individual deletes in batch)
    println!("Example 5: Batch delete (range simulation)");
    let mut queries = Vec::new();
    for i in 0..10 {
        let key = Key::from_str(&format!("data:{:04}", i));
        queries.push(query("data").delete(key));
    }

    println!("  Deleting {} items in batch...", queries.len());
    let start = Instant::now();
    let results = client.execute_batch(queries).await?;
    let elapsed = start.elapsed();

    println!("  ✓ Batch completed: {} operations", results.len());
    println!("  Time elapsed: {:?}", elapsed);
    println!();

    // Example 6: Performance comparison (sequential vs batch)
    println!("Example 6: Performance comparison - Sequential vs Batch");
    let test_size = 10;

    // Sequential operations
    println!("  Sequential: Inserting {} items one by one...", test_size);
    let start = Instant::now();
    for i in 0..test_size {
        let key = Key::from_str(&format!("seq:{}", i));
        let value = CipherBlob::new(format!("Sequential {}", i).into_bytes());
        client.set("perf", &key, &value).await?;
    }
    let sequential_time = start.elapsed();
    println!("  ✓ Sequential time: {:?}", sequential_time);
    println!(
        "    Average per operation: {:?}",
        sequential_time / test_size
    );

    // Batch operations
    println!("  Batch: Inserting {} items in one batch...", test_size);
    let mut queries = Vec::new();
    for i in 0..test_size {
        let key = Key::from_str(&format!("batch:{}", i));
        let value = CipherBlob::new(format!("Batch {}", i).into_bytes());
        queries.push(query("perf").set(key, value));
    }

    let start = Instant::now();
    let _results = client.execute_batch(queries).await?;
    let batch_time = start.elapsed();
    println!("  ✓ Batch time: {:?}", batch_time);
    println!("    Average per operation: {:?}", batch_time / test_size);

    if sequential_time > batch_time {
        let speedup = sequential_time.as_secs_f64() / batch_time.as_secs_f64();
        println!("\n  Performance Summary:");
        println!(
            "    Batch operations are {:.2}x faster than sequential!",
            speedup
        );
        println!("    Time saved: {:?}", sequential_time - batch_time);
    }
    println!();

    // Clean up
    client.close();
    println!("Done!");

    Ok(())
}
