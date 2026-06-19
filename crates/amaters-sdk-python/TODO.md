# amaters-sdk-python TODO

## v0.2.3 (in progress)

### Completed
- [x] Python test suite: 116 pytest tests (5 files: test_config.py, test_types.py, test_client_operations.py, test_error_handling.py, test_properties.py); all mock-backed, no compiled extension required

### Planned
- [ ] PyO3 export wiring — register `#[pyclass]`/`#[pyfunction]` exports in lib.rs for subscribe, streaming, and types modules

## v0.2.2 (2026-06-19)

### Completed
- [x] Fully async `pool_stats` and `close` methods (`PyPoolStats` return type)
- [x] `PyPoolStats` Python type exposed via PyO3
- [x] Python test suite: 80 tests (5 files: test_config.py, test_types.py, test_client_operations.py, test_error_handling.py, test_properties.py)
- [x] Hypothesis property-based tests (10 tests in test_properties.py)

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
- [x] Async Python client (all methods use `future_into_py`; `pool_stats`/`close` converted to async awaitables)
- [x] Type stubs (`.pyi` files) + `py.typed` marker (PEP 561) (completed 2026-05-07)
  - **Delivered:** `python/amaters/__init__.pyi` — full PEP 561 stubs for `AmateRSClient`, `ClientConfig`, `RetryConfig`, `Key`, `BatchResult`, `ScanResult`, `StreamIterator`, `BatchStreamIterator`, and exception classes (`AmateRSError`, `ConnectionError`, `TimeoutError`). `python/amaters/py.typed` (empty marker). `Cargo.toml` `[package.metadata.maturin]` + `pyproject.toml` `[tool.maturin]` both set `python-source = "python"` so maturin includes the stubs in the wheel automatically.
- [x] Documentation examples in `/python/` module (completed 2026-05-08)
  - **Delivered:** `python/amaters/examples/__init__.py` plus four runnable scripts — `quickstart.py` (CRUD walkthrough: set/get/contains/delete), `batch.py` (`batch_set`/`batch_get`/`batch`/`batch_delete`), `streaming.py` (range + prefix + batch streaming over a 100-key dataset), and `transactions.py` (placeholder calling out the still-pending Python-side transaction binding and demonstrating the `batch_set` substitute pattern). Each file documents the `AMATERS_SERVER` environment variable, runs via `python -m amaters.examples.<name>` or directly, and uses `asyncio.run(...)` with an `if __name__ == "__main__":` guard. `python-source = "python"` in `pyproject.toml` automatically includes the directory in the maturin wheel.
- [x] Prefix query method (currently implemented via `scan`; dedicated `prefix_query` pending) (completed 2026-05-08)
  - **Delivered:** New `AmateRSClient.prefix_query(collection, prefix) -> list[tuple[bytes, bytes]]` and `AmateRSClient.prefix_stream(collection, prefix, chunk_size) -> StreamIterator` PyO3 methods in `crates/amaters-sdk-python/src/lib.rs`, plus the supporting `prefix_upper_bound(&[u8]) -> Option<Vec<u8>>` helper and a 256-byte saturated end-key sentinel (`saturated_end_key()`) for the all-0xFF / empty-prefix fallback. PyO3 stubs added to `python/amaters/__init__.pyi`. Fourteen tests (8 helper unit tests + 4 tokio integration tests against `MockServerBuilder` + 2 sentinel sanity checks) all pass.
- [x] Subscribe / watch API for change notifications
- [x] Python package `__all__` exports and top-level `__init__.py` polish (completed 2026-05-08)
  - **Delivered:** `python/amaters/__init__.py` rewritten as a thin re-export shim from the compiled `amaters._internal` extension, with explicit `__all__ = [AmateRSClient, ClientConfig, RetryConfig, Key, BatchResult, ScanResult, StreamIterator, BatchStreamIterator, AmateRSError, ConnectionError, TimeoutError, __version__]`. The previous wrapper class that only forwarded 8 of 17 methods is removed; user code now sees the complete PyO3 API documented in `__init__.pyi`. The `.pyi` file gains a matching top-level `__all__`. Smoke-checked via `maturin develop` + `python -c "import amaters; print(amaters.__all__)"` — every name resolves, including the new `prefix_query`/`prefix_stream` methods on `AmateRSClient`. As part of this change, `pyproject.toml`'s stale `features = ["extension-module"]` reference (the feature was removed in 2026-04-26) was dropped so `maturin develop` works again with `PYO3_BUILD_EXTENSION_MODULE=1`.
- [x] Python test suite (completed 2026-06-03) — 116 pytest tests in `python/tests/` (5 files: test_config.py, test_types.py, test_client_operations.py, test_error_handling.py, test_properties.py); includes 10 Hypothesis property-based tests; all run without the compiled extension via mock-backed clients
- [x] Integration tests against a live AmateRS server
