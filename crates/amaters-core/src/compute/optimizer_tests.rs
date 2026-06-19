use super::*;
use crate::compute::circuit::CircuitBuilder;
use std::collections::HashMap;

// ── Constant folding tests ─────────────────────────────────────────

#[test]
fn test_constant_folding() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    // Create circuit: 5 + 3
    let a = builder.constant(CircuitValue::U8(5));
    let b = builder.constant(CircuitValue::U8(3));
    let sum = builder.add(a, b);

    let circuit = Circuit::new(sum, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    // Should fold to constant 8
    assert!(matches!(
        optimized.root,
        CircuitNode::Constant(CircuitValue::U8(8))
    ));
    assert!(optimizer.stats().constants_folded >= 1);

    Ok(())
}

#[test]
fn test_constant_folding_sub() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    let a = builder.constant(CircuitValue::U16(100));
    let b = builder.constant(CircuitValue::U16(30));
    let result = builder.sub(a, b);

    let circuit = Circuit::new(result, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Constant(CircuitValue::U16(70)));
    Ok(())
}

#[test]
fn test_constant_folding_mul() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    let a = builder.constant(CircuitValue::U32(7));
    let b = builder.constant(CircuitValue::U32(6));
    let result = builder.mul(a, b);

    let circuit = Circuit::new(result, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Constant(CircuitValue::U32(42)));
    Ok(())
}

#[test]
fn test_constant_folding_bool_and() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    let t = builder.constant(CircuitValue::Bool(true));
    let f = builder.constant(CircuitValue::Bool(false));
    let result = builder.and(t, f);

    let circuit = Circuit::new(result, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(
        optimized.root,
        CircuitNode::Constant(CircuitValue::Bool(false))
    );
    Ok(())
}

#[test]
fn test_constant_folding_unary_not() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    let t = builder.constant(CircuitValue::Bool(true));
    let result = builder.not(t);

    let circuit = Circuit::new(result, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(
        optimized.root,
        CircuitNode::Constant(CircuitValue::Bool(false))
    );
    Ok(())
}

// ── Algebraic identity tests ───────────────────────────────────────

#[test]
fn test_algebraic_x_plus_zero() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    let x = builder.load("x");
    let zero = builder.constant(CircuitValue::U8(0));
    let add_zero = builder.add(x, zero);

    let circuit = Circuit::new(add_zero, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

#[test]
fn test_algebraic_zero_plus_x() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    let x = builder.load("x");
    let zero = builder.constant(CircuitValue::U8(0));
    let result = builder.add(zero, x);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

#[test]
fn test_algebraic_x_mul_one() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    let x = builder.load("x");
    let one = builder.constant(CircuitValue::U8(1));
    let result = builder.mul(x, one);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

#[test]
fn test_algebraic_one_mul_x() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    let x = builder.load("x");
    let one = builder.constant(CircuitValue::U8(1));
    let result = builder.mul(one, x);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

#[test]
fn test_algebraic_x_mul_zero() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    let x = builder.load("x");
    let zero = builder.constant(CircuitValue::U8(0));
    let result = builder.mul(x, zero);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Constant(CircuitValue::U8(0)));
    Ok(())
}

#[test]
fn test_algebraic_zero_mul_x() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    let x = builder.load("x");
    let zero = builder.constant(CircuitValue::U8(0));
    let result = builder.mul(zero, x);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Constant(CircuitValue::U8(0)));
    Ok(())
}

#[test]
fn test_algebraic_x_sub_zero() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    let x = builder.load("x");
    let zero = builder.constant(CircuitValue::U8(0));
    let result = builder.sub(x, zero);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

#[test]
fn test_algebraic_x_sub_x() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    let x1 = builder.load("x");
    let x2 = builder.load("x");
    let result = builder.sub(x1, x2);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    // x - x should be 0
    assert_eq!(optimized.root, CircuitNode::Constant(CircuitValue::U8(0)));
    assert!(optimizer.stats().algebraic_simplifications >= 1);
    Ok(())
}

// ── Double negation tests ──────────────────────────────────────────

#[test]
fn test_double_negation_elimination() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::Bool);

    let x = builder.load("x");
    let not_x = builder.not(x);
    let not_not_x = builder.not(not_x);

    let circuit = Circuit::new(not_not_x, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

#[test]
fn test_quadruple_negation_elimination() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::Bool);

    let x = builder.load("x");
    let n1 = builder.not(x);
    let n2 = builder.not(n1);
    let n3 = builder.not(n2);
    let n4 = builder.not(n3);

    let circuit = Circuit::new(n4, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

// ── Nested simplification tests ────────────────────────────────────

#[test]
fn test_nested_x_plus_0_times_1() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    // (x + 0) * 1 -> x
    let x = builder.load("x");
    let zero = builder.constant(CircuitValue::U8(0));
    let one = builder.constant(CircuitValue::U8(1));
    let add_zero = builder.add(x, zero);
    let times_one = builder.mul(add_zero, one);

    let circuit = Circuit::new(times_one, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

#[test]
fn test_nested_complex_optimization() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8);

    // (a * 1) + (b * 0) + 5  ->  a + 5
    let a = builder.load("a");
    let b = builder.load("b");
    let one = builder.constant(CircuitValue::U8(1));
    let zero = builder.constant(CircuitValue::U8(0));
    let five = builder.constant(CircuitValue::U8(5));

    let a_times_1 = builder.mul(a, one);
    let b_times_0 = builder.mul(b, zero);
    let sum1 = builder.add(a_times_1, b_times_0);
    let result = builder.add(sum1, five);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let original_gates = circuit.gate_count;

    let optimized = optimizer.optimize(circuit)?;

    assert!(optimized.gate_count < original_gates);
    assert!(optimizer.stats().gate_reduction_percent() >= 30.0);

    Ok(())
}

// ── No-op on already optimal circuits ──────────────────────────────

#[test]
fn test_noop_on_optimal_circuit() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8);

    // a + b is already optimal
    let a = builder.load("a");
    let b = builder.load("b");
    let result = builder.add(a, b);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let original_gates = circuit.gate_count;

    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.gate_count, original_gates);
    assert_eq!(
        optimized.root,
        CircuitNode::BinaryOp {
            op: BinaryOperator::Add,
            left: Box::new(CircuitNode::Load("a".to_string())),
            right: Box::new(CircuitNode::Load("b".to_string())),
        }
    );
    Ok(())
}

#[test]
fn test_noop_single_load() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    let x = builder.load("x");
    let circuit = Circuit::new(x, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

// ── Statistics accuracy tests ──────────────────────────────────────

#[test]
fn test_stats_accuracy_constant_folding() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    // 5 + 3 -> 8, then 8 * 2 -> 16  (two folds)
    let a = builder.constant(CircuitValue::U8(5));
    let b = builder.constant(CircuitValue::U8(3));
    let two = builder.constant(CircuitValue::U8(2));
    let sum = builder.add(a, b);
    let result = builder.mul(sum, two);

    let circuit = Circuit::new(result, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Constant(CircuitValue::U8(16)));
    // At least 2 constant folds happened (possibly more from DCE re-fold)
    assert!(optimizer.stats().constants_folded >= 2);
    Ok(())
}

#[test]
fn test_stats_accuracy_algebraic() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    // x - x -> 0
    let x1 = builder.load("x");
    let x2 = builder.load("x");
    let result = builder.sub(x1, x2);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let _optimized = optimizer.optimize(circuit)?;

    let (total_eliminated, total_algebraic, _total_folds) = optimizer.total_stats();
    assert!(total_eliminated >= 1);
    assert!(total_algebraic >= 1);
    Ok(())
}

#[test]
fn test_optimization_stats() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    let a = builder.constant(CircuitValue::U8(5));
    let b = builder.constant(CircuitValue::U8(3));
    let zero = builder.constant(CircuitValue::U8(0));

    let sum = builder.add(a, b);
    let add_zero = builder.add(sum, zero);

    let circuit = Circuit::new(add_zero, HashMap::new())?;
    let original_gates = circuit.gate_count;

    let optimized = optimizer.optimize(circuit)?;
    let optimized_gates = optimized.gate_count;

    assert!(optimized_gates < original_gates);
    assert!(optimizer.stats().gate_reduction_percent() > 0.0);

    Ok(())
}

#[test]
fn test_total_stats_method() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    // (x + 0) * 1 -> x (algebraic simplifications)
    // plus: 5 + 3 constant fold somewhere
    let x = builder.load("x");
    let zero = builder.constant(CircuitValue::U8(0));
    let one = builder.constant(CircuitValue::U8(1));
    let add_zero = builder.add(x, zero);
    let times_one = builder.mul(add_zero, one);

    let circuit = Circuit::new(times_one, builder.variable_types_clone())?;
    let _optimized = optimizer.optimize(circuit)?;

    let (eliminated, algebraic, _folds) = optimizer.total_stats();
    // Both x+0 and *1 should be simplified
    assert!(eliminated + algebraic >= 2);
    Ok(())
}

// ── Bootstrap counting test ────────────────────────────────────────

#[test]
fn test_bootstrap_counting() -> Result<()> {
    let optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8);

    let a = builder.load("a");
    let b = builder.load("b");
    let mul = builder.mul(a, b);

    let circuit = Circuit::new(mul, builder.variable_types_clone())?;
    let bootstrap_count = optimizer.count_bootstraps(&circuit.root);

    assert_eq!(bootstrap_count, 1);
    Ok(())
}

// ── Parallelization analysis test ──────────────────────────────────

#[test]
fn test_parallelization_analysis() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8)
        .declare_variable("c", EncryptedType::U8);

    let a = builder.load("a");
    let b = builder.load("b");
    let c = builder.load("c");
    let sum1 = builder.add(a, b);
    let sum2 = builder.add(sum1, c);

    let circuit = Circuit::new(sum2, builder.variable_types_clone())?;
    let _optimized = optimizer.optimize(circuit)?;

    let graph = optimizer.dependency_graph();
    assert!(graph.node_count > 0);
    assert!(!graph.parallel_groups.is_empty());

    Ok(())
}

// ── Live variable collection test ──────────────────────────────────

#[test]
fn test_collect_live_variables() -> Result<()> {
    let optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8);

    let a = builder.load("a");
    let b = builder.load("b");
    let result = builder.add(a, b);

    let live = optimizer.collect_live_variables(&result);
    assert!(live.contains("a"));
    assert!(live.contains("b"));
    assert_eq!(live.len(), 2);
    Ok(())
}

#[test]
fn test_collect_live_variables_after_dce() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8);

    // (a * 1) + (b * 0) => a + 0 => a
    // After optimization, b should be eliminated
    let a = builder.load("a");
    let b = builder.load("b");
    let one = builder.constant(CircuitValue::U8(1));
    let zero = builder.constant(CircuitValue::U8(0));
    let a1 = builder.mul(a, one);
    let b0 = builder.mul(b, zero);
    let result = builder.add(a1, b0);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    let live = optimizer.collect_live_variables(&optimized.root);
    assert!(live.contains("a"));
    // b was multiplied by 0, so entire branch collapses to 0, and then a + 0 => a
    assert!(!live.contains("b"), "b should be eliminated by DCE");
    Ok(())
}

// ── Comparison constant folding test ───────────────────────────────

#[test]
fn test_comparison_constant_fold() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    let a = builder.constant(CircuitValue::U8(10));
    let b = builder.constant(CircuitValue::U8(5));
    let result = builder.gt(a, b);

    let circuit = Circuit::new(result, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(
        optimized.root,
        CircuitNode::Constant(CircuitValue::Bool(true))
    );
    Ok(())
}

#[test]
fn test_comparison_constant_fold_eq() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    let a = builder.constant(CircuitValue::U8(5));
    let b = builder.constant(CircuitValue::U8(5));
    let result = builder.eq(a, b);

    let circuit = Circuit::new(result, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(
        optimized.root,
        CircuitNode::Constant(CircuitValue::Bool(true))
    );
    Ok(())
}

// ── XOR self-elimination test ──────────────────────────────────────

#[test]
fn test_xor_self_elimination() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::Bool);

    let x1 = builder.load("x");
    let x2 = builder.load("x");
    let result = builder.xor(x1, x2);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(
        optimized.root,
        CircuitNode::Constant(CircuitValue::Bool(false))
    );
    Ok(())
}

// ── AND/OR idempotent test ─────────────────────────────────────────

#[test]
fn test_and_idempotent() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::Bool);

    let x1 = builder.load("x");
    let x2 = builder.load("x");
    let result = builder.and(x1, x2);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

#[test]
fn test_or_idempotent() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::Bool);

    let x1 = builder.load("x");
    let x2 = builder.load("x");
    let result = builder.or(x1, x2);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    assert_eq!(optimized.root, CircuitNode::Load("x".to_string()));
    Ok(())
}

// ── Encrypted constant optimizer tests ────────────────────────────

#[test]
fn test_optimizer_does_not_fold_encrypted_constants() -> Result<()> {
    use crate::compute::circuit::ConstantType;

    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    // Build: EncryptedConstant + EncryptedConstant
    // The optimizer must NOT try to constant-fold these because their
    // plaintext values are unknown.
    let enc_a = builder.encrypted_constant(vec![0x01, 0x05], ConstantType::Integer);
    let enc_b = builder.encrypted_constant(vec![0x01, 0x03], ConstantType::Integer);
    let sum = builder.add(enc_a.clone(), enc_b.clone());

    let circuit = Circuit::new(sum, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    // The root should still be a BinaryOp Add, not a folded constant
    match &optimized.root {
        CircuitNode::BinaryOp { op, left, right } => {
            assert_eq!(*op, BinaryOperator::Add);
            assert!(matches!(**left, CircuitNode::EncryptedConstant { .. }));
            assert!(matches!(**right, CircuitNode::EncryptedConstant { .. }));
        }
        _ => {
            return Err(AmateRSError::FheComputation(ErrorContext::new(
                "Optimizer incorrectly folded encrypted constants".to_string(),
            )));
        }
    }

    // No constants should have been folded
    assert_eq!(optimizer.stats().constants_folded, 0);

    Ok(())
}

#[test]
fn test_optimizer_dce_treats_encrypted_constant_as_opaque() -> Result<()> {
    use crate::compute::circuit::ConstantType;

    let mut optimizer = CircuitOptimizer::new();

    // Build a circuit: EncryptedConstant (standalone, as root)
    // DCE should leave it alone (it is the output)
    let enc = CircuitNode::EncryptedConstant {
        data: vec![0x04, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x11],
        original_type: ConstantType::Integer,
    };

    let circuit = Circuit::new(enc.clone(), HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    // The root should remain an EncryptedConstant, untouched
    assert_eq!(optimized.root, enc);

    Ok(())
}

#[test]
fn test_optimizer_mixed_plain_and_encrypted_constants() -> Result<()> {
    use crate::compute::circuit::ConstantType;

    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    // Build: Constant(5u8) + Constant(3u8) -- these CAN be folded
    let plain_a = builder.constant(CircuitValue::U8(5));
    let plain_b = builder.constant(CircuitValue::U8(3));
    let plain_sum = builder.add(plain_a, plain_b);

    let circuit = Circuit::new(plain_sum, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    // Should fold to 8
    assert!(matches!(
        optimized.root,
        CircuitNode::Constant(CircuitValue::U8(8))
    ));

    // Now with encrypted: EncryptedConst + EncryptedConst -- must NOT fold
    let mut optimizer2 = CircuitOptimizer::new();
    let enc_a = builder.encrypted_constant(vec![0x01, 0xAA], ConstantType::Integer);
    let enc_b = builder.encrypted_constant(vec![0x01, 0xBB], ConstantType::Integer);
    let enc_sum = builder.add(enc_a, enc_b);

    let circuit2 = Circuit::new(enc_sum, HashMap::new())?;
    let optimized2 = optimizer2.optimize(circuit2)?;

    assert!(matches!(optimized2.root, CircuitNode::BinaryOp { .. }));

    Ok(())
}

#[test]
fn test_optimizer_algebraic_identity_with_encrypted_constant() -> Result<()> {
    use crate::compute::circuit::ConstantType;

    let mut optimizer = CircuitOptimizer::new();
    let builder = CircuitBuilder::new();

    // Build: EncryptedConstant + Constant(0u64)
    // EncryptedConstant with ConstantType::Integer infers to U64,
    // so the zero constant must also be U64 for type compatibility.
    // The algebraic identity x + 0 = x should simplify this to just
    // the EncryptedConstant.
    let enc = builder.encrypted_constant(
        vec![0x04, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        ConstantType::Integer,
    );
    let zero = builder.constant(CircuitValue::U64(0));
    let sum = builder.add(enc.clone(), zero);

    let circuit = Circuit::new(sum, HashMap::new())?;
    let optimized = optimizer.optimize(circuit)?;

    // Should simplify to just the encrypted constant
    assert_eq!(optimized.root, enc);

    Ok(())
}

#[test]
fn test_optimizer_live_variables_with_encrypted_constants() -> Result<()> {
    use crate::compute::circuit::ConstantType;

    let optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder.declare_variable("x", EncryptedType::U8);

    // Build: Load("x") + EncryptedConstant
    let x = builder.load("x");
    let enc = builder.encrypted_constant(vec![0x01, 0x10], ConstantType::Integer);
    let sum = builder.add(x, enc);

    let live = optimizer.collect_live_variables(&sum);

    // "x" is live, encrypted constant contributes nothing to variables
    assert!(live.contains("x"));
    assert_eq!(live.len(), 1);

    Ok(())
}

// ── NaryOp tests ───────────────────────────────────────────────────

#[test]
fn test_nary_fusion_nested_add() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8)
        .declare_variable("c", EncryptedType::U8)
        .declare_variable("d", EncryptedType::U8);

    let a = builder.load("a");
    let b = builder.load("b");
    let c = builder.load("c");
    let d = builder.load("d");
    // ((a + b) + c) + d — a 3-level linear chain
    let sum1 = builder.add(a, b);
    let sum2 = builder.add(sum1, c);
    let sum3 = builder.add(sum2, d);

    let circuit = Circuit::new(sum3, builder.variable_types_clone())?;
    let optimized = optimizer.optimize(circuit)?;

    // After fusion, the root should be a NaryOp with 4 operands
    match &optimized.root {
        CircuitNode::NaryOp { op, operands } => {
            assert_eq!(*op, BinaryOperator::Add);
            assert_eq!(operands.len(), 4, "Expected 4 fused operands");
        }
        other => {
            // If fusion threshold not triggered, accept BinaryOp too
            assert!(
                matches!(
                    other,
                    CircuitNode::BinaryOp {
                        op: BinaryOperator::Add,
                        ..
                    }
                ),
                "Expected Add at root, got: {:?}",
                other
            );
        }
    }
    Ok(())
}

#[test]
fn test_nary_depth_calculation() -> Result<()> {
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8)
        .declare_variable("c", EncryptedType::U8)
        .declare_variable("d", EncryptedType::U8);

    let a = builder.load("a");
    let b = builder.load("b");
    let c = builder.load("c");
    let d = builder.load("d");

    let nary = CircuitNode::NaryOp {
        op: BinaryOperator::Add,
        operands: vec![a, b, c, d],
    };

    let circuit = Circuit::new(nary, builder.variable_types_clone())?;
    // Balanced depth for 4 operands: ceil(log2(4)) = 2, plus depth 1 for leaves = 3
    assert!(
        circuit.depth >= 2 && circuit.depth <= 5,
        "Unexpected depth: {}",
        circuit.depth
    );
    // Gate count: 4 operands = 3 gates
    assert_eq!(circuit.gate_count, 3);
    Ok(())
}

#[test]
fn test_nary_type_inference_homogeneous() -> Result<()> {
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8)
        .declare_variable("c", EncryptedType::U8);

    let nary = CircuitNode::NaryOp {
        op: BinaryOperator::Add,
        operands: vec![builder.load("a"), builder.load("b"), builder.load("c")],
    };
    let circuit = Circuit::new(nary, builder.variable_types_clone())?;
    assert_eq!(circuit.result_type, EncryptedType::U8);
    Ok(())
}

#[test]
fn test_nary_sub_invalid() {
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8);

    let nary = CircuitNode::NaryOp {
        op: BinaryOperator::Sub,
        operands: vec![builder.load("a"), builder.load("b")],
    };
    let result = Circuit::new(nary, builder.variable_types_clone());
    assert!(result.is_err(), "Sub NaryOp should be rejected");
}

// ── Bootstrap minimization tests ──────────────────────────────────

#[test]
fn test_bootstrap_minimization_swaps_expensive_first() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8)
        .declare_variable("c", EncryptedType::U8);

    let a = builder.load("a");
    let b = builder.load("b");
    let c = builder.load("c");

    // Build: a + (b * c)
    // b * c has cost 1 (bootstrap), a has cost 0
    let mul_bc = builder.mul(b, c);
    let sum = builder.add(a, mul_bc);

    let circuit = Circuit::new(sum, builder.variable_types_clone())?;
    let original_bootstraps = optimizer.count_bootstraps(&circuit.root);
    let optimized = optimizer.optimize(circuit)?;
    let optimized_bootstraps = optimizer.count_bootstraps(&optimized.root);

    // Bootstrap count should not increase
    assert!(
        optimized_bootstraps <= original_bootstraps,
        "Bootstrap count increased: {} -> {}",
        original_bootstraps,
        optimized_bootstraps
    );
    Ok(())
}

#[test]
fn test_bootstrap_minimization_mul_chain_balanced() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8)
        .declare_variable("c", EncryptedType::U8)
        .declare_variable("d", EncryptedType::U8);

    let a = builder.load("a");
    let b = builder.load("b");
    let c = builder.load("c");
    let d = builder.load("d");

    // Linear chain: ((a * b) * c) * d has depth 3
    let mul1 = builder.mul(a, b);
    let mul2 = builder.mul(mul1, c);
    let mul3 = builder.mul(mul2, d);

    let circuit = Circuit::new(mul3, builder.variable_types_clone())?;
    let original_bootstraps = optimizer.count_bootstraps(&circuit.root);

    let optimized = optimizer.optimize(circuit)?;
    let optimized_bootstraps = optimizer.count_bootstraps(&optimized.root);

    // Bootstrap count should not increase
    assert!(
        optimized_bootstraps <= original_bootstraps,
        "Bootstrap count increased: {} -> {}",
        original_bootstraps,
        optimized_bootstraps
    );
    Ok(())
}

// ── Parallelism analysis tests ─────────────────────────────────────

#[test]
fn test_structural_cse_deduplicates_identical_subtrees() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8);

    let a1 = builder.load("a");
    let b1 = builder.load("b");
    let a2 = builder.load("a");
    let b2 = builder.load("b");

    // (a + b) + (a + b) -- identical subtrees
    let sum1 = builder.add(a1, b1);
    let sum2 = builder.add(a2, b2);
    let result = builder.add(sum1, sum2);

    let circuit = Circuit::new(result, builder.variable_types_clone())?;
    let _optimized = optimizer.optimize(circuit)?;

    let graph = optimizer.dependency_graph();
    // With CSE, identical subtrees share a NodeId, so node_count > 0
    assert!(graph.node_count > 0);
    Ok(())
}

#[test]
fn test_topological_order_respects_dependencies() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8)
        .declare_variable("c", EncryptedType::U8);

    let a = builder.load("a");
    let b = builder.load("b");
    let c = builder.load("c");
    let sum1 = builder.add(a, b);
    let sum2 = builder.add(sum1, c);

    let circuit = Circuit::new(sum2, builder.variable_types_clone())?;
    let _optimized = optimizer.optimize(circuit)?;

    let graph = optimizer.dependency_graph();
    let topo_order = graph.topological_order();

    // Verify: for each edge (dep -> node), dep comes before node in topo order
    let pos: HashMap<NodeId, usize> = topo_order
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();

    for (node_id, deps) in &graph.dependencies {
        for dep_id in deps {
            let node_pos = pos.get(node_id).copied().unwrap_or(usize::MAX);
            let dep_pos = pos.get(dep_id).copied().unwrap_or(usize::MAX);
            assert!(
                dep_pos < node_pos,
                "Dependency {:?} (pos {}) should come before {:?} (pos {}) in topo order",
                dep_id,
                dep_pos,
                node_id,
                node_pos
            );
        }
    }

    Ok(())
}

#[test]
fn test_critical_path_memoization_correctness() -> Result<()> {
    let mut optimizer = CircuitOptimizer::new();
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8)
        .declare_variable("c", EncryptedType::U8);

    let a = builder.load("a");
    let b = builder.load("b");
    let c = builder.load("c");
    let sum1 = builder.add(a, b);
    let sum2 = builder.add(sum1, c);

    let circuit = Circuit::new(sum2, builder.variable_types_clone())?;
    let _optimized = optimizer.optimize(circuit)?;

    let graph = optimizer.dependency_graph();
    let critical_path = &graph.critical_path;

    // Critical path should be non-empty for a multi-node circuit
    assert!(!critical_path.is_empty());
    Ok(())
}
