# amaters-sdk-typescript

TypeScript/WASM SDK for [AmateRS](https://github.com/cool-japan/amaters) — a distributed, Fully Homomorphic Encrypted (FHE) database system. This crate compiles to WebAssembly via `wasm-bindgen`, making the AmateRS client API available in browsers and Node.js environments, and also ships a native HTTP/1.1 transport for server-side use.

> **Status**: Alpha — API is stabilising. Not yet recommended for production use.

- 84 tests
- 189 public API items
- Version: 0.2.0
- License: Apache-2.0

## Features

- **Dual transport** — gRPC (WASM) for browser/Node.js environments and native HTTP/1.1 for server-side runtimes
- **WebAssembly support** — compile to WASM for browser and Node.js via `wasm-bindgen`
- **Connection management** — configurable endpoints, timeout handling, and connection lifecycle
- **Batch operations** — multi-key get/set/delete in a single round-trip
- **Retry logic** — configurable retry with backoff for transient failures
- **Async/Promise API** — all client operations return native Promises
- **Panic hook integration** — optional `console_error_panic_hook` for readable WASM panics

## Installation

```bash
npm install amaters
# or
yarn add amaters
# or
pnpm add amaters
```

For WASM builds, use [wasm-pack](https://rustwasm.github.io/wasm-pack/):

```bash
wasm-pack build --target bundler
```

## Basic Usage

```typescript
import { AmateRSClient, ClientConfig } from "amaters";

async function main(): Promise<void> {
  const config: ClientConfig = {
    endpoint: "http://127.0.0.1:7777",
  };
  const client = await AmateRSClient.connect(config);

  // Store a value
  const encoder = new TextEncoder();
  await client.put("session:abc123:data", encoder.encode("hello world"));

  // Retrieve the value
  const raw = await client.get("session:abc123:data");
  const decoder = new TextDecoder();
  console.log("Stored value:", decoder.decode(raw));

  // Run a prefix query
  const results = await client.query("session:abc123:*");
  for (const [key, value] of Object.entries(results)) {
    console.log(`  ${key} =>`, decoder.decode(value as Uint8Array));
  }

  await client.close();
}

main().catch(console.error);
```

### Batch Operations

```typescript
// Batch write
await client.batchPut([
  ["key1", encoder.encode("value1")],
  ["key2", encoder.encode("value2")],
]);

// Batch read
const values = await client.batchGet(["key1", "key2"]);
```

### Retry Configuration

```typescript
const config: ClientConfig = {
  endpoint: "http://127.0.0.1:7777",
  maxRetries: 3,
  retryDelayMs: 200,
};
const client = await AmateRSClient.connect(config);
```

## Feature Flags (Cargo)

| Flag | Description |
|---|---|
| `console_error_panic_hook` (default) | Forward Rust panics to `console.error` in WASM |
| `serialization` | Enable JSON serialization helpers |

## Testing

```bash
# Rust unit tests
cargo test --all-features

# WASM tests via wasm-pack
wasm-pack test --headless --firefox
```

## Project

AmateRS is developed by COOLJAPAN OU (Team Kitasan).
Source and issue tracker: <https://github.com/cool-japan/amaters>
