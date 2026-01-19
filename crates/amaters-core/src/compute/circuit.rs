//! Circuit compilation and optimization
//!
//! This module provides circuit AST representation, type inference,
//! and basic optimization for FHE operations.

use crate::error::{AmateRSError, ErrorContext, Result};
use std::collections::HashMap;

/// Circuit AST node representing FHE operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitNode {
    /// Load a variable by name
    Load(String),

    /// Constant value
    Constant(CircuitValue),

    /// Binary operation
    BinaryOp {
        op: BinaryOperator,
        left: Box<CircuitNode>,
        right: Box<CircuitNode>,
    },

    /// Unary operation
    UnaryOp {
        op: UnaryOperator,
        operand: Box<CircuitNode>,
    },

    /// Comparison operation
    Compare {
        op: CompareOperator,
        left: Box<CircuitNode>,
        right: Box<CircuitNode>,
    },
}

/// Binary operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
}

impl BinaryOperator {
    /// Get the string representation
    pub fn as_str(&self) -> &str {
        match self {
            BinaryOperator::Add => "+",
            BinaryOperator::Sub => "-",
            BinaryOperator::Mul => "*",
            BinaryOperator::And => "AND",
            BinaryOperator::Or => "OR",
            BinaryOperator::Xor => "XOR",
        }
    }
}

/// Unary operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Not,
    Neg,
}

impl UnaryOperator {
    /// Get the string representation
    pub fn as_str(&self) -> &str {
        match self {
            UnaryOperator::Not => "NOT",
            UnaryOperator::Neg => "-",
        }
    }
}

/// Comparison operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOperator {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CompareOperator {
    /// Get the string representation
    pub fn as_str(&self) -> &str {
        match self {
            CompareOperator::Eq => "=",
            CompareOperator::Ne => "!=",
            CompareOperator::Lt => "<",
            CompareOperator::Le => "<=",
            CompareOperator::Gt => ">",
            CompareOperator::Ge => ">=",
        }
    }
}

/// Circuit value types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitValue {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
}

/// Encrypted type information
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncryptedType {
    Bool,
    U8,
    U16,
    U32,
    U64,
}

impl EncryptedType {
    /// Get the bit width of the type
    pub fn bit_width(&self) -> usize {
        match self {
            EncryptedType::Bool => 1,
            EncryptedType::U8 => 8,
            EncryptedType::U16 => 16,
            EncryptedType::U32 => 32,
            EncryptedType::U64 => 64,
        }
    }

    /// Check if this type is numeric
    pub fn is_numeric(&self) -> bool {
        !matches!(self, EncryptedType::Bool)
    }

    /// Check if this type is boolean
    pub fn is_boolean(&self) -> bool {
        matches!(self, EncryptedType::Bool)
    }
}

impl std::fmt::Display for EncryptedType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncryptedType::Bool => write!(f, "bool"),
            EncryptedType::U8 => write!(f, "u8"),
            EncryptedType::U16 => write!(f, "u16"),
            EncryptedType::U32 => write!(f, "u32"),
            EncryptedType::U64 => write!(f, "u64"),
        }
    }
}

/// Circuit representation with metadata
#[derive(Debug, Clone)]
pub struct Circuit {
    /// Root node of the circuit
    pub root: CircuitNode,

    /// Type information for variables
    pub variable_types: HashMap<String, EncryptedType>,

    /// Inferred result type
    pub result_type: EncryptedType,

    /// Circuit depth (for complexity estimation)
    pub depth: usize,

    /// Number of gates (for complexity estimation)
    pub gate_count: usize,
}

impl Circuit {
    /// Create a new circuit from a root node
    pub fn new(root: CircuitNode, variable_types: HashMap<String, EncryptedType>) -> Result<Self> {
        let result_type = Self::infer_type(&root, &variable_types)?;
        let depth = Self::compute_depth(&root);
        let gate_count = Self::count_gates(&root);

        Ok(Self {
            root,
            variable_types,
            result_type,
            depth,
            gate_count,
        })
    }

    /// Infer the type of a circuit node
    fn infer_type(
        node: &CircuitNode,
        variable_types: &HashMap<String, EncryptedType>,
    ) -> Result<EncryptedType> {
        match node {
            CircuitNode::Load(name) => variable_types.get(name).copied().ok_or_else(|| {
                AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Undefined variable: {}",
                    name
                )))
            }),

            CircuitNode::Constant(value) => Ok(match value {
                CircuitValue::Bool(_) => EncryptedType::Bool,
                CircuitValue::U8(_) => EncryptedType::U8,
                CircuitValue::U16(_) => EncryptedType::U16,
                CircuitValue::U32(_) => EncryptedType::U32,
                CircuitValue::U64(_) => EncryptedType::U64,
            }),

            CircuitNode::BinaryOp { op, left, right } => {
                let left_type = Self::infer_type(left, variable_types)?;
                let right_type = Self::infer_type(right, variable_types)?;

                match op {
                    BinaryOperator::And | BinaryOperator::Or | BinaryOperator::Xor => {
                        if left_type != EncryptedType::Bool || right_type != EncryptedType::Bool {
                            return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                                "Logical operation requires boolean operands, got {} and {}",
                                left_type, right_type
                            ))));
                        }
                        Ok(EncryptedType::Bool)
                    }

                    BinaryOperator::Add | BinaryOperator::Sub | BinaryOperator::Mul => {
                        if !left_type.is_numeric() || !right_type.is_numeric() {
                            return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                                "Arithmetic operation requires numeric operands, got {} and {}",
                                left_type, right_type
                            ))));
                        }

                        if left_type != right_type {
                            return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                                "Arithmetic operation requires matching types, got {} and {}",
                                left_type, right_type
                            ))));
                        }

                        Ok(left_type)
                    }
                }
            }

            CircuitNode::UnaryOp { op, operand } => {
                let operand_type = Self::infer_type(operand, variable_types)?;

                match op {
                    UnaryOperator::Not => {
                        if operand_type != EncryptedType::Bool {
                            return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                                "NOT operation requires boolean operand, got {}",
                                operand_type
                            ))));
                        }
                        Ok(EncryptedType::Bool)
                    }

                    UnaryOperator::Neg => {
                        if !operand_type.is_numeric() {
                            return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                                "Negation operation requires numeric operand, got {}",
                                operand_type
                            ))));
                        }
                        Ok(operand_type)
                    }
                }
            }

            CircuitNode::Compare { left, right, .. } => {
                let left_type = Self::infer_type(left, variable_types)?;
                let right_type = Self::infer_type(right, variable_types)?;

                if left_type != right_type {
                    return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                        "Comparison requires matching types, got {} and {}",
                        left_type, right_type
                    ))));
                }

                Ok(EncryptedType::Bool)
            }
        }
    }

    /// Compute the depth of the circuit
    fn compute_depth(node: &CircuitNode) -> usize {
        match node {
            CircuitNode::Load(_) | CircuitNode::Constant(_) => 1,

            CircuitNode::BinaryOp { left, right, .. }
            | CircuitNode::Compare { left, right, .. } => {
                1 + Self::compute_depth(left).max(Self::compute_depth(right))
            }

            CircuitNode::UnaryOp { operand, .. } => 1 + Self::compute_depth(operand),
        }
    }

    /// Count the number of gates in the circuit
    fn count_gates(node: &CircuitNode) -> usize {
        match node {
            CircuitNode::Load(_) | CircuitNode::Constant(_) => 0,

            CircuitNode::BinaryOp { left, right, .. }
            | CircuitNode::Compare { left, right, .. } => {
                1 + Self::count_gates(left) + Self::count_gates(right)
            }

            CircuitNode::UnaryOp { operand, .. } => 1 + Self::count_gates(operand),
        }
    }

    /// Validate the circuit for correctness
    pub fn validate(&self) -> Result<()> {
        Self::validate_node(&self.root, &self.variable_types)?;
        Ok(())
    }

    fn validate_node(
        node: &CircuitNode,
        variable_types: &HashMap<String, EncryptedType>,
    ) -> Result<()> {
        match node {
            CircuitNode::Load(name) => {
                if !variable_types.contains_key(name) {
                    return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                        "Undefined variable: {}",
                        name
                    ))));
                }
                Ok(())
            }

            CircuitNode::Constant(_) => Ok(()),

            CircuitNode::BinaryOp { left, right, .. }
            | CircuitNode::Compare { left, right, .. } => {
                Self::validate_node(left, variable_types)?;
                Self::validate_node(right, variable_types)?;
                Ok(())
            }

            CircuitNode::UnaryOp { operand, .. } => Self::validate_node(operand, variable_types),
        }
    }
}

/// Circuit builder for constructing circuits programmatically
#[derive(Default)]
pub struct CircuitBuilder {
    variable_types: HashMap<String, EncryptedType>,
}

impl CircuitBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the variable types map
    pub fn variable_types(&self) -> &HashMap<String, EncryptedType> {
        &self.variable_types
    }

    /// Clone the variable types map
    pub fn variable_types_clone(&self) -> HashMap<String, EncryptedType> {
        self.variable_types.clone()
    }

    /// Declare a variable with its type
    pub fn declare_variable(&mut self, name: impl Into<String>, ty: EncryptedType) -> &mut Self {
        self.variable_types.insert(name.into(), ty);
        self
    }

    /// Build the circuit from a root node
    pub fn build(&self, root: CircuitNode) -> Result<Circuit> {
        Circuit::new(root, self.variable_types.clone())
    }

    /// Create a load node
    pub fn load(&self, name: impl Into<String>) -> CircuitNode {
        CircuitNode::Load(name.into())
    }

    /// Create a constant node
    pub fn constant(&self, value: CircuitValue) -> CircuitNode {
        CircuitNode::Constant(value)
    }

    /// Create an addition node
    pub fn add(&self, left: CircuitNode, right: CircuitNode) -> CircuitNode {
        CircuitNode::BinaryOp {
            op: BinaryOperator::Add,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Create a subtraction node
    pub fn sub(&self, left: CircuitNode, right: CircuitNode) -> CircuitNode {
        CircuitNode::BinaryOp {
            op: BinaryOperator::Sub,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Create a multiplication node
    pub fn mul(&self, left: CircuitNode, right: CircuitNode) -> CircuitNode {
        CircuitNode::BinaryOp {
            op: BinaryOperator::Mul,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Create an AND node
    pub fn and(&self, left: CircuitNode, right: CircuitNode) -> CircuitNode {
        CircuitNode::BinaryOp {
            op: BinaryOperator::And,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Create an OR node
    pub fn or(&self, left: CircuitNode, right: CircuitNode) -> CircuitNode {
        CircuitNode::BinaryOp {
            op: BinaryOperator::Or,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Create an XOR node
    pub fn xor(&self, left: CircuitNode, right: CircuitNode) -> CircuitNode {
        CircuitNode::BinaryOp {
            op: BinaryOperator::Xor,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Create a NOT node
    pub fn not(&self, operand: CircuitNode) -> CircuitNode {
        CircuitNode::UnaryOp {
            op: UnaryOperator::Not,
            operand: Box::new(operand),
        }
    }

    /// Create an equality comparison node
    pub fn eq(&self, left: CircuitNode, right: CircuitNode) -> CircuitNode {
        CircuitNode::Compare {
            op: CompareOperator::Eq,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Create a less-than comparison node
    pub fn lt(&self, left: CircuitNode, right: CircuitNode) -> CircuitNode {
        CircuitNode::Compare {
            op: CompareOperator::Lt,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Create a greater-than comparison node
    pub fn gt(&self, left: CircuitNode, right: CircuitNode) -> CircuitNode {
        CircuitNode::Compare {
            op: CompareOperator::Gt,
            left: Box::new(left),
            right: Box::new(right),
        }
    }
}

/// Basic circuit optimizer for backward compatibility
///
/// This is a legacy optimizer kept for backward compatibility.
/// For advanced optimizations, use the `optimizer` module instead.
#[derive(Debug, Clone, Default)]
#[deprecated(
    since = "0.1.0",
    note = "Use CircuitOptimizer from optimizer module instead"
)]
pub struct CircuitOptimizer;

#[allow(deprecated)]
impl CircuitOptimizer {
    pub fn new() -> Self {
        Self
    }

    /// Optimize circuit by applying basic optimization passes
    ///
    /// For advanced optimizations including bootstrap minimization,
    /// gate fusion, and parallelization analysis, use the optimizer module.
    pub fn optimize(&self, circuit: Circuit) -> Result<Circuit> {
        // Delegate to the advanced optimizer
        let mut advanced_optimizer = crate::compute::optimizer::CircuitOptimizer::new();
        advanced_optimizer.optimize(circuit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_builder() -> Result<()> {
        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("a", EncryptedType::U8)
            .declare_variable("b", EncryptedType::U8);

        let a = builder.load("a");
        let b = builder.load("b");
        let sum = builder.add(a, b);

        let circuit = builder.build(sum)?;
        assert_eq!(circuit.result_type, EncryptedType::U8);
        assert_eq!(circuit.gate_count, 1);

        Ok(())
    }

    #[test]
    fn test_type_inference() -> Result<()> {
        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("x", EncryptedType::Bool)
            .declare_variable("y", EncryptedType::Bool);

        let x = builder.load("x");
        let y = builder.load("y");
        let result = builder.and(x, y);

        let circuit = builder.build(result)?;
        assert_eq!(circuit.result_type, EncryptedType::Bool);

        Ok(())
    }

    #[test]
    fn test_type_mismatch_error() {
        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("a", EncryptedType::U8)
            .declare_variable("b", EncryptedType::Bool);

        let a = builder.load("a");
        let b = builder.load("b");
        let invalid = builder.add(a, b);

        let result = builder.build(invalid);
        assert!(result.is_err());
    }

    #[test]
    #[allow(deprecated)]
    fn test_constant_folding() -> Result<()> {
        let optimizer = CircuitOptimizer::new();
        let builder = CircuitBuilder::new();

        let a = builder.constant(CircuitValue::U8(5));
        let b = builder.constant(CircuitValue::U8(3));
        let sum = builder.add(a, b);

        let circuit = Circuit::new(sum, HashMap::new())?;
        let optimized = optimizer.optimize(circuit)?;

        // Should fold to constant 8
        match optimized.root {
            CircuitNode::Constant(CircuitValue::U8(8)) => Ok(()),
            _ => Err(AmateRSError::FheComputation(ErrorContext::new(
                "Constant folding failed".to_string(),
            ))),
        }
    }

    #[test]
    fn test_circuit_depth() -> Result<()> {
        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("a", EncryptedType::U8)
            .declare_variable("b", EncryptedType::U8)
            .declare_variable("c", EncryptedType::U8);

        let a = builder.load("a");
        let b = builder.load("b");
        let c = builder.load("c");

        // (a + b) + c
        let sum1 = builder.add(a, b);
        let sum2 = builder.add(sum1, c);

        let circuit = builder.build(sum2)?;
        assert_eq!(circuit.depth, 3); // Load -> Add -> Add

        Ok(())
    }
}
