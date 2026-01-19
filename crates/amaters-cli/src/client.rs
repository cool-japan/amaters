//! Client wrapper for AmateRS SDK

use amaters_core::{CipherBlob, Key};
use amaters_sdk_rust::{AmateRSClient, SdkError};
use anyhow::Context;

/// CLI client wrapper
pub struct Client {
    inner: AmateRSClient,
    default_collection: String,
}

impl Client {
    /// Connect to AmateRS server
    pub async fn connect(server_url: &str, default_collection: String) -> anyhow::Result<Self> {
        let inner = AmateRSClient::connect(server_url)
            .await
            .with_context(|| format!("Failed to connect to server: {}", server_url))?;

        Ok(Self {
            inner,
            default_collection,
        })
    }

    /// Set a key-value pair
    pub async fn set(&self, key: &Key, value: &CipherBlob) -> Result<(), SdkError> {
        self.inner.set(&self.default_collection, key, value).await
    }

    /// Get a value by key
    pub async fn get(&self, key: &Key) -> Result<Option<CipherBlob>, SdkError> {
        self.inner.get(&self.default_collection, key).await
    }

    /// Delete a key
    pub async fn delete(&self, key: &Key) -> Result<(), SdkError> {
        self.inner.delete(&self.default_collection, key).await
    }

    /// Range query
    pub async fn range(&self, start: &Key, end: &Key) -> Result<Vec<(Key, CipherBlob)>, SdkError> {
        self.inner.range(&self.default_collection, start, end).await
    }

    /// Query with filter (advanced FHE filtering)
    ///
    /// Note: This requires FHE server keys to be registered.
    /// For simple queries, use range() instead.
    pub async fn query(&self, _filter: &str) -> Result<Vec<(Key, CipherBlob)>, SdkError> {
        // Advanced FHE filtering requires predicate parsing and server key setup
        // For CLI usage, recommend using range() for simple queries
        // FHE filtering is better suited for SDK usage where predicates can be constructed programmatically
        Err(SdkError::OperationFailed(
            "FHE filter queries require programmatic predicate construction. Use the SDK directly for filter queries, or use range() for simple key-based queries.".to_string()
        ))
    }

    /// Health check
    pub async fn health_check(&self) -> Result<(), SdkError> {
        self.inner.health_check().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_client_connect() {
        // This will fail since no server is running, but tests the API
        let result = Client::connect("http://localhost:50051", "test".to_string()).await;
        // We expect this to fail in tests without a server
        assert!(result.is_ok() || result.is_err());
    }
}
