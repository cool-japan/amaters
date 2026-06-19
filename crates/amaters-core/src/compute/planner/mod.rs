// Copyright 2026 COOLJAPAN OU (Team KitaSan)
// SPDX-License-Identifier: Apache-2.0

pub use super::plan_cache::{CacheKey, CacheStats, CachedPlan, PlanCache, PlanCacheConfig};
use crate::compute::EncryptedType;
use crate::compute::circuit::Circuit;
use crate::compute::predicate::PredicateCompiler;
use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::{CipherBlob, ColumnRef, JoinType, Key, Predicate, Query};
use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::Arc;
/// Logical query plan node
///
/// Represents the *intent* of a query before physical execution details
/// are decided. The logical plan is the subject of optimization rewrites.
#[derive(Debug, Clone)]
pub enum LogicalPlan {
    /// Full table/collection scan
    Scan {
        /// Name of the collection to scan
        collection: String,
    },
    /// Range scan with start/end keys
    RangeScan {
        /// Name of the collection
        collection: String,
        /// Inclusive start key (None = beginning)
        start_key: Option<Vec<u8>>,
        /// Exclusive end key (None = end)
        end_key: Option<Vec<u8>>,
    },
    /// Filter with predicate (operates on encrypted data via FHE)
    Filter {
        /// Input plan to filter
        input: Box<LogicalPlan>,
        /// Predicate to evaluate
        predicate: Predicate,
    },
    /// Projection (select specific columns)
    Project {
        /// Input plan to project
        input: Box<LogicalPlan>,
        /// Column names to retain
        columns: Vec<String>,
    },
    /// Limit number of results
    Limit {
        /// Input plan to limit
        input: Box<LogicalPlan>,
        /// Maximum number of results
        count: usize,
    },
    /// Point lookup by key
    PointLookup {
        /// Collection name
        collection: String,
        /// Key to look up
        key: Key,
    },
    /// Two-collection join
    Join {
        /// Left input plan
        left: Box<LogicalPlan>,
        /// Right input plan
        right: Box<LogicalPlan>,
        /// Join condition
        on: Predicate,
        /// Join type (Inner / Left / Right)
        join_type: JoinType,
    },
}
/// Physical query plan (executable)
///
/// Each variant maps directly to a concrete execution strategy.
#[derive(Debug, Clone)]
pub enum PhysicalPlan {
    /// Sequential full scan
    SeqScan {
        /// Collection to scan
        collection: String,
    },
    /// Index/range scan (pushdown to storage layer)
    IndexScan {
        /// Collection to scan
        collection: String,
        /// Inclusive start key
        start: Option<Vec<u8>>,
        /// Exclusive end key
        end: Option<Vec<u8>>,
    },
    /// FHE filter evaluation (evaluated on encrypted data)
    FheFilter {
        /// Input physical plan
        input: Box<PhysicalPlan>,
        /// Compiled FHE circuit for the filter
        circuit: Circuit,
        /// Original predicate (kept for introspection / explain)
        predicate: Predicate,
    },
    /// Client-side projection
    Projection {
        /// Input physical plan
        input: Box<PhysicalPlan>,
        /// Columns to retain
        columns: Vec<String>,
    },
    /// Limit result count
    Limit {
        /// Input physical plan
        input: Box<PhysicalPlan>,
        /// Maximum results
        count: usize,
    },
    /// Point lookup by key
    PointGet {
        /// Collection name
        collection: String,
        /// Key to look up
        key: Key,
    },
    /// Nested-loop join — O(n*m), used for encrypted-key / non-Eq predicates
    NestedLoopJoin {
        /// Outer (driving) side
        outer: Box<PhysicalPlan>,
        /// Build (inner) side iterated for every outer row
        build: Box<PhysicalPlan>,
        /// Join condition
        on: Predicate,
        /// Join type
        join_type: JoinType,
    },
    /// Hash join — O(n+m), used when the join condition is a single Eq predicate
    HashJoin {
        /// Probe side (larger estimated input)
        probe: Box<PhysicalPlan>,
        /// Build side hashed into memory (smaller estimated input)
        build: Box<PhysicalPlan>,
        /// Join condition (must be Predicate::Eq)
        on: Predicate,
        /// Join type
        join_type: JoinType,
    },
}
/// Cost estimate for a physical plan
#[derive(Debug, Clone)]
pub struct PlanCost {
    /// Estimated number of rows touched
    pub estimated_rows: u64,
    /// Estimated number of FHE gate operations
    pub estimated_fhe_ops: u64,
    /// Estimated I/O bytes transferred
    pub estimated_io_bytes: u64,
    /// Aggregated scalar cost (lower is better)
    pub total_cost: f64,
}
impl PlanCost {
    /// Cost weight per byte of I/O
    const IO_COST_PER_BYTE: f64 = 0.001;
    /// Cost weight per FHE gate operation (FHE is *very* expensive)
    const FHE_COST_PER_OP: f64 = 100.0;
    /// Cost weight per row scanned
    const SCAN_COST_PER_ROW: f64 = 0.01;
    /// Fixed cost per point lookup
    const POINT_LOOKUP_COST: f64 = 1.0;
    /// Compute the total cost from the individual estimates
    fn compute(estimated_rows: u64, estimated_fhe_ops: u64, estimated_io_bytes: u64) -> Self {
        let total_cost = (estimated_rows as f64 * Self::SCAN_COST_PER_ROW)
            + (estimated_fhe_ops as f64 * Self::FHE_COST_PER_OP)
            + (estimated_io_bytes as f64 * Self::IO_COST_PER_BYTE);
        Self {
            estimated_rows,
            estimated_fhe_ops,
            estimated_io_bytes,
            total_cost,
        }
    }
}
impl std::fmt::Display for PlanCost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PlanCost(rows={}, fhe_ops={}, io_bytes={}, total={:.2})",
            self.estimated_rows, self.estimated_fhe_ops, self.estimated_io_bytes, self.total_cost
        )
    }
}
/// Statistics used for cost estimation
///
/// Maintains per-collection cardinality estimates and global latency hints
/// so that the planner can make informed decisions.
pub struct PlannerStats {
    /// Estimated row count per collection
    pub estimated_collection_sizes: DashMap<String, u64>,
    /// Average value size in bytes across all collections
    pub average_value_size: u64,
    /// Estimated microsecond latency of a single FHE gate operation
    pub fhe_op_latency_us: u64,
    /// Cost of a single FHE comparison operation (Eq / Lt / Gt / Lte / Gte)
    pub fhe_comparison_cost: f64,
    /// Cost of a single FHE boolean operation (And / Or)
    pub fhe_boolean_cost: f64,
}
impl PlannerStats {
    /// Create default statistics with reasonable starting values
    fn new() -> Self {
        Self {
            estimated_collection_sizes: DashMap::new(),
            average_value_size: 256,
            fhe_op_latency_us: 1000,
            fhe_comparison_cost: 100.0,
            fhe_boolean_cost: 10.0,
        }
    }
    /// Return the estimated size of a collection, defaulting to 1000
    fn collection_size(&self, collection: &str) -> u64 {
        self.estimated_collection_sizes
            .get(collection)
            .map(|v| *v)
            .unwrap_or(1000)
    }
    /// Update the estimated size for a collection
    pub fn set_collection_size(&self, collection: impl Into<String>, size: u64) {
        self.estimated_collection_sizes
            .insert(collection.into(), size);
    }
    /// Estimate the fraction of rows a predicate will pass (0.0–1.0).
    ///
    /// Heuristics (no histograms available):
    /// - `Eq`            → 0.001  (high selectivity, rare match)
    /// - `Lt/Gt/Lte/Gte` → 0.3   (moderate selectivity)
    /// - `And(p1, p2)`   → s1 * s2
    /// - `Or(p1, p2)`    → 1 - (1-s1)*(1-s2)
    /// - `Not(p)`        → 1 - s
    pub fn predicate_selectivity(&self, pred: &Predicate) -> f64 {
        match pred {
            Predicate::Eq(_, _) => 0.001,
            Predicate::Lt(_, _)
            | Predicate::Gt(_, _)
            | Predicate::Lte(_, _)
            | Predicate::Gte(_, _) => 0.3,
            Predicate::And(p1, p2) => {
                self.predicate_selectivity(p1) * self.predicate_selectivity(p2)
            }
            Predicate::Or(p1, p2) => {
                let s1 = self.predicate_selectivity(p1);
                let s2 = self.predicate_selectivity(p2);
                1.0 - (1.0 - s1) * (1.0 - s2)
            }
            Predicate::Not(inner) => 1.0 - self.predicate_selectivity(inner),
        }
    }
    /// Estimate the relative FHE cost of evaluating a predicate (in abstract units).
    ///
    /// - Leaf comparisons (`Eq/Lt/Gt/Lte/Gte`) each cost 1.0 comparison unit.
    /// - `And`/`Or` cost 1.0 boolean-op + sum of children costs.
    /// - `Not` costs 0.5 boolean-op + child cost.
    pub fn predicate_fhe_cost(&self, pred: &Predicate) -> f64 {
        match pred {
            Predicate::Eq(_, _)
            | Predicate::Lt(_, _)
            | Predicate::Gt(_, _)
            | Predicate::Lte(_, _)
            | Predicate::Gte(_, _) => self.fhe_comparison_cost,
            Predicate::And(p1, p2) | Predicate::Or(p1, p2) => {
                self.fhe_boolean_cost + self.predicate_fhe_cost(p1) + self.predicate_fhe_cost(p2)
            }
            Predicate::Not(inner) => self.fhe_boolean_cost * 0.5 + self.predicate_fhe_cost(inner),
        }
    }
}
impl Default for PlannerStats {
    fn default() -> Self {
        Self::new()
    }
}
/// Query planner that converts `Query` into optimized `PhysicalPlan`
///
/// The planner applies predicate pushdown, filter merging, and cost-based
/// selection to produce an execution plan that minimises expensive FHE
/// operations and I/O.
///
/// Optionally maintains a plan cache to avoid re-planning identical queries.
pub struct QueryPlanner {
    /// Statistics for cost estimation
    stats: Arc<PlannerStats>,
    /// Optional plan cache
    cache: Option<Arc<PlanCache>>,
}
impl QueryPlanner {
    /// Create a new query planner with default statistics
    pub fn new() -> Self {
        Self {
            stats: Arc::new(PlannerStats::new()),
            cache: None,
        }
    }
    /// Create a planner with custom statistics
    pub fn with_stats(stats: Arc<PlannerStats>) -> Self {
        Self { stats, cache: None }
    }
    /// Enable plan caching with the given configuration
    pub fn with_cache(mut self, config: PlanCacheConfig) -> Self {
        self.cache = Some(Arc::new(PlanCache::new(config)));
        self
    }
    /// Get a reference to the planner statistics
    pub fn stats(&self) -> &PlannerStats {
        &self.stats
    }
    /// Get a reference to the plan cache, if enabled
    pub fn plan_cache(&self) -> Option<&PlanCache> {
        self.cache.as_deref()
    }
    /// Return cache statistics, or default stats if caching is not enabled
    pub fn cache_stats(&self) -> CacheStats {
        self.cache
            .as_ref()
            .map(|c| c.cache_stats())
            .unwrap_or_default()
    }
    /// Invalidate all cached plans (e.g., after a schema change)
    pub fn invalidate_all(&self) {
        if let Some(cache) = &self.cache {
            cache.invalidate_all();
        }
    }
    /// Invalidate cached plans matching a prefix (e.g., a collection name)
    pub fn invalidate_prefix(&self, prefix: &str) {
        if let Some(cache) = &self.cache {
            cache.invalidate_prefix(prefix);
        }
    }
    /// Plan a query
    ///
    /// If caching is enabled, checks the cache first and returns a cached
    /// plan if one exists and has not expired. Otherwise, plans the query
    /// from scratch and inserts the result into the cache.
    pub fn plan(&self, query: &Query) -> Result<PhysicalPlan> {
        let cache_key = CacheKey::from_query(query);
        if let Some(cache) = &self.cache {
            if let Some(cached_plan) = cache.get(&cache_key) {
                return Ok(cached_plan);
            }
        }
        let logical = self.to_logical(query)?;
        let optimized = self.optimize_logical(logical);
        let physical = self.to_physical(&optimized)?;
        if let Some(cache) = &self.cache {
            let normalized = CacheKey::normalize(&format!("{:?}", query));
            cache.insert(cache_key, physical.clone(), normalized);
        }
        Ok(physical)
    }
    /// Convert a high-level `Query` into a `LogicalPlan`
    fn to_logical(&self, query: &Query) -> Result<LogicalPlan> {
        match query {
            Query::Get { collection, key } => Ok(LogicalPlan::PointLookup {
                collection: collection.clone(),
                key: key.clone(),
            }),
            Query::Filter {
                collection,
                predicate,
            } => Ok(LogicalPlan::Filter {
                input: Box::new(LogicalPlan::Scan {
                    collection: collection.clone(),
                }),
                predicate: predicate.clone(),
            }),
            Query::Range {
                collection,
                start,
                end,
            } => Ok(LogicalPlan::RangeScan {
                collection: collection.clone(),
                start_key: Some(start.to_vec()),
                end_key: Some(end.to_vec()),
            }),
            Query::Set { collection, .. } => Ok(LogicalPlan::Scan {
                collection: collection.clone(),
            }),
            Query::Delete { collection, key } => Ok(LogicalPlan::PointLookup {
                collection: collection.clone(),
                key: key.clone(),
            }),
            Query::Update {
                collection,
                predicate,
                ..
            } => Ok(LogicalPlan::Filter {
                input: Box::new(LogicalPlan::Scan {
                    collection: collection.clone(),
                }),
                predicate: predicate.clone(),
            }),
            Query::Join {
                left_collection,
                right_collection,
                on,
                join_type,
                left_limit,
                right_limit,
            } => {
                let mut left: LogicalPlan = LogicalPlan::Scan {
                    collection: left_collection.clone(),
                };
                if let Some(n) = left_limit {
                    left = LogicalPlan::Limit {
                        input: Box::new(left),
                        count: *n,
                    };
                }
                let mut right: LogicalPlan = LogicalPlan::Scan {
                    collection: right_collection.clone(),
                };
                if let Some(n) = right_limit {
                    right = LogicalPlan::Limit {
                        input: Box::new(right),
                        count: *n,
                    };
                }
                Ok(LogicalPlan::Join {
                    left: Box::new(left),
                    right: Box::new(right),
                    on: on.clone(),
                    join_type: join_type.clone(),
                })
            }
        }
    }
    /// Apply all logical optimization passes
    fn optimize_logical(&self, plan: LogicalPlan) -> LogicalPlan {
        let plan = self.push_predicates_down(plan);
        let plan = self.merge_filters(plan);
        let plan = self.convert_filter_to_range_scan(plan);
        self.reorder_predicates_by_cost(plan)
    }
    /// Predicate pushdown: move filters closer to the data source
    ///
    /// Rules applied:
    /// - `Filter(And(p1, p2), input)` (non-Limit input) → split, recurse each conjunct.
    /// - `Filter(Project(input, cols), pred)` → push the filter below the projection.
    /// - `Filter(Join{..}, pred)` → push single-side conjuncts into the appropriate join arm.
    /// - `Filter(Filter(input, p1), p2)` is handled by `merge_filters`.
    fn push_predicates_down(&self, plan: LogicalPlan) -> LogicalPlan {
        match plan {
            LogicalPlan::Filter {
                input,
                predicate: Predicate::And(p1, p2),
            } if !matches!(*input, LogicalPlan::Limit { .. }) => {
                let inner = LogicalPlan::Filter {
                    input,
                    predicate: *p2,
                };
                let outer = LogicalPlan::Filter {
                    input: Box::new(inner),
                    predicate: *p1,
                };
                self.push_predicates_down(outer)
            }
            LogicalPlan::Filter { input, predicate }
                if matches!(*input, LogicalPlan::Join { .. }) =>
            {
                if let LogicalPlan::Join {
                    left,
                    right,
                    on,
                    join_type,
                } = *input
                {
                    let left_cols = Self::referenced_columns(&predicate);
                    let right_input_cols = Self::plan_output_columns(&right);
                    let left_input_cols = Self::plan_output_columns(&left);
                    let touches_left = left_cols.iter().any(|c| left_input_cols.contains(c));
                    let touches_right = left_cols.iter().any(|c| right_input_cols.contains(c));
                    match (touches_left, touches_right) {
                        (true, false) => {
                            let new_left = self.push_predicates_down(LogicalPlan::Filter {
                                input: left,
                                predicate,
                            });
                            self.push_predicates_down(LogicalPlan::Join {
                                left: Box::new(new_left),
                                right,
                                on,
                                join_type,
                            })
                        }
                        (false, true) => {
                            let new_right = self.push_predicates_down(LogicalPlan::Filter {
                                input: right,
                                predicate,
                            });
                            self.push_predicates_down(LogicalPlan::Join {
                                left,
                                right: Box::new(new_right),
                                on,
                                join_type,
                            })
                        }
                        _ => {
                            let joined = self.push_predicates_down(LogicalPlan::Join {
                                left,
                                right,
                                on,
                                join_type,
                            });
                            LogicalPlan::Filter {
                                input: Box::new(joined),
                                predicate,
                            }
                        }
                    }
                } else {
                    unreachable!("guard confirmed Join variant")
                }
            }
            LogicalPlan::Filter { input, predicate } => {
                let optimized_input = self.push_predicates_down(*input);
                match optimized_input {
                    LogicalPlan::Project {
                        input: proj_input,
                        columns,
                    } => {
                        let pred_cols = Self::referenced_columns(&predicate);
                        let proj_set: HashSet<&str> = columns.iter().map(|c| c.as_str()).collect();
                        if pred_cols.iter().all(|c| proj_set.contains(c.as_str())) {
                            LogicalPlan::Project {
                                input: Box::new(LogicalPlan::Filter {
                                    input: proj_input,
                                    predicate,
                                }),
                                columns,
                            }
                        } else {
                            let mut extended_cols = columns.clone();
                            for col in &pred_cols {
                                if !proj_set.contains(col.as_str()) {
                                    extended_cols.push(col.clone());
                                }
                            }
                            LogicalPlan::Project {
                                input: Box::new(LogicalPlan::Filter {
                                    input: Box::new(LogicalPlan::Project {
                                        input: proj_input,
                                        columns: extended_cols,
                                    }),
                                    predicate,
                                }),
                                columns,
                            }
                        }
                    }
                    other => LogicalPlan::Filter {
                        input: Box::new(other),
                        predicate,
                    },
                }
            }
            LogicalPlan::Project { input, columns } => LogicalPlan::Project {
                input: Box::new(self.push_predicates_down(*input)),
                columns,
            },
            LogicalPlan::Limit { input, count } => LogicalPlan::Limit {
                input: Box::new(self.push_predicates_down(*input)),
                count,
            },
            LogicalPlan::Join {
                left,
                right,
                on,
                join_type,
            } => LogicalPlan::Join {
                left: Box::new(self.push_predicates_down(*left)),
                right: Box::new(self.push_predicates_down(*right)),
                on,
                join_type,
            },
            other => other,
        }
    }
    /// Collect the set of column names that a plan might output.
    ///
    /// Used to determine whether a predicate touches columns from a specific
    /// join arm. For scans we have no schema, so we return an empty set (which
    /// causes cross-side classification and keeps the filter above the join,
    /// the safe default).
    fn plan_output_columns(plan: &LogicalPlan) -> HashSet<String> {
        match plan {
            LogicalPlan::Project { columns, .. } => columns.iter().cloned().collect(),
            _ => HashSet::new(),
        }
    }
    /// Merge adjacent filters into a single AND predicate
    ///
    /// `Filter(Filter(input, p1), p2)` => `Filter(input, And(p1, p2))`
    fn merge_filters(&self, plan: LogicalPlan) -> LogicalPlan {
        match plan {
            LogicalPlan::Filter { input, predicate } => {
                let optimized_input = self.merge_filters(*input);
                match optimized_input {
                    LogicalPlan::Filter {
                        input: inner_input,
                        predicate: inner_pred,
                    } => LogicalPlan::Filter {
                        input: inner_input,
                        predicate: Predicate::And(Box::new(inner_pred), Box::new(predicate)),
                    },
                    other => LogicalPlan::Filter {
                        input: Box::new(other),
                        predicate,
                    },
                }
            }
            LogicalPlan::Project { input, columns } => LogicalPlan::Project {
                input: Box::new(self.merge_filters(*input)),
                columns,
            },
            LogicalPlan::Limit { input, count } => LogicalPlan::Limit {
                input: Box::new(self.merge_filters(*input)),
                count,
            },
            LogicalPlan::Join {
                left,
                right,
                on,
                join_type,
            } => LogicalPlan::Join {
                left: Box::new(self.merge_filters(*left)),
                right: Box::new(self.merge_filters(*right)),
                on,
                join_type,
            },
            other => other,
        }
    }
    /// Convert a filter on key range into a `RangeScan` when possible
    ///
    /// If a `Filter(Scan(collection), pred)` has a predicate that is purely
    /// a key-range comparison (Gt/Lt/Gte/Lte on the `_key` column), we can
    /// replace the scan+filter with a more efficient `RangeScan`.
    fn convert_filter_to_range_scan(&self, plan: LogicalPlan) -> LogicalPlan {
        match plan {
            LogicalPlan::Filter { input, predicate } => {
                let optimized_input = self.convert_filter_to_range_scan(*input);
                if let LogicalPlan::Scan { ref collection } = optimized_input {
                    if let Some((start, end)) = Self::extract_key_range(&predicate) {
                        return LogicalPlan::RangeScan {
                            collection: collection.clone(),
                            start_key: start,
                            end_key: end,
                        };
                    }
                }
                LogicalPlan::Filter {
                    input: Box::new(optimized_input),
                    predicate,
                }
            }
            LogicalPlan::Project { input, columns } => LogicalPlan::Project {
                input: Box::new(self.convert_filter_to_range_scan(*input)),
                columns,
            },
            LogicalPlan::Limit { input, count } => LogicalPlan::Limit {
                input: Box::new(self.convert_filter_to_range_scan(*input)),
                count,
            },
            LogicalPlan::Join {
                left,
                right,
                on,
                join_type,
            } => LogicalPlan::Join {
                left: Box::new(self.convert_filter_to_range_scan(*left)),
                right: Box::new(self.convert_filter_to_range_scan(*right)),
                on,
                join_type,
            },
            other => other,
        }
    }
    /// Reorder conjuncts in `And` predicates so the cheaper (lower
    /// selectivity × fhe_cost) predicate is evaluated first (outer And-branch).
    ///
    /// This is a pure structural rewrite; semantics are unchanged because AND
    /// is commutative.
    fn reorder_predicates_by_cost(&self, plan: LogicalPlan) -> LogicalPlan {
        match plan {
            LogicalPlan::Filter { input, predicate } => {
                let reordered_pred = self.reorder_pred(&predicate);
                let optimized_input = self.reorder_predicates_by_cost(*input);
                LogicalPlan::Filter {
                    input: Box::new(optimized_input),
                    predicate: reordered_pred,
                }
            }
            LogicalPlan::Project { input, columns } => LogicalPlan::Project {
                input: Box::new(self.reorder_predicates_by_cost(*input)),
                columns,
            },
            LogicalPlan::Limit { input, count } => LogicalPlan::Limit {
                input: Box::new(self.reorder_predicates_by_cost(*input)),
                count,
            },
            LogicalPlan::Join {
                left,
                right,
                on,
                join_type,
            } => {
                let reordered_on = self.reorder_pred(&on);
                LogicalPlan::Join {
                    left: Box::new(self.reorder_predicates_by_cost(*left)),
                    right: Box::new(self.reorder_predicates_by_cost(*right)),
                    on: reordered_on,
                    join_type,
                }
            }
            other => other,
        }
    }
    /// Recursively reorder `And` sub-predicates cheapest-first.
    fn reorder_pred(&self, pred: &Predicate) -> Predicate {
        match pred {
            Predicate::And(p1, p2) => {
                let r1 = self.reorder_pred(p1);
                let r2 = self.reorder_pred(p2);
                let cost1 =
                    self.stats.predicate_selectivity(&r1) * self.stats.predicate_fhe_cost(&r1);
                let cost2 =
                    self.stats.predicate_selectivity(&r2) * self.stats.predicate_fhe_cost(&r2);
                if cost1 <= cost2 {
                    Predicate::And(Box::new(r1), Box::new(r2))
                } else {
                    Predicate::And(Box::new(r2), Box::new(r1))
                }
            }
            Predicate::Or(p1, p2) => Predicate::Or(
                Box::new(self.reorder_pred(p1)),
                Box::new(self.reorder_pred(p2)),
            ),
            Predicate::Not(inner) => Predicate::Not(Box::new(self.reorder_pred(inner))),
            other => other.clone(),
        }
    }
    /// Convert an optimized logical plan into a physical plan
    fn to_physical(&self, plan: &LogicalPlan) -> Result<PhysicalPlan> {
        match plan {
            LogicalPlan::Scan { collection } => Ok(PhysicalPlan::SeqScan {
                collection: collection.clone(),
            }),
            LogicalPlan::RangeScan {
                collection,
                start_key,
                end_key,
            } => Ok(PhysicalPlan::IndexScan {
                collection: collection.clone(),
                start: start_key.clone(),
                end: end_key.clone(),
            }),
            LogicalPlan::Filter { input, predicate } => {
                let physical_input = self.to_physical(input)?;
                let circuit = self.compile_predicate_circuit(predicate)?;
                Ok(PhysicalPlan::FheFilter {
                    input: Box::new(physical_input),
                    circuit,
                    predicate: predicate.clone(),
                })
            }
            LogicalPlan::Project { input, columns } => {
                let physical_input = self.to_physical(input)?;
                Ok(PhysicalPlan::Projection {
                    input: Box::new(physical_input),
                    columns: columns.clone(),
                })
            }
            LogicalPlan::Limit { input, count } => {
                let physical_input = self.to_physical(input)?;
                Ok(PhysicalPlan::Limit {
                    input: Box::new(physical_input),
                    count: *count,
                })
            }
            LogicalPlan::PointLookup { collection, key } => Ok(PhysicalPlan::PointGet {
                collection: collection.clone(),
                key: key.clone(),
            }),
            LogicalPlan::Join {
                left,
                right,
                on,
                join_type,
            } => {
                let left_phys = self.to_physical(left)?;
                let right_phys = self.to_physical(right)?;
                let left_rows = self.estimate_cost(&left_phys).estimated_rows;
                let right_rows = self.estimate_cost(&right_phys).estimated_rows;
                let use_hash = matches!(on, Predicate::Eq(_, _));
                if use_hash {
                    let (probe, build) = if left_rows <= right_rows {
                        (right_phys, left_phys)
                    } else {
                        (left_phys, right_phys)
                    };
                    Ok(PhysicalPlan::HashJoin {
                        probe: Box::new(probe),
                        build: Box::new(build),
                        on: on.clone(),
                        join_type: join_type.clone(),
                    })
                } else {
                    let (outer, build) = if left_rows <= right_rows {
                        (left_phys, right_phys)
                    } else {
                        (right_phys, left_phys)
                    };
                    Ok(PhysicalPlan::NestedLoopJoin {
                        outer: Box::new(outer),
                        build: Box::new(build),
                        on: on.clone(),
                        join_type: join_type.clone(),
                    })
                }
            }
        }
    }
    /// Estimate the cost of a physical plan
    pub fn estimate_cost(&self, plan: &PhysicalPlan) -> PlanCost {
        match plan {
            PhysicalPlan::SeqScan { collection } => {
                let rows = self.stats.collection_size(collection);
                let io_bytes = rows * self.stats.average_value_size;
                PlanCost::compute(rows, 0, io_bytes)
            }
            PhysicalPlan::IndexScan {
                collection,
                start,
                end,
            } => {
                let total = self.stats.collection_size(collection);
                let selectivity = match (start, end) {
                    (Some(_), Some(_)) => 0.10,
                    (Some(_), None) | (None, Some(_)) => 0.30,
                    (None, None) => 1.0,
                };
                let rows = ((total as f64) * selectivity).max(1.0) as u64;
                let io_bytes = rows * self.stats.average_value_size;
                PlanCost::compute(rows, 0, io_bytes)
            }
            PhysicalPlan::FheFilter { input, circuit, .. } => {
                let input_cost = self.estimate_cost(input);
                let fhe_ops = input_cost.estimated_rows * (circuit.gate_count as u64);
                let output_rows = (input_cost.estimated_rows / 2).max(1);
                let io_bytes = output_rows * self.stats.average_value_size;
                PlanCost::compute(
                    input_cost.estimated_rows,
                    input_cost.estimated_fhe_ops + fhe_ops,
                    input_cost.estimated_io_bytes + io_bytes,
                )
            }
            PhysicalPlan::Projection { input, .. } => {
                let mut cost = self.estimate_cost(input);
                cost.estimated_io_bytes = (cost.estimated_io_bytes as f64 * 0.8) as u64;
                cost.total_cost = (cost.estimated_rows as f64 * PlanCost::SCAN_COST_PER_ROW)
                    + (cost.estimated_fhe_ops as f64 * PlanCost::FHE_COST_PER_OP)
                    + (cost.estimated_io_bytes as f64 * PlanCost::IO_COST_PER_BYTE);
                cost
            }
            PhysicalPlan::Limit { input, count } => {
                let input_cost = self.estimate_cost(input);
                let rows = (*count as u64).min(input_cost.estimated_rows);
                let io_bytes = rows * self.stats.average_value_size;
                PlanCost::compute(rows, input_cost.estimated_fhe_ops, io_bytes)
            }
            PhysicalPlan::PointGet { .. } => PlanCost::compute(1, 0, self.stats.average_value_size),
            PhysicalPlan::NestedLoopJoin { outer, build, .. } => {
                let outer_cost = self.estimate_cost(outer);
                let build_cost = self.estimate_cost(build);
                let outer_rows = outer_cost.estimated_rows;
                let build_rows = build_cost.estimated_rows;
                let fhe_ops = outer_rows.saturating_mul(build_rows);
                let estimated_rows = outer_rows.saturating_mul(build_rows) / 2;
                let io_bytes = outer_cost.estimated_io_bytes + build_cost.estimated_io_bytes;
                PlanCost::compute(estimated_rows, fhe_ops, io_bytes)
            }
            PhysicalPlan::HashJoin { probe, build, .. } => {
                let probe_cost = self.estimate_cost(probe);
                let build_cost = self.estimate_cost(build);
                let probe_rows = probe_cost.estimated_rows;
                let build_rows = build_cost.estimated_rows;
                let fhe_ops = probe_cost.estimated_fhe_ops + build_cost.estimated_fhe_ops;
                let estimated_rows = probe_rows.saturating_mul(build_rows) / 2;
                let io_bytes = probe_cost.estimated_io_bytes + build_cost.estimated_io_bytes;
                PlanCost::compute(estimated_rows, fhe_ops, io_bytes)
            }
        }
    }
    /// Compare two physical plans by cost and return the cheaper one
    pub fn choose_cheaper<'a>(&self, a: &'a PhysicalPlan, b: &'a PhysicalPlan) -> &'a PhysicalPlan {
        let cost_a = self.estimate_cost(a);
        let cost_b = self.estimate_cost(b);
        if cost_a.total_cost <= cost_b.total_cost {
            a
        } else {
            b
        }
    }
    /// Extract all column names referenced in a predicate
    fn referenced_columns(predicate: &Predicate) -> Vec<String> {
        let mut cols = Vec::new();
        Self::collect_columns(predicate, &mut cols);
        cols.sort();
        cols.dedup();
        cols
    }
    fn collect_columns(predicate: &Predicate, out: &mut Vec<String>) {
        match predicate {
            Predicate::Eq(col, _)
            | Predicate::Gt(col, _)
            | Predicate::Lt(col, _)
            | Predicate::Gte(col, _)
            | Predicate::Lte(col, _) => {
                out.push(col.name.clone());
            }
            Predicate::And(l, r) | Predicate::Or(l, r) => {
                Self::collect_columns(l, out);
                Self::collect_columns(r, out);
            }
            Predicate::Not(inner) => {
                Self::collect_columns(inner, out);
            }
        }
    }
    /// Try to extract a key range from a predicate on the `_key` column
    ///
    /// Returns `Some((start, end))` where either bound may be `None`.
    /// Returns `None` if the predicate is not a simple key-range filter.
    fn extract_key_range(predicate: &Predicate) -> Option<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        match predicate {
            Predicate::Gt(col, blob) if col.name == "_key" => {
                Some((Some(blob.as_bytes().to_vec()), None))
            }
            Predicate::Gte(col, blob) if col.name == "_key" => {
                Some((Some(blob.as_bytes().to_vec()), None))
            }
            Predicate::Lt(col, blob) if col.name == "_key" => {
                Some((None, Some(blob.as_bytes().to_vec())))
            }
            Predicate::Lte(col, blob) if col.name == "_key" => {
                Some((None, Some(blob.as_bytes().to_vec())))
            }
            Predicate::And(left, right) => {
                let lr = Self::extract_key_range(left);
                let rr = Self::extract_key_range(right);
                match (lr, rr) {
                    (Some((s1, e1)), Some((s2, e2))) => {
                        let start = s1.or(s2);
                        let end = e1.or(e2);
                        Some((start, end))
                    }
                    (Some(range), None) | (None, Some(range)) => Some(range),
                    (None, None) => None,
                }
            }
            _ => None,
        }
    }
    /// Compile a predicate into an FHE circuit
    fn compile_predicate_circuit(&self, predicate: &Predicate) -> Result<Circuit> {
        let mut compiler = PredicateCompiler::new();
        compiler.compile(predicate, EncryptedType::U8)
    }
}
impl Default for QueryPlanner {
    fn default() -> Self {
        Self::new()
    }
}
impl std::fmt::Display for LogicalPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_indented(f, 0)
    }
}
impl LogicalPlan {
    fn fmt_indented(&self, f: &mut std::fmt::Formatter<'_>, indent: usize) -> std::fmt::Result {
        let pad = "  ".repeat(indent);
        match self {
            LogicalPlan::Scan { collection } => {
                writeln!(f, "{}Scan({})", pad, collection)
            }
            LogicalPlan::RangeScan {
                collection,
                start_key,
                end_key,
            } => {
                writeln!(
                    f,
                    "{}RangeScan({}, start={}, end={})",
                    pad,
                    collection,
                    start_key.is_some(),
                    end_key.is_some()
                )
            }
            LogicalPlan::Filter { input, predicate } => {
                writeln!(f, "{}Filter(pred={:?})", pad, predicate)?;
                input.fmt_indented(f, indent + 1)
            }
            LogicalPlan::Project { input, columns } => {
                writeln!(f, "{}Project({:?})", pad, columns)?;
                input.fmt_indented(f, indent + 1)
            }
            LogicalPlan::Limit { input, count } => {
                writeln!(f, "{}Limit({})", pad, count)?;
                input.fmt_indented(f, indent + 1)
            }
            LogicalPlan::PointLookup { collection, key } => {
                writeln!(f, "{}PointLookup({}, key={})", pad, collection, key)
            }
            LogicalPlan::Join {
                left,
                right,
                on,
                join_type,
            } => {
                let jt = match join_type {
                    JoinType::Inner => "Inner",
                    JoinType::Left => "Left",
                    JoinType::Right => "Right",
                };
                writeln!(f, "{}{}Join(on={:?})", pad, jt, on)?;
                left.fmt_indented(f, indent + 1)?;
                right.fmt_indented(f, indent + 1)
            }
        }
    }
}
impl std::fmt::Display for PhysicalPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_indented(f, 0)
    }
}
impl PhysicalPlan {
    fn fmt_indented(&self, f: &mut std::fmt::Formatter<'_>, indent: usize) -> std::fmt::Result {
        let pad = "  ".repeat(indent);
        match self {
            PhysicalPlan::SeqScan { collection } => {
                writeln!(f, "{}SeqScan({})", pad, collection)
            }
            PhysicalPlan::IndexScan {
                collection,
                start,
                end,
            } => {
                writeln!(
                    f,
                    "{}IndexScan({}, start={}, end={})",
                    pad,
                    collection,
                    start.is_some(),
                    end.is_some()
                )
            }
            PhysicalPlan::FheFilter {
                input, predicate, ..
            } => {
                writeln!(f, "{}FheFilter(pred={:?})", pad, predicate)?;
                input.fmt_indented(f, indent + 1)
            }
            PhysicalPlan::Projection { input, columns } => {
                writeln!(f, "{}Projection({:?})", pad, columns)?;
                input.fmt_indented(f, indent + 1)
            }
            PhysicalPlan::Limit { input, count } => {
                writeln!(f, "{}Limit({})", pad, count)?;
                input.fmt_indented(f, indent + 1)
            }
            PhysicalPlan::PointGet { collection, key } => {
                writeln!(f, "{}PointGet({}, key={})", pad, collection, key)
            }
            PhysicalPlan::NestedLoopJoin {
                outer,
                build,
                on,
                join_type,
            } => {
                let jt = match join_type {
                    JoinType::Inner => "Inner",
                    JoinType::Left => "Left",
                    JoinType::Right => "Right",
                };
                writeln!(f, "{}NestedLoopJoin[{}](on={:?})", pad, jt, on)?;
                outer.fmt_indented(f, indent + 1)?;
                build.fmt_indented(f, indent + 1)
            }
            PhysicalPlan::HashJoin {
                probe,
                build,
                on,
                join_type,
            } => {
                let jt = match join_type {
                    JoinType::Inner => "Inner",
                    JoinType::Left => "Left",
                    JoinType::Right => "Right",
                };
                writeln!(f, "{}HashJoin[{}](on={:?})", pad, jt, on)?;
                probe.fmt_indented(f, indent + 1)?;
                build.fmt_indented(f, indent + 1)
            }
        }
    }
}

#[cfg(test)]
mod tests;
