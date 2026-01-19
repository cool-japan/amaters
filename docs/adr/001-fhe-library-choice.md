# ADR 001: Choice of TFHE-rs for FHE Implementation

## Status
Accepted

## Context
AmateRS requires a production-grade Fully Homomorphic Encryption (FHE) library with:
- Post-quantum security guarantees
- GPU acceleration support
- Active development and maintenance
- Rust-native implementation (no FFI overhead)
- Reasonable performance for database operations

## Decision
Use Zama's **tfhe-rs** (v0.9+) as the primary FHE library.

## Alternatives Considered

1. **Microsoft SEAL** (C++)
   - Pros: Mature, well-documented
   - Cons: C++ FFI overhead, BFV/CKKS schemes less suitable for comparison operations

2. **TFHE-C++** (original)
   - Pros: Battle-tested
   - Cons: C++ FFI, less active development

3. **Concrete** (Python/Rust)
   - Pros: High-level API
   - Cons: More abstraction overhead

## Consequences

### Positive
- **LWE-based security**: Quantum-resistant by design
- **GPU support**: CUDA and Metal backends available
- **Pure Rust**: No FFI boundary, better safety guarantees
- **Boolean and Integer**: Supports both circuit types
- **Active development**: Zama is actively maintaining and improving

### Negative
- **Performance**: FHE is inherently slow (ms to seconds per operation)
- **Memory**: Large ciphertext sizes (KB to MB)
- **Learning curve**: FHE concepts are complex

## Mitigation Strategies
1. **Circuit optimization**: Minimize bootstrapping operations
2. **GPU acceleration**: Use CUDA/Metal for parallel operations
3. **Hybrid approach**: Keep metadata unencrypted, encrypt only sensitive fields
4. **Caching**: Cache frequently accessed ciphertexts

## References
- [tfhe-rs GitHub](https://github.com/zama-ai/tfhe-rs)
- [Zama Documentation](https://docs.zama.ai/tfhe-rs)
- [TFHE Paper](https://eprint.iacr.org/2018/421.pdf)
