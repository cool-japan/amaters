//! Compression algorithms demonstration.
//!
//! Demonstrates:
//! - All compression types (None, LZ4, Deflate)
//! - Compression ratio comparison
//! - Compress/decompress roundtrip verification
//! - Performance characteristics with different data patterns

use amaters::core::storage::compression::{CompressionType, compress_block, decompress_block};

fn main() -> amaters::core::Result<()> {
    println!("=== AmateRS Compression Example ===\n");

    // =========================================================================
    // 1. Basic roundtrip for each compression type
    // =========================================================================
    println!("--- Basic Roundtrip ---");

    let sample_data = b"Hello, AmateRS! This is a test of the compression subsystem.";
    let compression_types = [
        ("None", CompressionType::None),
        ("LZ4", CompressionType::Lz4),
        ("Deflate", CompressionType::Deflate),
    ];

    for (name, ct) in &compression_types {
        let compressed = compress_block(sample_data, *ct)?;
        let decompressed = decompress_block(&compressed, *ct, sample_data.len())?;

        assert_eq!(
            &decompressed,
            &sample_data[..],
            "Roundtrip failed for {}",
            name
        );
        println!(
            "  {:<10} original={:>4}B  compressed={:>4}B  ratio={:.2}x  roundtrip=OK",
            name,
            sample_data.len(),
            compressed.len(),
            sample_data.len() as f64 / compressed.len() as f64
        );
    }
    println!();

    // =========================================================================
    // 2. Compare compression ratios with different data patterns
    // =========================================================================
    println!("--- Compression Ratio Comparison ---");

    // Pattern 1: Highly repetitive data (compresses well)
    let repetitive_data = vec![0xAA_u8; 8192];
    print_compression_comparison("Repetitive (8KB)", &repetitive_data)?;

    // Pattern 2: Structured data (key-value pairs)
    let mut structured_data = Vec::with_capacity(8192);
    for i in 0..256 {
        structured_data
            .extend_from_slice(format!("key_{:04}=value_{:04}\n", i % 50, i % 50).as_bytes());
    }
    print_compression_comparison("Structured KV", &structured_data)?;

    // Pattern 3: Pseudo-random data (compresses poorly)
    let mut random_data = Vec::with_capacity(8192);
    let mut state: u64 = 0xDEAD_BEEF_CAFE_BABE;
    for _ in 0..8192 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        random_data.push((state & 0xFF) as u8);
    }
    print_compression_comparison("Pseudo-random", &random_data)?;

    // Pattern 4: JSON-like data
    let mut json_data = Vec::with_capacity(8192);
    for i in 0..100 {
        json_data.extend_from_slice(
            format!(
                r#"{{"id":{},"name":"user_{}","active":true,"score":{}}}"#,
                i,
                i,
                i * 42
            )
            .as_bytes(),
        );
        json_data.push(b'\n');
    }
    print_compression_comparison("JSON-like", &json_data)?;

    // Pattern 5: Binary blobs (simulated encrypted data)
    let mut binary_data = Vec::with_capacity(8192);
    for i in 0..8192_u32 {
        binary_data.push((i.wrapping_mul(7) % 256) as u8);
    }
    print_compression_comparison("Binary blob", &binary_data)?;

    println!();

    // =========================================================================
    // 3. Compression type metadata
    // =========================================================================
    println!("--- CompressionType Encoding ---");

    for (name, ct) in &compression_types {
        let byte_val = ct.to_byte();
        let recovered = CompressionType::from_byte(byte_val)?;
        println!(
            "  {:<10} byte={} recovered={:?} match={}",
            name,
            byte_val,
            recovered,
            *ct == recovered
        );
    }

    // Invalid byte
    match CompressionType::from_byte(255) {
        Ok(_) => println!("  byte=255 unexpectedly succeeded"),
        Err(e) => println!("  byte=255 correctly rejected: {}", e),
    }
    println!();

    // =========================================================================
    // 4. Empty data handling
    // =========================================================================
    println!("--- Edge Cases ---");

    let empty: &[u8] = b"";
    for (name, ct) in &compression_types {
        let compressed = compress_block(empty, *ct)?;
        let decompressed = decompress_block(&compressed, *ct, 0)?;
        println!(
            "  {} empty: compressed={}B decompressed={}B OK",
            name,
            compressed.len(),
            decompressed.len()
        );
    }

    // Single byte
    let single = &[42u8];
    for (name, ct) in &compression_types {
        let compressed = compress_block(single, *ct)?;
        let decompressed = decompress_block(&compressed, *ct, 1)?;
        assert_eq!(&decompressed, &single[..]);
        println!(
            "  {} single byte: compressed={}B roundtrip=OK",
            name,
            compressed.len()
        );
    }

    println!("\nExample finished.");
    Ok(())
}

/// Helper to print compression comparison for a data pattern
fn print_compression_comparison(label: &str, data: &[u8]) -> amaters::core::Result<()> {
    let original_size = data.len();

    let lz4_compressed = compress_block(data, CompressionType::Lz4)?;
    let deflate_compressed = compress_block(data, CompressionType::Deflate)?;

    // Verify roundtrip
    let lz4_decompressed = decompress_block(&lz4_compressed, CompressionType::Lz4, original_size)?;
    let deflate_decompressed =
        decompress_block(&deflate_compressed, CompressionType::Deflate, original_size)?;
    assert_eq!(lz4_decompressed, data, "LZ4 roundtrip failed for {}", label);
    assert_eq!(
        deflate_decompressed, data,
        "Deflate roundtrip failed for {}",
        label
    );

    let lz4_ratio = original_size as f64 / lz4_compressed.len() as f64;
    let deflate_ratio = original_size as f64 / deflate_compressed.len() as f64;

    let winner = if deflate_compressed.len() <= lz4_compressed.len() {
        "Deflate"
    } else {
        "LZ4"
    };

    println!(
        "  {:<16} original={:>6}B  LZ4={:>6}B ({:.2}x)  Deflate={:>6}B ({:.2}x)  winner={}",
        label,
        original_size,
        lz4_compressed.len(),
        lz4_ratio,
        deflate_compressed.len(),
        deflate_ratio,
        winner
    );

    Ok(())
}
