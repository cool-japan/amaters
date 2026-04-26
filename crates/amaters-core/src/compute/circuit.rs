//! Circuit compilation and optimization
//!
//! This module provides circuit AST representation, type inference,
//! and basic optimization for FHE operations.

use crate::error::{AmateRSError, ErrorContext, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Type tag for encrypted constants, indicating the original plaintext type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConstantType {
    /// Original value was an integer (u8, u16, u32, u64)
    Integer,
    /// Original value was a boolean
    Boolean,
    /// Original value was a floating-point number
    Float,
    /// Original value was raw bytes
    Bytes,
}

impl std::fmt::Display for ConstantType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstantType::Integer => write!(f, "integer"),
            ConstantType::Boolean => write!(f, "boolean"),
            ConstantType::Float => write!(f, "float"),
            ConstantType::Bytes => write!(f, "bytes"),
        }
    }
}

/// Circuit AST node representing FHE operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitNode {
    /// Load a variable by name
    Load(String),

    /// Constant value (plaintext)
    Constant(CircuitValue),

    /// Encrypted constant value (ciphertext form, opaque to the optimizer)
    ///
    /// This variant holds a constant that has already been encrypted for use
    /// in FHE evaluation. The optimizer must NOT attempt to constant-fold or
    /// simplify encrypted constants since their plaintext values are unknown.
    EncryptedConstant {
        /// Encrypted ciphertext data
        data: Vec<u8>,
        /// The type of the original plaintext value before encryption
        original_type: ConstantType,
    },

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

impl std::fmt::Display for CircuitNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitNode::Load(name) => write!(f, "Load({})", name),
            CircuitNode::Constant(value) => match value {
                CircuitValue::Bool(v) => write!(f, "Const({})", v),
                CircuitValue::U8(v) => write!(f, "Const({}u8)", v),
                CircuitValue::U16(v) => write!(f, "Const({}u16)", v),
                CircuitValue::U32(v) => write!(f, "Const({}u32)", v),
                CircuitValue::U64(v) => write!(f, "Const({}u64)", v),
            },
            CircuitNode::EncryptedConstant {
                data,
                original_type,
            } => {
                write!(f, "EncryptedConst({}, {} bytes)", original_type, data.len())
            }
            CircuitNode::BinaryOp { op, left, right } => {
                write!(f, "({} {} {})", left, op.as_str(), right)
            }
            CircuitNode::UnaryOp { op, operand } => {
                write!(f, "{}({})", op.as_str(), operand)
            }
            CircuitNode::Compare { op, left, right } => {
                write!(f, "({} {} {})", left, op.as_str(), right)
            }
        }
    }
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

            CircuitNode::EncryptedConstant { original_type, .. } => {
                Ok(match original_type {
                    ConstantType::Boolean => EncryptedType::Bool,
                    // For non-boolean encrypted constants, we default to U64
                    // since the exact width is not recoverable from the encrypted data.
                    // In practice, users should ensure type consistency.
                    ConstantType::Integer | ConstantType::Float | ConstantType::Bytes => {
                        EncryptedType::U64
                    }
                })
            }

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
            CircuitNode::Load(_)
            | CircuitNode::Constant(_)
            | CircuitNode::EncryptedConstant { .. } => 1,

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
            CircuitNode::Load(_)
            | CircuitNode::Constant(_)
            | CircuitNode::EncryptedConstant { .. } => 0,

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

            CircuitNode::Constant(_) | CircuitNode::EncryptedConstant { .. } => Ok(()),

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

    /// Create an encrypted constant node directly
    pub fn encrypted_constant(&self, data: Vec<u8>, original_type: ConstantType) -> CircuitNode {
        CircuitNode::EncryptedConstant {
            data,
            original_type,
        }
    }
}

// ---------------------------------------------------------------------------
// Encrypted constant helpers
// ---------------------------------------------------------------------------

/// Encrypt a plaintext circuit constant value using a symmetric key.
///
/// This uses a simple XOR-based stream cipher derived from the key for
/// demonstration purposes. In production, this would delegate to the
/// actual FHE encryption backend (e.g., TFHE key-switch + bootstrap).
///
/// The output ciphertext contains a 1-byte type tag followed by the
/// XOR-encrypted payload so that it can be correctly interpreted during
/// evaluation.
pub fn encrypt_constant(value: &CircuitValue, key: &[u8]) -> Result<Vec<u8>> {
    if key.is_empty() {
        return Err(AmateRSError::FheComputation(ErrorContext::new(
            "Encryption key must not be empty".to_string(),
        )));
    }

    // Serialize the plaintext value to bytes
    let (type_tag, plaintext): (u8, Vec<u8>) = match value {
        CircuitValue::Bool(v) => (0x00, vec![if *v { 1 } else { 0 }]),
        CircuitValue::U8(v) => (0x01, v.to_le_bytes().to_vec()),
        CircuitValue::U16(v) => (0x02, v.to_le_bytes().to_vec()),
        CircuitValue::U32(v) => (0x03, v.to_le_bytes().to_vec()),
        CircuitValue::U64(v) => (0x04, v.to_le_bytes().to_vec()),
    };

    // Generate a keystream by repeating and hashing the key material
    let keystream = derive_keystream(key, plaintext.len());

    // XOR plaintext with keystream
    let ciphertext: Vec<u8> = plaintext
        .iter()
        .zip(keystream.iter())
        .map(|(p, k)| p ^ k)
        .collect();

    // Prepend type tag (unencrypted, needed for type inference)
    let mut output = Vec::with_capacity(1 + ciphertext.len());
    output.push(type_tag);
    output.extend_from_slice(&ciphertext);

    Ok(output)
}

/// Decrypt an encrypted constant back to its plaintext value.
///
/// Inverse of [`encrypt_constant`]. Returns an error if the data is
/// malformed or the key does not match.
pub fn decrypt_constant(data: &[u8], key: &[u8]) -> Result<CircuitValue> {
    if key.is_empty() {
        return Err(AmateRSError::FheComputation(ErrorContext::new(
            "Decryption key must not be empty".to_string(),
        )));
    }
    if data.is_empty() {
        return Err(AmateRSError::FheComputation(ErrorContext::new(
            "Encrypted constant data is empty".to_string(),
        )));
    }

    let type_tag = data[0];
    let ciphertext = &data[1..];

    let keystream = derive_keystream(key, ciphertext.len());
    let plaintext: Vec<u8> = ciphertext
        .iter()
        .zip(keystream.iter())
        .map(|(c, k)| c ^ k)
        .collect();

    match type_tag {
        0x00 => {
            if plaintext.is_empty() {
                return Err(AmateRSError::FheComputation(ErrorContext::new(
                    "Encrypted boolean constant has no payload".to_string(),
                )));
            }
            Ok(CircuitValue::Bool(plaintext[0] != 0))
        }
        0x01 => {
            let arr: [u8; 1] = plaintext.as_slice().try_into().map_err(|_| {
                AmateRSError::FheComputation(ErrorContext::new(
                    "Invalid encrypted u8 constant length".to_string(),
                ))
            })?;
            Ok(CircuitValue::U8(u8::from_le_bytes(arr)))
        }
        0x02 => {
            let arr: [u8; 2] = plaintext.as_slice().try_into().map_err(|_| {
                AmateRSError::FheComputation(ErrorContext::new(
                    "Invalid encrypted u16 constant length".to_string(),
                ))
            })?;
            Ok(CircuitValue::U16(u16::from_le_bytes(arr)))
        }
        0x03 => {
            let arr: [u8; 4] = plaintext.as_slice().try_into().map_err(|_| {
                AmateRSError::FheComputation(ErrorContext::new(
                    "Invalid encrypted u32 constant length".to_string(),
                ))
            })?;
            Ok(CircuitValue::U32(u32::from_le_bytes(arr)))
        }
        0x04 => {
            let arr: [u8; 8] = plaintext.as_slice().try_into().map_err(|_| {
                AmateRSError::FheComputation(ErrorContext::new(
                    "Invalid encrypted u64 constant length".to_string(),
                ))
            })?;
            Ok(CircuitValue::U64(u64::from_le_bytes(arr)))
        }
        _ => Err(AmateRSError::FheComputation(ErrorContext::new(format!(
            "Unknown encrypted constant type tag: 0x{:02x}",
            type_tag
        )))),
    }
}

/// Recursively walk a circuit tree and encrypt all `Constant` nodes into
/// `EncryptedConstant` nodes. This is a pre-processing step before FHE
/// evaluation to ensure no plaintext constants leak into the encrypted
/// computation.
pub fn encrypt_circuit_constants(node: &CircuitNode, key: &[u8]) -> Result<CircuitNode> {
    match node {
        CircuitNode::Load(name) => Ok(CircuitNode::Load(name.clone())),

        CircuitNode::Constant(value) => {
            let data = encrypt_constant(value, key)?;
            let original_type = match value {
                CircuitValue::Bool(_) => ConstantType::Boolean,
                CircuitValue::U8(_)
                | CircuitValue::U16(_)
                | CircuitValue::U32(_)
                | CircuitValue::U64(_) => ConstantType::Integer,
            };
            Ok(CircuitNode::EncryptedConstant {
                data,
                original_type,
            })
        }

        // Already encrypted — pass through
        CircuitNode::EncryptedConstant {
            data,
            original_type,
        } => Ok(CircuitNode::EncryptedConstant {
            data: data.clone(),
            original_type: *original_type,
        }),

        CircuitNode::BinaryOp { op, left, right } => {
            let left = encrypt_circuit_constants(left, key)?;
            let right = encrypt_circuit_constants(right, key)?;
            Ok(CircuitNode::BinaryOp {
                op: *op,
                left: Box::new(left),
                right: Box::new(right),
            })
        }

        CircuitNode::UnaryOp { op, operand } => {
            let operand = encrypt_circuit_constants(operand, key)?;
            Ok(CircuitNode::UnaryOp {
                op: *op,
                operand: Box::new(operand),
            })
        }

        CircuitNode::Compare { op, left, right } => {
            let left = encrypt_circuit_constants(left, key)?;
            let right = encrypt_circuit_constants(right, key)?;
            Ok(CircuitNode::Compare {
                op: *op,
                left: Box::new(left),
                right: Box::new(right),
            })
        }
    }
}

/// Derive a deterministic keystream from a key for XOR encryption.
///
/// Uses a simple hash-chain approach: each block of the keystream is
/// derived by hashing (key || block_index) using a lightweight mixing
/// function. This is NOT cryptographically secure — real FHE would use
/// the actual FHE encryption scheme.
fn derive_keystream(key: &[u8], length: usize) -> Vec<u8> {
    let mut keystream = Vec::with_capacity(length);
    let mut block_index: u64 = 0;

    while keystream.len() < length {
        // Simple deterministic mixing: FNV-1a-inspired hash of key + block index
        let mut hash: u64 = 0xcbf29ce484222325;
        for &byte in key {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for &byte in &block_index.to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }

        // Extract bytes from the hash
        for &byte in &hash.to_le_bytes() {
            if keystream.len() < length {
                keystream.push(byte);
            }
        }

        block_index += 1;
    }

    keystream
}

/// Check whether a circuit node is an encrypted constant
pub fn is_encrypted_constant(node: &CircuitNode) -> bool {
    matches!(node, CircuitNode::EncryptedConstant { .. })
}

/// Count the number of plaintext constants in a circuit tree
pub fn count_plaintext_constants(node: &CircuitNode) -> usize {
    match node {
        CircuitNode::Constant(_) => 1,
        CircuitNode::EncryptedConstant { .. } | CircuitNode::Load(_) => 0,
        CircuitNode::BinaryOp { left, right, .. } | CircuitNode::Compare { left, right, .. } => {
            count_plaintext_constants(left) + count_plaintext_constants(right)
        }
        CircuitNode::UnaryOp { operand, .. } => count_plaintext_constants(operand),
    }
}

/// Count the number of encrypted constants in a circuit tree
pub fn count_encrypted_constants(node: &CircuitNode) -> usize {
    match node {
        CircuitNode::EncryptedConstant { .. } => 1,
        CircuitNode::Constant(_) | CircuitNode::Load(_) => 0,
        CircuitNode::BinaryOp { left, right, .. } | CircuitNode::Compare { left, right, .. } => {
            count_encrypted_constants(left) + count_encrypted_constants(right)
        }
        CircuitNode::UnaryOp { operand, .. } => count_encrypted_constants(operand),
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

    // ── Encrypted constant tests ──────────────────────────────────────

    #[test]
    fn test_encrypted_constant_creation() {
        let builder = CircuitBuilder::new();
        let enc = builder.encrypted_constant(vec![0xAA, 0xBB], ConstantType::Integer);

        match enc {
            CircuitNode::EncryptedConstant {
                data,
                original_type,
            } => {
                assert_eq!(data, vec![0xAA, 0xBB]);
                assert_eq!(original_type, ConstantType::Integer);
            }
            _ => panic!("Expected EncryptedConstant"),
        }
    }

    #[test]
    fn test_encrypt_constant_bool() -> Result<()> {
        let key = b"test-encryption-key";
        let value = CircuitValue::Bool(true);

        let encrypted = encrypt_constant(&value, key)?;
        assert!(!encrypted.is_empty());
        // Type tag should be 0x00 for Bool
        assert_eq!(encrypted[0], 0x00);

        let decrypted = decrypt_constant(&encrypted, key)?;
        assert_eq!(decrypted, value);

        Ok(())
    }

    #[test]
    fn test_encrypt_constant_u8() -> Result<()> {
        let key = b"test-key-u8";
        let value = CircuitValue::U8(42);

        let encrypted = encrypt_constant(&value, key)?;
        assert_eq!(encrypted[0], 0x01); // Type tag for U8

        let decrypted = decrypt_constant(&encrypted, key)?;
        assert_eq!(decrypted, value);

        Ok(())
    }

    #[test]
    fn test_encrypt_constant_u16() -> Result<()> {
        let key = b"test-key-u16";
        let value = CircuitValue::U16(12345);

        let encrypted = encrypt_constant(&value, key)?;
        assert_eq!(encrypted[0], 0x02);

        let decrypted = decrypt_constant(&encrypted, key)?;
        assert_eq!(decrypted, value);

        Ok(())
    }

    #[test]
    fn test_encrypt_constant_u32() -> Result<()> {
        let key = b"test-key-u32";
        let value = CircuitValue::U32(1_000_000);

        let encrypted = encrypt_constant(&value, key)?;
        assert_eq!(encrypted[0], 0x03);

        let decrypted = decrypt_constant(&encrypted, key)?;
        assert_eq!(decrypted, value);

        Ok(())
    }

    #[test]
    fn test_encrypt_constant_u64() -> Result<()> {
        let key = b"test-key-u64";
        let value = CircuitValue::U64(0xDEAD_BEEF_CAFE_BABE);

        let encrypted = encrypt_constant(&value, key)?;
        assert_eq!(encrypted[0], 0x04);

        let decrypted = decrypt_constant(&encrypted, key)?;
        assert_eq!(decrypted, value);

        Ok(())
    }

    #[test]
    fn test_encrypt_constant_wrong_key_produces_wrong_value() -> Result<()> {
        let key1 = b"correct-key";
        let key2 = b"wrong-key!!";
        let value = CircuitValue::U8(42);

        let encrypted = encrypt_constant(&value, key1)?;
        let decrypted = decrypt_constant(&encrypted, key2)?;
        // With the wrong key, we get a different value (XOR-based, so it decrypts but wrongly)
        assert_ne!(decrypted, value);

        Ok(())
    }

    #[test]
    fn test_encrypt_constant_empty_key_error() {
        let key: &[u8] = &[];
        let value = CircuitValue::U8(1);

        let result = encrypt_constant(&value, key);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_constant_empty_data_error() {
        let key = b"some-key";
        let result = decrypt_constant(&[], key);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_circuit_constants_transforms_all() -> Result<()> {
        let builder = CircuitBuilder::new();
        let key = b"circuit-encryption-key";

        // Build: Constant(5u8) + Constant(3u8)
        let a = builder.constant(CircuitValue::U8(5));
        let b = builder.constant(CircuitValue::U8(3));
        let sum = builder.add(a, b);

        // Before encryption: 2 plaintext constants, 0 encrypted
        assert_eq!(count_plaintext_constants(&sum), 2);
        assert_eq!(count_encrypted_constants(&sum), 0);

        // Encrypt
        let encrypted = encrypt_circuit_constants(&sum, key)?;

        // After encryption: 0 plaintext constants, 2 encrypted
        assert_eq!(count_plaintext_constants(&encrypted), 0);
        assert_eq!(count_encrypted_constants(&encrypted), 2);

        // Verify structure is preserved (BinaryOp Add with two EncryptedConstant children)
        match &encrypted {
            CircuitNode::BinaryOp { op, left, right } => {
                assert_eq!(*op, BinaryOperator::Add);
                assert!(matches!(**left, CircuitNode::EncryptedConstant { .. }));
                assert!(matches!(**right, CircuitNode::EncryptedConstant { .. }));
            }
            _ => panic!("Expected BinaryOp after encryption"),
        }

        Ok(())
    }

    #[test]
    fn test_encrypt_circuit_constants_preserves_loads() -> Result<()> {
        let mut builder = CircuitBuilder::new();
        builder.declare_variable("x", EncryptedType::U8);
        let key = b"key-for-loads-test";

        // Build: Load("x") + Constant(10u8)
        let x = builder.load("x");
        let c = builder.constant(CircuitValue::U8(10));
        let sum = builder.add(x, c);

        let encrypted = encrypt_circuit_constants(&sum, key)?;

        // Load should be preserved, Constant should become EncryptedConstant
        match &encrypted {
            CircuitNode::BinaryOp { left, right, .. } => {
                assert!(matches!(**left, CircuitNode::Load(ref name) if name == "x"));
                assert!(matches!(**right, CircuitNode::EncryptedConstant { .. }));
            }
            _ => panic!("Expected BinaryOp"),
        }

        Ok(())
    }

    #[test]
    fn test_encrypt_circuit_constants_already_encrypted_pass_through() -> Result<()> {
        let builder = CircuitBuilder::new();
        let key = b"key-pass-through";

        // Create an already-encrypted constant
        let enc = builder.encrypted_constant(vec![0x01, 0x02, 0x03], ConstantType::Integer);
        let original_data = vec![0x01, 0x02, 0x03];

        let result = encrypt_circuit_constants(&enc, key)?;

        // The encrypted constant should pass through unchanged
        match result {
            CircuitNode::EncryptedConstant {
                data,
                original_type,
            } => {
                assert_eq!(data, original_data);
                assert_eq!(original_type, ConstantType::Integer);
            }
            _ => panic!("Expected EncryptedConstant pass-through"),
        }

        Ok(())
    }

    #[test]
    fn test_encrypted_constant_display() {
        let node = CircuitNode::EncryptedConstant {
            data: vec![0xAA, 0xBB, 0xCC],
            original_type: ConstantType::Boolean,
        };
        let display = format!("{}", node);
        assert!(display.contains("EncryptedConst"));
        assert!(display.contains("boolean"));
        assert!(display.contains("3 bytes"));
    }

    #[test]
    fn test_circuit_node_display_variants() {
        // Load
        let load = CircuitNode::Load("x".to_string());
        assert_eq!(format!("{}", load), "Load(x)");

        // Constant
        let constant = CircuitNode::Constant(CircuitValue::U8(42));
        assert_eq!(format!("{}", constant), "Const(42u8)");

        // Bool constant
        let bool_const = CircuitNode::Constant(CircuitValue::Bool(true));
        assert_eq!(format!("{}", bool_const), "Const(true)");
    }

    #[test]
    fn test_constant_type_display() {
        assert_eq!(format!("{}", ConstantType::Integer), "integer");
        assert_eq!(format!("{}", ConstantType::Boolean), "boolean");
        assert_eq!(format!("{}", ConstantType::Float), "float");
        assert_eq!(format!("{}", ConstantType::Bytes), "bytes");
    }

    #[test]
    fn test_constant_type_variants() {
        // Verify all variants exist and are distinct
        let variants = [
            ConstantType::Integer,
            ConstantType::Boolean,
            ConstantType::Float,
            ConstantType::Bytes,
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn test_constant_type_serialization_roundtrip() -> Result<()> {
        let types = [
            ConstantType::Integer,
            ConstantType::Boolean,
            ConstantType::Float,
            ConstantType::Bytes,
        ];

        for ct in &types {
            let json = serde_json::to_string(ct).map_err(|e| {
                AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Serialization failed: {}",
                    e
                )))
            })?;
            let deserialized: ConstantType = serde_json::from_str(&json).map_err(|e| {
                AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Deserialization failed: {}",
                    e
                )))
            })?;
            assert_eq!(*ct, deserialized);
        }

        Ok(())
    }

    #[test]
    fn test_is_encrypted_constant() {
        let enc = CircuitNode::EncryptedConstant {
            data: vec![1, 2, 3],
            original_type: ConstantType::Integer,
        };
        assert!(is_encrypted_constant(&enc));

        let plain = CircuitNode::Constant(CircuitValue::U8(5));
        assert!(!is_encrypted_constant(&plain));

        let load = CircuitNode::Load("x".to_string());
        assert!(!is_encrypted_constant(&load));
    }

    #[test]
    fn test_encrypted_constant_in_circuit_validation() -> Result<()> {
        // EncryptedConstant should pass validation
        let enc = CircuitNode::EncryptedConstant {
            data: vec![0x00, 0x01],
            original_type: ConstantType::Boolean,
        };
        let circuit = Circuit::new(enc, HashMap::new())?;
        circuit.validate()?;
        // EncryptedConstant with Boolean type infers to Bool
        assert_eq!(circuit.result_type, EncryptedType::Bool);
        Ok(())
    }

    #[test]
    fn test_encrypted_constant_depth_and_gate_count() -> Result<()> {
        let builder = CircuitBuilder::new();

        // EncryptedConstant has depth 1 and gate count 0 (same as Constant/Load)
        let enc = builder.encrypted_constant(vec![0x01, 0x42], ConstantType::Integer);
        let circuit = Circuit::new(enc, HashMap::new())?;
        assert_eq!(circuit.depth, 1);
        assert_eq!(circuit.gate_count, 0);

        Ok(())
    }

    #[test]
    fn test_mixed_plain_and_encrypted_constants() -> Result<()> {
        let builder = CircuitBuilder::new();
        let key = b"mixed-circuit-key";

        // Build a circuit with both plaintext and encrypted constants
        let plain = builder.constant(CircuitValue::U8(10));
        let encrypted_data = encrypt_constant(&CircuitValue::U8(20), key)?;
        let enc = builder.encrypted_constant(encrypted_data, ConstantType::Integer);

        // In a real circuit, these would need compatible types. Here we just
        // verify counting works on a mixed tree.
        let not_node = CircuitNode::UnaryOp {
            op: UnaryOperator::Not,
            operand: Box::new(CircuitNode::Constant(CircuitValue::Bool(true))),
        };

        // Build a dummy tree with both types
        // (plain + enc) is not directly buildable due to type mismatch in
        // a strict sense, so let's just check counting on a flat structure.
        assert_eq!(count_plaintext_constants(&plain), 1);
        assert_eq!(count_encrypted_constants(&plain), 0);
        assert_eq!(count_plaintext_constants(&enc), 0);
        assert_eq!(count_encrypted_constants(&enc), 1);
        assert_eq!(count_plaintext_constants(&not_node), 1);
        assert_eq!(count_encrypted_constants(&not_node), 0);

        Ok(())
    }

    #[test]
    fn test_encrypt_constant_deterministic() -> Result<()> {
        let key = b"deterministic-test-key";
        let value = CircuitValue::U32(999);

        let enc1 = encrypt_constant(&value, key)?;
        let enc2 = encrypt_constant(&value, key)?;
        // Same key + same value => same ciphertext (deterministic)
        assert_eq!(enc1, enc2);

        Ok(())
    }

    #[test]
    fn test_encrypt_constant_different_keys_differ() -> Result<()> {
        let key1 = b"key-alpha";
        let key2 = b"key-bravo";
        let value = CircuitValue::U64(123456789);

        let enc1 = encrypt_constant(&value, key1)?;
        let enc2 = encrypt_constant(&value, key2)?;
        // Different keys should produce different ciphertext (with high probability)
        // Type tag (first byte) is the same, but payload differs
        assert_eq!(enc1[0], enc2[0]); // Same type tag
        assert_ne!(enc1[1..], enc2[1..]); // Different payload

        Ok(())
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip_all_types() -> Result<()> {
        let key = b"roundtrip-all-types";

        let values = vec![
            CircuitValue::Bool(false),
            CircuitValue::Bool(true),
            CircuitValue::U8(0),
            CircuitValue::U8(255),
            CircuitValue::U16(0),
            CircuitValue::U16(65535),
            CircuitValue::U32(0),
            CircuitValue::U32(u32::MAX),
            CircuitValue::U64(0),
            CircuitValue::U64(u64::MAX),
        ];

        for value in &values {
            let encrypted = encrypt_constant(value, key)?;
            let decrypted = decrypt_constant(&encrypted, key)?;
            assert_eq!(*value, decrypted, "Roundtrip failed for {:?}", value);
        }

        Ok(())
    }

    #[test]
    fn test_encrypt_circuit_constants_nested() -> Result<()> {
        let builder = CircuitBuilder::new();
        let key = b"nested-circuit-key";

        // Build: NOT(Constant(true) AND Constant(false))
        let t = builder.constant(CircuitValue::Bool(true));
        let f = builder.constant(CircuitValue::Bool(false));
        let and_node = builder.and(t, f);
        let not_node = builder.not(and_node);

        assert_eq!(count_plaintext_constants(&not_node), 2);
        assert_eq!(count_encrypted_constants(&not_node), 0);

        let encrypted = encrypt_circuit_constants(&not_node, key)?;

        assert_eq!(count_plaintext_constants(&encrypted), 0);
        assert_eq!(count_encrypted_constants(&encrypted), 2);

        // Verify structure: NOT(AND(EncryptedConstant, EncryptedConstant))
        match &encrypted {
            CircuitNode::UnaryOp { op, operand } => {
                assert_eq!(*op, UnaryOperator::Not);
                match operand.as_ref() {
                    CircuitNode::BinaryOp { op, left, right } => {
                        assert_eq!(*op, BinaryOperator::And);
                        assert!(is_encrypted_constant(left));
                        assert!(is_encrypted_constant(right));
                    }
                    _ => panic!("Expected BinaryOp inside UnaryOp"),
                }
            }
            _ => panic!("Expected UnaryOp at root"),
        }

        Ok(())
    }
}
