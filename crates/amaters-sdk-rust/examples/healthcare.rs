//! Healthcare FHE example — requires:
//!   1. A running amaters-server on localhost:50051 (`amaters-server start`)
//!   2. Built with `--features fhe` (`cargo run --example healthcare --features fhe`)
//!
//! The `fhe` feature enables TFHE-based encryption. Without it, the example
//! still compiles but uses stub encryption that returns unencrypted bytes.
//!
//! ## Usage
//!
//! ```bash
//! # With stub encryption (no server required for encrypt/decrypt demo):
//! cargo run --example healthcare -p amaters-sdk-rust
//!
//! # With real TFHE encryption (still needs a server for insert/query):
//! cargo run --example healthcare -p amaters-sdk-rust --features fhe
//! ```

use amaters_core::{CipherBlob, Key, Predicate, col};
use amaters_sdk_rust::{AmateRSClient, FheEncryptor, query};

/// A simplified patient record.
///
/// In a real healthcare application, all fields would be encrypted
/// individually so the server can perform FHE comparisons on them
/// without ever seeing the plaintext.
struct PatientRecord {
    id: u64,
    age: u32,
    /// Simplified genetic marker tag, e.g. "BRCA1".
    dna_marker: String,
}

/// Serialize a `PatientRecord` into a fixed-length byte payload that can be
/// encrypted as a single `CipherBlob`.
///
/// Layout (little-endian):
/// ```text
/// [0..8]   id       (u64)
/// [8..12]  age      (u32)
/// [12..44] dna_marker (32-byte zero-padded UTF-8)
/// ```
fn serialize_record(record: &PatientRecord) -> Vec<u8> {
    let mut buf = Vec::with_capacity(44);
    buf.extend_from_slice(&record.id.to_le_bytes());
    buf.extend_from_slice(&record.age.to_le_bytes());

    // Pad / truncate DNA marker to exactly 32 bytes.
    let marker_bytes = record.dna_marker.as_bytes();
    let mut marker_buf = [0u8; 32];
    let copy_len = marker_bytes.len().min(32);
    marker_buf[..copy_len].copy_from_slice(&marker_bytes[..copy_len]);
    buf.extend_from_slice(&marker_buf);

    buf
}

/// Deserialize a byte payload back into constituent fields.
///
/// Returns `None` if the payload is too short to decode.
fn deserialize_record(bytes: &[u8]) -> Option<(u64, u32, String)> {
    if bytes.len() < 44 {
        return None;
    }

    let id = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let age = u32::from_le_bytes(bytes[8..12].try_into().ok()?);

    // Strip trailing NUL bytes before converting to string.
    let marker_raw = &bytes[12..44];
    let nul_pos = marker_raw.iter().position(|&b| b == 0).unwrap_or(32);
    let dna_marker = String::from_utf8_lossy(&marker_raw[..nul_pos]).into_owned();

    Some((id, age, dna_marker))
}

/// Encode a `u32` age value as 4 little-endian bytes for use in a predicate
/// threshold `CipherBlob`.
fn age_bytes(age: u32) -> CipherBlob {
    CipherBlob::new(age.to_le_bytes().to_vec())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== AmateRS Healthcare FHE Example ===\n");

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
    // Step 1: Generate FHE keys (or load from disk in production)
    // --------------------------------------------------------------------------
    println!("Step 1: Generating FHE keys (may take a few seconds with --features fhe)...");
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

    // --------------------------------------------------------------------------
    // Step 3: Create 5 patient records (ages 50–80)
    // --------------------------------------------------------------------------
    let collection = "healthcare_patients";

    let records = vec![
        PatientRecord { id: 1001, age: 52, dna_marker: "BRCA1".to_string() },
        PatientRecord { id: 1002, age: 67, dna_marker: "BRCA2".to_string() },
        PatientRecord { id: 1003, age: 71, dna_marker: "TP53".to_string() },
        PatientRecord { id: 1004, age: 58, dna_marker: "MLH1".to_string() },
        PatientRecord { id: 1005, age: 79, dna_marker: "APC".to_string() },
    ];

    println!("Step 3: Encrypting and inserting {} patient records ...", records.len());

    for record in &records {
        let key = Key::from_str(&format!("patient:{}", record.id));
        let plaintext = serialize_record(record);
        let encrypted = encryptor.encrypt(&plaintext)?;

        client.set(collection, &key, &encrypted).await?;
        println!(
            "  Inserted patient {} (age {}, marker {})",
            record.id, record.age, record.dna_marker
        );
    }
    println!();

    // --------------------------------------------------------------------------
    // Step 4: FHE filter query — patients with age > 65
    // --------------------------------------------------------------------------
    println!("Step 4: Running FHE filter query: age > 65 ...");
    println!("  (The server evaluates the predicate on encrypted data without");
    println!("   ever seeing plaintext ages or markers.)\n");

    let threshold = age_bytes(65);
    // age is stored starting at byte offset 8 in the serialised payload.
    // The column ref "age_offset_8" is symbolic; the server applies the
    // predicate to the full encrypted blob. In a full FHE deployment the
    // predicate would target the encrypted age field specifically.
    let filter_predicate = Predicate::Gt(col("age"), threshold);
    let filter_query = query(collection).filter(filter_predicate);

    let result = client.execute_query(&filter_query).await?;

    // --------------------------------------------------------------------------
    // Step 5: Decrypt and display matching records
    // --------------------------------------------------------------------------
    println!("Step 5: Decrypting results ...\n");

    match result {
        amaters_sdk_rust::QueryResult::Multi(kvs) => {
            println!("  Patients with age > 65 ({} record(s) found):", kvs.len());
            println!("  {:-<55}", "");
            println!("  {:>10}  {:>5}  {:<15}", "Patient ID", "Age", "DNA Marker");
            println!("  {:-<55}", "");

            for (key, cipher) in &kvs {
                let plaintext = encryptor.decrypt(cipher)?;
                match deserialize_record(&plaintext) {
                    Some((id, age, marker)) => {
                        println!("  {:>10}  {:>5}  {:<15}", id, age, marker);
                    }
                    None => {
                        println!("  (could not decode record for key {})", key);
                    }
                }
            }
            println!("  {:-<55}", "");
        }
        amaters_sdk_rust::QueryResult::Single(_) => {
            println!("  (Unexpected single result from filter query)");
        }
        amaters_sdk_rust::QueryResult::Success { affected_rows } => {
            println!("  Query returned a Success result with {} affected rows.", affected_rows);
            println!("  This usually means the server processed the filter but returned no rows.");
        }
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
///
/// This shows that the FHE workflow is correct even without a live server.
fn demo_local_encrypt_decrypt(encryptor: &FheEncryptor) -> anyhow::Result<()> {
    println!("--- Local FHE Demo (no server) ---\n");

    let records = vec![
        PatientRecord { id: 2001, age: 62, dna_marker: "BRCA1".to_string() },
        PatientRecord { id: 2002, age: 70, dna_marker: "TP53".to_string() },
    ];

    for record in &records {
        let plaintext = serialize_record(record);
        let encrypted = encryptor.encrypt(&plaintext)?;
        let decrypted = encryptor.decrypt(&encrypted)?;

        assert_eq!(
            plaintext, decrypted,
            "Round-trip check failed for patient {}",
            record.id
        );

        let (id, age, marker) = deserialize_record(&decrypted)
            .ok_or_else(|| anyhow::anyhow!("Failed to deserialize record"))?;

        println!(
            "  Patient {} | age {} | marker {} — encrypt/decrypt round-trip OK",
            id, age, marker
        );
    }

    println!("\n  Encryption/decryption round-trip verified.\n");
    Ok(())
}
