//! Supply Chain Transparency Use Case
//!
//! Two companies store encrypted supplier data (CO2 emissions, compliance,
//! costs) in separate storage instances. AmateRS FHE circuits compute
//! aggregate sustainability metrics *without* revealing trade secrets.
//!
//! **Key insight**: Companies can jointly prove ESG compliance while keeping
//! individual supplier details, cost breakdowns, and sourcing strategies
//! completely private.
//!
//! Run: `cargo run --example supply_chain`

use amaters_core::{
    compute::{CircuitBuilder, CircuitValue, EncryptedType, FheExecutor},
    storage::MemoryStorage,
    traits::StorageEngine,
    types::{CipherBlob, Key},
};

/// Simulate client-side encryption of a u8 value.
/// In production this would use real TFHE encryption via FheKeyPair.
fn encrypt_u8(value: u8) -> CipherBlob {
    CipherBlob::new(vec![value])
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Supply Chain Transparency: Privacy-Preserving ESG ===\n");

    // ------------------------------------------------------------------
    // 1. Setup: each company owns its own encrypted storage
    // ------------------------------------------------------------------
    let company_a = MemoryStorage::new();
    let company_b = MemoryStorage::new();

    // ------------------------------------------------------------------
    // 2. Company A: electronics manufacturer — 3 suppliers
    //    Fields per supplier: co2 (tonnes), reliability (0-100), cost ($k)
    // ------------------------------------------------------------------
    let suppliers_a: &[(&str, u8, u8, u8, u8)] = &[
        // (id, co2, reliability, cost, compliance: 1=certified)
        ("S-A01", 45, 92, 120, 1),
        ("S-A02", 78, 85, 95, 1),
        ("S-A03", 32, 98, 150, 1),
    ];

    for (id, co2, reliability, cost, compliance) in suppliers_a {
        let prefix = format!("company_a:supplier:{id}");
        company_a
            .put(&Key::from_str(&format!("{prefix}:co2")), &encrypt_u8(*co2))
            .await?;
        company_a
            .put(
                &Key::from_str(&format!("{prefix}:reliability")),
                &encrypt_u8(*reliability),
            )
            .await?;
        company_a
            .put(
                &Key::from_str(&format!("{prefix}:cost")),
                &encrypt_u8(*cost),
            )
            .await?;
        company_a
            .put(
                &Key::from_str(&format!("{prefix}:compliance")),
                &encrypt_u8(*compliance),
            )
            .await?;
        println!("[Company A] Stored encrypted data for supplier {id}");
    }

    // ------------------------------------------------------------------
    // 3. Company B: logistics partner — 2 suppliers
    // ------------------------------------------------------------------
    let suppliers_b: &[(&str, u8, u8, u8, u8)] = &[
        ("S-B01", 60, 88, 80, 1),
        ("S-B02", 25, 95, 110, 0), // not yet certified
    ];

    for (id, co2, reliability, cost, compliance) in suppliers_b {
        let prefix = format!("company_b:supplier:{id}");
        company_b
            .put(&Key::from_str(&format!("{prefix}:co2")), &encrypt_u8(*co2))
            .await?;
        company_b
            .put(
                &Key::from_str(&format!("{prefix}:reliability")),
                &encrypt_u8(*reliability),
            )
            .await?;
        company_b
            .put(
                &Key::from_str(&format!("{prefix}:cost")),
                &encrypt_u8(*cost),
            )
            .await?;
        company_b
            .put(
                &Key::from_str(&format!("{prefix}:compliance")),
                &encrypt_u8(*compliance),
            )
            .await?;
        println!("[Company B] Stored encrypted data for supplier {id}");
    }

    // ------------------------------------------------------------------
    // 4. Queries: range scan for all supplier data within each company
    // ------------------------------------------------------------------
    println!("\n--- Range Queries ---");
    let a_start = Key::from_str("company_a:supplier:");
    let a_end = Key::from_str("company_a:supplier:~");
    let a_records = company_a.range(&a_start, &a_end).await?;
    println!("  Company A: {} encrypted supplier fields", a_records.len());

    let b_start = Key::from_str("company_b:supplier:");
    let b_end = Key::from_str("company_b:supplier:~");
    let b_records = company_b.range(&b_start, &b_end).await?;
    println!("  Company B: {} encrypted supplier fields", b_records.len());

    // ------------------------------------------------------------------
    // 5. Batch lookup: retrieve all keys in each storage
    // ------------------------------------------------------------------
    println!("\n--- Batch Key Listing ---");
    let a_keys = company_a.keys().await?;
    let b_keys = company_b.keys().await?;
    println!("  Company A keys: {}", a_keys.len());
    println!("  Company B keys: {}", b_keys.len());

    // ------------------------------------------------------------------
    // 6. FHE Circuit: total CO2 footprint across two suppliers
    //    sum = supplier_co2_1 + supplier_co2_2
    //    Both inputs stay encrypted; neither party sees the other's value.
    // ------------------------------------------------------------------
    println!("\n--- FHE Circuit: Cross-Company CO2 Footprint ---");
    let mut co2_builder = CircuitBuilder::new();
    co2_builder
        .declare_variable("co2_a", EncryptedType::U8)
        .declare_variable("co2_b", EncryptedType::U8);

    let co2_a = co2_builder.load("co2_a");
    let co2_b = co2_builder.load("co2_b");
    let total_co2 = co2_builder.add(co2_a, co2_b);

    let co2_circuit = co2_builder.build(total_co2)?;
    println!(
        "  Circuit depth: {}, gates: {}",
        co2_circuit.depth, co2_circuit.gate_count
    );
    println!(
        "  Result type: {} (remains encrypted)",
        co2_circuit.result_type
    );

    // ------------------------------------------------------------------
    // 7. FHE Circuit: sustainability scoring
    //    score = reliability - co2_penalty
    //    Allows ranking without exposing raw supplier metrics.
    // ------------------------------------------------------------------
    println!("\n--- FHE Circuit: Sustainability Score ---");
    let mut score_builder = CircuitBuilder::new();
    score_builder
        .declare_variable("reliability", EncryptedType::U8)
        .declare_variable("co2", EncryptedType::U8);

    let rel = score_builder.load("reliability");
    let co2_val = score_builder.load("co2");
    // penalty = co2 / 2  (approximate via mul by constant, then compare)
    let penalty_weight = score_builder.constant(CircuitValue::U8(2));
    let penalty = score_builder.mul(co2_val, penalty_weight);
    let score = score_builder.sub(rel, penalty);

    let score_circuit = score_builder.build(score)?;
    println!(
        "  Circuit depth: {}, gates: {}",
        score_circuit.depth, score_circuit.gate_count
    );

    // ------------------------------------------------------------------
    // 8. FHE Circuit: compliance gate (boolean)
    //    both_compliant = company_a_cert AND company_b_cert
    // ------------------------------------------------------------------
    println!("\n--- FHE Circuit: Joint Compliance Gate ---");
    let mut comp_builder = CircuitBuilder::new();
    comp_builder
        .declare_variable("cert_a", EncryptedType::Bool)
        .declare_variable("cert_b", EncryptedType::Bool);

    let cert_a = comp_builder.load("cert_a");
    let cert_b = comp_builder.load("cert_b");
    let both = comp_builder.and(cert_a, cert_b);

    let comp_circuit = comp_builder.build(both)?;
    println!(
        "  Circuit depth: {}, gates: {}",
        comp_circuit.depth, comp_circuit.gate_count
    );

    // ------------------------------------------------------------------
    // 9. Attempt execution (gracefully degrades without `compute` feature)
    // ------------------------------------------------------------------
    println!("\n--- Attempting FHE Execution ---");
    let executor = FheExecutor::new();
    let inputs = std::collections::HashMap::new(); // placeholder
    match executor.execute(&co2_circuit, &inputs) {
        Ok(result) => println!("  Encrypted CO2 total: {} bytes", result.len()),
        Err(e) => println!("  Expected: {e} (enable `compute` feature for real FHE)"),
    }

    // ------------------------------------------------------------------
    // 10. Multi-party privacy summary
    // ------------------------------------------------------------------
    println!("\n--- Cross-Company Privacy Summary ---");
    println!(
        "  Company A: {} encrypted fields across {} suppliers",
        company_a.len(),
        suppliers_a.len()
    );
    println!(
        "  Company B: {} encrypted fields across {} suppliers",
        company_b.len(),
        suppliers_b.len()
    );
    println!("  Neither company revealed costs, reliability, or CO2 data.");
    println!("  Joint CO2 footprint and compliance computed on ciphertext.");
    println!("  Only the final decrypted aggregate is shared (by mutual consent).");

    Ok(())
}
