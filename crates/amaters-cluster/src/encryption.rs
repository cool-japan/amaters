//! AES-256-GCM encryption and HMAC-SHA256 integrity for Raft log payloads.
//!
//! This module provides per-entry encryption using HKDF-derived keys and nonces,
//! plus HMAC-based integrity verification for the encrypted log chain.
//!
//! ## Design
//!
//! - Each log entry's AES-256-GCM key **and** nonce are deterministically derived from
//!   the master key and the entry index via HKDF-SHA256, so no nonce reuse is possible
//!   within a key epoch.
//! - HMAC-SHA256 is computed over `entry_index_le || nonce || ciphertext` to provide
//!   additional chain integrity beyond what GCM authentication already gives.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

// Bring KeyInit into scope explicitly so disambiguating `<HmacSha256 as KeyInit>::new_from_slice`
// is not needed at every call site.  We re-alias it to avoid shadowing `hmac::Mac`.

use crate::error::{RaftError, RaftResult};

type HmacSha256 = Hmac<Sha256>;

// ──────────────────────────────────────────────
// LogEncryptionKey
// ──────────────────────────────────────────────

/// A 32-byte master key used to derive per-entry AES-256-GCM keys and nonces.
pub struct LogEncryptionKey {
    key_bytes: [u8; 32],
}

impl LogEncryptionKey {
    /// Create a [`LogEncryptionKey`] from a raw 32-byte array.
    pub fn new(key_bytes: [u8; 32]) -> Self {
        Self { key_bytes }
    }

    /// Create a [`LogEncryptionKey`] from a byte slice.
    ///
    /// # Errors
    /// Returns [`RaftError::StorageError`] when `bytes.len() != 32`.
    pub fn from_slice(bytes: &[u8]) -> RaftResult<Self> {
        let key_bytes: [u8; 32] = bytes.try_into().map_err(|_| RaftError::StorageError {
            message: format!(
                "LogEncryptionKey requires exactly 32 bytes, got {}",
                bytes.len()
            ),
        })?;
        Ok(Self { key_bytes })
    }

    /// Generate a random [`LogEncryptionKey`] without an external RNG crate.
    ///
    /// Entropy comes from four independent `std::collections::hash_map::RandomState`
    /// instances (each OS-seeded) mixed with the current nanosecond timestamp,
    /// then stretched to 32 bytes via HKDF-SHA256.
    pub fn random() -> Self {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        use std::time::{SystemTime, UNIX_EPOCH};

        let ts_nanos: u128 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0u128);

        // Four independently OS-seeded instances give us independent hash states.
        let rs1 = RandomState::new();
        let rs2 = RandomState::new();
        let rs3 = RandomState::new();
        let rs4 = RandomState::new();

        let h1: u64 = {
            let mut h = rs1.build_hasher();
            h.write_u128(ts_nanos);
            h.finish()
        };
        let h2: u64 = {
            let mut h = rs2.build_hasher();
            // XOR with a large constant to decorrelate from h1
            h.write_u128(ts_nanos ^ 0xcafe_babe_dead_beef_1234_5678_abcd_ef01_u128);
            h.finish()
        };
        let h3: u64 = {
            let mut h = rs3.build_hasher();
            h.write_u64(h1);
            h.write_u64(h2);
            h.finish()
        };
        let h4: u64 = {
            let mut h = rs4.build_hasher();
            h.write_u64(h2 ^ h3);
            h.write_u128(ts_nanos.wrapping_add(0x9e37_79b9_7f4a_7c15_f39c_c060_5c0e_d609_u128));
            h.finish()
        };

        // Assemble 32-byte IKM from the four hash outputs.
        let mut ikm = [0u8; 32];
        ikm[0..8].copy_from_slice(&h1.to_le_bytes());
        ikm[8..16].copy_from_slice(&h2.to_le_bytes());
        ikm[16..24].copy_from_slice(&h3.to_le_bytes());
        ikm[24..32].copy_from_slice(&h4.to_le_bytes());

        let salt = b"amaters-log-encryption-key-v1";
        let hk = Hkdf::<Sha256>::new(Some(salt), &ikm);
        let mut key_bytes = [0u8; 32];
        // HKDF expand for 32 bytes of output with SHA-256 can never exceed the limit.
        hk.expand(b"master-key", &mut key_bytes)
            .expect("HKDF expand for 32 bytes cannot fail");

        Self { key_bytes }
    }
}

// ──────────────────────────────────────────────
// EncryptedPayload
// ──────────────────────────────────────────────

/// The encrypted form of a single Raft log entry payload.
#[derive(Debug, Clone)]
pub struct EncryptedPayload {
    /// Ciphertext produced by AES-256-GCM, including the 16-byte authentication tag.
    pub ciphertext: Vec<u8>,
    /// The 12-byte nonce used during encryption (derived from master key + entry index).
    pub nonce: [u8; 12],
}

// ──────────────────────────────────────────────
// EntryEncryptor
// ──────────────────────────────────────────────

/// Encrypts and decrypts Raft log entry payloads using AES-256-GCM.
///
/// The AES key **and** nonce for each entry are deterministically derived from
/// the master key and the entry index via HKDF-SHA256, ensuring unique key material
/// per entry without the need for a random nonce.
pub struct EntryEncryptor {
    master_key: LogEncryptionKey,
}

impl EntryEncryptor {
    /// Create a new [`EntryEncryptor`] backed by `key`.
    pub fn new(key: LogEncryptionKey) -> Self {
        Self { master_key: key }
    }

    /// Derive the per-entry AES-256-GCM key (32 bytes) and nonce (12 bytes).
    fn derive_key_and_nonce(&self, entry_index: u64) -> RaftResult<([u8; 32], [u8; 12])> {
        let hk = Hkdf::<Sha256>::new(None, &self.master_key.key_bytes);
        let mut derived = [0u8; 44]; // 32 bytes key + 12 bytes nonce
        hk.expand(&entry_index.to_le_bytes(), &mut derived)
            .map_err(|e| RaftError::StorageError {
                message: format!("HKDF expand failed for entry {entry_index}: {e}"),
            })?;

        let mut key = [0u8; 32];
        let mut nonce = [0u8; 12];
        key.copy_from_slice(&derived[..32]);
        nonce.copy_from_slice(&derived[32..44]);
        Ok((key, nonce))
    }

    /// Encrypt `plaintext` associated with `entry_index`.
    ///
    /// The returned [`EncryptedPayload`] contains the GCM ciphertext (with auth tag)
    /// and the nonce that was used.
    ///
    /// # Errors
    /// Returns [`RaftError::StorageError`] on any cryptographic failure.
    pub fn encrypt(&self, entry_index: u64, plaintext: &[u8]) -> RaftResult<EncryptedPayload> {
        let (key_bytes, nonce_bytes) = self.derive_key_and_nonce(entry_index)?;

        let key = Key::<Aes256Gcm>::from(key_bytes);
        let cipher = Aes256Gcm::new(&key);
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext =
            cipher
                .encrypt(&nonce, plaintext)
                .map_err(|e| RaftError::StorageError {
                    message: format!("AES-256-GCM encryption failed for entry {entry_index}: {e}"),
                })?;

        Ok(EncryptedPayload {
            ciphertext,
            nonce: nonce_bytes,
        })
    }

    /// Decrypt `payload` associated with `entry_index`.
    ///
    /// The AES key is re-derived from the master key and `entry_index`.
    /// The nonce stored in the payload is used for decryption.
    ///
    /// # Errors
    /// Returns [`RaftError::StorageError`] on key derivation failure or GCM
    /// authentication failure (including tampered ciphertext).
    pub fn decrypt(&self, entry_index: u64, payload: &EncryptedPayload) -> RaftResult<Vec<u8>> {
        let (key_bytes, _derived_nonce) = self.derive_key_and_nonce(entry_index)?;

        let key = Key::<Aes256Gcm>::from(key_bytes);
        let cipher = Aes256Gcm::new(&key);
        let nonce = Nonce::from(payload.nonce);

        cipher
            .decrypt(&nonce, payload.ciphertext.as_ref())
            .map_err(|e| RaftError::StorageError {
                message: format!("AES-256-GCM decryption failed for entry {entry_index}: {e}"),
            })
    }
}

// ──────────────────────────────────────────────
// LogIntegrityVerifier
// ──────────────────────────────────────────────

/// HMAC-SHA256 integrity verifier for encrypted Raft log entries.
///
/// Computes and verifies HMAC-SHA256 over `entry_index_le || nonce || ciphertext`,
/// providing additional chain integrity on top of GCM authentication.
pub struct LogIntegrityVerifier {
    key: [u8; 32],
}

impl LogIntegrityVerifier {
    /// Create a new [`LogIntegrityVerifier`] with a 32-byte HMAC key.
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Compute HMAC-SHA256 over `entry_index_le || nonce || ciphertext`.
    pub fn compute(&self, entry_index: u64, payload: &EncryptedPayload) -> [u8; 32] {
        let mut mac = <HmacSha256 as KeyInit>::new_from_slice(&self.key)
            .expect("HMAC-SHA256 accepts any key size including 32 bytes");
        mac.update(&entry_index.to_le_bytes());
        mac.update(&payload.nonce);
        mac.update(&payload.ciphertext);

        let result = mac.finalize().into_bytes();
        let mut tag = [0u8; 32];
        tag.copy_from_slice(&result);
        tag
    }

    /// Verify `tag` against the HMAC of `payload` using constant-time comparison.
    ///
    /// # Errors
    /// Returns [`RaftError::StorageError`] when the tag does not match.
    pub fn verify(
        &self,
        entry_index: u64,
        payload: &EncryptedPayload,
        tag: &[u8; 32],
    ) -> RaftResult<()> {
        let mut mac = <HmacSha256 as KeyInit>::new_from_slice(&self.key)
            .expect("HMAC-SHA256 accepts any key size including 32 bytes");
        mac.update(&entry_index.to_le_bytes());
        mac.update(&payload.nonce);
        mac.update(&payload.ciphertext);

        // `verify_slice` performs a constant-time comparison internally.
        mac.verify_slice(tag).map_err(|_| RaftError::StorageError {
            message: "HMAC-SHA256 integrity verification failed: tag mismatch".to_string(),
        })
    }
}

// ──────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = LogEncryptionKey::random();
        let encryptor = EntryEncryptor::new(key);
        let plaintext = b"Hello, Raft log entry!";

        let payload = encryptor
            .encrypt(42, plaintext)
            .expect("encrypt should succeed");
        let decrypted = encryptor
            .decrypt(42, &payload)
            .expect("decrypt should succeed");

        assert_eq!(decrypted.as_slice(), plaintext.as_ref());
    }

    #[test]
    fn test_different_indices_produce_different_ciphertexts() {
        let key = LogEncryptionKey::new([0xab; 32]);
        let encryptor = EntryEncryptor::new(key);
        let plaintext = b"same plaintext for both entries";

        let payload1 = encryptor.encrypt(1, plaintext).expect("encrypt entry 1");
        let payload2 = encryptor.encrypt(2, plaintext).expect("encrypt entry 2");

        assert_ne!(payload1.ciphertext, payload2.ciphertext);
        assert_ne!(payload1.nonce, payload2.nonce);
    }

    #[test]
    fn test_hmac_verify_valid() {
        let key = [0x12u8; 32];
        let verifier = LogIntegrityVerifier::new(key);
        let payload = EncryptedPayload {
            ciphertext: vec![0xde, 0xad, 0xbe, 0xef],
            nonce: [0u8; 12],
        };

        let tag = verifier.compute(7, &payload);
        verifier
            .verify(7, &payload, &tag)
            .expect("HMAC should verify successfully");
    }

    #[test]
    fn test_hmac_verify_tampered_fails() {
        let key = [0x34u8; 32];
        let verifier = LogIntegrityVerifier::new(key);
        let mut payload = EncryptedPayload {
            ciphertext: vec![0x01, 0x02, 0x03, 0x04, 0x05],
            nonce: [0u8; 12],
        };

        let tag = verifier.compute(99, &payload);

        // Flip one bit in the ciphertext to simulate tampering.
        payload.ciphertext[2] ^= 0xff;

        let result = verifier.verify(99, &payload, &tag);
        assert!(
            result.is_err(),
            "verification of tampered payload should fail"
        );
    }

    #[test]
    fn test_key_from_slice_wrong_length() {
        let too_short = [0u8; 16];
        assert!(
            LogEncryptionKey::from_slice(&too_short).is_err(),
            "should reject a 16-byte slice"
        );

        let too_long = [0u8; 64];
        assert!(
            LogEncryptionKey::from_slice(&too_long).is_err(),
            "should reject a 64-byte slice"
        );

        let correct = [0u8; 32];
        assert!(
            LogEncryptionKey::from_slice(&correct).is_ok(),
            "should accept a 32-byte slice"
        );
    }

    #[test]
    fn test_encrypt_empty_plaintext() {
        let key = LogEncryptionKey::new([0xcc; 32]);
        let encryptor = EntryEncryptor::new(key);

        let payload = encryptor
            .encrypt(0, b"")
            .expect("encrypting empty plaintext should succeed");
        let decrypted = encryptor
            .decrypt(0, &payload)
            .expect("decrypting empty ciphertext should succeed");

        assert!(
            decrypted.is_empty(),
            "round-tripped empty plaintext must be empty"
        );
    }
}
