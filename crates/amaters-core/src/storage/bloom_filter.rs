//! Bloom filter implementation for fast key lookups
//!
//! A Bloom filter is a space-efficient probabilistic data structure
//! that tests whether an element is a member of a set.
//!
//! Properties:
//! - False positives possible (may say "yes" when element not present)
//! - False negatives impossible (never says "no" when element present)
//! - Space efficient: ~10 bits per element for 1% false positive rate
//!
//! Use case: Check if SSTable contains a key before reading from disk

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::Key;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Bloom filter configuration
#[derive(Debug, Clone)]
pub struct BloomFilterConfig {
    /// Expected number of elements
    pub expected_elements: usize,
    /// Target false positive rate (e.g., 0.01 for 1%)
    pub false_positive_rate: f64,
}

impl Default for BloomFilterConfig {
    fn default() -> Self {
        Self {
            expected_elements: 10000,
            false_positive_rate: 0.01, // 1%
        }
    }
}

/// Bloom filter for probabilistic set membership testing
#[derive(Debug, Clone)]
pub struct BloomFilter {
    /// Bit array
    bits: Vec<u8>,
    /// Number of bits in the filter
    num_bits: usize,
    /// Number of hash functions
    num_hash_functions: usize,
    /// Number of elements inserted
    num_elements: usize,
}

impl BloomFilter {
    /// Create a new bloom filter with optimal parameters
    pub fn new(config: BloomFilterConfig) -> Self {
        // Calculate optimal number of bits
        // m = -n * ln(p) / (ln(2)^2)
        let num_bits = Self::optimal_num_bits(config.expected_elements, config.false_positive_rate);

        // Calculate optimal number of hash functions
        // k = (m / n) * ln(2)
        let num_hash_functions =
            Self::optimal_num_hash_functions(num_bits, config.expected_elements);

        let num_bytes = (num_bits + 7) / 8; // Round up to nearest byte
        let bits = vec![0u8; num_bytes];

        Self {
            bits,
            num_bits,
            num_hash_functions,
            num_elements: 0,
        }
    }

    /// Create bloom filter from raw data
    pub fn from_bytes(
        data: Vec<u8>,
        num_bits: usize,
        num_hash_functions: usize,
        num_elements: usize,
    ) -> Result<Self> {
        let expected_bytes = (num_bits + 7) / 8;
        if data.len() != expected_bytes {
            return Err(AmateRSError::ValidationError(ErrorContext::new(format!(
                "Invalid bloom filter size: expected {} bytes, got {}",
                expected_bytes,
                data.len()
            ))));
        }

        Ok(Self {
            bits: data,
            num_bits,
            num_hash_functions,
            num_elements,
        })
    }

    /// Calculate optimal number of bits for given parameters
    fn optimal_num_bits(expected_elements: usize, false_positive_rate: f64) -> usize {
        if expected_elements == 0 {
            return 128; // Minimum size
        }

        let ln2_squared = std::f64::consts::LN_2 * std::f64::consts::LN_2;
        let num_bits = -(expected_elements as f64) * false_positive_rate.ln() / ln2_squared;

        // Round up to nearest multiple of 8 (byte boundary)
        let num_bits = num_bits.ceil() as usize;
        ((num_bits + 7) / 8) * 8
    }

    /// Calculate optimal number of hash functions
    fn optimal_num_hash_functions(num_bits: usize, expected_elements: usize) -> usize {
        if expected_elements == 0 {
            return 1;
        }

        let k = (num_bits as f64 / expected_elements as f64) * std::f64::consts::LN_2;
        let k = k.ceil() as usize;

        // Limit to reasonable range
        k.clamp(1, 10)
    }

    /// Insert a key into the bloom filter
    pub fn insert(&mut self, key: &Key) {
        let (hash1, hash2) = self.hash_key(key);

        for i in 0..self.num_hash_functions {
            // Double hashing: h(k, i) = h1(k) + i * h2(k) mod m
            let bit_index = self.get_bit_index(hash1, hash2, i);
            self.set_bit(bit_index);
        }

        self.num_elements += 1;
    }

    /// Check if a key may be in the set
    ///
    /// Returns:
    /// - true: key MAY be in the set (possible false positive)
    /// - false: key is DEFINITELY NOT in the set (no false negatives)
    pub fn may_contain(&self, key: &Key) -> bool {
        let (hash1, hash2) = self.hash_key(key);

        for i in 0..self.num_hash_functions {
            let bit_index = self.get_bit_index(hash1, hash2, i);
            if !self.get_bit(bit_index) {
                return false; // Definitely not present
            }
        }

        true // Possibly present
    }

    /// Hash a key using two different hash functions
    fn hash_key(&self, key: &Key) -> (u64, u64) {
        // Hash 1: Default hasher
        let mut hasher1 = DefaultHasher::new();
        key.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        // Hash 2: Use a different seed
        let mut hasher2 = DefaultHasher::new();
        0xDEADBEEFu64.hash(&mut hasher2); // Seed
        key.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        (hash1, hash2)
    }

    /// Get bit index using double hashing
    fn get_bit_index(&self, hash1: u64, hash2: u64, i: usize) -> usize {
        // h(k, i) = (h1(k) + i * h2(k)) mod m
        let hash = hash1.wrapping_add((i as u64).wrapping_mul(hash2));
        (hash % (self.num_bits as u64)) as usize
    }

    /// Set a bit in the filter
    fn set_bit(&mut self, bit_index: usize) {
        let byte_index = bit_index / 8;
        let bit_offset = bit_index % 8;
        self.bits[byte_index] |= 1 << bit_offset;
    }

    /// Get a bit from the filter
    fn get_bit(&self, bit_index: usize) -> bool {
        let byte_index = bit_index / 8;
        let bit_offset = bit_index % 8;
        (self.bits[byte_index] & (1 << bit_offset)) != 0
    }

    /// Get the raw bytes of the bloom filter
    pub fn as_bytes(&self) -> &[u8] {
        &self.bits
    }

    /// Get bloom filter metadata for serialization
    pub fn metadata(&self) -> BloomFilterMetadata {
        BloomFilterMetadata {
            num_bits: self.num_bits,
            num_hash_functions: self.num_hash_functions,
            num_elements: self.num_elements,
        }
    }

    /// Get the number of elements inserted
    pub fn num_elements(&self) -> usize {
        self.num_elements
    }

    /// Get the current false positive rate estimate
    pub fn estimated_false_positive_rate(&self) -> f64 {
        if self.num_elements == 0 {
            return 0.0;
        }

        // p = (1 - e^(-kn/m))^k
        // where k = num_hash_functions, n = num_elements, m = num_bits
        let k = self.num_hash_functions as f64;
        let n = self.num_elements as f64;
        let m = self.num_bits as f64;

        let exponent = -k * n / m;
        let base = 1.0 - exponent.exp();
        base.powf(k)
    }

    /// Get the size in bytes
    pub fn size_bytes(&self) -> usize {
        self.bits.len()
    }
}

/// Bloom filter metadata for serialization
#[derive(Debug, Clone, Copy)]
pub struct BloomFilterMetadata {
    pub num_bits: usize,
    pub num_hash_functions: usize,
    pub num_elements: usize,
}

impl BloomFilterMetadata {
    /// Serialize metadata to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(24);
        bytes.extend_from_slice(&(self.num_bits as u64).to_le_bytes());
        bytes.extend_from_slice(&(self.num_hash_functions as u64).to_le_bytes());
        bytes.extend_from_slice(&(self.num_elements as u64).to_le_bytes());
        bytes
    }

    /// Deserialize metadata from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 24 {
            return Err(AmateRSError::ValidationError(ErrorContext::new(format!(
                "Invalid bloom filter metadata size: {}",
                bytes.len()
            ))));
        }

        let num_bits = u64::from_le_bytes(bytes[0..8].try_into().map_err(|_| {
            AmateRSError::ValidationError(ErrorContext::new("Failed to parse num_bits".to_string()))
        })?) as usize;

        let num_hash_functions = u64::from_le_bytes(bytes[8..16].try_into().map_err(|_| {
            AmateRSError::ValidationError(ErrorContext::new(
                "Failed to parse num_hash_functions".to_string(),
            ))
        })?) as usize;

        let num_elements = u64::from_le_bytes(bytes[16..24].try_into().map_err(|_| {
            AmateRSError::ValidationError(ErrorContext::new(
                "Failed to parse num_elements".to_string(),
            ))
        })?) as usize;

        Ok(Self {
            num_bits,
            num_hash_functions,
            num_elements,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bloom_filter_basic() {
        let config = BloomFilterConfig {
            expected_elements: 100,
            false_positive_rate: 0.01,
        };
        let mut filter = BloomFilter::new(config);

        // Insert keys
        let key1 = Key::from_str("test_key_1");
        let key2 = Key::from_str("test_key_2");
        let key3 = Key::from_str("test_key_3");

        filter.insert(&key1);
        filter.insert(&key2);

        // Check contains
        assert!(filter.may_contain(&key1));
        assert!(filter.may_contain(&key2));
        assert!(!filter.may_contain(&key3)); // Should be false (not inserted)
    }

    #[test]
    fn test_bloom_filter_false_positive_rate() {
        let config = BloomFilterConfig {
            expected_elements: 1000,
            false_positive_rate: 0.01,
        };
        let mut filter = BloomFilter::new(config);

        // Insert 1000 keys
        for i in 0..1000 {
            let key = Key::from_str(&format!("key_{:04}", i));
            filter.insert(&key);
        }

        // Test 1000 keys that were NOT inserted
        let mut false_positives = 0;
        for i in 1000..2000 {
            let key = Key::from_str(&format!("key_{:04}", i));
            if filter.may_contain(&key) {
                false_positives += 1;
            }
        }

        // False positive rate should be close to 1%
        let actual_rate = false_positives as f64 / 1000.0;
        assert!(actual_rate < 0.05); // Less than 5% (generous for test)
    }

    #[test]
    fn test_bloom_filter_serialization() {
        let mut filter = BloomFilter::new(BloomFilterConfig::default());

        // Insert some keys
        for i in 0..10 {
            let key = Key::from_str(&format!("key_{}", i));
            filter.insert(&key);
        }

        // Get bytes
        let bytes = filter.as_bytes().to_vec();
        let metadata = filter.metadata();

        // Reconstruct
        let filter2 = BloomFilter::from_bytes(
            bytes,
            metadata.num_bits,
            metadata.num_hash_functions,
            metadata.num_elements,
        )
        .expect("Failed to reconstruct BloomFilter from bytes");

        // Verify same behavior
        for i in 0..10 {
            let key = Key::from_str(&format!("key_{}", i));
            assert!(filter2.may_contain(&key));
        }
    }

    #[test]
    fn test_bloom_filter_metadata() {
        let metadata = BloomFilterMetadata {
            num_bits: 1024,
            num_hash_functions: 7,
            num_elements: 100,
        };

        let bytes = metadata.to_bytes();
        let metadata2 = BloomFilterMetadata::from_bytes(&bytes)
            .expect("Failed to deserialize BloomFilterMetadata from bytes");

        assert_eq!(metadata.num_bits, metadata2.num_bits);
        assert_eq!(metadata.num_hash_functions, metadata2.num_hash_functions);
        assert_eq!(metadata.num_elements, metadata2.num_elements);
    }

    #[test]
    fn test_bloom_filter_optimal_parameters() {
        let config = BloomFilterConfig {
            expected_elements: 10000,
            false_positive_rate: 0.01,
        };
        let filter = BloomFilter::new(config);

        // Verify reasonable parameters
        assert!(filter.num_bits > 0);
        assert!(filter.num_hash_functions > 0);
        assert!(filter.num_hash_functions <= 10);
    }

    #[test]
    fn test_bloom_filter_empty() {
        let filter = BloomFilter::new(BloomFilterConfig::default());
        let key = Key::from_str("test");

        // Empty filter should return false for any key
        assert!(!filter.may_contain(&key));
    }

    #[test]
    fn test_bloom_filter_no_false_negatives() {
        let mut filter = BloomFilter::new(BloomFilterConfig::default());

        // Insert many keys
        let keys: Vec<Key> = (0..1000)
            .map(|i| Key::from_str(&format!("key_{}", i)))
            .collect();

        for key in &keys {
            filter.insert(key);
        }

        // All inserted keys must be found (no false negatives)
        for key in &keys {
            assert!(filter.may_contain(key));
        }
    }

    #[test]
    fn test_bloom_filter_estimated_fpr() {
        let mut filter = BloomFilter::new(BloomFilterConfig {
            expected_elements: 100,
            false_positive_rate: 0.01,
        });

        // Initially 0
        assert_eq!(filter.estimated_false_positive_rate(), 0.0);

        // Insert elements
        for i in 0..100 {
            let key = Key::from_str(&format!("key_{}", i));
            filter.insert(&key);
        }

        // Should have non-zero FPR
        let fpr = filter.estimated_false_positive_rate();
        assert!(fpr > 0.0);
        assert!(fpr < 0.1); // Less than 10%
    }
}
