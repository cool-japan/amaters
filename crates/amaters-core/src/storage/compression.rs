//! Block compression for SSTable storage
//!
//! Provides pluggable compression algorithms for SSTable data blocks.
//! All implementations are Pure Rust (COOLJAPAN policy) using OxiARC crates.
//!
//! # Supported Algorithms
//!
//! - **None**: No compression (passthrough)
//! - **Lz4**: Fast compression via oxiarc-lz4 (best for read-heavy workloads)
//! - **Deflate**: Higher ratio compression via oxiarc-deflate (best for storage-constrained)

use crate::error::{AmateRSError, ErrorContext, Result};

/// Compression algorithm type for SSTable blocks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum CompressionType {
    /// No compression
    #[default]
    None = 0,
    /// LZ4 compression (fast, moderate ratio)
    Lz4 = 1,
    /// DEFLATE compression (slower, better ratio)
    Deflate = 2,
}

impl CompressionType {
    /// Convert from a raw byte value
    pub fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            0 => Ok(Self::None),
            1 => Ok(Self::Lz4),
            2 => Ok(Self::Deflate),
            other => Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
                "Unknown compression type byte: {}",
                other
            )))),
        }
    }

    /// Convert to a raw byte value
    pub fn to_byte(self) -> u8 {
        self as u8
    }
}

/// Maximum decompressed block size (16 MB safety limit)
const MAX_DECOMPRESSED_SIZE: usize = 16 * 1024 * 1024;

/// Default DEFLATE compression level (6 = good balance of speed/ratio)
const DEFLATE_LEVEL: u8 = 6;

/// Compress a data block using the specified algorithm.
///
/// Returns the compressed bytes. For `CompressionType::None`, returns a copy of the input.
///
/// # Errors
///
/// Returns an error if the compression algorithm fails internally.
pub fn compress_block(data: &[u8], compression: CompressionType) -> Result<Vec<u8>> {
    match compression {
        CompressionType::None => Ok(data.to_vec()),
        CompressionType::Lz4 => compress_lz4(data),
        CompressionType::Deflate => compress_deflate(data),
    }
}

/// Decompress a data block using the specified algorithm.
///
/// `original_size` is the expected size of the decompressed output, used as
/// a hint and safety limit for the decompressor.
///
/// # Errors
///
/// Returns an error if:
/// - The compressed data is corrupted
/// - The decompressed size exceeds safety limits
/// - The decompressed size doesn't match `original_size`
pub fn decompress_block(
    data: &[u8],
    compression: CompressionType,
    original_size: usize,
) -> Result<Vec<u8>> {
    if original_size > MAX_DECOMPRESSED_SIZE {
        return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
            "Decompressed size {} exceeds maximum allowed size {}",
            original_size, MAX_DECOMPRESSED_SIZE
        ))));
    }

    match compression {
        CompressionType::None => Ok(data.to_vec()),
        CompressionType::Lz4 => decompress_lz4(data, original_size),
        CompressionType::Deflate => decompress_deflate(data, original_size),
    }
}

/// Compress using LZ4 frame format
fn compress_lz4(data: &[u8]) -> Result<Vec<u8>> {
    oxiarc_lz4::compress(data).map_err(|e| {
        AmateRSError::StorageIntegrity(ErrorContext::new(format!("LZ4 compression failed: {}", e)))
    })
}

/// Decompress LZ4 frame format
fn decompress_lz4(data: &[u8], original_size: usize) -> Result<Vec<u8>> {
    // Use 2x original_size as max_output for safety margin
    let max_output = original_size.saturating_mul(2).min(MAX_DECOMPRESSED_SIZE);
    let decompressed = oxiarc_lz4::decompress(data, max_output).map_err(|e| {
        AmateRSError::StorageIntegrity(ErrorContext::new(format!(
            "LZ4 decompression failed: {}",
            e
        )))
    })?;

    if decompressed.len() != original_size {
        return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
            "LZ4 decompressed size mismatch: expected {}, got {}",
            original_size,
            decompressed.len()
        ))));
    }

    Ok(decompressed)
}

/// Compress using DEFLATE algorithm
fn compress_deflate(data: &[u8]) -> Result<Vec<u8>> {
    oxiarc_deflate::deflate::deflate(data, DEFLATE_LEVEL).map_err(|e| {
        AmateRSError::StorageIntegrity(ErrorContext::new(format!(
            "DEFLATE compression failed: {}",
            e
        )))
    })
}

/// Decompress DEFLATE data
fn decompress_deflate(data: &[u8], original_size: usize) -> Result<Vec<u8>> {
    let decompressed = oxiarc_deflate::inflate::inflate(data).map_err(|e| {
        AmateRSError::StorageIntegrity(ErrorContext::new(format!(
            "DEFLATE decompression failed: {}",
            e
        )))
    })?;

    if decompressed.len() != original_size {
        return Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
            "DEFLATE decompressed size mismatch: expected {}, got {}",
            original_size,
            decompressed.len()
        ))));
    }

    Ok(decompressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_type_roundtrip() -> Result<()> {
        for ct in [
            CompressionType::None,
            CompressionType::Lz4,
            CompressionType::Deflate,
        ] {
            let byte = ct.to_byte();
            let recovered = CompressionType::from_byte(byte)?;
            assert_eq!(ct, recovered);
        }
        Ok(())
    }

    #[test]
    fn test_compression_type_invalid_byte() {
        let result = CompressionType::from_byte(255);
        assert!(result.is_err());
    }

    #[test]
    fn test_compress_decompress_none() -> Result<()> {
        let data = b"hello world test data for none compression";
        let compressed = compress_block(data, CompressionType::None)?;
        assert_eq!(&compressed, data);

        let decompressed = decompress_block(&compressed, CompressionType::None, data.len())?;
        assert_eq!(&decompressed, data);
        Ok(())
    }

    #[test]
    fn test_compress_decompress_lz4() -> Result<()> {
        let data = b"hello world hello world hello world hello world hello world";
        let compressed = compress_block(data, CompressionType::Lz4)?;
        let decompressed = decompress_block(&compressed, CompressionType::Lz4, data.len())?;
        assert_eq!(&decompressed, &data[..]);
        Ok(())
    }

    #[test]
    fn test_compress_decompress_deflate() -> Result<()> {
        let data = b"hello world hello world hello world hello world hello world";
        let compressed = compress_block(data, CompressionType::Deflate)?;
        let decompressed = decompress_block(&compressed, CompressionType::Deflate, data.len())?;
        assert_eq!(&decompressed, &data[..]);
        Ok(())
    }

    #[test]
    fn test_deflate_better_ratio_than_lz4() -> Result<()> {
        // Generate repetitive data where deflate should compress better
        let mut data = Vec::with_capacity(4096);
        for i in 0..512 {
            data.extend_from_slice(
                &format!("key_{:04}=value_{:04}\n", i % 50, i % 50).into_bytes(),
            );
        }

        let lz4_compressed = compress_block(&data, CompressionType::Lz4)?;
        let deflate_compressed = compress_block(&data, CompressionType::Deflate)?;

        // DEFLATE should generally produce smaller output than LZ4
        assert!(
            deflate_compressed.len() <= lz4_compressed.len(),
            "DEFLATE ({}) should be <= LZ4 ({})",
            deflate_compressed.len(),
            lz4_compressed.len()
        );

        // Both should roundtrip correctly
        let lz4_recovered = decompress_block(&lz4_compressed, CompressionType::Lz4, data.len())?;
        let deflate_recovered =
            decompress_block(&deflate_compressed, CompressionType::Deflate, data.len())?;
        assert_eq!(lz4_recovered, data);
        assert_eq!(deflate_recovered, data);
        Ok(())
    }

    #[test]
    fn test_empty_data_compression() -> Result<()> {
        let data: &[u8] = b"";

        for ct in [
            CompressionType::None,
            CompressionType::Lz4,
            CompressionType::Deflate,
        ] {
            let compressed = compress_block(data, ct)?;
            let decompressed = decompress_block(&compressed, ct, 0)?;
            assert_eq!(decompressed.len(), 0, "Failed for {:?}", ct);
        }
        Ok(())
    }

    #[test]
    fn test_large_block_compression() -> Result<()> {
        // 64KB block of mixed data
        let mut data = Vec::with_capacity(65536);
        for i in 0..65536u32 {
            data.push((i % 256) as u8);
        }

        for ct in [CompressionType::Lz4, CompressionType::Deflate] {
            let compressed = compress_block(&data, ct)?;
            let decompressed = decompress_block(&compressed, ct, data.len())?;
            assert_eq!(decompressed, data, "Roundtrip failed for {:?}", ct);
        }
        Ok(())
    }

    #[test]
    fn test_size_mismatch_detection_lz4() -> Result<()> {
        let data = b"test data for mismatch detection";
        let compressed = compress_block(data, CompressionType::Lz4)?;

        // Wrong original_size should fail
        let result = decompress_block(&compressed, CompressionType::Lz4, data.len() + 10);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_size_mismatch_detection_deflate() -> Result<()> {
        let data = b"test data for mismatch detection in deflate";
        let compressed = compress_block(data, CompressionType::Deflate)?;

        // Wrong original_size should fail
        let result = decompress_block(&compressed, CompressionType::Deflate, data.len() + 10);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_exceeds_max_decompressed_size() {
        let data = b"small data";
        let result = decompress_block(data, CompressionType::Lz4, MAX_DECOMPRESSED_SIZE + 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_highly_compressible_data() -> Result<()> {
        // Data with lots of repetition should compress well
        let data = vec![0xAA; 8192];

        for ct in [CompressionType::Lz4, CompressionType::Deflate] {
            let compressed = compress_block(&data, ct)?;
            assert!(
                compressed.len() < data.len(),
                "{:?}: compressed {} should be < original {}",
                ct,
                compressed.len(),
                data.len()
            );
            let decompressed = decompress_block(&compressed, ct, data.len())?;
            assert_eq!(decompressed, data);
        }
        Ok(())
    }

    #[test]
    fn test_random_like_data() -> Result<()> {
        // Pseudo-random data (not truly random, but low compressibility)
        let mut data = Vec::with_capacity(4096);
        let mut state: u64 = 0xDEAD_BEEF_CAFE_BABE;
        for _ in 0..4096 {
            // Simple xorshift for deterministic pseudo-random bytes
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            data.push((state & 0xFF) as u8);
        }

        for ct in [CompressionType::Lz4, CompressionType::Deflate] {
            let compressed = compress_block(&data, ct)?;
            let decompressed = decompress_block(&compressed, ct, data.len())?;
            assert_eq!(decompressed, data, "Roundtrip failed for {:?}", ct);
        }
        Ok(())
    }
}
