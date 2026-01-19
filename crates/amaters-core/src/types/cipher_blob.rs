//! Encrypted data blob type for AmateRS
//!
//! CipherBlob represents encrypted data optimized for FHE operations.
//! It's designed to be zero-copy friendly and immutable.

use crate::error::{AmateRSError, ErrorContext, Result};
use std::sync::Arc;

/// Compression type for ciphertext
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionType {
    None,
    Lz4,
    Zstd,
}

/// Metadata for encrypted blob
#[derive(Debug, Clone)]
pub struct CipherMetadata {
    /// Uncompressed size in bytes
    pub size: usize,
    /// Compression algorithm used
    pub compression: CompressionType,
    /// CRC32 checksum for integrity
    pub checksum: u32,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Optional version tag
    pub version: Option<u32>,
}

impl CipherMetadata {
    /// Create new metadata
    pub fn new(size: usize) -> Self {
        Self {
            size,
            compression: CompressionType::None,
            checksum: 0,
            created_at: chrono::Utc::now(),
            version: None,
        }
    }

    /// Set compression type
    pub fn with_compression(mut self, compression: CompressionType) -> Self {
        self.compression = compression;
        self
    }

    /// Set checksum
    pub fn with_checksum(mut self, checksum: u32) -> Self {
        self.checksum = checksum;
        self
    }

    /// Set version
    pub fn with_version(mut self, version: u32) -> Self {
        self.version = Some(version);
        self
    }
}

/// Encrypted data blob - immutable, zero-copy friendly
///
/// CipherBlob wraps encrypted data in an Arc for cheap cloning and
/// includes metadata for integrity checking and compression.
#[derive(Clone, Debug)]
pub struct CipherBlob {
    data: Arc<Vec<u8>>,
    metadata: CipherMetadata,
}

impl CipherBlob {
    /// Maximum allowed ciphertext size (1GB)
    pub const MAX_SIZE: usize = 1024 * 1024 * 1024;

    /// Create a new cipher blob from raw bytes
    pub fn new(data: Vec<u8>) -> Self {
        let checksum = crc32fast::hash(&data);
        let metadata = CipherMetadata::new(data.len()).with_checksum(checksum);

        Self {
            data: Arc::new(data),
            metadata,
        }
    }

    /// Create from bytes with metadata
    pub fn with_metadata(data: Vec<u8>, metadata: CipherMetadata) -> Self {
        Self {
            data: Arc::new(data),
            metadata,
        }
    }

    /// Get reference to the raw encrypted data
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Get the size of the ciphertext
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Check if the blob is empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Get metadata
    pub fn metadata(&self) -> &CipherMetadata {
        &self.metadata
    }

    /// Verify integrity using checksum
    pub fn verify_integrity(&self) -> Result<()> {
        let computed = crc32fast::hash(&self.data);
        if computed == self.metadata.checksum {
            Ok(())
        } else {
            Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Checksum mismatch: expected {}, got {}",
                self.metadata.checksum, computed
            ))))
        }
    }

    /// Clone the underlying data (deep copy)
    pub fn to_vec(&self) -> Vec<u8> {
        (*self.data).clone()
    }

    /// Get the Arc-wrapped data for zero-copy sharing
    pub fn data_arc(&self) -> Arc<Vec<u8>> {
        Arc::clone(&self.data)
    }
}

impl PartialEq for CipherBlob {
    fn eq(&self, other: &Self) -> bool {
        self.data.as_slice() == other.data.as_slice()
    }
}

impl Eq for CipherBlob {}

impl From<Vec<u8>> for CipherBlob {
    fn from(data: Vec<u8>) -> Self {
        Self::new(data)
    }
}

impl From<&[u8]> for CipherBlob {
    fn from(data: &[u8]) -> Self {
        Self::new(data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cipher_blob_creation() -> Result<()> {
        let data = vec![1, 2, 3, 4, 5];
        let blob = CipherBlob::new(data.clone());

        assert_eq!(blob.len(), 5);
        assert_eq!(blob.as_bytes(), &data);
        assert!(!blob.is_empty());

        Ok(())
    }

    #[test]
    fn test_cipher_blob_integrity() -> Result<()> {
        let data = vec![1, 2, 3, 4, 5];
        let blob = CipherBlob::new(data);

        blob.verify_integrity()?;

        Ok(())
    }

    #[test]
    fn test_cipher_blob_clone() -> Result<()> {
        let blob1 = CipherBlob::new(vec![1, 2, 3]);
        let blob2 = blob1.clone();

        assert_eq!(blob1, blob2);
        assert!(Arc::ptr_eq(&blob1.data, &blob2.data)); // Same Arc

        Ok(())
    }

    #[test]
    fn test_cipher_metadata() -> Result<()> {
        let metadata = CipherMetadata::new(100)
            .with_compression(CompressionType::Lz4)
            .with_version(1);

        assert_eq!(metadata.size, 100);
        assert_eq!(metadata.compression, CompressionType::Lz4);
        assert_eq!(metadata.version, Some(1));

        Ok(())
    }
}
