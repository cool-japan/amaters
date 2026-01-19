//! FHE Server Key Management
//!
//! This module provides centralized management of TFHE server keys for multiple clients.
//! Each client can register their server key, and the system can execute FHE operations
//! using the appropriate key.

use crate::error::{AmateRSError, ErrorContext, Result};
use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::Arc;

#[cfg(feature = "compute")]
use tfhe::ServerKey;

/// Type alias for client identifiers
pub type ClientId = String;

/// Manages FHE server keys for multiple clients
///
/// This structure provides thread-safe storage and retrieval of TFHE server keys.
/// It supports both multi-client scenarios (where each client has their own key)
/// and single-client scenarios (where a global key is used).
///
/// # Example
///
/// ```rust,ignore
/// use amaters_core::compute::KeyManager;
/// use tfhe::ServerKey;
///
/// let manager = KeyManager::new();
///
/// // Register a key for a client
/// let server_key = ServerKey::new(&client_key);
/// manager.register_key("client_1".to_string(), server_key);
///
/// // Retrieve the key
/// let key = manager.get_key("client_1").expect("Client key should exist");
///
/// // Set as global for convenience
/// manager.set_global("client_1")?;
/// let global_key = manager.get_global().expect("Global key should be set");
/// ```
#[derive(Default)]
pub struct KeyManager {
    /// Map of client IDs to their server keys
    server_keys: DashMap<ClientId, Arc<ServerKey>>,

    /// Optional global server key (for single-client scenarios)
    global_key: RwLock<Option<Arc<ServerKey>>>,
}

impl KeyManager {
    /// Create a new empty key manager
    ///
    /// # Example
    ///
    /// ```rust
    /// use amaters_core::compute::KeyManager;
    ///
    /// let manager = KeyManager::new();
    /// assert_eq!(manager.key_count(), 0);
    /// ```
    pub fn new() -> Self {
        Self {
            server_keys: DashMap::new(),
            global_key: RwLock::new(None),
        }
    }

    /// Register a server key for a specific client
    ///
    /// If a key already exists for this client, it will be replaced.
    ///
    /// # Arguments
    ///
    /// * `client_id` - Unique identifier for the client
    /// * `key` - The TFHE server key to register
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use amaters_core::compute::KeyManager;
    ///
    /// let manager = KeyManager::new();
    /// let server_key = ServerKey::new(&client_key);
    /// manager.register_key("client_1".to_string(), server_key);
    /// ```
    #[cfg(feature = "compute")]
    pub fn register_key(&self, client_id: ClientId, key: ServerKey) {
        self.server_keys.insert(client_id, Arc::new(key));
    }

    /// Stub for register_key when compute feature is disabled
    #[cfg(not(feature = "compute"))]
    pub fn register_key(&self, _client_id: ClientId, _key: ()) {
        // No-op when compute is disabled
    }

    /// Get server key for a specific client
    ///
    /// Returns `None` if no key is registered for the given client.
    ///
    /// # Arguments
    ///
    /// * `client_id` - The client identifier
    ///
    /// # Returns
    ///
    /// An `Arc<ServerKey>` if found, `None` otherwise
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use amaters_core::compute::KeyManager;
    ///
    /// let manager = KeyManager::new();
    /// manager.register_key("client_1".to_string(), server_key);
    ///
    /// let key = manager.get_key("client_1").expect("Key should exist");
    /// ```
    #[cfg(feature = "compute")]
    pub fn get_key(&self, client_id: &str) -> Option<Arc<ServerKey>> {
        self.server_keys
            .get(client_id)
            .map(|entry| entry.value().clone())
    }

    /// Stub for get_key when compute feature is disabled
    #[cfg(not(feature = "compute"))]
    pub fn get_key(&self, _client_id: &str) -> Option<()> {
        None
    }

    /// Set a global server key from a registered client key
    ///
    /// This is a convenience method for single-client scenarios where you want
    /// to use one client's key as the default for all operations.
    ///
    /// # Arguments
    ///
    /// * `client_id` - The client whose key should become the global key
    ///
    /// # Errors
    ///
    /// Returns an error if no key is registered for the specified client.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use amaters_core::compute::KeyManager;
    ///
    /// let manager = KeyManager::new();
    /// manager.register_key("default".to_string(), server_key);
    /// manager.set_global("default")?;
    ///
    /// let global = manager.get_global().expect("Global key should be set");
    /// ```
    pub fn set_global(&self, client_id: &str) -> Result<()> {
        #[cfg(feature = "compute")]
        {
            let key = self.get_key(client_id).ok_or_else(|| {
                AmateRSError::FheComputation(ErrorContext::new(format!(
                    "No server key found for client: {}",
                    client_id
                )))
            })?;

            let mut global = self.global_key.write();
            *global = Some(key);
            Ok(())
        }

        #[cfg(not(feature = "compute"))]
        {
            let _ = client_id;
            Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
                "FHE compute feature is not enabled".to_string(),
            )))
        }
    }

    /// Get the global server key
    ///
    /// Returns `None` if no global key has been set.
    ///
    /// # Returns
    ///
    /// An `Arc<ServerKey>` if a global key is set, `None` otherwise
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use amaters_core::compute::KeyManager;
    ///
    /// let manager = KeyManager::new();
    /// assert!(manager.get_global().is_none());
    ///
    /// manager.register_key("default".to_string(), server_key);
    /// manager.set_global("default")?;
    /// assert!(manager.get_global().is_some());
    /// ```
    #[cfg(feature = "compute")]
    pub fn get_global(&self) -> Option<Arc<ServerKey>> {
        self.global_key.read().clone()
    }

    /// Stub for get_global when compute feature is disabled
    #[cfg(not(feature = "compute"))]
    pub fn get_global(&self) -> Option<()> {
        None
    }

    /// Remove a client's key
    ///
    /// Returns `true` if the key was found and removed, `false` otherwise.
    ///
    /// # Arguments
    ///
    /// * `client_id` - The client identifier
    ///
    /// # Returns
    ///
    /// `true` if a key was removed, `false` if no key was found
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use amaters_core::compute::KeyManager;
    ///
    /// let manager = KeyManager::new();
    /// manager.register_key("client_1".to_string(), server_key);
    ///
    /// assert!(manager.remove_key("client_1"));
    /// assert!(!manager.remove_key("client_1")); // Already removed
    /// ```
    pub fn remove_key(&self, client_id: &str) -> bool {
        self.server_keys.remove(client_id).is_some()
    }

    /// Get the number of registered keys
    ///
    /// # Returns
    ///
    /// The count of currently registered client keys
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use amaters_core::compute::KeyManager;
    ///
    /// let manager = KeyManager::new();
    /// assert_eq!(manager.key_count(), 0);
    ///
    /// manager.register_key("client_1".to_string(), server_key);
    /// assert_eq!(manager.key_count(), 1);
    /// ```
    pub fn key_count(&self) -> usize {
        self.server_keys.len()
    }

    /// Clear all registered keys (including global)
    ///
    /// This removes all client keys and clears the global key.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use amaters_core::compute::KeyManager;
    ///
    /// let manager = KeyManager::new();
    /// manager.register_key("client_1".to_string(), server_key);
    /// manager.set_global("client_1")?;
    ///
    /// manager.clear();
    /// assert_eq!(manager.key_count(), 0);
    /// assert!(manager.get_global().is_none());
    /// ```
    pub fn clear(&self) {
        self.server_keys.clear();
        let mut global = self.global_key.write();
        *global = None;
    }
}

#[cfg(all(test, feature = "compute"))]
mod tests {
    use super::*;
    use crate::compute::FheKeyPair;

    #[test]
    fn test_key_manager_new() {
        let manager = KeyManager::new();
        assert_eq!(manager.key_count(), 0);
        assert!(manager.get_global().is_none());
    }

    #[test]
    fn test_register_and_get_key() -> Result<()> {
        let manager = KeyManager::new();
        let keypair = FheKeyPair::generate()?;

        manager.register_key("client_1".to_string(), keypair.server_key().clone());

        let retrieved = manager.get_key("client_1");
        assert!(retrieved.is_some());
        assert_eq!(manager.key_count(), 1);

        Ok(())
    }

    #[test]
    fn test_get_nonexistent_key() {
        let manager = KeyManager::new();
        let result = manager.get_key("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_set_and_get_global() -> Result<()> {
        let manager = KeyManager::new();
        let keypair = FheKeyPair::generate()?;

        manager.register_key("default".to_string(), keypair.server_key().clone());
        manager.set_global("default")?;

        let global = manager.get_global();
        assert!(global.is_some());

        Ok(())
    }

    #[test]
    fn test_set_global_nonexistent_client() {
        let manager = KeyManager::new();
        let result = manager.set_global("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_key() -> Result<()> {
        let manager = KeyManager::new();
        let keypair = FheKeyPair::generate()?;

        manager.register_key("client_1".to_string(), keypair.server_key().clone());
        assert_eq!(manager.key_count(), 1);

        let removed = manager.remove_key("client_1");
        assert!(removed);
        assert_eq!(manager.key_count(), 0);

        // Try to remove again
        let removed_again = manager.remove_key("client_1");
        assert!(!removed_again);

        Ok(())
    }

    #[test]
    fn test_key_count() -> Result<()> {
        let manager = KeyManager::new();
        assert_eq!(manager.key_count(), 0);

        let keypair1 = FheKeyPair::generate()?;
        let keypair2 = FheKeyPair::generate()?;

        manager.register_key("client_1".to_string(), keypair1.server_key().clone());
        assert_eq!(manager.key_count(), 1);

        manager.register_key("client_2".to_string(), keypair2.server_key().clone());
        assert_eq!(manager.key_count(), 2);

        Ok(())
    }

    #[test]
    fn test_replace_existing_key() -> Result<()> {
        let manager = KeyManager::new();
        let keypair1 = FheKeyPair::generate()?;
        let keypair2 = FheKeyPair::generate()?;

        manager.register_key("client_1".to_string(), keypair1.server_key().clone());
        manager.register_key("client_1".to_string(), keypair2.server_key().clone());

        // Should still have only 1 key (replaced)
        assert_eq!(manager.key_count(), 1);

        Ok(())
    }

    #[test]
    fn test_clear() -> Result<()> {
        let manager = KeyManager::new();
        let keypair1 = FheKeyPair::generate()?;
        let keypair2 = FheKeyPair::generate()?;

        manager.register_key("client_1".to_string(), keypair1.server_key().clone());
        manager.register_key("client_2".to_string(), keypair2.server_key().clone());
        manager.set_global("client_1")?;

        assert_eq!(manager.key_count(), 2);
        assert!(manager.get_global().is_some());

        manager.clear();

        assert_eq!(manager.key_count(), 0);
        assert!(manager.get_global().is_none());

        Ok(())
    }

    #[test]
    fn test_concurrent_access() -> Result<()> {
        use std::thread;

        let manager = Arc::new(KeyManager::new());
        let mut handles = vec![];

        // Spawn multiple threads that register keys
        for i in 0..10 {
            let manager_clone = Arc::clone(&manager);
            let handle = thread::spawn(move || -> Result<()> {
                let keypair = FheKeyPair::generate()?;
                let client_id = format!("client_{}", i);
                manager_clone.register_key(client_id.clone(), keypair.server_key().clone());

                // Try to retrieve it
                let retrieved = manager_clone.get_key(&client_id);
                assert!(retrieved.is_some());

                Ok(())
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle
                .join()
                .expect("Thread panicked")
                .expect("Thread failed");
        }

        assert_eq!(manager.key_count(), 10);

        Ok(())
    }
}
