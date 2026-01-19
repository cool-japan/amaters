//! FHE Filter Query example for AmateRS SDK
//!
//! This example provides a comprehensive demonstration of FHE filtering:
//! - Multiple predicate types (Eq, Gt, Lt, Gte, Lte)
//! - Complex predicate combinations (AND, OR, NOT)
//! - Real-world use cases
//! - Performance considerations
//!
//! ## The Power of FHE Filtering
//!
//! Traditional databases require decrypting data to filter it.
//! With FHE, the server can filter encrypted data without ever
//! seeing the plaintext values.
//!
//! Flow:
//! 1. Client encrypts data with FHE keys
//! 2. Client sends encrypted data to server
//! 3. Client builds filter with encrypted predicates
//! 4. Server executes filter on encrypted data (homomorphically)
//! 5. Server returns encrypted results
//! 6. Client decrypts results
//!
//! The server NEVER sees plaintext!
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
//! cargo run --example filter_query
//! ```

use amaters_core::{CipherBlob, Key, Predicate, col};
use amaters_sdk_rust::{AmateRSClient, FheEncryptor, QueryResult, query};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== AmateRS SDK FHE Filter Query Example ===\n");

    #[cfg(not(feature = "fhe"))]
    println!("Note: Using stub FHE (data not actually encrypted)");
    #[cfg(not(feature = "fhe"))]
    println!("Enable 'fhe' feature for real TFHE encryption\n");

    // Setup: Create encryptor and connect
    println!("Setup: Initializing FHE encryptor...");
    let encryptor = FheEncryptor::new()?;
    println!("✓ Encryptor ready\n");

    println!("Connecting to server...");
    let client = AmateRSClient::connect("http://localhost:50051").await?;
    println!("✓ Connected!\n");

    // ===== Example 1: Employee Database with Salary Filtering =====
    println!("Example 1: Employee Database - Salary Filtering");
    println!("  Scenario: HR wants to find employees with salary in range\n");

    let employees = vec![
        ("emp:001", "Alice", 45_000u32),
        ("emp:002", "Bob", 55_000u32),
        ("emp:003", "Charlie", 65_000u32),
        ("emp:004", "Dave", 75_000u32),
        ("emp:005", "Eve", 85_000u32),
    ];

    println!("  Storing employee data (encrypted)...");
    for (id, name, salary) in &employees {
        let key = Key::from_str(id);
        // Store salary as encrypted bytes
        let salary_bytes = salary.to_le_bytes();
        let encrypted_salary = encryptor.encrypt(&salary_bytes)?;
        client.set("employees", &key, &encrypted_salary).await?;
        println!("    {} ({}) - salary: ${}", id, name, salary);
    }
    println!();

    // Filter: salary >= 50000 AND salary <= 70000
    println!("  Filter: salary >= 50,000 AND salary <= 70,000");
    let min_salary = 50_000u32.to_le_bytes();
    let max_salary = 70_000u32.to_le_bytes();
    let encrypted_min = encryptor.encrypt(&min_salary)?;
    let encrypted_max = encryptor.encrypt(&max_salary)?;

    let salary_filter = Predicate::And(
        Box::new(Predicate::Gte(col("salary"), encrypted_min)),
        Box::new(Predicate::Lte(col("salary"), encrypted_max)),
    );

    let filter_query = query("employees").filter(salary_filter);
    let result = client.execute_query(&filter_query).await?;

    println!("  Results (after decryption):");
    if let QueryResult::Multi(values) = result {
        for (key, encrypted_value) in values {
            let salary_bytes = encryptor.decrypt(&encrypted_value)?;
            if salary_bytes.len() >= 4 {
                let salary = u32::from_le_bytes([
                    salary_bytes[0],
                    salary_bytes[1],
                    salary_bytes[2],
                    salary_bytes[3],
                ]);
                println!("    {} - ${}", key, salary);
            }
        }
    }
    println!("  Expected: Bob (55k), Charlie (65k)\n");

    // ===== Example 2: Age Verification (Equality) =====
    println!("Example 2: Age Verification - Exact Match");
    println!("  Scenario: Find all users aged exactly 25\n");

    let users = vec![
        ("user:alice", 25u8),
        ("user:bob", 30u8),
        ("user:charlie", 25u8),
        ("user:dave", 35u8),
    ];

    println!("  Storing user ages (encrypted)...");
    for (id, age) in &users {
        let key = Key::from_str(id);
        let encrypted_age = encryptor.encrypt(&[*age])?;
        client.set("users", &key, &encrypted_age).await?;
        println!("    {} - age: {}", id, age);
    }
    println!();

    println!("  Filter: age = 25");
    let target_age = encryptor.encrypt(&[25u8])?;
    let age_filter = Predicate::Eq(col("age"), target_age);

    let filter_query = query("users").filter(age_filter);
    let result = client.execute_query(&filter_query).await?;

    println!("  Results (after decryption):");
    if let QueryResult::Multi(values) = result {
        for (key, encrypted_value) in values {
            let age_bytes = encryptor.decrypt(&encrypted_value)?;
            if !age_bytes.is_empty() {
                println!("    {} - age: {}", key, age_bytes[0]);
            }
        }
    }
    println!("  Expected: user:alice, user:charlie\n");

    // ===== Example 3: Complex Boolean Logic =====
    println!("Example 3: Complex Filtering - OR and NOT");
    println!("  Scenario: Find active users who are either admin OR moderator\n");

    let user_roles = vec![
        ("user:001", 1u8), // admin
        ("user:002", 2u8), // moderator
        ("user:003", 3u8), // regular user
        ("user:004", 1u8), // admin
        ("user:005", 4u8), // guest
    ];

    println!("  Storing user roles (encrypted)...");
    println!("    Roles: 1=admin, 2=moderator, 3=user, 4=guest");
    for (id, role) in &user_roles {
        let key = Key::from_str(id);
        let encrypted_role = encryptor.encrypt(&[*role])?;
        client.set("user_roles", &key, &encrypted_role).await?;
        println!("    {} - role: {}", id, role);
    }
    println!();

    // Filter: role = 1 OR role = 2
    println!("  Filter: role = admin (1) OR role = moderator (2)");
    let admin_role = encryptor.encrypt(&[1u8])?;
    let moderator_role = encryptor.encrypt(&[2u8])?;

    let role_filter = Predicate::Or(
        Box::new(Predicate::Eq(col("role"), admin_role)),
        Box::new(Predicate::Eq(col("role"), moderator_role)),
    );

    let filter_query = query("user_roles").filter(role_filter);
    let result = client.execute_query(&filter_query).await?;

    println!("  Results (after decryption):");
    if let QueryResult::Multi(values) = result {
        for (key, encrypted_value) in values {
            let role_bytes = encryptor.decrypt(&encrypted_value)?;
            if !role_bytes.is_empty() {
                let role_name = match role_bytes[0] {
                    1 => "admin",
                    2 => "moderator",
                    3 => "user",
                    4 => "guest",
                    _ => "unknown",
                };
                println!("    {} - role: {}", key, role_name);
            }
        }
    }
    println!("  Expected: user:001 (admin), user:002 (moderator), user:004 (admin)\n");

    // ===== Example 4: NOT Predicate =====
    println!("Example 4: Negation - Exclude Values");
    println!("  Scenario: Find all non-guest users\n");

    // Filter: NOT (role = 4)
    println!("  Filter: NOT (role = guest)");
    let guest_role = encryptor.encrypt(&[4u8])?;
    let not_guest_filter = Predicate::Not(Box::new(Predicate::Eq(col("role"), guest_role)));

    let filter_query = query("user_roles").filter(not_guest_filter);
    let result = client.execute_query(&filter_query).await?;

    println!("  Results (after decryption):");
    if let QueryResult::Multi(values) = result {
        println!("    Found {} non-guest users:", values.len());
        for (key, encrypted_value) in values {
            let role_bytes = encryptor.decrypt(&encrypted_value)?;
            if !role_bytes.is_empty() {
                let role_name = match role_bytes[0] {
                    1 => "admin",
                    2 => "moderator",
                    3 => "user",
                    4 => "guest",
                    _ => "unknown",
                };
                println!("      {} - role: {}", key, role_name);
            }
        }
    }
    println!("  Expected: All except user:005\n");

    // ===== Example 5: Three-Way Complex Filter =====
    println!("Example 5: Complex Multi-Condition Filter");
    println!("  Scenario: Healthcare - Find patients meeting criteria\n");

    let patients = vec![
        ("patient:001", 45u8, 120u8), // age, blood_pressure
        ("patient:002", 55u8, 140u8),
        ("patient:003", 35u8, 110u8),
        ("patient:004", 65u8, 150u8),
        ("patient:005", 50u8, 130u8),
    ];

    println!("  Storing patient data (encrypted)...");
    for (id, age, bp) in &patients {
        let key = Key::from_str(id);
        // Store as [age, blood_pressure]
        let data = vec![*age, *bp];
        let encrypted_data = encryptor.encrypt(&data)?;
        client.set("patients", &key, &encrypted_data).await?;
        println!("    {} - age: {}, BP: {}", id, age, bp);
    }
    println!();

    // Filter: age > 40 AND bp < 145 AND NOT (age > 60)
    println!("  Filter: age > 40 AND blood_pressure < 145 AND age <= 60");
    let age_40 = encryptor.encrypt(&[40u8])?;
    let bp_145 = encryptor.encrypt(&[145u8])?;
    let age_60 = encryptor.encrypt(&[60u8])?;

    let complex_filter = Predicate::And(
        Box::new(Predicate::And(
            Box::new(Predicate::Gt(col("age"), age_40)),
            Box::new(Predicate::Lt(col("blood_pressure"), bp_145)),
        )),
        Box::new(Predicate::Not(Box::new(Predicate::Gt(col("age"), age_60)))),
    );

    let filter_query = query("patients").filter(complex_filter);
    let result = client.execute_query(&filter_query).await?;

    println!("  Results (after decryption):");
    if let QueryResult::Multi(values) = result {
        for (key, encrypted_value) in values {
            let data = encryptor.decrypt(&encrypted_value)?;
            if data.len() >= 2 {
                println!("    {} - age: {}, BP: {}", key, data[0], data[1]);
            }
        }
    }
    println!("  Expected: patient:001 (45, 120), patient:005 (50, 130)\n");

    // Summary
    println!("Summary: FHE Filter Capabilities");
    println!("  ✓ Comparison operators: =, >, <, >=, <=");
    println!("  ✓ Boolean logic: AND, OR, NOT");
    println!("  ✓ Complex nested predicates");
    println!("  ✓ Server never sees plaintext");
    println!("  ✓ Results remain encrypted until client decrypts");
    println!();
    println!("  Use Cases:");
    println!("    - Healthcare: Private patient record queries");
    println!("    - Finance: Confidential credit score analysis");
    println!("    - HR: Salary and benefits filtering");
    println!("    - Compliance: Audit without exposing sensitive data");
    println!();

    // Clean up
    client.close();
    println!("Done!");

    Ok(())
}
