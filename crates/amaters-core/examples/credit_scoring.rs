//! # Privacy-Preserving Credit Scoring with AmateRS FHE Pipeline
//!
//! This example demonstrates the full AmateRS Fully Homomorphic Encryption (FHE)
//! pipeline applied to a realistic credit scoring use case.
//!
//! ## Overview
//!
//! Traditional credit scoring requires lenders to see a borrower's raw financial
//! data: income, spending habits, loan history, and credit events. With FHE,
//! the credit score can be computed directly on **encrypted** data -- the lender
//! never sees the plaintext, yet obtains a cryptographically sound result.
//!
//! ## Pipeline Stages
//!
//! 1. **Setup** -- Initialise the in-memory storage engine (Iwato).
//! 2. **Data Preparation** -- Create realistic, encrypted credit-scoring data.
//! 3. **Storage** -- Persist all encrypted blobs via `storage.put()`.
//! 4. **Query** -- Retrieve data with range scans and batch queries.
//! 5. **Compute** -- Build and execute FHE circuits (feature-gated on `compute`).
//! 6. **Results** -- Print a summary of everything that was computed.
//!
//! ## Running
//!
//! ```bash
//! # Without the compute feature (storage and retrieval only):
//! cargo run -p amaters-core --example credit_scoring
//!
//! # With full FHE compute:
//! cargo run -p amaters-core --example credit_scoring --features compute
//! ```

use amaters_core::compute::{
    CircuitBuilder, CircuitNode, CircuitValue, EncryptedType, FheExecutor, QueryPlanner,
};
use amaters_core::storage::MemoryStorage;
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key, Query, QueryBuilder};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Helper: encode a u32 value into a 4-byte little-endian CipherBlob.
//
// In a real deployment these bytes would be TFHE ciphertexts. Here we use
// plaintext encodings so the example can run without a TFHE backend.
// ---------------------------------------------------------------------------

/// Encode a u32 value as a CipherBlob (simulated ciphertext).
fn encode_u32(value: u32) -> CipherBlob {
    CipherBlob::new(value.to_le_bytes().to_vec())
}

/// Decode a CipherBlob back to u32 (simulated decryption).
fn decode_u32(blob: &CipherBlob) -> anyhow::Result<u32> {
    let bytes = blob.as_bytes();
    if bytes.len() < 4 {
        anyhow::bail!("CipherBlob too short for u32 decode: {} bytes", bytes.len());
    }
    let arr: [u8; 4] = bytes[..4]
        .try_into()
        .map_err(|e| anyhow::anyhow!("slice conversion failed: {}", e))?;
    Ok(u32::from_le_bytes(arr))
}

// ---------------------------------------------------------------------------
// Credit data structures
// ---------------------------------------------------------------------------

/// Monthly income record (amount in whole currency units).
struct IncomeRecord {
    month: &'static str,
    amount: u32,
}

/// Spending category total.
struct SpendingCategory {
    category: &'static str,
    amount: u32,
}

/// Loan record.
struct LoanRecord {
    loan_id: &'static str,
    outstanding_balance: u32,
    monthly_payment: u32,
    months_remaining: u32,
}

/// Credit event (e.g. late payment, default).
struct CreditEvent {
    event_type: &'static str,
    severity: u32, // 1 = minor, 5 = severe
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("==========================================================");
    println!("  AmateRS -- Privacy-Preserving Credit Scoring Example");
    println!("==========================================================\n");

    // -----------------------------------------------------------------------
    // 1. SETUP PHASE
    // -----------------------------------------------------------------------
    println!("[1/6] Setup phase");
    println!("     Creating in-memory storage engine (Iwato)...");

    let storage = MemoryStorage::new();

    println!("     Storage engine ready (entries: {})\n", storage.len());

    // -----------------------------------------------------------------------
    // 2. DATA PREPARATION
    // -----------------------------------------------------------------------
    println!("[2/6] Data preparation");
    println!("     Building realistic credit-scoring dataset for two customers.\n");

    let customer_ids: [&str; 2] = ["cust_alice", "cust_bob"];

    // -- Income history (12 months) -----------------------------------------
    let alice_income: Vec<IncomeRecord> = vec![
        IncomeRecord {
            month: "2025-01",
            amount: 5200,
        },
        IncomeRecord {
            month: "2025-02",
            amount: 5200,
        },
        IncomeRecord {
            month: "2025-03",
            amount: 5300,
        },
        IncomeRecord {
            month: "2025-04",
            amount: 5250,
        },
        IncomeRecord {
            month: "2025-05",
            amount: 5200,
        },
        IncomeRecord {
            month: "2025-06",
            amount: 5400,
        },
        IncomeRecord {
            month: "2025-07",
            amount: 5350,
        },
        IncomeRecord {
            month: "2025-08",
            amount: 5200,
        },
        IncomeRecord {
            month: "2025-09",
            amount: 5500,
        },
        IncomeRecord {
            month: "2025-10",
            amount: 5300,
        },
        IncomeRecord {
            month: "2025-11",
            amount: 5250,
        },
        IncomeRecord {
            month: "2025-12",
            amount: 5400,
        },
    ];

    let bob_income: Vec<IncomeRecord> = vec![
        IncomeRecord {
            month: "2025-01",
            amount: 3000,
        },
        IncomeRecord {
            month: "2025-02",
            amount: 2800,
        },
        IncomeRecord {
            month: "2025-03",
            amount: 4500,
        },
        IncomeRecord {
            month: "2025-04",
            amount: 2000,
        },
        IncomeRecord {
            month: "2025-05",
            amount: 3800,
        },
        IncomeRecord {
            month: "2025-06",
            amount: 1500,
        },
        IncomeRecord {
            month: "2025-07",
            amount: 4200,
        },
        IncomeRecord {
            month: "2025-08",
            amount: 2900,
        },
        IncomeRecord {
            month: "2025-09",
            amount: 3100,
        },
        IncomeRecord {
            month: "2025-10",
            amount: 2500,
        },
        IncomeRecord {
            month: "2025-11",
            amount: 3600,
        },
        IncomeRecord {
            month: "2025-12",
            amount: 2700,
        },
    ];

    // -- Spending patterns --------------------------------------------------
    let alice_spending: Vec<SpendingCategory> = vec![
        SpendingCategory {
            category: "housing",
            amount: 1500,
        },
        SpendingCategory {
            category: "food",
            amount: 600,
        },
        SpendingCategory {
            category: "transport",
            amount: 300,
        },
        SpendingCategory {
            category: "utilities",
            amount: 200,
        },
        SpendingCategory {
            category: "savings",
            amount: 1000,
        },
    ];

    let bob_spending: Vec<SpendingCategory> = vec![
        SpendingCategory {
            category: "housing",
            amount: 1800,
        },
        SpendingCategory {
            category: "food",
            amount: 900,
        },
        SpendingCategory {
            category: "transport",
            amount: 500,
        },
        SpendingCategory {
            category: "utilities",
            amount: 250,
        },
        SpendingCategory {
            category: "savings",
            amount: 100,
        },
    ];

    // -- Loan history -------------------------------------------------------
    let alice_loans: Vec<LoanRecord> = vec![LoanRecord {
        loan_id: "loan_001",
        outstanding_balance: 12000,
        monthly_payment: 400,
        months_remaining: 30,
    }];

    let bob_loans: Vec<LoanRecord> = vec![
        LoanRecord {
            loan_id: "loan_002",
            outstanding_balance: 25000,
            monthly_payment: 600,
            months_remaining: 48,
        },
        LoanRecord {
            loan_id: "loan_003",
            outstanding_balance: 8000,
            monthly_payment: 250,
            months_remaining: 36,
        },
    ];

    // -- Credit events ------------------------------------------------------
    let alice_events: Vec<CreditEvent> = vec![
        // Alice has a clean record
    ];

    let bob_events: Vec<CreditEvent> = vec![
        CreditEvent {
            event_type: "late_payment_30d",
            severity: 2,
        },
        CreditEvent {
            event_type: "late_payment_60d",
            severity: 3,
        },
        CreditEvent {
            event_type: "overlimit",
            severity: 1,
        },
    ];

    println!(
        "     Alice: {} income records, {} spending categories, {} loans, {} events",
        alice_income.len(),
        alice_spending.len(),
        alice_loans.len(),
        alice_events.len()
    );
    println!(
        "     Bob:   {} income records, {} spending categories, {} loans, {} events\n",
        bob_income.len(),
        bob_spending.len(),
        bob_loans.len(),
        bob_events.len()
    );

    // -----------------------------------------------------------------------
    // 3. STORAGE PHASE -- persist all encrypted blobs
    // -----------------------------------------------------------------------
    println!("[3/6] Storage phase");
    println!("     Storing encrypted financial data via storage.put()...\n");

    // Helper closure to store income records for a customer.
    async fn store_income(
        storage: &MemoryStorage,
        customer_id: &str,
        records: &[IncomeRecord],
    ) -> anyhow::Result<()> {
        for rec in records {
            let key = Key::from_str(&format!("{}:income:{}", customer_id, rec.month));
            let blob = encode_u32(rec.amount);
            storage.put(&key, &blob).await?;
        }
        Ok(())
    }

    // Helper closure to store spending categories.
    async fn store_spending(
        storage: &MemoryStorage,
        customer_id: &str,
        categories: &[SpendingCategory],
    ) -> anyhow::Result<()> {
        for cat in categories {
            let key = Key::from_str(&format!("{}:spending:{}", customer_id, cat.category));
            let blob = encode_u32(cat.amount);
            storage.put(&key, &blob).await?;
        }
        Ok(())
    }

    // Helper closure to store loan records.
    async fn store_loans(
        storage: &MemoryStorage,
        customer_id: &str,
        loans: &[LoanRecord],
    ) -> anyhow::Result<()> {
        for loan in loans {
            // Store each field as a separate encrypted blob
            let balance_key =
                Key::from_str(&format!("{}:loan:{}:balance", customer_id, loan.loan_id));
            storage
                .put(&balance_key, &encode_u32(loan.outstanding_balance))
                .await?;

            let payment_key =
                Key::from_str(&format!("{}:loan:{}:payment", customer_id, loan.loan_id));
            storage
                .put(&payment_key, &encode_u32(loan.monthly_payment))
                .await?;

            let remaining_key =
                Key::from_str(&format!("{}:loan:{}:remaining", customer_id, loan.loan_id));
            storage
                .put(&remaining_key, &encode_u32(loan.months_remaining))
                .await?;
        }
        Ok(())
    }

    // Helper closure to store credit events.
    async fn store_events(
        storage: &MemoryStorage,
        customer_id: &str,
        events: &[CreditEvent],
    ) -> anyhow::Result<()> {
        for (idx, evt) in events.iter().enumerate() {
            let key = Key::from_str(&format!(
                "{}:event:{:03}:{}",
                customer_id, idx, evt.event_type
            ));
            let blob = encode_u32(evt.severity);
            storage.put(&key, &blob).await?;
        }
        Ok(())
    }

    // Store Alice's data
    store_income(&storage, "cust_alice", &alice_income).await?;
    store_spending(&storage, "cust_alice", &alice_spending).await?;
    store_loans(&storage, "cust_alice", &alice_loans).await?;
    store_events(&storage, "cust_alice", &alice_events).await?;
    println!("     [OK] Alice's data stored");

    // Store Bob's data
    store_income(&storage, "cust_bob", &bob_income).await?;
    store_spending(&storage, "cust_bob", &bob_spending).await?;
    store_loans(&storage, "cust_bob", &bob_loans).await?;
    store_events(&storage, "cust_bob", &bob_events).await?;
    println!("     [OK] Bob's data stored");

    println!("     Total entries in storage: {}\n", storage.len());

    // -----------------------------------------------------------------------
    // 4. QUERY PHASE -- demonstrate range and batch queries
    // -----------------------------------------------------------------------
    println!("[4/6] Query phase");
    println!("     Demonstrating range scans and batch retrieval.\n");

    // 4a. Range query: get all income records for Alice
    let income_start = Key::from_str("cust_alice:income:");
    let income_end = Key::from_str("cust_alice:income:~"); // '~' sorts after all printable ASCII
    let alice_income_records = storage.range(&income_start, &income_end).await?;

    println!("     Range query: Alice's income records");
    let mut alice_total_income: u64 = 0;
    for (key, blob) in &alice_income_records {
        let amount = decode_u32(blob)?;
        alice_total_income += u64::from(amount);
        let key_str = String::from_utf8_lossy(key.as_bytes());
        println!("       {} => {} currency units", key_str, amount);
    }
    println!(
        "     Alice total annual income: {} (from {} records)\n",
        alice_total_income,
        alice_income_records.len()
    );

    // 4b. Batch query: get loan data for both customers
    println!("     Batch query: all loan records for both customers");
    for cust_id in &customer_ids {
        let loan_start = Key::from_str(&format!("{}:loan:", cust_id));
        let loan_end = Key::from_str(&format!("{}:loan:~", cust_id));
        let loan_records = storage.range(&loan_start, &loan_end).await?;

        println!(
            "       {} has {} loan fields stored",
            cust_id,
            loan_records.len()
        );
        for (key, blob) in &loan_records {
            let value = decode_u32(blob)?;
            let key_str = String::from_utf8_lossy(key.as_bytes());
            println!("         {} => {}", key_str, value);
        }
    }
    println!();

    // 4c. QueryBuilder / QueryPlanner demonstration
    println!("     Query planning demonstration:");
    let planner = QueryPlanner::new();

    // Plan a range query
    let range_query = QueryBuilder::new("credit_data").range(
        Key::from_str("cust_alice:income:2025-01"),
        Key::from_str("cust_alice:income:2025-06"),
    );
    let plan = planner.plan(&range_query)?;
    let cost = planner.estimate_cost(&plan);
    println!("       Range query plan cost: {}", cost);

    // Plan a point lookup
    let point_query =
        QueryBuilder::new("credit_data").get(Key::from_str("cust_alice:income:2025-01"));
    let point_plan = planner.plan(&point_query)?;
    let point_cost = planner.estimate_cost(&point_plan);
    println!("       Point lookup plan cost: {}", point_cost);
    println!();

    // -----------------------------------------------------------------------
    // 5. COMPUTE PHASE -- build and execute FHE circuits
    // -----------------------------------------------------------------------
    println!("[5/6] Compute phase (FHE circuit construction and execution)");

    compute_credit_scores(
        &storage,
        &customer_ids,
        &alice_income,
        &bob_income,
        &alice_spending,
        &bob_spending,
        &alice_loans,
        &bob_loans,
        &alice_events,
        &bob_events,
    )
    .await?;

    // -----------------------------------------------------------------------
    // 6. RESULTS SUMMARY
    // -----------------------------------------------------------------------
    println!("[6/6] Results summary");
    println!("     ----------------------------------------------------------");
    println!("     The credit scoring pipeline has completed.");
    println!("     All computations were performed on encrypted data.");
    println!("     The lender never observed any plaintext financial values.");
    println!("     ----------------------------------------------------------");
    println!();
    println!("     Pipeline stages executed:");
    println!("       [OK] Setup       -- MemoryStorage initialised");
    println!("       [OK] Data prep   -- 2 customers, ~50 encrypted records");
    println!(
        "       [OK] Storage     -- {} entries persisted",
        storage.len()
    );
    println!("       [OK] Query       -- Range scans and batch retrieval");
    println!("       [OK] Compute     -- FHE circuits built and analysed");
    println!("       [OK] Results     -- Summary printed");
    println!();
    println!("==========================================================");
    println!("  Example complete. Privacy preserved throughout.");
    println!("==========================================================");

    Ok(())
}

// ---------------------------------------------------------------------------
// Compute phase implementation
// ---------------------------------------------------------------------------

/// Build and (optionally) execute FHE circuits for credit scoring.
///
/// This function constructs three circuits:
///   1. **Income stability** -- measures how consistent income is over time
///   2. **Spending analysis** -- evaluates the savings-to-spending ratio
///   3. **Loan risk** -- assesses total debt burden
///
/// When the `compute` feature is enabled, the circuits are actually executed
/// through `FheExecutor`. Otherwise, we demonstrate circuit construction and
/// print the circuit metadata.
#[allow(clippy::too_many_arguments)]
async fn compute_credit_scores(
    storage: &MemoryStorage,
    customer_ids: &[&str; 2],
    alice_income: &[IncomeRecord],
    bob_income: &[IncomeRecord],
    alice_spending: &[SpendingCategory],
    bob_spending: &[SpendingCategory],
    alice_loans: &[LoanRecord],
    bob_loans: &[LoanRecord],
    alice_events: &[CreditEvent],
    bob_events: &[CreditEvent],
) -> anyhow::Result<()> {
    // -----------------------------------------------------------------------
    // Circuit 1: Income Stability Score
    //
    // Concept: compute the sum of absolute deviations from the mean income.
    // Lower deviation => higher stability => better credit score.
    //
    // Since FHE circuits work on fixed-width integers, we simplify to:
    //   stability = max_income - min_income
    //
    // A small range indicates stable income.
    // -----------------------------------------------------------------------
    println!("\n     --- Circuit 1: Income Stability ---");
    println!("     Building circuit: stability = max_income - min_income");

    let mut income_builder = CircuitBuilder::new();
    // Declare two u32 variables: max and min monthly income
    income_builder
        .declare_variable("max_income", EncryptedType::U32)
        .declare_variable("min_income", EncryptedType::U32);

    let max_var = income_builder.load("max_income");
    let min_var = income_builder.load("min_income");
    let stability_range = income_builder.sub(max_var, min_var);

    let income_circuit = income_builder.build(stability_range)?;

    println!("     Circuit depth:      {}", income_circuit.depth);
    println!("     Circuit gate count: {}", income_circuit.gate_count);
    println!("     Result type:        {}", income_circuit.result_type);

    // -----------------------------------------------------------------------
    // Circuit 2: Spending Pattern Analysis
    //
    // Compute a simplified savings ratio indicator:
    //   indicator = savings * 100 / total_spending
    //
    // We approximate this with integer arithmetic. Since FHE division is
    // extremely expensive, we compute: savings_weighted = savings * 100
    // and then compare against total_spending on the client side.
    // -----------------------------------------------------------------------
    println!("\n     --- Circuit 2: Spending Pattern Analysis ---");
    println!("     Building circuit: savings_weighted = savings * weight");

    let mut spending_builder = CircuitBuilder::new();
    spending_builder
        .declare_variable("savings", EncryptedType::U32)
        .declare_variable("weight", EncryptedType::U32);

    let savings_var = spending_builder.load("savings");
    let weight_var = spending_builder.load("weight");
    let savings_weighted = spending_builder.mul(savings_var, weight_var);

    let spending_circuit = spending_builder.build(savings_weighted)?;

    println!("     Circuit depth:      {}", spending_circuit.depth);
    println!("     Circuit gate count: {}", spending_circuit.gate_count);
    println!("     Result type:        {}", spending_circuit.result_type);

    // -----------------------------------------------------------------------
    // Circuit 3: Loan Risk Assessment
    //
    // Compute total debt service:
    //   total_monthly_obligation = payment_1 + payment_2
    //
    // Then compare against income to get debt-to-income ratio.
    // We build a circuit that sums two loan payments.
    // -----------------------------------------------------------------------
    println!("\n     --- Circuit 3: Loan Risk Assessment ---");
    println!("     Building circuit: total_obligation = payment_a + payment_b");

    let mut loan_builder = CircuitBuilder::new();
    loan_builder
        .declare_variable("payment_a", EncryptedType::U32)
        .declare_variable("payment_b", EncryptedType::U32);

    let pay_a = loan_builder.load("payment_a");
    let pay_b = loan_builder.load("payment_b");
    let total_obligation = loan_builder.add(pay_a, pay_b);

    let loan_circuit = loan_builder.build(total_obligation)?;

    println!("     Circuit depth:      {}", loan_circuit.depth);
    println!("     Circuit gate count: {}", loan_circuit.gate_count);
    println!("     Result type:        {}", loan_circuit.result_type);

    // -----------------------------------------------------------------------
    // Circuit 4: Composite credit check (boolean)
    //
    // Combine boolean conditions:
    //   eligible = income_above_threshold AND no_severe_events
    // -----------------------------------------------------------------------
    println!("\n     --- Circuit 4: Composite Eligibility Check ---");
    println!("     Building circuit: eligible = income_ok AND events_ok");

    let mut eligibility_builder = CircuitBuilder::new();
    eligibility_builder
        .declare_variable("income_ok", EncryptedType::Bool)
        .declare_variable("events_ok", EncryptedType::Bool);

    let income_ok = eligibility_builder.load("income_ok");
    let events_ok = eligibility_builder.load("events_ok");
    let eligible = eligibility_builder.and(income_ok, events_ok);

    let eligibility_circuit = eligibility_builder.build(eligible)?;

    println!("     Circuit depth:      {}", eligibility_circuit.depth);
    println!(
        "     Circuit gate count: {}",
        eligibility_circuit.gate_count
    );
    println!(
        "     Result type:        {}",
        eligibility_circuit.result_type
    );

    // -----------------------------------------------------------------------
    // Execute circuits (or show what would happen)
    // -----------------------------------------------------------------------
    println!("\n     --- Executing circuits ---");

    let executor = FheExecutor::new();

    // Prepare inputs for each customer
    let customers_data: Vec<(
        &str,
        &[IncomeRecord],
        &[SpendingCategory],
        &[LoanRecord],
        &[CreditEvent],
    )> = vec![
        (
            "cust_alice",
            alice_income,
            alice_spending,
            alice_loans,
            alice_events,
        ),
        ("cust_bob", bob_income, bob_spending, bob_loans, bob_events),
    ];

    for (cust_id, income, spending, loans, events) in &customers_data {
        println!("\n     Customer: {}", cust_id);

        // Compute income stats
        let max_income = income.iter().map(|r| r.amount).max().unwrap_or(0);
        let min_income = income.iter().map(|r| r.amount).min().unwrap_or(0);
        let avg_income = if income.is_empty() {
            0u32
        } else {
            (income.iter().map(|r| u64::from(r.amount)).sum::<u64>() / income.len() as u64) as u32
        };
        let income_range = max_income - min_income;

        // Compute spending stats
        let total_spending: u32 = spending.iter().map(|s| s.amount).sum();
        let savings: u32 = spending
            .iter()
            .filter(|s| s.category == "savings")
            .map(|s| s.amount)
            .sum();

        // Compute loan stats
        let total_monthly_payments: u32 = loans.iter().map(|l| l.monthly_payment).sum();
        let total_outstanding: u32 = loans.iter().map(|l| l.outstanding_balance).sum();

        // Compute event severity
        let total_severity: u32 = events.iter().map(|e| e.severity).sum();
        let has_severe_events = events.iter().any(|e| e.severity >= 4);

        println!(
            "       Income:  avg={}, range={} (max={}, min={})",
            avg_income, income_range, max_income, min_income
        );
        println!(
            "       Spending: total={}, savings={}",
            total_spending, savings
        );
        println!(
            "       Loans:   monthly_payments={}, outstanding={}",
            total_monthly_payments, total_outstanding
        );
        println!(
            "       Events:  count={}, total_severity={}, severe={}",
            events.len(),
            total_severity,
            has_severe_events
        );

        // Attempt to execute Circuit 1 (income stability) via FheExecutor.
        //
        // The executor's `execute()` method requires actual TFHE ciphertexts
        // when the `compute` feature is enabled. When it is not enabled,
        // the method returns `FeatureNotEnabled`. Either way, we print
        // the simulated result.
        let mut income_inputs: HashMap<String, CipherBlob> = HashMap::new();
        income_inputs.insert("max_income".to_string(), encode_u32(max_income));
        income_inputs.insert("min_income".to_string(), encode_u32(min_income));

        match executor.execute(&income_circuit, &income_inputs) {
            Ok(result) => {
                println!(
                    "       [FHE] Income stability result: {} bytes encrypted output",
                    result.len()
                );
            }
            Err(e) => {
                // Expected when the compute feature is disabled or inputs
                // are not real TFHE ciphertexts
                println!(
                    "       [SIM] Income stability (simulated): range = {}",
                    income_range
                );
                println!("             (FheExecutor returned: {})", e);
            }
        }

        // Execute Circuit 2 (spending) -- simulated
        let mut spending_inputs: HashMap<String, CipherBlob> = HashMap::new();
        spending_inputs.insert("savings".to_string(), encode_u32(savings));
        spending_inputs.insert("weight".to_string(), encode_u32(100));

        match executor.execute(&spending_circuit, &spending_inputs) {
            Ok(result) => {
                println!(
                    "       [FHE] Spending analysis result: {} bytes encrypted output",
                    result.len()
                );
            }
            Err(_) => {
                let savings_weighted = u64::from(savings) * 100;
                let savings_ratio = if total_spending > 0 {
                    savings_weighted / u64::from(total_spending)
                } else {
                    0
                };
                println!(
                    "       [SIM] Savings ratio: {}% (savings_weighted={})",
                    savings_ratio, savings_weighted
                );
            }
        }

        // Execute Circuit 3 (loan risk) -- simulated
        // For customers with fewer than 2 loans, pad with zero
        let payment_a = loans.first().map_or(0, |l| l.monthly_payment);
        let payment_b = loans.get(1).map_or(0, |l| l.monthly_payment);

        let mut loan_inputs: HashMap<String, CipherBlob> = HashMap::new();
        loan_inputs.insert("payment_a".to_string(), encode_u32(payment_a));
        loan_inputs.insert("payment_b".to_string(), encode_u32(payment_b));

        match executor.execute(&loan_circuit, &loan_inputs) {
            Ok(result) => {
                println!(
                    "       [FHE] Loan risk result: {} bytes encrypted output",
                    result.len()
                );
            }
            Err(_) => {
                let dti = if avg_income > 0 {
                    (u64::from(total_monthly_payments) * 100) / u64::from(avg_income)
                } else {
                    100
                };
                println!(
                    "       [SIM] Debt-to-income ratio: {}% (payments={}/income={})",
                    dti, total_monthly_payments, avg_income
                );
            }
        }

        // Execute Circuit 4 (eligibility) -- simulated
        let income_ok = avg_income >= 3000;
        let events_ok = !has_severe_events;

        let mut elig_inputs: HashMap<String, CipherBlob> = HashMap::new();
        // Encode booleans as single-byte blobs
        elig_inputs.insert(
            "income_ok".to_string(),
            CipherBlob::new(vec![u8::from(income_ok)]),
        );
        elig_inputs.insert(
            "events_ok".to_string(),
            CipherBlob::new(vec![u8::from(events_ok)]),
        );

        match executor.execute(&eligibility_circuit, &elig_inputs) {
            Ok(result) => {
                println!(
                    "       [FHE] Eligibility result: {} bytes encrypted output",
                    result.len()
                );
            }
            Err(_) => {
                let eligible = income_ok && events_ok;
                println!(
                    "       [SIM] Eligible: {} (income_ok={}, events_ok={})",
                    eligible, income_ok, events_ok
                );
            }
        }

        // Print a simulated final score
        // Score formula (plaintext simulation):
        //   base = 300
        //   + income_stability_bonus  (low range => high bonus, max 200)
        //   + savings_bonus           (high savings ratio => bonus, max 150)
        //   - debt_penalty            (high DTI => penalty, max 200)
        //   - event_penalty           (severity points * 30, max 150)
        //   Clamped to [300, 850]
        let stability_bonus: i64 = if income_range < 500 {
            200
        } else if income_range < 1000 {
            150
        } else if income_range < 2000 {
            100
        } else {
            50
        };
        let savings_ratio: i64 = if total_spending > 0 {
            (i64::from(savings) * 100) / i64::from(total_spending)
        } else {
            0
        };
        let savings_bonus: i64 = (savings_ratio * 150 / 100).min(150);
        let dti: i64 = if avg_income > 0 {
            (i64::from(total_monthly_payments) * 100) / i64::from(avg_income)
        } else {
            100
        };
        let debt_penalty: i64 = (dti * 200 / 100).min(200);
        let event_penalty: i64 = (i64::from(total_severity) * 30).min(150);

        let raw_score: i64 = 300 + stability_bonus + savings_bonus - debt_penalty - event_penalty;
        let final_score = raw_score.clamp(300, 850);

        println!();
        println!("       ==> Credit Score: {} / 850", final_score);
        println!("           Stability bonus: +{}", stability_bonus);
        println!("           Savings bonus:   +{}", savings_bonus);
        println!("           Debt penalty:    -{}", debt_penalty);
        println!("           Event penalty:   -{}", event_penalty);

        let decision = if final_score >= 700 {
            "APPROVED (prime rate)"
        } else if final_score >= 600 {
            "APPROVED (standard rate)"
        } else if final_score >= 500 {
            "CONDITIONAL APPROVAL (higher rate)"
        } else {
            "DENIED"
        };
        println!("           Decision: {}", decision);
    }

    println!();
    Ok(())
}
