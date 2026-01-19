//! FHE (Fully Homomorphic Encryption) benchmarks for AmateRS
//!
//! Benchmarks encryption, circuit execution, and predicate compilation.

#![cfg(feature = "compute")]

use amaters_core::compute::{
    CircuitBuilder, EncryptedBool, EncryptedType, EncryptedU8, EncryptedU16, EncryptedU32,
    EncryptedU64, FheExecutor, FheKeyPair, PredicateCompiler,
};
use amaters_core::types::{CipherBlob, Predicate};
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::collections::HashMap;

/// Benchmark FHE encryption operations
fn bench_fhe_encrypt(c: &mut Criterion) {
    let mut group = c.benchmark_group("fhe_encrypt");

    // Generate keys once
    let keypair = FheKeyPair::generate().expect("failed to generate FHE keys");
    keypair.set_as_global_server_key();

    // Boolean encryption
    group.bench_function("bool", |bencher| {
        bencher.iter(|| {
            black_box(EncryptedBool::encrypt(true, keypair.client_key()));
        });
    });

    // U8 encryption
    group.bench_function("u8", |bencher| {
        bencher.iter(|| {
            black_box(EncryptedU8::encrypt(42, keypair.client_key()));
        });
    });

    // U16 encryption
    group.bench_function("u16", |bencher| {
        bencher.iter(|| {
            black_box(EncryptedU16::encrypt(1000, keypair.client_key()));
        });
    });

    // U32 encryption
    group.bench_function("u32", |bencher| {
        bencher.iter(|| {
            black_box(EncryptedU32::encrypt(100000, keypair.client_key()));
        });
    });

    // U64 encryption
    group.bench_function("u64", |bencher| {
        bencher.iter(|| {
            black_box(EncryptedU64::encrypt(1000000000, keypair.client_key()));
        });
    });

    group.finish();
}

/// Benchmark FHE decryption operations
fn bench_fhe_decrypt(c: &mut Criterion) {
    let mut group = c.benchmark_group("fhe_decrypt");

    let keypair = FheKeyPair::generate().expect("failed to generate FHE keys");
    keypair.set_as_global_server_key();

    // Boolean decryption
    group.bench_function("bool", |bencher| {
        let encrypted = EncryptedBool::encrypt(true, keypair.client_key());
        bencher.iter(|| {
            black_box(encrypted.decrypt(keypair.client_key()));
        });
    });

    // U8 decryption
    group.bench_function("u8", |bencher| {
        let encrypted = EncryptedU8::encrypt(42, keypair.client_key());
        bencher.iter(|| {
            black_box(encrypted.decrypt(keypair.client_key()));
        });
    });

    // U16 decryption
    group.bench_function("u16", |bencher| {
        let encrypted = EncryptedU16::encrypt(1000, keypair.client_key());
        bencher.iter(|| {
            black_box(encrypted.decrypt(keypair.client_key()));
        });
    });

    // U32 decryption
    group.bench_function("u32", |bencher| {
        let encrypted = EncryptedU32::encrypt(100000, keypair.client_key());
        bencher.iter(|| {
            black_box(encrypted.decrypt(keypair.client_key()));
        });
    });

    // U64 decryption
    group.bench_function("u64", |bencher| {
        let encrypted = EncryptedU64::encrypt(1000000000, keypair.client_key());
        bencher.iter(|| {
            black_box(encrypted.decrypt(keypair.client_key()));
        });
    });

    group.finish();
}

/// Benchmark FHE circuit execution with simple predicates
fn bench_fhe_circuit_simple(c: &mut Criterion) {
    let mut group = c.benchmark_group("fhe_circuit_simple");

    let keypair = FheKeyPair::generate().expect("failed to generate FHE keys");
    keypair.set_as_global_server_key();

    // Equality check (Eq)
    group.bench_function("eq_u8", |bencher| {
        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("a", EncryptedType::U8)
            .declare_variable("b", EncryptedType::U8);

        let a_node = builder.load("a");
        let b_node = builder.load("b");
        let eq_node = builder.eq(a_node, b_node);
        let circuit = builder.build(eq_node).expect("failed to build circuit");

        let a = EncryptedU8::encrypt(5, keypair.client_key());
        let b_val = EncryptedU8::encrypt(5, keypair.client_key());

        let mut inputs = HashMap::new();
        inputs.insert(
            "a".to_string(),
            a.to_cipher_blob().expect("failed to convert to CipherBlob"),
        );
        inputs.insert(
            "b".to_string(),
            b_val
                .to_cipher_blob()
                .expect("failed to convert to CipherBlob"),
        );

        let executor = FheExecutor::new();

        bencher.iter(|| {
            black_box(
                executor
                    .execute(&circuit, &inputs)
                    .expect("failed to execute circuit"),
            );
        });
    });

    // Greater than check (Gt)
    group.bench_function("gt_u8", |bencher| {
        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("a", EncryptedType::U8)
            .declare_variable("b", EncryptedType::U8);

        let a_node = builder.load("a");
        let b_node = builder.load("b");
        let gt_node = builder.gt(a_node, b_node);
        let circuit = builder.build(gt_node).expect("failed to build circuit");

        let a = EncryptedU8::encrypt(10, keypair.client_key());
        let b_val = EncryptedU8::encrypt(5, keypair.client_key());

        let mut inputs = HashMap::new();
        inputs.insert(
            "a".to_string(),
            a.to_cipher_blob().expect("failed to convert to CipherBlob"),
        );
        inputs.insert(
            "b".to_string(),
            b_val
                .to_cipher_blob()
                .expect("failed to convert to CipherBlob"),
        );

        let executor = FheExecutor::new();

        bencher.iter(|| {
            black_box(
                executor
                    .execute(&circuit, &inputs)
                    .expect("failed to execute circuit"),
            );
        });
    });

    // Arithmetic addition
    group.bench_function("add_u8", |bencher| {
        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("a", EncryptedType::U8)
            .declare_variable("b", EncryptedType::U8);

        let a_node = builder.load("a");
        let b_node = builder.load("b");
        let sum_node = builder.add(a_node, b_node);
        let circuit = builder.build(sum_node).expect("failed to build circuit");

        let a = EncryptedU8::encrypt(5, keypair.client_key());
        let b_val = EncryptedU8::encrypt(3, keypair.client_key());

        let mut inputs = HashMap::new();
        inputs.insert(
            "a".to_string(),
            a.to_cipher_blob().expect("failed to convert to CipherBlob"),
        );
        inputs.insert(
            "b".to_string(),
            b_val
                .to_cipher_blob()
                .expect("failed to convert to CipherBlob"),
        );

        let executor = FheExecutor::new();

        bencher.iter(|| {
            black_box(
                executor
                    .execute(&circuit, &inputs)
                    .expect("failed to execute circuit"),
            );
        });
    });

    group.finish();
}

/// Benchmark FHE circuit execution with complex predicates
fn bench_fhe_circuit_complex(c: &mut Criterion) {
    let mut group = c.benchmark_group("fhe_circuit_complex");

    let keypair = FheKeyPair::generate().expect("failed to generate FHE keys");
    keypair.set_as_global_server_key();

    // AND of two comparisons: (a > b) AND (c < d)
    group.bench_function("and_comparisons", |bencher| {
        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("a", EncryptedType::U8)
            .declare_variable("b", EncryptedType::U8)
            .declare_variable("c", EncryptedType::U8)
            .declare_variable("d", EncryptedType::U8);

        let a_node = builder.load("a");
        let b_node = builder.load("b");
        let c_node = builder.load("c");
        let d_node = builder.load("d");

        let gt_node = builder.gt(a_node, b_node);
        let lt_node = builder.lt(c_node, d_node);
        let and_node = builder.and(gt_node, lt_node);

        let circuit = builder.build(and_node).expect("failed to build circuit");

        let a = EncryptedU8::encrypt(10, keypair.client_key());
        let b_val = EncryptedU8::encrypt(5, keypair.client_key());
        let c = EncryptedU8::encrypt(3, keypair.client_key());
        let d = EncryptedU8::encrypt(7, keypair.client_key());

        let mut inputs = HashMap::new();
        inputs.insert(
            "a".to_string(),
            a.to_cipher_blob().expect("failed to convert to CipherBlob"),
        );
        inputs.insert(
            "b".to_string(),
            b_val
                .to_cipher_blob()
                .expect("failed to convert to CipherBlob"),
        );
        inputs.insert(
            "c".to_string(),
            c.to_cipher_blob().expect("failed to convert to CipherBlob"),
        );
        inputs.insert(
            "d".to_string(),
            d.to_cipher_blob().expect("failed to convert to CipherBlob"),
        );

        let executor = FheExecutor::new();

        bencher.iter(|| {
            black_box(
                executor
                    .execute(&circuit, &inputs)
                    .expect("failed to execute circuit"),
            );
        });
    });

    // OR of two comparisons: (a == b) OR (c != d)
    group.bench_function("or_comparisons", |bencher| {
        let mut builder = CircuitBuilder::new();
        builder
            .declare_variable("a", EncryptedType::U8)
            .declare_variable("b", EncryptedType::U8)
            .declare_variable("c", EncryptedType::U8)
            .declare_variable("d", EncryptedType::U8);

        let a_node = builder.load("a");
        let b_node = builder.load("b");
        let c_node = builder.load("c");
        let d_node = builder.load("d");

        let eq_node = builder.eq(a_node, b_node);
        let ne_node = builder.not(builder.eq(c_node, d_node));
        let or_node = builder.or(eq_node, ne_node);

        let circuit = builder.build(or_node).expect("failed to build circuit");

        let a = EncryptedU8::encrypt(5, keypair.client_key());
        let b_val = EncryptedU8::encrypt(5, keypair.client_key());
        let c = EncryptedU8::encrypt(3, keypair.client_key());
        let d = EncryptedU8::encrypt(7, keypair.client_key());

        let mut inputs = HashMap::new();
        inputs.insert(
            "a".to_string(),
            a.to_cipher_blob().expect("failed to convert to CipherBlob"),
        );
        inputs.insert(
            "b".to_string(),
            b_val
                .to_cipher_blob()
                .expect("failed to convert to CipherBlob"),
        );
        inputs.insert(
            "c".to_string(),
            c.to_cipher_blob().expect("failed to convert to CipherBlob"),
        );
        inputs.insert(
            "d".to_string(),
            d.to_cipher_blob().expect("failed to convert to CipherBlob"),
        );

        let executor = FheExecutor::new();

        bencher.iter(|| {
            black_box(
                executor
                    .execute(&circuit, &inputs)
                    .expect("failed to execute circuit"),
            );
        });
    });

    group.finish();
}

/// Benchmark predicate compilation
/// DISABLED: ColumnValue type no longer exists, needs rework to use CipherBlob
#[allow(dead_code)]
fn bench_predicate_compilation(_c: &mut Criterion) {
    /* DISABLED - needs API update
    let mut group = c.benchmark_group("predicate_compilation");

    use amaters_core::types::col;

    // Simple equality predicate
    group.bench_function("simple_eq", |b| {
        let predicate = Predicate::eq(col("age"), ColumnValue::U8(25));
        let compiler = PredicateCompiler::new();

        bencher.iter(|| {
            black_box(
                compiler
                    .compile(&predicate, EncryptedType::U8)
                    .expect("failed to compile predicate"),
            );
        });
    });

    // Greater than predicate
    group.bench_function("simple_gt", |b| {
        let predicate = Predicate::gt(col("salary"), ColumnValue::U32(50000));
        let compiler = PredicateCompiler::new();

        bencher.iter(|| {
            black_box(
                compiler
                    .compile(&predicate, EncryptedType::U8)
                    .expect("failed to compile predicate"),
            );
        });
    });

    // Complex AND predicate
    group.bench_function("complex_and", |b| {
        let predicate = Predicate::and(
            Predicate::gt(col("age"), ColumnValue::U8(18)),
            Predicate::lt(col("age"), ColumnValue::U8(65)),
        );
        let compiler = PredicateCompiler::new();

        bencher.iter(|| {
            black_box(
                compiler
                    .compile(&predicate, EncryptedType::U8)
                    .expect("failed to compile predicate"),
            );
        });
    });

    // Complex OR predicate
    group.bench_function("complex_or", |b| {
        let predicate = Predicate::or(
            Predicate::eq(col("status"), ColumnValue::U8(1)),
            Predicate::eq(col("status"), ColumnValue::U8(2)),
        );
        let compiler = PredicateCompiler::new();

        bencher.iter(|| {
            black_box(
                compiler
                    .compile(&predicate, EncryptedType::U8)
                    .expect("failed to compile predicate"),
            );
        });
    });

    // Nested predicate
    group.bench_function("nested", |b| {
        let predicate = Predicate::and(
            Predicate::or(
                Predicate::eq(col("type"), ColumnValue::U8(1)),
                Predicate::eq(col("type"), ColumnValue::U8(2)),
            ),
            Predicate::gt(col("value"), ColumnValue::U32(1000)),
        );
        let compiler = PredicateCompiler::new();

        bencher.iter(|| {
            black_box(
                compiler
                    .compile(&predicate, EncryptedType::U8)
                    .expect("failed to compile predicate"),
            );
        });
    });

    group.finish();
    */
}

/// Benchmark different integer widths
fn bench_fhe_integer_widths(c: &mut Criterion) {
    let mut group = c.benchmark_group("fhe_integer_widths");

    let keypair = FheKeyPair::generate().expect("failed to generate FHE keys");
    keypair.set_as_global_server_key();

    for (width_name, enc_type) in [
        ("u8", EncryptedType::U8),
        ("u16", EncryptedType::U16),
        ("u32", EncryptedType::U32),
        ("u64", EncryptedType::U64),
    ] {
        group.bench_with_input(
            BenchmarkId::new("add", width_name),
            &enc_type,
            |bencher, &enc_type| {
                let mut builder = CircuitBuilder::new();
                builder
                    .declare_variable("a", enc_type)
                    .declare_variable("b", enc_type);

                let a_node = builder.load("a");
                let b_node = builder.load("b");
                let sum_node = builder.add(a_node, b_node);
                let circuit = builder.build(sum_node).expect("failed to build circuit");

                let (a_blob, b_blob) = match enc_type {
                    EncryptedType::U8 => {
                        let a = EncryptedU8::encrypt(5, keypair.client_key());
                        let b_val = EncryptedU8::encrypt(3, keypair.client_key());
                        (
                            a.to_cipher_blob().expect("failed to convert to CipherBlob"),
                            b_val
                                .to_cipher_blob()
                                .expect("failed to convert to CipherBlob"),
                        )
                    }
                    EncryptedType::U16 => {
                        let a = EncryptedU16::encrypt(500, keypair.client_key());
                        let b_val = EncryptedU16::encrypt(300, keypair.client_key());
                        (
                            a.to_cipher_blob().expect("failed to convert to CipherBlob"),
                            b_val
                                .to_cipher_blob()
                                .expect("failed to convert to CipherBlob"),
                        )
                    }
                    EncryptedType::U32 => {
                        let a = EncryptedU32::encrypt(50000, keypair.client_key());
                        let b_val = EncryptedU32::encrypt(30000, keypair.client_key());
                        (
                            a.to_cipher_blob().expect("failed to convert to CipherBlob"),
                            b_val
                                .to_cipher_blob()
                                .expect("failed to convert to CipherBlob"),
                        )
                    }
                    EncryptedType::U64 => {
                        let a = EncryptedU64::encrypt(5000000, keypair.client_key());
                        let b_val = EncryptedU64::encrypt(3000000, keypair.client_key());
                        (
                            a.to_cipher_blob().expect("failed to convert to CipherBlob"),
                            b_val
                                .to_cipher_blob()
                                .expect("failed to convert to CipherBlob"),
                        )
                    }
                    _ => panic!("unsupported type"),
                };

                let mut inputs = HashMap::new();
                inputs.insert("a".to_string(), a_blob);
                inputs.insert("b".to_string(), b_blob);

                let executor = FheExecutor::new();

                bencher.iter(|| {
                    black_box(
                        executor
                            .execute(&circuit, &inputs)
                            .expect("failed to execute circuit"),
                    );
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    fhe_benches,
    bench_fhe_encrypt,
    bench_fhe_decrypt,
    bench_fhe_circuit_simple,
    bench_fhe_circuit_complex,
    // bench_predicate_compilation, // DISABLED: needs API update for CipherBlob
    bench_fhe_integer_widths,
);
criterion_main!(fhe_benches);
