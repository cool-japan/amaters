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
- [ ] Streaming query support as async iteration in TypeScript layer (`AsyncIterator<KeyValuePair>`)
- [ ] WebSocket transport option for browser environments
- [ ] `isInitialized()` returning `true` after WASM init
- [ ] `query()` helper connected to WASM module (currently throws before init)
- [ ] npm package publish (`@amaters/sdk`)
- [ ] ESLint + Prettier CI enforcement
- [ ] WASM headless browser test suite (`wasm-pack test --headless --chrome`)
