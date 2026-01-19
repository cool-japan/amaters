//! Authentication module
//!
//! This module provides authentication services for the server:
//! - mTLS (Mutual TLS) client certificate validation
//! - JWT (JSON Web Token) authentication
//! - API key authentication
//!
//! Security model:
//! - Secure by default (deny unless explicitly allowed)
//! - Multiple authentication methods can be enabled simultaneously
//! - Authentication results in a validated identity (Principal)

use crate::config::{ApiKeySettings, AuthSettings, JwtSettings, MtlsSettings};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info, warn};
use x509_parser::prelude::*;

/// Authentication errors
#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Certificate validation failed: {0}")]
    CertificateError(String),

    #[error("JWT validation failed: {0}")]
    JwtError(String),

    #[error("API key validation failed: {0}")]
    ApiKeyError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("No authentication provided")]
    NoAuthProvided,

    #[error("Authentication method not enabled: {0}")]
    MethodNotEnabled(String),
}

pub type AuthResult<T> = Result<T, AuthError>;

/// Authenticated principal (user identity)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    /// Unique identifier for the user
    pub id: String,

    /// Username or common name
    pub name: String,

    /// Authentication method used
    pub auth_method: AuthMethod,

    /// Additional attributes (roles, groups, etc.)
    pub attributes: HashMap<String, String>,
}

impl Principal {
    /// Create a new principal
    pub fn new(id: String, name: String, auth_method: AuthMethod) -> Self {
        Self {
            id,
            name,
            auth_method,
            attributes: HashMap::new(),
        }
    }

    /// Add an attribute to the principal
    pub fn with_attribute(mut self, key: String, value: String) -> Self {
        self.attributes.insert(key, value);
        self
    }

    /// Get an attribute value
    pub fn get_attribute(&self, key: &str) -> Option<&String> {
        self.attributes.get(key)
    }

    /// Check if principal has a specific role
    pub fn has_role(&self, role: &str) -> bool {
        self.get_attribute("role")
            .map(|r| r == role)
            .unwrap_or(false)
    }
}

/// Authentication method used
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMethod {
    /// mTLS client certificate
    MutualTls,
    /// JWT token
    Jwt,
    /// API key
    ApiKey,
}

impl std::fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthMethod::MutualTls => write!(f, "mTLS"),
            AuthMethod::Jwt => write!(f, "JWT"),
            AuthMethod::ApiKey => write!(f, "API Key"),
        }
    }
}

/// JWT claims structure
#[derive(Debug, Serialize, Deserialize)]
struct JwtClaims {
    /// Subject (user ID)
    sub: String,
    /// Expiration time
    exp: usize,
    /// Issued at
    iat: Option<usize>,
    /// Issuer
    iss: Option<String>,
    /// Audience
    aud: Option<String>,
    /// User name
    name: Option<String>,
    /// Roles
    roles: Option<Vec<String>>,
    /// Custom attributes
    #[serde(flatten)]
    attributes: HashMap<String, serde_json::Value>,
}

/// API key entry
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiKeyEntry {
    /// Key ID
    id: String,
    /// Key name/description
    name: String,
    /// Hashed key value (if hashing enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    key_hash: Option<String>,
    /// Plain key value (if hashing disabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    /// User ID
    user_id: String,
    /// Roles
    #[serde(default)]
    roles: Vec<String>,
    /// Additional attributes
    #[serde(default)]
    attributes: HashMap<String, String>,
}

/// Authentication service
pub struct Authenticator {
    config: Arc<AuthSettings>,
    mtls_validator: Option<MtlsValidator>,
    jwt_validator: Option<JwtValidator>,
    api_key_validator: Option<ApiKeyValidator>,
}

impl Authenticator {
    /// Create a new authenticator
    pub fn new(config: AuthSettings) -> AuthResult<Self> {
        let config = Arc::new(config);

        // Initialize mTLS validator
        let mtls_validator = if config.mtls.enabled {
            Some(MtlsValidator::new(config.mtls.clone())?)
        } else {
            None
        };

        // Initialize JWT validator
        let jwt_validator = if config.jwt.enabled {
            Some(JwtValidator::new(config.jwt.clone())?)
        } else {
            None
        };

        // Initialize API key validator
        let api_key_validator = if config.api_key.enabled {
            Some(ApiKeyValidator::new(config.api_key.clone())?)
        } else {
            None
        };

        Ok(Self {
            config,
            mtls_validator,
            jwt_validator,
            api_key_validator,
        })
    }

    /// Authenticate using client certificate
    pub fn authenticate_certificate(&self, cert_der: &[u8]) -> AuthResult<Principal> {
        if !self.config.methods.contains(&"mtls".to_string()) {
            return Err(AuthError::MethodNotEnabled("mTLS".to_string()));
        }

        let validator = self
            .mtls_validator
            .as_ref()
            .ok_or_else(|| AuthError::MethodNotEnabled("mTLS".to_string()))?;

        validator.validate_certificate(cert_der)
    }

    /// Authenticate using JWT token
    pub fn authenticate_jwt(&self, token: &str) -> AuthResult<Principal> {
        if !self.config.methods.contains(&"jwt".to_string()) {
            return Err(AuthError::MethodNotEnabled("JWT".to_string()));
        }

        let validator = self
            .jwt_validator
            .as_ref()
            .ok_or_else(|| AuthError::MethodNotEnabled("JWT".to_string()))?;

        validator.validate_token(token)
    }

    /// Authenticate using API key
    pub fn authenticate_api_key(&self, key: &str) -> AuthResult<Principal> {
        if !self.config.methods.contains(&"api_key".to_string()) {
            return Err(AuthError::MethodNotEnabled("API Key".to_string()));
        }

        let validator = self
            .api_key_validator
            .as_ref()
            .ok_or_else(|| AuthError::MethodNotEnabled("API Key".to_string()))?;

        validator.validate_key(key)
    }

    /// Check if authentication is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Check if a specific method is enabled
    pub fn is_method_enabled(&self, method: &str) -> bool {
        self.config.methods.contains(&method.to_string())
    }
}

/// mTLS certificate validator
struct MtlsValidator {
    config: MtlsSettings,
    ca_certs: Vec<Vec<u8>>,
}

impl MtlsValidator {
    fn new(config: MtlsSettings) -> AuthResult<Self> {
        let ca_certs = if let Some(ref ca_dir) = config.ca_certs_dir {
            Self::load_ca_certificates(ca_dir)?
        } else {
            Vec::new()
        };

        Ok(Self { config, ca_certs })
    }

    fn load_ca_certificates(dir: &Path) -> AuthResult<Vec<Vec<u8>>> {
        let mut certs = Vec::new();

        if !dir.exists() {
            return Err(AuthError::ConfigError(format!(
                "CA certificates directory does not exist: {}",
                dir.display()
            )));
        }

        for entry_result in fs::read_dir(dir)? {
            let entry = entry_result?;
            let path = entry.path();

            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "crt" || ext == "pem" || ext == "der" {
                        let cert_data = fs::read(&path)?;
                        certs.push(cert_data);
                        debug!("Loaded CA certificate: {}", path.display());
                    }
                }
            }
        }

        info!("Loaded {} CA certificates", certs.len());
        Ok(certs)
    }

    fn validate_certificate(&self, cert_der: &[u8]) -> AuthResult<Principal> {
        // Parse the certificate
        let (_, cert) = X509Certificate::from_der(cert_der).map_err(|e| {
            AuthError::CertificateError(format!("Failed to parse certificate: {}", e))
        })?;

        // Verify certificate validity period
        let now = std::time::SystemTime::now();
        let not_before = cert.validity().not_before.to_datetime();
        let not_after = cert.validity().not_after.to_datetime();

        if now < not_before {
            return Err(AuthError::CertificateError(
                "Certificate not yet valid".to_string(),
            ));
        }

        if now > not_after {
            return Err(AuthError::CertificateError(
                "Certificate has expired".to_string(),
            ));
        }

        // Extract subject information
        let subject = cert.subject();
        let cn = subject
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .ok_or_else(|| AuthError::CertificateError("No CN in certificate".to_string()))?;

        let organization = subject
            .iter_organization()
            .next()
            .and_then(|o| o.as_str().ok());

        // Verify organization if restrictions are configured
        if !self.config.allowed_organizations.is_empty() {
            let org = organization.ok_or_else(|| {
                AuthError::CertificateError("Certificate has no organization".to_string())
            })?;

            if !self.config.allowed_organizations.contains(&org.to_string()) {
                return Err(AuthError::CertificateError(format!(
                    "Organization '{}' not allowed",
                    org
                )));
            }
        }

        // Create principal
        let mut principal = Principal::new(cn.to_string(), cn.to_string(), AuthMethod::MutualTls);

        if let Some(org) = organization {
            principal = principal.with_attribute("organization".to_string(), org.to_string());
        }

        debug!("Successfully authenticated certificate for user: {}", cn);
        Ok(principal)
    }
}

/// JWT token validator
struct JwtValidator {
    config: JwtSettings,
    decoding_key: DecodingKey,
    validation: Validation,
}

impl JwtValidator {
    fn new(config: JwtSettings) -> AuthResult<Self> {
        let algorithm = match config.algorithm.as_str() {
            "HS256" => Algorithm::HS256,
            "RS256" => Algorithm::RS256,
            _ => {
                return Err(AuthError::ConfigError(format!(
                    "Unsupported JWT algorithm: {}",
                    config.algorithm
                )));
            }
        };

        let decoding_key = match algorithm {
            Algorithm::HS256 => {
                let secret = config.secret.as_ref().ok_or_else(|| {
                    AuthError::ConfigError("JWT secret not configured".to_string())
                })?;
                DecodingKey::from_secret(secret.as_bytes())
            }
            Algorithm::RS256 => {
                let public_key_path = config.public_key_path.as_ref().ok_or_else(|| {
                    AuthError::ConfigError("JWT public key path not configured".to_string())
                })?;
                let pem = fs::read_to_string(public_key_path)?;
                DecodingKey::from_rsa_pem(pem.as_bytes()).map_err(|e| {
                    AuthError::ConfigError(format!("Failed to load RSA public key: {}", e))
                })?
            }
            _ => {
                return Err(AuthError::ConfigError(
                    "Algorithm not implemented".to_string(),
                ));
            }
        };

        let mut validation = Validation::new(algorithm);
        validation.validate_exp = true;

        if let Some(ref issuer) = config.issuer {
            validation.set_issuer(&[issuer]);
        }

        if let Some(ref audience) = config.audience {
            validation.set_audience(&[audience]);
        }

        Ok(Self {
            config,
            decoding_key,
            validation,
        })
    }

    fn validate_token(&self, token: &str) -> AuthResult<Principal> {
        // Decode and validate the token
        let token_data = decode::<JwtClaims>(token, &self.decoding_key, &self.validation)
            .map_err(|e| AuthError::JwtError(format!("Token validation failed: {}", e)))?;

        let claims = token_data.claims;

        // Create principal
        let name = claims.name.unwrap_or_else(|| claims.sub.clone());
        let mut principal = Principal::new(claims.sub, name, AuthMethod::Jwt);

        // Add roles
        if let Some(roles) = claims.roles {
            principal = principal.with_attribute("roles".to_string(), roles.join(","));
        }

        // Add custom attributes
        for (key, value) in claims.attributes {
            if let Some(s) = value.as_str() {
                principal = principal.with_attribute(key, s.to_string());
            }
        }

        debug!(
            "Successfully authenticated JWT token for user: {}",
            principal.name
        );
        Ok(principal)
    }
}

/// API key validator
struct ApiKeyValidator {
    config: ApiKeySettings,
    keys: HashMap<String, ApiKeyEntry>,
}

impl ApiKeyValidator {
    fn new(config: ApiKeySettings) -> AuthResult<Self> {
        let keys_file = config
            .keys_file
            .as_ref()
            .ok_or_else(|| AuthError::ConfigError("API keys file not configured".to_string()))?;

        let keys = Self::load_keys(keys_file, config.hash_keys)?;

        info!("Loaded {} API keys", keys.len());

        Ok(Self { config, keys })
    }

    fn load_keys(path: &Path, hash_keys: bool) -> AuthResult<HashMap<String, ApiKeyEntry>> {
        if !path.exists() {
            return Err(AuthError::ConfigError(format!(
                "API keys file does not exist: {}",
                path.display()
            )));
        }

        let contents = fs::read_to_string(path)?;
        let entries: Vec<ApiKeyEntry> = serde_json::from_str(&contents)?;

        let mut keys = HashMap::new();
        for entry in entries {
            let key_value = if hash_keys {
                entry
                    .key_hash
                    .clone()
                    .ok_or_else(|| AuthError::ConfigError("Missing key_hash".to_string()))?
            } else {
                entry
                    .key
                    .clone()
                    .ok_or_else(|| AuthError::ConfigError("Missing key".to_string()))?
            };

            keys.insert(key_value, entry);
        }

        Ok(keys)
    }

    fn validate_key(&self, key: &str) -> AuthResult<Principal> {
        let lookup_key = if self.config.hash_keys {
            Self::hash_key(key)
        } else {
            key.to_string()
        };

        let entry = self
            .keys
            .get(&lookup_key)
            .ok_or(AuthError::InvalidCredentials)?;

        // Create principal
        let mut principal = Principal::new(
            entry.user_id.clone(),
            entry.name.clone(),
            AuthMethod::ApiKey,
        );

        // Add roles
        if !entry.roles.is_empty() {
            principal = principal.with_attribute("roles".to_string(), entry.roles.join(","));
        }

        // Add custom attributes
        for (key, value) in &entry.attributes {
            principal = principal.with_attribute(key.clone(), value.clone());
        }

        debug!(
            "Successfully authenticated API key for user: {}",
            entry.user_id
        );
        Ok(principal)
    }

    fn hash_key(key: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        let result = hasher.finalize();
        BASE64.encode(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_principal_creation() {
        let principal = Principal::new(
            "user123".to_string(),
            "John Doe".to_string(),
            AuthMethod::Jwt,
        );

        assert_eq!(principal.id, "user123");
        assert_eq!(principal.name, "John Doe");
        assert_eq!(principal.auth_method, AuthMethod::Jwt);
    }

    #[test]
    fn test_principal_attributes() {
        let principal = Principal::new(
            "user123".to_string(),
            "John Doe".to_string(),
            AuthMethod::Jwt,
        )
        .with_attribute("role".to_string(), "admin".to_string())
        .with_attribute("department".to_string(), "Engineering".to_string());

        assert_eq!(principal.get_attribute("role"), Some(&"admin".to_string()));
        assert!(principal.has_role("admin"));
        assert!(!principal.has_role("user"));
    }

    #[test]
    fn test_api_key_hashing() {
        let key = "test-api-key-12345";
        let hash1 = ApiKeyValidator::hash_key(key);
        let hash2 = ApiKeyValidator::hash_key(key);

        assert_eq!(hash1, hash2); // Same key produces same hash
        assert!(!hash1.is_empty());
    }

    #[test]
    fn test_authenticator_creation() {
        let config = AuthSettings {
            enabled: true,
            methods: vec!["mtls".to_string()],
            mtls: MtlsSettings {
                enabled: true,
                ca_certs_dir: Some(env::temp_dir()),
                crl_path: None,
                verify_cn: true,
                allowed_organizations: Vec::new(),
            },
            jwt: JwtSettings::default(),
            api_key: ApiKeySettings::default(),
            reject_unauthenticated: true,
        };

        let auth_result = Authenticator::new(config);
        assert!(auth_result.is_ok());
    }

    #[test]
    fn test_jwt_validator_creation_missing_secret() {
        let config = JwtSettings {
            enabled: true,
            secret: None,
            public_key_path: None,
            algorithm: "HS256".to_string(),
            expiration_secs: 3600,
            issuer: None,
            audience: None,
        };

        let result = JwtValidator::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_auth_method_display() {
        assert_eq!(format!("{}", AuthMethod::MutualTls), "mTLS");
        assert_eq!(format!("{}", AuthMethod::Jwt), "JWT");
        assert_eq!(format!("{}", AuthMethod::ApiKey), "API Key");
    }
}
