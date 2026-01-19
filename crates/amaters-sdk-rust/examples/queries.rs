//! Query builder example for AmateRS SDK
//!
//! This example demonstrates the query builder API:
//! - Building simple queries (Get, Set, Delete)
//! - Creating filter predicates (Eq, Gt, Lt, Gte, Lte)
//! - Combining predicates with AND/OR/NOT
//! - Range queries
//! - Update operations with FHE arithmetic
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
//! cargo run --example queries
//! ```

use amaters_core::{CipherBlob, Key, Predicate, Update, col};
use amaters_sdk_rust::{AmateRSClient, query};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== AmateRS SDK Query Builder Example ===\n");

    // Connect to server
    println!("Connecting to server...");
    let client = AmateRSClient::connect("http://localhost:50051").await?;
    println!("Connected!\n");

    // Setup: Insert some test data first
    println!("Setup: Inserting test data...");
    client
        .set(
            "users",
            &Key::from_str("user:123"),
            &CipherBlob::new(vec![1, 2, 3]),
        )
        .await?;
    client
        .set(
            "users",
            &Key::from_str("user:456"),
            &CipherBlob::new(vec![4, 5, 6]),
        )
        .await?;
    println!("  ✓ Test data inserted\n");

    // Example 1: Simple Get query
    println!("Example 1: Simple Get query");
    let q = query("users").get(Key::from_str("user:123"));
    println!("  Query: Get user:123 from collection 'users'");
    let result = client.execute_query(&q).await?;
    match result {
        amaters_sdk_rust::QueryResult::Single(Some(value)) => {
            println!("  ✓ Query executed: retrieved {} bytes", value.len());
        }
        _ => println!("  ✓ Query executed: key not found"),
    }
    println!();

    // Example 2: Simple Set query
    println!("Example 2: Simple Set query");
    let q = query("users").set(
        Key::from_str("user:456"),
        CipherBlob::new(b"encrypted user data".to_vec()),
    );
    println!("  Query: Set user:456 in collection 'users'");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Example 3: Simple Delete query
    println!("Example 3: Simple Delete query");
    let q = query("users").delete(Key::from_str("user:789"));
    println!("  Query: Delete user:789 from collection 'users'");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Example 4: Filter with equality predicate
    println!("Example 4: Filter with equality predicate");
    let q = query("users")
        .where_clause()
        .eq(col("status"), CipherBlob::new(vec![1]))
        .build();
    println!("  Query: SELECT * FROM users WHERE status = 1");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Example 5: Filter with comparison predicates
    println!("Example 5: Filter with comparison predicates");
    let q = query("users")
        .where_clause()
        .gt(col("age"), CipherBlob::new(vec![18]))
        .build();
    println!("  Query: SELECT * FROM users WHERE age > 18");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Example 6: Complex filter with AND
    println!("Example 6: Complex filter with AND");
    let q = query("users")
        .where_clause()
        .eq(col("active"), CipherBlob::new(vec![1]))
        .and(Predicate::Gt(col("age"), CipherBlob::new(vec![18])))
        .build();
    println!("  Query: SELECT * FROM users WHERE active = 1 AND age > 18");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Example 7: Complex filter with OR
    println!("Example 7: Complex filter with OR");
    let q = query("users")
        .where_clause()
        .eq(col("role"), CipherBlob::new(vec![1]))
        .or(Predicate::Eq(col("role"), CipherBlob::new(vec![2])))
        .build();
    println!("  Query: SELECT * FROM users WHERE role = 1 OR role = 2");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Example 8: Complex nested predicates
    println!("Example 8: Complex nested predicates");
    let q = query("users")
        .where_clause()
        .eq(col("active"), CipherBlob::new(vec![1]))
        .and(Predicate::Gt(col("age"), CipherBlob::new(vec![18])))
        .and(Predicate::Or(
            Box::new(Predicate::Eq(col("role"), CipherBlob::new(vec![1]))),
            Box::new(Predicate::Eq(col("role"), CipherBlob::new(vec![2]))),
        ))
        .build();
    println!("  Query: SELECT * FROM users");
    println!("    WHERE active = 1");
    println!("      AND age > 18");
    println!("      AND (role = 1 OR role = 2)");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Example 9: NOT predicate
    println!("Example 9: NOT predicate");
    let q = query("users")
        .where_clause()
        .eq(col("active"), CipherBlob::new(vec![0]))
        .not()
        .build();
    println!("  Query: SELECT * FROM users WHERE NOT (active = 0)");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Example 10: Range query
    println!("Example 10: Range query");
    let q = query("logs").range(Key::from_str("2024-01-01"), Key::from_str("2024-12-31"));
    println!("  Query: SELECT * FROM logs WHERE key >= '2024-01-01' AND key < '2024-12-31'");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Example 11: Update query with Set operation
    println!("Example 11: Update query with Set operation");
    let updates = vec![Update::Set(col("status"), CipherBlob::new(vec![2]))];
    let q = query("users")
        .where_clause()
        .eq(col("id"), CipherBlob::new(vec![1]))
        .update(updates);
    println!("  Query: UPDATE users SET status = 2 WHERE id = 1");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Example 12: Update query with FHE operations
    println!("Example 12: Update query with FHE arithmetic operations");
    let updates = vec![
        Update::Add(col("counter"), CipherBlob::new(vec![1])),
        Update::Mul(col("multiplier"), CipherBlob::new(vec![2])),
    ];
    let q = query("stats")
        .where_clause()
        .eq(col("metric"), CipherBlob::new(vec![1]))
        .update(updates);
    println!("  Query: UPDATE stats");
    println!("    SET counter = counter + 1,");
    println!("        multiplier = multiplier * 2");
    println!("    WHERE metric = 1");
    println!("  Note: Add and Mul operations work on encrypted values (FHE)");
    let _result = client.execute_query(&q).await?;
    println!("  ✓ Query executed\n");

    // Clean up
    client.close();
    println!("Done!");

    Ok(())
}
