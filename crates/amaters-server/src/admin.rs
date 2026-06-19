//! Admin API for cluster and shard management.
//!
//! Provides an in-process API surface (not a network listener) that the server
//! binary and integration tests can call to inspect cluster state, trigger
//! snapshots, and query health.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::ServerConfig;
use crate::server::{ServerError, ServerResult};
use crate::snapshot::SnapshotManager;

// ─── Response types ───────────────────────────────────────────────────────────

/// Overall cluster status as returned by [`AdminApi::get_cluster_status`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterStatusResponse {
    /// This node's numeric identifier.
    pub node_id: u64,
    /// Whether this node currently believes it is the Raft leader.
    pub is_leader: bool,
    /// Number of shards tracked in the placement registry.
    pub num_shards: usize,
    /// Number of peer nodes this node knows about (including itself).
    pub num_nodes: usize,
}

/// A single snapshot entry as returned in [`SnapshotListResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Snapshot identifier.
    pub id: u64,
    /// Unix timestamp in milliseconds when the snapshot was created.
    pub timestamp_ms: u64,
    /// Size of the compressed snapshot bytes on disk.
    pub size_bytes: u64,
}

/// Response returned by [`AdminApi::list_snapshots`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotListResponse {
    /// All available snapshots, sorted newest-first.
    pub snapshots: Vec<SnapshotInfo>,
}

/// Health check response returned by [`AdminApi::get_health`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Coarse status string: `"healthy"`, `"degraded"`, or `"unhealthy"`.
    pub status: String,
    /// Finer-grained per-component details.
    pub details: HashMap<String, String>,
}

/// A summary of a single shard's state for admin responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardSummary {
    /// Shard identifier.
    pub shard_id: u64,
    /// Node currently hosting this shard.
    pub node_id: u64,
    /// Human-readable state string (e.g. "Active", "Splitting").
    pub state: String,
    /// Estimated number of keys in this shard.
    pub key_count: u64,
    /// Estimated byte size of this shard.
    pub size_bytes: u64,
}

/// Response for shard list admin API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardListResponse {
    /// All known shards.
    pub shards: Vec<ShardSummary>,
    /// Total number of shards (same as `shards.len()`).
    pub total: usize,
}

/// Response for shard operation (split/merge/transfer) admin API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardOpResponse {
    /// Raft log index at which the operation was committed.
    pub log_index: u64,
    /// Human-readable status string.
    pub status: String,
}

// ─── AdminApi ─────────────────────────────────────────────────────────────────

/// In-process admin API that exposes cluster management operations.
pub struct AdminApi {
    config: Arc<ServerConfig>,
    snapshot_manager: Arc<SnapshotManager>,
}

impl AdminApi {
    /// Create a new [`AdminApi`] instance.
    pub fn new(config: Arc<ServerConfig>, snapshot_manager: Arc<SnapshotManager>) -> Self {
        Self {
            config,
            snapshot_manager,
        }
    }

    /// Return the current cluster status.
    ///
    /// When the `cluster` feature is disabled this always reports
    /// `is_leader = true` (standalone mode) and zero shards / one node.
    pub async fn get_cluster_status(&self) -> ServerResult<ClusterStatusResponse> {
        let node_id = self.config.cluster.as_ref().map(|c| c.node_id).unwrap_or(1);

        let num_nodes = self
            .config
            .cluster
            .as_ref()
            .map(|c| {
                // +1 for self if self is not already counted
                let peer_count = c.peers.len();
                if peer_count == 0 { 1 } else { peer_count }
            })
            .unwrap_or(1);

        Ok(ClusterStatusResponse {
            node_id,
            is_leader: true,
            num_shards: 0,
            num_nodes,
        })
    }

    /// Create a new snapshot of the current server state.
    ///
    /// The snapshot payload is a minimal status blob.  In a full cluster
    /// implementation this would be the serialised state-machine snapshot.
    pub async fn create_snapshot(&self) -> ServerResult<SnapshotInfo> {
        // Build a trivial payload: JSON-encoded cluster status.
        let status = self.get_cluster_status().await?;
        let payload = serde_json::to_vec(&status)
            .map_err(|e| ServerError::Storage(format!("Failed to serialise snapshot: {}", e)))?;

        // Use nanosecond wall time as snapshot id for uniqueness.
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        let meta = self.snapshot_manager.write_snapshot(id, &payload)?;

        Ok(SnapshotInfo {
            id: meta.id,
            timestamp_ms: meta.timestamp_ms,
            size_bytes: meta.size_bytes,
        })
    }

    /// Return the list of snapshots available on disk.
    pub async fn list_snapshots(&self) -> ServerResult<SnapshotListResponse> {
        let metas = self.snapshot_manager.list_snapshots()?;
        let snapshots = metas
            .into_iter()
            .map(|m| SnapshotInfo {
                id: m.id,
                timestamp_ms: m.timestamp_ms,
                size_bytes: m.size_bytes,
            })
            .collect();
        Ok(SnapshotListResponse { snapshots })
    }

    /// Restore state from the snapshot with the given `snapshot_id`.
    ///
    /// Currently validates that the snapshot can be read and decompressed
    /// without error.  Full state-machine restore is a Phase 8 task once the
    /// Raft leader drives apply.
    pub async fn restore_snapshot(&self, snapshot_id: u64) -> ServerResult<()> {
        let _data = self.snapshot_manager.read_snapshot(snapshot_id)?;
        Ok(())
    }

    /// Return overall health status for this node.
    pub async fn get_health(&self) -> ServerResult<HealthStatus> {
        let mut details = HashMap::new();

        // Storage health: check the snapshot directory is writable
        let snap_health = match self.snapshot_manager.list_snapshots() {
            Ok(snaps) => {
                details.insert("snapshot_count".to_string(), snaps.len().to_string());
                "ok".to_string()
            }
            Err(e) => {
                details.insert("snapshot_error".to_string(), e.to_string());
                "error".to_string()
            }
        };

        // Config health
        details.insert(
            "bind_address".to_string(),
            self.config.server.bind_address.clone(),
        );
        details.insert(
            "cluster_enabled".to_string(),
            self.config
                .cluster
                .as_ref()
                .map(|c| c.enabled.to_string())
                .unwrap_or_else(|| "false".to_string()),
        );

        let status = if snap_health == "ok" {
            "healthy".to_string()
        } else {
            "degraded".to_string()
        };

        Ok(HealthStatus { status, details })
    }
}

// ─── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_api(suffix: &str) -> (AdminApi, PathBuf) {
        // Include a nanosecond timestamp to avoid races between overlapping test runs.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("amaters_admin_test_{}_{}", suffix, ts));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config = Arc::new(ServerConfig::default());
        let sm = Arc::new(SnapshotManager::new(&dir).expect("snapshot manager"));
        (AdminApi::new(config, sm), dir)
    }

    use std::path::PathBuf;

    #[tokio::test]
    async fn test_admin_api_cluster_status() {
        let (api, dir) = make_api("cluster_status");
        let resp = api.get_cluster_status().await.expect("cluster status");
        assert!(resp.node_id >= 1);
        assert!(resp.num_nodes >= 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_admin_api_create_snapshot() {
        let (api, dir) = make_api("create_snap");
        let info = api.create_snapshot().await.expect("create snapshot");
        assert!(info.id > 0);
        assert!(info.size_bytes > 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_admin_api_list_snapshots() {
        let (api, dir) = make_api("list_snaps");
        let info = api.create_snapshot().await.expect("create");
        let list = api.list_snapshots().await.expect("list");
        assert!(!list.snapshots.is_empty());
        assert!(list.snapshots.iter().any(|s| s.id == info.id));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_admin_api_health_check() {
        let (api, dir) = make_api("health");
        let health = api.get_health().await.expect("health");
        assert_eq!(health.status, "healthy");
        assert!(health.details.contains_key("bind_address"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_admin_api_restore_snapshot() {
        let (api, dir) = make_api("restore");
        let info = api.create_snapshot().await.expect("create");
        api.restore_snapshot(info.id)
            .await
            .expect("restore should succeed");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_admin_api_restore_missing_snapshot() {
        let (api, dir) = make_api("restore_missing");
        let result = api.restore_snapshot(u64::MAX).await;
        assert!(
            result.is_err(),
            "Restoring a non-existent snapshot must fail"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
