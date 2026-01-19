# amaters-server TODO

## Status Summary

**Phase 1 (Basic Server):** ✅ COMPLETE
**Phase 2 (Component Integration):** ✅ COMPLETE (MVP)
**Phase 3 (Request Handling):** ✅ COMPLETE (Basic queries + Auth/Authz)
**Phase 4 (Observability):** ✅ COMPLETE (Basic)
**Phase 5-8:** 📋 Future work

The server binary now has:
- Full CLI with start/stop/status/version/validate-config commands
- Configuration loading from TOML, environment variables, and CLI overrides
- Graceful shutdown with signal handling
- Memory storage integration (LSM-Tree pending trait implementation)
- **AQL service integration with storage engine**
- **Query execution handlers (GET, SET, DELETE, RANGE)**
- **Health check and server info endpoints**
- Health checking infrastructure
- Metrics collection (Prometheus-compatible)
- Structured logging with tracing
- **Authentication system (mTLS, JWT, API keys)**
- **Authorization system (RBAC with configurable roles)**
- **Audit logging for security events**

## Phase 1: Basic Server ✅

- [x] Command-line interface
  - [x] `start` command
  - [x] `stop` command
  - [x] `status` command
  - [x] `version` command
  - [x] `validate-config` command
- [x] Configuration loading
  - [x] TOML parser
  - [x] Environment variables
  - [x] CLI argument overrides
  - [x] Configuration validation
- [x] Graceful shutdown
  - [x] Signal handling (SIGTERM, SIGINT)
  - [x] Flush pending operations
  - [x] Close connections
  - [x] Save state

## Phase 2: Component Integration ✅ (Complete for MVP)

- [x] Storage integration (MemoryStorage only for now)
  - [x] Initialize storage engine
  - [x] Configure cache
  - [x] Set up WAL
  - [ ] LSM-Tree integration (waiting for StorageEngine trait impl)
- [ ] Compute integration (Future work)
  - [ ] Initialize FHE executor
  - [ ] Configure GPU backend
  - [ ] Circuit cache
- [x] Network integration ✅
  - [x] AQL service implementation (`amaters-net/src/server.rs`)
  - [x] Network service module (`src/service.rs`)
  - [x] Service integration with server runtime
  - [ ] Full gRPC server (using simplified approach for MVP)
  - [ ] Configure TLS
  - [ ] Connection pooling
- [ ] Cluster integration (Future work)
  - [ ] Initialize Raft
  - [ ] Join cluster
  - [ ] Shard management

## Phase 3: Request Handling ✅ (Basic Complete)

- [x] Query execution ✅
  - [x] Parse AQL queries (proto messages)
  - [x] Execute GET queries on storage
  - [x] Execute SET queries on storage
  - [x] Execute DELETE queries on storage
  - [x] Execute RANGE queries on storage
  - [ ] Execute FILTER queries (pending - requires FHE integration)
  - [ ] Execute UPDATE queries (pending - requires FHE integration)
  - [x] Return results (proto response format)
- [x] Error handling ✅
  - [x] Convert internal errors (NetError conversion)
  - [x] Error responses (ErrorResponse proto)
  - [x] Error categorization (retryable/non-retryable)
  - [ ] Retry logic (future enhancement)
- [x] Authentication ✅
  - [x] Client certificate validation (mTLS)
  - [x] Token-based auth (JWT - HS256/RS256)
  - [x] API keys (with hashing support)
  - [x] Authentication module (`src/auth.rs`)
  - [x] Multiple auth methods support
- [x] Authorization ✅
  - [x] Role-based access control (RBAC)
  - [x] Collection-level permissions
  - [x] Operation-level permissions (read/write/admin)
  - [x] Policy enforcement
  - [x] Authorization module (`src/authz.rs`)
  - [x] Built-in roles (admin/user/reader)
  - [x] Custom roles support (via config file)
- [x] Audit logging ✅
  - [x] Audit logging module (`src/audit.rs`)
  - [x] Authentication events
  - [x] Authorization decisions
  - [x] Security violations
  - [x] JSON format logs
  - [x] Configurable audit log path

## Phase 4: Observability ✅ (Basic)

- [x] Metrics
  - [x] Prometheus exporter format
  - [x] Custom metrics (counters, gauges)
  - [x] Metric aggregation (basic)
- [x] Logging
  - [x] Structured logging (tracing)
  - [x] Log levels (trace/debug/info/warn/error)
  - [ ] Log rotation (config ready, not implemented)
- [ ] Tracing (Future work)
  - [ ] OpenTelemetry
  - [ ] Distributed tracing
  - [ ] Span annotations
- [x] Health checks
  - [x] Health check module
  - [ ] HTTP endpoint (needs HTTP server)
  - [ ] gRPC health service (needs gRPC integration)
  - [x] Readiness probe (logic implemented)
  - [x] Liveness probe (logic implemented)

## Phase 5: Operations 📋

- [ ] Hot reload
  - [ ] Reload configuration
  - [ ] Reload TLS certificates
  - [ ] No downtime
- [ ] Backup & restore
  - [ ] Snapshot creation
  - [ ] Snapshot upload (S3)
  - [ ] Restore from snapshot
- [ ] Administration
  - [ ] Admin API
  - [ ] Cluster management
  - [ ] Shard operations
- [ ] Upgrades
  - [ ] Rolling upgrades
  - [ ] Version compatibility
  - [ ] Migration tools

## Phase 6: Performance 📋

- [ ] Resource limits
  - [ ] Memory limits
  - [ ] CPU limits
  - [ ] Disk limits
- [ ] Rate limiting
  - [ ] Per-client limits
  - [ ] Global limits
  - [ ] Adaptive limiting
- [ ] Caching
  - [ ] Query cache
  - [ ] Result cache
  - [ ] Circuit cache
- [ ] Connection management
  - [ ] Connection pooling
  - [ ] Keep-alive
  - [ ] Timeout handling

## Phase 7: Security 📋

- [ ] TLS/mTLS
  - [ ] Certificate management
  - [ ] Certificate rotation
  - [ ] Certificate validation
- [ ] Encryption
  - [ ] Data at rest
  - [ ] Data in transit
  - [ ] Key management
- [ ] Audit logging
  - [ ] Operation logs
  - [ ] Access logs
  - [ ] Security events
- [ ] Hardening
  - [ ] Seccomp profiles
  - [ ] Capability dropping
  - [ ] Non-root execution

## Phase 8: Testing 📋

- [ ] Integration tests
  - [ ] End-to-end tests
  - [ ] Cluster tests
  - [ ] Failure scenarios
- [ ] Load tests
  - [ ] Throughput tests
  - [ ] Latency tests
  - [ ] Resource usage
- [ ] Chaos tests
  - [ ] Node failures
  - [ ] Network issues
  - [ ] Disk failures

## Dependencies

- All amaters-* crates
- `clap` - CLI parsing
- `tokio` - Async runtime
- `tracing` - Logging/tracing
- `serde` - Serialization
- `toml` - Configuration

## Configuration Files

- [ ] `config.toml` - Main configuration
- [ ] `logging.toml` - Logging configuration
- [ ] `cluster.toml` - Cluster configuration

## Documentation

- [x] README with usage examples
- [ ] Configuration reference
- [ ] Operations guide
- [ ] Deployment guide
- [ ] Troubleshooting guide

## Notes

- Server must handle SIGTERM gracefully
- All operations should be observable
- Security-first approach
- Plan for zero-downtime upgrades
