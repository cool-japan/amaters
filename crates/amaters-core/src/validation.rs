//! Input validation helpers
//!
//! Following SciRS2 patterns for parameter checking.

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::{CipherBlob, Key};

/// Check that a value is positive
pub fn check_positive(value: usize, name: &str) -> Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(AmateRSError::ValidationError(ErrorContext::new(format!(
            "{} must be positive, got {}",
            name, value
        ))))
    }
}

/// Check that ciphertext size is within limits
pub fn check_ciphertext_size(blob: &CipherBlob) -> Result<()> {
    if blob.len() <= CipherBlob::MAX_SIZE {
        Ok(())
    } else {
        Err(AmateRSError::ValidationError(ErrorContext::new(format!(
            "Ciphertext size {} exceeds maximum {}",
            blob.len(),
            CipherBlob::MAX_SIZE
        ))))
    }
}

/// Check that key size is within limits
pub fn check_key_size(key: &Key) -> Result<()> {
    if key.len() <= Key::MAX_SIZE {
        Ok(())
    } else {
        Err(AmateRSError::ValidationError(ErrorContext::new(format!(
            "Key size {} exceeds maximum {}",
            key.len(),
            Key::MAX_SIZE
        ))))
    }
}

/// Check that a string is not empty
pub fn check_not_empty(s: &str, name: &str) -> Result<()> {
    if !s.is_empty() {
        Ok(())
    } else {
        Err(AmateRSError::ValidationError(ErrorContext::new(format!(
            "{} cannot be empty",
            name
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_positive() {
        assert!(check_positive(1, "test").is_ok());
        assert!(check_positive(0, "test").is_err());
    }

    #[test]
    fn test_check_ciphertext_size() {
        let small = CipherBlob::new(vec![1, 2, 3]);
        assert!(check_ciphertext_size(&small).is_ok());
    }

    #[test]
    fn test_check_key_size() {
        let key = Key::from_str("test");
        assert!(check_key_size(&key).is_ok());
    }

    #[test]
    fn test_check_not_empty() {
        assert!(check_not_empty("test", "name").is_ok());
        assert!(check_not_empty("", "name").is_err());
    }
}
