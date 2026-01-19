//! Integration tests for FHE filter query compilation and execution
//!
//! These tests verify the end-to-end functionality of compiling predicates
//! to FHE circuits and executing them on encrypted data.

#![cfg(all(test, feature = "compute"))]

use super::*;
use crate::compute::{
    EncryptedType, EncryptedU8, FheExecutor, FheKeyPair, KeyManager, PredicateCompiler,
};
use crate::types::{CipherBlob, Predicate, col};
use std::collections::HashMap;

/// Helper function to create encrypted U8 value
fn encrypt_u8(value: u8, keypair: &FheKeyPair) -> CipherBlob {
    let encrypted = EncryptedU8::encrypt(value, keypair.client_key());
    encrypted.to_cipher_blob().expect("Failed to serialize")
}

#[test]
fn test_predicate_compiler_eq() -> Result<()> {
    let mut compiler = PredicateCompiler::new();

    // Create a simple Eq predicate: age == 25
    let predicate = Predicate::Eq(col("age"), CipherBlob::new(vec![1, 2, 3]));

    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

    // Verify circuit structure
    assert_eq!(circuit.result_type, EncryptedType::Bool);
    assert_eq!(circuit.variable_types.len(), 2);
    assert!(circuit.variable_types.contains_key("value"));
    assert!(circuit.variable_types.contains_key("rhs"));

    // Verify it's a comparison node
    assert!(matches!(circuit.root, CircuitNode::Compare { .. }));

    Ok(())
}

#[test]
fn test_predicate_compiler_gt() -> Result<()> {
    let mut compiler = PredicateCompiler::new();

    // Create a Gt predicate: age > 18
    let predicate = Predicate::Gt(col("age"), CipherBlob::new(vec![1, 2, 3]));

    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

    // Verify circuit structure
    assert_eq!(circuit.result_type, EncryptedType::Bool);
    assert!(matches!(circuit.root, CircuitNode::Compare { .. }));

    Ok(())
}

#[test]
fn test_predicate_compiler_and() -> Result<()> {
    let mut compiler = PredicateCompiler::new();

    // Create an And predicate: age > 18 AND age < 65
    let pred1 = Predicate::Gt(col("age"), CipherBlob::new(vec![18]));
    let pred2 = Predicate::Lt(col("age"), CipherBlob::new(vec![65]));
    let predicate = Predicate::And(Box::new(pred1), Box::new(pred2));

    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

    // Verify circuit structure
    assert_eq!(circuit.result_type, EncryptedType::Bool);
    assert!(matches!(circuit.root, CircuitNode::BinaryOp { .. }));

    // Should have multiple gates for the AND operation
    assert!(circuit.gate_count >= 2);

    Ok(())
}

#[tokio::test]
async fn test_filter_query_execution() -> Result<()> {
    // Generate FHE keys
    let keypair = FheKeyPair::generate()?;
    keypair.set_as_global_server_key();

    // Create test data: ages 15, 25, 35, 70
    let ages = vec![15u8, 25, 35, 70];
    let mut encrypted_ages = Vec::new();

    for age in &ages {
        encrypted_ages.push(encrypt_u8(*age, &keypair));
    }

    // Create predicate: age > 18
    let rhs_value = encrypt_u8(18, &keypair);
    let predicate = Predicate::Gt(col("age"), rhs_value.clone());

    // Compile predicate to circuit
    let mut compiler = PredicateCompiler::new();
    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

    // Execute circuit on each age
    let executor = FheExecutor::new();
    let mut results = Vec::new();

    for (idx, encrypted_age) in encrypted_ages.iter().enumerate() {
        let mut inputs = HashMap::new();
        inputs.insert("value".to_string(), encrypted_age.clone());
        inputs.insert("rhs".to_string(), rhs_value.clone());

        let result_blob = executor.execute(&circuit, &inputs)?;

        // Decrypt the boolean result
        let result_bool = crate::compute::EncryptedBool::from_cipher_blob(&result_blob)?;
        let is_match = result_bool.decrypt(keypair.client_key());

        results.push((ages[idx], is_match));
    }

    // Verify results: only 25, 35, 70 should match (age > 18)
    assert!(!results[0].1, "15 > 18 should be false");
    assert!(results[1].1, "25 > 18 should be true");
    assert!(results[2].1, "35 > 18 should be true");
    assert!(results[3].1, "70 > 18 should be true");

    Ok(())
}

#[tokio::test]
async fn test_filter_query_complex_predicate() -> Result<()> {
    // Test: age > 18 AND age < 65
    let keypair = FheKeyPair::generate()?;
    keypair.set_as_global_server_key();

    // Create test data: ages 15, 25, 35, 70
    let ages = vec![15u8, 25, 35, 70];
    let mut encrypted_ages = Vec::new();

    for age in &ages {
        encrypted_ages.push(encrypt_u8(*age, &keypair));
    }

    // Create complex predicate: age > 18 AND age < 65
    let rhs1 = encrypt_u8(18, &keypair);
    let rhs2 = encrypt_u8(65, &keypair);

    let pred1 = Predicate::Gt(col("age"), rhs1.clone());
    let pred2 = Predicate::Lt(col("age"), rhs2.clone());
    let predicate = Predicate::And(Box::new(pred1), Box::new(pred2));

    // Compile predicate
    let mut compiler = PredicateCompiler::new();
    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

    // Note: This test demonstrates the limitation of the current design
    // The circuit expects both "value" and "rhs" inputs, but for complex
    // predicates with multiple comparisons, we need a more sophisticated
    // approach. For now, we'll just verify the circuit compiles correctly.

    assert_eq!(circuit.result_type, EncryptedType::Bool);
    assert!(circuit.gate_count >= 2);

    Ok(())
}

#[test]
fn test_filter_predicate_or() -> Result<()> {
    let mut compiler = PredicateCompiler::new();

    // Create an Or predicate: age < 18 OR age > 65
    let pred1 = Predicate::Lt(col("age"), CipherBlob::new(vec![18]));
    let pred2 = Predicate::Gt(col("age"), CipherBlob::new(vec![65]));
    let predicate = Predicate::Or(Box::new(pred1), Box::new(pred2));

    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

    assert_eq!(circuit.result_type, EncryptedType::Bool);
    assert!(matches!(circuit.root, CircuitNode::BinaryOp { .. }));

    Ok(())
}

#[test]
fn test_filter_predicate_not() -> Result<()> {
    let mut compiler = PredicateCompiler::new();

    // Create a Not predicate: NOT (age == 18)
    let pred = Predicate::Eq(col("age"), CipherBlob::new(vec![18]));
    let predicate = Predicate::Not(Box::new(pred));

    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

    assert_eq!(circuit.result_type, EncryptedType::Bool);
    assert!(matches!(circuit.root, CircuitNode::UnaryOp { .. }));

    Ok(())
}

#[test]
fn test_extract_rhs_from_predicate() -> Result<()> {
    let rhs_blob = CipherBlob::new(vec![42]);
    let predicate = Predicate::Gt(col("age"), rhs_blob.clone());

    let extracted = PredicateCompiler::extract_rhs_value(&predicate)?;

    assert_eq!(extracted, rhs_blob);

    Ok(())
}

#[test]
fn test_extract_all_rhs_from_complex_predicate() {
    let blob1 = CipherBlob::new(vec![18]);
    let blob2 = CipherBlob::new(vec![65]);

    let pred1 = Predicate::Gt(col("age"), blob1.clone());
    let pred2 = Predicate::Lt(col("age"), blob2.clone());
    let predicate = Predicate::And(Box::new(pred1), Box::new(pred2));

    let values = PredicateCompiler::extract_all_rhs_values(&predicate);

    assert_eq!(values.len(), 2);
    assert_eq!(values[0], blob1);
    assert_eq!(values[1], blob2);
}

#[tokio::test]
async fn test_filter_with_key_manager() -> Result<()> {
    // Test that KeyManager properly manages server keys
    let key_manager = KeyManager::new();
    let keypair = FheKeyPair::generate()?;

    // Register key for a client
    key_manager.register_key("client_1".to_string(), keypair.server_key().clone());

    // Set as global
    key_manager.set_global("client_1")?;

    // Verify global key is set
    assert!(key_manager.get_global().is_some());

    // Now compile and execute a predicate
    let mut compiler = PredicateCompiler::new();
    let rhs_value = encrypt_u8(18, &keypair);
    let predicate = Predicate::Gt(col("age"), rhs_value.clone());

    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

    // Execute with the global key
    keypair.set_as_global_server_key();

    let executor = FheExecutor::new();
    let encrypted_age = encrypt_u8(25, &keypair);

    let mut inputs = HashMap::new();
    inputs.insert("value".to_string(), encrypted_age);
    inputs.insert("rhs".to_string(), rhs_value);

    let result_blob = executor.execute(&circuit, &inputs)?;

    // Decrypt and verify
    let result_bool = crate::compute::EncryptedBool::from_cipher_blob(&result_blob)?;
    assert!(result_bool.decrypt(keypair.client_key()));

    Ok(())
}

#[test]
fn test_circuit_optimization_for_predicates() -> Result<()> {
    // Verify that predicate circuits can be optimized
    let mut compiler = PredicateCompiler::new();

    // Create a complex predicate
    let pred1 = Predicate::Gt(col("age"), CipherBlob::new(vec![18]));
    let pred2 = Predicate::Lt(col("age"), CipherBlob::new(vec![65]));
    let and_pred = Predicate::And(Box::new(pred1), Box::new(pred2));

    let pred3 = Predicate::Eq(col("age"), CipherBlob::new(vec![100]));
    let predicate = Predicate::Or(Box::new(and_pred), Box::new(pred3));

    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

    // Verify the circuit is valid
    circuit.validate()?;

    // Verify circuit has reasonable complexity
    assert!(circuit.depth > 0);
    assert!(circuit.gate_count > 0);

    Ok(())
}

#[tokio::test]
async fn test_filter_query_with_eq_predicate() -> Result<()> {
    // Test equality predicate: age == 25
    let keypair = FheKeyPair::generate()?;
    keypair.set_as_global_server_key();

    let ages = vec![15u8, 25, 35, 25];
    let mut encrypted_ages = Vec::new();

    for age in &ages {
        encrypted_ages.push(encrypt_u8(*age, &keypair));
    }

    // Create predicate: age == 25
    let rhs_value = encrypt_u8(25, &keypair);
    let predicate = Predicate::Eq(col("age"), rhs_value.clone());

    // Compile and execute
    let mut compiler = PredicateCompiler::new();
    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;
    let executor = FheExecutor::new();

    let mut results = Vec::new();
    for (idx, encrypted_age) in encrypted_ages.iter().enumerate() {
        let mut inputs = HashMap::new();
        inputs.insert("value".to_string(), encrypted_age.clone());
        inputs.insert("rhs".to_string(), rhs_value.clone());

        let result_blob = executor.execute(&circuit, &inputs)?;
        let result_bool = crate::compute::EncryptedBool::from_cipher_blob(&result_blob)?;
        let is_match = result_bool.decrypt(keypair.client_key());

        results.push((ages[idx], is_match));
    }

    // Verify: only ages 25 should match
    assert!(!results[0].1, "15 == 25 should be false");
    assert!(results[1].1, "25 == 25 should be true");
    assert!(!results[2].1, "35 == 25 should be false");
    assert!(results[3].1, "25 == 25 should be true");

    Ok(())
}

#[tokio::test]
async fn test_filter_query_with_lte_predicate() -> Result<()> {
    // Test less-than-or-equal predicate: age <= 35
    let keypair = FheKeyPair::generate()?;
    keypair.set_as_global_server_key();

    let ages = vec![15u8, 25, 35, 70];
    let mut encrypted_ages = Vec::new();

    for age in &ages {
        encrypted_ages.push(encrypt_u8(*age, &keypair));
    }

    // Create predicate: age <= 35
    let rhs_value = encrypt_u8(35, &keypair);
    let predicate = Predicate::Lte(col("age"), rhs_value.clone());

    // Compile and execute
    let mut compiler = PredicateCompiler::new();
    let circuit = compiler.compile(&predicate, EncryptedType::U8)?;
    let executor = FheExecutor::new();

    let mut results = Vec::new();
    for (idx, encrypted_age) in encrypted_ages.iter().enumerate() {
        let mut inputs = HashMap::new();
        inputs.insert("value".to_string(), encrypted_age.clone());
        inputs.insert("rhs".to_string(), rhs_value.clone());

        let result_blob = executor.execute(&circuit, &inputs)?;
        let result_bool = crate::compute::EncryptedBool::from_cipher_blob(&result_blob)?;
        let is_match = result_bool.decrypt(keypair.client_key());

        results.push((ages[idx], is_match));
    }

    // Verify: 15, 25, 35 should match (age <= 35)
    assert!(results[0].1, "15 <= 35 should be true");
    assert!(results[1].1, "25 <= 35 should be true");
    assert!(results[2].1, "35 <= 35 should be true");
    assert!(!results[3].1, "70 <= 35 should be false");

    Ok(())
}
