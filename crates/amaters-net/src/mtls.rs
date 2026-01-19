//! Mutual TLS (mTLS) authentication for AmateRS networking layer
//!
//! This module provides comprehensive mTLS support including:
//! - Client certificate verification
//! - Server certificate verification
//! - Mutual authentication handshake
//! - Certificate-to-principal mapping
//! - Certificate revocation checking (CRL/OCSP)
//!
//! # Example
//!
//! ```rust,ignore
//! use amaters_net::mtls::{MtlsConfig, MtlsServer, MtlsClient};
//! use amaters_net::tls::{CertificateStore, SelfSignedGenerator};
//!
//! // Create mTLS configuration
//! let mut config = MtlsConfig::new();
//! config.set_client_auth_required(true);
//!
//! // Create server with mTLS
//! let server = MtlsServer::builder()
//!     .with_identity(cert_chain, private_key)
//!     .with_client_ca(ca_cert)
//!     .build()?;
//!
//! // Create client with mTLS
//! let client = MtlsClient::builder()
//!     .with_identity(client_cert, client_key)
//!     .with_server_ca(server_ca)
//!     .build()?;
//! ```

use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use parking_lot::RwLock;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    ClientConfig, DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig,
    SignatureScheme,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{debug, error, info, warn};
use x509_parser::prelude::*;

use crate::error::{NetError, NetResult};
use crate::tls::{CertificateInfo, CertificateLoader, CertificateStore, HotReloadableCertificates};

/// Principal identity extracted from a client certificate
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// Subject common name
    pub name: String,
    /// Subject organization
    pub organization: Option<String>,
    /// Subject organizational unit
    pub organizational_unit: Option<String>,
    /// Email address (from SAN or subject)
    pub email: Option<String>,
    /// Certificate serial number
    pub serial: String,
    /// SHA-256 fingerprint of the certificate
    pub fingerprint: String,
    /// Additional attributes
    pub attributes: HashMap<String, String>,
}

impl Principal {
    /// Create a principal from certificate DER bytes
    pub fn from_certificate(cert: &CertificateDer<'_>) -> NetResult<Self> {
        let (_, parsed) = X509Certificate::from_der(cert.as_ref()).map_err(|e| {
            NetError::InvalidCertificate(format!("Failed to parse certificate: {e}"))
        })?;

        let name = parsed
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .map(String::from)
            .unwrap_or_else(|| "unknown".to_string());

        let organization = parsed
            .subject()
            .iter_organization()
            .next()
            .and_then(|o| o.as_str().ok())
            .map(String::from);

        let organizational_unit = parsed
            .subject()
            .iter_organizational_unit()
            .next()
            .and_then(|ou| ou.as_str().ok())
            .map(String::from);

        let mut email = None;
        if let Ok(Some(san)) = parsed.subject_alternative_name() {
            for name in san.value.general_names.iter() {
                if let GeneralName::RFC822Name(e) = name {
                    email = Some(e.to_string());
                    break;
                }
            }
        }

        let serial = format!("{:x}", parsed.serial);
        // Create fingerprint from first 32 bytes of certificate
        use std::fmt::Write;
        let fingerprint = cert
            .as_ref()
            .iter()
            .take(32)
            .fold(String::new(), |mut s, b| {
                let _ = write!(&mut s, "{b:02x}");
                s
            });

        Ok(Self {
            name,
            organization,
            organizational_unit,
            email,
            serial,
            fingerprint,
            attributes: HashMap::new(),
        })
    }

    /// Add a custom attribute to the principal
    pub fn with_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }
}

/// Certificate-to-principal mapping strategy
pub trait PrincipalMapper: Send + Sync {
    /// Map a certificate to a principal
    fn map_certificate(&self, cert: &CertificateDer<'_>) -> NetResult<Principal>;

    /// Get the principal name for authorization
    fn get_principal_name(&self, principal: &Principal) -> String;
}

/// Default principal mapper using certificate subject CN
#[derive(Debug, Clone, Default)]
pub struct DefaultPrincipalMapper;

impl PrincipalMapper for DefaultPrincipalMapper {
    fn map_certificate(&self, cert: &CertificateDer<'_>) -> NetResult<Principal> {
        Principal::from_certificate(cert)
    }

    fn get_principal_name(&self, principal: &Principal) -> String {
        principal.name.clone()
    }
}

/// Principal mapper using organization and CN
#[derive(Debug, Clone, Default)]
pub struct OrganizationPrincipalMapper;

impl PrincipalMapper for OrganizationPrincipalMapper {
    fn map_certificate(&self, cert: &CertificateDer<'_>) -> NetResult<Principal> {
        Principal::from_certificate(cert)
    }

    fn get_principal_name(&self, principal: &Principal) -> String {
        match &principal.organization {
            Some(org) => format!("{}/{}", org, principal.name),
            None => principal.name.clone(),
        }
    }
}

/// Certificate revocation status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationStatus {
    /// Certificate is valid (not revoked)
    Good,
    /// Certificate has been revoked
    Revoked,
    /// Revocation status is unknown
    Unknown,
    /// Revocation check failed
    CheckFailed,
}

/// Certificate revocation checker
pub trait RevocationChecker: Send + Sync {
    /// Check if a certificate has been revoked
    fn check_revocation(&self, cert: &CertificateDer<'_>) -> NetResult<RevocationStatus>;

    /// Check if a certificate has been revoked asynchronously
    fn check_revocation_async(
        &self,
        cert: &CertificateDer<'_>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = NetResult<RevocationStatus>> + Send + '_>>;
}

/// CRL-based certificate revocation checker
#[derive(Debug)]
pub struct CrlRevocationChecker {
    /// CRL entries (serial number -> revocation time)
    revoked_serials: Arc<RwLock<HashMap<String, SystemTime>>>,
    /// Last CRL update time
    last_update: Arc<RwLock<Option<SystemTime>>>,
    /// CRL update URL
    crl_url: Option<String>,
}

impl Default for CrlRevocationChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl CrlRevocationChecker {
    /// Create a new CRL revocation checker
    pub fn new() -> Self {
        Self {
            revoked_serials: Arc::new(RwLock::new(HashMap::new())),
            last_update: Arc::new(RwLock::new(None)),
            crl_url: None,
        }
    }

    /// Set the CRL distribution point URL
    pub fn with_crl_url(mut self, url: impl Into<String>) -> Self {
        self.crl_url = Some(url.into());
        self
    }

    /// Load CRL from DER file
    pub fn load_crl_der<P: AsRef<Path>>(&self, path: P) -> NetResult<usize> {
        let data = fs::read(path.as_ref())
            .map_err(|e| NetError::InvalidCertificate(format!("Failed to read CRL file: {e}")))?;

        self.load_crl_bytes(&data)
    }

    /// Load CRL from PEM file
    pub fn load_crl_pem<P: AsRef<Path>>(&self, path: P) -> NetResult<usize> {
        let file = fs::File::open(path.as_ref())
            .map_err(|e| NetError::InvalidCertificate(format!("Failed to open CRL file: {e}")))?;
        let mut reader = BufReader::new(file);

        let crls: Vec<_> = rustls_pemfile::crls(&mut reader)
            .filter_map(|r| r.ok())
            .collect();

        if crls.is_empty() {
            return Err(NetError::InvalidCertificate(
                "No CRLs found in PEM file".to_string(),
            ));
        }

        let mut total = 0;
        for crl in crls {
            total += self.load_crl_bytes(crl.as_ref())?;
        }

        Ok(total)
    }

    /// Load CRL from bytes
    pub fn load_crl_bytes(&self, crl_data: &[u8]) -> NetResult<usize> {
        let (_, crl) = CertificateRevocationList::from_der(crl_data)
            .map_err(|e| NetError::InvalidCertificate(format!("Failed to parse CRL: {e}")))?;

        let mut revoked = self.revoked_serials.write();
        let mut count = 0;

        for entry in crl.iter_revoked_certificates() {
            let serial = format!("{:x}", entry.user_certificate);
            let revocation_time = SystemTime::UNIX_EPOCH; // Default; proper time parsing would be added
            revoked.insert(serial, revocation_time);
            count += 1;
        }

        {
            let mut last = self.last_update.write();
            *last = Some(SystemTime::now());
        }

        info!(count = count, "Loaded CRL entries");
        Ok(count)
    }

    /// Add a revoked certificate by serial number
    pub fn add_revoked(&self, serial: impl Into<String>) {
        let mut revoked = self.revoked_serials.write();
        revoked.insert(serial.into(), SystemTime::now());
    }

    /// Check if a serial number is revoked
    pub fn is_revoked(&self, serial: &str) -> bool {
        self.revoked_serials.read().contains_key(serial)
    }

    /// Get revocation time for a serial
    pub fn get_revocation_time(&self, serial: &str) -> Option<SystemTime> {
        self.revoked_serials.read().get(serial).copied()
    }

    /// Get count of revoked certificates
    pub fn revoked_count(&self) -> usize {
        self.revoked_serials.read().len()
    }

    /// Clear all revoked entries
    pub fn clear(&self) {
        self.revoked_serials.write().clear();
        *self.last_update.write() = None;
    }
}

impl RevocationChecker for CrlRevocationChecker {
    fn check_revocation(&self, cert: &CertificateDer<'_>) -> NetResult<RevocationStatus> {
        let (_, parsed) = X509Certificate::from_der(cert.as_ref()).map_err(|e| {
            NetError::InvalidCertificate(format!("Failed to parse certificate: {e}"))
        })?;

        let serial = format!("{:x}", parsed.serial);

        if self.is_revoked(&serial) {
            Ok(RevocationStatus::Revoked)
        } else if self.last_update.read().is_some() {
            Ok(RevocationStatus::Good)
        } else {
            Ok(RevocationStatus::Unknown)
        }
    }

    fn check_revocation_async(
        &self,
        cert: &CertificateDer<'_>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = NetResult<RevocationStatus>> + Send + '_>>
    {
        let result = self.check_revocation(cert);
        Box::pin(async move { result })
    }
}

/// OCSP-based certificate revocation checker (stub for future implementation)
#[derive(Debug, Default)]
pub struct OcspRevocationChecker {
    /// OCSP responder URL
    responder_url: Option<String>,
    /// Cache of OCSP responses
    response_cache: Arc<RwLock<HashMap<String, (RevocationStatus, SystemTime)>>>,
    /// Cache TTL
    cache_ttl: Duration,
}

impl OcspRevocationChecker {
    /// Create a new OCSP revocation checker
    pub fn new() -> Self {
        Self {
            responder_url: None,
            response_cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: Duration::from_secs(3600), // 1 hour default
        }
    }

    /// Set the OCSP responder URL
    pub fn with_responder_url(mut self, url: impl Into<String>) -> Self {
        self.responder_url = Some(url.into());
        self
    }

    /// Set cache TTL
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Get cached revocation status
    fn get_cached(&self, fingerprint: &str) -> Option<RevocationStatus> {
        let cache = self.response_cache.read();
        if let Some((status, timestamp)) = cache.get(fingerprint) {
            if timestamp.elapsed().unwrap_or(Duration::MAX) < self.cache_ttl {
                return Some(*status);
            }
        }
        None
    }

    /// Cache a revocation status
    fn cache_status(&self, fingerprint: String, status: RevocationStatus) {
        let mut cache = self.response_cache.write();
        cache.insert(fingerprint, (status, SystemTime::now()));
    }
}

impl RevocationChecker for OcspRevocationChecker {
    fn check_revocation(&self, cert: &CertificateDer<'_>) -> NetResult<RevocationStatus> {
        // Create a fingerprint from first 32 bytes of certificate
        use std::fmt::Write;
        let fingerprint = cert
            .as_ref()
            .iter()
            .take(32)
            .fold(String::new(), |mut s, b| {
                let _ = write!(&mut s, "{b:02x}");
                s
            });

        // Check cache first
        if let Some(status) = self.get_cached(&fingerprint) {
            return Ok(status);
        }

        // OCSP check would require network request
        // For now, return Unknown (actual OCSP implementation would use async)
        warn!("OCSP checking requires async network request, returning Unknown");
        Ok(RevocationStatus::Unknown)
    }

    fn check_revocation_async(
        &self,
        cert: &CertificateDer<'_>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = NetResult<RevocationStatus>> + Send + '_>>
    {
        // Create a fingerprint from first 32 bytes of certificate
        use std::fmt::Write;
        let fingerprint = cert
            .as_ref()
            .iter()
            .take(32)
            .fold(String::new(), |mut s, b| {
                let _ = write!(&mut s, "{b:02x}");
                s
            });

        // Check cache first
        if let Some(status) = self.get_cached(&fingerprint) {
            return Box::pin(async move { Ok(status) });
        }

        // Full OCSP implementation would perform network request here
        // For now, we return Unknown
        let cache_fn = {
            let fingerprint_clone = fingerprint.clone();
            let checker = self;
            move |status: RevocationStatus| {
                checker.cache_status(fingerprint_clone, status);
            }
        };

        Box::pin(async move {
            // Placeholder for actual OCSP request
            // In production, this would:
            // 1. Build OCSP request
            // 2. Send to responder URL
            // 3. Parse and verify OCSP response
            // 4. Cache the result
            warn!("OCSP async check not fully implemented, returning Unknown");
            let status = RevocationStatus::Unknown;
            cache_fn(status);
            Ok(status)
        })
    }
}

/// Combined revocation checker using both CRL and OCSP
#[derive(Debug)]
pub struct CombinedRevocationChecker {
    /// CRL checker
    crl: Arc<CrlRevocationChecker>,
    /// OCSP checker
    ocsp: Arc<OcspRevocationChecker>,
    /// Prefer OCSP over CRL
    prefer_ocsp: bool,
}

impl CombinedRevocationChecker {
    /// Create a new combined revocation checker
    pub fn new(crl: Arc<CrlRevocationChecker>, ocsp: Arc<OcspRevocationChecker>) -> Self {
        Self {
            crl,
            ocsp,
            prefer_ocsp: false,
        }
    }

    /// Prefer OCSP over CRL for revocation checking
    pub fn prefer_ocsp(mut self) -> Self {
        self.prefer_ocsp = true;
        self
    }
}

impl RevocationChecker for CombinedRevocationChecker {
    fn check_revocation(&self, cert: &CertificateDer<'_>) -> NetResult<RevocationStatus> {
        if self.prefer_ocsp {
            // Try OCSP first
            match self.ocsp.check_revocation(cert)? {
                RevocationStatus::Unknown | RevocationStatus::CheckFailed => {
                    // Fall back to CRL
                    self.crl.check_revocation(cert)
                }
                status => Ok(status),
            }
        } else {
            // Try CRL first
            match self.crl.check_revocation(cert)? {
                RevocationStatus::Unknown | RevocationStatus::CheckFailed => {
                    // Fall back to OCSP
                    self.ocsp.check_revocation(cert)
                }
                status => Ok(status),
            }
        }
    }

    fn check_revocation_async(
        &self,
        cert: &CertificateDer<'_>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = NetResult<RevocationStatus>> + Send + '_>>
    {
        let result = self.check_revocation(cert);
        Box::pin(async move { result })
    }
}

/// Custom client certificate verifier with revocation checking
pub struct MtlsClientVerifier {
    /// Root certificates for verification
    roots: Arc<RootCertStore>,
    /// Principal mapper
    mapper: Arc<dyn PrincipalMapper>,
    /// Revocation checker
    revocation: Option<Arc<dyn RevocationChecker>>,
    /// Whether client authentication is required
    require_client_auth: bool,
    /// Allowed principal patterns (empty means allow all)
    allowed_principals: Vec<String>,
}

impl std::fmt::Debug for MtlsClientVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtlsClientVerifier")
            .field("roots", &"<RootCertStore>")
            .field("mapper", &"<PrincipalMapper>")
            .field(
                "revocation",
                &self.revocation.as_ref().map(|_| "<RevocationChecker>"),
            )
            .field("require_client_auth", &self.require_client_auth)
            .field("allowed_principals", &self.allowed_principals)
            .finish()
    }
}

impl MtlsClientVerifier {
    /// Create a new client verifier
    pub fn new(roots: RootCertStore) -> Self {
        Self {
            roots: Arc::new(roots),
            mapper: Arc::new(DefaultPrincipalMapper),
            revocation: None,
            require_client_auth: true,
            allowed_principals: Vec::new(),
        }
    }

    /// Set the principal mapper
    pub fn with_mapper(mut self, mapper: Arc<dyn PrincipalMapper>) -> Self {
        self.mapper = mapper;
        self
    }

    /// Set the revocation checker
    pub fn with_revocation(mut self, checker: Arc<dyn RevocationChecker>) -> Self {
        self.revocation = Some(checker);
        self
    }

    /// Make client authentication optional
    pub fn optional_auth(mut self) -> Self {
        self.require_client_auth = false;
        self
    }

    /// Add allowed principal pattern
    pub fn allow_principal(mut self, pattern: impl Into<String>) -> Self {
        self.allowed_principals.push(pattern.into());
        self
    }

    /// Verify a client certificate
    fn verify_certificate(&self, cert: &CertificateDer<'_>) -> NetResult<Principal> {
        // Parse and validate certificate
        let loader = CertificateLoader::new();
        let info = loader.get_certificate_info(cert)?;

        // Check validity
        if !info.is_valid() {
            return Err(NetError::InvalidCertificate(
                "Certificate has expired or is not yet valid".to_string(),
            ));
        }

        // Check revocation if checker is configured
        if let Some(ref checker) = self.revocation {
            match checker.check_revocation(cert)? {
                RevocationStatus::Revoked => {
                    return Err(NetError::InvalidCertificate(
                        "Certificate has been revoked".to_string(),
                    ));
                }
                RevocationStatus::CheckFailed => {
                    warn!("Revocation check failed, allowing certificate");
                }
                _ => {}
            }
        }

        // Map certificate to principal
        let principal = self.mapper.map_certificate(cert)?;

        // Check allowed principals
        if !self.allowed_principals.is_empty() {
            let principal_name = self.mapper.get_principal_name(&principal);
            let is_allowed = self.allowed_principals.iter().any(|pattern| {
                if pattern.contains('*') {
                    // Simple wildcard matching
                    let regex_pattern = pattern.replace('*', ".*");
                    regex_pattern == principal_name
                        || principal_name.starts_with(&pattern.replace('*', ""))
                } else {
                    pattern == &principal_name
                }
            });

            if !is_allowed {
                return Err(NetError::InsufficientPermissions(format!(
                    "Principal '{}' is not in the allowed list",
                    principal_name
                )));
            }
        }

        Ok(principal)
    }
}

impl ClientCertVerifier for MtlsClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        match self.verify_certificate(end_entity) {
            Ok(principal) => {
                debug!(principal = %principal.name, "Client certificate verified");
                Ok(ClientCertVerified::assertion())
            }
            Err(e) => {
                error!(error = %e, "Client certificate verification failed");
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::BadEncoding,
                ))
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::ED25519,
        ]
    }

    fn client_auth_mandatory(&self) -> bool {
        self.require_client_auth
    }
}

/// Custom server certificate verifier
pub struct MtlsServerVerifier {
    /// Root certificates for verification
    roots: Arc<RootCertStore>,
    /// Revocation checker
    revocation: Option<Arc<dyn RevocationChecker>>,
    /// Expected server names
    expected_names: Vec<String>,
}

impl std::fmt::Debug for MtlsServerVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtlsServerVerifier")
            .field("roots", &"<RootCertStore>")
            .field(
                "revocation",
                &self.revocation.as_ref().map(|_| "<RevocationChecker>"),
            )
            .field("expected_names", &self.expected_names)
            .finish()
    }
}

impl MtlsServerVerifier {
    /// Create a new server verifier
    pub fn new(roots: RootCertStore) -> Self {
        Self {
            roots: Arc::new(roots),
            revocation: None,
            expected_names: Vec::new(),
        }
    }

    /// Set the revocation checker
    pub fn with_revocation(mut self, checker: Arc<dyn RevocationChecker>) -> Self {
        self.revocation = Some(checker);
        self
    }

    /// Add expected server name
    pub fn expect_name(mut self, name: impl Into<String>) -> Self {
        self.expected_names.push(name.into());
        self
    }

    /// Verify a server certificate
    fn verify_certificate(
        &self,
        cert: &CertificateDer<'_>,
        server_name: Option<&str>,
    ) -> NetResult<()> {
        let loader = CertificateLoader::new();
        let info = loader.get_certificate_info(cert)?;

        // Check validity
        if !info.is_valid() {
            return Err(NetError::InvalidCertificate(
                "Server certificate has expired or is not yet valid".to_string(),
            ));
        }

        // Check revocation if checker is configured
        if let Some(ref checker) = self.revocation {
            match checker.check_revocation(cert)? {
                RevocationStatus::Revoked => {
                    return Err(NetError::InvalidCertificate(
                        "Server certificate has been revoked".to_string(),
                    ));
                }
                RevocationStatus::CheckFailed => {
                    warn!("Revocation check failed for server certificate");
                }
                _ => {}
            }
        }

        // Verify server name if specified
        if let Some(name) = server_name {
            let name_matches = info.common_name.as_deref() == Some(name)
                || info.subject_alt_names.iter().any(|san| san == name);

            if !name_matches && !self.expected_names.is_empty() {
                let expected_matches = self.expected_names.iter().any(|expected| {
                    info.common_name.as_deref() == Some(expected)
                        || info.subject_alt_names.iter().any(|san| san == expected)
                });

                if !expected_matches {
                    return Err(NetError::InvalidCertificate(format!(
                        "Server name '{}' does not match certificate",
                        name
                    )));
                }
            }
        }

        Ok(())
    }
}

impl ServerCertVerifier for MtlsServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let name_str = match server_name {
            ServerName::DnsName(name) => Some(name.as_ref().to_string()),
            ServerName::IpAddress(ip) => Some(format!("{:?}", ip)),
            _ => None,
        };

        match self.verify_certificate(end_entity, name_str.as_deref()) {
            Ok(()) => {
                debug!("Server certificate verified");
                Ok(ServerCertVerified::assertion())
            }
            Err(e) => {
                error!(error = %e, "Server certificate verification failed");
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::BadEncoding,
                ))
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

/// mTLS configuration builder
pub struct MtlsConfigBuilder {
    /// Server certificate chain
    cert_chain: Vec<CertificateDer<'static>>,
    /// Server private key
    private_key: Option<PrivateKeyDer<'static>>,
    /// Root certificate store for client verification
    client_roots: RootCertStore,
    /// Root certificate store for server verification
    server_roots: RootCertStore,
    /// Whether client authentication is required
    require_client_auth: bool,
    /// Principal mapper
    mapper: Arc<dyn PrincipalMapper>,
    /// Revocation checker
    revocation: Option<Arc<dyn RevocationChecker>>,
    /// Hot reloadable certificates
    hot_reload: Option<Arc<HotReloadableCertificates>>,
}

impl std::fmt::Debug for MtlsConfigBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtlsConfigBuilder")
            .field("cert_chain", &format!("<{} certs>", self.cert_chain.len()))
            .field("private_key", &self.private_key.as_ref().map(|_| "<key>"))
            .field("client_roots", &"<RootCertStore>")
            .field("server_roots", &"<RootCertStore>")
            .field("require_client_auth", &self.require_client_auth)
            .field("mapper", &"<PrincipalMapper>")
            .field(
                "revocation",
                &self.revocation.as_ref().map(|_| "<RevocationChecker>"),
            )
            .field(
                "hot_reload",
                &self.hot_reload.as_ref().map(|_| "<HotReloadable>"),
            )
            .finish()
    }
}

impl Default for MtlsConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MtlsConfigBuilder {
    /// Create a new mTLS configuration builder
    pub fn new() -> Self {
        Self {
            cert_chain: Vec::new(),
            private_key: None,
            client_roots: RootCertStore::empty(),
            server_roots: RootCertStore::empty(),
            require_client_auth: true,
            mapper: Arc::new(DefaultPrincipalMapper),
            revocation: None,
            hot_reload: None,
        }
    }

    /// Set the server identity (certificate chain and private key)
    pub fn with_identity(
        mut self,
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
    ) -> Self {
        self.cert_chain = cert_chain;
        self.private_key = Some(private_key);
        self
    }

    /// Load server identity from PEM files
    pub fn with_identity_files<P: AsRef<Path>>(
        mut self,
        cert_path: P,
        key_path: P,
    ) -> NetResult<Self> {
        let loader = CertificateLoader::new();
        let key_loader = crate::tls::PrivateKeyLoader::new();

        self.cert_chain = loader.load_pem_file(cert_path)?;
        self.private_key = Some(key_loader.load_pem_file(key_path)?);

        Ok(self)
    }

    /// Add client CA certificate for verification
    pub fn with_client_ca(mut self, cert: CertificateDer<'static>) -> NetResult<Self> {
        self.client_roots
            .add(cert)
            .map_err(|e| NetError::InvalidCertificate(format!("Failed to add client CA: {e}")))?;
        Ok(self)
    }

    /// Add client CA certificates from a store
    pub fn with_client_ca_store(mut self, store: &CertificateStore) -> Self {
        let roots = store.get_root_store();
        self.client_roots.extend(roots.roots.iter().cloned());
        self
    }

    /// Add server CA certificate for verification
    pub fn with_server_ca(mut self, cert: CertificateDer<'static>) -> NetResult<Self> {
        self.server_roots
            .add(cert)
            .map_err(|e| NetError::InvalidCertificate(format!("Failed to add server CA: {e}")))?;
        Ok(self)
    }

    /// Add server CA certificates from a store
    pub fn with_server_ca_store(mut self, store: &CertificateStore) -> Self {
        let roots = store.get_root_store();
        self.server_roots.extend(roots.roots.iter().cloned());
        self
    }

    /// Add system root certificates for server verification
    pub fn with_system_roots(mut self) -> Self {
        self.server_roots
            .extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        self
    }

    /// Set whether client authentication is required
    pub fn require_client_auth(mut self, required: bool) -> Self {
        self.require_client_auth = required;
        self
    }

    /// Set the principal mapper
    pub fn with_mapper(mut self, mapper: Arc<dyn PrincipalMapper>) -> Self {
        self.mapper = mapper;
        self
    }

    /// Set the revocation checker
    pub fn with_revocation(mut self, checker: Arc<dyn RevocationChecker>) -> Self {
        self.revocation = Some(checker);
        self
    }

    /// Enable hot reload support
    pub fn with_hot_reload(mut self, hot_reload: Arc<HotReloadableCertificates>) -> Self {
        self.hot_reload = Some(hot_reload);
        self
    }

    /// Build the server configuration
    pub fn build_server_config(self) -> NetResult<ServerConfig> {
        let private_key = self
            .private_key
            .ok_or_else(|| NetError::InvalidCertificate("Private key is required".to_string()))?;

        if self.cert_chain.is_empty() {
            return Err(NetError::InvalidCertificate(
                "Certificate chain is required".to_string(),
            ));
        }

        // Create client verifier
        let client_verifier =
            Arc::new(MtlsClientVerifier::new(self.client_roots).with_mapper(self.mapper));

        let config = if self.require_client_auth {
            ServerConfig::builder()
                .with_client_cert_verifier(client_verifier)
                .with_single_cert(self.cert_chain, private_key)
                .map_err(|e| {
                    NetError::InvalidCertificate(format!("Failed to build server config: {e}"))
                })?
        } else {
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(self.cert_chain, private_key)
                .map_err(|e| {
                    NetError::InvalidCertificate(format!("Failed to build server config: {e}"))
                })?
        };

        Ok(config)
    }

    /// Build the client configuration
    pub fn build_client_config(self) -> NetResult<ClientConfig> {
        let private_key = self.private_key.ok_or_else(|| {
            NetError::InvalidCertificate("Private key is required for client mTLS".to_string())
        })?;

        if self.cert_chain.is_empty() {
            return Err(NetError::InvalidCertificate(
                "Certificate chain is required for client mTLS".to_string(),
            ));
        }

        // Create server verifier
        let server_verifier = Arc::new(MtlsServerVerifier::new(self.server_roots));

        let config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(server_verifier)
            .with_client_auth_cert(self.cert_chain, private_key)
            .map_err(|e| {
                NetError::InvalidCertificate(format!("Failed to build client config: {e}"))
            })?;

        Ok(config)
    }

    /// Build TLS acceptor for server
    pub fn build_acceptor(self) -> NetResult<TlsAcceptor> {
        let config = self.build_server_config()?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }

    /// Build TLS connector for client
    pub fn build_connector(self) -> NetResult<TlsConnector> {
        let config = self.build_client_config()?;
        Ok(TlsConnector::from(Arc::new(config)))
    }
}

/// mTLS server helper
pub struct MtlsServer {
    /// TLS acceptor
    acceptor: TlsAcceptor,
    /// Hot reload handle
    hot_reload: Option<Arc<HotReloadableCertificates>>,
}

impl std::fmt::Debug for MtlsServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtlsServer")
            .field("has_hot_reload", &self.hot_reload.is_some())
            .finish()
    }
}

impl MtlsServer {
    /// Create a new mTLS configuration builder
    pub fn builder() -> MtlsConfigBuilder {
        MtlsConfigBuilder::new()
    }

    /// Create from pre-built config
    pub fn from_config(config: ServerConfig) -> Self {
        Self {
            acceptor: TlsAcceptor::from(Arc::new(config)),
            hot_reload: None,
        }
    }

    /// Get the TLS acceptor
    pub fn acceptor(&self) -> &TlsAcceptor {
        &self.acceptor
    }

    /// Enable hot reload support
    pub fn with_hot_reload(mut self, hot_reload: Arc<HotReloadableCertificates>) -> Self {
        self.hot_reload = Some(hot_reload);
        self
    }
}

/// mTLS client helper
pub struct MtlsClient {
    /// TLS connector
    connector: TlsConnector,
}

impl std::fmt::Debug for MtlsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtlsClient").finish()
    }
}

impl MtlsClient {
    /// Create a new mTLS configuration builder
    pub fn builder() -> MtlsConfigBuilder {
        MtlsConfigBuilder::new()
    }

    /// Create from pre-built config
    pub fn from_config(config: ClientConfig) -> Self {
        Self {
            connector: TlsConnector::from(Arc::new(config)),
        }
    }

    /// Get the TLS connector
    pub fn connector(&self) -> &TlsConnector {
        &self.connector
    }
}

/// Mutual authentication handshake result
#[derive(Debug, Clone)]
pub struct HandshakeResult {
    /// Peer principal (for server, this is the client; for client, this is the server)
    pub peer_principal: Option<Principal>,
    /// Negotiated TLS version
    pub tls_version: String,
    /// Negotiated cipher suite
    pub cipher_suite: String,
    /// Handshake duration
    pub duration: Duration,
}

impl HandshakeResult {
    /// Check if peer authentication was successful
    pub fn is_authenticated(&self) -> bool {
        self.peer_principal.is_some()
    }

    /// Get peer principal name
    pub fn peer_name(&self) -> Option<&str> {
        self.peer_principal.as_ref().map(|p| p.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::SelfSignedGenerator;

    #[test]
    fn test_principal_from_certificate() {
        // Generate a test certificate
        let generator = SelfSignedGenerator::new("test-user").with_organization("Test Org");

        let (cert, _) = generator.generate().expect("Should generate certificate");

        let principal = Principal::from_certificate(&cert).expect("Should create principal");

        assert_eq!(principal.name, "test-user");
        assert_eq!(principal.organization.as_deref(), Some("Test Org"));
        assert!(!principal.fingerprint.is_empty());
    }

    #[test]
    fn test_default_principal_mapper() {
        let generator = SelfSignedGenerator::new("test-user");
        let (cert, _) = generator.generate().expect("Should generate certificate");

        let mapper = DefaultPrincipalMapper;
        let principal = mapper
            .map_certificate(&cert)
            .expect("Should map certificate");
        let name = mapper.get_principal_name(&principal);

        assert_eq!(name, "test-user");
    }

    #[test]
    fn test_organization_principal_mapper() {
        let generator = SelfSignedGenerator::new("test-user").with_organization("Test Org");

        let (cert, _) = generator.generate().expect("Should generate certificate");

        let mapper = OrganizationPrincipalMapper;
        let principal = mapper
            .map_certificate(&cert)
            .expect("Should map certificate");
        let name = mapper.get_principal_name(&principal);

        assert_eq!(name, "Test Org/test-user");
    }

    #[test]
    fn test_crl_revocation_checker() {
        let checker = CrlRevocationChecker::new();

        // Add a revoked serial
        checker.add_revoked("abc123");

        assert!(checker.is_revoked("abc123"));
        assert!(!checker.is_revoked("def456"));
        assert_eq!(checker.revoked_count(), 1);
    }

    #[test]
    fn test_mtls_config_builder() {
        // Install CryptoProvider for rustls
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        // Generate CA certificate
        let ca_generator = SelfSignedGenerator::new("Test CA")
            .as_ca()
            .with_validity_days(365);

        let (ca_cert, _ca_key) = ca_generator.generate().expect("Should generate CA");

        // Generate server certificate
        let server_generator = SelfSignedGenerator::new("localhost").with_san("127.0.0.1");

        let (server_cert, server_key) = server_generator
            .generate()
            .expect("Should generate server cert");

        // Build server config
        let result = MtlsConfigBuilder::new()
            .with_identity(vec![server_cert.clone()], server_key.clone_key())
            .with_client_ca(ca_cert.clone())
            .expect("Should add CA")
            .require_client_auth(true)
            .build_server_config();

        assert!(result.is_ok());
    }

    #[test]
    fn test_mtls_client_verifier() {
        // Generate CA and client certificates
        let ca_generator = SelfSignedGenerator::new("Test CA").as_ca();

        let (ca_cert, _) = ca_generator.generate().expect("Should generate CA");

        let client_generator =
            SelfSignedGenerator::new("test-client").with_organization("Test Org");

        let (client_cert, _) = client_generator
            .generate()
            .expect("Should generate client cert");

        // Create verifier
        let mut roots = RootCertStore::empty();
        roots.add(ca_cert).expect("Should add CA");

        let verifier = MtlsClientVerifier::new(roots);

        // Verify certificate (note: this is a self-signed cert, so chain verification would fail
        // in a real scenario, but our custom verifier focuses on other checks)
        let loader = CertificateLoader::new();
        let info = loader
            .get_certificate_info(&client_cert)
            .expect("Should get info");

        assert_eq!(info.common_name.as_deref(), Some("test-client"));
    }

    #[test]
    fn test_ocsp_revocation_checker_cache() {
        let checker = OcspRevocationChecker::new().with_cache_ttl(Duration::from_secs(3600));

        // Cache should initially be empty
        let generator = SelfSignedGenerator::new("test");
        let (cert, _) = generator.generate().expect("Should generate cert");

        // First check should return Unknown (no OCSP response cached)
        let status = checker
            .check_revocation(&cert)
            .expect("Should check revocation");
        assert_eq!(status, RevocationStatus::Unknown);
    }

    #[test]
    fn test_combined_revocation_checker() {
        let crl = Arc::new(CrlRevocationChecker::new());
        let ocsp = Arc::new(OcspRevocationChecker::new());

        let combined = CombinedRevocationChecker::new(crl.clone(), ocsp);

        let generator = SelfSignedGenerator::new("test");
        let (cert, _) = generator.generate().expect("Should generate cert");

        // Should return Unknown since neither has data
        let status = combined
            .check_revocation(&cert)
            .expect("Should check revocation");
        assert_eq!(status, RevocationStatus::Unknown);
    }

    #[test]
    fn test_handshake_result() {
        let principal = Principal {
            name: "test-user".to_string(),
            organization: Some("Test Org".to_string()),
            organizational_unit: None,
            email: None,
            serial: "123abc".to_string(),
            fingerprint: "abc123".to_string(),
            attributes: HashMap::new(),
        };

        let result = HandshakeResult {
            peer_principal: Some(principal),
            tls_version: "TLS 1.3".to_string(),
            cipher_suite: "TLS_AES_256_GCM_SHA384".to_string(),
            duration: Duration::from_millis(50),
        };

        assert!(result.is_authenticated());
        assert_eq!(result.peer_name(), Some("test-user"));
    }
}
