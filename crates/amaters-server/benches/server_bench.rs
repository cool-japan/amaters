//! Load / throughput benchmarks for amaters-server.

use amaters_server::snapshot::SnapshotManager;
use amaters_server::version::VersionHandshake;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

fn bench_snapshot_write(c: &mut Criterion) {
    let dir = std::env::temp_dir().join("amaters_bench_snap_write");
    std::fs::create_dir_all(&dir).expect("create bench dir");
    let sm = SnapshotManager::new(&dir).expect("create SnapshotManager");

    let mut group = c.benchmark_group("snapshot_write");
    for size_kb in [1usize, 64, 1024, 4096] {
        let data = vec![0xABu8; size_kb * 1024];
        group.bench_with_input(BenchmarkId::new("size_kb", size_kb), &data, |b, d| {
            let mut id = 0u64;
            b.iter(|| {
                id += 1;
                sm.write_snapshot(id, d).expect("write snapshot");
            });
        });
    }
    group.finish();
    std::fs::remove_dir_all(&dir).ok();
}

fn bench_snapshot_read(c: &mut Criterion) {
    let dir = std::env::temp_dir().join("amaters_bench_snap_read");
    std::fs::create_dir_all(&dir).expect("create bench dir");
    let sm = SnapshotManager::new(&dir).expect("create SnapshotManager");
    // Pre-write snapshots of various sizes
    for (id, size_kb) in [(1u64, 1usize), (2, 64), (3, 1024)] {
        let data = vec![0xCDu8; size_kb * 1024];
        sm.write_snapshot(id, &data).expect("pre-write");
    }

    let mut group = c.benchmark_group("snapshot_read");
    for (id, size_kb) in [(1u64, 1usize), (2, 64), (3, 1024)] {
        group.bench_with_input(BenchmarkId::new("size_kb", size_kb), &id, |b, &snap_id| {
            b.iter(|| sm.read_snapshot(snap_id).expect("read snapshot"));
        });
    }
    group.finish();
    std::fs::remove_dir_all(&dir).ok();
}

fn bench_snapshot_list(c: &mut Criterion) {
    let dir = std::env::temp_dir().join("amaters_bench_snap_list");
    std::fs::create_dir_all(&dir).expect("create bench dir");
    let sm = SnapshotManager::new(&dir).expect("create SnapshotManager");
    for i in 1u64..=100 {
        sm.write_snapshot(i, &[0u8; 512]).expect("pre-write");
    }
    c.bench_function("snapshot_list_100", |b| {
        b.iter(|| sm.list_snapshots().expect("list snapshots"));
    });
    std::fs::remove_dir_all(&dir).ok();
}

fn bench_version_handshake_compat(c: &mut Criterion) {
    let current = VersionHandshake::current();
    let peer = VersionHandshake::current();
    c.bench_function("version_handshake_compat_check", |b| {
        b.iter(|| current.is_compatible_with(&peer));
    });
}

criterion_group!(
    server_benches,
    bench_snapshot_write,
    bench_snapshot_read,
    bench_snapshot_list,
    bench_version_handshake_compat,
);
criterion_main!(server_benches);
