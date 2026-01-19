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
mod types;

use wasm_bindgen::prelude::*;

pub use client::*;
pub use error::*;
pub use query::*;
pub use types::*;

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
}

/// Get the SDK version
#[wasm_bindgen(js_name = getVersion)]
pub fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Check if the SDK is initialized
#[wasm_bindgen(js_name = isInitialized)]
pub fn is_initialized() -> bool {
    true
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
    fn test_initialized() {
        assert!(is_initialized());
    }
}
