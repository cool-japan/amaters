//! Quickstart example for AmateRS SDK
//!
//! This example demonstrates basic usage of the SDK:
//! - Connecting to a server
//! - Setting and getting encrypted values (CipherBlob)
//! - Deleting keys
//! - Checking if keys exist
//! - Error handling
//! - Health checks and connection pooling
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
//! cargo run --example quickstart
//! ```

use amaters_core::{CipherBlob, Key};
use amaters_sdk_rust::{AmateRSClient, ClientConfig};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing for logging
    tracing_subscriber::fmt::init();

    println!("=== AmateRS SDK Quickstart Example ===\n");

    // Configure the client
    println!("Configuring client...");
    let config = ClientConfig::new("http://localhost:50051")
        .with_connect_timeout(Duration::from_secs(5))
        .with_request_timeout(Duration::from_secs(10))
        .with_max_connections(10);

    // Connect to the server
    println!("Connecting to server at {}...", config.server_addr);
    let client = AmateRSClient::connect_with_config(config).await?;
    println!("Connected successfully!\n");

    // Collection name
    let collection = "users";

    // Example 1: Set a value
    // CipherBlob can contain any encrypted data - the server never sees plaintext
    println!("Example 1: Setting a value");
    let key = Key::from_str("user:alice");
    let value = CipherBlob::new(b"Alice's encrypted data".to_vec());
    println!("  Setting key '{}' with {} bytes", key, value.len());
    client.set(collection, &key, &value).await?;
    println!("  ✓ Value set successfully\n");

    // Example 2: Get a value
    // The get operation retrieves the encrypted data from the server
    println!("Example 2: Getting a value");
    println!("  Retrieving key '{}'", key);
    match client.get(collection, &key).await? {
        Some(retrieved) => {
            println!("  ✓ Found value: {} bytes", retrieved.len());
            println!("    Value matches: {}", retrieved == value);
            // In production, you would decrypt this with your FHE keys
        }
        None => {
            println!("  ✗ Key not found");
        }
    }
    println!();

    // Example 3: Check if key exists
    println!("Example 3: Checking if key exists");
    let exists = client.contains(collection, &key).await?;
    println!("  Key '{}' exists: {}", key, exists);
    println!();

    // Example 4: Set multiple values
    println!("Example 4: Setting multiple values");
    let users = vec![
        ("user:bob", "Bob's data"),
        ("user:charlie", "Charlie's data"),
        ("user:dave", "Dave's data"),
    ];

    for (key_str, data) in &users {
        let key = Key::from_str(key_str);
        let value = CipherBlob::new(data.as_bytes().to_vec());
        client.set(collection, &key, &value).await?;
        println!("  ✓ Set key '{}'", key_str);
    }
    println!();

    // Example 5: Delete a key
    println!("Example 5: Deleting a key");
    println!("  Deleting key '{}'", key);
    client.delete(collection, &key).await?;
    println!("  ✓ Key deleted successfully");

    // Verify deletion
    let exists = client.contains(collection, &key).await?;
    println!("  Key still exists: {}", exists);
    println!();

    // Example 6: Connection pool stats
    println!("Example 6: Connection pool statistics");
    let stats = client.pool_stats();
    println!("  Total connections: {}", stats.total_connections);
    println!("  Active connections: {}", stats.active_connections);
    println!("  Idle connections: {}", stats.idle_connections);
    println!("  Max connections: {}", stats.max_connections);
    println!();

    // Example 7: Health check
    println!("Example 7: Health check");
    match client.health_check().await {
        Ok(()) => println!("  ✓ Server is healthy"),
        Err(e) => println!("  ✗ Health check failed: {}", e),
    }
    println!();

    // Clean up
    println!("Closing client connections...");
    client.close();
    println!("Done!");

    Ok(())
}
