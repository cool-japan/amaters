use super::*;
use crate::storage::Memtable;
use tempfile::tempdir;

#[test]
fn test_wal_entry_encode_decode() -> Result<()> {
    let key = Key::from_str("test_key");
    let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);
    let entry = WalEntry::put(42, key.clone(), value.clone());

    let bytes = entry.encode();
    let decoded = WalEntry::decode(&bytes)?;

    assert_eq!(decoded.sequence, 42);
    assert_eq!(decoded.entry_type, WalEntryType::Put);
    assert_eq!(decoded.key, key);
    assert_eq!(decoded.value, Some(value));

    Ok(())
}

#[test]
fn test_wal_delete_entry() -> Result<()> {
    let key = Key::from_str("delete_me");
    let entry = WalEntry::delete(99, key.clone());

    let bytes = entry.encode();
    let decoded = WalEntry::decode(&bytes)?;

    assert_eq!(decoded.sequence, 99);
    assert_eq!(decoded.entry_type, WalEntryType::Delete);
    assert_eq!(decoded.key, key);
    assert_eq!(decoded.value, None);

    Ok(())
}

#[test]
fn test_wal_checksum_verification() -> Result<()> {
    let key = Key::from_str("test");
    let value = CipherBlob::new(vec![1, 2, 3]);
    let entry = WalEntry::put(1, key, value);

    // Verify should pass
    entry.verify_checksum()?;

    // Corrupt checksum
    let mut corrupted = entry.clone();
    corrupted.checksum = 0;

    // Verify should fail
    assert!(corrupted.verify_checksum().is_err());

    Ok(())
}

#[test]
fn test_wal_basic_operations() -> Result<()> {
    let temp_dir = tempdir().map_err(|e| {
        AmateRSError::IoError(ErrorContext::new(format!(
            "Failed to create temp dir: {}",
            e
        )))
    })?;
    let wal_path = temp_dir.path().join("test.wal");

    let mut wal = Wal::create(&wal_path)?;

    // Write some entries
    let seq1 = wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
    let seq2 = wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
    let seq3 = wal.delete(Key::from_str("key1"))?;

    assert_eq!(seq1, 0);
    assert_eq!(seq2, 1);
    assert_eq!(seq3, 2);

    wal.flush()?;

    // Verify a WAL file was created (may be rotated, so check path() returns something that exists)
    assert!(wal.path().exists());

    Ok(())
}

#[test]
fn test_wal_sequence_increment() -> Result<()> {
    let temp_dir = tempdir().map_err(|e| {
        AmateRSError::IoError(ErrorContext::new(format!(
            "Failed to create temp dir: {}",
            e
        )))
    })?;
    let wal_path = temp_dir.path().join("test_seq.wal");

    let mut wal = Wal::create(&wal_path)?;

    assert_eq!(wal.sequence(), 0);

    wal.put(Key::from_str("key"), CipherBlob::new(vec![1]))?;
    assert_eq!(wal.sequence(), 1);

    wal.delete(Key::from_str("key"))?;
    assert_eq!(wal.sequence(), 2);

    Ok(())
}

#[test]
fn test_wal_entry_large_value() -> Result<()> {
    let key = Key::from_str("large");
    let large_value = CipherBlob::new(vec![0u8; 10_000]);
    let entry = WalEntry::put(1, key.clone(), large_value.clone());

    let bytes = entry.encode();
    let decoded = WalEntry::decode(&bytes)?;

    assert_eq!(decoded.key, key);
    assert_eq!(decoded.value, Some(large_value));

    Ok(())
}

#[test]
fn test_wal_rotation() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_rotation");
    std::fs::create_dir_all(&temp_dir).ok();

    let config = WalConfig {
        wal_dir: temp_dir.clone(),
        max_file_size: 1024,  // Small size to trigger rotation
        sync_on_write: false, // Disable for speed
        ..Default::default()
    };

    let mut wal = Wal::with_config(config)?;

    let initial_file_number = wal.current_file_number();

    // Write enough data to trigger rotation
    for i in 0..20 {
        wal.put(
            Key::from_str(&format!("key_{}", i)),
            CipherBlob::new(vec![i as u8; 100]),
        )?;
    }

    // File number should have increased due to rotation
    assert!(wal.current_file_number() > initial_file_number);

    // Verify new file exists
    assert!(wal.path().exists());

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_cleanup() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_cleanup");
    std::fs::create_dir_all(&temp_dir).ok();

    let config = WalConfig {
        wal_dir: temp_dir.clone(),
        max_file_size: 512, // Very small to trigger many rotations
        max_wal_files: 3,   // Keep only 3 files
        sync_on_write: false,
    };

    let mut wal = Wal::with_config(config)?;

    // Write enough data to create many WAL files
    for i in 0..100 {
        wal.put(
            Key::from_str(&format!("key_{}", i)),
            CipherBlob::new(vec![i as u8; 100]),
        )?;
    }

    // Count WAL files
    let wal_file_count = std::fs::read_dir(&temp_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("wal_")
                && e.file_name().to_string_lossy().ends_with(".log")
        })
        .count();

    // Should have at most max_wal_files
    assert!(wal_file_count <= 3);

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_manual_cleanup() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_manual_cleanup");
    std::fs::create_dir_all(&temp_dir).ok();

    let config = WalConfig {
        wal_dir: temp_dir.clone(),
        max_file_size: 512,
        max_wal_files: 5,
        sync_on_write: false,
    };

    let mut wal = Wal::with_config(config)?;

    // Create several WAL files
    for i in 0..80 {
        wal.put(
            Key::from_str(&format!("key_{}", i)),
            CipherBlob::new(vec![i as u8; 100]),
        )?;
    }

    // Manually trigger cleanup
    wal.cleanup()?;

    // Count files
    let wal_file_count = std::fs::read_dir(&temp_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("wal_")
                && e.file_name().to_string_lossy().ends_with(".log")
        })
        .count();

    assert!(wal_file_count <= 5);

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_recovery_basic() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_recovery_basic");
    std::fs::create_dir_all(&temp_dir).ok();

    // Write some entries
    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            sync_on_write: true,
            ..Default::default()
        };

        let mut wal = Wal::with_config(config)?;

        wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
        wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
        wal.delete(Key::from_str("key1"))?;
        wal.put(Key::from_str("key3"), CipherBlob::new(vec![7, 8, 9]))?;

        wal.flush()?;
    }

    // Recover entries
    let (entries, max_sequence) = Wal::recover(&temp_dir)?;

    assert_eq!(entries.len(), 4);
    assert_eq!(max_sequence, 3);

    // Verify entries
    assert_eq!(entries[0].key, Key::from_str("key1"));
    assert_eq!(entries[0].entry_type, WalEntryType::Put);
    assert_eq!(entries[0].value, Some(CipherBlob::new(vec![1, 2, 3])));

    assert_eq!(entries[1].key, Key::from_str("key2"));
    assert_eq!(entries[1].entry_type, WalEntryType::Put);

    assert_eq!(entries[2].key, Key::from_str("key1"));
    assert_eq!(entries[2].entry_type, WalEntryType::Delete);
    assert_eq!(entries[2].value, None);

    assert_eq!(entries[3].key, Key::from_str("key3"));
    assert_eq!(entries[3].entry_type, WalEntryType::Put);

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_recovery_multiple_files() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_recovery_multiple");
    std::fs::create_dir_all(&temp_dir).ok();

    // Write entries across multiple WAL files
    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            max_file_size: 512, // Small to trigger rotation
            sync_on_write: true,
            ..Default::default()
        };

        let mut wal = Wal::with_config(config)?;

        // Write enough data to create multiple files
        for i in 0..20 {
            wal.put(
                Key::from_str(&format!("key_{}", i)),
                CipherBlob::new(vec![i as u8; 100]),
            )?;
        }

        wal.flush()?;
    }

    // Recover all entries
    let (entries, max_sequence) = Wal::recover(&temp_dir)?;

    assert_eq!(entries.len(), 20);
    assert_eq!(max_sequence, 19);

    // Verify entries are in sequence order
    for (i, entry) in entries.iter().enumerate() {
        assert_eq!(entry.sequence, i as u64);
        assert_eq!(entry.key, Key::from_str(&format!("key_{}", i)));
    }

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_recovery_empty_directory() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_recovery_empty");
    std::fs::create_dir_all(&temp_dir).ok();

    // Recover from empty directory
    let (entries, max_sequence) = Wal::recover(&temp_dir)?;

    assert_eq!(entries.len(), 0);
    assert_eq!(max_sequence, 0);

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_recovery_nonexistent_directory() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("nonexistent_wal_dir_12345");

    // Recover from non-existent directory
    let (entries, max_sequence) = Wal::recover(&temp_dir)?;

    assert_eq!(entries.len(), 0);
    assert_eq!(max_sequence, 0);

    Ok(())
}

#[test]
fn test_wal_replay_to_memtable() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_replay_memtable");
    std::fs::create_dir_all(&temp_dir).ok();

    // Write some entries
    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            sync_on_write: true,
            ..Default::default()
        };

        let mut wal = Wal::with_config(config)?;

        wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
        wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
        wal.delete(Key::from_str("key1"))?;
        wal.put(Key::from_str("key3"), CipherBlob::new(vec![7, 8, 9]))?;

        wal.flush()?;
    }

    // Create a new memtable and replay WAL
    let memtable = Memtable::new();
    let max_sequence = Wal::replay_to_memtable(&temp_dir, &memtable)?;

    assert_eq!(max_sequence, 3);

    // Verify memtable state
    assert_eq!(memtable.get(&Key::from_str("key1"))?, None); // Deleted
    assert_eq!(
        memtable.get(&Key::from_str("key2"))?,
        Some(CipherBlob::new(vec![4, 5, 6]))
    );
    assert_eq!(
        memtable.get(&Key::from_str("key3"))?,
        Some(CipherBlob::new(vec![7, 8, 9]))
    );

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_reader_basic() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_reader_basic");
    std::fs::create_dir_all(&temp_dir).ok();

    let wal_file = temp_dir.join("test.wal");

    // Write some entries
    {
        let mut wal = Wal::create(&wal_file)?;
        wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
        wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
        wal.flush()?;
    }

    // Read entries with WalReader
    let wal_file_actual = temp_dir.join("wal_00000000.log");
    let mut reader = WalReader::open(&wal_file_actual)?;

    let entry1 = reader.read_entry()?.expect("Should have entry 1");
    assert_eq!(entry1.sequence, 0);
    assert_eq!(entry1.key, Key::from_str("key1"));

    let entry2 = reader.read_entry()?.expect("Should have entry 2");
    assert_eq!(entry2.sequence, 1);
    assert_eq!(entry2.key, Key::from_str("key2"));

    let entry3 = reader.read_entry()?;
    assert_eq!(entry3, None); // End of file

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_recovery_with_truncated_file() -> Result<()> {
    use std::env;
    use std::io::Write as IoWrite;

    let temp_dir = env::temp_dir().join("test_wal_recovery_truncated");
    std::fs::create_dir_all(&temp_dir).ok();

    // Write some valid entries, then truncate the last one
    let wal_file = temp_dir.join("wal_00000000.log");
    {
        let mut wal = Wal::create(&wal_file)?;
        wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
        wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
        wal.flush()?;

        // Append incomplete entry (length prefix only, no data)
        let mut file = OpenOptions::new().append(true).open(&wal_file)?;
        let incomplete_len = 1234u32;
        file.write_all(&incomplete_len.to_le_bytes())?;
        file.flush()?;
    }

    // Recovery should handle truncated entry gracefully
    let (entries, _) = Wal::recover(&temp_dir)?;

    // Should recover the 2 complete entries, skip the truncated one
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].key, Key::from_str("key1"));
    assert_eq!(entries[1].key, Key::from_str("key2"));

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_sequence_recovery_after_crash() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_seq_recovery_crash");
    // Clean up from any previous run
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    // Phase 1: Write entries and then drop (simulate crash)
    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            sync_on_write: true,
            ..Default::default()
        };

        let mut wal = Wal::with_config(config)?;

        wal.put(Key::from_str("a"), CipherBlob::new(vec![1]))?;
        wal.put(Key::from_str("b"), CipherBlob::new(vec![2]))?;
        wal.put(Key::from_str("c"), CipherBlob::new(vec![3]))?;
        wal.put(Key::from_str("d"), CipherBlob::new(vec![4]))?;
        wal.put(Key::from_str("e"), CipherBlob::new(vec![5]))?;
        wal.flush()?;
        // sequences 0..4 written, next should be 5
    }

    // Phase 2: Open a new WAL instance - should recover sequence
    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            sync_on_write: true,
            ..Default::default()
        };

        let mut wal = Wal::with_config(config)?;

        // Sequence should continue from 5 (max was 4, so next is 5)
        assert_eq!(wal.sequence(), 5);

        // Write more entries
        let seq = wal.put(Key::from_str("f"), CipherBlob::new(vec![6]))?;
        assert_eq!(seq, 5);

        let seq = wal.put(Key::from_str("g"), CipherBlob::new(vec![7]))?;
        assert_eq!(seq, 6);

        wal.flush()?;
    }

    // Phase 3: Verify all entries are recoverable
    let (entries, max_sequence) = Wal::recover(&temp_dir)?;
    assert_eq!(entries.len(), 7);
    assert_eq!(max_sequence, 6);

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_corruption_detection_and_partial_recovery() -> Result<()> {
    use std::env;
    use std::io::Write as IoWrite;

    let temp_dir = env::temp_dir().join("test_wal_corruption_detect");
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    let wal_file = temp_dir.join("wal_00000000.log");

    // Write valid entries
    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            sync_on_write: true,
            ..Default::default()
        };

        let mut wal = Wal::with_config(config)?;
        wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
        wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
        wal.put(Key::from_str("key3"), CipherBlob::new(vec![7, 8, 9]))?;
        wal.flush()?;
    }

    // Corrupt the middle entry by modifying bytes in the WAL file
    {
        let data = std::fs::read(&wal_file).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to read WAL: {}", e)))
        })?;

        let mut corrupted_data = data.clone();
        // The first entry starts at offset 0: 4 bytes length prefix + entry data
        // Find the start of the second entry by reading the first entry's length
        let first_entry_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let second_entry_start = 4 + first_entry_len;

        // Corrupt bytes inside the second entry (after length prefix, corrupt the checksum area)
        let corrupt_offset = second_entry_start + 4 + 10; // Skip length prefix and some bytes
        if corrupt_offset < corrupted_data.len() {
            corrupted_data[corrupt_offset] ^= 0xFF;
        }

        let mut file = File::create(&wal_file).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to create file: {}", e)))
        })?;
        file.write_all(&corrupted_data).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to write file: {}", e)))
        })?;
        file.flush().map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to flush file: {}", e)))
        })?;
    }

    // Recovery should detect corruption and skip the corrupted entry
    let (entries, _max_seq, stats) = Wal::recover_with_stats(&temp_dir)?;

    // We should have recovered 2 out of 3 entries (one corrupted)
    assert_eq!(stats.entries_corrupted, 1);
    assert_eq!(stats.entries_recovered, entries.len() as u64);
    assert!(stats.bytes_recovered > 0);
    // At least 2 entries should be recovered (first and possibly third)
    assert!(entries.len() >= 2);

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_truncate_before() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_truncate_before");
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    let config = WalConfig {
        wal_dir: temp_dir.clone(),
        max_file_size: 512, // Small to trigger rotation
        max_wal_files: 100, // Don't auto-cleanup
        sync_on_write: true,
    };

    let mut wal = Wal::with_config(config)?;

    // Write enough entries to create multiple WAL files
    for i in 0..30 {
        wal.put(
            Key::from_str(&format!("key_{}", i)),
            CipherBlob::new(vec![i as u8; 100]),
        )?;
    }
    wal.flush()?;

    // Ensure multiple files were created
    let file_count_before = std::fs::read_dir(&temp_dir)
        .map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to read dir: {}", e)))
        })?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("wal_") && name.ends_with(".log")
        })
        .count();
    assert!(file_count_before > 1, "Should have multiple WAL files");

    // Truncate all entries with sequence <= 10
    let truncated = wal.truncate_before(10)?;

    // Should have truncated at least one file
    assert!(truncated > 0, "Should have truncated at least one file");

    // Verify remaining entries all have sequence > 10 or are in the current file
    let (remaining_entries, _) = Wal::recover(&temp_dir)?;
    // Entries in remaining files should include those with seq > 10
    // (some with seq <= 10 may remain if they share a file with seq > 10 entries)
    let has_high_seq = remaining_entries.iter().any(|e| e.sequence > 10);
    assert!(has_high_seq, "Should still have entries with sequence > 10");

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_size_tracking() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_size_tracking");
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    let config = WalConfig {
        wal_dir: temp_dir.clone(),
        sync_on_write: true,
        ..Default::default()
    };

    let mut wal = Wal::with_config(config)?;

    // Initial size should be 0
    assert_eq!(wal.current_size(), 0);

    // Write an entry
    wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))?;
    let size_after_one = wal.current_size();
    assert!(size_after_one > 0, "Size should increase after writing");

    // Write another entry
    wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))?;
    let size_after_two = wal.current_size();
    assert!(
        size_after_two > size_after_one,
        "Size should increase with more entries"
    );

    wal.flush()?;

    // Total WAL size should match current size (single file)
    let total = wal.total_wal_size()?;
    assert_eq!(total, size_after_two);

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_total_size_multiple_files() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_total_size_multi");
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    let config = WalConfig {
        wal_dir: temp_dir.clone(),
        max_file_size: 512,
        max_wal_files: 100,
        sync_on_write: true,
    };

    let mut wal = Wal::with_config(config)?;

    for i in 0..20 {
        wal.put(
            Key::from_str(&format!("key_{}", i)),
            CipherBlob::new(vec![i as u8; 100]),
        )?;
    }
    wal.flush()?;

    let total = wal.total_wal_size()?;
    assert!(total > 0, "Total WAL size should be positive");

    // Total should be larger than current file size if we have multiple files
    if wal.current_file_number() > 0 {
        assert!(
            total >= wal.current_size(),
            "Total size should be >= current file size"
        );
    }

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_empty_recovery() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_empty_recovery");
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    // Create an empty WAL file
    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            sync_on_write: true,
            ..Default::default()
        };
        let wal = Wal::with_config(config)?;
        drop(wal);
    }

    // Recovery from directory with empty WAL file
    let (entries, max_seq, stats) = Wal::recover_with_stats(&temp_dir)?;
    assert_eq!(entries.len(), 0);
    assert_eq!(max_seq, 0);
    assert_eq!(stats.entries_recovered, 0);
    assert_eq!(stats.entries_corrupted, 0);

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_single_entry_recovery() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_single_entry_recovery");
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            sync_on_write: true,
            ..Default::default()
        };

        let mut wal = Wal::with_config(config)?;
        wal.put(Key::from_str("only_key"), CipherBlob::new(vec![42]))?;
        wal.flush()?;
    }

    let (entries, max_seq, stats) = Wal::recover_with_stats(&temp_dir)?;
    assert_eq!(entries.len(), 1);
    assert_eq!(max_seq, 0);
    assert_eq!(stats.entries_recovered, 1);
    assert_eq!(stats.entries_corrupted, 0);
    assert!(stats.bytes_recovered > 0);
    assert_eq!(entries[0].key, Key::from_str("only_key"));

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_large_recovery() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_large_recovery");
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    let entry_count = 500;

    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            max_file_size: 4096,
            max_wal_files: 1000,
            sync_on_write: false,
        };

        let mut wal = Wal::with_config(config)?;

        for i in 0..entry_count {
            wal.put(
                Key::from_str(&format!("large_key_{:05}", i)),
                CipherBlob::new(vec![(i % 256) as u8; 50]),
            )?;
        }
        wal.flush()?;
    }

    // Recover and verify
    let (entries, max_seq, stats) = Wal::recover_with_stats(&temp_dir)?;
    assert_eq!(entries.len(), entry_count);
    assert_eq!(max_seq, (entry_count - 1) as u64);
    assert_eq!(stats.entries_recovered, entry_count as u64);
    assert_eq!(stats.entries_corrupted, 0);
    assert!(stats.bytes_recovered > 0);

    // Verify sequence order
    for (i, entry) in entries.iter().enumerate() {
        assert_eq!(entry.sequence, i as u64);
    }

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_truncate_keeps_current_file() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_truncate_keeps_current");
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    let config = WalConfig {
        wal_dir: temp_dir.clone(),
        max_file_size: 512,
        max_wal_files: 100,
        sync_on_write: true,
    };

    let mut wal = Wal::with_config(config)?;

    for i in 0..30 {
        wal.put(
            Key::from_str(&format!("key_{}", i)),
            CipherBlob::new(vec![i as u8; 100]),
        )?;
    }
    wal.flush()?;

    let current_file_num = wal.current_file_number();

    // Truncate everything (use a very high sequence number)
    wal.truncate_before(u64::MAX)?;

    // Current file should still exist
    let current_path = Wal::wal_file_path(&temp_dir, current_file_num);
    assert!(
        current_path.exists(),
        "Current active WAL file should not be removed"
    );

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_sequence_recovery_across_rotations() -> Result<()> {
    use std::env;

    let temp_dir = env::temp_dir().join("test_wal_seq_recovery_rotation");
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    // Phase 1: Write entries across multiple rotations
    let entries_written;
    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            max_file_size: 512,
            max_wal_files: 100,
            sync_on_write: true,
        };

        let mut wal = Wal::with_config(config)?;

        for i in 0..25 {
            wal.put(
                Key::from_str(&format!("rkey_{}", i)),
                CipherBlob::new(vec![i as u8; 80]),
            )?;
        }
        wal.flush()?;
        entries_written = wal.sequence();
    }

    // Phase 2: Open new WAL and verify sequence continues
    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            max_file_size: 512,
            max_wal_files: 100,
            sync_on_write: true,
        };

        let wal = Wal::with_config(config)?;
        assert_eq!(
            wal.sequence(),
            entries_written,
            "Sequence should continue from where it left off"
        );
    }

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

#[test]
fn test_wal_recovery_stats_with_corruption() -> Result<()> {
    use std::env;
    use std::io::Write as IoWrite;

    let temp_dir = env::temp_dir().join("test_wal_recovery_stats_corrupt");
    std::fs::remove_dir_all(&temp_dir).ok();
    std::fs::create_dir_all(&temp_dir).ok();

    let wal_file = temp_dir.join("wal_00000000.log");

    // Write valid entries
    {
        let config = WalConfig {
            wal_dir: temp_dir.clone(),
            sync_on_write: true,
            ..Default::default()
        };

        let mut wal = Wal::with_config(config)?;
        wal.put(Key::from_str("s1"), CipherBlob::new(vec![10]))?;
        wal.put(Key::from_str("s2"), CipherBlob::new(vec![20]))?;
        wal.flush()?;
    }

    // Append garbage data that looks like a valid length prefix but has bad content
    {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&wal_file)
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to open for corruption: {}",
                    e
                )))
            })?;
        // Write a length prefix for 30 bytes, then 30 bytes of garbage
        let fake_len = 30u32;
        file.write_all(&fake_len.to_le_bytes())
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("write error: {}", e))))?;
        file.write_all(&[0xDE; 30])
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("write error: {}", e))))?;
        file.flush()
            .map_err(|e| AmateRSError::IoError(ErrorContext::new(format!("flush error: {}", e))))?;
    }

    let (_entries, _max_seq, stats) = Wal::recover_with_stats(&temp_dir)?;

    assert_eq!(stats.entries_recovered, 2);
    assert!(
        stats.entries_corrupted >= 1,
        "Should detect at least one corrupted entry"
    );

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}
