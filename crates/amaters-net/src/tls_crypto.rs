//! Pure Rust cryptographic primitives for encrypted PEM key handling
//!
//! This module provides:
//! - Encrypted PEM format detection (PKCS#8, legacy OpenSSL)
//! - PKCS#8 encrypted key parsing and decryption (ASN.1/DER)
//! - Legacy OpenSSL encrypted key decryption
//! - PBKDF2-HMAC-SHA256 and PBKDF2-HMAC-SHA1 key derivation
//! - AES-CBC decryption (128/192/256-bit)
//! - Pure Rust SHA-256, SHA-1, MD5 hash implementations
//! - EVP_BytesToKey key derivation (legacy OpenSSL compatibility)
//!
//! All implementations are 100% pure Rust with zero C/Fortran dependencies.

use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tracing::debug;

use crate::error::{NetError, NetResult};

// ============================================================================
// Encrypted PEM support (Pure Rust)
// ============================================================================

/// Detected encryption format for a PEM file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptedPemFormat {
    /// Not encrypted — standard PEM
    NotEncrypted,
    /// PKCS#8 encrypted private key (`BEGIN ENCRYPTED PRIVATE KEY`)
    Pkcs8Encrypted,
    /// Legacy OpenSSL encrypted format (`Proc-Type: 4,ENCRYPTED`)
    LegacyEncrypted,
}

/// Detect whether a PEM string contains an encrypted private key
pub fn detect_encrypted_pem(pem_str: &str) -> EncryptedPemFormat {
    if pem_str.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----") {
        EncryptedPemFormat::Pkcs8Encrypted
    } else if pem_str.contains("Proc-Type: 4,ENCRYPTED") {
        EncryptedPemFormat::LegacyEncrypted
    } else {
        EncryptedPemFormat::NotEncrypted
    }
}

/// Resolve effective password: use provided password or fall back to env var
pub(crate) fn resolve_password(password: &str) -> NetResult<String> {
    if !password.is_empty() {
        return Ok(password.to_string());
    }

    match std::env::var("AMATERS_KEY_PASSWORD") {
        Ok(env_pw) if !env_pw.is_empty() => {
            debug!("Using password from AMATERS_KEY_PASSWORD environment variable");
            Ok(env_pw)
        }
        _ => Err(NetError::InvalidCertificate(
            "Key is encrypted but no password provided. Set AMATERS_KEY_PASSWORD or pass a password."
                .to_string(),
        )),
    }
}

/// Parse DEK-Info header from legacy OpenSSL encrypted PEM
///
/// Returns (algorithm_name, iv_bytes)
pub fn parse_dek_info(pem_str: &str) -> NetResult<(String, Vec<u8>)> {
    for line in pem_str.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("DEK-Info:") {
            let rest = rest.trim();
            let parts: Vec<&str> = rest.splitn(2, ',').collect();
            if parts.len() != 2 {
                return Err(NetError::InvalidCertificate(
                    "Malformed DEK-Info header: expected 'algorithm,IV'".to_string(),
                ));
            }
            let algorithm = parts[0].trim().to_string();
            let iv_hex = parts[1].trim();
            let iv = hex_decode(iv_hex).map_err(|e| {
                NetError::InvalidCertificate(format!("Invalid IV hex in DEK-Info: {e}"))
            })?;
            return Ok((algorithm, iv));
        }
    }
    Err(NetError::InvalidCertificate(
        "No DEK-Info header found in legacy encrypted PEM".to_string(),
    ))
}

/// Decrypt a PKCS#8 encrypted PEM key
///
/// Parses the ASN.1 DER structure to extract:
/// - Encryption algorithm (PBES2 with PBKDF2 + AES-CBC)
/// - PBKDF2 parameters (salt, iteration count)
/// - AES IV
///
/// Then derives the key and decrypts.
pub(crate) fn decrypt_pkcs8_encrypted_pem(
    pem_str: &str,
    password: &str,
) -> NetResult<PrivateKeyDer<'static>> {
    let der_data = extract_pem_body(pem_str, "ENCRYPTED PRIVATE KEY")?;
    let decrypted = decrypt_pkcs8_der(&der_data, password.as_bytes())?;
    Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(decrypted)))
}

/// Decrypt a legacy OpenSSL encrypted PEM key
pub(crate) fn decrypt_legacy_encrypted_pem(
    pem_str: &str,
    password: &str,
) -> NetResult<PrivateKeyDer<'static>> {
    let (algorithm, iv) = parse_dek_info(pem_str)?;

    // Determine key size from algorithm
    let key_len = match algorithm.as_str() {
        "AES-256-CBC" => 32,
        "AES-128-CBC" => 16,
        "AES-192-CBC" => 24,
        "DES-EDE3-CBC" => 24,
        other => {
            return Err(NetError::InvalidCertificate(format!(
                "Unsupported encryption algorithm: {other}"
            )));
        }
    };

    // Extract the base64 body (skip headers and Proc-Type/DEK-Info lines)
    let body = extract_legacy_pem_body(pem_str)?;

    // Legacy OpenSSL uses EVP_BytesToKey for key derivation (MD5-based)
    let derived_key = evp_bytes_to_key_md5(password.as_bytes(), &iv[..8], key_len);

    // Decrypt
    let decrypted = if algorithm.starts_with("AES") {
        aes_cbc_decrypt(&body, &derived_key, &iv)?
    } else if algorithm == "DES-EDE3-CBC" {
        return Err(NetError::InvalidCertificate(
            "DES-EDE3-CBC is not supported in pure Rust mode. Please re-encrypt with AES-256-CBC."
                .to_string(),
        ));
    } else {
        return Err(NetError::InvalidCertificate(format!(
            "Unsupported encryption algorithm: {algorithm}"
        )));
    };

    // Remove PKCS#7 padding
    let unpadded = remove_pkcs7_padding(&decrypted)?;

    // The decrypted content is a PKCS#1 RSA private key in DER format
    // (legacy format encrypts the inner key, not wrapped in PKCS#8)
    // Try to interpret as PKCS#8 first, fall back to PKCS#1
    if is_pkcs8_key_der(unpadded) {
        Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            unpadded.to_vec(),
        )))
    } else {
        // Legacy encrypted RSA keys contain raw PKCS#1
        Ok(PrivateKeyDer::Pkcs1(unpadded.to_vec().into()))
    }
}

/// Extract base64-encoded body from a PEM block with given label
fn extract_pem_body(pem_str: &str, label: &str) -> NetResult<Vec<u8>> {
    let begin_marker = format!("-----BEGIN {label}-----");
    let end_marker = format!("-----END {label}-----");

    let start = pem_str.find(&begin_marker).ok_or_else(|| {
        NetError::InvalidCertificate(format!("Missing PEM header: {begin_marker}"))
    })? + begin_marker.len();

    let end = pem_str
        .find(&end_marker)
        .ok_or_else(|| NetError::InvalidCertificate(format!("Missing PEM footer: {end_marker}")))?;

    let body = &pem_str[start..end];
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    base64_decode_pure(&b64)
        .map_err(|e| NetError::InvalidCertificate(format!("Invalid base64 in PEM: {e}")))
}

/// Extract base64 body from legacy encrypted PEM (skip Proc-Type and DEK-Info headers)
fn extract_legacy_pem_body(pem_str: &str) -> NetResult<Vec<u8>> {
    let mut in_body = false;
    let mut past_headers = false;
    let mut b64 = String::new();

    for line in pem_str.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("-----BEGIN ") && trimmed.ends_with("-----") {
            in_body = true;
            continue;
        }
        if trimmed.starts_with("-----END ") && trimmed.ends_with("-----") {
            break;
        }
        if !in_body {
            continue;
        }
        // Skip Proc-Type and DEK-Info headers
        if trimmed.starts_with("Proc-Type:") || trimmed.starts_with("DEK-Info:") {
            continue;
        }
        // Skip empty line after headers
        if !past_headers && trimmed.is_empty() {
            past_headers = true;
            continue;
        }
        past_headers = true;
        b64.push_str(trimmed);
    }

    base64_decode_pure(&b64)
        .map_err(|e| NetError::InvalidCertificate(format!("Invalid base64 in legacy PEM: {e}")))
}

/// Check if DER data looks like PKCS#8 (starts with SEQUENCE containing version INTEGER)
fn is_pkcs8_key_der(data: &[u8]) -> bool {
    // PKCS#8 PrivateKeyInfo starts with SEQUENCE > INTEGER (version 0)
    // then AlgorithmIdentifier SEQUENCE
    if data.len() < 4 {
        return false;
    }
    // 0x30 = SEQUENCE tag
    if data[0] != 0x30 {
        return false;
    }
    // Parse length to find content start
    let (_, content_offset) = match parse_asn1_length(&data[1..]) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let content_start = 1 + content_offset;
    if content_start >= data.len() {
        return false;
    }
    // First element should be INTEGER (0x02) with value 0 (version)
    if data[content_start] == 0x02 {
        // Check for AlgorithmIdentifier after the version
        let ver_start = content_start + 1;
        if ver_start < data.len() {
            let (ver_len, ver_len_size) = match parse_asn1_length(&data[ver_start..]) {
                Ok(v) => v,
                Err(_) => return false,
            };
            let after_ver = ver_start + ver_len_size + ver_len;
            if after_ver < data.len() && data[after_ver] == 0x30 {
                return true;
            }
        }
    }
    false
}

// ============================================================================
// PKCS#8 encrypted DER parsing (ASN.1)
// ============================================================================

/// Parse ASN.1 length encoding, returns (length, bytes_consumed)
pub(crate) fn parse_asn1_length(data: &[u8]) -> NetResult<(usize, usize)> {
    if data.is_empty() {
        return Err(NetError::InvalidCertificate(
            "ASN.1 parse error: unexpected end of data for length".to_string(),
        ));
    }
    if data[0] < 0x80 {
        Ok((data[0] as usize, 1))
    } else if data[0] == 0x80 {
        Err(NetError::InvalidCertificate(
            "ASN.1 indefinite length not supported".to_string(),
        ))
    } else {
        let num_bytes = (data[0] & 0x7f) as usize;
        if num_bytes > 4 || num_bytes + 1 > data.len() {
            return Err(NetError::InvalidCertificate(
                "ASN.1 parse error: length too large or truncated".to_string(),
            ));
        }
        let mut length = 0usize;
        for &b in &data[1..=num_bytes] {
            length = length
                .checked_shl(8)
                .ok_or_else(|| NetError::InvalidCertificate("ASN.1 length overflow".to_string()))?
                .checked_add(b as usize)
                .ok_or_else(|| NetError::InvalidCertificate("ASN.1 length overflow".to_string()))?;
        }
        Ok((length, num_bytes + 1))
    }
}

/// Skip over an ASN.1 TLV (tag-length-value), returning bytes consumed
fn skip_asn1_tlv(data: &[u8]) -> NetResult<usize> {
    if data.is_empty() {
        return Err(NetError::InvalidCertificate(
            "ASN.1 parse error: empty TLV".to_string(),
        ));
    }
    let (len, len_size) = parse_asn1_length(&data[1..])?;
    Ok(1 + len_size + len)
}

/// PKCS#8 EncryptedPrivateKeyInfo structure:
/// SEQUENCE {
///   SEQUENCE { -- encryptionAlgorithm (AlgorithmIdentifier)
///     OID -- PBES2 (1.2.840.113549.1.5.13)
///     SEQUENCE { -- PBES2-params
///       SEQUENCE { -- keyDerivationFunc (AlgorithmIdentifier)
///         OID -- PBKDF2 (1.2.840.113549.1.5.12)
///         SEQUENCE { -- PBKDF2-params
///           OCTET STRING -- salt
///           INTEGER -- iterationCount
///           [optional INTEGER -- keyLength]
///           [optional SEQUENCE -- prf AlgorithmIdentifier, default HMAC-SHA1]
///         }
///       }
///       SEQUENCE { -- encryptionScheme (AlgorithmIdentifier)
///         OID -- e.g. AES-256-CBC (2.16.840.1.101.3.4.1.42)
///         OCTET STRING -- IV
///       }
///     }
///   }
///   OCTET STRING -- encryptedData
/// }
fn decrypt_pkcs8_der(data: &[u8], password: &[u8]) -> NetResult<Vec<u8>> {
    let mut pos = 0;

    // Outer SEQUENCE
    if data.get(pos) != Some(&0x30) {
        return Err(NetError::InvalidCertificate(
            "PKCS#8 encrypted: expected outer SEQUENCE".to_string(),
        ));
    }
    pos += 1;
    let (_outer_len, outer_len_size) = parse_asn1_length(&data[pos..])?;
    pos += outer_len_size;

    // encryptionAlgorithm SEQUENCE
    if data.get(pos) != Some(&0x30) {
        return Err(NetError::InvalidCertificate(
            "PKCS#8 encrypted: expected algorithm SEQUENCE".to_string(),
        ));
    }
    pos += 1;
    let (algo_seq_len, algo_len_size) = parse_asn1_length(&data[pos..])?;
    pos += algo_len_size;
    let algo_seq_end = pos + algo_seq_len;

    // Read PBES2 OID
    let pbes2_oid = parse_asn1_oid(&data[pos..])?;
    pos += skip_asn1_tlv(&data[pos..])?;

    // Verify PBES2 OID: 1.2.840.113549.1.5.13
    if pbes2_oid != [42, 134, 72, 134, 247, 13, 1, 5, 13] {
        return Err(NetError::InvalidCertificate(format!(
            "Unsupported encryption algorithm OID (expected PBES2): {:?}",
            pbes2_oid
        )));
    }

    // PBES2-params SEQUENCE
    if data.get(pos) != Some(&0x30) {
        return Err(NetError::InvalidCertificate(
            "PKCS#8 encrypted: expected PBES2-params SEQUENCE".to_string(),
        ));
    }
    pos += 1;
    let (_pbes2_len, pbes2_len_size) = parse_asn1_length(&data[pos..])?;
    pos += pbes2_len_size;

    // keyDerivationFunc SEQUENCE
    if data.get(pos) != Some(&0x30) {
        return Err(NetError::InvalidCertificate(
            "PKCS#8 encrypted: expected KDF SEQUENCE".to_string(),
        ));
    }
    pos += 1;
    let (kdf_seq_len, kdf_len_size) = parse_asn1_length(&data[pos..])?;
    pos += kdf_len_size;
    let kdf_seq_end = pos + kdf_seq_len;

    // Read PBKDF2 OID
    let pbkdf2_oid = parse_asn1_oid(&data[pos..])?;
    pos += skip_asn1_tlv(&data[pos..])?;

    // Verify PBKDF2 OID: 1.2.840.113549.1.5.12
    if pbkdf2_oid != [42, 134, 72, 134, 247, 13, 1, 5, 12] {
        return Err(NetError::InvalidCertificate(format!(
            "Unsupported KDF OID (expected PBKDF2): {:?}",
            pbkdf2_oid
        )));
    }

    // PBKDF2-params SEQUENCE
    if data.get(pos) != Some(&0x30) {
        return Err(NetError::InvalidCertificate(
            "PKCS#8 encrypted: expected PBKDF2-params SEQUENCE".to_string(),
        ));
    }
    pos += 1;
    let (pbkdf2_params_len, pbkdf2_params_len_size) = parse_asn1_length(&data[pos..])?;
    pos += pbkdf2_params_len_size;
    let pbkdf2_params_end = pos + pbkdf2_params_len;

    // salt OCTET STRING
    let salt = parse_asn1_octet_string(&data[pos..])?;
    pos += skip_asn1_tlv(&data[pos..])?;

    // iterationCount INTEGER
    let iterations = parse_asn1_integer_value(&data[pos..])?;
    pos += skip_asn1_tlv(&data[pos..])?;

    // Optional: keyLength INTEGER and PRF AlgorithmIdentifier
    // We detect the PRF to decide SHA-1 vs SHA-256
    let mut prf_is_sha256 = false;
    while pos < pbkdf2_params_end {
        let tag = data.get(pos).copied().unwrap_or(0);
        if tag == 0x02 {
            // keyLength — skip
            pos += skip_asn1_tlv(&data[pos..])?;
        } else if tag == 0x30 {
            // PRF AlgorithmIdentifier
            let prf_inner_start = pos + 1;
            let (prf_seq_len, prf_seq_len_size) = parse_asn1_length(&data[prf_inner_start..])?;
            let prf_content_start = prf_inner_start + prf_seq_len_size;
            let prf_oid = parse_asn1_oid(&data[prf_content_start..])?;
            // HMAC-SHA-256: 1.2.840.113549.2.9
            if prf_oid == [42, 134, 72, 134, 247, 13, 2, 9] {
                prf_is_sha256 = true;
            }
            // HMAC-SHA-1: 1.2.840.113549.2.7  (default if not SHA-256)
            pos += skip_asn1_tlv(&data[pos..])?;
        } else {
            pos += skip_asn1_tlv(&data[pos..])?;
        }
    }
    pos = kdf_seq_end;

    // encryptionScheme SEQUENCE
    if data.get(pos) != Some(&0x30) {
        return Err(NetError::InvalidCertificate(
            "PKCS#8 encrypted: expected encryption scheme SEQUENCE".to_string(),
        ));
    }
    pos += 1;
    let (enc_seq_len, enc_len_size) = parse_asn1_length(&data[pos..])?;
    pos += enc_len_size;

    let enc_oid = parse_asn1_oid(&data[pos..])?;
    pos += skip_asn1_tlv(&data[pos..])?;

    // Determine AES key size from encryption OID
    // AES-128-CBC: 2.16.840.1.101.3.4.1.2
    // AES-192-CBC: 2.16.840.1.101.3.4.1.22
    // AES-256-CBC: 2.16.840.1.101.3.4.1.42
    let key_len = match enc_oid.as_slice() {
        [96, 134, 72, 1, 101, 3, 4, 1, 2] => 16,
        [96, 134, 72, 1, 101, 3, 4, 1, 22] => 24,
        [96, 134, 72, 1, 101, 3, 4, 1, 42] => 32,
        _ => {
            return Err(NetError::InvalidCertificate(format!(
                "Unsupported encryption scheme OID: {:?}",
                enc_oid
            )));
        }
    };

    // IV OCTET STRING
    let iv = parse_asn1_octet_string(&data[pos..])?;

    // Move past the algorithm sequence to get encrypted data
    let pos = algo_seq_end;

    // encryptedData OCTET STRING
    let encrypted_data = parse_asn1_octet_string(&data[pos..])?;

    // Derive key using PBKDF2
    let derived_key = if prf_is_sha256 {
        pbkdf2_hmac_sha256(password, &salt, iterations as u32, key_len)
    } else {
        pbkdf2_hmac_sha1(password, &salt, iterations as u32, key_len)
    };

    // Decrypt with AES-CBC
    let decrypted = aes_cbc_decrypt(&encrypted_data, &derived_key, &iv)?;

    // Remove PKCS#7 padding
    let unpadded = remove_pkcs7_padding(&decrypted)?;

    Ok(unpadded.to_vec())
}

/// Parse an ASN.1 OID, returning raw OID bytes (not decoded dotted form)
fn parse_asn1_oid(data: &[u8]) -> NetResult<Vec<u8>> {
    if data.is_empty() || data[0] != 0x06 {
        return Err(NetError::InvalidCertificate(
            "ASN.1 parse error: expected OID tag (0x06)".to_string(),
        ));
    }
    let (len, len_size) = parse_asn1_length(&data[1..])?;
    let start = 1 + len_size;
    let end = start + len;
    if end > data.len() {
        return Err(NetError::InvalidCertificate(
            "ASN.1 parse error: OID data truncated".to_string(),
        ));
    }
    Ok(data[start..end].to_vec())
}

/// Parse an ASN.1 OCTET STRING, returning the contained bytes
fn parse_asn1_octet_string(data: &[u8]) -> NetResult<Vec<u8>> {
    if data.is_empty() || data[0] != 0x04 {
        return Err(NetError::InvalidCertificate(
            "ASN.1 parse error: expected OCTET STRING tag (0x04)".to_string(),
        ));
    }
    let (len, len_size) = parse_asn1_length(&data[1..])?;
    let start = 1 + len_size;
    let end = start + len;
    if end > data.len() {
        return Err(NetError::InvalidCertificate(
            "ASN.1 parse error: OCTET STRING data truncated".to_string(),
        ));
    }
    Ok(data[start..end].to_vec())
}

/// Parse an ASN.1 INTEGER value as usize
fn parse_asn1_integer_value(data: &[u8]) -> NetResult<usize> {
    if data.is_empty() || data[0] != 0x02 {
        return Err(NetError::InvalidCertificate(
            "ASN.1 parse error: expected INTEGER tag (0x02)".to_string(),
        ));
    }
    let (len, len_size) = parse_asn1_length(&data[1..])?;
    let start = 1 + len_size;
    let end = start + len;
    if end > data.len() {
        return Err(NetError::InvalidCertificate(
            "ASN.1 parse error: INTEGER data truncated".to_string(),
        ));
    }
    let mut value = 0usize;
    for &b in &data[start..end] {
        value = value
            .checked_shl(8)
            .ok_or_else(|| NetError::InvalidCertificate("ASN.1 INTEGER overflow".to_string()))?
            .checked_add(b as usize)
            .ok_or_else(|| NetError::InvalidCertificate("ASN.1 INTEGER overflow".to_string()))?;
    }
    Ok(value)
}

// ============================================================================
// Pure Rust PBKDF2 implementations
// ============================================================================

/// PBKDF2-HMAC-SHA256 key derivation (pure Rust)
pub fn pbkdf2_hmac_sha256(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    key_len: usize,
) -> Vec<u8> {
    let mut result = Vec::with_capacity(key_len);
    let mut block_num = 1u32;

    while result.len() < key_len {
        let block = pbkdf2_f_sha256(password, salt, iterations, block_num);
        let needed = key_len - result.len();
        result.extend_from_slice(&block[..needed.min(32)]);
        block_num += 1;
    }

    result.truncate(key_len);
    result
}

/// PBKDF2 F function for SHA-256
fn pbkdf2_f_sha256(password: &[u8], salt: &[u8], iterations: u32, block_num: u32) -> [u8; 32] {
    // U_1 = HMAC(password, salt || INT(block_num))
    let mut msg = Vec::with_capacity(salt.len() + 4);
    msg.extend_from_slice(salt);
    msg.extend_from_slice(&block_num.to_be_bytes());

    let mut u_prev = hmac_sha256(password, &msg);
    let mut result = u_prev;

    for _ in 1..iterations {
        let u_curr = hmac_sha256(password, &u_prev);
        for (r, c) in result.iter_mut().zip(u_curr.iter()) {
            *r ^= c;
        }
        u_prev = u_curr;
    }

    result
}

/// HMAC-SHA256 (pure Rust)
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;

    let normalized_key = if key.len() > BLOCK_SIZE {
        let h = sha256(key);
        let mut k = [0u8; BLOCK_SIZE];
        k[..32].copy_from_slice(&h);
        k
    } else {
        let mut k = [0u8; BLOCK_SIZE];
        k[..key.len()].copy_from_slice(key);
        k
    };

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= normalized_key[i];
        opad[i] ^= normalized_key[i];
    }

    // inner = SHA256(ipad || message)
    let mut inner_msg = Vec::with_capacity(BLOCK_SIZE + message.len());
    inner_msg.extend_from_slice(&ipad);
    inner_msg.extend_from_slice(message);
    let inner_hash = sha256(&inner_msg);

    // outer = SHA256(opad || inner_hash)
    let mut outer_msg = Vec::with_capacity(BLOCK_SIZE + 32);
    outer_msg.extend_from_slice(&opad);
    outer_msg.extend_from_slice(&inner_hash);
    sha256(&outer_msg)
}

/// Pure Rust SHA-256 implementation
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Pre-processing: padding
    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit (64-byte) block
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut result = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        result[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
    }
    result
}

/// PBKDF2-HMAC-SHA1 key derivation (pure Rust) for legacy compatibility
pub fn pbkdf2_hmac_sha1(password: &[u8], salt: &[u8], iterations: u32, key_len: usize) -> Vec<u8> {
    let mut result = Vec::with_capacity(key_len);
    let mut block_num = 1u32;

    while result.len() < key_len {
        let block = pbkdf2_f_sha1(password, salt, iterations, block_num);
        let needed = key_len - result.len();
        result.extend_from_slice(&block[..needed.min(20)]);
        block_num += 1;
    }

    result.truncate(key_len);
    result
}

/// PBKDF2 F function for SHA-1
fn pbkdf2_f_sha1(password: &[u8], salt: &[u8], iterations: u32, block_num: u32) -> [u8; 20] {
    let mut msg = Vec::with_capacity(salt.len() + 4);
    msg.extend_from_slice(salt);
    msg.extend_from_slice(&block_num.to_be_bytes());

    let mut u_prev = hmac_sha1(password, &msg);
    let mut result = u_prev;

    for _ in 1..iterations {
        let u_curr = hmac_sha1(password, &u_prev);
        for (r, c) in result.iter_mut().zip(u_curr.iter()) {
            *r ^= c;
        }
        u_prev = u_curr;
    }

    result
}

/// HMAC-SHA1 (pure Rust)
fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    const BLOCK_SIZE: usize = 64;

    let normalized_key = if key.len() > BLOCK_SIZE {
        let h = sha1(key);
        let mut k = [0u8; BLOCK_SIZE];
        k[..20].copy_from_slice(&h);
        k
    } else {
        let mut k = [0u8; BLOCK_SIZE];
        k[..key.len()].copy_from_slice(key);
        k
    };

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= normalized_key[i];
        opad[i] ^= normalized_key[i];
    }

    let mut inner_msg = Vec::with_capacity(BLOCK_SIZE + message.len());
    inner_msg.extend_from_slice(&ipad);
    inner_msg.extend_from_slice(message);
    let inner_hash = sha1(&inner_msg);

    let mut outer_msg = Vec::with_capacity(BLOCK_SIZE + 20);
    outer_msg.extend_from_slice(&opad);
    outer_msg.extend_from_slice(&inner_hash);
    sha1(&outer_msg)
}

/// Pure Rust SHA-1 implementation
pub(crate) fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);

        for (i, w_i) in w.iter().enumerate() {
            let (f_val, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f_val)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*w_i);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut result = [0u8; 20];
    result[0..4].copy_from_slice(&h0.to_be_bytes());
    result[4..8].copy_from_slice(&h1.to_be_bytes());
    result[8..12].copy_from_slice(&h2.to_be_bytes());
    result[12..16].copy_from_slice(&h3.to_be_bytes());
    result[16..20].copy_from_slice(&h4.to_be_bytes());
    result
}

/// EVP_BytesToKey using MD5 -- used by legacy OpenSSL encrypted PEM
///
/// Derives key material from password + salt using iterated MD5 hashing.
pub(crate) fn evp_bytes_to_key_md5(password: &[u8], salt: &[u8], key_len: usize) -> Vec<u8> {
    let mut result = Vec::with_capacity(key_len);
    let mut d_prev: Option<[u8; 16]> = None;

    while result.len() < key_len {
        let mut input = Vec::new();
        if let Some(prev) = d_prev {
            input.extend_from_slice(&prev);
        }
        input.extend_from_slice(password);
        input.extend_from_slice(&salt[..8.min(salt.len())]);
        let hash = md5(&input);
        let needed = key_len - result.len();
        result.extend_from_slice(&hash[..needed.min(16)]);
        d_prev = Some(hash);
    }

    result.truncate(key_len);
    result
}

/// Pure Rust MD5 implementation (for EVP_BytesToKey only)
pub(crate) fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    #[allow(clippy::unreadable_literal)]
    const T: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }

        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);

        for i in 0..64 {
            let (f_val, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let f_val = f_val.wrapping_add(a).wrapping_add(T[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f_val.rotate_left(S[i]));
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut result = [0u8; 16];
    result[0..4].copy_from_slice(&a0.to_le_bytes());
    result[4..8].copy_from_slice(&b0.to_le_bytes());
    result[8..12].copy_from_slice(&c0.to_le_bytes());
    result[12..16].copy_from_slice(&d0.to_le_bytes());
    result
}

// ============================================================================
// Pure Rust AES-CBC decryption
// ============================================================================

/// AES S-Box
pub(crate) const AES_SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// Inverse AES S-Box
const AES_INV_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
    0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
    0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
    0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
    0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
    0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
    0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
    0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
    0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
    0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
    0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
    0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
    0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
    0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
    0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
];

/// AES round constants
const AES_RCON: [u8; 11] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36,
];

/// Galois field multiplication by 2
fn gf_mul2(a: u8) -> u8 {
    if a & 0x80 != 0 {
        (a << 1) ^ 0x1b
    } else {
        a << 1
    }
}

/// Galois field multiplication
pub(crate) fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut result: u8 = 0;
    while b != 0 {
        if b & 1 != 0 {
            result ^= a;
        }
        a = gf_mul2(a);
        b >>= 1;
    }
    result
}

/// AES key expansion
pub(crate) fn aes_key_expansion(key: &[u8]) -> NetResult<Vec<[u8; 4]>> {
    let nk = key.len() / 4; // Number of 32-bit words in key
    let nr = nk + 6; // Number of rounds
    let total_words = 4 * (nr + 1);

    let mut w: Vec<[u8; 4]> = Vec::with_capacity(total_words);

    // First Nk words are the key itself
    for i in 0..nk {
        w.push([key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]]);
    }

    for i in nk..total_words {
        let mut temp = w[i - 1];

        if i % nk == 0 {
            // RotWord
            temp = [temp[1], temp[2], temp[3], temp[0]];
            // SubWord
            temp = [
                AES_SBOX[temp[0] as usize],
                AES_SBOX[temp[1] as usize],
                AES_SBOX[temp[2] as usize],
                AES_SBOX[temp[3] as usize],
            ];
            // XOR with Rcon
            let rcon_idx = i / nk;
            if rcon_idx >= AES_RCON.len() {
                return Err(NetError::InvalidCertificate(
                    "AES key expansion: rcon index out of bounds".to_string(),
                ));
            }
            temp[0] ^= AES_RCON[rcon_idx];
        } else if nk > 6 && i % nk == 4 {
            // For AES-256: SubWord on 4th word
            temp = [
                AES_SBOX[temp[0] as usize],
                AES_SBOX[temp[1] as usize],
                AES_SBOX[temp[2] as usize],
                AES_SBOX[temp[3] as usize],
            ];
        }

        let prev = w[i - nk];
        w.push([
            prev[0] ^ temp[0],
            prev[1] ^ temp[1],
            prev[2] ^ temp[2],
            prev[3] ^ temp[3],
        ]);
    }

    Ok(w)
}

/// AES block decryption (single 16-byte block)
fn aes_decrypt_block(block: &[u8; 16], round_keys: &[[u8; 4]]) -> [u8; 16] {
    let nr = round_keys.len() / 4 - 1;
    let mut state = [[0u8; 4]; 4];

    // Copy input to state (column-major order)
    for c in 0..4 {
        for r in 0..4 {
            state[r][c] = block[c * 4 + r];
        }
    }

    // Initial round key addition
    for c in 0..4 {
        for r in 0..4 {
            state[r][c] ^= round_keys[nr * 4 + c][r];
        }
    }

    // Main rounds (in reverse)
    for round in (1..nr).rev() {
        // InvShiftRows
        inv_shift_rows(&mut state);
        // InvSubBytes
        inv_sub_bytes(&mut state);
        // AddRoundKey
        for c in 0..4 {
            for r in 0..4 {
                state[r][c] ^= round_keys[round * 4 + c][r];
            }
        }
        // InvMixColumns
        inv_mix_columns(&mut state);
    }

    // Final round
    inv_shift_rows(&mut state);
    inv_sub_bytes(&mut state);
    for c in 0..4 {
        for r in 0..4 {
            state[r][c] ^= round_keys[c][r];
        }
    }

    // State to output
    let mut output = [0u8; 16];
    for c in 0..4 {
        for r in 0..4 {
            output[c * 4 + r] = state[r][c];
        }
    }
    output
}

fn inv_sub_bytes(state: &mut [[u8; 4]; 4]) {
    for row in state.iter_mut() {
        for val in row.iter_mut() {
            *val = AES_INV_SBOX[*val as usize];
        }
    }
}

fn inv_shift_rows(state: &mut [[u8; 4]; 4]) {
    // Row 0: no shift
    // Row 1: shift right by 1
    let tmp = state[1][3];
    state[1][3] = state[1][2];
    state[1][2] = state[1][1];
    state[1][1] = state[1][0];
    state[1][0] = tmp;
    // Row 2: shift right by 2
    let (t0, t1) = (state[2][0], state[2][1]);
    state[2][0] = state[2][2];
    state[2][1] = state[2][3];
    state[2][2] = t0;
    state[2][3] = t1;
    // Row 3: shift right by 3
    let tmp = state[3][0];
    state[3][0] = state[3][1];
    state[3][1] = state[3][2];
    state[3][2] = state[3][3];
    state[3][3] = tmp;
}

#[allow(clippy::needless_range_loop)]
fn inv_mix_columns(state: &mut [[u8; 4]; 4]) {
    // Column-major operation: each column c spans all 4 rows, so range loop is clearest
    for c in 0..4 {
        let s0 = state[0][c];
        let s1 = state[1][c];
        let s2 = state[2][c];
        let s3 = state[3][c];

        state[0][c] = gf_mul(s0, 0x0e) ^ gf_mul(s1, 0x0b) ^ gf_mul(s2, 0x0d) ^ gf_mul(s3, 0x09);
        state[1][c] = gf_mul(s0, 0x09) ^ gf_mul(s1, 0x0e) ^ gf_mul(s2, 0x0b) ^ gf_mul(s3, 0x0d);
        state[2][c] = gf_mul(s0, 0x0d) ^ gf_mul(s1, 0x09) ^ gf_mul(s2, 0x0e) ^ gf_mul(s3, 0x0b);
        state[3][c] = gf_mul(s0, 0x0b) ^ gf_mul(s1, 0x0d) ^ gf_mul(s2, 0x09) ^ gf_mul(s3, 0x0e);
    }
}

/// AES-CBC decryption
pub(crate) fn aes_cbc_decrypt(ciphertext: &[u8], key: &[u8], iv: &[u8]) -> NetResult<Vec<u8>> {
    if ciphertext.len() % 16 != 0 {
        return Err(NetError::InvalidCertificate(
            "Decryption failed: ciphertext length is not a multiple of 16".to_string(),
        ));
    }
    if iv.len() != 16 {
        return Err(NetError::InvalidCertificate(format!(
            "Decryption failed: IV must be 16 bytes, got {}",
            iv.len()
        )));
    }
    if key.len() != 16 && key.len() != 24 && key.len() != 32 {
        return Err(NetError::InvalidCertificate(format!(
            "Decryption failed: invalid key length {} (expected 16, 24, or 32)",
            key.len()
        )));
    }

    let round_keys = aes_key_expansion(key)?;
    let mut result = Vec::with_capacity(ciphertext.len());
    let mut prev_cipher_block = [0u8; 16];
    prev_cipher_block.copy_from_slice(&iv[..16]);

    for chunk in ciphertext.chunks_exact(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);

        let decrypted_block = aes_decrypt_block(&block, &round_keys);

        // XOR with previous ciphertext block (CBC mode)
        for i in 0..16 {
            result.push(decrypted_block[i] ^ prev_cipher_block[i]);
        }

        prev_cipher_block = block;
    }

    Ok(result)
}

/// Remove PKCS#7 padding from decrypted data
pub(crate) fn remove_pkcs7_padding(data: &[u8]) -> NetResult<&[u8]> {
    if data.is_empty() {
        return Err(NetError::InvalidCertificate(
            "Decryption failed: empty decrypted data".to_string(),
        ));
    }

    let pad_len = *data.last().unwrap_or(&0) as usize;
    if pad_len == 0 || pad_len > 16 || pad_len > data.len() {
        return Err(NetError::InvalidCertificate(
            "Decryption failed: wrong password or corrupted key (invalid PKCS#7 padding)"
                .to_string(),
        ));
    }

    // Verify all padding bytes
    for &b in &data[data.len() - pad_len..] {
        if b as usize != pad_len {
            return Err(NetError::InvalidCertificate(
                "Decryption failed: wrong password or corrupted key (invalid PKCS#7 padding)"
                    .to_string(),
            ));
        }
    }

    Ok(&data[..data.len() - pad_len])
}

// ============================================================================
// Hex and Base64 utilities
// ============================================================================

/// Decode hex string to bytes
pub(crate) fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("Hex string has odd length".to_string());
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let mut chars = hex.chars();
    while let (Some(h), Some(l)) = (chars.next(), chars.next()) {
        let high = h
            .to_digit(16)
            .ok_or_else(|| format!("Invalid hex character: {h}"))? as u8;
        let low = l
            .to_digit(16)
            .ok_or_else(|| format!("Invalid hex character: {l}"))? as u8;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

/// Decode base64 string to bytes (pure Rust)
pub(crate) fn base64_decode_pure(input: &str) -> Result<Vec<u8>, String> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    let mut pad_count = 0;

    for c in input.chars() {
        if c == '=' {
            pad_count += 1;
            continue;
        }
        if pad_count > 0 {
            return Err("Invalid base64: data after padding".to_string());
        }
        let val = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(format!("Invalid base64 character: {c}")),
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(output)
}
