# amaters-net TODO

## Phase 1: Protocol Design ✅

- [x] Define gRPC service schema
  - [x] Create .proto files for AQL
  - [x] Define request/response messages
  - [x] Define streaming operations
  - [x] Add versioning support
- [x] Design error handling
  - [x] Network errors
  - [x] Timeout handling
  - [x] Retry strategies
- [x] Connection state management
  - [x] Connection lifecycle
  - [x] Reconnection logic
  - [x] Graceful shutdown

## Phase 2: gRPC Implementation 🚧

### Server Side
- [x] Implement gRPC server (stub implementation)
  - [x] Service trait implementation (stubbed, awaiting full service generation)
  - [x] Request routing (designed)
  - [ ] Middleware support (auth, logging)
  - [x] Error conversion
- [x] Request handling (designed)
  - [x] Parse AQL queries (conversion functions implemented)
  - [x] Execute via amaters-core (designed)
  - [x] Return responses (designed)
- [x] Streaming support (designed)
  - [x] Bidirectional streaming (protocol defined)
  - [x] Backpressure handling (protocol defined)
  - [x] Stream cancellation (protocol defined)

### Client Side
- [x] Implement gRPC client (stub implementation)
  - [x] Connection management (designed)
  - [x] Request building (conversion functions implemented)
  - [x] Response handling (designed)
- [x] Client configuration
  - [x] Timeout settings
  - [x] Retry policies (error categorization for retries)
  - [ ] Connection pooling

**Note**: Server and client have stub implementations. Full gRPC service integration requires proper tonic-build service generation configuration, which will be completed in the next iteration.

## Phase 3: QUIC Transport 📋

- [ ] Integrate quinn (QUIC library)
  - [ ] Replace HTTP/2 with HTTP/3
  - [ ] Configure QUIC parameters
  - [ ] Handle connection migration
- [ ] 0-RTT optimization
  - [ ] Session resumption
  - [ ] Early data support
- [ ] Multiplexing
  - [ ] Concurrent streams
  - [ ] Stream prioritization
  - [ ] Flow control

## Phase 4: Security (mTLS) 📋

### Certificate Management
- [ ] Certificate generation
  - [ ] Self-signed for development
  - [ ] CA integration for production
- [ ] Certificate validation
  - [ ] Client certificate verification
  - [ ] Server certificate verification
  - [ ] Chain validation
- [ ] Certificate rotation
  - [ ] Reload certificates
  - [ ] Graceful transition

### Authentication
- [ ] Client authentication
  - [ ] mTLS verification
  - [ ] Token-based auth (optional)
  - [ ] API keys (optional)
- [ ] Authorization
  - [ ] Role-based access control
  - [ ] Query-level permissions

## Phase 5: Connection Pooling 📋

- [ ] Connection pool implementation
  - [ ] Pool configuration (min/max size)
  - [ ] Connection health checks
  - [ ] Idle connection timeout
  - [ ] Connection reuse
- [ ] Load balancing
  - [ ] Round-robin
  - [ ] Least connections
  - [ ] Weighted balancing
- [ ] Circuit breaker
  - [ ] Failure detection
  - [ ] Automatic recovery
  - [ ] Fallback strategies

## Phase 6: Observability 📋

### Metrics
- [ ] Connection metrics
  - [ ] Active connections
  - [ ] Connection errors
  - [ ] Connection duration
- [ ] Request metrics
  - [ ] Request rate
  - [ ] Request latency
  - [ ] Request errors
- [ ] Network metrics
  - [ ] Bytes sent/received
  - [ ] Packet loss
  - [ ] RTT (round-trip time)

### Logging
- [ ] Structured logging
  - [ ] Request/response logging
  - [ ] Error logging
  - [ ] Debug logging
- [ ] Log levels
  - [ ] Configurable verbosity
  - [ ] Performance impact

### Tracing
- [ ] Distributed tracing
  - [ ] OpenTelemetry integration
  - [ ] Trace context propagation
  - [ ] Span creation

## Phase 7: Performance Optimization 📋

- [ ] Zero-copy operations
  - [ ] Minimize data copying
  - [ ] Use shared buffers
- [ ] Batching
  - [ ] Batch multiple requests
  - [ ] Reduce network round-trips
- [ ] Compression
  - [ ] gRPC compression (gzip, deflate)
  - [ ] Application-level compression
- [ ] Benchmarking
  - [ ] Throughput benchmarks
  - [ ] Latency benchmarks
  - [ ] Resource usage profiling

## Phase 8: Testing 🚧

### Unit Tests
- [x] Protocol serialization tests (conversion tests)
- [x] Connection handling tests (basic client/server creation)
- [x] Error handling tests (error code mapping and categorization)
- [ ] Pool management tests

### Integration Tests
- [ ] Client-server communication
- [ ] mTLS authentication
- [ ] Stream handling
- [ ] Error scenarios

### Load Tests
- [ ] High connection count
- [ ] High request rate
- [ ] Long-running connections
- [ ] Resource limits

### Chaos Tests
- [ ] Network partitions
- [ ] Server failures
- [ ] Certificate expiry
- [ ] Connection drops

## Dependencies

- [x] `tonic` (gRPC)
- [x] `tonic-build` (codegen)
- [x] `prost` (protobuf)
- [ ] `quinn` (QUIC)
- [ ] `rustls` (TLS)
- [ ] `tokio-rustls` (async TLS)
- [ ] `tower` (middleware)

## Protocol Files

- [x] `protocol/aql.proto` - Query protocol
- [x] `protocol/types.proto` - Common types
- [x] `protocol/errors.proto` - Error messages
- [x] `protocol/query.proto` - Query operations

## Configuration

- [ ] TOML-based configuration
- [ ] Environment variables
- [ ] CLI arguments
- [ ] Hot reload support

## Documentation

- [ ] API documentation
- [ ] Protocol specification
- [ ] Security guide
- [ ] Performance tuning guide
- [ ] Examples

## Notes

- QUIC is UDP-based, ensure firewall rules allow it
- mTLS requires proper certificate infrastructure
- Connection pooling critical for performance
- Monitor network metrics for bottlenecks
