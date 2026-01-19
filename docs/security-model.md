# AmateRS Security Model

## Threat Model

### Assumptions
1. **Honest-but-Curious Server**: Server follows protocol but may try to learn plaintext
2. **Network Adversary**: Can observe but not modify network traffic (TLS/QUIC)
3. **Client Trusted**: Client key management is secure

### Out of Scope
- Physical attacks on client devices
- Side-channel attacks (timing, power analysis)
- Social engineering attacks

## Threat Assessment

| Threat | Attack Vector | Countermeasure | Risk Level |
|--------|---------------|----------------|------------|
| **Server Snooping** | Admin reads database files | All data FHE-encrypted | ✅ Mitigated |
| **Memory Dump** | Attacker dumps server RAM | Data remains encrypted in memory | ✅ Mitigated |
| **Data Tampering** | Attacker modifies ciphertext | CRC32 checksums + future ZKPs | ⚠️ Partial |
| **Network MITM** | Intercept client-server traffic | mTLS + QUIC encryption | ✅ Mitigated |
| **Quantum Attack** | Quantum computer breaks encryption | TFHE is quantum-resistant (LWE) | ✅ Mitigated |
| **Computation Forgery** | Server returns wrong result | Future: Verifiable FHE | ⚠️ Planned |
| **Key Theft** | Attacker steals client keys | Client-side key management | ⚠️ User Responsibility |
| **DoS Attack** | Flood server with requests | Rate limiting + circuit breaker | ⚠️ Partial |

## Security Properties

### Confidentiality
- ✅ **Encryption at Rest**: All data encrypted on disk
- ✅ **Encryption in Transit**: TLS/QUIC for network
- ✅ **Encryption in Use**: FHE maintains encryption during computation

### Integrity
- ✅ **Checksums**: CRC32 for ciphertext integrity
- 🚧 **Verifiable Computation**: ZK-SNARKs (future)

### Availability
- ⚠️ **Replication**: Raft consensus for fault tolerance
- ⚠️ **DoS Protection**: Rate limiting (partial)

## Key Management

### Client Keys
- **Generation**: Client generates FHE key pairs locally
- **Storage**: Secure key storage (OS keychain recommended)
- **Rotation**: Periodic key rotation supported
- **Backup**: User responsibility - lost keys = lost data

### Server Keys
- **TLS Certificates**: For mTLS authentication
- **No Data Keys**: Server never has access to FHE keys

## Compliance

### GDPR
- ✅ **Right to Erasure**: Delete encrypted data + keys
- ✅ **Data Minimization**: Only encrypted data stored
- ✅ **Privacy by Design**: FHE ensures server can't read data

### HIPAA (Healthcare)
- ✅ **Encryption**: Exceeds HIPAA encryption requirements
- ✅ **Access Controls**: Cryptographic access control
- ⚠️ **Audit Logs**: Need to implement

## Best Practices

1. **Never log plaintext**: Only log encrypted data hashes
2. **Rotate keys regularly**: Annual key rotation recommended
3. **Monitor anomalies**: Watch for unusual query patterns
4. **Rate limit**: Protect against DoS attacks
5. **Update regularly**: Keep tfhe-rs and dependencies current

## Future Enhancements

1. **Verifiable FHE**: Prove computation correctness with ZK-SNARKs
2. **Multi-party Computation**: Threshold encryption for shared data
3. **Differential Privacy**: Add noise to query results
4. **Hardware Security**: Support for secure enclaves (SGX, SEV)
