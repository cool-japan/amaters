//! Advanced storage engine (LSM-Tree) example.
//!
//! Demonstrates:
//! - Creating an LsmTree with custom configuration
//! - Writing many entries to trigger memtable flush
//! - Triggering and observing compaction
//! - WAL recovery
//! - Displaying statistics

use amaters::core::storage::compression::CompressionType;
use amaters::core::storage::{
    BlockCacheConfig, BloomFilterConfig, CompactionConfig, CompactionStrategy, LsmTree,
    LsmTreeConfig, MemtableConfig, SSTableConfig,
};
use amaters::core::{CipherBlob, Key};

fn main() -> amaters::core::Result<()> {
    println!("=== AmateRS Storage Engine Example ===\n");

    let temp_dir = std::env::temp_dir().join("amaters_storage_engine_example");
    let data_dir = temp_dir.join("data");
    let wal_dir = temp_dir.join("wal");

    // Clean up any previous run
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    // =========================================================================
    // 1. Create LsmTree with custom configuration
    // =========================================================================
    println!("--- Custom Configuration ---");

    let config = LsmTreeConfig {
        data_dir: data_dir.clone(),
        wal_dir: wal_dir.clone(),
        memtable_config: MemtableConfig {
            // Use a small memtable to trigger flushes quickly
            max_size_bytes: 32 * 1024, // 32 KB
            enable_wal: true,
        },
        sstable_config: SSTableConfig {
            block_size: 4096,
            compression_type: CompressionType::Lz4,
        },
        block_cache_config: BlockCacheConfig::default(),
        compaction_config: CompactionConfig {
            strategy: CompactionStrategy::LevelBased,
            l0_threshold: 2, // Compact after 2 L0 SSTables (low for demo)
            level_multiplier: 10,
            ..CompactionConfig::default()
        },
        value_log_config: None,
        max_levels: 7,
        l0_compaction_threshold: 2,
        level_size_multiplier: 10,
    };

    println!(
        "  Memtable size:       {} KB",
        config.memtable_config.max_size_bytes / 1024
    );
    println!(
        "  SSTable block size:  {} bytes",
        config.sstable_config.block_size
    );
    println!(
        "  Compression:         {:?}",
        config.sstable_config.compression_type
    );
    println!(
        "  Compaction strategy: {:?}",
        config.compaction_config.strategy
    );
    println!(
        "  L0 threshold:        {}",
        config.compaction_config.l0_threshold
    );
    println!("  Max levels:          {}", config.max_levels);
    println!();

    let tree = LsmTree::with_config(config)?;
    println!("LSM-Tree created at: {}\n", temp_dir.display());

    // =========================================================================
    // 2. Write many entries to trigger flushes
    // =========================================================================
    println!("--- Bulk Write ---");

    let num_entries = 500;
    let value_size = 256; // 256 bytes per value

    for i in 0..num_entries {
        let key = Key::from_str(&format!("record:{:06}", i));
        // Create a value with some structure for better compression
        let mut value_data = Vec::with_capacity(value_size);
        for j in 0..value_size {
            value_data.push(((i + j) % 256) as u8);
        }
        let value = CipherBlob::new(value_data);
        tree.put(key, value)?;

        // Print progress every 100 entries
        if (i + 1) % 100 == 0 {
            println!("  Written {}/{} entries", i + 1, num_entries);
        }
    }
    println!();

    // =========================================================================
    // 3. Display statistics after writes
    // =========================================================================
    println!("--- Statistics After Writes ---");
    print_stats(&tree);
    println!();

    // =========================================================================
    // 4. Verify data integrity
    // =========================================================================
    println!("--- Data Verification ---");

    let mut verified = 0;
    let mut missing = 0;
    for i in 0..num_entries {
        let key = Key::from_str(&format!("record:{:06}", i));
        match tree.get(&key)? {
            Some(value) => {
                // Verify first byte
                let expected_first = (i % 256) as u8;
                if value.as_bytes().first() == Some(&expected_first) {
                    verified += 1;
                }
            }
            None => {
                missing += 1;
            }
        }
    }
    println!(
        "  Verified: {}/{}  Missing: {}",
        verified, num_entries, missing
    );
    println!();

    // =========================================================================
    // 5. Range scan performance
    // =========================================================================
    println!("--- Range Scan ---");

    let start = Key::from_str("record:000100");
    let end = Key::from_str("record:000200");
    let results = tree.range(&start, &end)?;
    println!(
        "  Range [record:000100, record:000200): {} results",
        results.len()
    );

    if let Some((first_key, first_value)) = results.first() {
        println!(
            "    First: {} ({} bytes)",
            String::from_utf8_lossy(first_key.as_bytes()),
            first_value.len()
        );
    }
    if let Some((last_key, last_value)) = results.last() {
        println!(
            "    Last:  {} ({} bytes)",
            String::from_utf8_lossy(last_key.as_bytes()),
            last_value.len()
        );
    }
    println!();

    // =========================================================================
    // 6. Flush and close
    // =========================================================================
    println!("--- Flush & Close ---");
    tree.flush()?;
    println!("  All pending writes flushed to disk.");

    // Print final stats
    print_stats(&tree);

    tree.close()?;
    println!("  LSM-Tree closed gracefully.");
    println!();

    // =========================================================================
    // 7. WAL Recovery demonstration
    // =========================================================================
    println!("--- WAL Recovery ---");

    // Re-open the tree from disk (recovers from WAL + SSTables)
    let recovery_config = LsmTreeConfig {
        data_dir: data_dir.clone(),
        wal_dir: wal_dir.clone(),
        memtable_config: MemtableConfig {
            max_size_bytes: 32 * 1024,
            enable_wal: true,
        },
        sstable_config: SSTableConfig {
            block_size: 4096,
            compression_type: CompressionType::Lz4,
        },
        block_cache_config: BlockCacheConfig::default(),
        compaction_config: CompactionConfig::default(),
        value_log_config: None,
        max_levels: 7,
        l0_compaction_threshold: 4,
        level_size_multiplier: 10,
    };

    let recovered_tree = LsmTree::with_config(recovery_config)?;
    println!("  LSM-Tree recovered from disk.");

    // Verify some data survived
    let check_key = Key::from_str("record:000042");
    match recovered_tree.get(&check_key)? {
        Some(value) => {
            println!(
                "  Verified record:000042 after recovery: {} bytes",
                value.len()
            );
        }
        None => {
            println!("  record:000042 not found after recovery (may be in WAL only)");
        }
    }

    // Show recovered stats
    print_stats(&recovered_tree);

    recovered_tree.close()?;
    println!("  Recovery complete.\n");

    // =========================================================================
    // 8. Bloom filter configuration example
    // =========================================================================
    println!("--- Bloom Filter Config ---");
    let bloom_config = BloomFilterConfig {
        expected_elements: 100_000,
        false_positive_rate: 0.001, // 0.1%
    };
    println!("  Expected elements:    {}", bloom_config.expected_elements);
    println!(
        "  False positive rate:  {}%",
        bloom_config.false_positive_rate * 100.0
    );
    println!();

    // =========================================================================
    // Cleanup
    // =========================================================================
    std::fs::remove_dir_all(&temp_dir).ok();
    println!("Cleanup complete. Example finished.");

    Ok(())
}

/// Print LSM-Tree statistics
fn print_stats(tree: &LsmTree) {
    let stats = tree.stats();
    println!("  Statistics:");
    println!("    Memtable size:     {} bytes", stats.memtable_size);
    println!("    Number of levels:  {}", stats.num_levels);
    println!(
        "    Cache hit rate:    {:.2}%",
        stats.cache_hit_rate * 100.0
    );
    println!("    Cache size:        {} bytes", stats.cache_size);

    for level in &stats.levels {
        if !level.sstables.is_empty() {
            println!(
                "    Level {}: {} SSTables, {} bytes total",
                level.level,
                level.sstables.len(),
                level.total_size
            );
        }
    }

    let cs = &stats.compaction_stats;
    if cs.compactions_completed > 0 {
        println!(
            "    Compaction: {} completed, {} bytes read, {} bytes written",
            cs.compactions_completed, cs.bytes_read, cs.bytes_written
        );
    }
}
