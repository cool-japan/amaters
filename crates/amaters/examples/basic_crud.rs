//! Basic CRUD operations with AmateRS LSM-Tree storage engine.
//!
//! Demonstrates:
//! - Creating an LsmTree with default configuration
//! - Put/Get/Delete operations
//! - Range scans
//! - Error handling patterns

use amaters::core::storage::LsmTree;
use amaters::core::{CipherBlob, Key};

fn main() -> amaters::core::Result<()> {
    // Use a temporary directory for storage
    let temp_dir = std::env::temp_dir().join("amaters_basic_crud_example");

    // Clean up any previous run
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    // =========================================================================
    // 1. Create an LSM-Tree with default configuration
    // =========================================================================
    println!("=== AmateRS Basic CRUD Example ===\n");
    println!("Creating LSM-Tree at: {}", temp_dir.display());

    let tree = LsmTree::new(&temp_dir)?;
    println!("LSM-Tree created successfully.\n");

    // =========================================================================
    // 2. Put operations - store encrypted data
    // =========================================================================
    println!("--- PUT Operations ---");

    // Store some user records (simulated encrypted data)
    let users = vec![
        ("user:001", vec![10, 20, 30, 40]),
        ("user:002", vec![50, 60, 70, 80]),
        ("user:003", vec![90, 100, 110, 120]),
        ("user:004", vec![130, 140, 150, 160]),
        ("user:005", vec![170, 180, 190, 200]),
    ];

    for (key_str, data) in &users {
        let key = Key::from_str(key_str);
        let value = CipherBlob::new(data.clone());
        tree.put(key, value)?;
        println!("  PUT {} -> {} bytes", key_str, data.len());
    }
    println!();

    // =========================================================================
    // 3. Get operations - retrieve data
    // =========================================================================
    println!("--- GET Operations ---");

    // Retrieve an existing key
    let key = Key::from_str("user:003");
    match tree.get(&key)? {
        Some(value) => {
            println!(
                "  GET user:003 -> {:?} ({} bytes)",
                value.as_bytes(),
                value.len()
            );
        }
        None => {
            println!("  GET user:003 -> NOT FOUND (unexpected)");
        }
    }

    // Try to retrieve a non-existent key
    let missing_key = Key::from_str("user:999");
    match tree.get(&missing_key)? {
        Some(_) => {
            println!("  GET user:999 -> Found (unexpected)");
        }
        None => {
            println!("  GET user:999 -> NOT FOUND (expected)");
        }
    }
    println!();

    // =========================================================================
    // 4. Delete operations
    // =========================================================================
    println!("--- DELETE Operations ---");

    let delete_key = Key::from_str("user:002");
    tree.delete(delete_key.clone())?;
    println!("  DELETE user:002");

    // Verify deletion
    match tree.get(&delete_key)? {
        Some(_) => println!("  Verify: user:002 still exists (unexpected)"),
        None => println!("  Verify: user:002 successfully deleted"),
    }
    println!();

    // =========================================================================
    // 5. Range scan
    // =========================================================================
    println!("--- RANGE SCAN ---");

    let start = Key::from_str("user:001");
    let end = Key::from_str("user:005");
    let results = tree.range(&start, &end)?;

    println!("  Range [user:001, user:005): {} results", results.len());
    for (key, value) in &results {
        println!(
            "    {} -> {} bytes",
            String::from_utf8_lossy(key.as_bytes()),
            value.len()
        );
    }
    println!();

    // =========================================================================
    // 6. Overwrite an existing key
    // =========================================================================
    println!("--- OVERWRITE ---");

    let overwrite_key = Key::from_str("user:001");
    let new_value = CipherBlob::new(vec![255, 254, 253, 252, 251]);
    tree.put(overwrite_key.clone(), new_value)?;
    println!("  PUT user:001 -> [255, 254, 253, 252, 251]");

    match tree.get(&overwrite_key)? {
        Some(value) => {
            println!("  GET user:001 -> {:?}", value.as_bytes());
        }
        None => {
            println!("  GET user:001 -> NOT FOUND (unexpected)");
        }
    }
    println!();

    // =========================================================================
    // 7. Error handling example
    // =========================================================================
    println!("--- ERROR HANDLING ---");

    // Demonstrate using Result with the ? operator
    fn safe_lookup(tree: &LsmTree, key_str: &str) -> amaters::core::Result<Option<Vec<u8>>> {
        let key = Key::from_str(key_str);
        let result = tree.get(&key)?;
        Ok(result.map(|blob| blob.as_bytes().to_vec()))
    }

    match safe_lookup(&tree, "user:003") {
        Ok(Some(data)) => println!("  Safe lookup user:003: {} bytes", data.len()),
        Ok(None) => println!("  Safe lookup user:003: not found"),
        Err(e) => println!("  Safe lookup user:003: error - {}", e),
    }

    // =========================================================================
    // Cleanup
    // =========================================================================
    tree.close()?;
    std::fs::remove_dir_all(&temp_dir).ok();
    println!("Cleanup complete. Example finished.");

    Ok(())
}
