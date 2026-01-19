//! Authorization module
//!
//! This module provides authorization services:
//! - Role-based access control (RBAC)
//! - Collection-level permissions
//! - Operation-level permissions (read/write/admin)
//! - Policy enforcement
//!
//! Security model:
//! - Deny by default (secure)
//! - Explicit permissions required
//! - Supports hierarchical roles

use crate::auth::Principal;
use crate::config::AuthorizationSettings;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info};

/// Authorization errors
#[derive(Error, Debug)]
pub enum AuthzError {
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Role not found: {0}")]
    RoleNotFound(String),

    #[error("Invalid permission: {0}")]
    InvalidPermission(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),
}

pub type AuthzResult<T> = Result<T, AuthzError>;

/// Permission levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
pub enum Permission {
    /// Read access
    Read,
    /// Write access (includes read)
    Write,
    /// Admin access (includes read and write)
    Admin,
}

impl Permission {
    /// Check if this permission includes another permission
    pub fn includes(&self, other: Permission) -> bool {
        matches!(
            (self, other),
            (Permission::Admin, _)
                | (Permission::Write, Permission::Read)
                | (Permission::Write, Permission::Write)
                | (Permission::Read, Permission::Read)
        )
    }

    /// Parse permission from string
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "read" => Some(Permission::Read),
            "write" => Some(Permission::Write),
            "admin" => Some(Permission::Admin),
            _ => None,
        }
    }
}

impl std::str::FromStr for Permission {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Permission::parse(s).ok_or(())
    }
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Permission::Read => write!(f, "read"),
            Permission::Write => write!(f, "write"),
            Permission::Admin => write!(f, "admin"),
        }
    }
}

/// Authorization action
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Read from collection
    Read,
    /// Write to collection
    Write,
    /// Delete from collection
    Delete,
    /// Create collection
    CreateCollection,
    /// Drop collection
    DropCollection,
    /// List collections
    ListCollections,
    /// Administrative operation
    Admin,
}

impl Action {
    /// Get the minimum permission level required for this action
    pub fn required_permission(&self) -> Permission {
        match self {
            Action::Read | Action::ListCollections => Permission::Read,
            Action::Write | Action::Delete => Permission::Write,
            Action::CreateCollection | Action::DropCollection | Action::Admin => Permission::Admin,
        }
    }
}

/// Resource being accessed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    /// Specific collection
    Collection(String),
    /// All collections
    AllCollections,
    /// Server administration
    Server,
}

/// Role definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    /// Role name
    pub name: String,
    /// Role description
    pub description: String,
    /// Permissions for this role
    pub permissions: Vec<PermissionRule>,
    /// Roles that this role inherits from
    #[serde(default)]
    pub inherits: Vec<String>,
}

/// Permission rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Resource pattern (e.g., "collection:users", "collection:*", "server")
    pub resource: String,
    /// Actions allowed (read, write, admin)
    pub actions: Vec<String>,
}

/// Policy file format
#[derive(Debug, Serialize, Deserialize)]
struct PolicyFile {
    /// Role definitions
    roles: Vec<Role>,
}

/// Authorization service
pub struct Authorizer {
    config: Arc<AuthorizationSettings>,
    roles: HashMap<String, Role>,
    default_role: String,
    deny_by_default: bool,
}

impl Authorizer {
    /// Create a new authorizer
    pub fn new(config: AuthorizationSettings) -> AuthzResult<Self> {
        let config = Arc::new(config);

        // Load roles
        let roles = if let Some(ref roles_file) = config.roles_file {
            Self::load_roles(roles_file)?
        } else {
            Self::default_roles()
        };

        let deny_by_default = config.default_mode == "deny-by-default";

        Ok(Self {
            default_role: config.default_role.clone(),
            config,
            roles,
            deny_by_default,
        })
    }

    /// Load roles from file
    fn load_roles(path: &Path) -> AuthzResult<HashMap<String, Role>> {
        if !path.exists() {
            return Err(AuthzError::ConfigError(format!(
                "Roles file does not exist: {}",
                path.display()
            )));
        }

        let contents = fs::read_to_string(path)?;

        // Try JSON first, then TOML
        let policy: PolicyFile = if path.extension().and_then(|s| s.to_str()) == Some("json") {
            serde_json::from_str(&contents)?
        } else {
            toml::from_str(&contents)?
        };

        let mut roles = HashMap::new();
        for role in policy.roles {
            roles.insert(role.name.clone(), role);
        }

        info!("Loaded {} roles from {}", roles.len(), path.display());
        Ok(roles)
    }

    /// Get default built-in roles
    fn default_roles() -> HashMap<String, Role> {
        let mut roles = HashMap::new();

        // Admin role - full access
        roles.insert("admin".to_string(), Role {
            name: "admin".to_string(),
            description: "Administrator with full access".to_string(),
            permissions: vec![PermissionRule {
                resource: "*".to_string(),
                actions: vec!["admin".to_string()],
            }],
            inherits: Vec::new(),
        });

        // User role - read/write to own collections
        roles.insert("user".to_string(), Role {
            name: "user".to_string(),
            description: "Regular user with read/write access".to_string(),
            permissions: vec![PermissionRule {
                resource: "collection:*".to_string(),
                actions: vec!["read".to_string(), "write".to_string()],
            }],
            inherits: Vec::new(),
        });

        // Reader role - read-only access
        roles.insert("reader".to_string(), Role {
            name: "reader".to_string(),
            description: "Read-only user".to_string(),
            permissions: vec![PermissionRule {
                resource: "collection:*".to_string(),
                actions: vec!["read".to_string()],
            }],
            inherits: Vec::new(),
        });

        info!("Using default built-in roles");
        roles
    }

    /// Check if a principal is authorized to perform an action on a resource
    pub fn authorize(
        &self,
        principal: &Principal,
        action: &Action,
        resource: &Resource,
    ) -> AuthzResult<()> {
        if !self.config.enabled {
            // Authorization disabled - allow all
            return Ok(());
        }

        // Get the user's role
        let role_name = principal
            .get_attribute("role")
            .map(|s| s.as_str())
            .unwrap_or(&self.default_role);

        // Check if user has permission
        if self.has_permission(role_name, action, resource)? {
            debug!(
                "Authorized: user={} role={} action={:?} resource={:?}",
                principal.name, role_name, action, resource
            );
            Ok(())
        } else {
            Err(AuthzError::PermissionDenied(format!(
                "User '{}' with role '{}' not authorized to {:?} on {:?}",
                principal.name, role_name, action, resource
            )))
        }
    }

    /// Check if a role has permission to perform an action on a resource
    fn has_permission(
        &self,
        role_name: &str,
        action: &Action,
        resource: &Resource,
    ) -> AuthzResult<bool> {
        let role = self
            .roles
            .get(role_name)
            .ok_or_else(|| AuthzError::RoleNotFound(role_name.to_string()))?;

        // Get all permissions (including inherited)
        let permissions = self.collect_permissions(role)?;

        // Check if any permission matches
        let required_permission = action.required_permission();

        for rule in &permissions {
            if self.matches_resource(&rule.resource, resource) {
                // Check if rule grants required permission
                for action_str in &rule.actions {
                    if let Some(granted_permission) = Permission::parse(action_str) {
                        if granted_permission.includes(required_permission) {
                            return Ok(true);
                        }
                    }
                }
            }
        }

        // No matching permission found
        Ok(!self.deny_by_default)
    }

    /// Collect all permissions for a role (including inherited)
    fn collect_permissions(&self, role: &Role) -> AuthzResult<Vec<PermissionRule>> {
        let mut permissions = role.permissions.clone();
        let mut visited = HashSet::new();
        visited.insert(role.name.clone());

        // Recursively collect inherited permissions
        for parent_name in &role.inherits {
            if visited.contains(parent_name) {
                // Circular inheritance detected
                continue;
            }

            let parent = self.roles.get(parent_name).ok_or_else(|| {
                AuthzError::ConfigError(format!("Parent role '{}' not found", parent_name))
            })?;

            let parent_permissions = self.collect_permissions(parent)?;
            permissions.extend(parent_permissions);
            visited.insert(parent_name.clone());
        }

        Ok(permissions)
    }

    /// Check if a resource pattern matches a specific resource
    fn matches_resource(&self, pattern: &str, resource: &Resource) -> bool {
        match (pattern, resource) {
            // Wildcard matches everything
            ("*", _) => true,

            // Collection patterns
            ("collection:*", Resource::Collection(_)) => true,
            ("collection:*", Resource::AllCollections) => true,

            // Specific collection
            (p, Resource::Collection(name)) if p.starts_with("collection:") => {
                let pattern_name = &p["collection:".len()..];
                pattern_name == name || pattern_name == "*"
            }

            // Server pattern
            ("server", Resource::Server) => true,

            _ => false,
        }
    }

    /// Get role information
    pub fn get_role(&self, role_name: &str) -> Option<&Role> {
        self.roles.get(role_name)
    }

    /// List all available roles
    pub fn list_roles(&self) -> Vec<&Role> {
        self.roles.values().collect()
    }

    /// Check if authorization is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthMethod;

    #[test]
    fn test_permission_includes() {
        assert!(Permission::Admin.includes(Permission::Read));
        assert!(Permission::Admin.includes(Permission::Write));
        assert!(Permission::Admin.includes(Permission::Admin));
        assert!(Permission::Write.includes(Permission::Read));
        assert!(Permission::Write.includes(Permission::Write));
        assert!(!Permission::Write.includes(Permission::Admin));
        assert!(Permission::Read.includes(Permission::Read));
        assert!(!Permission::Read.includes(Permission::Write));
        assert!(!Permission::Read.includes(Permission::Admin));
    }

    #[test]
    fn test_permission_from_str() {
        assert_eq!(Permission::parse("read"), Some(Permission::Read));
        assert_eq!(Permission::parse("write"), Some(Permission::Write));
        assert_eq!(Permission::parse("admin"), Some(Permission::Admin));
        assert_eq!(Permission::parse("invalid"), None);
    }

    #[test]
    fn test_action_required_permission() {
        assert_eq!(Action::Read.required_permission(), Permission::Read);
        assert_eq!(Action::Write.required_permission(), Permission::Write);
        assert_eq!(Action::Delete.required_permission(), Permission::Write);
        assert_eq!(
            Action::CreateCollection.required_permission(),
            Permission::Admin
        );
        assert_eq!(Action::Admin.required_permission(), Permission::Admin);
    }

    #[test]
    fn test_authorizer_creation() {
        let config = AuthorizationSettings {
            enabled: true,
            default_role: "user".to_string(),
            roles_file: None,
            policies_file: None,
            collection_permissions: true,
            default_mode: "deny-by-default".to_string(),
            audit_enabled: true,
            audit_log_path: None,
        };

        let authz = Authorizer::new(config).expect("Failed to create authorizer");
        assert!(authz.is_enabled());
        assert_eq!(authz.list_roles().len(), 3); // admin, user, reader
    }

    #[test]
    fn test_admin_role_authorization() {
        let config = AuthorizationSettings {
            enabled: true,
            default_role: "user".to_string(),
            roles_file: None,
            policies_file: None,
            collection_permissions: true,
            default_mode: "deny-by-default".to_string(),
            audit_enabled: true,
            audit_log_path: None,
        };

        let authz = Authorizer::new(config).expect("Failed to create authorizer");

        let principal = Principal::new(
            "admin1".to_string(),
            "Admin User".to_string(),
            AuthMethod::Jwt,
        )
        .with_attribute("role".to_string(), "admin".to_string());

        // Admin should have access to everything
        assert!(
            authz
                .authorize(
                    &principal,
                    &Action::Read,
                    &Resource::Collection("test".to_string())
                )
                .is_ok()
        );

        assert!(
            authz
                .authorize(
                    &principal,
                    &Action::Write,
                    &Resource::Collection("test".to_string())
                )
                .is_ok()
        );

        assert!(
            authz
                .authorize(&principal, &Action::CreateCollection, &Resource::Server)
                .is_ok()
        );
    }

    #[test]
    fn test_user_role_authorization() {
        let config = AuthorizationSettings {
            enabled: true,
            default_role: "user".to_string(),
            roles_file: None,
            policies_file: None,
            collection_permissions: true,
            default_mode: "deny-by-default".to_string(),
            audit_enabled: true,
            audit_log_path: None,
        };

        let authz = Authorizer::new(config).expect("Failed to create authorizer");

        let principal = Principal::new(
            "user1".to_string(),
            "Regular User".to_string(),
            AuthMethod::Jwt,
        )
        .with_attribute("role".to_string(), "user".to_string());

        // User should have read/write access to collections
        assert!(
            authz
                .authorize(
                    &principal,
                    &Action::Read,
                    &Resource::Collection("test".to_string())
                )
                .is_ok()
        );

        assert!(
            authz
                .authorize(
                    &principal,
                    &Action::Write,
                    &Resource::Collection("test".to_string())
                )
                .is_ok()
        );

        // User should NOT have admin access
        assert!(
            authz
                .authorize(&principal, &Action::CreateCollection, &Resource::Server)
                .is_err()
        );
    }

    #[test]
    fn test_reader_role_authorization() {
        let config = AuthorizationSettings {
            enabled: true,
            default_role: "user".to_string(),
            roles_file: None,
            policies_file: None,
            collection_permissions: true,
            default_mode: "deny-by-default".to_string(),
            audit_enabled: true,
            audit_log_path: None,
        };

        let authz = Authorizer::new(config).expect("Failed to create authorizer");

        let principal = Principal::new(
            "reader1".to_string(),
            "Read User".to_string(),
            AuthMethod::Jwt,
        )
        .with_attribute("role".to_string(), "reader".to_string());

        // Reader should have read access
        assert!(
            authz
                .authorize(
                    &principal,
                    &Action::Read,
                    &Resource::Collection("test".to_string())
                )
                .is_ok()
        );

        // Reader should NOT have write access
        assert!(
            authz
                .authorize(
                    &principal,
                    &Action::Write,
                    &Resource::Collection("test".to_string())
                )
                .is_err()
        );
    }

    #[test]
    fn test_authorization_disabled() {
        let config = AuthorizationSettings {
            enabled: false,
            default_role: "user".to_string(),
            roles_file: None,
            policies_file: None,
            collection_permissions: true,
            default_mode: "deny-by-default".to_string(),
            audit_enabled: true,
            audit_log_path: None,
        };

        let authz = Authorizer::new(config).expect("Failed to create authorizer");

        let principal = Principal::new(
            "user1".to_string(),
            "Test User".to_string(),
            AuthMethod::Jwt,
        );

        // When disabled, all operations should be allowed
        assert!(
            authz
                .authorize(&principal, &Action::Admin, &Resource::Server)
                .is_ok()
        );
    }

    #[test]
    fn test_resource_matching() {
        let config = AuthorizationSettings {
            enabled: true,
            default_role: "user".to_string(),
            roles_file: None,
            policies_file: None,
            collection_permissions: true,
            default_mode: "deny-by-default".to_string(),
            audit_enabled: true,
            audit_log_path: None,
        };

        let authz = Authorizer::new(config).expect("Failed to create authorizer");

        // Test wildcard pattern
        assert!(authz.matches_resource("*", &Resource::Collection("test".to_string())));
        assert!(authz.matches_resource("*", &Resource::Server));

        // Test collection pattern
        assert!(authz.matches_resource("collection:*", &Resource::Collection("test".to_string())));
        assert!(!authz.matches_resource("collection:*", &Resource::Server));

        // Test specific collection
        assert!(
            authz.matches_resource("collection:test", &Resource::Collection("test".to_string()))
        );
        assert!(!authz.matches_resource(
            "collection:test",
            &Resource::Collection("other".to_string())
        ));
    }
}
