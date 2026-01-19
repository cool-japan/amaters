# AmateRS Server Authentication & Authorization

This document describes the authentication and authorization system implemented in AmateRS Server.

## Overview

AmateRS Server implements a comprehensive security layer with:

- **Multiple authentication methods**: mTLS, JWT, API Keys
- **Role-based access control (RBAC)**: Fine-grained permissions
- **Audit logging**: Complete security event tracking
- **Secure by default**: Deny-by-default policy

## Authentication Methods

### 1. mTLS (Mutual TLS) - Recommended

mTLS provides the strongest security by requiring both the server and client to authenticate using X.509 certificates.

**Configuration:**

```toml
[network]
tls_enabled = true
tls_cert = "/path/to/server.crt"
tls_key = "/path/to/server.key"
tls_ca = "/path/to/ca.crt"
require_client_cert = true

[auth.mtls]
enabled = true
ca_certs_dir = "/path/to/trusted_cas"
crl_path = "/path/to/crl.pem"  # Optional
verify_cn = true
allowed_organizations = ["Your Org", "Partner Org"]  # Optional
```

**How it works:**
1. Server validates client certificate against trusted CAs
2. Extracts Common Name (CN) as user identity
3. Optionally verifies organization (O) field
4. Checks certificate validity period and revocation status

### 2. JWT (JSON Web Tokens)

JWT provides stateless authentication suitable for distributed systems.

**Supported algorithms:**
- HS256 (HMAC with SHA-256) - symmetric key
- RS256 (RSA with SHA-256) - asymmetric keys

**Configuration with HS256:**

```toml
[auth.jwt]
enabled = true
secret = "your-secret-key"
algorithm = "HS256"
expiration_secs = 3600
issuer = "amaters-server"
audience = "amaters-clients"
```

**Configuration with RS256:**

```toml
[auth.jwt]
enabled = true
public_key_path = "/path/to/public_key.pem"
algorithm = "RS256"
expiration_secs = 3600
issuer = "amaters-server"
audience = "amaters-clients"
```

**Expected JWT claims:**
```json
{
  "sub": "user-id",
  "name": "User Name",
  "exp": 1735689600,
  "iat": 1735686000,
  "iss": "amaters-server",
  "aud": "amaters-clients",
  "roles": ["admin"],
  "custom_field": "custom_value"
}
```

### 3. API Keys

API keys provide simple authentication for service accounts and automation.

**Configuration:**

```toml
[auth.api_key]
enabled = true
keys_file = "/path/to/api_keys.json"
header_name = "X-API-Key"
hash_keys = true
```

**API Keys file format:**

```json
[
  {
    "id": "key-001",
    "name": "Service Account Key",
    "key_hash": "base64-encoded-sha256-hash",
    "user_id": "service-account-1",
    "roles": ["admin"],
    "attributes": {
      "description": "Key for automated deployments"
    }
  }
]
```

**Generating API key hash:**

```bash
echo -n "your-api-key" | sha256sum | xxd -r -p | base64
```

## Authorization (RBAC)

### Built-in Roles

AmateRS Server includes three built-in roles:

1. **admin**: Full access to all resources
   - All operations on all collections
   - Server administration
   - User management

2. **user**: Standard user with read/write access
   - Read/write to all collections
   - Cannot create/drop collections
   - Cannot perform admin operations

3. **reader**: Read-only access
   - Read from all collections
   - Cannot modify data
   - Cannot create/drop collections

### Custom Roles

Define custom roles in a TOML file:

```toml
[[roles]]
name = "data_scientist"
description = "Data scientist with specific collection access"
permissions = [
    { resource = "collection:datasets", actions = ["read", "write"] },
    { resource = "collection:models", actions = ["read"] }
]
inherits = ["reader"]
```

**Permission levels:**
- `read`: Read-only access
- `write`: Read and write access (includes read)
- `admin`: Full administrative access (includes read and write)

**Resource patterns:**
- `*`: All resources
- `collection:*`: All collections
- `collection:name`: Specific collection
- `server`: Server administration

### Authorization Configuration

```toml
[authz]
enabled = true
default_role = "user"
roles_file = "/path/to/roles.toml"
collection_permissions = true
default_mode = "deny-by-default"
audit_enabled = true
audit_log_path = "/var/log/amaters/audit.jsonl"
```

## Audit Logging

All authentication and authorization events are logged for security auditing.

### Audit Event Types

1. **Authentication**: Login attempts (success/failure)
2. **Authorization**: Permission checks (allow/deny)
3. **Admin**: Administrative operations
4. **SecurityViolation**: Security-related incidents
5. **ConfigChange**: Configuration modifications

### Audit Log Format

Logs are written in JSON Lines format (one JSON object per line):

```json
{
  "id": "uuid-v4",
  "timestamp": "2026-01-17T12:00:00Z",
  "event_type": "authentication",
  "result": "success",
  "principal": {
    "id": "user123",
    "name": "John Doe",
    "role": "admin"
  },
  "auth_method": "JWT",
  "source_ip": "192.168.1.100"
}
```

### Viewing Audit Logs

```bash
# View recent authentication events
jq 'select(.event_type == "authentication")' /var/log/amaters/audit.jsonl

# View denied authorization attempts
jq 'select(.event_type == "authorization" and .result == "denied")' /var/log/amaters/audit.jsonl

# View all events for a specific user
jq 'select(.principal.id == "user123")' /var/log/amaters/audit.jsonl
```

## Security Best Practices

### 1. Authentication

- **Use mTLS in production**: Strongest security with certificate-based auth
- **Rotate JWT secrets regularly**: Change secrets every 90 days
- **Use RS256 for JWT in distributed systems**: Easier key distribution
- **Hash API keys**: Always enable `hash_keys = true`
- **Limit API key lifetime**: Set expiration dates in key attributes

### 2. Authorization

- **Principle of least privilege**: Grant minimum necessary permissions
- **Use deny-by-default**: Always set `default_mode = "deny-by-default"`
- **Review roles regularly**: Audit role definitions quarterly
- **Separate admin accounts**: Use dedicated admin accounts, not shared credentials

### 3. Audit Logging

- **Enable audit logging**: Always set `audit_enabled = true`
- **Secure audit logs**: Protect audit log files with appropriate permissions
- **Monitor audit logs**: Set up alerts for security violations
- **Retain logs**: Keep audit logs for compliance requirements (typically 1+ year)

### 4. Network Security

- **Enable TLS**: Always use TLS in production
- **Require client certificates**: Set `require_client_cert = true` for mTLS
- **Restrict allowed organizations**: Use `allowed_organizations` for certificate validation
- **Use CRL**: Configure certificate revocation lists

### 5. Configuration Security

- **Protect configuration files**: Set restrictive file permissions (600 or 400)
- **Use environment variables**: For sensitive values like JWT secrets
- **Validate configuration**: Run `amaters-server validate-config` before deployment
- **Backup configurations**: Keep encrypted backups of auth configurations

## Example Deployment

### Production Setup with mTLS

1. **Generate certificates:**

```bash
# Generate CA
openssl genrsa -out ca.key 4096
openssl req -new -x509 -days 3650 -key ca.key -out ca.crt

# Generate server certificate
openssl genrsa -out server.key 4096
openssl req -new -key server.key -out server.csr
openssl x509 -req -days 365 -in server.csr -CA ca.crt -CAkey ca.key -out server.crt

# Generate client certificate
openssl genrsa -out client.key 4096
openssl req -new -key client.key -out client.csr
openssl x509 -req -days 365 -in client.csr -CA ca.crt -CAkey ca.key -out client.crt
```

2. **Configure server:**

```toml
[network]
tls_enabled = true
tls_cert = "/etc/amaters/certs/server.crt"
tls_key = "/etc/amaters/certs/server.key"
tls_ca = "/etc/amaters/certs/ca.crt"
require_client_cert = true

[auth.mtls]
enabled = true
ca_certs_dir = "/etc/amaters/certs/trusted_cas"
verify_cn = true

[authz]
enabled = true
default_role = "user"
default_mode = "deny-by-default"
audit_enabled = true
audit_log_path = "/var/log/amaters/audit.jsonl"
```

3. **Set file permissions:**

```bash
chmod 600 /etc/amaters/certs/*.key
chmod 644 /etc/amaters/certs/*.crt
chmod 600 /etc/amaters/config.toml
```

4. **Start server:**

```bash
amaters-server start --config /etc/amaters/config.toml
```

## Troubleshooting

### Authentication Issues

**Problem**: "Authentication failed: Invalid credentials"

- **JWT**: Verify token hasn't expired, check secret/public key
- **mTLS**: Verify client certificate is signed by trusted CA
- **API Key**: Verify key hash matches, check keys file path

**Problem**: "Certificate validation failed"

- Check certificate validity period (not expired/not yet valid)
- Verify certificate chain (client cert → CA)
- Check organization field matches `allowed_organizations`
- Verify CRL if configured

### Authorization Issues

**Problem**: "Permission denied"

- Verify user's role in JWT claims or API key attributes
- Check role definition in roles file
- Verify resource pattern matches
- Review audit logs for authorization denials

### Configuration Issues

**Problem**: "Configuration validation failed"

- Run `amaters-server validate-config --show` to see effective config
- Check for required fields (e.g., JWT secret when HS256 is used)
- Verify file paths exist and are readable

## API Reference

See inline documentation in:
- `src/auth.rs`: Authentication implementation
- `src/authz.rs`: Authorization implementation
- `src/audit.rs`: Audit logging implementation
- `src/config.rs`: Configuration structures

## Examples

See the `examples/` directory for:
- `server_with_auth.toml`: Complete server configuration with auth
- `roles.toml`: Custom role definitions
- `api_keys.json`: API key configuration

## Future Enhancements

Planned improvements:
- LDAP/Active Directory integration
- SAML 2.0 support
- OAuth2/OIDC integration
- Dynamic role assignment
- Time-based access control
- IP-based access restrictions
- Multi-factor authentication (MFA)
