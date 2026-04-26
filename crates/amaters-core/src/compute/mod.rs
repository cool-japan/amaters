//! Compute engine module (Yata - The Eight-Span Mirror)
//!
//! This module provides FHE circuit execution on encrypted data using TFHE.
//!
//! # Architecture
//!
//! The compute engine consists of four main components:
//!
//! - **Key Management** (`keys`): Client and server key generation, serialization
//! - **FHE Operations** (`operations`): Encrypted boolean and integer operations
//! - **Circuit Compilation** (`circuit`): AST representation and type inference
//! - **FHE Executor** (`FheExecutor`): Circuit execution engine
//!
//! # Example
//!
//! ```rust,ignore
//! use amaters_core::compute::{FheKeyPair, CircuitBuilder, EncryptedType, FheExecutor};
//!
//! // Generate keys
//! let keypair = FheKeyPair::generate()?;
//! keypair.set_as_global_server_key();
//!
//! // Build a circuit: a + b
//! let mut builder = CircuitBuilder::new();
//! builder.declare_variable("a", EncryptedType::U8)
//!        .declare_variable("b", EncryptedType::U8);
//!
//! let a = builder.load("a");
//! let b = builder.load("b");
//! let sum = builder.add(a, b);
//! let circuit = builder.build(sum)?;
//!
//! // Execute circuit
//! let executor = FheExecutor::new();
//! let result = executor.execute(&circuit, &inputs)?;
//! ```

pub mod circuit;
pub mod gpu;
pub mod key_manager;
pub mod keys;
pub mod operations;
pub mod optimizer;
pub mod plan_cache;
pub mod planner;
pub mod predicate;

#[cfg(test)]
mod filter_tests;

// Re-export commonly used types
pub use circuit::{
    BinaryOperator, Circuit, CircuitBuilder, CircuitNode, CircuitValue, CompareOperator,
    ConstantType, EncryptedType, UnaryOperator, count_encrypted_constants,
    count_plaintext_constants, decrypt_constant, encrypt_circuit_constants, encrypt_constant,
    is_encrypted_constant,
};
pub use key_manager::{ClientId, KeyManager};
pub use keys::{FheKeyPair, InMemoryKeyStorage, KeyStorage};
pub use operations::{EncryptedBool, EncryptedU8, EncryptedU16, EncryptedU32, EncryptedU64};
pub use optimizer::{CircuitOptimizer, DependencyGraph, NodeId, OptimizationStats};
pub use planner::{LogicalPlan, PhysicalPlan, PlanCost, PlannerStats, QueryPlanner};
pub use predicate::{PredicateCompiler, compile_predicate};

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::CipherBlob;
use std::collections::HashMap;

/// FHE executor for circuit execution
///
/// This executor takes a compiled circuit and encrypted inputs,
/// executes the circuit on the encrypted data, and returns encrypted results.
#[derive(Debug, Clone)]
pub struct FheExecutor {
    optimizer: CircuitOptimizer,
    optimization_enabled: bool,
}

impl FheExecutor {
    /// Create a new FHE executor with optimization enabled
    pub fn new() -> Self {
        Self {
            optimizer: CircuitOptimizer::new(),
            optimization_enabled: true,
        }
    }

    /// Create a new FHE executor with optimization control
    pub fn with_optimization(enable: bool) -> Self {
        Self {
            optimizer: if enable {
                CircuitOptimizer::new()
            } else {
                CircuitOptimizer::disabled()
            },
            optimization_enabled: enable,
        }
    }

    /// Get the optimization statistics from the last execution
    pub fn optimization_stats(&self) -> &OptimizationStats {
        self.optimizer.stats()
    }

    /// Get the dependency graph from the last execution
    pub fn dependency_graph(&self) -> &DependencyGraph {
        self.optimizer.dependency_graph()
    }

    /// Execute FHE circuit on encrypted data
    ///
    /// # Arguments
    ///
    /// * `circuit` - The compiled circuit to execute
    /// * `inputs` - Map of variable names to encrypted values (CipherBlob)
    ///
    /// # Returns
    ///
    /// The encrypted result as a CipherBlob
    #[cfg(feature = "compute")]
    pub fn execute(
        &self,
        circuit: &Circuit,
        inputs: &HashMap<String, CipherBlob>,
    ) -> Result<CipherBlob> {
        // Validate circuit
        circuit.validate()?;

        // Check that all required inputs are provided
        for var_name in circuit.variable_types.keys() {
            if !inputs.contains_key(var_name) {
                return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Missing input for variable: {}",
                    var_name
                ))));
            }
        }

        // Optimize circuit if enabled
        let optimized = if self.optimization_enabled {
            // Need mutable reference for optimizer
            let mut optimizer = self.optimizer.clone();
            optimizer.optimize(circuit.clone())?
        } else {
            circuit.clone()
        };

        // Execute the circuit
        let result_value = self.execute_node(&optimized.root, inputs, &optimized.variable_types)?;

        // Serialize result to CipherBlob
        match result_value {
            EncryptedValue::Bool(v) => v.to_cipher_blob(),
            EncryptedValue::U8(v) => v.to_cipher_blob(),
            EncryptedValue::U16(v) => v.to_cipher_blob(),
            EncryptedValue::U32(v) => v.to_cipher_blob(),
            EncryptedValue::U64(v) => v.to_cipher_blob(),
        }
    }

    /// Stub implementation when compute feature is disabled
    #[cfg(not(feature = "compute"))]
    pub fn execute(
        &self,
        _circuit: &Circuit,
        _inputs: &HashMap<String, CipherBlob>,
    ) -> Result<CipherBlob> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }

    /// Execute a single circuit node recursively
    #[cfg(feature = "compute")]
    #[allow(clippy::only_used_in_recursion)]
    fn execute_node(
        &self,
        node: &CircuitNode,
        inputs: &HashMap<String, CipherBlob>,
        variable_types: &HashMap<String, EncryptedType>,
    ) -> Result<EncryptedValue> {
        match node {
            CircuitNode::Load(name) => {
                let blob = inputs.get(name).ok_or_else(|| {
                    AmateRSError::FheComputation(ErrorContext::new(format!(
                        "Missing input: {}",
                        name
                    )))
                })?;

                let var_type = variable_types.get(name).ok_or_else(|| {
                    AmateRSError::FheComputation(ErrorContext::new(format!(
                        "Unknown variable type: {}",
                        name
                    )))
                })?;

                match var_type {
                    EncryptedType::Bool => {
                        Ok(EncryptedValue::Bool(EncryptedBool::from_cipher_blob(blob)?))
                    }
                    EncryptedType::U8 => {
                        Ok(EncryptedValue::U8(EncryptedU8::from_cipher_blob(blob)?))
                    }
                    EncryptedType::U16 => {
                        Ok(EncryptedValue::U16(EncryptedU16::from_cipher_blob(blob)?))
                    }
                    EncryptedType::U32 => {
                        Ok(EncryptedValue::U32(EncryptedU32::from_cipher_blob(blob)?))
                    }
                    EncryptedType::U64 => {
                        Ok(EncryptedValue::U64(EncryptedU64::from_cipher_blob(blob)?))
                    }
                }
            }

            CircuitNode::Constant(_value) => {
                // Plaintext constants in FHE context are not directly supported.
                // Use encrypt_circuit_constants() to pre-process the circuit before
                // execution, converting all Constant nodes to EncryptedConstant.
                Err(AmateRSError::FheComputation(ErrorContext::new(
                    "Plaintext constants cannot be used in FHE execution. \
                     Use encrypt_circuit_constants() to encrypt constants before evaluation."
                        .to_string(),
                )))
            }

            CircuitNode::EncryptedConstant {
                data,
                original_type,
            } => {
                // Encrypted constants are already in ciphertext form.
                // Deserialize the CipherBlob from the encrypted data and
                // convert to the appropriate EncryptedValue based on original_type.
                let blob = CipherBlob::new(data.clone());
                match original_type {
                    ConstantType::Boolean => Ok(EncryptedValue::Bool(
                        EncryptedBool::from_cipher_blob(&blob)?,
                    )),
                    ConstantType::Integer => {
                        // Try to deserialize as the most common integer type (U64)
                        // In practice, the caller should ensure the encrypted data
                        // matches the expected type from the circuit context.
                        Ok(EncryptedValue::U64(EncryptedU64::from_cipher_blob(&blob)?))
                    }
                    ConstantType::Float | ConstantType::Bytes => {
                        Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                            "EncryptedConstant of type {} is not directly evaluable in FHE circuits",
                            original_type
                        ))))
                    }
                }
            }

            CircuitNode::BinaryOp { op, left, right } => {
                let left_val = self.execute_node(left, inputs, variable_types)?;
                let right_val = self.execute_node(right, inputs, variable_types)?;

                match (op, left_val, right_val) {
                    // Boolean operations
                    (BinaryOperator::And, EncryptedValue::Bool(l), EncryptedValue::Bool(r)) => {
                        Ok(EncryptedValue::Bool(l.and(&r)))
                    }
                    (BinaryOperator::Or, EncryptedValue::Bool(l), EncryptedValue::Bool(r)) => {
                        Ok(EncryptedValue::Bool(l.or(&r)))
                    }
                    (BinaryOperator::Xor, EncryptedValue::Bool(l), EncryptedValue::Bool(r)) => {
                        Ok(EncryptedValue::Bool(l.xor(&r)))
                    }

                    // U8 arithmetic
                    (BinaryOperator::Add, EncryptedValue::U8(l), EncryptedValue::U8(r)) => {
                        Ok(EncryptedValue::U8(l.add(&r)))
                    }
                    (BinaryOperator::Sub, EncryptedValue::U8(l), EncryptedValue::U8(r)) => {
                        Ok(EncryptedValue::U8(l.sub(&r)))
                    }
                    (BinaryOperator::Mul, EncryptedValue::U8(l), EncryptedValue::U8(r)) => {
                        Ok(EncryptedValue::U8(l.mul(&r)))
                    }

                    // U16 arithmetic
                    (BinaryOperator::Add, EncryptedValue::U16(l), EncryptedValue::U16(r)) => {
                        Ok(EncryptedValue::U16(l.add(&r)))
                    }
                    (BinaryOperator::Sub, EncryptedValue::U16(l), EncryptedValue::U16(r)) => {
                        Ok(EncryptedValue::U16(l.sub(&r)))
                    }
                    (BinaryOperator::Mul, EncryptedValue::U16(l), EncryptedValue::U16(r)) => {
                        Ok(EncryptedValue::U16(l.mul(&r)))
                    }

                    // U32 arithmetic
                    (BinaryOperator::Add, EncryptedValue::U32(l), EncryptedValue::U32(r)) => {
                        Ok(EncryptedValue::U32(l.add(&r)))
                    }
                    (BinaryOperator::Sub, EncryptedValue::U32(l), EncryptedValue::U32(r)) => {
                        Ok(EncryptedValue::U32(l.sub(&r)))
                    }
                    (BinaryOperator::Mul, EncryptedValue::U32(l), EncryptedValue::U32(r)) => {
                        Ok(EncryptedValue::U32(l.mul(&r)))
                    }

                    // U64 arithmetic
                    (BinaryOperator::Add, EncryptedValue::U64(l), EncryptedValue::U64(r)) => {
                        Ok(EncryptedValue::U64(l.add(&r)))
                    }
                    (BinaryOperator::Sub, EncryptedValue::U64(l), EncryptedValue::U64(r)) => {
                        Ok(EncryptedValue::U64(l.sub(&r)))
                    }
                    (BinaryOperator::Mul, EncryptedValue::U64(l), EncryptedValue::U64(r)) => {
                        Ok(EncryptedValue::U64(l.mul(&r)))
                    }

                    _ => Err(AmateRSError::FheComputation(ErrorContext::new(
                        "Type mismatch in binary operation".to_string(),
                    ))),
                }
            }

            CircuitNode::UnaryOp { op, operand } => {
                let operand_val = self.execute_node(operand, inputs, variable_types)?;

                match (op, operand_val) {
                    (UnaryOperator::Not, EncryptedValue::Bool(v)) => {
                        Ok(EncryptedValue::Bool(v.not()))
                    }

                    _ => Err(AmateRSError::FheComputation(ErrorContext::new(
                        "Type mismatch in unary operation".to_string(),
                    ))),
                }
            }

            CircuitNode::Compare { op, left, right } => {
                let left_val = self.execute_node(left, inputs, variable_types)?;
                let right_val = self.execute_node(right, inputs, variable_types)?;

                match (left_val, right_val) {
                    (EncryptedValue::U8(l), EncryptedValue::U8(r)) => {
                        let result = match op {
                            CompareOperator::Eq => l.eq(&r),
                            CompareOperator::Ne => l.ne(&r),
                            CompareOperator::Lt => l.lt(&r),
                            CompareOperator::Le => l.le(&r),
                            CompareOperator::Gt => l.gt(&r),
                            CompareOperator::Ge => l.ge(&r),
                        };
                        Ok(EncryptedValue::Bool(result))
                    }

                    (EncryptedValue::U16(l), EncryptedValue::U16(r)) => {
                        let result = match op {
                            CompareOperator::Eq => l.eq(&r),
                            CompareOperator::Ne => l.ne(&r),
                            CompareOperator::Lt => l.lt(&r),
                            CompareOperator::Le => l.le(&r),
                            CompareOperator::Gt => l.gt(&r),
                            CompareOperator::Ge => l.ge(&r),
                        };
                        Ok(EncryptedValue::Bool(result))
                    }

                    (EncryptedValue::U32(l), EncryptedValue::U32(r)) => {
                        let result = match op {
                            CompareOperator::Eq => l.eq(&r),
                            CompareOperator::Ne => l.ne(&r),
                            CompareOperator::Lt => l.lt(&r),
                            CompareOperator::Le => l.le(&r),
                            CompareOperator::Gt => l.gt(&r),
                            CompareOperator::Ge => l.ge(&r),
                        };
                        Ok(EncryptedValue::Bool(result))
                    }

                    (EncryptedValue::U64(l), EncryptedValue::U64(r)) => {
                        let result = match op {
                            CompareOperator::Eq => l.eq(&r),
                            CompareOperator::Ne => l.ne(&r),
                            CompareOperator::Lt => l.lt(&r),
                            CompareOperator::Le => l.le(&r),
                            CompareOperator::Gt => l.gt(&r),
                            CompareOperator::Ge => l.ge(&r),
                        };
                        Ok(EncryptedValue::Bool(result))
                    }

                    _ => Err(AmateRSError::FheComputation(ErrorContext::new(
                        "Type mismatch in comparison".to_string(),
                    ))),
                }
            }
        }
    }
}

impl Default for FheExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal enum for holding encrypted values during execution
#[cfg(feature = "compute")]
enum EncryptedValue {
    Bool(EncryptedBool),
    U8(EncryptedU8),
    U16(EncryptedU16),
    U32(EncryptedU32),
    U64(EncryptedU64),
}

// Legacy types for backward compatibility (to be removed in future versions)

/// Circuit gate (legacy - use CircuitNode instead)
#[deprecated(since = "0.1.0", note = "Use CircuitNode instead")]
#[derive(Debug, Clone)]
pub enum Gate {
    Add,
    Mul,
    Not,
    Bootstrap,
}

#[cfg(all(test, feature = "compute"))]
mod tests {
    use super::*;

    #[test]
    fn test_fhe_executor_basic() -> Result<()> {
        // Generate keys
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        // Build circuit: a + b
        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("a", EncryptedType::U8)
            .declare_variable("b", EncryptedType::U8);

        let a_node = builder.load("a");
        let b_node = builder.load("b");
        let sum_node = builder.add(a_node, b_node);

        let circuit = builder.build(sum_node)?;

        // Prepare inputs
        let a = EncryptedU8::encrypt(5, keypair.client_key());
        let b = EncryptedU8::encrypt(3, keypair.client_key());

        let mut inputs = HashMap::new();
        inputs.insert("a".to_string(), a.to_cipher_blob()?);
        inputs.insert("b".to_string(), b.to_cipher_blob()?);

        // Execute
        let executor = FheExecutor::new();
        let result_blob = executor.execute(&circuit, &inputs)?;

        // Verify
        let result = EncryptedU8::from_cipher_blob(&result_blob)?;
        assert_eq!(result.decrypt(keypair.client_key()), 8);

        Ok(())
    }

    #[test]
    fn test_fhe_executor_boolean() -> Result<()> {
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("x", EncryptedType::Bool)
            .declare_variable("y", EncryptedType::Bool);

        let x_node = builder.load("x");
        let y_node = builder.load("y");
        let and_node = builder.and(x_node, y_node);

        let circuit = builder.build(and_node)?;

        let x = EncryptedBool::encrypt(true, keypair.client_key());
        let y = EncryptedBool::encrypt(false, keypair.client_key());

        let mut inputs = HashMap::new();
        inputs.insert("x".to_string(), x.to_cipher_blob()?);
        inputs.insert("y".to_string(), y.to_cipher_blob()?);

        let executor = FheExecutor::new();
        let result_blob = executor.execute(&circuit, &inputs)?;

        let result = EncryptedBool::from_cipher_blob(&result_blob)?;
        assert!(!result.decrypt(keypair.client_key()));

        Ok(())
    }

    #[test]
    fn test_fhe_executor_comparison() -> Result<()> {
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("a", EncryptedType::U8)
            .declare_variable("b", EncryptedType::U8);

        let a_node = builder.load("a");
        let b_node = builder.load("b");
        let gt_node = builder.gt(a_node, b_node);

        let circuit = builder.build(gt_node)?;

        let a = EncryptedU8::encrypt(10, keypair.client_key());
        let b = EncryptedU8::encrypt(5, keypair.client_key());

        let mut inputs = HashMap::new();
        inputs.insert("a".to_string(), a.to_cipher_blob()?);
        inputs.insert("b".to_string(), b.to_cipher_blob()?);

        let executor = FheExecutor::new();
        let result_blob = executor.execute(&circuit, &inputs)?;

        let result = EncryptedBool::from_cipher_blob(&result_blob)?;
        assert!(result.decrypt(keypair.client_key()));

        Ok(())
    }

    #[test]
    fn test_missing_input_error() -> Result<()> {
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        let mut builder = CircuitBuilder::new();
        builder.declare_variable("a", EncryptedType::U8);

        let a_node = builder.load("a");
        let circuit = builder.build(a_node)?;

        let inputs = HashMap::new(); // No inputs provided

        let executor = FheExecutor::new();
        let result = executor.execute(&circuit, &inputs);

        assert!(result.is_err());

        Ok(())
    }
}
