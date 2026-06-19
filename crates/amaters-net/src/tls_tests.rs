use super::*;
use crate::tls_crypto::{
    AES_SBOX, aes_cbc_decrypt, aes_key_expansion, base64_decode_pure, evp_bytes_to_key_md5, gf_mul,
    hex_decode, md5, parse_asn1_length, remove_pkcs7_padding, sha1, sha256,
};
use std::env::temp_dir;

#[test]
fn test_self_signed_generator() {
    let generator = SelfSignedGenerator::new("test.example.com")
        .with_san("localhost")
        .with_san("127.0.0.1")
        .with_organization("Test Org")
        .with_validity_days(30);

    let result = generator.generate();
    assert!(result.is_ok());

    let (cert, key) = result.expect("Should generate certificate");
    assert!(!cert.as_ref().is_empty());

    // Verify we can parse the certificate
    let loader = CertificateLoader::new();
    let info = loader
        .get_certificate_info(&cert)
        .expect("Should parse certificate");

    assert_eq!(info.common_name.as_deref(), Some("test.example.com"));
    assert!(info.is_valid());
}

#[test]
fn test_ca_certificate_generation() {
    let ca_generator = SelfSignedGenerator::new("Test CA")
        .as_ca()
        .with_validity_days(365);

    let (ca_cert, ca_key) = ca_generator.generate().expect("Should generate CA");

    let loader = CertificateLoader::new();
    let ca_info = loader
        .get_certificate_info(&ca_cert)
        .expect("Should parse CA certificate");

    assert!(ca_info.is_ca);
    assert_eq!(ca_info.common_name.as_deref(), Some("Test CA"));
}

#[test]
fn test_certificate_store() {
    let mut store = CertificateStore::new();

    // Generate a test certificate
    let generator = SelfSignedGenerator::new("test").as_ca();
    let (cert, _) = generator.generate().expect("Should generate certificate");

    assert!(store.is_empty());
    store.add_certificate(cert).expect("Should add certificate");
    assert!(!store.is_empty());
    assert_eq!(store.len(), 1);
}

#[test]
fn test_certificate_store_system_roots() {
    let mut store = CertificateStore::new();
    let added = store.add_system_roots().expect("Should add system roots");

    // Should have added some root certificates
    assert!(added > 0);
    assert!(!store.is_empty());
}

#[test]
fn test_certificate_info_validity() {
    let generator = SelfSignedGenerator::new("test").with_validity_days(30);

    let (cert, _) = generator.generate().expect("Should generate certificate");

    let loader = CertificateLoader::new();
    let info = loader.get_certificate_info(&cert).expect("Should get info");

    assert!(info.is_valid());
    assert!(!info.expires_within(Duration::from_secs(0)));

    // Should expire within 31 days
    assert!(info.expires_within(Duration::from_secs(31 * 24 * 60 * 60)));
}

#[test]
fn test_hot_reloadable_certificates() {
    let hot_certs = HotReloadableCertificates::new();

    // Generate test certificates
    let generator = SelfSignedGenerator::new("test");
    let (cert, key) = generator.generate().expect("Should generate certificate");

    assert_eq!(hot_certs.get_version(), 0);

    hot_certs.set_certificates(vec![cert], key);

    assert_eq!(hot_certs.get_version(), 1);
    assert!(!hot_certs.get_cert_chain().is_empty());
    assert!(hot_certs.get_private_key().is_some());
}

#[test]
fn test_pem_certificate_loading() {
    // Generate a certificate and save it to a temp file
    let generator = SelfSignedGenerator::new("test");
    let (cert, _) = generator.generate().expect("Should generate certificate");

    // Create PEM content
    let pem_content = format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        base64_encode(cert.as_ref())
    );

    let temp_path = temp_dir().join("test_cert.pem");
    fs::write(&temp_path, &pem_content).expect("Should write temp file");

    let loader = CertificateLoader::new();
    let result = loader.load_pem_file(&temp_path);

    // Clean up
    let _ = fs::remove_file(&temp_path);

    assert!(result.is_ok());
}

#[test]
fn test_der_certificate_loading() {
    // Generate a certificate and save it as DER
    let generator = SelfSignedGenerator::new("test");
    let (cert, _) = generator.generate().expect("Should generate certificate");

    let temp_path = temp_dir().join("test_cert.der");
    fs::write(&temp_path, cert.as_ref()).expect("Should write temp file");

    let loader = CertificateLoader::new();
    let result = loader.load_der_file(&temp_path);

    // Clean up
    let _ = fs::remove_file(&temp_path);

    assert!(result.is_ok());
}

/// Simple base64 encoding for tests
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::new();
    let mut i = 0;

    while i < data.len() {
        let b1 = data[i];
        let b2 = data.get(i + 1).copied().unwrap_or(0);
        let b3 = data.get(i + 2).copied().unwrap_or(0);

        result.push(ALPHABET[(b1 >> 2) as usize] as char);
        result.push(ALPHABET[(((b1 & 0x03) << 4) | (b2 >> 4)) as usize] as char);

        if i + 1 < data.len() {
            result.push(ALPHABET[(((b2 & 0x0f) << 2) | (b3 >> 6)) as usize] as char);
        } else {
            result.push('=');
        }

        if i + 2 < data.len() {
            result.push(ALPHABET[(b3 & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }

        i += 3;
    }

    // Add line breaks every 64 characters for PEM format
    let mut formatted = String::new();
    for (idx, ch) in result.chars().enumerate() {
        if idx > 0 && idx % 64 == 0 {
            formatted.push('\n');
        }
        formatted.push(ch);
    }

    formatted
}

// ========================================================================
// Encrypted PEM tests
// ========================================================================

#[test]
fn test_detect_encrypted_pem_unencrypted() {
    let pem = "-----BEGIN PRIVATE KEY-----\nMIIE...\n-----END PRIVATE KEY-----\n";
    assert_eq!(detect_encrypted_pem(pem), EncryptedPemFormat::NotEncrypted);
}

#[test]
fn test_detect_encrypted_pem_pkcs8() {
    let pem =
        "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIE...\n-----END ENCRYPTED PRIVATE KEY-----\n";
    assert_eq!(
        detect_encrypted_pem(pem),
        EncryptedPemFormat::Pkcs8Encrypted
    );
}

#[test]
fn test_detect_encrypted_pem_legacy() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-256-CBC,AABB\n\nMIIE...\n-----END RSA PRIVATE KEY-----\n";
    assert_eq!(
        detect_encrypted_pem(pem),
        EncryptedPemFormat::LegacyEncrypted
    );
}

#[test]
fn test_load_unencrypted_passthrough() {
    // Generate a key, write it unencrypted, then load via load_encrypted_pem_file
    let generator = SelfSignedGenerator::new("test-passthrough");
    let (_cert, key) = generator.generate().expect("Should generate certificate");

    // Serialize the PKCS#8 key to PEM
    let key_der = match &key {
        PrivateKeyDer::Pkcs8(k) => k.secret_pkcs8_der().to_vec(),
        _ => panic!("Expected PKCS#8 key"),
    };
    let pem_content = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        base64_encode(&key_der)
    );

    let temp_path = temp_dir().join("test_unencrypted_passthrough.pem");
    fs::write(&temp_path, &pem_content).expect("Should write temp file");

    let loader = PrivateKeyLoader::new();
    let result = loader.load_encrypted_pem_file(&temp_path, "any_password");

    let _ = fs::remove_file(&temp_path);

    assert!(
        result.is_ok(),
        "Unencrypted key should load with any password: {:?}",
        result.err()
    );
}

#[test]
fn test_encrypted_key_no_password() {
    let pem = "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIFHDBOBgkqhkiG...\n-----END ENCRYPTED PRIVATE KEY-----\n";
    let temp_path = temp_dir().join("test_no_password.pem");
    fs::write(&temp_path, pem).expect("Should write temp file");

    // Clear env var to ensure it's not set
    unsafe { std::env::remove_var("AMATERS_KEY_PASSWORD") };

    let loader = PrivateKeyLoader::new();
    let result = loader.load_encrypted_pem_file(&temp_path, "");

    let _ = fs::remove_file(&temp_path);

    assert!(result.is_err());
    let err_msg = format!("{}", result.expect_err("Should be an error"));
    assert!(
        err_msg.contains("no password"),
        "Error should mention no password: {err_msg}"
    );
}

#[test]
fn test_encrypted_key_empty_password_triggers_env_check() {
    // When password is empty, should check env var
    unsafe { std::env::remove_var("AMATERS_KEY_PASSWORD") };

    let result = resolve_password("");
    assert!(result.is_err());

    // Now set env var
    unsafe { std::env::set_var("AMATERS_KEY_PASSWORD", "test_env_pw") };
    let result = resolve_password("");
    assert!(result.is_ok());
    assert_eq!(result.expect("Should succeed"), "test_env_pw");

    // Clean up
    unsafe { std::env::remove_var("AMATERS_KEY_PASSWORD") };
}

#[test]
fn test_encrypted_key_env_variable() {
    unsafe { std::env::set_var("AMATERS_KEY_PASSWORD", "env_password_123") };

    let result = resolve_password("");
    assert!(result.is_ok());
    assert_eq!(result.expect("Should resolve from env"), "env_password_123");

    // Direct password should take precedence
    let result = resolve_password("direct_pw");
    assert!(result.is_ok());
    assert_eq!(result.expect("Should use direct pw"), "direct_pw");

    unsafe { std::env::remove_var("AMATERS_KEY_PASSWORD") };
}

#[test]
fn test_parse_dek_info_header_aes256() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-256-CBC,AABBCCDD11223344AABBCCDD11223344\n\nbase64data\n-----END RSA PRIVATE KEY-----\n";

    let result = parse_dek_info(pem);
    assert!(result.is_ok(), "Should parse DEK-Info: {:?}", result.err());

    let (algo, iv) = result.expect("Should succeed");
    assert_eq!(algo, "AES-256-CBC");
    assert_eq!(iv.len(), 16);
    assert_eq!(iv[0], 0xAA);
    assert_eq!(iv[1], 0xBB);
}

#[test]
fn test_parse_dek_info_header_aes128() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00112233445566778899AABBCCDDEEFF\n\nbase64data\n-----END RSA PRIVATE KEY-----\n";

    let result = parse_dek_info(pem);
    assert!(result.is_ok());

    let (algo, iv) = result.expect("Should succeed");
    assert_eq!(algo, "AES-128-CBC");
    assert_eq!(iv.len(), 16);
}

#[test]
fn test_parse_dek_info_missing() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\n\nbase64data\n-----END RSA PRIVATE KEY-----\n";

    let result = parse_dek_info(pem);
    assert!(result.is_err());
}

#[test]
fn test_parse_encrypted_pkcs8_format() {
    let pem = "-----BEGIN ENCRYPTED PRIVATE KEY-----\ndata\n-----END ENCRYPTED PRIVATE KEY-----\n";
    assert_eq!(
        detect_encrypted_pem(pem),
        EncryptedPemFormat::Pkcs8Encrypted
    );
}

#[test]
fn test_legacy_encrypted_format_detection() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-256-CBC,0011223344556677\n\nSomeBase64Data\n-----END RSA PRIVATE KEY-----\n";
    assert_eq!(
        detect_encrypted_pem(pem),
        EncryptedPemFormat::LegacyEncrypted
    );
}

#[test]
fn test_key_derivation_pbkdf2_sha256() {
    // Test vector: PBKDF2 with known password and salt
    let password = b"password";
    let salt = b"salt";
    let iterations = 1;
    let key_len = 32;

    let derived = pbkdf2_hmac_sha256(password, salt, iterations, key_len);
    assert_eq!(derived.len(), key_len);

    // Known test vector for PBKDF2-HMAC-SHA256("password", "salt", 1, 32)
    // RFC 7914 / RFC 6070 compatible
    let expected: [u8; 32] = [
        0x12, 0x0f, 0xb6, 0xcf, 0xfc, 0xf8, 0xb3, 0x2c, 0x43, 0xe7, 0x22, 0x52, 0x56, 0xc4, 0xf8,
        0x37, 0xa8, 0x65, 0x48, 0xc9, 0x2c, 0xcc, 0x35, 0x48, 0x08, 0x05, 0x98, 0x7c, 0xb7, 0x0b,
        0xe1, 0x7b,
    ];
    assert_eq!(derived, expected, "PBKDF2-HMAC-SHA256 test vector mismatch");
}

#[test]
fn test_key_derivation_deterministic() {
    let password = b"my_secret";
    let salt = b"random_salt_12345678";
    let iterations = 100;

    let key1 = pbkdf2_hmac_sha256(password, salt, iterations, 32);
    let key2 = pbkdf2_hmac_sha256(password, salt, iterations, 32);
    assert_eq!(key1, key2, "Same inputs should produce same derived key");

    let key3 = pbkdf2_hmac_sha256(b"different", salt, iterations, 32);
    assert_ne!(
        key1, key3,
        "Different passwords should produce different keys"
    );
}

#[test]
fn test_sha256_known_vectors() {
    // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    let empty_hash = sha256(b"");
    assert_eq!(empty_hash[0], 0xe3);
    assert_eq!(empty_hash[1], 0xb0);
    assert_eq!(empty_hash[31], 0x55);

    // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
    let abc_hash = sha256(b"abc");
    assert_eq!(abc_hash[0], 0xba);
    assert_eq!(abc_hash[1], 0x78);
    assert_eq!(abc_hash[31], 0xad);
}

#[test]
fn test_sha1_known_vectors() {
    // SHA-1("") = da39a3ee5e6b4b0d3255bfef95601890afd80709
    let empty_hash = sha1(b"");
    assert_eq!(empty_hash[0], 0xda);
    assert_eq!(empty_hash[1], 0x39);
    assert_eq!(empty_hash[19], 0x09);

    // SHA-1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
    let abc_hash = sha1(b"abc");
    assert_eq!(abc_hash[0], 0xa9);
    assert_eq!(abc_hash[1], 0x99);
    assert_eq!(abc_hash[19], 0x9d);
}

#[test]
fn test_md5_known_vectors() {
    // MD5("") = d41d8cd98f00b204e9800998ecf8427e
    let empty_hash = md5(b"");
    assert_eq!(empty_hash[0], 0xd4);
    assert_eq!(empty_hash[1], 0x1d);
    assert_eq!(empty_hash[15], 0x7e);

    // MD5("abc") = 900150983cd24fb0d6963f7d28e17f72
    let abc_hash = md5(b"abc");
    assert_eq!(abc_hash[0], 0x90);
    assert_eq!(abc_hash[1], 0x01);
    assert_eq!(abc_hash[15], 0x72);
}

#[test]
fn test_aes_cbc_roundtrip() {
    // Test AES-CBC encryption/decryption roundtrip using AES encrypt + our decrypt
    let key = [0x00u8; 32]; // AES-256 key
    let iv = [0x00u8; 16];

    // Create plaintext with PKCS#7 padding (16 bytes data + 16 bytes padding)
    let mut plaintext = vec![0x41u8; 16]; // "AAAAAAAAAAAAAAAA"
    // Add PKCS#7 padding (full block of 0x10)
    plaintext.extend_from_slice(&[0x10u8; 16]);

    // Manually encrypt with AES-CBC for test
    let round_keys = aes_key_expansion(&key).expect("Key expansion should work");
    let mut ciphertext = Vec::new();
    let mut prev_block = iv;

    for chunk in plaintext.chunks_exact(16) {
        let mut block = [0u8; 16];
        for i in 0..16 {
            block[i] = chunk[i] ^ prev_block[i];
        }
        let encrypted = aes_encrypt_block_for_test(&block, &round_keys);
        ciphertext.extend_from_slice(&encrypted);
        prev_block = encrypted;
    }

    // Now decrypt
    let decrypted = aes_cbc_decrypt(&ciphertext, &key, &iv).expect("Decryption should work");
    let unpadded = remove_pkcs7_padding(&decrypted).expect("Padding removal should work");

    assert_eq!(unpadded, &[0x41u8; 16]);
}

/// AES encrypt block (for test roundtrip only)
fn aes_encrypt_block_for_test(block: &[u8; 16], round_keys: &[[u8; 4]]) -> [u8; 16] {
    let nr = round_keys.len() / 4 - 1;
    let mut state = [[0u8; 4]; 4];

    for c in 0..4 {
        for r in 0..4 {
            state[r][c] = block[c * 4 + r];
        }
    }

    // Initial round key addition
    for c in 0..4 {
        for r in 0..4 {
            state[r][c] ^= round_keys[c][r];
        }
    }

    for round in 1..nr {
        // SubBytes
        for row in state.iter_mut() {
            for val in row.iter_mut() {
                *val = AES_SBOX[*val as usize];
            }
        }
        // ShiftRows
        shift_rows_for_test(&mut state);
        // MixColumns
        mix_columns_for_test(&mut state);
        // AddRoundKey
        for c in 0..4 {
            for r in 0..4 {
                state[r][c] ^= round_keys[round * 4 + c][r];
            }
        }
    }

    // Final round (no MixColumns)
    for row in state.iter_mut() {
        for val in row.iter_mut() {
            *val = AES_SBOX[*val as usize];
        }
    }
    shift_rows_for_test(&mut state);
    for c in 0..4 {
        for r in 0..4 {
            state[r][c] ^= round_keys[nr * 4 + c][r];
        }
    }

    let mut output = [0u8; 16];
    for c in 0..4 {
        for r in 0..4 {
            output[c * 4 + r] = state[r][c];
        }
    }
    output
}

fn shift_rows_for_test(state: &mut [[u8; 4]; 4]) {
    // Row 1: shift left by 1
    let tmp = state[1][0];
    state[1][0] = state[1][1];
    state[1][1] = state[1][2];
    state[1][2] = state[1][3];
    state[1][3] = tmp;
    // Row 2: shift left by 2
    let (t0, t1) = (state[2][0], state[2][1]);
    state[2][0] = state[2][2];
    state[2][1] = state[2][3];
    state[2][2] = t0;
    state[2][3] = t1;
    // Row 3: shift left by 3
    let tmp = state[3][3];
    state[3][3] = state[3][2];
    state[3][2] = state[3][1];
    state[3][1] = state[3][0];
    state[3][0] = tmp;
}

#[allow(clippy::needless_range_loop)]
fn mix_columns_for_test(state: &mut [[u8; 4]; 4]) {
    for c in 0..4 {
        let s0 = state[0][c];
        let s1 = state[1][c];
        let s2 = state[2][c];
        let s3 = state[3][c];

        state[0][c] = gf_mul(s0, 2) ^ gf_mul(s1, 3) ^ s2 ^ s3;
        state[1][c] = s0 ^ gf_mul(s1, 2) ^ gf_mul(s2, 3) ^ s3;
        state[2][c] = s0 ^ s1 ^ gf_mul(s2, 2) ^ gf_mul(s3, 3);
        state[3][c] = gf_mul(s0, 3) ^ s1 ^ s2 ^ gf_mul(s3, 2);
    }
}

#[test]
fn test_hex_decode_valid() {
    let result = hex_decode("AABBCCDD");
    assert!(result.is_ok());
    assert_eq!(result.expect("hex"), vec![0xAA, 0xBB, 0xCC, 0xDD]);
}

#[test]
fn test_hex_decode_invalid() {
    assert!(hex_decode("GG").is_err());
    assert!(hex_decode("A").is_err()); // odd length
}

#[test]
fn test_base64_decode_roundtrip() {
    let original = b"Hello, World!";
    let encoded = base64_encode(original);
    let decoded = base64_decode_pure(&encoded).expect("Should decode");
    assert_eq!(decoded, original);
}

#[test]
fn test_pkcs7_padding_removal() {
    // Valid padding: last byte is 0x04, and last 4 bytes are all 0x04
    let mut data = vec![0x41; 12];
    data.extend_from_slice(&[0x04, 0x04, 0x04, 0x04]);
    let result = remove_pkcs7_padding(&data);
    assert!(result.is_ok());
    assert_eq!(result.expect("unpadded").len(), 12);

    // Invalid padding
    let bad_data = vec![0x41; 16];
    // Last byte is 0x41 = 65, which is > 16
    let result = remove_pkcs7_padding(&bad_data);
    assert!(result.is_err());
}

#[test]
fn test_encrypted_key_wrong_password() {
    // Create a fake PKCS#8 encrypted PEM with invalid data
    // The decryption should fail with a descriptive error about wrong password
    let fake_encrypted_data: Vec<u8> = [0xDE, 0xAD, 0xBE, 0xEF]
        .iter()
        .copied()
        .cycle()
        .take(64)
        .collect();
    let encoded = base64_encode(&fake_encrypted_data);
    let pem = format!(
        "-----BEGIN ENCRYPTED PRIVATE KEY-----\n{encoded}\n-----END ENCRYPTED PRIVATE KEY-----\n"
    );

    let temp_path = temp_dir().join("test_wrong_password.pem");
    fs::write(&temp_path, &pem).expect("Should write temp file");

    let loader = PrivateKeyLoader::new();
    let result = loader.load_encrypted_pem_file(&temp_path, "wrong_password");

    let _ = fs::remove_file(&temp_path);

    // Should fail because the data is not valid ASN.1
    assert!(result.is_err());
}

#[test]
fn test_evp_bytes_to_key_deterministic() {
    let password = b"test_password";
    let salt = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];

    let key1 = evp_bytes_to_key_md5(password, &salt, 32);
    let key2 = evp_bytes_to_key_md5(password, &salt, 32);
    assert_eq!(key1, key2, "Same inputs should produce same key");
    assert_eq!(key1.len(), 32);

    let key3 = evp_bytes_to_key_md5(b"different", &salt, 32);
    assert_ne!(key1, key3);
}

#[test]
fn test_load_encrypted_pem_roundtrip() {
    // Generate a key, encrypt it with AES-CBC, write as legacy encrypted PEM, load back
    let generator = SelfSignedGenerator::new("roundtrip-test");
    let (_cert, key) = generator.generate().expect("Should generate certificate");

    let key_der = match &key {
        PrivateKeyDer::Pkcs8(k) => k.secret_pkcs8_der().to_vec(),
        _ => panic!("Expected PKCS#8 key"),
    };

    let password = b"test_roundtrip_pw";
    let iv = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
        0x10,
    ];

    // Derive key using EVP_BytesToKey
    let aes_key = evp_bytes_to_key_md5(password, &iv[..8], 32);

    // Add PKCS#7 padding
    let pad_len = 16 - (key_der.len() % 16);
    let mut padded = key_der.clone();
    for _ in 0..pad_len {
        padded.push(pad_len as u8);
    }

    // Encrypt with AES-256-CBC
    let round_keys = aes_key_expansion(&aes_key).expect("Key expansion should work");
    let mut ciphertext = Vec::new();
    let mut prev_block = iv;

    for chunk in padded.chunks_exact(16) {
        let mut block = [0u8; 16];
        for i in 0..16 {
            block[i] = chunk[i] ^ prev_block[i];
        }
        let encrypted = aes_encrypt_block_for_test(&block, &round_keys);
        ciphertext.extend_from_slice(&encrypted);
        prev_block = encrypted;
    }

    // Format IV as hex
    let iv_hex: String = iv.iter().map(|b| format!("{b:02X}")).collect();

    // Build legacy encrypted PEM
    let b64_body = base64_encode(&ciphertext);
    let pem = format!(
        "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-256-CBC,{iv_hex}\n\n{b64_body}\n-----END RSA PRIVATE KEY-----\n"
    );

    let temp_path = temp_dir().join("test_encrypted_roundtrip.pem");
    fs::write(&temp_path, &pem).expect("Should write temp file");

    let loader = PrivateKeyLoader::new();
    let result = loader.load_encrypted_pem_file(
        &temp_path,
        std::str::from_utf8(password).expect("valid utf8"),
    );

    let _ = fs::remove_file(&temp_path);

    assert!(
        result.is_ok(),
        "Encrypted PEM roundtrip should succeed: {:?}",
        result.err()
    );

    // Verify the decrypted key matches the original
    let loaded_key = result.expect("Should succeed");
    match &loaded_key {
        PrivateKeyDer::Pkcs8(k) => {
            assert_eq!(
                k.secret_pkcs8_der(),
                key_der.as_slice(),
                "Decrypted key should match original"
            );
        }
        PrivateKeyDer::Pkcs1(k) => {
            // The key_der is PKCS#8, but since we wrote it as legacy RSA PEM
            // and is_pkcs8_key_der should detect it, it should come back as PKCS#8
            // However, if it's recognized as PKCS#1, the raw DER should still be valid
            assert!(
                !k.secret_pkcs1_der().is_empty(),
                "Decrypted key should not be empty"
            );
        }
        _ => panic!("Unexpected key type"),
    }
}

#[test]
fn test_asn1_length_parsing() {
    // Short form: length < 128
    let data = [0x05]; // length 5
    let (len, consumed) = parse_asn1_length(&data).expect("Should parse");
    assert_eq!(len, 5);
    assert_eq!(consumed, 1);

    // Long form: 1 byte length
    let data = [0x81, 0x80]; // length 128
    let (len, consumed) = parse_asn1_length(&data).expect("Should parse");
    assert_eq!(len, 128);
    assert_eq!(consumed, 2);

    // Long form: 2 byte length
    let data = [0x82, 0x01, 0x00]; // length 256
    let (len, consumed) = parse_asn1_length(&data).expect("Should parse");
    assert_eq!(len, 256);
    assert_eq!(consumed, 3);
}

#[test]
fn test_pbkdf2_hmac_sha1_basic() {
    // Test vector from RFC 6070: PBKDF2-HMAC-SHA1("password", "salt", 1, 20)
    let derived = pbkdf2_hmac_sha1(b"password", b"salt", 1, 20);
    assert_eq!(derived.len(), 20);

    let expected: [u8; 20] = [
        0x0c, 0x60, 0xc8, 0x0f, 0x96, 0x1f, 0x0e, 0x71, 0xf3, 0xa9, 0xb5, 0x24, 0xaf, 0x60, 0x12,
        0x06, 0x2f, 0xe0, 0x37, 0xa6,
    ];
    assert_eq!(derived, expected, "PBKDF2-HMAC-SHA1 test vector mismatch");
}
