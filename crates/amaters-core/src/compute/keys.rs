//! Key management for FHE operations
//!
//! This module handles client and server key generation, serialization,
//! and management for TFHE operations.

use crate::error::{AmateRSError, ErrorContext, Result};

#[cfg(feature = "compute")]
use tfhe::prelude::*;
#[cfg(feature = "compute")]
use tfhe::{ConfigBuilder, FheBool, FheUint8, FheUint16, FheUint32, generate_keys, set_server_key};

/// Key pair for FHE operations
///
/// Contains both client key (for encryption/decryption) and server key (for FHE operations).
/// The keys are generated together and must be used as a pair.
#[cfg(feature = "compute")]
#[derive(Clone)]
pub struct FheKeyPair {
    client_key: tfhe::ClientKey,
    server_key: tfhe::ServerKey,
}

#[cfg(feature = "compute")]
impl FheKeyPair {
    /// Generate a new key pair with default parameters
    ///
    /// This uses TFHE's default configuration which provides good security/performance balance.
    pub fn generate() -> Result<Self> {
        let config = ConfigBuilder::default().build();
        let (client_key, server_key) = generate_keys(config);

        Ok(Self {
            client_key,
            server_key,
        })
    }

    /// Generate a new key pair with custom configuration
    pub fn generate_with_config(config: tfhe::Config) -> Result<Self> {
        let (client_key, server_key) = generate_keys(config);

        Ok(Self {
            client_key,
            server_key,
        })
    }

    /// Get reference to client key
    pub fn client_key(&self) -> &tfhe::ClientKey {
        &self.client_key
    }

    /// Get reference to server key
    pub fn server_key(&self) -> &tfhe::ServerKey {
        &self.server_key
    }

    /// Set this key pair's server key as the global server key
    ///
    /// TFHE operations require a server key to be set globally.
    pub fn set_as_global_server_key(&self) {
        set_server_key(self.server_key.clone());
    }

    /// Serialize client key to bytes
    pub fn serialize_client_key(&self) -> Result<Vec<u8>> {
        bincode::serialize(&self.client_key).map_err(|e| {
            AmateRSError::Serialization(ErrorContext::new(format!(
                "Failed to serialize client key: {}",
                e
            )))
        })
    }

    /// Serialize server key to bytes
    pub fn serialize_server_key(&self) -> Result<Vec<u8>> {
        bincode::serialize(&self.server_key).map_err(|e| {
            AmateRSError::Serialization(ErrorContext::new(format!(
                "Failed to serialize server key: {}",
                e
            )))
        })
    }

    /// Deserialize client key from bytes
    pub fn deserialize_client_key(bytes: &[u8]) -> Result<tfhe::ClientKey> {
        bincode::deserialize(bytes).map_err(|e| {
            AmateRSError::Deserialization(ErrorContext::new(format!(
                "Failed to deserialize client key: {}",
                e
            )))
        })
    }

    /// Deserialize server key from bytes
    pub fn deserialize_server_key(bytes: &[u8]) -> Result<tfhe::ServerKey> {
        bincode::deserialize(bytes).map_err(|e| {
            AmateRSError::Deserialization(ErrorContext::new(format!(
                "Failed to deserialize server key: {}",
                e
            )))
        })
    }

    /// Create key pair from serialized keys
    pub fn from_serialized(client_key_bytes: &[u8], server_key_bytes: &[u8]) -> Result<Self> {
        let client_key = Self::deserialize_client_key(client_key_bytes)?;
        let server_key = Self::deserialize_server_key(server_key_bytes)?;

        Ok(Self {
            client_key,
            server_key,
        })
    }
}

/// Stub implementation when compute feature is disabled
#[cfg(not(feature = "compute"))]
#[derive(Clone, Debug)]
pub struct FheKeyPair {
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(not(feature = "compute"))]
impl FheKeyPair {
    pub fn generate() -> Result<Self> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }

    pub fn serialize_client_key(&self) -> Result<Vec<u8>> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }

    pub fn serialize_server_key(&self) -> Result<Vec<u8>> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }
}

/// Key storage interface for managing FHE keys
///
/// This trait defines how keys are stored and retrieved.
/// Implementations can use file system, memory, or remote key management services.
pub trait KeyStorage: Send + Sync {
    /// Store client key
    fn store_client_key(&self, key_id: &str, key: &[u8]) -> Result<()>;

    /// Store server key
    fn store_server_key(&self, key_id: &str, key: &[u8]) -> Result<()>;

    /// Retrieve client key
    fn retrieve_client_key(&self, key_id: &str) -> Result<Vec<u8>>;

    /// Retrieve server key
    fn retrieve_server_key(&self, key_id: &str) -> Result<Vec<u8>>;

    /// Delete keys
    fn delete_keys(&self, key_id: &str) -> Result<()>;

    /// List all key IDs
    fn list_key_ids(&self) -> Result<Vec<String>>;
}

/// In-memory key storage for testing and development
#[derive(Default)]
pub struct InMemoryKeyStorage {
    client_keys: std::sync::Arc<dashmap::DashMap<String, Vec<u8>>>,
    server_keys: std::sync::Arc<dashmap::DashMap<String, Vec<u8>>>,
}

impl InMemoryKeyStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyStorage for InMemoryKeyStorage {
    fn store_client_key(&self, key_id: &str, key: &[u8]) -> Result<()> {
        self.client_keys.insert(key_id.to_string(), key.to_vec());
        Ok(())
    }

    fn store_server_key(&self, key_id: &str, key: &[u8]) -> Result<()> {
        self.server_keys.insert(key_id.to_string(), key.to_vec());
        Ok(())
    }

    fn retrieve_client_key(&self, key_id: &str) -> Result<Vec<u8>> {
        self.client_keys
            .get(key_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| {
                AmateRSError::KeyNotFound(ErrorContext::new(format!(
                    "Client key not found: {}",
                    key_id
                )))
            })
    }

    fn retrieve_server_key(&self, key_id: &str) -> Result<Vec<u8>> {
        self.server_keys
            .get(key_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| {
                AmateRSError::KeyNotFound(ErrorContext::new(format!(
                    "Server key not found: {}",
                    key_id
                )))
            })
    }

    fn delete_keys(&self, key_id: &str) -> Result<()> {
        self.client_keys.remove(key_id);
        self.server_keys.remove(key_id);
        Ok(())
    }

    fn list_key_ids(&self) -> Result<Vec<String>> {
        Ok(self
            .client_keys
            .iter()
            .map(|entry| entry.key().clone())
            .collect())
    }
}

#[cfg(all(test, feature = "compute"))]
mod tests {
    use super::*;

    #[test]
    fn test_key_generation() -> Result<()> {
        let _keypair = FheKeyPair::generate()?;
        // Successfully generated keys - test passes
        Ok(())
    }

    #[test]
    fn test_key_serialization() -> Result<()> {
        let keypair = FheKeyPair::generate()?;

        let client_bytes = keypair.serialize_client_key()?;
        let server_bytes = keypair.serialize_server_key()?;

        assert!(!client_bytes.is_empty());
        assert!(!server_bytes.is_empty());

        Ok(())
    }

    #[test]
    fn test_key_deserialization() -> Result<()> {
        let keypair = FheKeyPair::generate()?;

        let client_bytes = keypair.serialize_client_key()?;
        let server_bytes = keypair.serialize_server_key()?;

        let restored = FheKeyPair::from_serialized(&client_bytes, &server_bytes)?;

        // Verify keys work by doing a simple encryption/decryption
        let value = true;
        let encrypted = FheBool::encrypt(value, restored.client_key());
        let decrypted: bool = encrypted.decrypt(restored.client_key());
        assert_eq!(value, decrypted);

        Ok(())
    }

    #[test]
    fn test_key_storage() -> Result<()> {
        let storage = InMemoryKeyStorage::new();
        let keypair = FheKeyPair::generate()?;

        let client_bytes = keypair.serialize_client_key()?;
        let server_bytes = keypair.serialize_server_key()?;

        storage.store_client_key("test_key", &client_bytes)?;
        storage.store_server_key("test_key", &server_bytes)?;

        let retrieved_client = storage.retrieve_client_key("test_key")?;
        let retrieved_server = storage.retrieve_server_key("test_key")?;

        assert_eq!(client_bytes, retrieved_client);
        assert_eq!(server_bytes, retrieved_server);

        let key_ids = storage.list_key_ids()?;
        assert_eq!(key_ids.len(), 1);
        assert_eq!(key_ids[0], "test_key");

        storage.delete_keys("test_key")?;
        assert!(storage.retrieve_client_key("test_key").is_err());

        Ok(())
    }
}
