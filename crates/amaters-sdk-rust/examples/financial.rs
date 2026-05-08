//! Financial FHE example — requires:
//!   1. A running amaters-server on localhost:50051 (`amaters-server start`)
//!   2. Built with `--features fhe` (`cargo run --example financial --features fhe`)
//!
//! The `fhe` feature enables TFHE-based encryption. Without it, the example
//! still compiles but uses stub encryption that returns unencrypted bytes.
//!
//! ## Usage
//!
//! ```bash
//! # With stub encryption:
//! cargo run --example financial -p amaters-sdk-rust
//!
//! # With real TFHE encryption:
//! cargo run --example financial -p amaters-sdk-rust --features fhe
//! ```

use amaters_core::{CipherBlob, Key, Predicate, col};
use amaters_sdk_rust::{AmateRSClient, FheEncryptor, PaginationConfig, query};

/// A simplified credit profile for loan approval.
///
/// In a production system each field would be an independently-encrypted
/// ciphertext so the server can evaluate predicates on individual fields.
struct CreditProfile {
    applicant_id: u64,
    annual_income: u64, // in USD
    total_debt: u64,    // in USD
    credit_score: u32,  // 300-850
}

/// Serialise a `CreditProfile` to bytes.
///
/// Layout (little-endian):
/// ```text
/// [0..8]   applicant_id   (u64)
/// [8..16]  annual_income  (u64)
/// [16..24] total_debt     (u64)
/// [24..28] credit_score   (u32)
/// ```
fn serialize_profile(p: &CreditProfile) -> Vec<u8> {
    let mut buf = Vec::with_capacity(28);
    buf.extend_from_slice(&p.applicant_id.to_le_bytes());
    buf.extend_from_slice(&p.annual_income.to_le_bytes());
    buf.extend_from_slice(&p.total_debt.to_le_bytes());
    buf.extend_from_slice(&p.credit_score.to_le_bytes());
    buf
}

/// Deserialise a byte slice produced by [`serialize_profile`].
///
/// Returns `None` when the slice is too short.
fn deserialize_profile(bytes: &[u8]) -> Option<(u64, u64, u64, u32)> {
    if bytes.len() < 28 {
        return None;
    }
    let applicant_id = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let annual_income = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let total_debt = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
    let credit_score = u32::from_le_bytes(bytes[24..28].try_into().ok()?);
    Some((applicant_id, annual_income, total_debt, credit_score))
}

/// Build a `CipherBlob` threshold from a `u64` value (LE-encoded).
fn threshold_u64(v: u64) -> CipherBlob {
    CipherBlob::new(v.to_le_bytes().to_vec())
}

/// Print the table header for the results section.
fn print_header() {
    println!("  {:-<70}", "");
    println!(
        "  {:>12}  {:>14}  {:>12}  {:>8}",
        "Applicant ID", "Annual Income", "Total Debt", "Score"
    );
    println!("  {:-<70}", "");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== AmateRS Financial Credit Scoring FHE Example ===\n");

    #[cfg(not(feature = "fhe"))]
    {
        println!("Note: Running with stub encryption (data is NOT encrypted).");
        println!("Use --features fhe for real TFHE-based encryption.\n");
    }
    #[cfg(feature = "fhe")]
    {
        println!("Running with real TFHE encryption.\n");
    }

    // --------------------------------------------------------------------------
    // Step 1: Generate FHE keys
    // --------------------------------------------------------------------------
    println!("Step 1: Generating FHE keys ...");
    let encryptor = FheEncryptor::new()?;
    println!("  Keys ready.\n");

    // --------------------------------------------------------------------------
    // Step 2: Connect to the server
    // --------------------------------------------------------------------------
    println!("Step 2: Connecting to amaters-server at http://localhost:50051 ...");
    let client = match AmateRSClient::connect("http://localhost:50051").await {
        Ok(c) => {
            println!("  Connected successfully.\n");
            c
        }
        Err(e) => {
            println!(
                "  Could not connect to server: {e}\n\
                 \n\
                 To run this example with a live server:\n\
                 \n\
                 1. Start the server:  cargo run --bin amaters-server\n\
                 2. Re-run this example.\n\
                 \n\
                 The remainder of this demo will show the encryption/decryption\n\
                 workflow without server interaction.\n"
            );
            demo_local_encrypt_decrypt(&encryptor)?;
            return Ok(());
        }
    };

    let collection = "financial_profiles";

    // --------------------------------------------------------------------------
    // Step 3: Insert 10 credit profiles
    // --------------------------------------------------------------------------
    let profiles = vec![
        CreditProfile { applicant_id: 3001, annual_income: 85_000, total_debt:  6_000, credit_score: 720 },
        CreditProfile { applicant_id: 3002, annual_income: 42_000, total_debt: 18_000, credit_score: 640 },
        CreditProfile { applicant_id: 3003, annual_income: 130_000, total_debt:  3_000, credit_score: 790 },
        CreditProfile { applicant_id: 3004, annual_income: 55_000, total_debt:  9_500, credit_score: 700 },
        CreditProfile { applicant_id: 3005, annual_income: 31_000, total_debt: 25_000, credit_score: 580 },
        CreditProfile { applicant_id: 3006, annual_income: 72_000, total_debt:  7_800, credit_score: 740 },
        CreditProfile { applicant_id: 3007, annual_income: 48_000, total_debt: 11_200, credit_score: 665 },
        CreditProfile { applicant_id: 3008, annual_income: 95_000, total_debt:  4_200, credit_score: 760 },
        CreditProfile { applicant_id: 3009, annual_income: 29_000, total_debt: 30_000, credit_score: 520 },
        CreditProfile { applicant_id: 3010, annual_income: 61_000, total_debt:  8_400, credit_score: 710 },
    ];

    println!("Step 3: Encrypting and inserting {} credit profiles ...", profiles.len());

    for profile in &profiles {
        let key = Key::from_str(&format!("applicant:{}", profile.applicant_id));
        let plaintext = serialize_profile(profile);
        let encrypted = encryptor.encrypt(&plaintext)?;
        client.set(collection, &key, &encrypted).await?;
        println!(
            "  Inserted applicant {} (income ${}, debt ${}, score {})",
            profile.applicant_id, profile.annual_income, profile.total_debt, profile.credit_score
        );
    }
    println!();

    // --------------------------------------------------------------------------
    // Step 4: FHE filter query — income > 50_000 AND debt < 10_000
    //
    // Note: The server-side predicate operates on the encrypted blob as a whole.
    // In a full FHE deployment the predicate would target individual encrypted
    // fields. Here we use Predicate::And with column symbolic names to
    // demonstrate the API surface.
    // --------------------------------------------------------------------------
    println!("Step 4: Running FHE filter query: income > 50000 AND debt < 10000 ...");
    println!("  (Server evaluates predicates on ciphertext — never sees plaintext values.)\n");

    let income_threshold = threshold_u64(50_000);
    let debt_threshold = threshold_u64(10_000);

    let predicate = Predicate::And(
        Box::new(Predicate::Gt(col("annual_income"), income_threshold)),
        Box::new(Predicate::Lt(col("total_debt"), debt_threshold)),
    );

    let filter_query = query(collection).filter(predicate);
    let result = client.execute_query(&filter_query).await?;

    // --------------------------------------------------------------------------
    // Step 5: Decrypt and display approved applicants
    // --------------------------------------------------------------------------
    println!("Step 5: Decrypting results ...\n");

    let approved: Vec<(u64, u64, u64, u32)> = match result {
        amaters_sdk_rust::QueryResult::Multi(kvs) => {
            let mut rows = Vec::new();
            for (_key, cipher) in &kvs {
                let plaintext = encryptor.decrypt(cipher)?;
                if let Some(fields) = deserialize_profile(&plaintext) {
                    rows.push(fields);
                }
            }
            rows
        }
        amaters_sdk_rust::QueryResult::Single(_) => {
            println!("  (Unexpected single result from filter query)");
            vec![]
        }
        amaters_sdk_rust::QueryResult::Success { affected_rows } => {
            println!(
                "  Query returned Success with {} affected rows.",
                affected_rows
            );
            vec![]
        }
    };

    println!("  Approved applicants ({} record(s)):", approved.len());
    print_header();
    for (id, income, debt, score) in &approved {
        println!(
            "  {:>12}  {:>14}  {:>12}  {:>8}",
            id,
            format!("${}", income),
            format!("${}", debt),
            score
        );
    }
    println!("  {:-<70}", "");
    println!();

    // --------------------------------------------------------------------------
    // Step 6: Paginated scan — retrieve all profiles 5 at a time
    // --------------------------------------------------------------------------
    println!("Step 6: Demonstrating cursor-based pagination (page size = 5) ...\n");

    let prefix = Key::from_str("applicant:");
    let page_size = 5;
    let pagination = PaginationConfig::new(page_size);

    let first_page = client.scan(collection, &prefix, &pagination).await?;

    println!(
        "  Page 1: {} item(s), has_more={}",
        first_page.items.len(),
        first_page.has_more
    );
    for (key, _cipher) in &first_page.items {
        println!("    {}", key);
    }

    if first_page.has_more {
        if let Some(cursor) = first_page.next_cursor {
            let next_pagination = PaginationConfig::new(page_size).with_cursor(cursor);
            let second_page = client.scan(collection, &prefix, &next_pagination).await?;

            println!(
                "\n  Page 2: {} item(s), has_more={}",
                second_page.items.len(),
                second_page.has_more
            );
            for (key, _cipher) in &second_page.items {
                println!("    {}", key);
            }
        }
    } else {
        println!("  (All records fit on a single page)");
    }

    println!();

    // --------------------------------------------------------------------------
    // Clean up
    // --------------------------------------------------------------------------
    client.close();
    println!("Done!");

    Ok(())
}

/// Runs a local encryption/decryption demonstration when the server is absent.
fn demo_local_encrypt_decrypt(encryptor: &FheEncryptor) -> anyhow::Result<()> {
    println!("--- Local FHE Demo (no server) ---\n");

    let profiles = vec![
        CreditProfile { applicant_id: 9001, annual_income: 60_000, total_debt: 5_000, credit_score: 750 },
        CreditProfile { applicant_id: 9002, annual_income: 40_000, total_debt: 15_000, credit_score: 620 },
    ];

    for p in &profiles {
        let plaintext = serialize_profile(p);
        let encrypted = encryptor.encrypt(&plaintext)?;
        let decrypted = encryptor.decrypt(&encrypted)?;

        assert_eq!(
            plaintext, decrypted,
            "Round-trip failed for applicant {}",
            p.applicant_id
        );

        let (id, income, debt, score) = deserialize_profile(&decrypted)
            .ok_or_else(|| anyhow::anyhow!("Failed to deserialize profile"))?;

        println!(
            "  Applicant {} | income ${} | debt ${} | score {} — round-trip OK",
            id, income, debt, score
        );
    }

    println!("\n  Encryption/decryption round-trip verified.\n");
    Ok(())
}
