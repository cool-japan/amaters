//! Healthcare & Genomics Use Case
//!
//! Demonstrates how AmateRS stores encrypted patient data (demographics,
//! genetic markers, medical history) and builds FHE circuits for
//! privacy-preserving cancer risk prediction and drug interaction checks.
//!
//! **Key insight**: The server never sees plaintext patient data. Hospitals
//! and labs can combine encrypted datasets without revealing raw records.
//!
//! Run: `cargo run --example healthcare_genomics`

use amaters_core::{
    compute::{CircuitBuilder, CircuitValue, EncryptedType, FheExecutor},
    storage::MemoryStorage,
    traits::StorageEngine,
    types::{CipherBlob, Key},
};

// ---------------------------------------------------------------------------
// Helper: simulate client-side encryption of a u8 value into a CipherBlob.
// In production this would use real TFHE encryption via FheKeyPair.
// ---------------------------------------------------------------------------
fn encrypt_u8(value: u8) -> CipherBlob {
    CipherBlob::new(vec![value])
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Healthcare & Genomics: Privacy-Preserving Analysis ===\n");

    // ------------------------------------------------------------------
    // 1. Setup: one MemoryStorage per data source (hospital vs. genetics lab)
    // ------------------------------------------------------------------
    let hospital_store = MemoryStorage::new();
    let lab_store = MemoryStorage::new();

    // ------------------------------------------------------------------
    // 2. Hospital stores encrypted patient demographics & medical history
    // ------------------------------------------------------------------
    let patients = [
        ("P-1001", 45_u8, 1_u8, 1_u8),   // age, sex(1=F), prior_cancer(1=yes)
        ("P-1002", 62_u8, 0_u8, 0_u8),   // age, sex(0=M), prior_cancer(0=no)
        ("P-1003", 38_u8, 1_u8, 0_u8),
    ];

    for (id, age, sex, prior_cancer) in &patients {
        let age_key = Key::from_str(&format!("patient:{id}:demographics:age"));
        let sex_key = Key::from_str(&format!("patient:{id}:demographics:sex"));
        let history_key = Key::from_str(&format!("patient:{id}:history:prior_cancer"));
        let allergy_key = Key::from_str(&format!("patient:{id}:history:drug_allergy"));

        hospital_store.put(&age_key, &encrypt_u8(*age)).await?;
        hospital_store.put(&sex_key, &encrypt_u8(*sex)).await?;
        hospital_store.put(&history_key, &encrypt_u8(*prior_cancer)).await?;
        // Drug allergy flag (0 = none known)
        hospital_store.put(&allergy_key, &encrypt_u8(0)).await?;

        println!("[Hospital] Stored encrypted record for patient {id}");
    }

    // ------------------------------------------------------------------
    // 3. Genetics lab stores encrypted genetic markers (BRCA1/BRCA2)
    //    Variant risk encoded as 0=wild-type, 1=heterozygous, 2=homozygous
    // ------------------------------------------------------------------
    let genetic_data: &[(&str, u8, u8)] = &[
        ("P-1001", 1, 0), // BRCA1 heterozygous, BRCA2 wild-type
        ("P-1002", 0, 0), // both wild-type
        ("P-1003", 2, 1), // BRCA1 homozygous, BRCA2 heterozygous
    ];

    for (id, brca1, brca2) in genetic_data {
        let brca1_key = Key::from_str(&format!("patient:{id}:genetics:brca1"));
        let brca2_key = Key::from_str(&format!("patient:{id}:genetics:brca2"));

        lab_store.put(&brca1_key, &encrypt_u8(*brca1)).await?;
        lab_store.put(&brca2_key, &encrypt_u8(*brca2)).await?;

        println!("[GeneticsLab] Stored encrypted markers for patient {id}");
    }

    // ------------------------------------------------------------------
    // 4. Queries: range scan to retrieve all data for a patient
    // ------------------------------------------------------------------
    println!("\n--- Range Query: all records for patient P-1001 ---");
    let start = Key::from_str("patient:P-1001:");
    let end = Key::from_str("patient:P-1001:~");
    let records = hospital_store.range(&start, &end).await?;
    println!(
        "  Found {} encrypted hospital records (plaintext never exposed)",
        records.len()
    );

    let lab_records = lab_store.range(&start, &end).await?;
    println!(
        "  Found {} encrypted genetics records from lab",
        lab_records.len()
    );

    // ------------------------------------------------------------------
    // 5. Compute: build FHE circuits (works without `compute` feature
    //    for circuit *definition*; execution needs the feature flag)
    // ------------------------------------------------------------------
    println!("\n--- FHE Circuit: Cancer Risk Score ---");
    // Risk model (simplified): risk = brca1_variant * 30 + brca2_variant * 20 + age
    // All arithmetic stays encrypted; the server learns nothing.
    let mut risk_builder = CircuitBuilder::new();
    risk_builder
        .declare_variable("brca1", EncryptedType::U8)
        .declare_variable("brca2", EncryptedType::U8)
        .declare_variable("age", EncryptedType::U8);

    let brca1 = risk_builder.load("brca1");
    let w1 = risk_builder.constant(CircuitValue::U8(30));
    let brca1_score = risk_builder.mul(brca1, w1);

    let brca2 = risk_builder.load("brca2");
    let w2 = risk_builder.constant(CircuitValue::U8(20));
    let brca2_score = risk_builder.mul(brca2, w2);

    let genetic_score = risk_builder.add(brca1_score, brca2_score);
    let age = risk_builder.load("age");
    let total_risk = risk_builder.add(genetic_score, age);

    let risk_circuit = risk_builder.build(total_risk)?;
    println!(
        "  Circuit depth: {}, gates: {}",
        risk_circuit.depth, risk_circuit.gate_count
    );
    println!("  Result type: {} (encrypted)", risk_circuit.result_type);

    // ------------------------------------------------------------------
    // 6. Compute: drug interaction circuit (boolean logic)
    // ------------------------------------------------------------------
    println!("\n--- FHE Circuit: Drug Interaction Check ---");
    // has_interaction = has_allergy AND brca1_positive
    let mut drug_builder = CircuitBuilder::new();
    drug_builder
        .declare_variable("has_allergy", EncryptedType::Bool)
        .declare_variable("brca1_positive", EncryptedType::Bool);

    let allergy = drug_builder.load("has_allergy");
    let brca1_pos = drug_builder.load("brca1_positive");
    let interaction = drug_builder.and(allergy, brca1_pos);

    let drug_circuit = drug_builder.build(interaction)?;
    println!(
        "  Circuit depth: {}, gates: {}",
        drug_circuit.depth, drug_circuit.gate_count
    );

    // ------------------------------------------------------------------
    // 7. Attempt execution (gracefully degrades without `compute` feature)
    // ------------------------------------------------------------------
    println!("\n--- Attempting FHE Execution ---");
    let executor = FheExecutor::new();
    let inputs = std::collections::HashMap::new(); // placeholder
    match executor.execute(&risk_circuit, &inputs) {
        Ok(result) => println!("  Encrypted result: {} bytes", result.len()),
        Err(e) => println!("  Expected: {e} (enable `compute` feature for real FHE)"),
    }

    // ------------------------------------------------------------------
    // 8. Multi-party summary
    // ------------------------------------------------------------------
    println!("\n--- Multi-Party Privacy Summary ---");
    println!(
        "  Hospital records: {} entries (encrypted)",
        hospital_store.len()
    );
    println!("  Lab records: {} entries (encrypted)", lab_store.len());
    println!("  Neither party revealed plaintext to the other.");
    println!("  Circuits compute risk scores on encrypted data end-to-end.");

    Ok(())
}
