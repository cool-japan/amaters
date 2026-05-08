//! FHE key management for amaters-cli
//!
//! Provides commands for generating, importing, exporting, and managing FHE keys.
//! Keys are stored in `~/.amaters/keys/` directory.

use crate::config::Config;
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
        keys.sort_by_key(|k| std::cmp::Reverse(k.created_at));

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

// ---------------------------------------------------------------------------
// Default key management
// ---------------------------------------------------------------------------

/// Handle the `key default` subcommand.
///
/// Behaviour:
/// - `name = Some(n)`: sets `config.default_key = Some(n)` and persists.
/// - `clear = true`: unsets `config.default_key` and persists.
/// - `show = true` (or no flags): prints the current default key.
pub fn handle_key_default(
    config: &mut Config,
    config_path: &Path,
    name: Option<String>,
    clear: bool,
    show: bool,
) -> Result<()> {
    if let Some(key_name) = name {
        config.default_key = Some(key_name.clone());
        config.save_atomic_to(config_path)?;
        println!("Default key set to '{}'", key_name);
    } else if clear {
        config.default_key = None;
        config.save_atomic_to(config_path)?;
        println!("Default key cleared");
    } else if show {
        match &config.default_key {
            Some(k) => println!("{}", k),
            None => println!("(none)"),
        }
    } else {
        // No flags: treat as implicit --show
        match &config.default_key {
            Some(k) => println!("{}", k),
            None => println!("(none)"),
        }
    }
    Ok(())
}

/// Resolve the effective FHE key name.
///
/// Priority:
/// 1. `explicit` — provided via `--key <name>` flag.
/// 2. `config.default_key` — set via `key default <name>`.
/// 3. Error — no key specified and no default configured.
pub fn resolve_key_name(explicit: Option<&str>, config: &Config) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name.to_string());
    }
    if let Some(name) = &config.default_key {
        return Ok(name.clone());
    }
    anyhow::bail!(
        "No key specified and no default set. \
         Use --key <name> or run 'key default <name>'."
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Item 4: Default key tests
    // -----------------------------------------------------------------------

    fn make_temp_config(suffix: &str) -> (Config, std::path::PathBuf) {
        let dir = env::temp_dir().join(format!("amaters_defkey_{}", suffix));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("config.toml");
        let config = Config::default();
        config.save_to(&path).expect("save default config");
        (config, path)
    }

    #[test]
    fn test_default_key_set_persists_to_config() -> Result<()> {
        let (mut config, path) = make_temp_config("set_persists");

        handle_key_default(&mut config, &path, Some("my-key".to_string()), false, false)?;

        // Reload from disk and verify
        let loaded = Config::load_from(&path)?;
        assert_eq!(loaded.default_key, Some("my-key".to_string()));

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
        Ok(())
    }

    #[test]
    fn test_default_key_clear_removes_setting() -> Result<()> {
        let (mut config, path) = make_temp_config("clear_removes");

        // First set a default key
        handle_key_default(&mut config, &path, Some("my-key".to_string()), false, false)?;
        let loaded = Config::load_from(&path)?;
        assert_eq!(loaded.default_key, Some("my-key".to_string()));

        // Now clear it
        let mut config2 = loaded;
        handle_key_default(&mut config2, &path, None, true, false)?;

        let loaded2 = Config::load_from(&path)?;
        assert_eq!(loaded2.default_key, None);

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
        Ok(())
    }

    #[test]
    fn test_default_key_show_displays_current() -> Result<()> {
        let (mut config, path) = make_temp_config("show_displays");
        config.default_key = Some("visible-key".to_string());
        config.save_to(&path)?;

        // show=true should succeed without error
        handle_key_default(&mut config, &path, None, false, true)?;

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
        Ok(())
    }

    #[test]
    fn test_default_key_show_none_when_unset() -> Result<()> {
        let (mut config, path) = make_temp_config("show_none");
        // default_key is None by default

        handle_key_default(&mut config, &path, None, false, true)?;

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
        Ok(())
    }

    #[test]
    fn test_default_key_used_when_flag_absent() -> Result<()> {
        let config = Config {
            default_key: Some("default-fhe-key".to_string()),
            ..Config::default()
        };

        let resolved = resolve_key_name(None, &config)?;
        assert_eq!(resolved, "default-fhe-key");
        Ok(())
    }

    #[test]
    fn test_explicit_flag_overrides_default() -> Result<()> {
        let config = Config {
            default_key: Some("default-key".to_string()),
            ..Config::default()
        };

        let resolved = resolve_key_name(Some("explicit-key"), &config)?;
        assert_eq!(resolved, "explicit-key");
        Ok(())
    }

    #[test]
    fn test_no_default_no_flag_errors_helpfully() {
        let config = Config::default(); // default_key is None
        let result = resolve_key_name(None, &config);
        assert!(result.is_err());
        let msg = result.expect_err("should be error").to_string();
        assert!(
            msg.contains("No key specified"),
            "Error message should mention 'No key specified', got: {msg}"
        );
        assert!(
            msg.contains("key default"),
            "Error message should hint about 'key default', got: {msg}"
        );
    }

    #[test]
    fn test_default_key_atomic_write_no_partial_file() -> Result<()> {
        let (mut config, path) = make_temp_config("atomic_write");

        handle_key_default(&mut config, &path, Some("atomic-key".to_string()), false, false)?;

        // The .tmp file must be gone after successful write.
        let tmp_path = path.with_extension("toml.tmp");
        assert!(
            !tmp_path.exists(),
            ".tmp file should not exist after atomic write"
        );

        // The actual config file must exist and have the key.
        assert!(path.exists(), "config file should exist");
        let loaded = Config::load_from(&path)?;
        assert_eq!(loaded.default_key, Some("atomic-key".to_string()));

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
        Ok(())
    }
}
