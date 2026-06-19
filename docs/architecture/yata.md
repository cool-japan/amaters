# Yata - The Eight-Span Mirror: FHE Compute Subsystem

## 1. Role

Yata is the FHE (Fully Homomorphic Encryption) compute subsystem of AmateRS. The name comes from the project blueprint: "Yata = compute". The module comment in `crates/amaters-core/src/compute/mod.rs` reads:

> "Compute engine module (Yata - The Eight-Span Mirror)"

Yata has three responsibilities:

1. Execute circuit operations over TFHE ciphertexts (`FheExecutor`)
2. Plan and optimize queries against encrypted data (`QueryPlanner`, `PhysicalPlan`)
3. Manage FHE keys per client (`KeyManager`, `FheKeyPair`)

The `compute` feature flag gates all actual FHE execution. Without it the server runs in plaintext mode and `FheExecutor::execute` returns `Err(AmateRSError::FeatureNotEnabled(...))`.

---

## 2. Key Structures

### FheExecutor

Source: `crates/amaters-core/src/compute/mod.rs`

```
pub struct FheExecutor {
    optimizer: CircuitOptimizer,
    optimization_enabled: bool,
}
```

| Method | Description |
|---|---|
| `new() -> Self` | Creates executor with optimization enabled |
| `with_optimization(enable: bool) -> Self` | Creates executor with explicit optimization flag |
| `execute(&self, circuit: &Circuit, inputs: &HashMap<String, CipherBlob>) -> Result<CipherBlob>` | Entry point; compiled only with `#[cfg(feature = "compute")]`; stub variant returns error |
| `execute_node(...)` | Recursive evaluator dispatching over `CircuitNode` variants |

---

### Circuit Types

Source: `crates/amaters-core/src/compute/circuit.rs`

**Builders and containers**

- `CircuitBuilder` — constructs a `Circuit` by adding nodes
- `Circuit` — immutable DAG of `CircuitNode`s with a declared `variable_types` map
- `CircuitValue` — a node value at compile time

**Operator enums**

| Enum | Variants |
|---|---|
| `BinaryOperator` | `Add`, `Sub`, `Mul`, `And`, `Or`, `Xor` |
| `UnaryOperator` | `Not`, `Neg` |
| `CompareOperator` | `Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge` |

**Type enums**

- `EncryptedType`: `Bool`, `U8`, `U16`, `U32`, `U64`
- `ConstantType`: plain constant types (Float and Bytes variants are unsupported in FHE evaluation)

**Node variants**

`CircuitNode` covers: `Load`, `Constant`, `EncryptedConstant`, `BinaryOp`, `UnaryOp`, `Compare`, `NaryOp`

**Runtime encrypted value**

`EncryptedValue` (only compiled with `#[cfg(feature = "compute")]`):

```
enum EncryptedValue {
    Bool(EncryptedBool),
    U8(EncryptedU8),
    U16(EncryptedU16),
    U32(EncryptedU32),
    U64(EncryptedU64),
}
```

**Deprecated**

`Gate` enum (`Add`, `Mul`, `Not`, `Bootstrap`) was deprecated in 0.1.0. Use `CircuitNode` instead.

---

### CircuitOptimizer

Source: `crates/amaters-core/src/compute/optimizer.rs`

```
pub struct CircuitOptimizer { /* ... */ }
pub struct DependencyGraph {
    pub dependencies: HashMap<NodeId, Vec<NodeId>>,
    pub parallel_groups: Vec<Vec<NodeId>>,
    pub critical_path: Vec<NodeId>,
}
pub struct NodeId(pub usize);
pub struct OptimizationStats { /* ... */ }
```

`CircuitOptimizer::disabled()` returns an inert optimizer that skips all passes. `DependencyGraph` exposes a `topological_order()` method for ordered evaluation.

---

### QueryPlanner

Source: `crates/amaters-core/src/compute/planner/mod.rs`

```
pub struct QueryPlanner {
    stats: Arc<PlannerStats>,
    cache: Option<Arc<PlanCache>>,
}
```

`plan(&self, query: &Query) -> Result<PhysicalPlan>` runs this pipeline:

```
check PlanCache
     |
     v (miss)
to_logical(query)  ->  LogicalPlan
     |
     v
optimize_logical:
  1. push_predicates_down
  2. merge_filters
  3. convert_filter_to_range_scan
  4. reorder_predicates_by_cost
     |
     v
to_physical  ->  PhysicalPlan
```

**LogicalPlan variants**: `Scan`, `RangeScan`, `Filter`, `Project`, `Limit`, `PointLookup`, `Join`

**PhysicalPlan variants**: `SeqScan`, `IndexScan`, `FheFilter`, `Projection`, `Limit`, `PointGet`, `NestedLoopJoin`, `HashJoin`

`FheFilter` carries a compiled `Circuit` and retains the original `Predicate` for introspection.

---

### PlanCost

Source: `crates/amaters-core/src/compute/planner/mod.rs`

```
pub struct PlanCost {
    pub estimated_rows: u64,
    pub estimated_fhe_ops: u64,
    pub estimated_io_bytes: u64,
    pub total_cost: f64,
}
```

Cost formula:

```
total_cost = (estimated_rows      * 0.01 )   // SCAN_COST_PER_ROW
           + (estimated_fhe_ops   * 100.0)   // FHE_COST_PER_OP
           + (estimated_io_bytes  * 0.001)   // IO_COST_PER_BYTE
```

Point lookup baseline: `POINT_LOOKUP_COST = 1.0`. The high weight of `FHE_COST_PER_OP` deliberately pushes the optimizer to minimize homomorphic operations.

---

### PlannerStats

Source: `crates/amaters-core/src/compute/planner/mod.rs`

Tracks `fhe_op_latency_us`, `fhe_comparison_cost`, `fhe_boolean_cost`, and predicate selectivity. Uses `DashMap` for concurrent read/write access during query execution.

---

### PredicateCompiler

Source: `crates/amaters-core/src/compute/predicate.rs`

```
pub fn compile_predicate(predicate: &Predicate) -> Result<Circuit>
```

Called by `QueryPlanner::to_physical` when a logical `Filter` node is lowered to a `PhysicalPlan::FheFilter`. The resulting `Circuit` may then be cached by `CircuitCache`.

---

### Key Management

Sources: `crates/amaters-core/src/compute/keys.rs`, `crates/amaters-core/src/compute/key_manager.rs`

- `FheKeyPair` — holds a paired client key and server key
  - `fn generate() -> Result<Self>`
  - `fn set_as_global_server_key(&self)`
- `InMemoryKeyStorage` — implements the `KeyStorage` trait
- `KeyManager` — manages per-client `FheKeyPair`s indexed by `ClientId`

---

### Server Integration

Source: `crates/amaters-net/src/server.rs`

```
pub struct AqlServiceImpl<S: StorageEngine> {
    #[cfg(feature = "compute")]
    key_manager: Arc<KeyManager>,
    circuit_cache: CircuitCache,
    // ...
}
```

Constructors:
- `new(storage: Arc<S>)` — creates default `KeyManager` and `CircuitCache`
- `with_key_manager(storage: Arc<S>, key_manager: Arc<KeyManager>)` — inject a pre-built key manager

`CircuitCache` (from `crate::circuit_cache` in `amaters-net`) uses `get_or_compile(&predicate, || {...})` for lazy, memoized circuit compilation.

---

## 3. Data Flow

### Query Execution Path

```
Client
  |
  | AQL query
  v
AqlServiceImpl
  |
  | QueryPlanner::plan(query)
  v
 PlanCache? ---hit--> PhysicalPlan
  |
  | miss
  v
to_logical(query)
  |
  v
optimize_logical (4 passes)
  |
  v
to_physical
  | (Filter nodes)
  v
PredicateCompiler::compile_predicate
  |  CircuitCache::get_or_compile
  v
PhysicalPlan (contains FheFilter with Circuit)
  |
  v
FheExecutor::execute(circuit, inputs)
  |
  | optimization_enabled?
  |--yes--> CircuitOptimizer::optimize(circuit)
  |
  v
execute_node (recursive over CircuitNode DAG)
  |
  v
CipherBlob  -->  Client
```

### Key Load Path

On key registration: `AqlServiceImpl` calls `KeyManager::store(client_id, FheKeyPair)`. At query time it retrieves the stored pair via `KeyManager::get(client_id)` and calls `FheKeyPair::set_as_global_server_key()` before invoking `FheExecutor`.

---

## 4. Invariants

1. **Feature gate**: The `compute` feature is required for actual FHE execution. Without it, `FheExecutor::execute` always returns `Err(AmateRSError::FeatureNotEnabled(...))`. The server compiles and runs in plaintext mode.

2. **Trivial encryption of constants**: `CircuitNode::Constant` values are encrypted via `try_encrypt_trivial`. They carry no confidentiality guarantees; any party with the public parameters can recover the plaintext.

3. **Unsupported constant types**: `EncryptedConstant` nodes with `original_type: ConstantType::Float` or `ConstantType::Bytes` cannot be evaluated inside an FHE circuit. The executor returns `Err` for these variants.

4. **Complete inputs required**: Every column declared in `Circuit::variable_types` must be present in the `inputs` map passed to `FheExecutor::execute`. A missing entry returns `Err(AmateRSError::FheComputation(...))`.

5. **Deprecated Gate enum**: `Gate` (`Add`, `Mul`, `Not`, `Bootstrap`) is deprecated since 0.1.0 and must not appear in new circuit construction. Use `CircuitNode` variants instead.

---

## 5. Extension Points

**Adding encrypted types**: add a variant to `EncryptedType` (`circuit.rs`) and `EncryptedValue` (`mod.rs`, behind `#[cfg(feature = "compute")]`), implement the `EncryptedU*` wrapper in `operations.rs`, then add dispatch arms in `FheExecutor::execute_node`.

**Adding circuit operators**: extend `BinaryOperator`, `UnaryOperator`, or `CompareOperator` in `circuit.rs`; handle the variant in `CircuitBuilder` and in `FheExecutor::execute_node`.

**Optimization passes**: disable via `FheExecutor::with_optimization(false)` or `CircuitOptimizer::disabled()`; add passes in `optimizer.rs`. `DependencyGraph::parallel_groups` supports parallel-execution analysis.

**Plan/circuit cache tuning**: `PlanCache` and `PlanCacheConfig` live in `crates/amaters-core/src/compute/plan_cache.rs`; `CircuitCacheConfig` is in `crates/amaters-net/src/circuit_cache.rs`.

**GPU acceleration**: `pub mod gpu` is declared in `crates/amaters-core/src/compute/mod.rs`. Verify implementation status in `gpu.rs` before relying on GPU paths.
