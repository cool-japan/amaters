//! Live data migration tracker for load-balanced shard movement.
//!
//! [`MigrationTracker`] maintains a concurrent registry of in-progress
//! [`Migration`]s and enforces the invariant that a shard can participate in
//! at most one migration at a time.
//!
//! The companion function [`compute_rebalance_plan`] analyses a
//! [`ShardRegistry`] snapshot and proposes `(shard_id, from_node, to_node)`
//! triples that would reduce inter-node imbalance.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::{DashMap, DashSet};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::shard::{ShardId, ShardRegistry};
use crate::types::NodeId;

// ── MigrationStatus ───────────────────────────────────────────────────────────

/// Progress state of a single shard migration.
#[derive(Debug, Clone, PartialEq)]
pub enum MigrationStatus {
    /// Accepted, not yet started.
    Pending,
    /// Data is actively being copied.
    InProgress,
    /// Copy complete; checksums are being verified before traffic cutover.
    Verifying,
    /// Migration finished successfully; source shard may be released.
    Complete,
    /// Migration aborted.
    Failed {
        /// Human-readable failure reason.
        reason: String,
    },
}

// ── Migration ─────────────────────────────────────────────────────────────────

/// Represents one in-progress shard migration (moving data from one node to
/// another).
#[derive(Debug, Clone)]
pub struct Migration {
    /// Unique identifier for this migration.
    pub id: Uuid,
    /// The shard being migrated.
    pub shard_id: ShardId,
    /// Source node.
    pub from_node: NodeId,
    /// Destination node.
    pub to_node: NodeId,
    /// Current status.
    pub status: MigrationStatus,
    /// Wall-clock milliseconds when the migration was accepted.
    pub started_at_ms: u64,
    /// Bytes copied so far (updated by progress calls).
    pub bytes_migrated: u64,
    /// Total bytes to copy (may be 0 if unknown at start).
    pub total_bytes: u64,
}

impl Migration {
    fn new(shard_id: ShardId, from_node: NodeId, to_node: NodeId) -> Self {
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            id: Uuid::new_v4(),
            shard_id,
            from_node,
            to_node,
            status: MigrationStatus::Pending,
            started_at_ms,
            bytes_migrated: 0,
            total_bytes: 0,
        }
    }
}

// ── MigrationTracker ─────────────────────────────────────────────────────────

/// Concurrent registry of active [`Migration`]s.
///
/// # Conflict prevention
///
/// A shard can be involved in at most one migration at a time.  Calling
/// [`begin_migration`][Self::begin_migration] for a shard that already has an
/// active migration returns an `Err`.
///
/// Migrations in terminal states ([`MigrationStatus::Complete`] or
/// [`MigrationStatus::Failed`]) no longer block new migrations for the same
/// shard.
pub struct MigrationTracker {
    /// All migrations indexed by their UUID.
    migrations: DashMap<Uuid, Migration>,
    /// Maps `shard_id → migration_id` for non-terminal migrations only.
    active_shard_migrations: DashMap<ShardId, Uuid>,
}

impl MigrationTracker {
    /// Construct an empty tracker.
    pub fn new() -> Self {
        Self {
            migrations: DashMap::new(),
            active_shard_migrations: DashMap::new(),
        }
    }

    /// Begin a new migration for `shard_id` from `from_node` to `to_node`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the shard is already involved in a non-terminal
    /// migration.
    pub fn begin_migration(
        &self,
        shard_id: ShardId,
        from_node: NodeId,
        to_node: NodeId,
    ) -> Result<Uuid, String> {
        // Conflict check: shard must not already have an active migration.
        if self.active_shard_migrations.contains_key(&shard_id) {
            return Err(format!(
                "shard {} already has an active migration",
                shard_id
            ));
        }

        let migration = Migration::new(shard_id, from_node, to_node);
        let id = migration.id;

        self.active_shard_migrations.insert(shard_id, id);
        self.migrations.insert(id, migration);

        info!(
            migration_id = %id,
            shard_id = shard_id,
            from_node = from_node,
            to_node = to_node,
            "Migration begun"
        );

        Ok(id)
    }

    /// Update the byte-level progress of migration `id`.
    ///
    /// Returns `true` if the migration existed and its status was updated to
    /// [`MigrationStatus::InProgress`].  Returns `false` if the migration was
    /// not found.
    pub fn update_progress(&self, id: Uuid, bytes_migrated: u64, total_bytes: u64) -> bool {
        match self.migrations.get_mut(&id) {
            None => {
                warn!(migration_id = %id, "update_progress: migration not found");
                false
            }
            Some(mut m) => {
                m.bytes_migrated = bytes_migrated;
                m.total_bytes = total_bytes;
                if m.status == MigrationStatus::Pending {
                    m.status = MigrationStatus::InProgress;
                }
                debug!(
                    migration_id = %id,
                    bytes_migrated = bytes_migrated,
                    total_bytes = total_bytes,
                    "Migration progress updated"
                );
                true
            }
        }
    }

    /// Mark migration `id` as [`MigrationStatus::Complete`] and release the
    /// shard lock.
    ///
    /// Returns `true` if the migration existed.
    pub fn complete_migration(&self, id: Uuid) -> bool {
        match self.migrations.get_mut(&id) {
            None => {
                warn!(migration_id = %id, "complete_migration: migration not found");
                false
            }
            Some(mut m) => {
                let shard_id = m.shard_id;
                m.status = MigrationStatus::Complete;
                drop(m);
                // Release shard lock only if it still maps to this migration.
                self.active_shard_migrations
                    .remove_if(&shard_id, |_, v| *v == id);
                info!(migration_id = %id, shard_id = shard_id, "Migration completed");
                true
            }
        }
    }

    /// Mark migration `id` as [`MigrationStatus::Failed`] and release the
    /// shard lock.
    ///
    /// Returns `true` if the migration existed.
    pub fn fail_migration(&self, id: Uuid, reason: String) -> bool {
        match self.migrations.get_mut(&id) {
            None => {
                warn!(migration_id = %id, "fail_migration: migration not found");
                false
            }
            Some(mut m) => {
                let shard_id = m.shard_id;
                m.status = MigrationStatus::Failed {
                    reason: reason.clone(),
                };
                drop(m);
                self.active_shard_migrations
                    .remove_if(&shard_id, |_, v| *v == id);
                warn!(
                    migration_id = %id,
                    shard_id = shard_id,
                    reason = %reason,
                    "Migration failed"
                );
                true
            }
        }
    }

    /// Return a snapshot of the [`Migration`] with the given `id`, or `None`.
    pub fn get_migration(&self, id: Uuid) -> Option<Migration> {
        self.migrations.get(&id).map(|m| m.clone())
    }

    /// Return snapshots of all migrations that are not in a terminal state.
    pub fn active_migrations(&self) -> Vec<Migration> {
        self.migrations
            .iter()
            .filter(|r| {
                !matches!(
                    r.status,
                    MigrationStatus::Complete | MigrationStatus::Failed { .. }
                )
            })
            .map(|r| r.clone())
            .collect()
    }

    /// Return `true` if `shard_id` currently has a non-terminal migration.
    pub fn is_shard_migrating(&self, shard_id: &ShardId) -> bool {
        self.active_shard_migrations.contains_key(shard_id)
    }
}

impl Default for MigrationTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Rebalance plan ────────────────────────────────────────────────────────────

/// Compute which shards should migrate to rebalance load.
///
/// Returns at most `max_concurrent_migrations` `(shard_id, from_node, to_node)`
/// proposals.  Nodes with a shard count more than `imbalance_threshold` above
/// the cluster mean are considered overloaded; nodes below the mean by the same
/// threshold are considered underloaded.
///
/// Shards that already have an active migration are excluded.
pub fn compute_rebalance_plan(
    registry: &ShardRegistry,
    tracker: &MigrationTracker,
    imbalance_threshold: f64,
    max_concurrent_migrations: usize,
) -> Vec<(ShardId, NodeId, NodeId)> {
    use std::collections::HashMap;

    let shards = registry.get_all();
    if shards.is_empty() || max_concurrent_migrations == 0 {
        return Vec::new();
    }

    // Build per-node shard lists.
    let mut node_shards: HashMap<NodeId, Vec<ShardId>> = HashMap::new();
    for shard in &shards {
        node_shards.entry(shard.node_id).or_default().push(shard.id);
    }

    if node_shards.len() < 2 {
        return Vec::new();
    }

    let mean = shards.len() as f64 / node_shards.len() as f64;

    // Identify overloaded and underloaded nodes.
    let mut overloaded: Vec<(NodeId, Vec<ShardId>)> = node_shards
        .iter()
        .filter(|(_, ids)| ids.len() as f64 > mean * (1.0 + imbalance_threshold))
        .map(|(nid, ids)| (*nid, ids.clone()))
        .collect();

    let mut underloaded: Vec<(NodeId, usize)> = node_shards
        .iter()
        .filter(|(_, ids)| (ids.len() as f64) < mean * (1.0 - imbalance_threshold))
        .map(|(nid, ids)| (*nid, ids.len()))
        .collect();

    if overloaded.is_empty() || underloaded.is_empty() {
        return Vec::new();
    }

    // Sort for determinism.
    overloaded.sort_by_key(|(nid, _)| *nid);
    underloaded.sort_by_key(|(nid, _)| *nid);

    let mut plan: Vec<(ShardId, NodeId, NodeId)> = Vec::new();

    'outer: for (from_node, shard_ids) in &overloaded {
        for shard_id in shard_ids {
            if tracker.is_shard_migrating(shard_id) {
                continue;
            }
            if let Some((to_node, _)) = underloaded.first_mut() {
                plan.push((*shard_id, *from_node, *to_node));
                if plan.len() >= max_concurrent_migrations {
                    break 'outer;
                }
            }
        }
    }

    plan
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard::{KeyRange, ShardMetadata, ShardRegistry};
    use amaters_core::Key;

    fn make_registry_with_distribution(
        distribution: &[(ShardId, NodeId, &str, &str)],
    ) -> ShardRegistry {
        let registry = ShardRegistry::new();
        for &(shard_id, node_id, start, end) in distribution {
            let range =
                KeyRange::new(Key::from_str(start), Key::from_str(end)).expect("valid range");
            let shard = ShardMetadata::new(shard_id, range, node_id);
            registry.register(shard).expect("register");
        }
        registry
    }

    // ── test_begin_migration_prevents_duplicate ───────────────────────────────

    #[test]
    fn test_begin_migration_prevents_duplicate() {
        let tracker = MigrationTracker::new();

        // First migration for shard 1: should succeed.
        let result = tracker.begin_migration(1, 10, 20);
        assert!(result.is_ok(), "first migration should succeed");

        // Second migration for the same shard: must fail.
        let result2 = tracker.begin_migration(1, 10, 20);
        assert!(
            result2.is_err(),
            "duplicate migration for shard 1 should be rejected"
        );
        let err_msg = result2.expect_err("second migration should fail");
        assert!(
            err_msg.contains("shard 1"),
            "error message should mention the shard id"
        );
    }

    // ── test_migration_lifecycle ──────────────────────────────────────────────

    #[test]
    fn test_migration_lifecycle() {
        let tracker = MigrationTracker::new();

        let id = tracker.begin_migration(2, 10, 20).expect("begin_migration");

        // Initial state: Pending.
        let m = tracker.get_migration(id).expect("get migration");
        assert_eq!(m.status, MigrationStatus::Pending);
        assert!(tracker.is_shard_migrating(&2));

        // Update progress → InProgress.
        assert!(tracker.update_progress(id, 512, 1024));
        let m = tracker.get_migration(id).expect("get migration");
        assert_eq!(m.status, MigrationStatus::InProgress);
        assert_eq!(m.bytes_migrated, 512);
        assert_eq!(m.total_bytes, 1024);

        // Complete.
        assert!(tracker.complete_migration(id));
        let m = tracker.get_migration(id).expect("get migration");
        assert_eq!(m.status, MigrationStatus::Complete);
        // Shard lock should be released.
        assert!(!tracker.is_shard_migrating(&2));

        // Should now be able to start a new migration for the same shard.
        assert!(tracker.begin_migration(2, 20, 10).is_ok());
    }

    // ── test_migration_failed_state ───────────────────────────────────────────

    #[test]
    fn test_migration_failed_state() {
        let tracker = MigrationTracker::new();
        let id = tracker.begin_migration(3, 10, 20).expect("begin_migration");

        assert!(tracker.fail_migration(id, "disk full".to_string()));
        let m = tracker.get_migration(id).expect("get migration");
        assert!(
            matches!(m.status, MigrationStatus::Failed { ref reason } if reason == "disk full"),
            "expected Failed with reason 'disk full', got {:?}",
            m.status
        );
        // Shard lock must be released after failure.
        assert!(!tracker.is_shard_migrating(&3));
    }

    // ── test_rebalance_plan_targets_overloaded_node ───────────────────────────

    #[test]
    fn test_rebalance_plan_targets_overloaded_node() {
        // 6 shards on node A (id 1), 2 shards on node B (id 2).
        // mean = 8/2 = 4.  Node A has 6 (50% above mean); Node B has 2 (50% below).
        let registry = make_registry_with_distribution(&[
            (1, 1, "a0", "a1"),
            (2, 1, "a1", "a2"),
            (3, 1, "a2", "a3"),
            (4, 1, "a3", "a4"),
            (5, 1, "a4", "a5"),
            (6, 1, "a5", "a6"),
            (7, 2, "b0", "b1"),
            (8, 2, "b1", "b2"),
        ]);
        let tracker = MigrationTracker::new();
        // imbalance_threshold 0.2 → overloaded if > mean*(1+0.2)=4.8, underloaded if < mean*(1-0.2)=3.2
        let plan = compute_rebalance_plan(&registry, &tracker, 0.2, 10);

        assert!(
            !plan.is_empty(),
            "plan should be non-empty for imbalanced cluster"
        );
        for (shard_id, from_node, to_node) in &plan {
            assert_eq!(*from_node, 1, "moves should come from overloaded node 1");
            assert_eq!(*to_node, 2, "moves should go to underloaded node 2");
            assert!(
                *shard_id >= 1 && *shard_id <= 6,
                "only shards on node 1 should be moved"
            );
        }
    }

    // ── test_no_rebalance_when_balanced ───────────────────────────────────────

    #[test]
    fn test_no_rebalance_when_balanced() {
        // 4 shards on node 1, 4 shards on node 2 → perfectly balanced.
        let registry = make_registry_with_distribution(&[
            (1, 1, "a0", "a1"),
            (2, 1, "a1", "a2"),
            (3, 1, "a2", "a3"),
            (4, 1, "a3", "a4"),
            (5, 2, "b0", "b1"),
            (6, 2, "b1", "b2"),
            (7, 2, "b2", "b3"),
            (8, 2, "b3", "b4"),
        ]);
        let tracker = MigrationTracker::new();
        let plan = compute_rebalance_plan(&registry, &tracker, 0.2, 10);

        assert!(
            plan.is_empty(),
            "plan should be empty for balanced cluster, got {:?}",
            plan
        );
    }

    // ── test_active_migrations_excludes_terminal ──────────────────────────────

    #[test]
    fn test_active_migrations_excludes_terminal() {
        let tracker = MigrationTracker::new();
        let id1 = tracker.begin_migration(10, 1, 2).expect("begin 10");
        let id2 = tracker.begin_migration(11, 1, 2).expect("begin 11");
        let id3 = tracker.begin_migration(12, 1, 2).expect("begin 12");

        tracker.complete_migration(id1);
        tracker.fail_migration(id2, "oops".to_string());

        let active = tracker.active_migrations();
        let active_ids: Vec<Uuid> = active.iter().map(|m| m.id).collect();

        assert!(
            !active_ids.contains(&id1),
            "completed migration should not appear in active list"
        );
        assert!(
            !active_ids.contains(&id2),
            "failed migration should not appear in active list"
        );
        assert!(
            active_ids.contains(&id3),
            "pending migration should appear in active list"
        );
    }

    // ── test_max_concurrent_migrations_respected ──────────────────────────────

    #[test]
    fn test_max_concurrent_migrations_respected() {
        let registry = make_registry_with_distribution(&[
            (1, 1, "a0", "a1"),
            (2, 1, "a1", "a2"),
            (3, 1, "a2", "a3"),
            (4, 1, "a3", "a4"),
            (5, 1, "a4", "a5"),
            (6, 1, "a5", "a6"),
            (7, 2, "b0", "b1"),
            (8, 2, "b1", "b2"),
        ]);
        let tracker = MigrationTracker::new();
        let plan = compute_rebalance_plan(&registry, &tracker, 0.2, 2);

        assert!(
            plan.len() <= 2,
            "plan must not exceed max_concurrent_migrations=2, got {}",
            plan.len()
        );
    }
}
