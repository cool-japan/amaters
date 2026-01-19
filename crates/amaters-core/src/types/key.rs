//! Database key type for AmateRS
//!
//! Keys are immutable, ordered byte sequences used to identify data.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Database key type
///
/// Keys are Arc-wrapped byte slices for cheap cloning and sharing.
/// They implement Ord for use in sorted data structures (LSM-Tree).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key(Arc<[u8]>);

impl Key {
    /// Maximum key size (64KB)
    pub const MAX_SIZE: usize = 64 * 1024;

    /// Create a new key from bytes
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self(bytes.into())
    }

    /// Create from a byte slice
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self(bytes.into())
    }

    /// Create from a string
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        Self(s.as_bytes().into())
    }

    /// Get the key as a byte slice
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Get the length of the key
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Check if the key is empty
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Convert to a string if valid UTF-8
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).to_string()
    }

    /// Get the Arc-wrapped bytes for zero-copy sharing
    pub fn as_arc(&self) -> &Arc<[u8]> {
        &self.0
    }

    /// Clone into a Vec
    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }
}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Try to display as UTF-8, fall back to hex
        if let Ok(s) = std::str::from_utf8(&self.0) {
            write!(f, "Key(\"{}\")", s)
        } else {
            write!(f, "Key(0x{})", hex::encode(&self.0))
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_lossy())
    }
}

impl From<Vec<u8>> for Key {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

impl From<&[u8]> for Key {
    fn from(bytes: &[u8]) -> Self {
        Self::from_slice(bytes)
    }
}

impl From<&str> for Key {
    fn from(s: &str) -> Self {
        Self::from_str(s)
    }
}

impl From<String> for Key {
    fn from(s: String) -> Self {
        Self::from_str(&s)
    }
}

impl AsRef<[u8]> for Key {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

// Helper to avoid depending on hex crate (simple implementation)
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        use std::fmt::Write;
        bytes.iter().fold(String::new(), |mut s, b| {
            let _ = write!(&mut s, "{:02x}", b);
            s
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_creation() {
        let key1 = Key::from_str("test_key");
        let key2 = Key::from_slice(b"test_key");
        let key3 = Key::new(b"test_key".to_vec());

        assert_eq!(key1, key2);
        assert_eq!(key2, key3);
        assert_eq!(key1.len(), 8);
    }

    #[test]
    fn test_key_ordering() {
        let key1 = Key::from_str("aaa");
        let key2 = Key::from_str("bbb");
        let key3 = Key::from_str("ccc");

        assert!(key1 < key2);
        assert!(key2 < key3);
        assert!(key1 < key3);
    }

    #[test]
    fn test_key_clone() {
        let key1 = Key::from_str("test");
        let key2 = key1.clone();

        assert_eq!(key1, key2);
        assert!(Arc::ptr_eq(key1.as_arc(), key2.as_arc())); // Same Arc
    }

    #[test]
    fn test_key_hash() {
        use std::collections::HashMap;

        let mut map = HashMap::new();
        let key = Key::from_str("test");
        map.insert(key.clone(), 42);

        assert_eq!(map.get(&key), Some(&42));
    }

    #[test]
    fn test_key_display() {
        let key = Key::from_str("hello");
        assert_eq!(format!("{}", key), "hello");
    }
}
