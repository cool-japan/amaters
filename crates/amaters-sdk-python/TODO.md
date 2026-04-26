# amaters-sdk-python TODO

## v0.2.0 (2026-04-26)

### Completed
- [x] PyO3 bindings for core AmateRS client (`AmateRSClient`)
- [x] Configuration types (`ClientConfig`, `RetryConfig`) with public visibility
- [x] Streaming types (`StreamIterator`, `BatchStreamIterator`) with public visibility
- [x] maturin build integration (ABI3 wheel, Python 3.8+)
- [x] Async client methods via `pyo3-async-runtimes` / Tokio backend
- [x] CRUD operations: `set`, `get`, `delete`, `contains`
- [x] Batch operations: `batch`, `batch_set`, `batch_get`, `batch_delete`
- [x] Range queries: `range_query`, `count`, `keys`
- [x] Cursor-based pagination: `scan`
- [x] Streaming iterators: `range_stream`, `batch_stream`
- [x] Connection pool statistics: `pool_stats`
- [x] Context manager protocol (`__enter__` / `__exit__`)
- [x] `Key`, `BatchResult`, `ScanResult` Python wrapper types
- [x] Error mapping from `SdkError` to Python exceptions (`ConnectionError`, `TimeoutError`, `ValueError`, `RuntimeError`)
- [x] `serialization` feature flag (Oxicode)
- [x] `fhe` feature flag (tfhe integration)

### Planned
- [ ] Async Python client (native `asyncio` without blocking Tokio runtime per call)
- [ ] Type stubs (`.pyi` files) for IDE completion and mypy support
- [ ] Documentation examples in `/python/` module
- [ ] Prefix query method (currently implemented via `scan`; dedicated `prefix_query` pending)
- [ ] Subscribe / watch API for change notifications
- [ ] Python package `__all__` exports and top-level `__init__.py` polish
- [ ] Integration tests against a live AmateRS server
