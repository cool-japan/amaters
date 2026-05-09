//! # AmateRS TypeScript SDK (WASM)
//!
//! This crate provides WebAssembly bindings for the AmateRS Rust SDK,
//! enabling TypeScript/JavaScript developers to interact with the AmateRS
//! Fully Homomorphic Encrypted (FHE) Database.
//!
//! ## Features
//!
//! - Full TypeScript type definitions
//! - Async/await support using wasm-bindgen-futures
//! - Fluent query builder API
//! - Connection pooling and retry logic
//! - Proper error handling with TypeScript-friendly errors
//!
//! ## Usage
//!
//! ```typescript
//! import { AmateRSClient, Key, CipherBlob, QueryBuilder } from '@amaters/sdk';
//!
//! // Connect to server
//! const client = await AmateRSClient.connect('http://localhost:50051');
//!
//! // Set a value
//! await client.set('users', Key.fromString('user:123'), CipherBlob.fromBytes(data));
//!
//! // Get a value
//! const value = await client.get('users', Key.fromString('user:123'));
//!
//! // Use query builder
//! const query = new QueryBuilder('users')
//!     .whereClause()
//!     .eq('status', CipherBlob.fromBytes(statusData))
//!     .build();
//! ```

#![allow(clippy::type_complexity)]
#![allow(clippy::new_without_default)]

mod client;
mod error;
mod query;
mod transport;
mod types;

use std::sync::atomic::{AtomicBool, Ordering};
use wasm_bindgen::prelude::*;

pub use client::*;
pub use error::*;
pub use query::*;
pub use transport::*;
pub use types::*;

/// Tracks whether the WASM module has been initialized via `init()`.
static WASM_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialize the WASM module
///
/// This should be called once when the module is loaded.
/// It sets up panic hooks for better error messages.
#[wasm_bindgen(start)]
pub fn init() {
    // Set up panic hook for better error messages
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();

    // Initialize tracing for WASM
    #[cfg(target_arch = "wasm32")]
    {
        let _ = tracing_wasm::try_set_as_global_default();
    }

    WASM_INITIALIZED.store(true, Ordering::Release);
}

/// Get the SDK version
#[wasm_bindgen(js_name = getVersion)]
pub fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Check if the SDK is initialized.
///
/// Returns `true` only after `init()` has been called (which happens
/// automatically when the WASM module loads in a browser/Node environment).
/// In native test contexts `init()` is not auto-called, so this returns
/// `false` until `init()` is invoked manually.
#[wasm_bindgen(js_name = isInitialized)]
pub fn is_initialized() -> bool {
    WASM_INITIALIZED.load(Ordering::Acquire)
}

/// Execute a query against an AmateRS server.
///
/// This is a top-level convenience export that parses the provided JSON,
/// validates it, and returns a JSON-encoded result array. A transient
/// in-memory client is used for the execution path; full HTTP transport
/// support requires a browser/WASM environment with the complete transport
/// layer (see `AmateRSClient::connect`).
///
/// # Arguments
/// - `server_url`: e.g. `"http://localhost:50051"` (reserved for future HTTP transport)
/// - `collection`: collection name
/// - `query_json`: JSON-encoded query object (e.g. `{"type":"get","key":"foo"}`)
///
/// # Returns
/// JSON-encoded empty result array on success, or a JavaScript Error on
/// parse/serialization failure.
#[wasm_bindgen(js_name = query)]
pub async fn wasm_query(
    server_url: String,
    collection: String,
    query_json: String,
) -> std::result::Result<JsValue, JsValue> {
    // Validate query_json is parseable JSON; propagate parse errors as JS errors.
    serde_json::from_str::<serde_json::Value>(&query_json)
        .map_err(|e| JsValue::from_str(&format!("invalid query_json: {}", e)))?;

    // Validate that server_url and collection are non-empty.
    if server_url.trim().is_empty() {
        return Err(JsValue::from_str("server_url must not be empty"));
    }
    if collection.trim().is_empty() {
        return Err(JsValue::from_str("collection must not be empty"));
    }

    // Return empty result array. Full HTTP transport for WASM requires the
    // browser fetch API via web-sys and is tracked separately.
    let empty: Vec<serde_json::Value> = Vec::new();
    serde_wasm_bindgen::to_value(&empty)
        .map_err(|e| JsValue::from_str(&format!("serialization error: {}", e)))
}

/// Validate inputs for `wasm_stream_query`. Sync so it can be exercised in
/// native unit tests without an async executor.
///
/// Returns `Ok(())` if every argument is well-formed; returns a human-readable
/// error message otherwise. The error string mirrors the JS-side error that
/// `wasm_stream_query` produces when it dispatches `on_error`.
pub(crate) fn validate_stream_args(
    server_url: &str,
    collection: &str,
    query_json: &str,
) -> std::result::Result<(), String> {
    if server_url.trim().is_empty() {
        return Err("server_url must not be empty".to_string());
    }
    if collection.trim().is_empty() {
        return Err("collection must not be empty".to_string());
    }
    serde_json::from_str::<serde_json::Value>(query_json)
        .map(|_| ())
        .map_err(|e| format!("invalid query_json: {}", e))
}

/// Stream a query against an AmateRS server, emitting key-value chunks via
/// JavaScript callbacks for async-iterator consumption from TypeScript.
///
/// The TypeScript wrapper `streamQuery` adapts these callbacks into an
/// `AsyncIterableIterator<KeyValuePair>` using a queue + Promise-pull state
/// machine; see `crates/amaters-sdk-typescript/src/ts/index.ts`.
///
/// # Arguments
/// - `server_url`: e.g. `"http://localhost:50051"` (reserved for future HTTP transport).
/// - `collection`: collection name.
/// - `query_json`: JSON-encoded query object.
/// - `on_chunk`: JS function `(key: string, value: string) => void`, invoked for each emitted KV pair.
/// - `on_done`: JS function `() => void`, invoked once after the final chunk.
/// - `on_error`: JS function `(message: string) => void`, invoked instead of `on_done` if streaming fails.
///
/// # Stub limitation
/// Real server-streaming RPC integration is tracked separately; the producer
/// here is deterministic for SDK-layer testing. It emits exactly three
/// synthetic chunks (`k1`/`v1`, `k2`/`v2`, `k3`/`v3`) followed by `on_done`,
/// in a single future poll. Cancellation through the TS-side iterator's
/// `return()` is therefore TS-side bookkeeping until the real streaming
/// transport lands.
///
/// # Returns
/// A JavaScript value (currently `JsValue::UNDEFINED`) on success; a
/// JavaScript Error if input validation fails. On validation failure
/// `on_error` is also invoked with the message before the `Err` is returned.
#[wasm_bindgen(js_name = streamQuery)]
pub async fn wasm_stream_query(
    server_url: String,
    collection: String,
    query_json: String,
    on_chunk: js_sys::Function,
    on_done: js_sys::Function,
    on_error: js_sys::Function,
) -> std::result::Result<JsValue, JsValue> {
    if let Err(message) = validate_stream_args(&server_url, &collection, &query_json) {
        let msg_jsval = JsValue::from_str(&message);
        // Best-effort error notification to the JS side. The Err return is
        // the canonical signal; we ignore the callback's own thrown errors
        // so that validation never silently succeeds.
        let _ = on_error.call1(&JsValue::NULL, &msg_jsval);
        return Err(msg_jsval);
    }

    // Deterministic stub producer. See the function-level "Stub limitation".
    // Propagating callback errors via `?` surfaces a JS-side abort (e.g.
    // iterator return() throwing back through the bridge) as an ordered
    // shutdown rather than a stream error.
    for (key, value) in [("k1", "v1"), ("k2", "v2"), ("k3", "v3")] {
        let k_jsval = JsValue::from_str(key);
        let v_jsval = JsValue::from_str(value);
        on_chunk.call2(&JsValue::NULL, &k_jsval, &v_jsval)?;
    }
    on_done.call0(&JsValue::NULL)?;
    Ok(JsValue::UNDEFINED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        let version = get_version();
        assert!(!version.is_empty());
    }

    #[test]
    fn test_initialized_state_tracking() {
        // Each test controls the state it exercises to avoid races when
        // nextest runs tests in parallel against the process-global AtomicBool.
        WASM_INITIALIZED.store(false, Ordering::Release);
        assert!(!is_initialized(), "should be false before init");

        WASM_INITIALIZED.store(true, Ordering::Release);
        assert!(is_initialized(), "should be true after storing true");

        // Reset to avoid leaking state into other tests.
        WASM_INITIALIZED.store(false, Ordering::Release);
    }

    #[test]
    fn test_set_initialized_and_reset() {
        WASM_INITIALIZED.store(true, Ordering::Release);
        assert!(is_initialized());
        WASM_INITIALIZED.store(false, Ordering::Release);
        assert!(!is_initialized());
    }

    #[test]
    fn test_query_export_exists() {
        // Verify the async function signature compiles and can be named without
        // actually driving the future (no async executor available in native tests).
        let _fut = wasm_query("http://localhost:50051".into(), "users".into(), "{}".into());
    }

    #[test]
    fn test_validate_stream_args_accepts_valid_inputs() {
        assert!(
            validate_stream_args(
                "http://localhost:50051",
                "users",
                "{\"type\":\"get\",\"key\":\"foo\"}",
            )
            .is_ok()
        );
        // Plain JSON values are accepted (string, number, bool) — only requires parseable JSON.
        assert!(validate_stream_args("http://x", "c", "null").is_ok());
        assert!(validate_stream_args("http://x", "c", "[]").is_ok());
    }

    #[test]
    fn test_validate_stream_args_rejects_empty_url() {
        let err = validate_stream_args("", "users", "{}").expect_err("empty url must fail");
        assert!(
            err.contains("server_url"),
            "error should mention server_url, got: {}",
            err
        );
        // Whitespace-only is also rejected.
        let err = validate_stream_args("   ", "users", "{}").expect_err("whitespace url must fail");
        assert!(err.contains("server_url"));
    }

    #[test]
    fn test_validate_stream_args_rejects_empty_collection() {
        let err = validate_stream_args("http://localhost:50051", "", "{}")
            .expect_err("empty collection must fail");
        assert!(
            err.contains("collection"),
            "error should mention collection, got: {}",
            err
        );
        let err = validate_stream_args("http://localhost:50051", "  ", "{}")
            .expect_err("whitespace collection must fail");
        assert!(err.contains("collection"));
    }

    #[test]
    fn test_validate_stream_args_rejects_invalid_json() {
        let err = validate_stream_args("http://localhost:50051", "users", "not json")
            .expect_err("invalid JSON must fail");
        assert!(
            err.contains("invalid query_json"),
            "error should mention invalid query_json, got: {}",
            err
        );
    }

    #[test]
    fn test_wasm_stream_query_export_exists() {
        // Native tests cannot drive the WASM async executor or construct
        // `js_sys::Function` callbacks — verify the exported signature
        // compiles and can be referenced. The runtime behavior is exercised
        // via the TypeScript test suite (see `test/streaming.test.ts`) and
        // the validation logic is exercised synchronously above via
        // `validate_stream_args`.
        let _ = wasm_stream_query;
    }
}
