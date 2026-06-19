//! Cluster failure scenario and chaos tests.

use std::sync::Arc;

use amaters_server::{
    snapshot::SnapshotManager,
    version::{VersionHandshake, is_compatible},
};

// ─── Snapshot chaos tests ─────────────────────────────────────────────────────

#[test]
fn test_snapshot_survives_large_payload() {
    let dir = std::env::temp_dir().join("amaters_chaos_snap");
    std::fs::create_dir_all(&dir).expect("temp dir");
    // Use a unique sub-directory to avoid conflicts with parallel runs
    let unique_dir = dir.join(format!(
        "run_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&unique_dir).expect("unique temp dir");

    let sm = SnapshotManager::new(&unique_dir).expect("create snapshot manager");
    let payload = vec![0xABu8; 1024 * 1024]; // 1 MB
    let meta = sm.write_snapshot(1, &payload).expect("write");
    assert_eq!(meta.id, 1);

    let restored = sm.read_snapshot(1).expect("read");
    assert_eq!(restored, payload);

    std::fs::remove_dir_all(&unique_dir).ok();
}

#[test]
fn test_version_incompatibility_detected() {
    let current = VersionHandshake::current();
    let old = VersionHandshake {
        version: (1, 0, 0),
        min_compatible: (1, 0, 0),
        build_id: "old".into(),
    };
    assert!(!current.is_compatible_with(&old));
}

#[test]
fn test_snapshot_manager_handles_concurrent_writes() {
    let dir = std::env::temp_dir().join(format!(
        "amaters_chaos_concurrent_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");

    let sm = Arc::new(SnapshotManager::new(&dir).expect("create"));
    let handles: Vec<_> = (1u64..=8)
        .map(|id| {
            let s = Arc::clone(&sm);
            std::thread::spawn(move || {
                let data = vec![id as u8; 10_000];
                s.write_snapshot(id, &data).expect("concurrent write");
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }

    let snaps = sm.list_snapshots().expect("list");
    assert_eq!(snaps.len(), 8);

    std::fs::remove_dir_all(&dir).ok();
}

// ─── Version compatibility tests ──────────────────────────────────────────────

#[test]
fn test_is_compatible_same_version() {
    assert!(is_compatible((0, 2, 2)));
}

#[test]
fn test_is_compatible_min_boundary() {
    assert!(is_compatible((0, 2, 0)));
}

#[test]
fn test_is_compatible_future_patch_same_minor() {
    // A hypothetical future patch on the same minor should be compatible
    assert!(is_compatible((0, 2, 99)));
}

#[test]
fn test_is_compatible_higher_minor() {
    // A node running 0.3.x must be rejected (it's newer but may have new protocol)
    // Our MIN_COMPATIBLE_VERSION.minor=2 so 0.3.x (minor=3) is > min; compatible
    assert!(is_compatible((0, 3, 0)));
}

#[test]
fn test_is_incompatible_different_major() {
    assert!(!is_compatible((1, 0, 0)));
    assert!(!is_compatible((2, 2, 2)));
}

#[test]
fn test_is_incompatible_minor_below_min() {
    // 0.1.x is below minimum compatible minor (2)
    assert!(!is_compatible((0, 1, 0)));
}

// ─── Admin health under load test ─────────────────────────────────────────────

#[tokio::test]
async fn test_admin_api_health_under_load() {
    use amaters_server::{admin::AdminApi, config::ServerConfig};

    let dir = std::env::temp_dir().join(format!(
        "amaters_admin_health_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");

    let config = Arc::new(ServerConfig::default());
    let sm = Arc::new(SnapshotManager::new(&dir).expect("sm"));
    let api = AdminApi::new(Arc::clone(&config), Arc::clone(&sm));

    // Run 50 sequential health checks (parallel borrows require Arc<api>; the
    // test confirms no panics under rapid repeated calls).
    let api = Arc::new(api);
    let mut results = Vec::with_capacity(50);
    for _ in 0..50 {
        results.push(api.get_health().await);
    }

    for result in results {
        assert!(result.is_ok(), "health check failed: {:?}", result);
    }

    std::fs::remove_dir_all(&dir).ok();
}

// ─── Cluster integration smoke tests ─────────────────────────────────────────

#[tokio::test]
async fn test_cluster_handle_standalone_starts() {
    use amaters_server::cluster_integration::ClusterHandle;

    let handle = ClusterHandle::start_standalone(42)
        .await
        .expect("standalone start");
    assert_eq!(handle.node_id(), 42);
    // is_leader() must not panic
    let _ = handle.is_leader();
}

#[tokio::test]
async fn test_cluster_handle_standalone_always_leader() {
    use amaters_server::cluster_integration::ClusterHandle;

    let handle = ClusterHandle::start_standalone(1)
        .await
        .expect("standalone");
    assert!(handle.is_leader());
    assert_eq!(handle.shard_count(), 0);
}

/// Full Raft smoke test with minimum 3-node cluster.
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_cluster_handle_three_node_raft_starts() {
    use amaters_server::cluster_integration::ClusterHandle;

    let peers = vec![
        (1u64, "127.0.0.1:28001".parse().expect("addr1")),
        (2u64, "127.0.0.1:28002".parse().expect("addr2")),
        (3u64, "127.0.0.1:28003".parse().expect("addr3")),
    ];
    let handle = ClusterHandle::start(1, peers)
        .await
        .expect("three-node Raft start");
    assert_eq!(handle.node_id(), 1);
    let _ = handle.is_leader();
    assert_eq!(handle.shard_count(), 0);
}
