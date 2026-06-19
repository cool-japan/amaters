//! Auto-generated test module (consolidated from inline `#[cfg(test)] mod` blocks)

use crate::compute::EncryptedType;
use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::{CipherBlob, JoinType, Key, Predicate, Query};

use super::*;

#[cfg(test)]
mod tests_2 {
    use super::*;
    use crate::types::col;
    fn make_blob(v: u8) -> CipherBlob {
        CipherBlob::new(vec![v])
    }
    #[test]
    fn test_scan_plan() -> Result<()> {
        let planner = QueryPlanner::new();
        let query = Query::Filter {
            collection: "users".to_string(),
            predicate: Predicate::Gt(col("age"), make_blob(18)),
        };
        let plan = planner.plan(&query)?;
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
                assert!(matches!(predicate, Predicate::And(_, _)));
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
        let seq_scan = PhysicalPlan::SeqScan {
            collection: "data".to_string(),
        };
        let seq_cost = planner.estimate_cost(&seq_scan);
        let idx_scan = PhysicalPlan::IndexScan {
            collection: "data".to_string(),
            start: Some(vec![10]),
            end: Some(vec![50]),
        };
        let idx_cost = planner.estimate_cost(&idx_scan);
        assert!(
            idx_cost.total_cost < seq_cost.total_cost,
            "IndexScan cost ({}) should be less than SeqScan cost ({})",
            idx_cost.total_cost,
            seq_cost.total_cost,
        );
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
        match &plan {
            PhysicalPlan::FheFilter { circuit, .. } => {
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
        assert!(cost.estimated_fhe_ops > 0);
        assert!(cost.total_cost > 0.0);
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
        assert!(matches!(cheaper, PhysicalPlan::IndexScan { .. }));
        Ok(())
    }
    #[test]
    fn test_filter_not_pushed_below_limit() -> Result<()> {
        let planner = QueryPlanner::new();
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
    #[test]
    fn test_conjunction_split_and_pushdown() {
        let planner = QueryPlanner::new();
        let plan = LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Scan {
                collection: "tbl".to_string(),
            }),
            predicate: Predicate::And(
                Box::new(Predicate::Gt(col("a"), make_blob(10))),
                Box::new(Predicate::Lt(col("b"), make_blob(20))),
            ),
        };
        let optimized = planner.push_predicates_down(plan);
        fn count_filters(p: &LogicalPlan) -> usize {
            match p {
                LogicalPlan::Filter { input, .. } => 1 + count_filters(input),
                LogicalPlan::Scan { .. } | LogicalPlan::RangeScan { .. } => 0,
                LogicalPlan::Project { input, .. } | LogicalPlan::Limit { input, .. } => {
                    count_filters(input)
                }
                LogicalPlan::PointLookup { .. } => 0,
                LogicalPlan::Join { left, right, .. } => count_filters(left) + count_filters(right),
            }
        }
        assert_eq!(
            count_filters(&optimized),
            2,
            "And should be split into 2 Filter nodes, got: {:?}",
            optimized
        );
    }
    #[test]
    fn test_predicate_reorder_cheap_eq_first() {
        let planner = QueryPlanner::new();
        let pred = Predicate::And(
            Box::new(Predicate::Gt(col("age"), make_blob(18))),
            Box::new(Predicate::Eq(col("id"), make_blob(42))),
        );
        let reordered = planner.reorder_pred(&pred);
        match reordered {
            Predicate::And(p1, _p2) => {
                assert!(
                    matches!(*p1, Predicate::Eq(_, _)),
                    "Expected Eq to be moved to first (cheaper) branch, got: {:?}",
                    *p1
                );
            }
            other => panic!("Expected And after reorder, got: {:?}", other),
        }
    }
    #[test]
    fn test_join_hash_for_eq_predicate() -> Result<()> {
        let planner = QueryPlanner::new();
        let query = Query::Join {
            left_collection: "users".to_string(),
            right_collection: "orders".to_string(),
            on: Predicate::Eq(col("user_id"), make_blob(1)),
            join_type: JoinType::Inner,
            left_limit: None,
            right_limit: None,
        };
        let plan = planner.plan(&query)?;
        assert!(
            matches!(plan, PhysicalPlan::HashJoin { .. }),
            "Eq join condition should produce HashJoin, got: {:?}",
            plan
        );
        Ok(())
    }
    #[test]
    fn test_join_nested_loop_for_gt_predicate() -> Result<()> {
        let planner = QueryPlanner::new();
        let query = Query::Join {
            left_collection: "a".to_string(),
            right_collection: "b".to_string(),
            on: Predicate::Gt(col("score"), make_blob(50)),
            join_type: JoinType::Left,
            left_limit: None,
            right_limit: None,
        };
        let plan = planner.plan(&query)?;
        assert!(
            matches!(plan, PhysicalPlan::NestedLoopJoin { .. }),
            "Gt join condition should produce NestedLoopJoin, got: {:?}",
            plan
        );
        Ok(())
    }
    #[test]
    fn test_join_smaller_side_is_build() -> Result<()> {
        let planner = QueryPlanner::new();
        planner.stats().set_collection_size("small_table", 100);
        planner.stats().set_collection_size("large_table", 10_000);
        let query = Query::Join {
            left_collection: "small_table".to_string(),
            right_collection: "large_table".to_string(),
            on: Predicate::Eq(col("id"), make_blob(1)),
            join_type: JoinType::Inner,
            left_limit: None,
            right_limit: None,
        };
        let plan = planner.plan(&query)?;
        match &plan {
            PhysicalPlan::HashJoin { build, probe, .. } => {
                let build_cost = planner.estimate_cost(build);
                let probe_cost = planner.estimate_cost(probe);
                assert!(
                    build_cost.estimated_rows <= probe_cost.estimated_rows,
                    "Build side ({} rows) should be <= probe side ({} rows)",
                    build_cost.estimated_rows,
                    probe_cost.estimated_rows
                );
            }
            other => panic!("Expected HashJoin, got: {:?}", other),
        }
        Ok(())
    }
    #[test]
    fn test_join_explain_shows_join_type() -> Result<()> {
        let planner = QueryPlanner::new();
        let query_inner = Query::Join {
            left_collection: "a".to_string(),
            right_collection: "b".to_string(),
            on: Predicate::Eq(col("id"), make_blob(1)),
            join_type: JoinType::Inner,
            left_limit: None,
            right_limit: None,
        };
        let plan_inner = planner.plan(&query_inner)?;
        let display_inner = format!("{}", plan_inner);
        assert!(
            display_inner.contains("Inner"),
            "Display should mention 'Inner', got: {}",
            display_inner
        );
        let query_left = Query::Join {
            left_collection: "a".to_string(),
            right_collection: "b".to_string(),
            on: Predicate::Gt(col("val"), make_blob(5)),
            join_type: JoinType::Left,
            left_limit: None,
            right_limit: None,
        };
        let plan_left = planner.plan(&query_left)?;
        let display_left = format!("{}", plan_left);
        assert!(
            display_left.contains("Left"),
            "Display should mention 'Left', got: {}",
            display_left
        );
        Ok(())
    }
    #[test]
    fn test_hash_join_cost_lower_than_nested_loop() {
        let planner = QueryPlanner::new();
        planner.stats().set_collection_size("x", 500);
        planner.stats().set_collection_size("y", 500);
        let scan_x = PhysicalPlan::SeqScan {
            collection: "x".to_string(),
        };
        let scan_y = PhysicalPlan::SeqScan {
            collection: "y".to_string(),
        };
        let nlj = PhysicalPlan::NestedLoopJoin {
            outer: Box::new(scan_x.clone()),
            build: Box::new(scan_y.clone()),
            on: Predicate::Gt(col("v"), make_blob(1)),
            join_type: JoinType::Inner,
        };
        let hj = PhysicalPlan::HashJoin {
            probe: Box::new(scan_x),
            build: Box::new(scan_y),
            on: Predicate::Eq(col("id"), make_blob(1)),
            join_type: JoinType::Inner,
        };
        let nlj_cost = planner.estimate_cost(&nlj);
        let hj_cost = planner.estimate_cost(&hj);
        assert!(
            hj_cost.total_cost < nlj_cost.total_cost,
            "HashJoin cost ({:.2}) should be less than NestedLoopJoin cost ({:.2})",
            hj_cost.total_cost,
            nlj_cost.total_cost
        );
    }
}
