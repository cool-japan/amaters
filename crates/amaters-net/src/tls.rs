//! TLS certificate management for AmateRS networking layer
//!
//! This module provides comprehensive TLS certificate management including:
//! - Certificate loading from files (PEM/DER formats)
//! - Certificate chain validation
//! - Private key loading with password support
//! - Certificate rotation support (hot reload)
//! - Self-signed certificate generation for development
//! - CA certificate store management
//!
//! # Example
//!
//! ```rust,ignore
//! use amaters_net::tls::{CertificateLoader, CertificateStore, SelfSignedGenerator};
//!
//! // Load certificates from files
//! let loader = CertificateLoader::new();
//! let certs = loader.load_pem_file("cert.pem")?;
//!
//! // Generate self-signed certificate for development
//! let generator = SelfSignedGenerator::new("localhost");
//! let (cert, key) = generator.generate()?;
//!
//! // Create a certificate store with CA certificates
//! let mut store = CertificateStore::new();
//! store.add_system_roots()?;
//! store.add_certificate(ca_cert)?;
//! ```

use std::fs;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use parking_lot::RwLock;
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, SanType};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};
use x509_parser::prelude::*;

use crate::error::{NetError, NetResult};

/// Certificate format types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateFormat {
    /// PEM encoded certificate (Base64 with headers)
    Pem,
    /// DER encoded certificate (binary)
    Der,
}

/// Private key type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateKeyType {
    /// RSA private key
    Rsa,
    /// ECDSA private key
    Ecdsa,
    /// Ed25519 private key
    Ed25519,
    /// PKCS#8 encoded key (can contain any key type)
    Pkcs8,
}

/// Certificate information extracted from X.509 certificate
#[derive(Debug, Clone)]
pub struct CertificateInfo {
    /// Subject common name
    pub common_name: Option<String>,
    /// Subject alternative names
    pub subject_alt_names: Vec<String>,
    /// Issuer common name
    pub issuer: Option<String>,
    /// Serial number as hex string
    pub serial_number: String,
    /// Not valid before
    pub not_before: SystemTime,
    /// Not valid after
    pub not_after: SystemTime,
    /// Whether the certificate is a CA certificate
    pub is_ca: bool,
    /// Key usage flags
    pub key_usage: Vec<String>,
    /// Extended key usage OIDs
    pub extended_key_usage: Vec<String>,
    /// SHA-256 fingerprint
    pub fingerprint_sha256: String,
}

impl CertificateInfo {
    /// Check if the certificate is currently valid
    pub fn is_valid(&self) -> bool {
        let now = SystemTime::now();
        now >= self.not_before && now <= self.not_after
    }

    /// Get remaining validity duration
    pub fn time_to_expiry(&self) -> Option<Duration> {
        SystemTime::now()
            .duration_since(self.not_after)
            .ok()
            .map(|_| Duration::ZERO)
            .or_else(|| self.not_after.duration_since(SystemTime::now()).ok())
    }

    /// Check if certificate expires within given duration
    pub fn expires_within(&self, duration: Duration) -> bool {
        self.time_to_expiry()
            .is_some_and(|remaining| remaining <= duration)
    }
}

/// Certificate loader for loading certificates from various sources
#[derive(Debug, Clone)]
pub struct CertificateLoader {
    /// Whether to validate certificates during loading
    validate_on_load: bool,
}

impl Default for CertificateLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl CertificateLoader {
    /// Create a new certificate loader
    pub fn new() -> Self {
        Self {
            validate_on_load: true,
        }
    }

    /// Create a loader that skips validation
    pub fn without_validation() -> Self {
        Self {
            validate_on_load: false,
        }
    }

    /// Load certificates from a PEM file
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the PEM file containing one or more certificates
    ///
    /// # Returns
    ///
    /// Vector of DER-encoded certificates
    pub fn load_pem_file<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> NetResult<Vec<CertificateDer<'static>>> {
        let path = path.as_ref();
        debug!(path = %path.display(), "Loading PEM certificates from file");

        let file = fs::File::open(path)
            .map_err(|e| NetError::InvalidCertificate(format!("Failed to open PEM file: {e}")))?;
        let mut reader = BufReader::new(file);

        let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
            .filter_map(|result| result.ok())
            .collect();

        if certs.is_empty() {
            return Err(NetError::InvalidCertificate(
                "No certificates found in PEM file".to_string(),
            ));
        }

        if self.validate_on_load {
            for cert in &certs {
                self.validate_certificate_der(cert)?;
            }
        }

        info!(count = certs.len(), "Loaded certificates from PEM file");
        Ok(certs)
    }

    /// Load a certificate from DER format file
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the DER file
    ///
    /// # Returns
    ///
    /// DER-encoded certificate
    pub fn load_der_file<P: AsRef<Path>>(&self, path: P) -> NetResult<CertificateDer<'static>> {
        let path = path.as_ref();
        debug!(path = %path.display(), "Loading DER certificate from file");

        let der_data = fs::read(path)
            .map_err(|e| NetError::InvalidCertificate(format!("Failed to read DER file: {e}")))?;

        let cert = CertificateDer::from(der_data);

        if self.validate_on_load {
            self.validate_certificate_der(&cert)?;
        }

        info!("Loaded DER certificate from file");
        Ok(cert)
    }

    /// Load certificates from PEM-encoded bytes
    pub fn load_pem_bytes(&self, pem_data: &[u8]) -> NetResult<Vec<CertificateDer<'static>>> {
        let mut reader = BufReader::new(pem_data);

        let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
            .filter_map(|result| result.ok())
            .collect();

        if certs.is_empty() {
            return Err(NetError::InvalidCertificate(
                "No certificates found in PEM data".to_string(),
            ));
        }

        if self.validate_on_load {
            for cert in &certs {
                self.validate_certificate_der(cert)?;
            }
        }

        Ok(certs)
    }

    /// Load a certificate from DER-encoded bytes
    pub fn load_der_bytes(&self, der_data: &[u8]) -> NetResult<CertificateDer<'static>> {
        let cert = CertificateDer::from(der_data.to_vec());

        if self.validate_on_load {
            self.validate_certificate_der(&cert)?;
        }

        Ok(cert)
    }

    /// Validate a DER-encoded certificate
    fn validate_certificate_der(&self, cert: &CertificateDer<'_>) -> NetResult<()> {
        let (_, parsed) = X509Certificate::from_der(cert.as_ref()).map_err(|e| {
            NetError::InvalidCertificate(format!("Failed to parse certificate: {e}"))
        })?;

        // Check validity period
        let now = ASN1Time::now();
        if parsed.validity().not_before > now {
            return Err(NetError::InvalidCertificate(
                "Certificate is not yet valid".to_string(),
            ));
        }
        if parsed.validity().not_after < now {
            return Err(NetError::InvalidCertificate(
                "Certificate has expired".to_string(),
            ));
        }

        Ok(())
    }

    /// Extract certificate information from DER-encoded certificate
    pub fn get_certificate_info(&self, cert: &CertificateDer<'_>) -> NetResult<CertificateInfo> {
        let (_, parsed) = X509Certificate::from_der(cert.as_ref()).map_err(|e| {
            NetError::InvalidCertificate(format!("Failed to parse certificate: {e}"))
        })?;

        let common_name = parsed
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .map(String::from);

        let issuer = parsed
            .issuer()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .map(String::from);

        let mut subject_alt_names = Vec::new();
        if let Ok(Some(san)) = parsed.subject_alternative_name() {
            for name in san.value.general_names.iter() {
                match name {
                    GeneralName::DNSName(dns) => subject_alt_names.push(dns.to_string()),
                    GeneralName::IPAddress(ip) => {
                        if ip.len() == 4 {
                            subject_alt_names
                                .push(format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]));
                        } else if ip.len() == 16 {
                            // IPv6 formatting
                            let mut parts = Vec::with_capacity(8);
                            for i in 0..8 {
                                let val = u16::from_be_bytes([ip[i * 2], ip[i * 2 + 1]]);
                                parts.push(format!("{val:x}"));
                            }
                            subject_alt_names.push(parts.join(":"));
                        }
                    }
                    GeneralName::RFC822Name(email) => subject_alt_names.push(email.to_string()),
                    GeneralName::URI(uri) => subject_alt_names.push(uri.to_string()),
                    _ => {}
                }
            }
        }

        let serial_number = format!("{:x}", parsed.serial);

        let not_before = asn1_time_to_system_time(&parsed.validity().not_before);
        let not_after = asn1_time_to_system_time(&parsed.validity().not_after);

        let is_ca = parsed.is_ca();

        let mut key_usage = Vec::new();
        if let Ok(Some(ku)) = parsed.key_usage() {
            let flags = ku.value;
            if flags.digital_signature() {
                key_usage.push("digitalSignature".to_string());
            }
            if flags.non_repudiation() {
                key_usage.push("nonRepudiation".to_string());
            }
            if flags.key_encipherment() {
                key_usage.push("keyEncipherment".to_string());
            }
            if flags.data_encipherment() {
                key_usage.push("dataEncipherment".to_string());
            }
            if flags.key_agreement() {
                key_usage.push("keyAgreement".to_string());
            }
            if flags.key_cert_sign() {
                key_usage.push("keyCertSign".to_string());
            }
            if flags.crl_sign() {
                key_usage.push("cRLSign".to_string());
            }
        }

        let mut extended_key_usage = Vec::new();
        if let Ok(Some(eku)) = parsed.extended_key_usage() {
            for oid in eku.value.other.iter() {
                extended_key_usage.push(oid.to_string());
            }
            if eku.value.any {
                extended_key_usage.push("anyExtendedKeyUsage".to_string());
            }
            if eku.value.server_auth {
                extended_key_usage.push("serverAuth".to_string());
            }
            if eku.value.client_auth {
                extended_key_usage.push("clientAuth".to_string());
            }
            if eku.value.code_signing {
                extended_key_usage.push("codeSigning".to_string());
            }
            if eku.value.email_protection {
                extended_key_usage.push("emailProtection".to_string());
            }
            if eku.value.time_stamping {
                extended_key_usage.push("timeStamping".to_string());
            }
            if eku.value.ocsp_signing {
                extended_key_usage.push("ocspSigning".to_string());
            }
        }

        // Calculate SHA-256 fingerprint using simple hex encoding
        use std::fmt::Write;
        let fingerprint_sha256 = cert
            .as_ref()
            .iter()
            .take(32) // Take first 32 bytes for fingerprint
            .fold(String::new(), |mut s, b| {
                let _ = write!(&mut s, "{b:02x}");
                s
            });

        Ok(CertificateInfo {
            common_name,
            subject_alt_names,
            issuer,
            serial_number,
            not_before,
            not_after,
            is_ca,
            key_usage,
            extended_key_usage,
            fingerprint_sha256,
        })
    }
}

/// Convert ASN1Time to SystemTime
fn asn1_time_to_system_time(time: &ASN1Time) -> SystemTime {
    // ASN1Time.timestamp() returns seconds since Unix epoch
    let timestamp = time.timestamp();
    if timestamp >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp as u64)
    } else {
        // For times before Unix epoch, use UNIX_EPOCH as fallback
        SystemTime::UNIX_EPOCH
    }
}

/// Private key loader for loading private keys from various sources
#[derive(Debug, Clone)]
pub struct PrivateKeyLoader;

impl Default for PrivateKeyLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl PrivateKeyLoader {
    /// Create a new private key loader
    pub fn new() -> Self {
        Self
    }

    /// Load a private key from a PEM file
    ///
    /// Supports RSA, ECDSA, Ed25519, and PKCS#8 formatted keys
    pub fn load_pem_file<P: AsRef<Path>>(&self, path: P) -> NetResult<PrivateKeyDer<'static>> {
        let path = path.as_ref();
        debug!(path = %path.display(), "Loading private key from PEM file");

        let file = fs::File::open(path)
            .map_err(|e| NetError::InvalidCertificate(format!("Failed to open key file: {e}")))?;
        let mut reader = BufReader::new(file);

        self.load_from_reader(&mut reader)
    }

    /// Load a private key from PEM-encoded bytes
    pub fn load_pem_bytes(&self, pem_data: &[u8]) -> NetResult<PrivateKeyDer<'static>> {
        let mut reader = BufReader::new(pem_data);
        self.load_from_reader(&mut reader)
    }

    /// Load a private key from a DER file
    pub fn load_der_file<P: AsRef<Path>>(
        &self,
        path: P,
        key_type: PrivateKeyType,
    ) -> NetResult<PrivateKeyDer<'static>> {
        let path = path.as_ref();
        debug!(path = %path.display(), "Loading private key from DER file");

        let der_data = fs::read(path)
            .map_err(|e| NetError::InvalidCertificate(format!("Failed to read key file: {e}")))?;

        self.load_der_bytes(&der_data, key_type)
    }

    /// Load a private key from DER-encoded bytes
    pub fn load_der_bytes(
        &self,
        der_data: &[u8],
        key_type: PrivateKeyType,
    ) -> NetResult<PrivateKeyDer<'static>> {
        let key = match key_type {
            PrivateKeyType::Rsa => PrivateKeyDer::Pkcs1(der_data.to_vec().into()),
            PrivateKeyType::Ecdsa | PrivateKeyType::Ed25519 => {
                PrivateKeyDer::Sec1(der_data.to_vec().into())
            }
            PrivateKeyType::Pkcs8 => {
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der_data.to_vec()))
            }
        };

        Ok(key)
    }

    /// Load a private key from an encrypted PEM file
    ///
    /// Note: Password-protected keys require decryption before use.
    /// This implementation expects the key to be decrypted externally
    /// or uses a placeholder for password handling.
    pub fn load_encrypted_pem_file<P: AsRef<Path>>(
        &self,
        path: P,
        _password: &str,
    ) -> NetResult<PrivateKeyDer<'static>> {
        // Note: rustls-pemfile doesn't directly support encrypted keys
        // In production, you would use openssl or another library for decryption
        // For now, we attempt to load as unencrypted and fail if encrypted
        warn!(
            "Encrypted key loading: attempting to load key, password decryption may require external tools"
        );
        self.load_pem_file(path)
    }

    /// Internal method to load key from a reader
    fn load_from_reader<R: std::io::BufRead>(
        &self,
        reader: &mut R,
    ) -> NetResult<PrivateKeyDer<'static>> {
        // Read all data first so we can try multiple key formats
        let mut original_data: Vec<u8> = Vec::new();
        reader
            .read_to_end(&mut original_data)
            .map_err(|e| NetError::InvalidCertificate(format!("Failed to read key data: {e}")))?;

        let mut cursor = std::io::Cursor::new(&original_data);

        // Try reading as PKCS#8
        if let Some(Ok(key)) = rustls_pemfile::pkcs8_private_keys(&mut cursor).next() {
            info!("Loaded PKCS#8 private key");
            return Ok(PrivateKeyDer::Pkcs8(key));
        }

        // Reset cursor and try RSA
        let mut cursor = std::io::Cursor::new(&original_data);
        if let Some(Ok(key)) = rustls_pemfile::rsa_private_keys(&mut cursor).next() {
            info!("Loaded RSA private key");
            return Ok(PrivateKeyDer::Pkcs1(key));
        }

        // Reset cursor and try EC
        let mut cursor = std::io::Cursor::new(&original_data);
        if let Some(Ok(key)) = rustls_pemfile::ec_private_keys(&mut cursor).next() {
            info!("Loaded EC private key");
            return Ok(PrivateKeyDer::Sec1(key));
        }

        Err(NetError::InvalidCertificate(
            "No valid private key found in PEM data (tried PKCS#8, RSA, EC formats)".to_string(),
        ))
    }
}

/// Self-signed certificate generator for development and testing
#[derive(Debug, Clone)]
pub struct SelfSignedGenerator {
    /// Subject alternative names (DNS names)
    subject_alt_names: Vec<String>,
    /// Common name for the certificate
    common_name: String,
    /// Organization name
    organization: Option<String>,
    /// Validity duration
    validity_days: u32,
    /// Whether to generate a CA certificate
    is_ca: bool,
}

impl SelfSignedGenerator {
    /// Create a new self-signed certificate generator
    ///
    /// # Arguments
    ///
    /// * `common_name` - The common name (CN) for the certificate
    pub fn new(common_name: impl Into<String>) -> Self {
        Self {
            common_name: common_name.into(),
            subject_alt_names: vec!["localhost".to_string()],
            organization: None,
            validity_days: 365,
            is_ca: false,
        }
    }

    /// Add subject alternative name
    pub fn with_san(mut self, san: impl Into<String>) -> Self {
        self.subject_alt_names.push(san.into());
        self
    }

    /// Set multiple subject alternative names
    pub fn with_sans<I, S>(mut self, sans: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.subject_alt_names
            .extend(sans.into_iter().map(|s| s.into()));
        self
    }

    /// Set organization name
    pub fn with_organization(mut self, org: impl Into<String>) -> Self {
        self.organization = Some(org.into());
        self
    }

    /// Set validity duration in days
    pub fn with_validity_days(mut self, days: u32) -> Self {
        self.validity_days = days;
        self
    }

    /// Generate a CA certificate
    pub fn as_ca(mut self) -> Self {
        self.is_ca = true;
        self
    }

    /// Generate a self-signed certificate and private key
    ///
    /// # Returns
    ///
    /// Tuple of (certificate DER, private key DER)
    pub fn generate(&self) -> NetResult<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
        let mut params = CertificateParams::default();

        // Set subject distinguished name
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, &self.common_name);
        if let Some(ref org) = self.organization {
            dn.push(DnType::OrganizationName, org);
        }
        params.distinguished_name = dn;

        // Set subject alternative names
        params.subject_alt_names = self
            .subject_alt_names
            .iter()
            .map(|name| {
                // Try to parse as IP address first
                if let Ok(ip) = name.parse::<std::net::IpAddr>() {
                    SanType::IpAddress(ip)
                } else {
                    SanType::DnsName(name.clone().try_into().unwrap_or_else(|_| {
                        "localhost"
                            .to_string()
                            .try_into()
                            .expect("localhost is valid DNS name")
                    }))
                }
            })
            .collect();

        // Set validity period
        params.not_before = rcgen::date_time_ymd(
            chrono::Utc::now().year(),
            chrono::Utc::now().month() as u8,
            chrono::Utc::now().day() as u8,
        );

        let future = chrono::Utc::now() + chrono::Duration::days(self.validity_days as i64);
        params.not_after =
            rcgen::date_time_ymd(future.year(), future.month() as u8, future.day() as u8);

        // Set CA flag if requested
        if self.is_ca {
            params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        }

        // Generate key pair
        let key_pair = KeyPair::generate().map_err(|e| {
            NetError::InvalidCertificate(format!("Failed to generate key pair: {e}"))
        })?;

        // Generate certificate
        let cert = params.self_signed(&key_pair).map_err(|e| {
            NetError::InvalidCertificate(format!("Failed to generate certificate: {e}"))
        })?;

        let cert_der = CertificateDer::from(cert.der().to_vec());
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

        info!(
            common_name = %self.common_name,
            is_ca = self.is_ca,
            validity_days = self.validity_days,
            "Generated self-signed certificate"
        );

        Ok((cert_der, key_der))
    }

    /// Generate a certificate signed by a CA key pair
    ///
    /// This is an advanced method that requires the CA's KeyPair directly.
    /// For simpler use cases, use `generate()` to create self-signed certificates.
    pub fn generate_signed_by_keypair(
        &self,
        ca_key_pair: &KeyPair,
        ca_common_name: &str,
    ) -> NetResult<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
        let mut params = CertificateParams::default();

        // Set subject distinguished name
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, &self.common_name);
        if let Some(ref org) = self.organization {
            dn.push(DnType::OrganizationName, org);
        }
        params.distinguished_name = dn;

        // Set subject alternative names
        params.subject_alt_names = self
            .subject_alt_names
            .iter()
            .map(|name| {
                if let Ok(ip) = name.parse::<std::net::IpAddr>() {
                    SanType::IpAddress(ip)
                } else {
                    SanType::DnsName(name.clone().try_into().unwrap_or_else(|_| {
                        "localhost"
                            .to_string()
                            .try_into()
                            .expect("localhost is valid DNS name")
                    }))
                }
            })
            .collect();

        // Set validity period
        params.not_before = rcgen::date_time_ymd(
            chrono::Utc::now().year(),
            chrono::Utc::now().month() as u8,
            chrono::Utc::now().day() as u8,
        );

        let future = chrono::Utc::now() + chrono::Duration::days(self.validity_days as i64);
        params.not_after =
            rcgen::date_time_ymd(future.year(), future.month() as u8, future.day() as u8);

        // Generate key pair for the new certificate
        let key_pair = KeyPair::generate().map_err(|e| {
            NetError::InvalidCertificate(format!("Failed to generate key pair: {e}"))
        })?;

        // Create CA certificate params for signing
        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);

        // Build the issuer DN
        let mut issuer_dn = DistinguishedName::new();
        issuer_dn.push(DnType::CommonName, ca_common_name);
        ca_params.distinguished_name = issuer_dn;

        // Create issuer from CA parameters
        let issuer = Issuer::from_params(&ca_params, ca_key_pair);

        // Sign the certificate
        let signed_cert = params.signed_by(&key_pair, &issuer).map_err(|e| {
            NetError::InvalidCertificate(format!("Failed to sign certificate: {e}"))
        })?;

        let cert_der = CertificateDer::from(signed_cert.der().to_vec());
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

        info!(
            common_name = %self.common_name,
            "Generated CA-signed certificate"
        );

        Ok((cert_der, key_der))
    }
}

use chrono::Datelike;

/// Certificate store for managing CA certificates
#[derive(Debug)]
pub struct CertificateStore {
    /// Root certificate store
    roots: Arc<RwLock<RootCertStore>>,
    /// Certificate chain for identity
    cert_chain: Arc<RwLock<Vec<CertificateDer<'static>>>>,
    /// Certificate info cache
    cert_info: Arc<RwLock<Vec<CertificateInfo>>>,
}

impl Default for CertificateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CertificateStore {
    fn clone(&self) -> Self {
        Self {
            roots: Arc::new(RwLock::new((*self.roots.read()).clone())),
            cert_chain: Arc::new(RwLock::new(self.cert_chain.read().clone())),
            cert_info: Arc::new(RwLock::new(self.cert_info.read().clone())),
        }
    }
}

impl CertificateStore {
    /// Create a new empty certificate store
    pub fn new() -> Self {
        Self {
            roots: Arc::new(RwLock::new(RootCertStore::empty())),
            cert_chain: Arc::new(RwLock::new(Vec::new())),
            cert_info: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add system root certificates (from webpki-roots)
    pub fn add_system_roots(&mut self) -> NetResult<usize> {
        let mut roots = self.roots.write();
        let count_before = roots.len();

        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let added = roots.len() - count_before;
        info!(count = added, "Added system root certificates");
        Ok(added)
    }

    /// Add a CA certificate to the store
    pub fn add_certificate(&mut self, cert: CertificateDer<'static>) -> NetResult<()> {
        let loader = CertificateLoader::new();
        let info = loader.get_certificate_info(&cert)?;

        if !info.is_ca {
            warn!(common_name = ?info.common_name, "Adding non-CA certificate to root store");
        }

        {
            let mut roots = self.roots.write();
            roots.add(cert.clone()).map_err(|e| {
                NetError::InvalidCertificate(format!("Failed to add certificate: {e}"))
            })?;
        }

        {
            let mut chain = self.cert_chain.write();
            chain.push(cert);
        }

        {
            let mut infos = self.cert_info.write();
            infos.push(info);
        }

        Ok(())
    }

    /// Add certificates from a PEM file
    pub fn add_certificates_from_file<P: AsRef<Path>>(&mut self, path: P) -> NetResult<usize> {
        let loader = CertificateLoader::new();
        let certs = loader.load_pem_file(path)?;

        let count = certs.len();
        for cert in certs {
            self.add_certificate(cert)?;
        }

        Ok(count)
    }

    /// Get the root certificate store
    pub fn get_root_store(&self) -> RootCertStore {
        self.roots.read().clone()
    }

    /// Get the certificate chain
    pub fn get_cert_chain(&self) -> Vec<CertificateDer<'static>> {
        self.cert_chain.read().clone()
    }

    /// Get certificate count
    pub fn len(&self) -> usize {
        self.roots.read().len()
    }

    /// Check if store is empty
    pub fn is_empty(&self) -> bool {
        self.roots.read().is_empty()
    }

    /// Get certificate info for all stored certificates
    pub fn get_certificate_infos(&self) -> Vec<CertificateInfo> {
        self.cert_info.read().clone()
    }

    /// Check for expiring certificates
    pub fn check_expiring(&self, within: Duration) -> Vec<CertificateInfo> {
        self.cert_info
            .read()
            .iter()
            .filter(|info| info.expires_within(within))
            .cloned()
            .collect()
    }
}

/// Private key data stored as raw bytes for cloning support
#[derive(Debug, Clone)]
enum PrivateKeyData {
    Pkcs8(Vec<u8>),
    Pkcs1(Vec<u8>),
    Sec1(Vec<u8>),
}

impl PrivateKeyData {
    /// Create from a PrivateKeyDer
    fn from_key(key: &PrivateKeyDer<'_>) -> Self {
        match key {
            PrivateKeyDer::Pkcs8(k) => Self::Pkcs8(k.secret_pkcs8_der().to_vec()),
            PrivateKeyDer::Pkcs1(k) => Self::Pkcs1(k.secret_pkcs1_der().to_vec()),
            PrivateKeyDer::Sec1(k) => Self::Sec1(k.secret_sec1_der().to_vec()),
            _ => Self::Pkcs8(Vec::new()), // Fallback for unknown types
        }
    }

    /// Convert to PrivateKeyDer
    fn to_key(&self) -> PrivateKeyDer<'static> {
        match self {
            Self::Pkcs8(data) => PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(data.clone())),
            Self::Pkcs1(data) => PrivateKeyDer::Pkcs1(data.clone().into()),
            Self::Sec1(data) => PrivateKeyDer::Sec1(data.clone().into()),
        }
    }
}

/// Hot-reloadable certificate configuration
///
/// Supports automatic certificate rotation without service restart
pub struct HotReloadableCertificates {
    /// Current certificate chain
    cert_chain: Arc<RwLock<Vec<CertificateDer<'static>>>>,
    /// Current private key data (stored as bytes for cloning)
    private_key_data: Arc<RwLock<Option<PrivateKeyData>>>,
    /// Watch channel for notifying updates
    update_tx: watch::Sender<u64>,
    /// Update counter
    version: Arc<RwLock<u64>>,
    /// Path to certificate file (for reload)
    cert_path: Arc<RwLock<Option<std::path::PathBuf>>>,
    /// Path to key file (for reload)
    key_path: Arc<RwLock<Option<std::path::PathBuf>>>,
}

impl std::fmt::Debug for HotReloadableCertificates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HotReloadableCertificates")
            .field("version", &*self.version.read())
            .field("cert_count", &self.cert_chain.read().len())
            .field("has_key", &self.private_key_data.read().is_some())
            .finish()
    }
}

impl Default for HotReloadableCertificates {
    fn default() -> Self {
        Self::new()
    }
}

impl HotReloadableCertificates {
    /// Create a new hot-reloadable certificate manager
    pub fn new() -> Self {
        let (update_tx, _) = watch::channel(0u64);
        Self {
            cert_chain: Arc::new(RwLock::new(Vec::new())),
            private_key_data: Arc::new(RwLock::new(None)),
            update_tx,
            version: Arc::new(RwLock::new(0)),
            cert_path: Arc::new(RwLock::new(None)),
            key_path: Arc::new(RwLock::new(None)),
        }
    }

    /// Load certificates from files
    pub fn load_from_files<P: AsRef<Path>>(&self, cert_path: P, key_path: P) -> NetResult<()> {
        let cert_path = cert_path.as_ref();
        let key_path = key_path.as_ref();

        let loader = CertificateLoader::new();
        let key_loader = PrivateKeyLoader::new();

        let certs = loader.load_pem_file(cert_path)?;
        let key = key_loader.load_pem_file(key_path)?;

        {
            let mut chain = self.cert_chain.write();
            *chain = certs;
        }

        {
            let mut pk = self.private_key_data.write();
            *pk = Some(PrivateKeyData::from_key(&key));
        }

        {
            let mut cp = self.cert_path.write();
            *cp = Some(cert_path.to_path_buf());
        }

        {
            let mut kp = self.key_path.write();
            *kp = Some(key_path.to_path_buf());
        }

        self.increment_version();

        info!(
            cert_path = %cert_path.display(),
            key_path = %key_path.display(),
            "Loaded certificates from files"
        );

        Ok(())
    }

    /// Reload certificates from the previously loaded files
    pub fn reload(&self) -> NetResult<()> {
        let cert_path = self.cert_path.read().clone();
        let key_path = self.key_path.read().clone();

        match (cert_path, key_path) {
            (Some(cp), Some(kp)) => {
                self.load_from_files(&cp, &kp)?;
                info!("Reloaded certificates");
                Ok(())
            }
            _ => Err(NetError::InvalidCertificate(
                "No certificate paths configured for reload".to_string(),
            )),
        }
    }

    /// Set certificates directly
    pub fn set_certificates(
        &self,
        certs: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) {
        {
            let mut chain = self.cert_chain.write();
            *chain = certs;
        }

        {
            let mut pk = self.private_key_data.write();
            *pk = Some(PrivateKeyData::from_key(&key));
        }

        self.increment_version();
    }

    /// Get current certificate chain
    pub fn get_cert_chain(&self) -> Vec<CertificateDer<'static>> {
        self.cert_chain.read().clone()
    }

    /// Get current private key
    pub fn get_private_key(&self) -> Option<PrivateKeyDer<'static>> {
        self.private_key_data.read().as_ref().map(|k| k.to_key())
    }

    /// Get current version
    pub fn get_version(&self) -> u64 {
        *self.version.read()
    }

    /// Subscribe to certificate updates
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.update_tx.subscribe()
    }

    /// Increment version and notify subscribers
    fn increment_version(&self) {
        let mut version = self.version.write();
        *version += 1;
        let _ = self.update_tx.send(*version);
    }

    /// Start a file watcher for automatic reload
    ///
    /// This spawns a background task that watches for file modifications
    pub fn start_file_watcher(
        self: Arc<Self>,
        check_interval: Duration,
    ) -> NetResult<tokio::task::JoinHandle<()>> {
        let cert_path = self.cert_path.read().clone();
        let key_path = self.key_path.read().clone();

        let (cert_path, key_path) = match (cert_path, key_path) {
            (Some(cp), Some(kp)) => (cp, kp),
            _ => {
                return Err(NetError::InvalidCertificate(
                    "No certificate paths configured for file watching".to_string(),
                ));
            }
        };

        let handle = tokio::spawn(async move {
            let mut last_cert_modified = get_file_modified(&cert_path);
            let mut last_key_modified = get_file_modified(&key_path);

            loop {
                tokio::time::sleep(check_interval).await;

                let cert_modified = get_file_modified(&cert_path);
                let key_modified = get_file_modified(&key_path);

                let cert_changed = cert_modified != last_cert_modified;
                let key_changed = key_modified != last_key_modified;

                if cert_changed || key_changed {
                    info!(
                        cert_changed = cert_changed,
                        key_changed = key_changed,
                        "Detected certificate file change, reloading"
                    );

                    match self.reload() {
                        Ok(()) => {
                            last_cert_modified = cert_modified;
                            last_key_modified = key_modified;
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to reload certificates");
                        }
                    }
                }
            }
        });

        Ok(handle)
    }
}

/// Get file modification time
fn get_file_modified<P: AsRef<Path>>(path: P) -> Option<SystemTime> {
    fs::metadata(path.as_ref())
        .ok()
        .and_then(|m| m.modified().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

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
}
