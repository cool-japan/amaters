# Contributing to AmateRS

Thank you for your interest in contributing to AmateRS! This document provides guidelines and best practices for contributing to the project.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Coding Standards](#coding-standards)
- [Testing Guidelines](#testing-guidelines)
- [Documentation](#documentation)
- [Pull Request Process](#pull-request-process)
- [Issue Guidelines](#issue-guidelines)

## Code of Conduct

Be respectful, professional, and constructive in all interactions.

## Getting Started

### Prerequisites

- Rust nightly (specified in `rust-toolchain.toml`)
- Git
- Basic understanding of cryptography and databases (helpful)

### Setup Development Environment

```bash
# Clone repository
git clone https://github.com/cool-japan/amaters
cd amaters

# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Rust toolchain will be automatically selected from rust-toolchain.toml

# Build project
cargo build --workspace

# Run tests
cargo test --workspace

# Run clippy
cargo clippy --workspace --all-features -- -D warnings
```

### Project Structure

```
amaters/
├── crates/              # All workspace crates
│   ├── amaters-core/    # Core kernel
│   ├── amaters-net/     # Network layer
│   ├── amaters-cluster/ # Consensus
│   ├── amaters-server/  # Server binary
│   ├── amaters-sdk-rust/# Rust SDK
│   └── amaters-cli/     # CLI tool
├── docs/                # Documentation
├── examples/            # Example code
└── README.md
```

## Development Workflow

### 1. Create a Branch

```bash
# Create feature branch
git checkout -b feature/your-feature-name

# Or bug fix branch
git checkout -b fix/issue-123
```

### 2. Make Changes

Follow the [Coding Standards](#coding-standards) below.

### 3. Test Your Changes

```bash
# Run tests
cargo test --workspace

# Run specific crate tests
cargo test -p amaters-core

# Run with features
cargo test --workspace --all-features
```

### 4. Run Lints

```bash
# Clippy
cargo clippy --workspace --all-features -- -D warnings

# Format code
cargo fmt --all

# Check formatting
cargo fmt --all -- --check
```

### 5. Commit Changes

```bash
# Write clear, descriptive commit messages
git commit -m "feat: add FHE circuit optimization

- Implement bootstrap minimization
- Add dead code elimination
- Improve performance by 30%

Closes #123"
```

**Commit Message Format:**
- `feat:` - New feature
- `fix:` - Bug fix
- `docs:` - Documentation changes
- `test:` - Test additions/changes
- `refactor:` - Code refactoring
- `perf:` - Performance improvements
- `chore:` - Maintenance tasks

### 6. Push and Create PR

```bash
git push origin feature/your-feature-name
```

Then create a Pull Request on GitHub.

## Coding Standards

### General Principles

1. **No Unwrap Policy** ⚠️ CRITICAL
   - NEVER use `.unwrap()` in production code
   - Use `.expect("descriptive message")` only in tests
   - Return `Result<T>` for fallible operations
   - Use `?` operator for error propagation

```rust
// ❌ BAD
let value = map.get(&key).unwrap();

// ✅ GOOD
let value = map.get(&key)
    .ok_or_else(|| AmateRSError::NotFound(error_context!("Key not found")))?;
```

2. **Refactoring Policy** 📏
   - Keep files under 2000 lines
   - Use `rslines 50` to find large files
   - Use `splitrs` tool for refactoring
   - Split by logical boundaries (modules, traits)

3. **Naming Conventions**
   - `snake_case` for functions, variables, modules
   - `PascalCase` for types, traits, enums
   - `SCREAMING_SNAKE_CASE` for constants
   - Clear, descriptive names (avoid abbreviations)

```rust
// ✅ GOOD
struct CipherBlob { ... }
fn encrypt_data() -> Result<CipherBlob> { ... }
const MAX_BUFFER_SIZE: usize = 1024;
```

4. **Workspace Policy**
   - All versions in root `Cargo.toml` `[workspace.dependencies]`
   - Use `version.workspace = true` in subcrates
   - No version control in subcrate `Cargo.toml`
   - Different keywords/categories per crate OK

5. **COOLJAPAN Policy** 🇯🇵
   - Use OxiBLAS instead of OpenBLAS
   - Use Oxicode instead of bincode
   - Pure Rust: No C/Fortran dependencies by default
   - Feature-gate non-Rust dependencies if absolutely necessary

6. **Error Handling**
   - Use `thiserror` for error types
   - Provide rich error context
   - Include location information
   - Support error chaining

```rust
#[derive(Error, Debug, Clone)]
pub enum MyError {
    #[error("{0}")]
    StorageError(ErrorContext),
}

// Usage
return Err(MyError::StorageError(
    error_context!("Failed to write data")
));
```

7. **Testing**
   - Write tests for all new features
   - Use `Result<()>` return type in tests
   - Use `tempfile::tempdir()` for file tests
   - Property-based tests with `proptest` for complex logic

```rust
#[test]
fn test_feature() -> Result<()> {
    let temp_dir = tempfile::tempdir()?;
    // test code
    Ok(())
}
```

8. **Documentation**
   - Public items must have doc comments
   - Include examples in doc comments
   - Document panics, errors, safety
   - Keep docs up to date

```rust
/// Encrypts data using FHE
///
/// # Arguments
/// * `plaintext` - Data to encrypt
/// * `public_key` - Public encryption key
///
/// # Returns
/// Encrypted ciphertext blob
///
/// # Errors
/// Returns `CryptoError` if encryption fails
///
/// # Examples
/// ```
/// let cipher = encrypt(&data, &pk)?;
/// ```
pub fn encrypt(plaintext: &[u8], public_key: &PublicKey) -> Result<CipherBlob> {
    // implementation
}
```

### Rust-Specific Guidelines

- Use `async/await` for I/O operations
- Prefer `Arc<T>` for shared ownership
- Use `Cow<T>` for potential clones
- Leverage zero-cost abstractions
- Profile before optimizing
- Use `#[inline]` judiciously
- Avoid premature optimization

### Performance Considerations

- Minimize allocations in hot paths
- Use buffer pools for repeated allocations
- Profile with `cargo flamegraph`
- Benchmark with `criterion`
- Consider SIMD for data-parallel operations

## Testing Guidelines

### Unit Tests

Located in `#[cfg(test)]` modules within source files.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_something() -> Result<()> {
        // arrange
        let input = create_input();

        // act
        let output = process(input)?;

        // assert
        assert_eq!(output, expected);
        Ok(())
    }
}
```

### Integration Tests

Located in `tests/` directories of each crate.

```rust
// crates/amaters-core/tests/storage_tests.rs
use amaters_core::{storage::*, traits::*};

#[tokio::test]
async fn test_end_to_end_storage() -> anyhow::Result<()> {
    // test implementation
    Ok(())
}
```

### Property-Based Tests

Use `proptest` for testing properties:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_encrypt_decrypt_roundtrip(data: Vec<u8>) {
        let (pk, sk) = generate_keys();
        let cipher = encrypt(&data, &pk)?;
        let decrypted = decrypt(&cipher, &sk)?;
        prop_assert_eq!(data, decrypted);
    }
}
```

### Benchmarks

Use `criterion` for benchmarks:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_encrypt(c: &mut Criterion) {
    c.bench_function("encrypt 1KB", |b| {
        let data = vec![0u8; 1024];
        let pk = generate_public_key();
        b.iter(|| encrypt(black_box(&data), black_box(&pk)));
    });
}

criterion_group!(benches, benchmark_encrypt);
criterion_main!(benches);
```

## Documentation

### Types of Documentation

1. **Code Documentation**
   - All public items must have doc comments
   - Include examples where appropriate
   - Explain why, not just what

2. **README Files**
   - Each crate has its own README
   - Include usage examples
   - Link to relevant docs

3. **Architecture Decision Records (ADRs)**
   - Document major decisions in `docs/adr/`
   - Follow ADR template
   - Include context, decision, consequences

4. **API Documentation**
   - Generated from doc comments
   - View with `cargo doc --open`

### Writing Good Documentation

```rust
/// Brief one-line summary
///
/// More detailed explanation of what this does,
/// including any important caveats or considerations.
///
/// # Arguments
/// * `arg1` - Description of arg1
/// * `arg2` - Description of arg2
///
/// # Returns
/// Description of return value
///
/// # Errors
/// When this function returns errors
///
/// # Panics
/// When this function panics (if ever)
///
/// # Safety
/// Safety requirements (for unsafe functions)
///
/// # Examples
/// ```
/// use my_crate::my_function;
/// let result = my_function(42)?;
/// assert_eq!(result, 84);
/// ```
pub fn my_function(arg1: i32, arg2: i32) -> Result<i32> {
    // implementation
}
```

## Pull Request Process

### Before Submitting PR

- [ ] Code compiles without warnings
- [ ] All tests pass
- [ ] Clippy passes with no warnings
- [ ] Code is formatted (`cargo fmt`)
- [ ] Documentation is updated
- [ ] CHANGELOG.md is updated (if applicable)
- [ ] Related issue is referenced

### PR Description Template

```markdown
## Description
Brief description of changes

## Type of Change
- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change
- [ ] Documentation update

## Related Issues
Fixes #123
Relates to #456

## Testing
- [ ] Unit tests added/updated
- [ ] Integration tests added/updated
- [ ] Manual testing performed

## Checklist
- [ ] Code compiles
- [ ] Tests pass
- [ ] Clippy clean
- [ ] Formatted
- [ ] Documented

## Screenshots (if applicable)
```

### Review Process

1. Maintainers will review your PR
2. Address feedback and update PR
3. Once approved, PR will be merged
4. Thank you for contributing!

## Issue Guidelines

### Reporting Bugs

Use the bug report template:

```markdown
**Describe the bug**
Clear description of the bug

**To Reproduce**
Steps to reproduce:
1. Do X
2. Do Y
3. See error

**Expected behavior**
What you expected to happen

**Environment**
- OS: [e.g., Ubuntu 22.04]
- Rust version: [e.g., 1.85.0]
- AmateRS version: [e.g., 0.1.0]

**Additional context**
Any other relevant information
```

### Feature Requests

Use the feature request template:

```markdown
**Is your feature request related to a problem?**
Clear description of the problem

**Describe the solution you'd like**
What you want to happen

**Describe alternatives you've considered**
Other solutions considered

**Additional context**
Any other relevant information
```

## Getting Help

- **Documentation**: Check the README and docs/
- **Issues**: Search existing issues
- **Discussions**: Start a discussion on GitHub
- **Contact**: contact@cooljapan.tech

## Recognition

Contributors will be:
- Listed in CHANGELOG.md
- Credited in release notes
- Acknowledged in documentation

Thank you for contributing to AmateRS! 🎉

---

**Last Updated**: 2026-01-18
**Maintained by**: COOLJAPAN OU (Team KitaSan)
