//! Criterion benchmarks for amaters-cluster.
//!
//! Covers:
//!   - Raft election latency (single election round-trip)
//!   - Log proposal throughput (single-node, no network)
//!   - AppendEntries processing latency (follower side)
//!   - Placement coordinator planning cost (small / medium / large registry)
//!   - ShardRegistry lookup and mutation throughput

use amaters_cluster::{
    AppendEntriesRequest, Command, KeyRange, PlacementCoordinator, PlacementPolicy, RaftConfig,
    RaftNode, RequestVoteRequest, ShardMetadata, ShardRegistry,
    partitioner::{HashRing, RangePartitioner},
};
use amaters_core::types::Key;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_cluster3() -> (RaftNode, RaftNode, RaftNode) {
    let peers = vec![1, 2, 3];
    (
        RaftNode::new(RaftConfig::new(1, peers.clone())).expect("n1"),
        RaftNode::new(RaftConfig::new(2, peers.clone())).expect("n2"),
        RaftNode::new(RaftConfig::new(3, peers)).expect("n3"),
    )
}

fn elect_leader(leader: &RaftNode, followers: &[&RaftNode]) {
    leader.start_election();
    let req = RequestVoteRequest::new(
        leader.current_term(),
        leader.node_id(),
        leader.last_log_index(),
        0,
    );
    for follower in followers {
        let resp = follower.handle_request_vote(req.clone());
        if resp.vote_granted && leader.handle_vote_response(follower.node_id(), resp) {
            break;
        }
    }
}

fn build_shard_registry(num_shards: usize) -> ShardRegistry {
    let registry = ShardRegistry::new();
    for i in 0..num_shards {
        let shard_id = i as u64 + 1;
        let node_id = (i % 3) as u64 + 1; // distribute across 3 nodes
        let start = Key::from_str(&format!("{:08x}", i * 1000));
        let end = Key::from_str(&format!("{:08x}", (i + 1) * 1000));
        if let Ok(range) = KeyRange::new(start, end) {
            let meta = ShardMetadata::new(shard_id, range, node_id);
            let _ = registry.register(meta);
        }
    }
    registry
}

// ---------------------------------------------------------------------------
// Benchmark: Raft election latency
// ---------------------------------------------------------------------------

fn bench_raft_election(c: &mut Criterion) {
    c.bench_function("raft_election_3node", |b| {
        b.iter(|| {
            let (n1, n2, n3) = make_cluster3();
            elect_leader(&n1, &[&n2, &n3]);
            black_box(n1.state())
        })
    });
}

// ---------------------------------------------------------------------------
// Benchmark: Log proposal throughput (leader only, no replication)
// ---------------------------------------------------------------------------

fn bench_proposal_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("proposal_throughput");

    for size in [16usize, 128, 1024, 8192] {
        group.bench_with_input(BenchmarkId::new("payload_bytes", size), &size, |b, &sz| {
            let (n1, n2, n3) = make_cluster3();
            elect_leader(&n1, &[&n2, &n3]);
            let payload = vec![0xABu8; sz];
            b.iter(|| {
                let cmd = Command::new(payload.clone());
                black_box(n1.propose(cmd).expect("propose"))
            })
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: AppendEntries processing on follower side
// ---------------------------------------------------------------------------

fn bench_append_entries_follower(c: &mut Criterion) {
    let mut group = c.benchmark_group("append_entries_follower");

    for entry_count in [1usize, 10, 100] {
        group.bench_with_input(
            BenchmarkId::new("entries", entry_count),
            &entry_count,
            |b, &n| {
                b.iter_batched(
                    || {
                        let (n1, n2, n3) = make_cluster3();
                        elect_leader(&n1, &[&n2, &n3]);
                        for i in 0..n {
                            n1.propose(Command::from_str(&format!("k{}", i)))
                                .expect("propose");
                        }
                        let reqs = n1.create_replication_requests();
                        (n2, n3, reqs)
                    },
                    |(n2, n3, reqs)| {
                        for (peer_id, req) in reqs {
                            let follower = if peer_id == n2.node_id() { &n2 } else { &n3 };
                            black_box(follower.handle_append_entries(req));
                        }
                    },
                    BatchSize::SmallInput,
                )
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: Full replication round-trip (propose → replicate → commit)
// ---------------------------------------------------------------------------

fn bench_replication_round_trip(c: &mut Criterion) {
    c.bench_function("replication_round_trip_single", |b| {
        b.iter_batched(
            || {
                let (n1, n2, n3) = make_cluster3();
                elect_leader(&n1, &[&n2, &n3]);
                (n1, n2, n3)
            },
            |(n1, n2, n3)| {
                let idx = n1
                    .propose(Command::from_str("bench_key=value"))
                    .expect("propose");
                for (peer_id, req) in n1.create_replication_requests() {
                    let follower = if peer_id == n2.node_id() { &n2 } else { &n3 };
                    let resp = follower.handle_append_entries(req);
                    if resp.success {
                        let _ = n1.handle_replication_response(peer_id, resp);
                    }
                }
                black_box(n1.commit_index() >= idx)
            },
            BatchSize::SmallInput,
        )
    });
}

// ---------------------------------------------------------------------------
// Benchmark: Placement coordinator planning cost
// ---------------------------------------------------------------------------

fn bench_placement_planning(c: &mut Criterion) {
    let mut group = c.benchmark_group("placement_planning");
    let coordinator = PlacementCoordinator::new(PlacementPolicy::default_policy());

    for shard_count in [8usize, 32, 128] {
        let registry = build_shard_registry(shard_count);
        group.bench_with_input(
            BenchmarkId::new("shards", shard_count),
            &registry,
            |b, reg| b.iter(|| black_box(coordinator.plan(reg))),
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: ShardRegistry lookup and mutation
// ---------------------------------------------------------------------------

fn bench_shard_registry(c: &mut Criterion) {
    let mut group = c.benchmark_group("shard_registry");

    let registry_32 = build_shard_registry(32);

    // find_shard_for_key
    group.bench_function("find_shard_for_key_32shards", |b| {
        let query_key = Key::from_str("00001f4a"); // falls inside shard range
        b.iter(|| black_box(registry_32.find_shard_for_key(&query_key)))
    });

    // register new shard
    group.bench_function("register_shard", |b| {
        b.iter_batched(
            || build_shard_registry(16),
            |registry| {
                let start = Key::from_str("fffff000");
                let end = Key::from_str("fffffffx");
                if let Ok(range) = KeyRange::new(start, end) {
                    let meta = ShardMetadata::new(99, range, 1);
                    black_box(registry.register(meta))
                } else {
                    black_box(Ok(()))
                }
            },
            BatchSize::SmallInput,
        )
    });

    // get_by_node
    group.bench_function("get_by_node_32shards", |b| {
        b.iter(|| black_box(registry_32.get_by_node(1)))
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: Single-proposal commit latency distribution
// ---------------------------------------------------------------------------

/// Measures the end-to-end latency of a single propose → replicate → commit
/// round trip, giving criterion a distribution of 100 samples so the
/// mean ± std-dev are reported.
///
/// In a real cluster the network RTT dominates; here the measurement captures
/// the in-memory scheduling overhead and mutex contention cost.
fn bench_commit_latency_distribution(c: &mut Criterion) {
    let mut group = c.benchmark_group("commit_latency_distribution");
    group.sample_size(100);

    group.bench_function("single_proposal_roundtrip", |b| {
        b.iter_batched(
            || {
                let (n1, n2, n3) = make_cluster3();
                elect_leader(&n1, &[&n2, &n3]);
                (n1, n2, n3)
            },
            |(n1, n2, n3)| {
                let idx = n1
                    .propose(Command::from_str("latency_probe"))
                    .expect("propose");
                for (peer_id, req) in n1.create_replication_requests() {
                    let follower = if peer_id == n2.node_id() { &n2 } else { &n3 };
                    let resp = follower.handle_append_entries(req);
                    if resp.success {
                        let _ = n1.handle_replication_response(peer_id, resp);
                    }
                }
                black_box(n1.commit_index() >= idx)
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: Shard routing (HashRing and RangePartitioner)
// ---------------------------------------------------------------------------

/// Benchmarks `HashRing::get_shard_for_key` and `RangePartitioner::get_shard_for_key`
/// over 1 000 pre-generated keys to amortise setup cost.
fn bench_shard_routing(c: &mut Criterion) {
    let mut group = c.benchmark_group("shard_routing");

    const N_SHARDS: usize = 32;
    const VIRTUAL_NODES: usize = 100;
    const N_KEYS: usize = 1_000;

    // ── HashRing ─────────────────────────────────────────────────────────────
    let mut ring = HashRing::new(VIRTUAL_NODES);
    for i in 0..N_SHARDS {
        ring.add_shard(i as u64 + 1);
    }
    // Pre-generate N_KEYS lookup keys so the measurement is pure routing cost.
    let keys: Vec<Key> = (0..N_KEYS)
        .map(|i: usize| Key::from_str(&format!("{:016x}", i.wrapping_mul(997))))
        .collect();

    group.bench_function("hash_ring_get_shard_for_key_1000keys", |b| {
        b.iter(|| {
            for key in &keys {
                black_box(ring.get_shard_for_key(key));
            }
        })
    });

    // ── RangePartitioner ─────────────────────────────────────────────────────
    let registry = build_shard_registry(N_SHARDS);
    let range_partitioner = RangePartitioner::from_registry(&registry);

    group.bench_function("range_partitioner_get_shard_for_key_1000keys", |b| {
        b.iter(|| {
            for key in &keys {
                black_box(range_partitioner.get_shard_for_key(key));
            }
        })
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: RequestVote processing
// ---------------------------------------------------------------------------

fn bench_request_vote(c: &mut Criterion) {
    c.bench_function("handle_request_vote", |b| {
        b.iter_batched(
            || {
                let peers = vec![1, 2];
                let n1 = RaftNode::new(RaftConfig::new(1, peers.clone())).expect("n1");
                let n2 = RaftNode::new(RaftConfig::new(2, peers)).expect("n2");
                n1.start_election();
                let req = RequestVoteRequest::new(n1.current_term(), n1.node_id(), 0, 0);
                (n2, req)
            },
            |(n2, req)| black_box(n2.handle_request_vote(req)),
            BatchSize::SmallInput,
        )
    });
}

// ---------------------------------------------------------------------------
// Benchmark: Heartbeat (empty AppendEntries) processing
// ---------------------------------------------------------------------------

fn bench_heartbeat(c: &mut Criterion) {
    c.bench_function("handle_heartbeat_follower", |b| {
        b.iter_batched(
            || {
                let (n1, n2, n3) = make_cluster3();
                elect_leader(&n1, &[&n2, &n3]);
                // heartbeat(term, leader_id, prev_log_index, prev_log_term, leader_commit)
                let hb = AppendEntriesRequest::heartbeat(
                    n1.current_term(),
                    n1.node_id(),
                    n1.last_log_index(),
                    0,
                    n1.commit_index(),
                );
                (n2, n3, hb)
            },
            |(n2, _n3, hb)| black_box(n2.handle_append_entries(hb)),
            BatchSize::SmallInput,
        )
    });
}

// ---------------------------------------------------------------------------
// Criterion groups
// ---------------------------------------------------------------------------

criterion_group!(
    raft_benches,
    bench_raft_election,
    bench_proposal_throughput,
    bench_append_entries_follower,
    bench_replication_round_trip,
    bench_commit_latency_distribution,
    bench_request_vote,
    bench_heartbeat,
);

criterion_group!(
    cluster_benches,
    bench_placement_planning,
    bench_shard_registry,
    bench_shard_routing,
);

criterion_main!(raft_benches, cluster_benches);
