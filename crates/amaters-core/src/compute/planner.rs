//! Query planner with predicate pushdown and cost-based optimization
//!
//! This module provides a query planner that transforms high-level `Query` objects
//! into optimized physical execution plans. It applies several optimization strategies:
//!
//! 1. **Predicate Pushdown** - Push filter predicates as close to the data source as possible
//! 2. **Filter Merging** - Combine adjacent filter operations into compound predicates
//! 3. **Cost-Based Optimization** - Estimate and compare plan costs to choose the cheapest one
//! 4. **Range Scan Conversion** - Convert key-range filters into efficient range scans
//!
//! # Architecture
//!
//! The planner works in three phases:
//!
//! 1. **Logical Planning** - Convert a `Query` into a `LogicalPlan` tree
//! 2. **Logical Optimization** - Apply rewrite rules (predicate pushdown, filter merge, etc.)
//! 3. **Physical Planning** - Convert the optimized logical plan into a `PhysicalPlan`
//!
//! # Example
//!
//! ```rust,ignore
//! use amaters_core::compute::planner::QueryPlanner;
//! use amaters_core::types::{Query, QueryBuilder, Predicate, col, CipherBlob};
//!
//! let planner = QueryPlanner::new();
//! let query = QueryBuilder::new("users").filter(
//!     Predicate::Gt(col("age"), CipherBlob::new(vec![18]))
//! );
//!
//! let plan = planner.plan(&query)?;
//! let cost = planner.estimate_cost(&plan);
//! println!("Estimated cost: {}", cost.total_cost);
//! ```

use crate::compute::EncryptedType;
use crate::compute::circuit::Circuit;
use crate::compute::predicate::PredicateCompiler;
use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::{CipherBlob, ColumnRef, Key, Predicate, Query};
use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::Arc;

pub use super::plan_cache::{CacheKey, CacheStats, CachedPlan, PlanCache, PlanCacheConfig};

// ---------------------------------------------------------------------------
// Logical plan
// ---------------------------------------------------------------------------

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
}

// ---------------------------------------------------------------------------
// Physical plan
// ---------------------------------------------------------------------------

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
}

// ---------------------------------------------------------------------------
// Cost model
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Planner statistics
// ---------------------------------------------------------------------------

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
}

impl PlannerStats {
    /// Create default statistics with reasonable starting values
    fn new() -> Self {
        Self {
            estimated_collection_sizes: DashMap::new(),
            average_value_size: 256,
            fhe_op_latency_us: 1000,
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
}

impl Default for PlannerStats {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Query Planner
// ---------------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Public entry point
    // -----------------------------------------------------------------------

    /// Plan a query
    ///
    /// If caching is enabled, checks the cache first and returns a cached
    /// plan if one exists and has not expired. Otherwise, plans the query
    /// from scratch and inserts the result into the cache.
    pub fn plan(&self, query: &Query) -> Result<PhysicalPlan> {
        let cache_key = CacheKey::from_query(query);

        // Check cache first
        if let Some(cache) = &self.cache {
            if let Some(cached_plan) = cache.get(&cache_key) {
                return Ok(cached_plan);
            }
        }

        // Plan from scratch
        let logical = self.to_logical(query)?;
        let optimized = self.optimize_logical(logical);
        let physical = self.to_physical(&optimized)?;

        // Insert into cache
        if let Some(cache) = &self.cache {
            let normalized = CacheKey::normalize(&format!("{:?}", query));
            cache.insert(cache_key, physical.clone(), normalized);
        }

        Ok(physical)
    }

    // -----------------------------------------------------------------------
    // Logical plan construction
    // -----------------------------------------------------------------------

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

            Query::Set { collection, .. } => {
                // Write operations do not really need a read plan, but we model
                // them as a point lookup for the target key so that upstream can
                // check for existence first.
                Ok(LogicalPlan::Scan {
                    collection: collection.clone(),
                })
            }

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
        }
    }

    // -----------------------------------------------------------------------
    // Logical optimizations
    // -----------------------------------------------------------------------

    /// Apply all logical optimization passes
    fn optimize_logical(&self, plan: LogicalPlan) -> LogicalPlan {
        let plan = self.push_predicates_down(plan);
        let plan = self.merge_filters(plan);
        self.convert_filter_to_range_scan(plan)
    }

    /// Predicate pushdown: move filters closer to the data source
    ///
    /// Rules applied:
    /// - `Filter(Project(input, cols), pred)` -> if pred only references
    ///   columns in `cols`, push the filter below the projection.
    /// - `Filter(Filter(input, p1), p2)` is handled by `merge_filters`.
    fn push_predicates_down(&self, plan: LogicalPlan) -> LogicalPlan {
        match plan {
            // Rule: push filter below projection when possible
            LogicalPlan::Filter { input, predicate } => {
                let optimized_input = self.push_predicates_down(*input);

                match optimized_input {
                    // Filter over Project -> check if we can push through
                    LogicalPlan::Project {
                        input: proj_input,
                        columns,
                    } => {
                        let pred_cols = Self::referenced_columns(&predicate);
                        let proj_set: HashSet<&str> = columns.iter().map(|c| c.as_str()).collect();

                        if pred_cols.iter().all(|c| proj_set.contains(c.as_str())) {
                            // All predicate columns exist in the projection,
                            // so we can push the filter below.
                            LogicalPlan::Project {
                                input: Box::new(LogicalPlan::Filter {
                                    input: proj_input,
                                    predicate,
                                }),
                                columns,
                            }
                        } else {
                            // Some columns not in projection; need to widen
                            // the projection to include predicate columns,
                            // then re-project afterwards.
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

                    // Filter over Limit: cannot push filter below Limit
                    // because Limit is a cardinality-changing operation on
                    // encrypted data where we cannot peek.
                    other => LogicalPlan::Filter {
                        input: Box::new(other),
                        predicate,
                    },
                }
            }

            // Recurse into other plan nodes
            LogicalPlan::Project { input, columns } => LogicalPlan::Project {
                input: Box::new(self.push_predicates_down(*input)),
                columns,
            },

            LogicalPlan::Limit { input, count } => LogicalPlan::Limit {
                input: Box::new(self.push_predicates_down(*input)),
                count,
            },

            // Leaf nodes are returned unchanged
            other => other,
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
                    } => {
                        // Merge the two predicates with AND
                        LogicalPlan::Filter {
                            input: inner_input,
                            predicate: Predicate::And(Box::new(inner_pred), Box::new(predicate)),
                        }
                    }
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

            other => other,
        }
    }

    // -----------------------------------------------------------------------
    // Physical plan construction
    // -----------------------------------------------------------------------

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
        }
    }

    // -----------------------------------------------------------------------
    // Cost estimation
    // -----------------------------------------------------------------------

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
                // Estimate selectivity: a range scan typically touches a fraction.
                // Without histograms we use a heuristic: if both bounds present
                // assume 10%, one bound 30%, no bounds = full scan.
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
                // FHE filter applies the circuit to every row from the input
                let fhe_ops = input_cost.estimated_rows * (circuit.gate_count as u64);
                // After filter, assume 50% selectivity without better stats
                let output_rows = (input_cost.estimated_rows / 2).max(1);
                let io_bytes = output_rows * self.stats.average_value_size;
                PlanCost::compute(
                    input_cost.estimated_rows,
                    input_cost.estimated_fhe_ops + fhe_ops,
                    input_cost.estimated_io_bytes + io_bytes,
                )
            }

            PhysicalPlan::Projection { input, .. } => {
                // Projection is cheap; just trim columns
                let mut cost = self.estimate_cost(input);
                // Slightly reduce IO since we return fewer bytes
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
                // Note: FHE ops from input still happen because we do not know
                // which rows will survive until after FHE evaluation.
                PlanCost::compute(rows, input_cost.estimated_fhe_ops, io_bytes)
            }

            PhysicalPlan::PointGet { .. } => PlanCost::compute(1, 0, self.stats.average_value_size),
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

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

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
                // Combine two half-ranges
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
        // Default to U8 type for now; in a full implementation the type
        // would be inferred from schema metadata.
        compiler.compile(predicate, EncryptedType::U8)
    }
}

impl Default for QueryPlanner {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Display implementations for explain/debugging
// ---------------------------------------------------------------------------

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
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::col;

    fn make_blob(v: u8) -> CipherBlob {
        CipherBlob::new(vec![v])
    }

    // -- Basic planning tests -----------------------------------------------

    #[test]
    fn test_scan_plan() -> Result<()> {
        let planner = QueryPlanner::new();
        let query = Query::Filter {
            collection: "users".to_string(),
            predicate: Predicate::Gt(col("age"), make_blob(18)),
        };

        let plan = planner.plan(&query)?;

        // Should produce FheFilter over SeqScan because "age" is not "_key"
        match &plan {
            PhysicalPlan::FheFilter { input, .. } => {
                assert!(matches!(input.as_ref(), PhysicalPlan::SeqScan { .. }));
            }
            other => {
                return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Expected FheFilter, got: {:?}",
                    other
                ))));
            }
        }
        Ok(())
    }

    #[test]
    fn test_range_scan_pushdown() -> Result<()> {
        let planner = QueryPlanner::new();

        // Filter on _key column should convert to IndexScan
        let query = Query::Filter {
            collection: "data".to_string(),
            predicate: Predicate::And(
                Box::new(Predicate::Gte(col("_key"), make_blob(10))),
                Box::new(Predicate::Lt(col("_key"), make_blob(50))),
            ),
        };

        let plan = planner.plan(&query)?;

        match &plan {
            PhysicalPlan::IndexScan {
                collection,
                start,
                end,
            } => {
                assert_eq!(collection, "data");
                assert!(start.is_some());
                assert!(end.is_some());
            }
            other => {
                return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Expected IndexScan, got: {:?}",
                    other
                ))));
            }
        }
        Ok(())
    }

    #[test]
    fn test_predicate_pushdown() -> Result<()> {
        let planner = QueryPlanner::new();

        // Construct: Filter(Project(Scan, [age]), pred_on_age)
        // The filter should be pushed below the projection.
        let scan = LogicalPlan::Scan {
            collection: "users".to_string(),
        };
        let project = LogicalPlan::Project {
            input: Box::new(scan),
            columns: vec!["age".to_string(), "name".to_string()],
        };
        let filter = LogicalPlan::Filter {
            input: Box::new(project),
            predicate: Predicate::Gt(col("age"), make_blob(18)),
        };

        let optimized = planner.push_predicates_down(filter);

        // After pushdown: Project([age, name], Filter(Scan, pred))
        match &optimized {
            LogicalPlan::Project { input, columns } => {
                assert!(columns.contains(&"age".to_string()));
                assert!(matches!(input.as_ref(), LogicalPlan::Filter { .. }));
            }
            other => {
                return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Expected Project, got: {:?}",
                    other
                ))));
            }
        }
        Ok(())
    }

    #[test]
    fn test_filter_merge() -> Result<()> {
        let planner = QueryPlanner::new();

        // Construct: Filter(Filter(Scan, p1), p2) -> Filter(Scan, And(p1, p2))
        let scan = LogicalPlan::Scan {
            collection: "users".to_string(),
        };
        let filter1 = LogicalPlan::Filter {
            input: Box::new(scan),
            predicate: Predicate::Gt(col("age"), make_blob(18)),
        };
        let filter2 = LogicalPlan::Filter {
            input: Box::new(filter1),
            predicate: Predicate::Lt(col("age"), make_blob(65)),
        };

        let optimized = planner.merge_filters(filter2);

        match &optimized {
            LogicalPlan::Filter { input, predicate } => {
                // Should be a single filter with AND predicate
                assert!(matches!(predicate, Predicate::And(_, _)));
                // Input should be Scan, not another Filter
                assert!(matches!(input.as_ref(), LogicalPlan::Scan { .. }));
            }
            other => {
                return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Expected Filter, got: {:?}",
                    other
                ))));
            }
        }
        Ok(())
    }

    #[test]
    fn test_cost_estimation() -> Result<()> {
        let planner = QueryPlanner::new();
        planner.stats().set_collection_size("data", 10_000);

        // Full scan cost
        let seq_scan = PhysicalPlan::SeqScan {
            collection: "data".to_string(),
        };
        let seq_cost = planner.estimate_cost(&seq_scan);

        // Index scan cost (should be cheaper)
        let idx_scan = PhysicalPlan::IndexScan {
            collection: "data".to_string(),
            start: Some(vec![10]),
            end: Some(vec![50]),
        };
        let idx_cost = planner.estimate_cost(&idx_scan);

        // Index scan should be cheaper than full scan
        assert!(
            idx_cost.total_cost < seq_cost.total_cost,
            "IndexScan cost ({}) should be less than SeqScan cost ({})",
            idx_cost.total_cost,
            seq_cost.total_cost,
        );

        // Point get should be the cheapest
        let point = PhysicalPlan::PointGet {
            collection: "data".to_string(),
            key: Key::from_str("k"),
        };
        let point_cost = planner.estimate_cost(&point);
        assert!(
            point_cost.total_cost < idx_cost.total_cost,
            "PointGet cost ({}) should be less than IndexScan cost ({})",
            point_cost.total_cost,
            idx_cost.total_cost,
        );

        Ok(())
    }

    #[test]
    fn test_limit_planning() -> Result<()> {
        let planner = QueryPlanner::new();

        // Build a filter query and wrap with Limit via logical plan
        let scan = LogicalPlan::Scan {
            collection: "logs".to_string(),
        };
        let filter = LogicalPlan::Filter {
            input: Box::new(scan),
            predicate: Predicate::Eq(col("level"), make_blob(1)),
        };
        let limited = LogicalPlan::Limit {
            input: Box::new(filter),
            count: 10,
        };

        let physical = planner.to_physical(&limited)?;

        // Limit should be on top
        match &physical {
            PhysicalPlan::Limit { input, count } => {
                assert_eq!(*count, 10);
                assert!(matches!(input.as_ref(), PhysicalPlan::FheFilter { .. }));
            }
            other => {
                return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Expected Limit, got: {:?}",
                    other
                ))));
            }
        }

        Ok(())
    }

    #[test]
    fn test_plan_with_fhe_filter() -> Result<()> {
        let planner = QueryPlanner::new();
        let query = Query::Filter {
            collection: "accounts".to_string(),
            predicate: Predicate::And(
                Box::new(Predicate::Gt(col("balance"), make_blob(100))),
                Box::new(Predicate::Lt(col("balance"), make_blob(200))),
            ),
        };

        let plan = planner.plan(&query)?;

        // Should have an FheFilter with a compiled circuit
        match &plan {
            PhysicalPlan::FheFilter { circuit, .. } => {
                // The circuit should have gate_count > 0 for AND of two comparisons
                assert!(circuit.gate_count > 0);
                assert_eq!(circuit.result_type, EncryptedType::Bool);
            }
            other => {
                return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Expected FheFilter, got: {:?}",
                    other
                ))));
            }
        }
        Ok(())
    }

    #[test]
    fn test_complex_plan() -> Result<()> {
        let planner = QueryPlanner::new();
        planner.stats().set_collection_size("orders", 50_000);

        // Complex query: Filter with non-key predicate -> should remain FheFilter
        let query = Query::Filter {
            collection: "orders".to_string(),
            predicate: Predicate::Or(
                Box::new(Predicate::Eq(col("status"), make_blob(1))),
                Box::new(Predicate::And(
                    Box::new(Predicate::Gt(col("amount"), make_blob(100))),
                    Box::new(Predicate::Lt(col("amount"), make_blob(255))),
                )),
            ),
        };

        let plan = planner.plan(&query)?;
        let cost = planner.estimate_cost(&plan);

        // Should have a non-trivial cost due to FHE ops
        assert!(cost.estimated_fhe_ops > 0);
        assert!(cost.total_cost > 0.0);

        // Verify display works
        let plan_str = format!("{}", plan);
        assert!(!plan_str.is_empty());

        Ok(())
    }

    #[test]
    fn test_get_query_planning() -> Result<()> {
        let planner = QueryPlanner::new();
        let query = Query::Get {
            collection: "users".to_string(),
            key: Key::from_str("user:42"),
        };

        let plan = planner.plan(&query)?;

        match &plan {
            PhysicalPlan::PointGet { collection, key } => {
                assert_eq!(collection, "users");
                assert_eq!(key.to_string_lossy(), "user:42");
            }
            other => {
                return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Expected PointGet, got: {:?}",
                    other
                ))));
            }
        }

        let cost = planner.estimate_cost(&plan);
        assert_eq!(cost.estimated_rows, 1);
        assert_eq!(cost.estimated_fhe_ops, 0);

        Ok(())
    }

    #[test]
    fn test_range_query_planning() -> Result<()> {
        let planner = QueryPlanner::new();
        let query = Query::Range {
            collection: "events".to_string(),
            start: Key::from_str("2024-01"),
            end: Key::from_str("2024-12"),
        };

        let plan = planner.plan(&query)?;

        match &plan {
            PhysicalPlan::IndexScan {
                collection,
                start,
                end,
            } => {
                assert_eq!(collection, "events");
                assert!(start.is_some());
                assert!(end.is_some());
            }
            other => {
                return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Expected IndexScan, got: {:?}",
                    other
                ))));
            }
        }
        Ok(())
    }

    #[test]
    fn test_cost_comparison() -> Result<()> {
        let planner = QueryPlanner::new();
        planner.stats().set_collection_size("items", 100_000);

        let scan = PhysicalPlan::SeqScan {
            collection: "items".to_string(),
        };

        let idx = PhysicalPlan::IndexScan {
            collection: "items".to_string(),
            start: Some(vec![1]),
            end: Some(vec![10]),
        };

        let cheaper = planner.choose_cheaper(&scan, &idx);

        // IndexScan should win
        assert!(matches!(cheaper, PhysicalPlan::IndexScan { .. }));

        Ok(())
    }

    #[test]
    fn test_filter_not_pushed_below_limit() -> Result<()> {
        let planner = QueryPlanner::new();

        // Filter(Limit(Scan, 10), pred) -> Filter should stay on top
        let scan = LogicalPlan::Scan {
            collection: "data".to_string(),
        };
        let limited = LogicalPlan::Limit {
            input: Box::new(scan),
            count: 10,
        };
        let filter = LogicalPlan::Filter {
            input: Box::new(limited),
            predicate: Predicate::Gt(col("x"), make_blob(5)),
        };

        let optimized = planner.push_predicates_down(filter);

        // Filter should remain on top of Limit
        match &optimized {
            LogicalPlan::Filter { input, .. } => {
                assert!(matches!(input.as_ref(), LogicalPlan::Limit { .. }));
            }
            other => {
                return Err(AmateRSError::FheComputation(ErrorContext::new(format!(
                    "Expected Filter on top, got: {:?}",
                    other
                ))));
            }
        }

        Ok(())
    }

    #[test]
    fn test_stats_update() {
        let planner = QueryPlanner::new();
        planner.stats().set_collection_size("big_table", 1_000_000);

        let size = planner.stats().collection_size("big_table");
        assert_eq!(size, 1_000_000);

        // Unknown collection should default to 1000
        let default_size = planner.stats().collection_size("unknown");
        assert_eq!(default_size, 1000);
    }

    #[test]
    fn test_referenced_columns() {
        let pred = Predicate::And(
            Box::new(Predicate::Gt(col("age"), make_blob(18))),
            Box::new(Predicate::Or(
                Box::new(Predicate::Lt(col("salary"), make_blob(100))),
                Box::new(Predicate::Eq(col("age"), make_blob(30))),
            )),
        );

        let cols = QueryPlanner::referenced_columns(&pred);
        assert_eq!(cols, vec!["age".to_string(), "salary".to_string()]);
    }

    #[test]
    fn test_display_plan_cost() {
        let cost = PlanCost::compute(1000, 50, 256_000);
        let display = format!("{}", cost);
        assert!(display.contains("1000"));
        assert!(display.contains("50"));
    }

    #[test]
    fn test_logical_plan_display() {
        let plan = LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Scan {
                collection: "t".to_string(),
            }),
            predicate: Predicate::Eq(col("x"), make_blob(1)),
        };

        let s = format!("{}", plan);
        assert!(s.contains("Filter"));
        assert!(s.contains("Scan"));
    }
}
