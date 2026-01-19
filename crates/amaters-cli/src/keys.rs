//! FHE key management for amaters-cli
//!
//! Provides commands for generating, importing, exporting, and managing FHE keys.
//! Keys are stored in `~/.amaters/keys/` directory.

use amaters_sdk_rust::FheKeys;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Key metadata stored alongside keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMetadata {
    /// Key name/identifier
    pub name: String,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Key type (e.g., "client", "server", "evaluation")
    pub key_type: String,
    /// Optional description
    pub description: Option<String>,
    /// Key size in bytes
    pub size_bytes: u64,
}

/// Key storage manager
pub struct KeyManager {
    keys_dir: PathBuf,
}

impl KeyManager {
    /// Create a new key manager
    pub fn new() -> Result<Self> {
        let keys_dir = Self::keys_directory()?;

        // Create directory if it doesn't exist
        fs::create_dir_all(&keys_dir)
            .with_context(|| format!("Failed to create keys directory: {:?}", keys_dir))?;

        Ok(Self { keys_dir })
    }

    /// Get the keys directory path
    pub fn keys_directory() -> Result<PathBuf> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .context("Could not determine home directory")?;

        Ok(PathBuf::from(home).join(".amaters").join("keys"))
    }

    /// Generate a new FHE key pair
    pub fn generate(&self, name: &str, description: Option<String>) -> Result<KeyMetadata> {
        let key_path = self.key_path(name);
        let metadata_path = self.metadata_path(name);

        if key_path.exists() {
            anyhow::bail!("Key '{}' already exists", name);
        }

        // Generate FHE keys
        let keys = FheKeys::generate().context("Failed to generate FHE keys")?;

        // Save keys to file
        keys.save_to_file(&key_path)
            .context("Failed to save keys to file")?;

        // Get file size
        let size_bytes = match fs::metadata(&key_path) {
            Ok(metadata) => metadata.len(),
            Err(_) => 0,
        };

        // Create and save metadata
        let metadata = KeyMetadata {
            name: name.to_string(),
            created_at: chrono::Utc::now(),
            key_type: "client".to_string(),
            description,
            size_bytes,
        };

        self.save_metadata(&metadata)?;

        Ok(metadata)
    }

    /// Import keys from a file
    pub fn import(
        &self,
        name: &str,
        source_path: &Path,
        description: Option<String>,
    ) -> Result<KeyMetadata> {
        let key_path = self.key_path(name);
        let metadata_path = self.metadata_path(name);

        if key_path.exists() {
            anyhow::bail!("Key '{}' already exists", name);
        }

        // Validate that the source file exists and is readable
        if !source_path.exists() {
            anyhow::bail!("Source key file does not exist: {:?}", source_path);
        }

        // Try to load the keys to validate the format
        let _keys = FheKeys::load_from_file(source_path)
            .context("Failed to load keys from source file (invalid format?)")?;

        // Copy the key file
        fs::copy(source_path, &key_path)
            .with_context(|| format!("Failed to copy key file from {:?}", source_path))?;

        // Get file size
        let size_bytes = match fs::metadata(&key_path) {
            Ok(metadata) => metadata.len(),
            Err(_) => 0,
        };

        // Create and save metadata
        let metadata = KeyMetadata {
            name: name.to_string(),
            created_at: chrono::Utc::now(),
            key_type: "client".to_string(),
            description,
            size_bytes,
        };

        self.save_metadata(&metadata)?;

        Ok(metadata)
    }

    /// Export keys to a file
    pub fn export(&self, name: &str, dest_path: &Path) -> Result<()> {
        let key_path = self.key_path(name);

        if !key_path.exists() {
            anyhow::bail!("Key '{}' not found", name);
        }

        // Prevent overwriting
        if dest_path.exists() {
            anyhow::bail!("Destination file already exists: {:?}", dest_path);
        }

        // Copy the key file
        fs::copy(&key_path, dest_path)
            .with_context(|| format!("Failed to copy key file to {:?}", dest_path))?;

        Ok(())
    }

    /// List all available keys
    pub fn list(&self) -> Result<Vec<KeyMetadata>> {
        let mut keys = Vec::new();

        if !self.keys_dir.exists() {
            return Ok(keys);
        }

        for entry in fs::read_dir(&self.keys_dir)? {
            let entry = entry?;
            let path = entry.path();

            // Look for metadata files
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(metadata) = self.load_metadata_from_path(&path) {
                    keys.push(metadata);
                }
            }
        }

        // Sort by creation time (newest first)
        keys.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(keys)
    }

    /// Get metadata for a specific key
    pub fn get_metadata(&self, name: &str) -> Result<KeyMetadata> {
        let metadata_path = self.metadata_path(name);

        if !metadata_path.exists() {
            anyhow::bail!("Key '{}' not found", name);
        }

        self.load_metadata_from_path(&metadata_path)
    }

    /// Delete a key
    pub fn delete(&self, name: &str) -> Result<()> {
        let key_path = self.key_path(name);
        let metadata_path = self.metadata_path(name);

        if !key_path.exists() {
            anyhow::bail!("Key '{}' not found", name);
        }

        // Delete key file
        fs::remove_file(&key_path)
            .with_context(|| format!("Failed to delete key file: {:?}", key_path))?;

        // Delete metadata file
        if metadata_path.exists() {
            fs::remove_file(&metadata_path)
                .with_context(|| format!("Failed to delete metadata file: {:?}", metadata_path))?;
        }

        Ok(())
    }

    /// Load keys
    pub fn load(&self, name: &str) -> Result<FheKeys> {
        let key_path = self.key_path(name);

        if !key_path.exists() {
            anyhow::bail!("Key '{}' not found", name);
        }

        FheKeys::load_from_file(&key_path).context("Failed to load keys")
    }

    /// Get the path to a key file
    fn key_path(&self, name: &str) -> PathBuf {
        self.keys_dir.join(format!("{}.key", name))
    }

    /// Get the path to a metadata file
    fn metadata_path(&self, name: &str) -> PathBuf {
        self.keys_dir.join(format!("{}.json", name))
    }

    /// Save metadata to file
    fn save_metadata(&self, metadata: &KeyMetadata) -> Result<()> {
        let metadata_path = self.metadata_path(&metadata.name);
        let json =
            serde_json::to_string_pretty(metadata).context("Failed to serialize metadata")?;

        fs::write(&metadata_path, json)
            .with_context(|| format!("Failed to write metadata file: {:?}", metadata_path))?;

        Ok(())
    }

    /// Load metadata from file
    fn load_metadata_from_path(&self, path: &Path) -> Result<KeyMetadata> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read metadata file: {:?}", path))?;

        let metadata: KeyMetadata = serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse metadata file: {:?}", path))?;

        Ok(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_key_manager_creation() -> Result<()> {
        let temp_dir = env::temp_dir().join("amaters_cli_test_keys");

        // Clean up if exists
        let _ = fs::remove_dir_all(&temp_dir);

        // Should create directory
        let keys_dir = temp_dir.clone();
        fs::create_dir_all(&keys_dir)?;

        let manager = KeyManager { keys_dir };
        assert!(manager.keys_dir.exists());

        // Clean up
        fs::remove_dir_all(&temp_dir)?;

        Ok(())
    }

    #[test]
    fn test_key_metadata_serialization() -> Result<()> {
        let metadata = KeyMetadata {
            name: "test_key".to_string(),
            created_at: chrono::Utc::now(),
            key_type: "client".to_string(),
            description: Some("Test key".to_string()),
            size_bytes: 1024,
        };

        let json = serde_json::to_string(&metadata)?;
        let deserialized: KeyMetadata = serde_json::from_str(&json)?;

        assert_eq!(metadata.name, deserialized.name);
        assert_eq!(metadata.key_type, deserialized.key_type);
        assert_eq!(metadata.size_bytes, deserialized.size_bytes);

        Ok(())
    }

    #[test]
    fn test_list_empty_keys() -> Result<()> {
        let temp_dir = env::temp_dir().join("amaters_cli_test_empty");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir)?;

        let manager = KeyManager {
            keys_dir: temp_dir.clone(),
        };
        let keys = manager.list()?;

        assert_eq!(keys.len(), 0);

        fs::remove_dir_all(&temp_dir)?;
        Ok(())
    }
}
