//! Versioned document migration framework.
//!
//! Provides a registry of version-to-version migrations and computes
//! the shortest migration path between any two reachable versions using BFS.

use crate::server::{ServerError, ServerResult};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

/// A mutable document context passed through each migration step.
pub struct MigrationContext {
    doc: Value,
}

impl MigrationContext {
    /// Create a new migration context from a document value.
    pub fn new(doc: Value) -> Self {
        Self { doc }
    }

    /// Get a field from the document by key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.doc.get(key)
    }

    /// Set a field in the document.
    pub fn set(&mut self, key: &str, value: Value) {
        if let Value::Object(map) = &mut self.doc {
            map.insert(key.to_owned(), value);
        }
    }

    /// Remove a field from the document, returning its previous value if present.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        if let Value::Object(map) = &mut self.doc {
            map.remove(key)
        } else {
            None
        }
    }

    /// Consume the context and return the underlying document.
    pub fn into_doc(self) -> Value {
        self.doc
    }

    /// Borrow the underlying document value.
    pub fn doc(&self) -> &Value {
        &self.doc
    }
}

/// A single version-to-version migration step.
pub trait Migration: Send + Sync {
    /// The version this migration migrates FROM.
    #[allow(clippy::wrong_self_convention)]
    fn from_version(&self) -> (u64, u64, u64);

    /// The version this migration migrates TO.
    fn to_version(&self) -> (u64, u64, u64);

    /// Human-readable description of what this migration does.
    fn description(&self) -> &str;

    /// Apply the migration to a document context.
    fn migrate(&self, ctx: &mut MigrationContext) -> ServerResult<()>;
}

/// An ordered sequence of migration steps computed by [`MigrationRegistry::plan`].
///
/// Holds a reference to the registry's migration list and the indices of the
/// steps to apply, avoiding any copies of the migration objects.
pub struct MigrationPlan<'a> {
    migrations: &'a [Box<dyn Migration>],
    indices: Vec<usize>,
}

impl<'a> std::fmt::Debug for MigrationPlan<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MigrationPlan")
            .field("steps", &self.indices.len())
            .finish()
    }
}

impl<'a> MigrationPlan<'a> {
    fn new(migrations: &'a [Box<dyn Migration>], indices: Vec<usize>) -> Self {
        Self {
            migrations,
            indices,
        }
    }

    /// Returns true if no migration steps are needed (source == target version).
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Number of migration steps in this plan.
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Apply all migration steps in order to the given context.
    ///
    /// Stops and returns the first error encountered.
    pub fn apply(&self, ctx: &mut MigrationContext) -> ServerResult<()> {
        for &idx in &self.indices {
            self.migrations[idx].migrate(ctx)?;
        }
        Ok(())
    }

    /// Return descriptions of each step in this plan for logging/audit.
    pub fn step_descriptions(&self) -> Vec<&str> {
        self.indices
            .iter()
            .map(|&idx| self.migrations[idx].description())
            .collect()
    }
}

/// Registry of all known migrations.
///
/// Use [`MigrationRegistry::register`] to add migrations, then
/// [`MigrationRegistry::plan`] to compute the shortest path between versions.
pub struct MigrationRegistry {
    migrations: Vec<Box<dyn Migration>>,
}

impl MigrationRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            migrations: Vec::new(),
        }
    }

    /// Register a migration. Returns `&mut Self` for chaining.
    pub fn register(&mut self, m: impl Migration + 'static) -> &mut Self {
        self.migrations.push(Box::new(m));
        self
    }

    /// Compute the shortest migration path from `from` to `to` using BFS.
    ///
    /// Returns an empty plan if `from == to`.
    /// Returns `Err(ServerError::Migration(...))` if no path exists.
    ///
    /// When multiple paths of equal length exist, BFS naturally picks one;
    /// the specific choice among equal-length paths is not guaranteed.
    pub fn plan(
        &self,
        from: (u64, u64, u64),
        to: (u64, u64, u64),
    ) -> ServerResult<MigrationPlan<'_>> {
        if from == to {
            return Ok(MigrationPlan::new(&self.migrations, vec![]));
        }

        // Build adjacency list: from_version → Vec<migration_index>
        let mut adj: HashMap<(u64, u64, u64), Vec<usize>> = HashMap::new();
        for (idx, m) in self.migrations.iter().enumerate() {
            adj.entry(m.from_version()).or_default().push(idx);
        }

        // BFS: each queue entry is (current_version, path_of_indices_so_far)
        let mut queue: VecDeque<((u64, u64, u64), Vec<usize>)> = VecDeque::new();
        let mut visited: HashSet<(u64, u64, u64)> = HashSet::new();
        queue.push_back((from, vec![]));
        visited.insert(from);

        while let Some((cur, path)) = queue.pop_front() {
            if let Some(neighbors) = adj.get(&cur) {
                for &idx in neighbors {
                    let next = self.migrations[idx].to_version();
                    let mut new_path = path.clone();
                    new_path.push(idx);

                    if next == to {
                        return Ok(MigrationPlan::new(&self.migrations, new_path));
                    }

                    if !visited.contains(&next) {
                        visited.insert(next);
                        queue.push_back((next, new_path));
                    }
                }
            }
        }

        Err(ServerError::Migration(format!(
            "No migration path from {from:?} to {to:?}"
        )))
    }
}

impl Default for MigrationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A simple migration that sets a single field to a fixed value.
    struct SetFieldMigration {
        from: (u64, u64, u64),
        to: (u64, u64, u64),
        desc: String,
        field: String,
        value: Value,
    }

    impl SetFieldMigration {
        fn new(from: (u64, u64, u64), to: (u64, u64, u64), field: &str, value: Value) -> Self {
            let desc = format!("{from:?} -> {to:?}: set {field}");
            Self {
                from,
                to,
                desc,
                field: field.to_owned(),
                value,
            }
        }
    }

    impl Migration for SetFieldMigration {
        fn from_version(&self) -> (u64, u64, u64) {
            self.from
        }

        fn to_version(&self) -> (u64, u64, u64) {
            self.to
        }

        fn description(&self) -> &str {
            &self.desc
        }

        fn migrate(&self, ctx: &mut MigrationContext) -> ServerResult<()> {
            ctx.set(&self.field, self.value.clone());
            Ok(())
        }
    }

    #[test]
    fn test_linear_path() {
        let mut registry = MigrationRegistry::new();
        registry
            .register(SetFieldMigration::new(
                (0, 1, 0),
                (0, 2, 0),
                "step1",
                json!("applied"),
            ))
            .register(SetFieldMigration::new(
                (0, 2, 0),
                (0, 3, 0),
                "step2",
                json!("applied"),
            ));

        let plan = registry
            .plan((0, 1, 0), (0, 3, 0))
            .expect("plan should exist");
        assert_eq!(plan.len(), 2);

        let mut ctx = MigrationContext::new(json!({}));
        plan.apply(&mut ctx).expect("apply should succeed");

        let doc = ctx.into_doc();
        assert_eq!(doc.get("step1"), Some(&json!("applied")));
        assert_eq!(doc.get("step2"), Some(&json!("applied")));
    }

    #[test]
    fn test_already_at_target_returns_empty_plan() {
        let registry = MigrationRegistry::new();
        let plan = registry
            .plan((0, 2, 0), (0, 2, 0))
            .expect("same-version plan");
        assert!(plan.is_empty());
        assert_eq!(plan.len(), 0);
    }

    #[test]
    fn test_no_path_returns_migration_error() {
        let registry = MigrationRegistry::new(); // empty — no migrations
        let result = registry.plan((0, 1, 0), (0, 9, 0));
        assert!(
            matches!(result, Err(ServerError::Migration(_))),
            "expected Migration error, got: {result:?}"
        );
    }

    #[test]
    fn test_branch_picks_shortest_path() {
        // Graph:
        //   (0,1,0) → (0,2,0) → (0,3,0)  [2 hops]
        //   (0,1,0) → (0,3,0)             [1 hop, direct]
        // BFS should pick the 1-hop direct route.
        let mut registry = MigrationRegistry::new();
        registry
            .register(SetFieldMigration::new(
                (0, 1, 0),
                (0, 2, 0),
                "intermediate",
                json!(true),
            ))
            .register(SetFieldMigration::new(
                (0, 2, 0),
                (0, 3, 0),
                "via_intermediate",
                json!(true),
            ))
            .register(SetFieldMigration::new(
                (0, 1, 0),
                (0, 3, 0),
                "direct",
                json!(true),
            ));

        let plan = registry
            .plan((0, 1, 0), (0, 3, 0))
            .expect("plan should exist");
        assert_eq!(plan.len(), 1, "BFS should pick the direct 1-hop path");

        let mut ctx = MigrationContext::new(json!({}));
        plan.apply(&mut ctx).expect("apply should succeed");
        let doc = ctx.into_doc();
        assert_eq!(doc.get("direct"), Some(&json!(true)));
        assert_eq!(
            doc.get("intermediate"),
            None,
            "2-hop path must not be taken"
        );
    }

    #[test]
    fn test_cycle_terminates() {
        // Cycle: (0,1,0) → (0,2,0) → (0,1,0)
        // Target (0,3,0) is unreachable; BFS must terminate and return error.
        let mut registry = MigrationRegistry::new();
        registry
            .register(SetFieldMigration::new(
                (0, 1, 0),
                (0, 2, 0),
                "fwd",
                json!(1),
            ))
            .register(SetFieldMigration::new(
                (0, 2, 0),
                (0, 1, 0),
                "back",
                json!(2),
            ));

        let result = registry.plan((0, 1, 0), (0, 3, 0));
        assert!(
            matches!(result, Err(ServerError::Migration(_))),
            "expected Migration error for unreachable target, got: {result:?}"
        );
    }

    #[test]
    fn test_step_descriptions() {
        let mut registry = MigrationRegistry::new();
        registry
            .register(SetFieldMigration::new(
                (0, 1, 0),
                (0, 2, 0),
                "f",
                json!(null),
            ))
            .register(SetFieldMigration::new(
                (0, 2, 0),
                (0, 3, 0),
                "g",
                json!(null),
            ));

        let plan = registry.plan((0, 1, 0), (0, 3, 0)).expect("plan");
        let descs = plan.step_descriptions();
        assert_eq!(descs.len(), 2);
        // Each description must be non-empty
        for d in descs {
            assert!(!d.is_empty());
        }
    }

    #[test]
    fn test_migration_context_set_get_remove() {
        let mut ctx = MigrationContext::new(json!({"x": 1}));
        assert_eq!(ctx.get("x"), Some(&json!(1)));
        ctx.set("y", json!(2));
        assert_eq!(ctx.get("y"), Some(&json!(2)));
        ctx.remove("x");
        assert_eq!(ctx.get("x"), None);
        let doc = ctx.into_doc();
        assert_eq!(doc.get("y"), Some(&json!(2)));
    }

    #[test]
    fn test_registry_default() {
        let registry = MigrationRegistry::default();
        assert!(registry.migrations.is_empty());
    }
}
