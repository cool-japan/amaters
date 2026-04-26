//! FHE operations module
//!
//! This module provides encrypted data types and operations for TFHE-based computations.
//! All operations are performed on encrypted data without decryption.

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::CipherBlob;

#[cfg(feature = "compute")]
use tfhe::prelude::*;
#[cfg(feature = "compute")]
use tfhe::{FheBool, FheUint8, FheUint16, FheUint32, FheUint64};

/// Encrypted boolean value
#[cfg(feature = "compute")]
#[derive(Clone)]
pub struct EncryptedBool {
    inner: FheBool,
}

#[cfg(feature = "compute")]
impl EncryptedBool {
    /// Create from TFHE FheBool
    pub fn from_fhe(value: FheBool) -> Self {
        Self { inner: value }
    }

    /// Get inner FheBool
    pub fn inner(&self) -> &FheBool {
        &self.inner
    }

    /// Encrypt a boolean value
    pub fn encrypt(value: bool, client_key: &tfhe::ClientKey) -> Self {
        Self {
            inner: FheBool::encrypt(value, client_key),
        }
    }

    /// Decrypt to boolean
    pub fn decrypt(&self, client_key: &tfhe::ClientKey) -> bool {
        self.inner.decrypt(client_key)
    }

    /// Serialize to CipherBlob
    pub fn to_cipher_blob(&self) -> Result<CipherBlob> {
        let bytes = oxicode::serde::encode_serde(&self.inner).map_err(|e| {
            AmateRSError::Serialization(ErrorContext::new(format!(
                "Failed to serialize EncryptedBool: {}",
                e
            )))
        })?;
        Ok(CipherBlob::new(bytes))
    }

    /// Deserialize from CipherBlob
    pub fn from_cipher_blob(blob: &CipherBlob) -> Result<Self> {
        let inner: FheBool = oxicode::serde::decode_serde(blob.as_bytes()).map_err(|e| {
            AmateRSError::Deserialization(ErrorContext::new(format!(
                "Failed to deserialize EncryptedBool: {}",
                e
            )))
        })?;
        Ok(Self { inner })
    }

    /// Logical AND operation
    pub fn and(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner & &other.inner,
        }
    }

    /// Logical OR operation
    pub fn or(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner | &other.inner,
        }
    }

    /// Logical XOR operation
    pub fn xor(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner ^ &other.inner,
        }
    }

    /// Logical NOT operation
    pub fn not(&self) -> Self {
        Self {
            inner: !&self.inner,
        }
    }
}

/// Encrypted unsigned 8-bit integer
#[cfg(feature = "compute")]
#[derive(Clone)]
pub struct EncryptedU8 {
    inner: FheUint8,
}

#[cfg(feature = "compute")]
impl EncryptedU8 {
    /// Create from TFHE FheUint8
    pub fn from_fhe(value: FheUint8) -> Self {
        Self { inner: value }
    }

    /// Get inner FheUint8
    pub fn inner(&self) -> &FheUint8 {
        &self.inner
    }

    /// Encrypt a u8 value
    pub fn encrypt(value: u8, client_key: &tfhe::ClientKey) -> Self {
        Self {
            inner: FheUint8::encrypt(value, client_key),
        }
    }

    /// Decrypt to u8
    pub fn decrypt(&self, client_key: &tfhe::ClientKey) -> u8 {
        self.inner.decrypt(client_key)
    }

    /// Serialize to CipherBlob
    pub fn to_cipher_blob(&self) -> Result<CipherBlob> {
        let bytes = oxicode::serde::encode_serde(&self.inner).map_err(|e| {
            AmateRSError::Serialization(ErrorContext::new(format!(
                "Failed to serialize EncryptedU8: {}",
                e
            )))
        })?;
        Ok(CipherBlob::new(bytes))
    }

    /// Deserialize from CipherBlob
    pub fn from_cipher_blob(blob: &CipherBlob) -> Result<Self> {
        let inner: FheUint8 = oxicode::serde::decode_serde(blob.as_bytes()).map_err(|e| {
            AmateRSError::Deserialization(ErrorContext::new(format!(
                "Failed to deserialize EncryptedU8: {}",
                e
            )))
        })?;
        Ok(Self { inner })
    }

    /// Add two encrypted values
    pub fn add(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner + &other.inner,
        }
    }

    /// Subtract two encrypted values
    pub fn sub(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner - &other.inner,
        }
    }

    /// Multiply two encrypted values
    pub fn mul(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner * &other.inner,
        }
    }

    /// Compare equality
    pub fn eq(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.eq(&other.inner),
        }
    }

    /// Compare less than
    pub fn lt(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.lt(&other.inner),
        }
    }

    /// Compare less than or equal
    pub fn le(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.le(&other.inner),
        }
    }

    /// Compare greater than
    pub fn gt(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.gt(&other.inner),
        }
    }

    /// Compare greater than or equal
    pub fn ge(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.ge(&other.inner),
        }
    }

    /// Compare not equal
    pub fn ne(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.ne(&other.inner),
        }
    }
}

/// Encrypted unsigned 16-bit integer
#[cfg(feature = "compute")]
#[derive(Clone)]
pub struct EncryptedU16 {
    inner: FheUint16,
}

#[cfg(feature = "compute")]
impl EncryptedU16 {
    pub fn from_fhe(value: FheUint16) -> Self {
        Self { inner: value }
    }

    pub fn inner(&self) -> &FheUint16 {
        &self.inner
    }

    pub fn encrypt(value: u16, client_key: &tfhe::ClientKey) -> Self {
        Self {
            inner: FheUint16::encrypt(value, client_key),
        }
    }

    pub fn decrypt(&self, client_key: &tfhe::ClientKey) -> u16 {
        self.inner.decrypt(client_key)
    }

    pub fn to_cipher_blob(&self) -> Result<CipherBlob> {
        let bytes = oxicode::serde::encode_serde(&self.inner).map_err(|e| {
            AmateRSError::Serialization(ErrorContext::new(format!(
                "Failed to serialize EncryptedU16: {}",
                e
            )))
        })?;
        Ok(CipherBlob::new(bytes))
    }

    pub fn from_cipher_blob(blob: &CipherBlob) -> Result<Self> {
        let inner: FheUint16 = oxicode::serde::decode_serde(blob.as_bytes()).map_err(|e| {
            AmateRSError::Deserialization(ErrorContext::new(format!(
                "Failed to deserialize EncryptedU16: {}",
                e
            )))
        })?;
        Ok(Self { inner })
    }

    pub fn add(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner + &other.inner,
        }
    }

    pub fn sub(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner - &other.inner,
        }
    }

    pub fn mul(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner * &other.inner,
        }
    }

    pub fn eq(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.eq(&other.inner),
        }
    }

    pub fn lt(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.lt(&other.inner),
        }
    }

    pub fn le(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.le(&other.inner),
        }
    }

    pub fn gt(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.gt(&other.inner),
        }
    }

    pub fn ge(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.ge(&other.inner),
        }
    }

    pub fn ne(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.ne(&other.inner),
        }
    }
}

/// Encrypted unsigned 32-bit integer
#[cfg(feature = "compute")]
#[derive(Clone)]
pub struct EncryptedU32 {
    inner: FheUint32,
}

#[cfg(feature = "compute")]
impl EncryptedU32 {
    pub fn from_fhe(value: FheUint32) -> Self {
        Self { inner: value }
    }

    pub fn inner(&self) -> &FheUint32 {
        &self.inner
    }

    pub fn encrypt(value: u32, client_key: &tfhe::ClientKey) -> Self {
        Self {
            inner: FheUint32::encrypt(value, client_key),
        }
    }

    pub fn decrypt(&self, client_key: &tfhe::ClientKey) -> u32 {
        self.inner.decrypt(client_key)
    }

    pub fn to_cipher_blob(&self) -> Result<CipherBlob> {
        let bytes = oxicode::serde::encode_serde(&self.inner).map_err(|e| {
            AmateRSError::Serialization(ErrorContext::new(format!(
                "Failed to serialize EncryptedU32: {}",
                e
            )))
        })?;
        Ok(CipherBlob::new(bytes))
    }

    pub fn from_cipher_blob(blob: &CipherBlob) -> Result<Self> {
        let inner: FheUint32 = oxicode::serde::decode_serde(blob.as_bytes()).map_err(|e| {
            AmateRSError::Deserialization(ErrorContext::new(format!(
                "Failed to deserialize EncryptedU32: {}",
                e
            )))
        })?;
        Ok(Self { inner })
    }

    pub fn add(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner + &other.inner,
        }
    }

    pub fn sub(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner - &other.inner,
        }
    }

    pub fn mul(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner * &other.inner,
        }
    }

    pub fn eq(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.eq(&other.inner),
        }
    }

    pub fn lt(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.lt(&other.inner),
        }
    }

    pub fn le(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.le(&other.inner),
        }
    }

    pub fn gt(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.gt(&other.inner),
        }
    }

    pub fn ge(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.ge(&other.inner),
        }
    }

    pub fn ne(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.ne(&other.inner),
        }
    }
}

/// Encrypted unsigned 64-bit integer
#[cfg(feature = "compute")]
#[derive(Clone)]
pub struct EncryptedU64 {
    inner: FheUint64,
}

#[cfg(feature = "compute")]
impl EncryptedU64 {
    pub fn from_fhe(value: FheUint64) -> Self {
        Self { inner: value }
    }

    pub fn inner(&self) -> &FheUint64 {
        &self.inner
    }

    pub fn encrypt(value: u64, client_key: &tfhe::ClientKey) -> Self {
        Self {
            inner: FheUint64::encrypt(value, client_key),
        }
    }

    pub fn decrypt(&self, client_key: &tfhe::ClientKey) -> u64 {
        self.inner.decrypt(client_key)
    }

    pub fn to_cipher_blob(&self) -> Result<CipherBlob> {
        let bytes = oxicode::serde::encode_serde(&self.inner).map_err(|e| {
            AmateRSError::Serialization(ErrorContext::new(format!(
                "Failed to serialize EncryptedU64: {}",
                e
            )))
        })?;
        Ok(CipherBlob::new(bytes))
    }

    pub fn from_cipher_blob(blob: &CipherBlob) -> Result<Self> {
        let inner: FheUint64 = oxicode::serde::decode_serde(blob.as_bytes()).map_err(|e| {
            AmateRSError::Deserialization(ErrorContext::new(format!(
                "Failed to deserialize EncryptedU64: {}",
                e
            )))
        })?;
        Ok(Self { inner })
    }

    pub fn add(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner + &other.inner,
        }
    }

    pub fn sub(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner - &other.inner,
        }
    }

    pub fn mul(&self, other: &Self) -> Self {
        Self {
            inner: &self.inner * &other.inner,
        }
    }

    pub fn eq(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.eq(&other.inner),
        }
    }

    pub fn lt(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.lt(&other.inner),
        }
    }

    pub fn le(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.le(&other.inner),
        }
    }

    pub fn gt(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.gt(&other.inner),
        }
    }

    pub fn ge(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.ge(&other.inner),
        }
    }

    pub fn ne(&self, other: &Self) -> EncryptedBool {
        EncryptedBool {
            inner: self.inner.ne(&other.inner),
        }
    }
}

/// Stub implementations when compute feature is disabled
#[cfg(not(feature = "compute"))]
#[derive(Clone, Debug)]
pub struct EncryptedBool {
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(not(feature = "compute"))]
impl EncryptedBool {
    pub fn to_cipher_blob(&self) -> Result<CipherBlob> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }

    pub fn from_cipher_blob(_blob: &CipherBlob) -> Result<Self> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }
}

#[cfg(not(feature = "compute"))]
#[derive(Clone, Debug)]
pub struct EncryptedU8 {
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(not(feature = "compute"))]
impl EncryptedU8 {
    pub fn to_cipher_blob(&self) -> Result<CipherBlob> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }

    pub fn from_cipher_blob(_blob: &CipherBlob) -> Result<Self> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }
}

#[cfg(not(feature = "compute"))]
#[derive(Clone, Debug)]
pub struct EncryptedU16 {
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(not(feature = "compute"))]
impl EncryptedU16 {
    pub fn to_cipher_blob(&self) -> Result<CipherBlob> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }

    pub fn from_cipher_blob(_blob: &CipherBlob) -> Result<Self> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }
}

#[cfg(not(feature = "compute"))]
#[derive(Clone, Debug)]
pub struct EncryptedU32 {
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(not(feature = "compute"))]
impl EncryptedU32 {
    pub fn to_cipher_blob(&self) -> Result<CipherBlob> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }

    pub fn from_cipher_blob(_blob: &CipherBlob) -> Result<Self> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }
}

#[cfg(not(feature = "compute"))]
#[derive(Clone, Debug)]
pub struct EncryptedU64 {
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(not(feature = "compute"))]
impl EncryptedU64 {
    pub fn to_cipher_blob(&self) -> Result<CipherBlob> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }

    pub fn from_cipher_blob(_blob: &CipherBlob) -> Result<Self> {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }
}

#[cfg(all(test, feature = "compute"))]
mod tests {
    use super::*;
    use crate::compute::keys::FheKeyPair;

    #[test]
    fn test_encrypted_bool_operations() -> Result<()> {
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        let a = EncryptedBool::encrypt(true, keypair.client_key());
        let b = EncryptedBool::encrypt(false, keypair.client_key());

        // Test AND
        let result = a.and(&b);
        assert!(!result.decrypt(keypair.client_key()));

        // Test OR
        let result = a.or(&b);
        assert!(result.decrypt(keypair.client_key()));

        // Test XOR
        let result = a.xor(&b);
        assert!(result.decrypt(keypair.client_key()));

        // Test NOT
        let result = a.not();
        assert!(!result.decrypt(keypair.client_key()));

        Ok(())
    }

    #[test]
    fn test_encrypted_u8_arithmetic() -> Result<()> {
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        let a = EncryptedU8::encrypt(5, keypair.client_key());
        let b = EncryptedU8::encrypt(3, keypair.client_key());

        // Test addition
        let result = a.add(&b);
        assert_eq!(result.decrypt(keypair.client_key()), 8);

        // Test subtraction
        let result = a.sub(&b);
        assert_eq!(result.decrypt(keypair.client_key()), 2);

        // Test multiplication
        let result = a.mul(&b);
        assert_eq!(result.decrypt(keypair.client_key()), 15);

        Ok(())
    }

    #[test]
    fn test_encrypted_u8_comparison() -> Result<()> {
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        let a = EncryptedU8::encrypt(5, keypair.client_key());
        let b = EncryptedU8::encrypt(3, keypair.client_key());

        // Test equality
        let result = a.eq(&b);
        assert!(!result.decrypt(keypair.client_key()));

        // Test less than
        let result = a.lt(&b);
        assert!(!result.decrypt(keypair.client_key()));

        // Test greater than
        let result = a.gt(&b);
        assert!(result.decrypt(keypair.client_key()));

        // Test less than or equal
        let result = a.le(&b);
        assert!(!result.decrypt(keypair.client_key()));

        // Test greater than or equal
        let result = a.ge(&b);
        assert!(result.decrypt(keypair.client_key()));

        // Test not equal
        let result = a.ne(&b);
        assert!(result.decrypt(keypair.client_key()));

        Ok(())
    }

    #[test]
    fn test_encrypted_value_serialization() -> Result<()> {
        let keypair = FheKeyPair::generate()?;
        keypair.set_as_global_server_key();

        let value = EncryptedU8::encrypt(42, keypair.client_key());
        let blob = value.to_cipher_blob()?;

        assert!(!blob.is_empty());

        let restored = EncryptedU8::from_cipher_blob(&blob)?;
        assert_eq!(restored.decrypt(keypair.client_key()), 42);

        Ok(())
    }
}
