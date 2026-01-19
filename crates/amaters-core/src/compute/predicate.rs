//! Predicate-to-FHE-Circuit Compiler
//!
//! This module provides compilation of AmateRS query predicates into FHE circuits
//! that can be executed on encrypted data without revealing plaintext values.

use crate::compute::{
    Circuit, CircuitBuilder, CircuitNode, CircuitValue, CompareOperator, EncryptedType,
};
use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::{CipherBlob, ColumnRef, Predicate};

/// Compiles query predicates into executable FHE circuits
///
/// The PredicateCompiler transforms high-level query predicates (like `age > 18`)
/// into FHE circuits that can evaluate these conditions on encrypted data.
/// The result is always an encrypted boolean indicating whether the predicate
/// matches or not.
///
/// # Example
///
/// ```rust,ignore
/// use amaters_core::compute::{PredicateCompiler, EncryptedType};
/// use amaters_core::types::{Predicate, col, CipherBlob};
///
/// let mut compiler = PredicateCompiler::new();
///
/// // Compile: age > 18
/// let predicate = Predicate::Gt(col("age"), encrypted_18);
/// let circuit = compiler.compile(&predicate, EncryptedType::U8)?;
///
/// // The circuit can now be executed on encrypted age values
/// ```
pub struct PredicateCompiler {
    builder: CircuitBuilder,
}

impl PredicateCompiler {
    /// Create a new predicate compiler
    pub fn new() -> Self {
        Self {
            builder: CircuitBuilder::new(),
        }
    }

    /// Compile a predicate into an FHE circuit
    ///
    /// The resulting circuit will have inputs for:
    /// - `value`: The encrypted column value to test
    /// - `rhs`: The encrypted comparison value (right-hand side)
    ///
    /// The circuit output is an encrypted boolean indicating the predicate result.
    ///
    /// # Arguments
    ///
    /// * `predicate` - The predicate to compile
    /// * `value_type` - The encrypted type of the values being compared
    ///
    /// # Returns
    ///
    /// A `Circuit` that evaluates the predicate on encrypted data
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The predicate references undefined columns
    /// - Type inference fails
    /// - The circuit construction is invalid
    pub fn compile(&mut self, predicate: &Predicate, value_type: EncryptedType) -> Result<Circuit> {
        // Declare variables for the circuit
        self.builder.declare_variable("value", value_type);
        self.builder.declare_variable("rhs", value_type);

        // Compile the predicate into a circuit node
        let root = self.compile_node(predicate)?;

        // Build and return the circuit
        self.builder.build(root)
    }

    /// Recursively compile a predicate node into a circuit node
    fn compile_node(&self, predicate: &Predicate) -> Result<CircuitNode> {
        match predicate {
            Predicate::Eq(col, _value) => {
                // Equality: value == rhs
                self.validate_column(col)?;
                let value_node = self.builder.load("value");
                let rhs_node = self.builder.load("rhs");
                Ok(self.builder.eq(value_node, rhs_node))
            }

            Predicate::Gt(col, _value) => {
                // Greater than: value > rhs
                self.validate_column(col)?;
                let value_node = self.builder.load("value");
                let rhs_node = self.builder.load("rhs");
                Ok(self.builder.gt(value_node, rhs_node))
            }

            Predicate::Lt(col, _value) => {
                // Less than: value < rhs
                self.validate_column(col)?;
                let value_node = self.builder.load("value");
                let rhs_node = self.builder.load("rhs");
                Ok(self.builder.lt(value_node, rhs_node))
            }

            Predicate::Gte(col, _value) => {
                // Greater than or equal: value >= rhs
                self.validate_column(col)?;
                let value_node = self.builder.load("value");
                let rhs_node = self.builder.load("rhs");
                // Implement as NOT (value < rhs)
                let lt_node = self.builder.lt(value_node, rhs_node);
                Ok(self.builder.not(lt_node))
            }

            Predicate::Lte(col, _value) => {
                // Less than or equal: value <= rhs
                self.validate_column(col)?;
                let value_node = self.builder.load("value");
                let rhs_node = self.builder.load("rhs");
                // Implement as NOT (value > rhs)
                let gt_node = self.builder.gt(value_node, rhs_node);
                Ok(self.builder.not(gt_node))
            }

            Predicate::And(left, right) => {
                // Logical AND: left AND right
                // Note: This requires both predicates to reference the same value
                // For now, we'll compile recursively but this may need refinement
                // for multi-column predicates
                let left_circuit = self.compile_node(left)?;
                let right_circuit = self.compile_node(right)?;
                Ok(self.builder.and(left_circuit, right_circuit))
            }

            Predicate::Or(left, right) => {
                // Logical OR: left OR right
                let left_circuit = self.compile_node(left)?;
                let right_circuit = self.compile_node(right)?;
                Ok(self.builder.or(left_circuit, right_circuit))
            }

            Predicate::Not(pred) => {
                // Logical NOT: NOT pred
                let pred_circuit = self.compile_node(pred)?;
                Ok(self.builder.not(pred_circuit))
            }
        }
    }

    /// Validate that a column reference is supported
    ///
    /// For now, we only support single-column predicates with the column named "value"
    fn validate_column(&self, col: &ColumnRef) -> Result<()> {
        // In the current design, we're evaluating predicates on individual values
        // The column reference should match what we're testing
        // For now, we accept any column name since we're binding it to "value"
        let _ = col;
        Ok(())
    }

    /// Extract the RHS (right-hand side) value from a predicate
    ///
    /// This walks the predicate tree to find comparison values.
    /// For composite predicates (And/Or/Not), it extracts from the first
    /// comparison it encounters.
    ///
    /// # Arguments
    ///
    /// * `predicate` - The predicate to extract from
    ///
    /// # Returns
    ///
    /// The encrypted value used in the predicate comparison
    ///
    /// # Errors
    ///
    /// Returns an error if the predicate contains no comparison operations
    pub fn extract_rhs_value(predicate: &Predicate) -> Result<CipherBlob> {
        match predicate {
            Predicate::Eq(_, value)
            | Predicate::Gt(_, value)
            | Predicate::Lt(_, value)
            | Predicate::Gte(_, value)
            | Predicate::Lte(_, value) => Ok(value.clone()),

            Predicate::And(left, _right) => {
                // For AND, extract from left (could also merge both)
                Self::extract_rhs_value(left)
            }

            Predicate::Or(left, _right) => {
                // For OR, extract from left
                Self::extract_rhs_value(left)
            }

            Predicate::Not(pred) => {
                // For NOT, extract from inner predicate
                Self::extract_rhs_value(pred)
            }
        }
    }

    /// Extract all RHS values from a predicate
    ///
    /// For composite predicates, this returns all comparison values.
    /// This is useful for complex predicates like `age > 18 AND age < 65`
    /// which have multiple RHS values.
    ///
    /// # Arguments
    ///
    /// * `predicate` - The predicate to extract from
    ///
    /// # Returns
    ///
    /// A vector of all encrypted values used in comparisons
    pub fn extract_all_rhs_values(predicate: &Predicate) -> Vec<CipherBlob> {
        match predicate {
            Predicate::Eq(_, value)
            | Predicate::Gt(_, value)
            | Predicate::Lt(_, value)
            | Predicate::Gte(_, value)
            | Predicate::Lte(_, value) => vec![value.clone()],

            Predicate::And(left, right) => {
                let mut values = Self::extract_all_rhs_values(left);
                values.extend(Self::extract_all_rhs_values(right));
                values
            }

            Predicate::Or(left, right) => {
                let mut values = Self::extract_all_rhs_values(left);
                values.extend(Self::extract_all_rhs_values(right));
                values
            }

            Predicate::Not(pred) => Self::extract_all_rhs_values(pred),
        }
    }

    /// Get the required encrypted type for a predicate's values
    ///
    /// This analyzes the predicate to determine what type of encrypted values
    /// it operates on. This is useful for automatic type inference.
    ///
    /// # Arguments
    ///
    /// * `predicate` - The predicate to analyze
    ///
    /// # Returns
    ///
    /// The encrypted type hint, or None if it cannot be determined
    pub fn infer_value_type(_predicate: &Predicate) -> Option<EncryptedType> {
        // For now, we don't have type information in the predicate itself
        // This would require extending the Predicate enum with type metadata
        // or analyzing the CipherBlob metadata
        None
    }
}

impl Default for PredicateCompiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper function to compile a simple predicate
///
/// This is a convenience wrapper around PredicateCompiler for single predicates.
///
/// # Example
///
/// ```rust,ignore
/// use amaters_core::compute::{compile_predicate, EncryptedType};
/// use amaters_core::types::{Predicate, col};
///
/// let predicate = Predicate::Gt(col("age"), encrypted_18);
/// let circuit = compile_predicate(&predicate, EncryptedType::U8)?;
/// ```
pub fn compile_predicate(predicate: &Predicate, value_type: EncryptedType) -> Result<Circuit> {
    let mut compiler = PredicateCompiler::new();
    compiler.compile(predicate, value_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::col;

    fn make_test_blob(value: u8) -> CipherBlob {
        CipherBlob::new(vec![value])
    }

    #[test]
    fn test_compiler_creation() {
        let compiler = PredicateCompiler::new();
        assert_eq!(compiler.builder.variable_types().len(), 0);
    }

    #[test]
    fn test_compile_eq_predicate() -> Result<()> {
        let mut compiler = PredicateCompiler::new();
        let predicate = Predicate::Eq(col("age"), make_test_blob(18));

        let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

        assert_eq!(circuit.result_type, EncryptedType::Bool);
        assert_eq!(circuit.variable_types.len(), 2);
        assert!(circuit.variable_types.contains_key("value"));
        assert!(circuit.variable_types.contains_key("rhs"));

        Ok(())
    }

    #[test]
    fn test_compile_gt_predicate() -> Result<()> {
        let mut compiler = PredicateCompiler::new();
        let predicate = Predicate::Gt(col("age"), make_test_blob(18));

        let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

        assert_eq!(circuit.result_type, EncryptedType::Bool);
        assert!(circuit.gate_count > 0);

        Ok(())
    }

    #[test]
    fn test_compile_lt_predicate() -> Result<()> {
        let mut compiler = PredicateCompiler::new();
        let predicate = Predicate::Lt(col("age"), make_test_blob(65));

        let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

        assert_eq!(circuit.result_type, EncryptedType::Bool);

        Ok(())
    }

    #[test]
    fn test_compile_gte_predicate() -> Result<()> {
        let mut compiler = PredicateCompiler::new();
        let predicate = Predicate::Gte(col("age"), make_test_blob(18));

        let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

        assert_eq!(circuit.result_type, EncryptedType::Bool);
        // Gte is implemented as NOT (value < rhs), so should have a NOT gate
        assert!(matches!(circuit.root, CircuitNode::UnaryOp { .. }));

        Ok(())
    }

    #[test]
    fn test_compile_lte_predicate() -> Result<()> {
        let mut compiler = PredicateCompiler::new();
        let predicate = Predicate::Lte(col("age"), make_test_blob(65));

        let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

        assert_eq!(circuit.result_type, EncryptedType::Bool);
        // Lte is implemented as NOT (value > rhs), so should have a NOT gate
        assert!(matches!(circuit.root, CircuitNode::UnaryOp { .. }));

        Ok(())
    }

    #[test]
    fn test_compile_and_predicate() -> Result<()> {
        let mut compiler = PredicateCompiler::new();

        // age > 18 AND age < 65
        let pred1 = Predicate::Gt(col("age"), make_test_blob(18));
        let pred2 = Predicate::Lt(col("age"), make_test_blob(65));
        let predicate = Predicate::And(Box::new(pred1), Box::new(pred2));

        let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

        assert_eq!(circuit.result_type, EncryptedType::Bool);
        assert!(matches!(circuit.root, CircuitNode::BinaryOp { .. }));

        // Should have more gates due to AND
        assert!(circuit.gate_count >= 2);

        Ok(())
    }

    #[test]
    fn test_compile_or_predicate() -> Result<()> {
        let mut compiler = PredicateCompiler::new();

        // age < 18 OR age > 65
        let pred1 = Predicate::Lt(col("age"), make_test_blob(18));
        let pred2 = Predicate::Gt(col("age"), make_test_blob(65));
        let predicate = Predicate::Or(Box::new(pred1), Box::new(pred2));

        let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

        assert_eq!(circuit.result_type, EncryptedType::Bool);
        assert!(matches!(circuit.root, CircuitNode::BinaryOp { .. }));

        Ok(())
    }

    #[test]
    fn test_compile_not_predicate() -> Result<()> {
        let mut compiler = PredicateCompiler::new();

        // NOT (age == 18)
        let pred = Predicate::Eq(col("age"), make_test_blob(18));
        let predicate = Predicate::Not(Box::new(pred));

        let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

        assert_eq!(circuit.result_type, EncryptedType::Bool);
        assert!(matches!(circuit.root, CircuitNode::UnaryOp { .. }));

        Ok(())
    }

    #[test]
    fn test_compile_complex_predicate() -> Result<()> {
        let mut compiler = PredicateCompiler::new();

        // (age > 18 AND age < 65) OR age == 100
        let pred1 = Predicate::Gt(col("age"), make_test_blob(18));
        let pred2 = Predicate::Lt(col("age"), make_test_blob(65));
        let and_pred = Predicate::And(Box::new(pred1), Box::new(pred2));

        let pred3 = Predicate::Eq(col("age"), make_test_blob(100));
        let predicate = Predicate::Or(Box::new(and_pred), Box::new(pred3));

        let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

        assert_eq!(circuit.result_type, EncryptedType::Bool);
        // Complex predicate should have multiple gates
        assert!(circuit.gate_count >= 3);
        assert!(circuit.depth >= 2);

        Ok(())
    }

    #[test]
    fn test_extract_rhs_value() -> Result<()> {
        let blob = make_test_blob(42);
        let predicate = Predicate::Gt(col("age"), blob.clone());

        let extracted = PredicateCompiler::extract_rhs_value(&predicate)?;
        assert_eq!(extracted, blob);

        Ok(())
    }

    #[test]
    fn test_extract_rhs_from_and() -> Result<()> {
        let blob1 = make_test_blob(18);
        let blob2 = make_test_blob(65);

        let pred1 = Predicate::Gt(col("age"), blob1.clone());
        let pred2 = Predicate::Lt(col("age"), blob2);
        let predicate = Predicate::And(Box::new(pred1), Box::new(pred2));

        // Should extract from left predicate
        let extracted = PredicateCompiler::extract_rhs_value(&predicate)?;
        assert_eq!(extracted, blob1);

        Ok(())
    }

    #[test]
    fn test_extract_all_rhs_values() {
        let blob1 = make_test_blob(18);
        let blob2 = make_test_blob(65);

        let pred1 = Predicate::Gt(col("age"), blob1.clone());
        let pred2 = Predicate::Lt(col("age"), blob2.clone());
        let predicate = Predicate::And(Box::new(pred1), Box::new(pred2));

        let values = PredicateCompiler::extract_all_rhs_values(&predicate);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], blob1);
        assert_eq!(values[1], blob2);
    }

    #[test]
    fn test_compile_predicate_helper() -> Result<()> {
        let predicate = Predicate::Eq(col("age"), make_test_blob(18));
        let circuit = compile_predicate(&predicate, EncryptedType::U8)?;

        assert_eq!(circuit.result_type, EncryptedType::Bool);

        Ok(())
    }

    #[test]
    fn test_circuit_validation() -> Result<()> {
        let mut compiler = PredicateCompiler::new();
        let predicate = Predicate::Gt(col("age"), make_test_blob(18));

        let circuit = compiler.compile(&predicate, EncryptedType::U8)?;

        // Circuit should be valid
        circuit.validate()?;

        Ok(())
    }
}
