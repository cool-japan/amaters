//! Criterion benchmarks for AmateRS SDK client operations.
//!
//! Benchmarks run against an in-process stub server (`common/stub_server.rs`).
//! Does NOT test FHE, auth, or streaming — pure throughput measurement.
//!
//! # Running
//!
//! ```bash
//! # Build-only check (no server required):
//! cargo bench -p amaters-sdk-rust --no-run
//!
//! # Run benchmarks (starts an in-process stub server automatically):
//! cargo bench -p amaters-sdk-rust
//! ```

mod common;

use amaters_core::{CipherBlob, Key};
use amaters_sdk_rust::AmateRSClient;
use common::stub_server::StubServer;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a key with a unique numeric suffix.
fn make_key(idx: usize) -> Key {
    Key::from_str(&format!("bench:key:{:06}", idx))
}

/// Create a zero-filled `CipherBlob` of `size` bytes.
fn make_value(size: usize) -> CipherBlob {
    CipherBlob::new(vec![0u8; size])
}

/// Build a single-threaded Tokio runtime for the benchmarks.
fn bench_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for benchmarks")
}

// ---------------------------------------------------------------------------
// bench_set
// ---------------------------------------------------------------------------

fn bench_set(c: &mut Criterion) {
    let rt = bench_runtime();

    // Start the stub server and create a connected client once; reuse across
    // all iterations.
    let (client, _server) = rt.block_on(async {
        let server = StubServer::start()
            .await
            .expect("failed to start stub server");
        let client = AmateRSClient::connect(server.endpoint())
            .await
            .expect("failed to connect client to stub server");
        (client, server)
    });

    let mut group = c.benchmark_group("sdk_set");

    for size in [64usize, 1024, 16 * 1024] {
        let value = make_value(size);
        let key = make_key(size);

        group.bench_with_input(
            BenchmarkId::new("value_bytes", size),
            &(key, value),
            |b, (key, value)| {
                b.to_async(&rt).iter(|| async {
                    client
                        .set("bench_collection", key, value)
                        .await
                        .expect("set failed");
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// bench_get
// ---------------------------------------------------------------------------

fn bench_get(c: &mut Criterion) {
    let rt = bench_runtime();

    let (client, _server) = rt.block_on(async {
        let server = StubServer::start()
            .await
            .expect("failed to start stub server");
        let client = AmateRSClient::connect(server.endpoint())
            .await
            .expect("failed to connect client to stub server");

        // Pre-populate values so gets have something to retrieve.
        for size in [64usize, 1024, 16 * 1024] {
            let key = make_key(size);
            let value = make_value(size);
            client
                .set("bench_collection", &key, &value)
                .await
                .expect("pre-populate set failed");
        }

        (client, server)
    });

    let mut group = c.benchmark_group("sdk_get");

    for size in [64usize, 1024, 16 * 1024] {
        let key = make_key(size);

        group.bench_with_input(BenchmarkId::new("value_bytes", size), &key, |b, key| {
            b.to_async(&rt).iter(|| async {
                let _ = client
                    .get("bench_collection", key)
                    .await
                    .expect("get failed");
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// bench_delete
// ---------------------------------------------------------------------------

fn bench_delete(c: &mut Criterion) {
    let rt = bench_runtime();

    let (client, _server) = rt.block_on(async {
        let server = StubServer::start()
            .await
            .expect("failed to start stub server");
        let client = AmateRSClient::connect(server.endpoint())
            .await
            .expect("failed to connect client to stub server");
        (client, server)
    });

    let mut group = c.benchmark_group("sdk_delete");

    // We need to set before each delete iteration.
    // Use `iter_custom` or re-insert inside the iter closure.
    group.bench_function("delete_existing_key", |b| {
        b.to_async(&rt).iter(|| async {
            // Insert first so the delete has something to operate on.
            let key = make_key(0);
            let value = make_value(64);
            client
                .set("bench_del_collection", &key, &value)
                .await
                .expect("set for delete bench failed");

            client
                .delete("bench_del_collection", &key)
                .await
                .expect("delete failed");
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion wiring
// ---------------------------------------------------------------------------

criterion_group!(benches, bench_set, bench_get, bench_delete);
criterion_main!(benches);
