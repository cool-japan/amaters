//! Audit logging module
//!
//! This module provides audit logging for security events:
//! - Authentication attempts (success/failure)
//! - Authorization decisions (allow/deny)
//! - Administrative operations
//! - Suspicious activities
//!
//! Audit logs are written in structured JSON format for easy parsing and analysis.

use crate::auth::{AuthMethod, Principal};
use crate::authz::{Action, Resource};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tracing::warn;
use uuid::Uuid;

/// Audit errors
#[derive(Error, Debug)]
pub enum AuditError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Audit log not configured")]
    NotConfigured,
}

pub type AuditResult<T> = Result<T, AuditError>;

/// Audit event type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    /// Authentication attempt
    Authentication,
    /// Authorization check
    Authorization,
    /// Administrative operation
    Admin,
    /// Security violation
    SecurityViolation,
    /// Configuration change
    ConfigChange,
}

/// Audit event result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    /// Operation succeeded
    Success,
    /// Operation failed
    Failure,
    /// Operation denied by policy
    Denied,
}

/// Audit event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Event ID
    pub id: String,

    /// Event timestamp
    pub timestamp: DateTime<Utc>,

    /// Event type
    pub event_type: AuditEventType,

    /// Event result
    pub result: AuditOutcome,

    /// User principal (if authenticated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<PrincipalInfo>,

    /// Authentication method used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,

    /// Action performed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,

    /// Resource accessed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,

    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,

    /// Source IP address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ip: Option<String>,
}

/// Simplified principal info for audit logs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrincipalInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl From<&Principal> for PrincipalInfo {
    fn from(principal: &Principal) -> Self {
        Self {
            id: principal.id.clone(),
            name: principal.name.clone(),
            role: principal.get_attribute("role").cloned(),
        }
    }
}

/// Audit logger
pub struct AuditLogger {
    writer: Arc<Mutex<Option<BufWriter<File>>>>,
    log_path: Option<PathBuf>,
}

impl AuditLogger {
    /// Create a new audit logger
    pub fn new(log_path: Option<PathBuf>) -> AuditResult<Self> {
        let writer = if let Some(ref path) = log_path {
            Some(Self::open_log_file(path)?)
        } else {
            None
        };

        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            log_path,
        })
    }

    /// Open or create the audit log file
    fn open_log_file(path: &Path) -> AuditResult<BufWriter<File>> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(path)?;

        Ok(BufWriter::new(file))
    }

    /// Log an audit event
    pub fn log(&self, event: AuditEvent) -> AuditResult<()> {
        // Always log to tracing
        match event.result {
            AuditOutcome::Success => {
                tracing::info!(
                    event_id = %event.id,
                    event_type = ?event.event_type,
                    principal = ?event.principal,
                    action = ?event.action,
                    resource = ?event.resource,
                    "Audit: Success"
                );
            }
            AuditOutcome::Failure => {
                tracing::warn!(
                    event_id = %event.id,
                    event_type = ?event.event_type,
                    principal = ?event.principal,
                    error = ?event.error,
                    "Audit: Failure"
                );
            }
            AuditOutcome::Denied => {
                tracing::warn!(
                    event_id = %event.id,
                    event_type = ?event.event_type,
                    principal = ?event.principal,
                    action = ?event.action,
                    resource = ?event.resource,
                    "Audit: Denied"
                );
            }
        }

        // Write to file if configured
        if let Ok(mut writer_guard) = self.writer.lock() {
            if let Some(ref mut writer) = *writer_guard {
                let json = serde_json::to_string(&event)?;
                writeln!(writer, "{}", json)?;
                writer.flush()?;
            }
        }

        Ok(())
    }

    /// Log successful authentication
    pub fn log_auth_success(&self, principal: &Principal, source_ip: Option<String>) {
        let event = AuditEvent {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::Authentication,
            result: AuditOutcome::Success,
            principal: Some(principal.into()),
            auth_method: Some(principal.auth_method.to_string()),
            action: None,
            resource: None,
            error: None,
            metadata: None,
            source_ip,
        };

        if let Err(e) = self.log(event) {
            warn!("Failed to log audit event: {}", e);
        }
    }

    /// Log failed authentication
    pub fn log_auth_failure(
        &self,
        auth_method: AuthMethod,
        error: &str,
        source_ip: Option<String>,
    ) {
        let event = AuditEvent {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::Authentication,
            result: AuditOutcome::Failure,
            principal: None,
            auth_method: Some(auth_method.to_string()),
            action: None,
            resource: None,
            error: Some(error.to_string()),
            metadata: None,
            source_ip,
        };

        if let Err(e) = self.log(event) {
            warn!("Failed to log audit event: {}", e);
        }
    }

    /// Log successful authorization
    pub fn log_authz_success(
        &self,
        principal: &Principal,
        action: &Action,
        resource: &Resource,
        source_ip: Option<String>,
    ) {
        let event = AuditEvent {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::Authorization,
            result: AuditOutcome::Success,
            principal: Some(principal.into()),
            auth_method: None,
            action: Some(format!("{:?}", action)),
            resource: Some(format!("{:?}", resource)),
            error: None,
            metadata: None,
            source_ip,
        };

        if let Err(e) = self.log(event) {
            warn!("Failed to log audit event: {}", e);
        }
    }

    /// Log authorization denial
    pub fn log_authz_denied(
        &self,
        principal: &Principal,
        action: &Action,
        resource: &Resource,
        reason: &str,
        source_ip: Option<String>,
    ) {
        let event = AuditEvent {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::Authorization,
            result: AuditOutcome::Denied,
            principal: Some(principal.into()),
            auth_method: None,
            action: Some(format!("{:?}", action)),
            resource: Some(format!("{:?}", resource)),
            error: Some(reason.to_string()),
            metadata: None,
            source_ip,
        };

        if let Err(e) = self.log(event) {
            warn!("Failed to log audit event: {}", e);
        }
    }

    /// Log administrative operation
    pub fn log_admin_operation(
        &self,
        principal: &Principal,
        operation: &str,
        success: bool,
        error: Option<String>,
        source_ip: Option<String>,
    ) {
        let event = AuditEvent {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::Admin,
            result: if success {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failure
            },
            principal: Some(principal.into()),
            auth_method: None,
            action: Some(operation.to_string()),
            resource: None,
            error,
            metadata: None,
            source_ip,
        };

        if let Err(e) = self.log(event) {
            warn!("Failed to log audit event: {}", e);
        }
    }

    /// Log security violation
    pub fn log_security_violation(
        &self,
        principal: Option<&Principal>,
        violation: &str,
        source_ip: Option<String>,
    ) {
        let event = AuditEvent {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::SecurityViolation,
            result: AuditOutcome::Denied,
            principal: principal.map(|p| p.into()),
            auth_method: None,
            action: None,
            resource: None,
            error: Some(violation.to_string()),
            metadata: None,
            source_ip,
        };

        if let Err(e) = self.log(event) {
            warn!("Failed to log audit event: {}", e);
        }
    }

    /// Log configuration change
    pub fn log_config_change(
        &self,
        principal: &Principal,
        change_description: &str,
        source_ip: Option<String>,
    ) {
        let event = AuditEvent {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::ConfigChange,
            result: AuditOutcome::Success,
            principal: Some(principal.into()),
            auth_method: None,
            action: Some(change_description.to_string()),
            resource: None,
            error: None,
            metadata: None,
            source_ip,
        };

        if let Err(e) = self.log(event) {
            warn!("Failed to log audit event: {}", e);
        }
    }

    /// Check if audit logging is configured
    pub fn is_configured(&self) -> bool {
        self.log_path.is_some()
    }

    /// Get the audit log path
    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self {
            writer: Arc::new(Mutex::new(None)),
            log_path: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthMethod;
    use std::env;

    #[test]
    fn test_audit_event_serialization() {
        let event = AuditEvent {
            id: "test-123".to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::Authentication,
            result: AuditOutcome::Success,
            principal: Some(PrincipalInfo {
                id: "user1".to_string(),
                name: "Test User".to_string(),
                role: Some("admin".to_string()),
            }),
            auth_method: Some("JWT".to_string()),
            action: None,
            resource: None,
            error: None,
            metadata: None,
            source_ip: Some("192.168.1.1".to_string()),
        };

        let json = serde_json::to_string(&event).expect("Failed to serialize");
        assert!(json.contains("test-123"));
        assert!(json.contains("user1"));
    }

    #[test]
    fn test_audit_logger_without_file() {
        let logger = AuditLogger::new(None).expect("Failed to create logger");
        assert!(!logger.is_configured());

        let principal = Principal::new(
            "user1".to_string(),
            "Test User".to_string(),
            AuthMethod::Jwt,
        );

        // Should not fail even without file
        logger.log_auth_success(&principal, None);
    }

    #[test]
    fn test_audit_logger_with_file() {
        let temp_dir = env::temp_dir();
        let log_path = temp_dir.join(format!("audit_test_{}.jsonl", Uuid::new_v4()));

        let logger = AuditLogger::new(Some(log_path.clone())).expect("Failed to create logger");
        assert!(logger.is_configured());

        let principal = Principal::new(
            "user1".to_string(),
            "Test User".to_string(),
            AuthMethod::Jwt,
        );

        logger.log_auth_success(&principal, Some("127.0.0.1".to_string()));

        // Verify file was created
        assert!(log_path.exists());

        // Cleanup
        std::fs::remove_file(&log_path).ok();
    }

    #[test]
    fn test_principal_info_conversion() {
        let principal = Principal::new(
            "user1".to_string(),
            "Test User".to_string(),
            AuthMethod::Jwt,
        )
        .with_attribute("role".to_string(), "admin".to_string());

        let info: PrincipalInfo = (&principal).into();
        assert_eq!(info.id, "user1");
        assert_eq!(info.name, "Test User");
        assert_eq!(info.role, Some("admin".to_string()));
    }

    #[test]
    fn test_log_auth_failure() {
        let logger = AuditLogger::new(None).expect("Failed to create logger");

        logger.log_auth_failure(
            AuthMethod::Jwt,
            "Invalid token",
            Some("192.168.1.1".to_string()),
        );

        // Should not panic
    }

    #[test]
    fn test_log_authz_denied() {
        let logger = AuditLogger::new(None).expect("Failed to create logger");

        let principal = Principal::new(
            "user1".to_string(),
            "Test User".to_string(),
            AuthMethod::Jwt,
        );

        logger.log_authz_denied(
            &principal,
            &Action::Admin,
            &Resource::Server,
            "Insufficient permissions",
            Some("192.168.1.1".to_string()),
        );

        // Should not panic
    }

    #[test]
    fn test_log_security_violation() {
        let logger = AuditLogger::new(None).expect("Failed to create logger");

        let principal = Principal::new(
            "user1".to_string(),
            "Test User".to_string(),
            AuthMethod::Jwt,
        );

        logger.log_security_violation(
            Some(&principal),
            "Attempted SQL injection",
            Some("192.168.1.1".to_string()),
        );

        // Should not panic
    }
}
