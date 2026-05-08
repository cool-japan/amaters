# amaters-sdk-typescript TODO

## v0.2.0 (2026-04-26)

### Completed
- [x] TypeScript/JavaScript client SDK (`src/ts/index.ts`)
- [x] Browser-compatible WASM build via wasm-pack
- [x] Version aligned to 0.2.0 (`getVersion()` returns `'0.2.0'`)
- [x] `getVersion()` API
- [x] `AmateRSClient` interface (connect, set, get, delete, contains, range, batch, executeQuery, healthCheck, close, reconnect)
- [x] `ClientConfig` with builder methods (timeout, max connections, retries, backoff)
- [x] `Key`, `CipherBlob`, `QueryBuilder`, `Predicate`, `UpdateOp` types
- [x] Batch operations (`BatchOperation` with set/delete/get)
- [x] Range queries
- [x] FHE predicate filtering (Eq, Gt, Lt, Gte, Lte, And, Or, Not)
- [x] FHE update operations (Set, Add, Mul)
- [x] Native HTTP/1.1 transport (Rust, `src/transport.rs`)
- [x] Subscription/streaming handle in transport layer (`src/transport.rs`)
- [x] Error types with retry semantics (`ErrorCode`, `AmateRSError`)
- [x] Node.js and browser dual export (`package.json` exports map)
- [x] 84 passing tests

### Planned
- [x] Streaming query support as async iteration in TypeScript layer (`AsyncIterableIterator<KeyValuePair>`) (done 2026-05-08)
  - **Goal:** Expose `streamQuery(serverUrl, collection, query)` returning an `AsyncIterableIterator<KeyValuePair>`, so consumers can `for await (const kv of client.streamQuery(...))` over query results. Mirrors sdk-rust's `QueryStream`.
  - **Design:**
    - **Rust** (`src/lib.rs`): new `#[wasm_bindgen(js_name = streamQuery)] pub async fn wasm_stream_query(server_url, collection, query_json, on_chunk, on_done, on_error) -> Result<JsValue, JsValue>`. Validates inputs through a sync helper `validate_stream_args` (so tests can exercise the error path natively without an async executor). Producer is a deterministic 3-chunk stub (matches `wasm_query`'s "transient in-memory client" posture); real server-streaming RPC integration is tracked separately. On chunk fires `on_chunk(key_str, value_str)`; on stream end fires `on_done()`; on error fires `on_error(message)` and returns `Err`.
    - **TypeScript** (`src/ts/index.ts`): factor the queue + Promise-pull state machine into `createStreamIterator<T>(start)`, a generic helper that takes a `start(onChunk, onDone, onError)` callback. `streamQuery` is a thin adapter that auto-awaits `init()` then invokes `wasm_stream_query`. Yields the existing public `KeyValuePair` shape (reuses local `makeKeyFromString`/`makeCipherBlobFromString` helpers that satisfy the public `Key` / `CipherBlob` interfaces) — does NOT introduce a new string-shaped `KeyValuePair`. Cancellation via `return()` flips an `aborted` flag and drains pending waiters with `done: true`. Error propagation rejects pending waiters with the WASM-side message. Backpressure: `next()` either pulls from the queue or registers a waiter for the next chunk.
  - **Files:** `crates/amaters-sdk-typescript/src/lib.rs` (modified), `crates/amaters-sdk-typescript/src/ts/index.ts` (modified), `crates/amaters-sdk-typescript/test/streaming.test.ts` (new), `crates/amaters-sdk-typescript/tsconfig.json` (modified — extended `include` to cover `test/`).
  - **Tests:**
    - Rust: `test_validate_stream_args_rejects_empty_url`, `test_validate_stream_args_rejects_empty_collection`, `test_validate_stream_args_rejects_invalid_json`, `test_validate_stream_args_accepts_valid_inputs`, `test_wasm_stream_query_export_exists` (compile-only; native tests cannot drive WASM async).
    - TypeScript: 14 tests total in `test/streaming.test.ts` covering synchronous push, asynchronous push, queue draining, parked-pull resolve-on-push, `return()` cancellation that drops post-cancel chunks, `return()` cancellation that drains pending parked pulls, `for-await-of break` triggers `return()`, error propagation to pending pulls, eager error before consumer pulls, dropping chunks after `onDone`, idempotent `Symbol.asyncIterator`, end-to-end stub yield, mid-stream cancellation, error on empty server URL / empty collection. Driven via `createStreamIterator` directly with fake producers — no WASM dependency.
  - **Risk:** (a) Async-iterator + WASM-callback bridge backpressure is implicit (consumer pulls before next chunk); documented in rustdoc + JSDoc. (b) `KeyValuePair` name collision avoided by reusing the existing public type instead of redefining as `{key: string; value: string}` per the original pseudocode. (c) Stub producer in the Rust side fires all chunks synchronously inside one future poll; the TS-side stub uses microtask-staggered emission so consumer-side cancellation is exercised meaningfully. Both stubs go away once the real server-streaming RPC lands.
- [ ] WebSocket transport option for browser environments
- [x] `isInitialized()` real state tracking + `query()` wired to new WASM export (done 2026-05-07)
  - **Goal:** Track WASM init state honestly; surface a working `query()` that auto-inits and dispatches to a real wasm-bindgen export instead of throwing.
  - **Design:**
    - **Rust** (`src/lib.rs`): `static WASM_INITIALIZED: AtomicBool` set to `true` in `init()`; `is_initialized()` reads via `Ordering::Acquire`. Added `#[wasm_bindgen(js_name = query)] pub async fn wasm_query(...)` — parses `query_json`, validates inputs, returns empty `JsValue` array (full HTTP transport tracked separately).
    - **TypeScript** (`src/ts/index.ts`): Module-level `let _initialized = false; let _initPromise: Promise<void> | null = null;`. `init()` idempotent (shares in-flight promise). `isInitialized()` returns `_initialized`. Added `executeQuery()` that awaits `init()` before dispatching to WASM export.
  - **Files:** `crates/amaters-sdk-typescript/src/lib.rs` (modified), `crates/amaters-sdk-typescript/src/ts/index.ts` (modified)
  - **Tests:** `test_initialized_state_tracking`, `test_set_initialized_and_reset`, `test_query_export_exists`
- [ ] npm package publish (`@amaters/sdk`)
- [ ] ESLint + Prettier CI enforcement
- [ ] WASM headless browser test suite (`wasm-pack test --headless --chrome`)
