//! Query builder and query planner example.
//!
//! Demonstrates:
//! - Building queries with the fluent API (from amaters-sdk-rust)
//! - Using the QueryPlanner from amaters-core
//! - Plan caching
//! - Inspecting physical plans and cost estimates

use amaters::core::compute::planner::{PlanCacheConfig, QueryPlanner};
use amaters::core::{CipherBlob, Key, Predicate, Query, QueryBuilder, col};
use amaters::sdk::query;
use std::time::Duration;

fn main() -> amaters::core::Result<()> {
    println!("=== AmateRS Query Builder Example ===\n");

    // =========================================================================
    // 1. Build queries with the core QueryBuilder
    // =========================================================================
    println!("--- Core QueryBuilder ---");

    // Point lookup
    let get_query = QueryBuilder::new("users").get(Key::from_str("user:123"));
    println!("  Get query: {:?}", get_query);

    // Set query
    let set_query = QueryBuilder::new("users")
        .set(Key::from_str("user:456"), CipherBlob::new(vec![1, 2, 3, 4]));
    println!("  Set query: {:?}", set_query);

    // Filter query
    let filter_query =
        QueryBuilder::new("users").filter(Predicate::Eq(col("status"), CipherBlob::new(vec![1])));
    println!("  Filter query: {:?}", filter_query);

    // Range query
    let range_query =
        QueryBuilder::new("events").range(Key::from_str("2024-01-01"), Key::from_str("2024-12-31"));
    println!("  Range query: {:?}", range_query);
    println!();

    // =========================================================================
    // 2. Build queries with the SDK fluent API
    // =========================================================================
    println!("--- SDK Fluent Query Builder ---");

    // Simple get
    let q1 = query("users").get(Key::from_str("user:123"));
    println!("  Fluent get: {:?}", q1);

    // Fluent filter with where clause
    let q2 = query("users")
        .where_clause()
        .eq(col("status"), CipherBlob::new(vec![1]))
        .and(Predicate::Gt(col("age"), CipherBlob::new(vec![18])))
        .build();
    println!("  Fluent filter: {:?}", q2);

    // Range query via fluent API
    let q3 = query("logs").range(Key::from_str("2024-01-01"), Key::from_str("2024-06-30"));
    println!("  Fluent range: {:?}", q3);

    // Delete query
    let q4 = query("sessions").delete(Key::from_str("session:expired"));
    println!("  Fluent delete: {:?}", q4);
    println!();

    // =========================================================================
    // 3. Use the QueryPlanner
    // =========================================================================
    println!("--- QueryPlanner ---");

    let planner = QueryPlanner::new();

    // Plan a point lookup
    let get_plan = planner.plan(&get_query)?;
    println!("  Get plan: {:?}", get_plan);

    let get_cost = planner.estimate_cost(&get_plan);
    println!("  Get cost: {}", get_cost);

    // Plan a filter query
    let filter_plan = planner.plan(&filter_query)?;
    println!("  Filter plan: {:?}", filter_plan);

    let filter_cost = planner.estimate_cost(&filter_plan);
    println!("  Filter cost: {}", filter_cost);

    // Plan a range query
    let range_plan = planner.plan(&range_query)?;
    println!("  Range plan: {:?}", range_plan);

    let range_cost = planner.estimate_cost(&range_plan);
    println!("  Range cost: {}", range_cost);
    println!();

    // =========================================================================
    // 4. Plan caching
    // =========================================================================
    println!("--- Plan Caching ---");

    let cached_planner = QueryPlanner::new().with_cache(PlanCacheConfig {
        max_entries: 100,
        ttl: Duration::from_secs(60),
    });

    // First call: cache miss
    let _plan1 = cached_planner.plan(&get_query)?;
    let stats1 = cached_planner.cache_stats();
    println!(
        "  After 1st plan: hits={}, misses={}, size={}",
        stats1.hits, stats1.misses, stats1.size
    );

    // Second call with same query: cache hit
    let _plan2 = cached_planner.plan(&get_query)?;
    let stats2 = cached_planner.cache_stats();
    println!(
        "  After 2nd plan: hits={}, misses={}, size={}",
        stats2.hits, stats2.misses, stats2.size
    );

    // Different query: cache miss
    let _plan3 = cached_planner.plan(&filter_query)?;
    let stats3 = cached_planner.cache_stats();
    println!(
        "  After 3rd plan: hits={}, misses={}, size={}",
        stats3.hits, stats3.misses, stats3.size
    );

    // Invalidate all
    cached_planner.invalidate_all();
    let stats4 = cached_planner.cache_stats();
    println!(
        "  After invalidation: hits={}, misses={}, evictions={}, size={}",
        stats4.hits, stats4.misses, stats4.evictions, stats4.size
    );
    println!();

    // =========================================================================
    // 5. Cost comparison
    // =========================================================================
    println!("--- Cost Comparison ---");

    // Set collection size estimates for more accurate cost modeling
    let planner_with_stats = QueryPlanner::new();
    planner_with_stats
        .stats()
        .set_collection_size("users", 10_000);
    planner_with_stats
        .stats()
        .set_collection_size("events", 1_000_000);

    let queries: Vec<(&str, Query)> = vec![
        ("Point lookup", get_query),
        (
            "Filter scan",
            QueryBuilder::new("users")
                .filter(Predicate::Eq(col("status"), CipherBlob::new(vec![1]))),
        ),
        (
            "Range scan",
            QueryBuilder::new("events")
                .range(Key::from_str("2024-01-01"), Key::from_str("2024-06-30")),
        ),
    ];

    for (label, q) in &queries {
        let plan = planner_with_stats.plan(q)?;
        let cost = planner_with_stats.estimate_cost(&plan);
        println!(
            "  {:<20} total_cost={:.2}  rows={}  fhe_ops={}  io_bytes={}",
            label,
            cost.total_cost,
            cost.estimated_rows,
            cost.estimated_fhe_ops,
            cost.estimated_io_bytes
        );
    }

    println!("\nExample finished.");
    Ok(())
}
