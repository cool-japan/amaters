# AmateRS Security Best Practices

This guide is written for operators and security engineers deploying AmateRS in production. It covers operational security decisions: key management, transport security, authentication, authorization, audit logging, and the boundaries of what the system does and does not protect. For the underlying security model and threat-model assumptions, see [docs/security-model.md](security-model.md).

---

## 1. Key Management

AmateRS handles two distinct categories of cryptographic keys: FHE client keys and cluster log-encryption keys. These have very different operational profiles.

### FHE Client Keys

Fully Homomorphic Encryption keys are generated client-side by calling `FheKeyPair::generate()`. The resulting key pair has a strict split: the client key (secret) and the server evaluation key (`server_key`). The server evaluation key may be sent to the server and is required for server-side computation. The client key must never leave the client.

This is a hard operational constraint with no recovery path. If a client key is lost, every `CipherBlob` value encrypted under that key becomes permanently inaccessible. AmateRS does not implement key escrow, key wrapping for backup, or any server-side recovery mechanism. Operators must establish offline backup procedures for client keys before encrypting any data.

Recommended practices:

- Store client keys in hardware security modules (HSMs) or purpose-built secrets managers (e.g., HashiCorp Vault, AWS KMS) with strict access controls.
- Keep offline encrypted backups of client keys, tested for restorability, before writing any production data.
- Never log, transmit, or serialize the client key. Audit any code path that handles `FheKeyPair` to verify only the evaluation key is forwarded.
- Treat key loss as unrecoverable data loss. Document this in runbooks so on-call engineers do not attempt futile recovery procedures.

### Log Encryption (Cluster)

Cluster log entries are encrypted using `EntryEncryptor`, which applies AES-256-GCM with per-entry keys derived via HKDF. The `KeyManager` component manages the active key and a rotation history.

The `key_retention_count` configuration (default: 3) controls how many past keys are retained. Retained keys allow decryption of log entries written before a rotation. Reducing `key_retention_count` below the default risks losing the ability to read recent pre-rotation entries during a recovery scenario. Increasing it indefinitely is not advisable either, as retained keys represent additional material that must be protected.

Key rotation should be performed on a scheduled basis and whenever a key is suspected of compromise. After rotation, confirm that the old key is purged from memory and not persisted beyond the configured retention count.

---

## 2. TLS Configuration

All network communication should run over TLS. AmateRS uses rustls under the hood via tonic transport, which means no OpenSSL dependency and a memory-safe TLS stack.

### Enabling TLS

TLS is enabled via `network.tls_enabled = true`. The server certificate is loaded from the path given in `network.tls_cert`, and the private key from `network.tls_key`. Both fields are required when TLS is enabled. Leaving TLS disabled in any environment that carries real data or has network exposure is a misconfiguration.

### Mutual TLS (mTLS)

For server-to-server and privileged client connections, mutual TLS is strongly recommended. Set `network.require_client_cert = true` to require client authentication at the transport layer. When this is set, a CA certificate must be provided via `network.tls_ca`, and the server will reject any connection whose client certificate is not signed by that CA.

mTLS is built on the `TlsServerBuilder::build(config)` path, which constructs a `ServerTlsConfig`. When `require_client_cert` is true, client authentication is set to required — not optional. There is no partial or advisory mode. This means that enabling mTLS at the network layer is an all-or-nothing enforcement: all connecting clients must present a valid certificate.

Do not use `require_client_cert = false` in clusters where node-to-node authentication matters. Use it only when accepting connections from clients that cannot present certificates, and compensate with application-layer authentication (see Section 3).

### Certificate Rotation with LiveTlsAcceptor

AmateRS supports hot certificate rotation via `LiveTlsAcceptor`. When a new certificate is ready, call `rotate(new_config)` on the acceptor. The rotation is atomic at the acceptor level: new connections after the call use the new certificate, while in-flight connections complete under the old certificate. No server restart is required.

Operational recommendations:

- Automate certificate renewal (e.g., via ACME or an internal PKI) and wire the renewal callback to `rotate()`.
- Monitor certificate expiry and alert well before the rotation deadline — at least 14 days for short-lived certificates (90-day issuance), longer for annual certificates.
- Validate the new certificate and key pair before calling `rotate()`. A malformed rotation that takes effect immediately will break all new incoming connections.
- Keep the previous certificate in a rollback store until the rotation has been confirmed stable under production traffic.

---

## 3. Authentication and Authorization

### Authentication Overview

Authentication is disabled by default (`auth.enabled = false`). In any deployment that is not fully air-gapped and isolated, authentication must be explicitly enabled. The `reject_unauthenticated` flag defaults to true, meaning that once authentication is enabled, any request that does not carry valid credentials is rejected outright.

Three authentication methods are available via `AuthSettings`, and they can be combined.

**mTLS authentication** (`MtlsSettings`): Relies on client certificate validation already performed at the transport layer. Additional controls include `verify_cn` (default true), which validates the certificate's Common Name, `crl_path` for certificate revocation list checking, and `allowed_organizations` for restricting to specific certificate O fields. Always keep `verify_cn` enabled unless you have a strong reason not to.

**JWT authentication** (`JwtSettings`): Supports HS256, RS256, ES256, and EdDSA algorithms via the `algorithm` field. Symmetric HS256 is only appropriate when the secret cannot leave the signing service; for any multi-party scenario, use an asymmetric algorithm (RS256, ES256, or EdDSA) with the public key loaded via `public_key_path`, `ec_public_key_path`, or `ed_public_key_path`. Token lifetime is controlled by `expiration_secs` (default 3600 seconds). Set `issuer` and `audience` to prevent token reuse across systems — if either field is configured on the server, incoming JWTs must carry matching claims. Short expiration values (under 15 minutes for highly privileged operations) reduce the window of opportunity for stolen token abuse.

**API key authentication** (`ApiKeySettings`): Keys are read from the file at `keys_file` and the key is expected in the HTTP header named by `header_name` (default: `X-API-Key`). The `hash_keys` flag defaults to true and should never be set to false in production. Storing plaintext API keys on disk defeats the purpose of the mechanism and leaves credentials exposed if the keys file is read by an unauthorized process.

### Authorization

Authorization is enabled by default (`enabled = true` in `AuthorizationSettings`) and uses a deny-by-default mode (`default_mode = "deny-by-default"`). Do not switch to `"allow-by-default"` in production. The deny-by-default posture means that new users and new resources start with no permissions, and access must be explicitly granted.

The `Permission` enum has three levels: `Read`, `Write`, and `Admin`. Assign the minimum necessary permission level. Use `Admin` only for principals that genuinely need administrative access.

Per-collection RBAC is enabled via `collection_permissions = true`. This allows different permissions to be configured per collection, which is essential in multi-tenant or multi-classification deployments. Treat collection-level permissions as your primary access isolation mechanism. If `collection_permissions` is disabled, all principals with `Write` access can write to all collections — which is likely not the intended posture.

---

## 4. Audit Logging

The audit module captures security-relevant events as structured JSON. Each `AuditEvent` record includes a UUID event identifier, timestamp, event type, outcome, the acting principal, the authentication method used, the action performed, the target resource, and any associated error message.

`AuditEventType` covers: `Authentication`, `Authorization`, `Admin`, `SecurityViolation`, and `ConfigChange`. All five types are important from a security operations perspective. `SecurityViolation` and failed `Authorization` events in particular should be routed to a SIEM or alerting system.

`AuditOutcome` has three values: `Success`, `Failure`, and `Denied`. Sustained sequences of `Denied` from a single principal, or `Failure` on `Authentication` events, are indicators of probing or compromised credentials.

Audit logging is enabled by default via `audit_enabled = true` in `AuthorizationSettings`. Do not disable it. The output path is configured via `authz.audit_log_path`.

Operational requirements:

- Write audit logs to append-only storage. The process writing audit events should not have delete or overwrite permissions on the audit log directory.
- Ship audit logs to a remote sink (e.g., a SIEM, object storage with write-once policies) in near real-time. Local-only audit logs are vulnerable to deletion if a node is compromised.
- Establish a retention policy. Regulatory requirements vary, but a minimum of 90 days online with 1 year archived is a common baseline for security event data.
- Alert on gaps in audit log continuity. A node that stops producing audit events while continuing to serve traffic is a signal worth investigating.
- The UUID `id` field enables cross-referencing a denied request in application logs with the corresponding audit event.

---

## 5. Constant-Time Operations

AmateRS implements constant-time comparison and selection primitives in `constant_time_eq`, `constant_time_select`, and `constant_time_select_slice`. These are used in the authentication middleware and the API key validator to prevent timing side-channel attacks on credential comparison.

The XOR-accumulate pattern used in `constant_time_eq` ensures that the comparison time does not vary based on where the first differing byte appears, which would otherwise allow an attacker to recover a secret one byte at a time via repeated timing measurements.

### Known Limitation: Key Length Leakage

The constant-time comparison is applied only to the key value bytes. If the submitted key length differs from the expected key length, the function returns false immediately without entering the XOR loop. This means that the response time is shorter for wrong-length inputs than for correct-length inputs, leaking the length of the valid API key to a network observer capable of making many timed requests.

This is an accepted limitation and is consistent with standard practice in most authentication libraries. The practical exploitability is low: length information alone does not allow an attacker to reconstruct the key value, and the attacker must already be on a network path that allows precise latency measurement. Nonetheless, operators should be aware of this behavior. Using API keys of a fixed, standard length (e.g., 32 bytes) reduces the information value of any length leakage.

---

## 6. Rolling Upgrades and Version Compatibility

AmateRS uses a structured version handshake on peer connection. The current version is 0.2.2 and the minimum compatible version is 0.2.0, as expressed by `MIN_COMPATIBLE_VERSION` and `CURRENT_VERSION`. Peers with a different major version or a minor version below the minimum are rejected by `is_compatible()`.

The `VersionHandshake` message exchanged on connect carries the node's current version, its declared minimum compatible version, and a build identifier. Both nodes in a connection exchange this information, so incompatibility is detected symmetrically — either side can reject the connection.

For the current release line, nodes running 0.2.0, 0.2.1, and 0.2.2 can coexist in a cluster. This means rolling upgrades within the 0.2.x series are supported: you can upgrade nodes one at a time without taking the cluster offline.

Upgrade procedure recommendations:

- Upgrade nodes one at a time and allow each node to rejoin the cluster and stabilize before proceeding to the next.
- Monitor for `is_compatible()` rejection errors in logs during the upgrade window. Rejections indicate a version skew problem and may mean a node is running an older build than expected.
- Do not leave a cluster in a mixed-version state for longer than necessary. Mixed-version clusters are a transitional state, not a long-term configuration.
- After completing an upgrade, verify that all nodes are reporting the expected version via the version handshake and that no compatibility rejections are occurring.
- Be aware that a future minor version bump (e.g., to 0.3.x) may change `MIN_COMPATIBLE_VERSION` in a way that excludes 0.2.x nodes entirely. Consult the changelog before upgrading across minor boundaries.

---

## 7. What FHE Does Not Protect

FHE allows the server to perform computations on encrypted values without decrypting them, which is a strong privacy property. However, FHE in AmateRS does not provide full oblivious computation. Operators and users must understand the following limitations:

**Access patterns are visible.** The server can observe which keys are queried. The key namespace is not hidden. An attacker with server access can determine which records are accessed and how frequently, even without being able to read the encrypted values.

**Query timing reveals circuit complexity.** FHE operations have execution times that vary with the type of computation being performed. An observer can infer something about the nature of a query from how long it takes to execute.

**Key length is observable.** The server sees ciphertext sizes, which are related to the plaintext type width. This does not reveal plaintext values but does leak type information.

**Metadata is plaintext.** Collection names, key names, and query structure are not encrypted. A server operator or attacker with read access to the server can see the full metadata layer of the data model.

**Computation correctness is not verifiable.** The server could, in principle, return a malformed or incorrect ciphertext as a query result without the client being able to detect it cryptographically. Verifiable FHE via ZK-SNARKs is planned as a post-1.0 feature and is not implemented in the current release.

**Byzantine behavior is a separate concern.** Node-level Byzantine fault tolerance is discussed in [docs/bft-evaluation.md](bft-evaluation.md).

The FHE layer protects value confidentiality while computation is in progress on the server. It does not substitute for access control, transport security, or audit logging, all of which remain necessary.

---

## 8. Operator Checklist

Use this checklist before placing an AmateRS deployment in production or before a security review.

### Transport Security
- [ ] `network.tls_enabled` is set to `true` on all nodes
- [ ] Certificate and key paths (`network.tls_cert`, `network.tls_key`) point to valid, non-expired credentials
- [ ] Certificate expiry monitoring and automated rotation are configured
- [ ] For cluster and privileged connections, `network.require_client_cert = true` is set
- [ ] `network.tls_ca` is set to a CA that only signs authorized clients

### Authentication
- [ ] `auth.enabled = true` is set; default-disabled authentication has been explicitly turned on
- [ ] `reject_unauthenticated = true` is confirmed (default, but verify)
- [ ] If using JWT, an asymmetric algorithm (RS256, ES256, or EdDSA) is in use for multi-party deployments
- [ ] JWT `issuer` and `audience` fields are configured to prevent cross-system token reuse
- [ ] JWT `expiration_secs` is appropriate for the sensitivity of the operations being performed
- [ ] If using API keys, `hash_keys = true` is confirmed (never store plaintext keys)
- [ ] If using mTLS auth, `verify_cn = true` is confirmed and `allowed_organizations` is populated

### Authorization
- [ ] `authz.enabled = true` is confirmed
- [ ] `default_mode = "deny-by-default"` is in use
- [ ] Per-collection RBAC (`collection_permissions = true`) is enabled for multi-tenant deployments
- [ ] Principal roles follow least-privilege; `Admin` permission is granted only where required

### Audit Logging
- [ ] `audit_enabled = true` is confirmed
- [ ] `authz.audit_log_path` points to append-only, adequately sized storage
- [ ] Audit logs are being shipped to a remote sink
- [ ] Alerting is configured on `SecurityViolation` and `Denied` audit outcomes
- [ ] A log retention policy is in place (minimum 90 days online recommended)

### Key Management
- [ ] FHE client keys have offline encrypted backups, tested for restorability
- [ ] Client key material has been audited to confirm it is never transmitted to the server
- [ ] `key_retention_count` has been reviewed for the cluster's recovery requirements
- [ ] Log encryption key rotation is scheduled and documented in runbooks

### Upgrades
- [ ] All nodes in the cluster are within the compatible version range (0.2.0–0.2.2)
- [ ] Version compatibility rejection errors are monitored during and after upgrades
- [ ] Upgrade procedure specifies one-node-at-a-time sequencing
