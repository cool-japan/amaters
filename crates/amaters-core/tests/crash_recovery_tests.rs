//! Crash-recovery integration tests for the LSM-Tree storage engine.
//!
//! These tests exercise the WAL-based recovery path by:
//! 1. Writing committed state.
//! 2. Forcibly flushing or dropping the storage instance.
//! 3. Re-opening the same directory.
//! 4. Asserting that all durably-committed data is readable.
//!
//! The `test_recovery_partial_wal` test additionally truncates the last WAL
//! segment to half its size and verifies that the engine opens without panic
//! and remains usable after recovery.

use amaters_core::storage::{LsmTreeConfig, LsmTreeStorage};
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use std::fs;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an `LsmTreeStorage` whose data and WAL directories both live under
/// `dir`, so nothing leaks outside the temp directory.
fn open_storage(dir: &std::path::Path) -> LsmTreeStorage {
    let config = LsmTreeConfig {
        data_dir: dir.to_path_buf(),
        wal_dir: dir.join("wal"),
        ..Default::default()
    };
    LsmTreeStorage::with_config(config).expect("failed to open storage")
}

/// Write `count` key-value pairs to `storage` using keys `"key_00"` …
/// `"key_{count-1:02}"` and values `[i as u8; 32]`.
async fn write_n_keys(storage: &LsmTreeStorage, count: usize) {
    for i in 0..count {
        let key = Key::from_str(&format!("key_{i:02}"));
        let blob = CipherBlob::new(vec![i as u8; 32]);
        storage.put(&key, &blob).await.expect("put failed");
    }
}

// ---------------------------------------------------------------------------
// test_recovery_all_committed
// ---------------------------------------------------------------------------

/// After writing 10 committed key-value pairs, flushing, and dropping the
/// storage, re-opening the same directory must make all 10 keys readable.
#[tokio::test]
async fn test_recovery_all_committed() {
    let tmp = TempDir::new().expect("failed to create temp dir");
    let dir = tmp.path().to_path_buf();

    // Phase 1: write + flush + drop.
    {
        let storage = open_storage(&dir);
        write_n_keys(&storage, 10).await;
        // Explicit flush so the memtable is persisted to SSTable before drop.
        storage.flush().await.expect("flush failed");
    }

    // Phase 2: reopen and verify.
    {
        let storage = open_storage(&dir);
        for i in 0..10usize {
            let key = Key::from_str(&format!("key_{i:02}"));
            let result = storage.get(&key).await.expect("get failed");
            let blob = result.unwrap_or_else(|| panic!("key_{i:02} not found after recovery"));
            assert_eq!(
                blob.as_bytes(),
                &vec![i as u8; 32],
                "value mismatch for key_{i:02}"
            );
        }
    }

    // TempDir drops here, cleaning up.
}

// ---------------------------------------------------------------------------
// test_recovery_partial_wal
// ---------------------------------------------------------------------------

/// Write 10 keys, drop storage (WAL on disk), then truncate the last WAL
/// segment to half its size.  Re-open the storage and assert:
/// - No panic during recovery.
/// - The storage is usable (a new key can be written and read back).
///
/// The exact set of recovered keys depends on WAL format alignment; we
/// deliberately avoid asserting a specific count because partial records at
/// the truncation boundary may be discarded by the CRC-checking replayer.
#[tokio::test]
async fn test_recovery_partial_wal() {
    let tmp = TempDir::new().expect("failed to create temp dir");
    let dir = tmp.path().to_path_buf();

    // Phase 1: write + drop (WAL stays on disk, memtable NOT explicitly flushed).
    {
        let storage = open_storage(&dir);
        write_n_keys(&storage, 10).await;
        // Do NOT call flush — we want unmerged WAL records on disk.
    }

    // Phase 2: locate and truncate the last WAL file.
    let wal_dir = dir.join("wal");
    if wal_dir.exists() {
        let mut wal_files: Vec<_> = fs::read_dir(&wal_dir)
            .expect("failed to read wal dir")
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                s.starts_with("wal_") && s.ends_with(".log")
            })
            .collect();

        // Sort by name so we pick the last (highest-numbered) segment.
        wal_files.sort_by_key(|e| e.file_name());

        if let Some(last) = wal_files.last() {
            let path = last.path();
            let size = fs::metadata(&path).expect("metadata failed").len();
            if size > 0 {
                let truncated_size = size / 2;
                let file = fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .expect("failed to open WAL for truncation");
                file.set_len(truncated_size).expect("set_len failed");
            }
        }
    }

    // Phase 3: reopen — must not panic.
    {
        let storage = open_storage(&dir);

        // The storage must be usable: write + read a new key.
        let probe_key = Key::from_str("probe_after_recovery");
        let probe_val = CipherBlob::new(vec![0xAB; 16]);
        storage
            .put(&probe_key, &probe_val)
            .await
            .expect("put after recovery failed");

        let result = storage
            .get(&probe_key)
            .await
            .expect("get after recovery failed");
        assert!(
            result.is_some(),
            "probe key must be readable after partial-WAL recovery"
        );
    }

    // TempDir drops here.
}

// ---------------------------------------------------------------------------
// test_recovery_empty_wal
// ---------------------------------------------------------------------------

/// Open storage, write nothing, drop, reopen — must succeed and contain no
/// keys (empty state).
#[tokio::test]
async fn test_recovery_empty_wal() {
    let tmp = TempDir::new().expect("failed to create temp dir");
    let dir = tmp.path().to_path_buf();

    // Phase 1: open and immediately drop without writing anything.
    {
        let _storage = open_storage(&dir);
    }

    // Phase 2: reopen — must not panic and must have no keys.
    {
        let storage = open_storage(&dir);

        // Spot-check a handful of key names that were never written.
        for i in 0..5usize {
            let key = Key::from_str(&format!("key_{i:02}"));
            let result = storage.get(&key).await.expect("get failed");
            assert!(
                result.is_none(),
                "key_{i:02} unexpectedly found in empty storage"
            );
        }
    }

    // TempDir drops here.
}
