//! FHE (Fully Homomorphic Encryption) key management and operations
//!
//! This module provides client-side encryption and decryption capabilities.
//! The actual FHE operations are feature-gated and require the `fhe` feature.

use crate::error::{Result, SdkError};
use amaters_core::{CipherBlob, Key};
use std::path::Path;

/// FHE client keys for encryption/decryption
///
/// When the `fhe` feature is enabled, this wraps TFHE client keys for real
/// homomorphic encryption. Without the feature, it acts as a passthrough stub.
#[derive(Clone)]
pub struct FheKeys {
    #[cfg(feature = "fhe")]
    _keys: tfhe::ClientKey,
    #[cfg(not(feature = "fhe"))]
    _placeholder: (),
}

impl FheKeys {
    /// Generate new FHE keys
    ///
    /// This is a computationally expensive operation (can take several seconds)
    /// when the `fhe` feature is enabled.
    pub fn generate() -> Result<Self> {
        #[cfg(feature = "fhe")]
        {
            let config = tfhe::ConfigBuilder::default().build();
            let client_key = tfhe::ClientKey::generate(config);
            Ok(Self { _keys: client_key })
        }
        #[cfg(not(feature = "fhe"))]
        {
            Ok(Self { _placeholder: () })
        }
    }

    /// Load keys from a file
    ///
    /// Reads serialized key data from the given path and deserializes
    /// using oxicode (when `fhe` + `serialization` features are enabled).
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self> {
        #[cfg(feature = "fhe")]
        {
            let bytes = std::fs::read(path.as_ref())
                .map_err(|e| SdkError::Fhe(format!("failed to read key file: {}", e)))?;
            #[cfg(feature = "serialization")]
            {
                let client_key: tfhe::ClientKey = oxicode::serde::decode_serde(&bytes)
                    .map_err(|e| SdkError::Fhe(format!("failed to deserialize keys: {}", e)))?;
                Ok(Self { _keys: client_key })
            }
            #[cfg(not(feature = "serialization"))]
            {
                let _ = bytes;
                Err(SdkError::Fhe(
                    "serialization feature required for key file loading".to_string(),
                ))
            }
        }
        #[cfg(not(feature = "fhe"))]
        {
            let _ = path;
            Ok(Self { _placeholder: () })
        }
    }

    /// Save keys to a file
    ///
    /// Serializes keys using oxicode and writes to the given path
    /// (when `fhe` + `serialization` features are enabled).
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        #[cfg(feature = "fhe")]
        {
            #[cfg(feature = "serialization")]
            {
                let bytes = oxicode::serde::encode_serde(&self._keys)
                    .map_err(|e| SdkError::Fhe(format!("failed to serialize keys: {}", e)))?;
                std::fs::write(path.as_ref(), &bytes)
                    .map_err(|e| SdkError::Fhe(format!("failed to write key file: {}", e)))?;
                Ok(())
            }
            #[cfg(not(feature = "serialization"))]
            {
                let _ = path;
                Err(SdkError::Fhe(
                    "serialization feature required for key file saving".to_string(),
                ))
            }
        }
        #[cfg(not(feature = "fhe"))]
        {
            let _ = path;
            Ok(())
        }
    }

    /// Serialize keys to bytes using oxicode
    #[cfg(feature = "serialization")]
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        #[cfg(feature = "fhe")]
        {
            oxicode::serde::encode_serde(&self._keys).map_err(|e| {
                SdkError::Serialization(format!("failed to serialize FHE keys: {}", e))
            })
        }
        #[cfg(not(feature = "fhe"))]
        {
            Ok(Vec::new())
        }
    }

    /// Deserialize keys from bytes using oxicode
    #[cfg(feature = "serialization")]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        #[cfg(feature = "fhe")]
        {
            let client_key: tfhe::ClientKey = oxicode::serde::decode_serde(bytes).map_err(|e| {
                SdkError::Serialization(format!("failed to deserialize FHE keys: {}", e))
            })?;
            Ok(Self { _keys: client_key })
        }
        #[cfg(not(feature = "fhe"))]
        {
            let _ = bytes;
            Ok(Self { _placeholder: () })
        }
    }
}

/// FHE encryptor for client-side encryption
pub struct FheEncryptor {
    keys: FheKeys,
}

impl FheEncryptor {
    /// Create a new encryptor with generated keys
    pub fn new() -> Result<Self> {
        Ok(Self {
            keys: FheKeys::generate()?,
        })
    }

    /// Create an encryptor with existing keys
    pub fn with_keys(keys: FheKeys) -> Self {
        Self { keys }
    }

    /// Get a reference to the keys
    pub fn keys(&self) -> &FheKeys {
        &self.keys
    }

    /// Encrypt a value
    ///
    /// When `fhe` is enabled, encrypts each byte using TFHE FheUint8 and
    /// serializes the resulting ciphertexts into a single CipherBlob.
    /// Without `fhe`, this is a passthrough that wraps plaintext as-is (NOT secure).
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<CipherBlob> {
        #[cfg(feature = "fhe")]
        {
            use tfhe::prelude::FheTryEncrypt;

            // Encrypt each byte as an FheUint8 and collect serialized ciphertexts
            let mut encrypted_parts: Vec<Vec<u8>> = Vec::with_capacity(plaintext.len());
            for &byte in plaintext {
                let encrypted: tfhe::FheUint8 = tfhe::FheUint8::try_encrypt(byte, &self.keys._keys)
                    .map_err(|e| SdkError::Fhe(format!("failed to encrypt byte: {}", e)))?;
                // Serialize each encrypted value
                #[cfg(feature = "serialization")]
                {
                    let serialized = oxicode::serde::encode_serde(&encrypted).map_err(|e| {
                        SdkError::Fhe(format!("failed to serialize encrypted byte: {}", e))
                    })?;
                    encrypted_parts.push(serialized);
                }
                #[cfg(not(feature = "serialization"))]
                {
                    let _ = encrypted;
                    return Err(SdkError::Fhe(
                        "serialization feature required for FHE encryption".to_string(),
                    ));
                }
            }
            // Pack: [count(u64)] [len1(u64)][data1] [len2(u64)][data2] ...
            let count = plaintext.len() as u64;
            let total_size = 8 + encrypted_parts.iter().map(|p| 8 + p.len()).sum::<usize>();
            let mut blob_data = Vec::with_capacity(total_size);
            blob_data.extend_from_slice(&count.to_le_bytes());
            for part in &encrypted_parts {
                let len = part.len() as u64;
                blob_data.extend_from_slice(&len.to_le_bytes());
                blob_data.extend_from_slice(part);
            }
            Ok(CipherBlob::new(blob_data))
        }
        #[cfg(not(feature = "fhe"))]
        {
            // For testing: just wrap the plaintext as-is
            // WARNING: This is NOT secure - only for development
            Ok(CipherBlob::new(plaintext.to_vec()))
        }
    }

    /// Decrypt a ciphertext
    ///
    /// When `fhe` is enabled, deserializes FheUint8 ciphertexts from the blob
    /// and decrypts each one. Without `fhe`, returns the raw blob data (NOT secure).
    pub fn decrypt(&self, ciphertext: &CipherBlob) -> Result<Vec<u8>> {
        #[cfg(feature = "fhe")]
        {
            use tfhe::prelude::FheDecrypt;

            let data = ciphertext.to_vec();
            if data.len() < 8 {
                return Err(SdkError::Fhe("ciphertext too short".to_string()));
            }
            let count = u64::from_le_bytes(
                data[..8]
                    .try_into()
                    .map_err(|_| SdkError::Fhe("invalid ciphertext header".to_string()))?,
            ) as usize;

            let mut offset = 8usize;
            let mut plaintext = Vec::with_capacity(count);

            for _ in 0..count {
                if offset + 8 > data.len() {
                    return Err(SdkError::Fhe(
                        "ciphertext truncated: missing length field".to_string(),
                    ));
                }
                let part_len = u64::from_le_bytes(
                    data[offset..offset + 8]
                        .try_into()
                        .map_err(|_| SdkError::Fhe("invalid ciphertext part length".to_string()))?,
                ) as usize;
                offset += 8;

                if offset + part_len > data.len() {
                    return Err(SdkError::Fhe(
                        "ciphertext truncated: insufficient data".to_string(),
                    ));
                }

                #[cfg(feature = "serialization")]
                {
                    let encrypted: tfhe::FheUint8 = oxicode::serde::decode_serde(
                        &data[offset..offset + part_len],
                    )
                    .map_err(|e| {
                        SdkError::Fhe(format!("failed to deserialize encrypted byte: {}", e))
                    })?;
                    let byte: u8 = encrypted.decrypt(&self.keys._keys);
                    plaintext.push(byte);
                }
                #[cfg(not(feature = "serialization"))]
                {
                    return Err(SdkError::Fhe(
                        "serialization feature required for FHE decryption".to_string(),
                    ));
                }

                offset += part_len;
            }

            Ok(plaintext)
        }
        #[cfg(not(feature = "fhe"))]
        {
            // For testing: just unwrap the data as-is
            // WARNING: This is NOT secure - only for development
            Ok(ciphertext.to_vec())
        }
    }

    /// Encrypt a key
    pub fn encrypt_key(&self, key: &Key) -> Result<CipherBlob> {
        #[cfg(feature = "fhe")]
        {
            self.encrypt(key.as_bytes())
        }
        #[cfg(not(feature = "fhe"))]
        {
            // For testing: just wrap the key bytes
            Ok(CipherBlob::new(key.to_vec()))
        }
    }

    /// Batch encrypt multiple values
    pub fn encrypt_batch(&self, plaintexts: &[&[u8]]) -> Result<Vec<CipherBlob>> {
        plaintexts.iter().map(|p| self.encrypt(p)).collect()
    }
}

impl Default for FheEncryptor {
    fn default() -> Self {
        Self::new().expect("failed to create default encryptor")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fhe_keys_generate_no_fhe() {
        #[cfg(not(feature = "fhe"))]
        {
            let keys = FheKeys::generate().expect("generate keys should succeed");
            // Verify save_to_file works (no-op in stub mode)
            let dir = std::env::temp_dir();
            let path = dir.join("test_fhe_keys_generate");
            keys.save_to_file(&path)
                .expect("save_to_file should succeed in stub mode");
        }
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip_no_fhe() {
        #[cfg(not(feature = "fhe"))]
        {
            let encryptor = FheEncryptor::new().expect("create encryptor");
            let plaintext = b"hello world roundtrip test";
            let ciphertext = encryptor.encrypt(plaintext).expect("encrypt");
            let decrypted = encryptor.decrypt(&ciphertext).expect("decrypt");

            // In stub mode, it should be identity
            assert_eq!(decrypted, plaintext);
        }
    }

    #[test]
    fn test_file_save_load_roundtrip_no_fhe() {
        #[cfg(not(feature = "fhe"))]
        {
            let dir = std::env::temp_dir();
            let path = dir.join("test_fhe_keys_save_load");

            let keys = FheKeys::generate().expect("generate keys");
            keys.save_to_file(&path).expect("save keys");

            let _loaded = FheKeys::load_from_file(&path).expect("load keys");
            // In stub mode, both are placeholder values, so just verify no error
        }
    }

    #[cfg(feature = "serialization")]
    #[test]
    fn test_serialization_roundtrip_no_fhe() {
        #[cfg(not(feature = "fhe"))]
        {
            let keys = FheKeys::generate().expect("generate keys");
            let bytes = keys.to_bytes().expect("serialize keys");
            let _restored = FheKeys::from_bytes(&bytes).expect("deserialize keys");
            // In stub mode, to_bytes returns empty vec, from_bytes accepts anything
        }
    }

    #[test]
    fn test_batch_encrypt_no_fhe() {
        #[cfg(not(feature = "fhe"))]
        {
            let encryptor = FheEncryptor::new().expect("create encryptor");
            let data: Vec<&[u8]> = vec![b"one", b"two", b"three"];

            let encrypted = encryptor.encrypt_batch(&data).expect("batch encrypt");
            assert_eq!(encrypted.len(), 3);

            // Verify each can be decrypted back
            for (i, ct) in encrypted.iter().enumerate() {
                let decrypted = encryptor.decrypt(ct).expect("decrypt");
                assert_eq!(decrypted, data[i]);
            }
        }
    }

    #[test]
    fn test_encrypt_key_no_fhe() {
        #[cfg(not(feature = "fhe"))]
        {
            let encryptor = FheEncryptor::new().expect("create encryptor");
            let key = Key::new(b"test-key-data".to_vec());
            let cipher = encryptor.encrypt_key(&key).expect("encrypt key");
            let decrypted = encryptor.decrypt(&cipher).expect("decrypt");
            assert_eq!(decrypted, key.as_bytes());
        }
    }

    #[test]
    fn test_encryptor_with_keys() {
        #[cfg(not(feature = "fhe"))]
        {
            let keys = FheKeys::generate().expect("generate keys");
            let encryptor = FheEncryptor::with_keys(keys);
            let _keys_ref = encryptor.keys();
            let plaintext = b"test with_keys";
            let ciphertext = encryptor.encrypt(plaintext).expect("encrypt");
            let decrypted = encryptor.decrypt(&ciphertext).expect("decrypt");
            assert_eq!(decrypted, plaintext);
        }
    }

    #[test]
    fn test_empty_plaintext_no_fhe() {
        #[cfg(not(feature = "fhe"))]
        {
            let encryptor = FheEncryptor::new().expect("create encryptor");
            let plaintext = b"";
            let ciphertext = encryptor.encrypt(plaintext).expect("encrypt empty");
            let decrypted = encryptor.decrypt(&ciphertext).expect("decrypt empty");
            assert_eq!(decrypted, plaintext);
        }
    }
}
