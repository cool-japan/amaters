//! Garbage Collection for Value Log
//!
//! This module contains GC-related types and methods for the ValueLog.
//! It handles space reclamation by identifying segments with high dead ratios
//! and rewriting live entries to new segments.

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::{CipherBlob, Key};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::value_log::{VLogEntry, ValueLog};

/// Per-segment statistics for tracking live/dead entries
#[derive(Debug, Clone)]
pub struct SegmentStats {
    /// Total bytes in the segment
    pub total_bytes: u64,
    /// Bytes occupied by live entries
    pub live_bytes: u64,
    /// Bytes occupied by dead (invalidated) entries
    pub dead_bytes: u64,
    /// Total number of entries written to this segment
    pub entry_count: u64,
    /// Number of live entries remaining
    pub live_count: u64,
    /// Timestamp when the segment was created
    pub created_at: Instant,
}

impl SegmentStats {
    /// Create new stats for a fresh segment
    pub(crate) fn new() -> Self {
        Self {
            total_bytes: 0,
            live_bytes: 0,
            dead_bytes: 0,
            entry_count: 0,
            live_count: 0,
            created_at: Instant::now(),
        }
    }

    /// Record a new live entry
    pub(crate) fn record_write(&mut self, entry_bytes: u64) {
        self.total_bytes += entry_bytes;
        self.live_bytes += entry_bytes;
        self.entry_count += 1;
        self.live_count += 1;
    }

    /// Mark an entry as dead (move bytes from live to dead)
    pub(crate) fn mark_entry_dead(&mut self, entry_bytes: u64) {
        let move_bytes = entry_bytes.min(self.live_bytes);
        self.live_bytes -= move_bytes;
        self.dead_bytes += move_bytes;
        if self.live_count > 0 {
            self.live_count -= 1;
        }
    }

    /// Get the dead ratio (dead_bytes / total_bytes)
    pub fn dead_ratio(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            self.dead_bytes as f64 / self.total_bytes as f64
        }
    }
}

/// Configuration for garbage collection
#[derive(Debug, Clone)]
pub struct GcConfig {
    /// Dead ratio threshold to trigger GC (default: 0.5 = 50%)
    pub trigger_threshold: f64,
    /// Minimum age of a segment before it can be GC'd (default: 1 hour)
    pub min_segment_age: Duration,
    /// Maximum bytes to process per GC run (default: 256MB)
    pub max_gc_bytes_per_run: u64,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            trigger_threshold: 0.5,
            min_segment_age: Duration::from_secs(3600),
            max_gc_bytes_per_run: 256 * 1024 * 1024,
        }
    }
}

/// Result of a garbage collection run
#[derive(Debug, Clone)]
pub struct GcResult {
    /// Number of segments that were collected
    pub segments_collected: usize,
    /// Total bytes reclaimed
    pub bytes_reclaimed: u64,
    /// Total entries rewritten to new segments
    pub entries_rewritten: u64,
    /// Duration of the GC run
    pub duration: Duration,
}

/// Garbage collection statistics
#[derive(Debug, Clone)]
pub struct GcStats {
    /// File ID that was garbage collected
    pub file_id: u64,
    /// Number of live values kept
    pub live_count: usize,
    /// Number of dead values removed
    pub dead_count: usize,
    /// Bytes reclaimed
    pub reclaimed_bytes: u64,
}

/// GC-related methods for ValueLog
impl ValueLog {
    /// Mark a value as dead/stale in segment stats
    ///
    /// This should be called when a key is overwritten or deleted,
    /// invalidating the old value in the vLog.
    pub fn mark_dead(&self, pointer: &super::value_log::ValuePointer) {
        if let Some(mut stats) = self.segment_stats.get_mut(&pointer.file_id) {
            stats.mark_entry_dead(pointer.length as u64);
        }
    }

    /// Get the dead ratio for a given segment (file_id)
    ///
    /// Returns dead_bytes / total_bytes, or 0.0 if the segment has no data.
    pub fn dead_ratio(&self, file_id: u64) -> f64 {
        self.segment_stats
            .get(&file_id)
            .map(|stats| stats.dead_ratio())
            .unwrap_or(0.0)
    }

    /// Get a copy of the segment stats for a given file_id
    pub fn segment_stats(&self, file_id: u64) -> Option<SegmentStats> {
        self.segment_stats.get(&file_id).map(|s| s.clone())
    }

    /// Check if GC is currently running
    pub fn is_gc_running(&self) -> bool {
        self.gc_running.load(Ordering::Relaxed)
    }

    /// Get total reclaimable bytes across all segments
    pub fn total_reclaimable_bytes(&self) -> u64 {
        self.segment_stats
            .iter()
            .map(|entry| entry.value().dead_bytes)
            .sum()
    }

    /// Collect garbage across all eligible segments
    ///
    /// Finds segments exceeding the dead ratio threshold and old enough,
    /// then rewrites live entries to new segments and deletes old ones.
    ///
    /// `is_live_fn`: Function that checks if a key is still live in the LSM-Tree.
    /// The function receives the key and should return true if the key's current
    /// value pointer still points to this entry.
    pub fn collect_garbage<F>(&self, is_live_fn: F) -> Result<GcResult>
    where
        F: Fn(&Key) -> bool,
    {
        // Set GC running flag
        if self
            .gc_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(AmateRSError::StorageIntegrity(ErrorContext::new(
                "GC is already running",
            )));
        }

        let start = Instant::now();
        let result = self.collect_garbage_inner(&is_live_fn);

        // Always clear the GC flag
        self.gc_running.store(false, Ordering::SeqCst);

        result.map(
            |(segments_collected, bytes_reclaimed, entries_rewritten)| GcResult {
                segments_collected,
                bytes_reclaimed,
                entries_rewritten,
                duration: start.elapsed(),
            },
        )
    }

    /// Inner GC logic (separated for clean flag management)
    fn collect_garbage_inner<F>(&self, is_live_fn: &F) -> Result<(usize, u64, u64)>
    where
        F: Fn(&Key) -> bool,
    {
        let current_file_id = *self.current_file_id.read();
        let threshold = self.gc_config.trigger_threshold;
        let min_age = self.gc_config.min_segment_age;
        let max_bytes = self.gc_config.max_gc_bytes_per_run;

        // Find candidate segments
        let mut candidates: Vec<(u64, f64, u64)> = Vec::new();
        for entry in self.segment_stats.iter() {
            let seg_id = *entry.key();
            let stats = entry.value();

            // Skip the active segment
            if seg_id == current_file_id {
                continue;
            }

            // Check age
            if stats.created_at.elapsed() < min_age {
                continue;
            }

            // Check dead ratio
            let ratio = stats.dead_ratio();
            if ratio >= threshold {
                candidates.push((seg_id, ratio, stats.total_bytes));
            }
        }

        // Sort by dead ratio descending (highest garbage first)
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut total_segments = 0usize;
        let mut total_bytes_reclaimed = 0u64;
        let mut total_entries_rewritten = 0u64;
        let mut bytes_processed = 0u64;

        for (seg_id, _ratio, seg_bytes) in candidates {
            if bytes_processed + seg_bytes > max_bytes {
                break;
            }

            match self.reclaim_segment(seg_id, is_live_fn) {
                Ok((reclaimed, rewritten)) => {
                    total_segments += 1;
                    total_bytes_reclaimed += reclaimed;
                    total_entries_rewritten += rewritten;
                    bytes_processed += seg_bytes;
                }
                Err(e) => {
                    // Log but continue with other segments
                    tracing::warn!("GC failed for segment {}: {}", seg_id, e);
                }
            }
        }

        Ok((
            total_segments,
            total_bytes_reclaimed,
            total_entries_rewritten,
        ))
    }

    /// Reclaim a single segment by rewriting live entries to the active segment
    ///
    /// Returns (bytes_reclaimed, entries_rewritten) on success.
    pub fn reclaim_segment<F>(&self, file_id: u64, is_live_fn: &F) -> Result<(u64, u64)>
    where
        F: Fn(&Key) -> bool,
    {
        let file_path = Self::vlog_file_path(&self.config.vlog_dir, file_id);

        // Acquire write lock on the segment to ensure no readers during deletion
        let reader_lock = self
            .segment_readers
            .entry(file_id)
            .or_insert_with(|| std::sync::Arc::new(parking_lot::RwLock::new(())))
            .clone();

        // Read all live entries first (under read lock, so concurrent reads still work)
        let (live_entries, original_size) = {
            let _read_guard = reader_lock.read();
            self.read_live_entries(file_id, is_live_fn)?
        };

        let entries_rewritten = live_entries.len() as u64;

        // Write live entries to the current active segment
        for (key, value) in &live_entries {
            self.append(key.clone(), value.clone())?;
        }
        self.flush()?;

        // Now acquire write lock to safely delete the old segment
        {
            let _write_guard = reader_lock.write();
            if file_path.exists() {
                std::fs::remove_file(&file_path).map_err(|e| {
                    AmateRSError::IoError(ErrorContext::new(format!(
                        "Failed to delete old vLog segment {}: {}",
                        file_id, e
                    )))
                })?;
            }
        }

        // Calculate reclaimed bytes
        let new_live_bytes: u64 = live_entries
            .iter()
            .map(|(k, v)| {
                // entry overhead: magic(4) + key_len(4) + key + value_len(4) + value + checksum(4)
                (16 + k.len() + v.len()) as u64
            })
            .sum();
        let bytes_reclaimed = original_size.saturating_sub(new_live_bytes);

        // Remove old segment stats and reader lock
        self.segment_stats.remove(&file_id);
        self.segment_readers.remove(&file_id);

        Ok((bytes_reclaimed, entries_rewritten))
    }

    /// Read all live entries from a segment
    fn read_live_entries<F>(
        &self,
        file_id: u64,
        is_live_fn: &F,
    ) -> Result<(Vec<(Key, CipherBlob)>, u64)>
    where
        F: Fn(&Key) -> bool,
    {
        let file_path = Self::vlog_file_path(&self.config.vlog_dir, file_id);

        let old_file = File::open(&file_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to open vLog segment {} for GC: {}",
                file_id, e
            )))
        })?;

        let file_size = old_file
            .metadata()
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to get segment {} size: {}",
                    file_id, e
                )))
            })?
            .len();

        let mut reader = BufReader::new(old_file);
        let mut offset = 0u64;
        let mut live_entries = Vec::new();

        while offset < file_size {
            match Self::read_next_entry(&mut reader) {
                Ok(Some((key, value, entry_size))) => {
                    offset += entry_size as u64;
                    if is_live_fn(&key) {
                        live_entries.push((key, value));
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(
                        "Error reading entry at offset {} in segment {}: {}",
                        offset,
                        file_id,
                        e
                    );
                    break;
                }
            }
        }

        Ok((live_entries, file_size))
    }

    /// Read the next entry from a reader, returning (key, value, entry_size) or None at EOF
    fn read_next_entry(reader: &mut BufReader<File>) -> Result<Option<(Key, CipherBlob, usize)>> {
        // Read magic
        let mut magic_bytes = [0u8; 4];
        match reader.read_exact(&mut magic_bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => {
                return Err(AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read magic: {}",
                    e
                ))));
            }
        }

        let magic = u32::from_le_bytes(magic_bytes);
        if magic != 0x564C4F47 {
            return Ok(None);
        }

        // Read key length
        let mut key_len_bytes = [0u8; 4];
        reader.read_exact(&mut key_len_bytes).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to read key length: {}",
                e
            )))
        })?;
        let key_len = u32::from_le_bytes(key_len_bytes) as usize;

        // Read key
        let mut key_bytes = vec![0u8; key_len];
        reader.read_exact(&mut key_bytes).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to read key: {}", e)))
        })?;
        let key = Key::from_slice(&key_bytes);

        // Read value length
        let mut value_len_bytes = [0u8; 4];
        reader.read_exact(&mut value_len_bytes).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to read value length: {}",
                e
            )))
        })?;
        let value_len = u32::from_le_bytes(value_len_bytes) as usize;

        // Read value
        let mut value_bytes = vec![0u8; value_len];
        reader.read_exact(&mut value_bytes).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to read value: {}", e)))
        })?;
        let value = CipherBlob::new(value_bytes);

        // Read checksum
        let mut checksum_bytes = [0u8; 4];
        reader.read_exact(&mut checksum_bytes).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to read checksum: {}", e)))
        })?;

        let entry_size = 4 + 4 + key_len + 4 + value_len + 4;

        Ok(Some((key, value, entry_size)))
    }

    /// Perform garbage collection on a vLog file
    ///
    /// Scans the file and rewrites live values to a new file, discarding dead values.
    /// This is typically called when a file has too much garbage.
    ///
    /// `is_live_fn`: Function that checks if a key is still live in the LSM-Tree
    pub fn garbage_collect_file<F>(&self, file_id: u64, is_live_fn: F) -> Result<GcStats>
    where
        F: Fn(&Key) -> bool,
    {
        let file_path = Self::vlog_file_path(&self.config.vlog_dir, file_id);

        // Open old file for reading
        let old_file = File::open(&file_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to open vLog file for GC: {}",
                e
            )))
        })?;

        let file_size = old_file
            .metadata()
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to get file size: {}", e)))
            })?
            .len();

        let mut reader = BufReader::new(old_file);
        let mut offset = 0u64;

        let mut live_values = Vec::new();
        let mut dead_count = 0usize;
        let mut live_count = 0usize;

        // Scan file and identify live values
        while offset < file_size {
            // Read entry length (magic + key_len + key + value_len + value + checksum)
            let _start_offset = offset;

            // Try to read entry
            let mut magic_bytes = [0u8; 4];
            match reader.read_exact(&mut magic_bytes) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // End of file
                    break;
                }
                Err(e) => {
                    return Err(AmateRSError::IoError(ErrorContext::new(format!(
                        "Failed to read magic: {}",
                        e
                    ))));
                }
            }

            // Verify magic
            let magic = u32::from_le_bytes(magic_bytes);
            if magic != 0x564C4F47 {
                // Corrupted entry, skip
                break;
            }

            // Read key length
            let mut key_len_bytes = [0u8; 4];
            reader.read_exact(&mut key_len_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read key length: {}",
                    e
                )))
            })?;
            let key_len = u32::from_le_bytes(key_len_bytes) as usize;

            // Read key
            let mut key_bytes = vec![0u8; key_len];
            reader.read_exact(&mut key_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read key: {}", e)))
            })?;
            let key = Key::from_slice(&key_bytes);

            // Read value length
            let mut value_len_bytes = [0u8; 4];
            reader.read_exact(&mut value_len_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read value length: {}",
                    e
                )))
            })?;
            let value_len = u32::from_le_bytes(value_len_bytes) as usize;

            // Read value
            let mut value_bytes = vec![0u8; value_len];
            reader.read_exact(&mut value_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read value: {}", e)))
            })?;
            let value = CipherBlob::new(value_bytes);

            // Read checksum
            let mut checksum_bytes = [0u8; 4];
            reader.read_exact(&mut checksum_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read checksum: {}", e)))
            })?;

            // Calculate entry size
            let entry_size = 4 + 4 + key_len + 4 + value_len + 4;
            offset += entry_size as u64;

            // Check if value is live
            if is_live_fn(&key) {
                live_values.push((key, value));
                live_count += 1;
            } else {
                dead_count += 1;
            }
        }

        // Rewrite live values to new file
        let new_file_id = Self::find_latest_vlog(&self.config)? + 1;
        let new_file_path = Self::vlog_file_path(&self.config.vlog_dir, new_file_id);

        let new_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&new_file_path)
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to create new vLog file: {}",
                    e
                )))
            })?;

        let mut new_writer = BufWriter::new(new_file);

        for (key, value) in live_values {
            let entry = VLogEntry::new(key, value);
            let entry_bytes = entry.encode();
            new_writer.write_all(&entry_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to write GC entry: {}",
                    e
                )))
            })?;
        }

        new_writer.flush().map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!("Failed to flush GC file: {}", e)))
        })?;

        // Delete old file
        std::fs::remove_file(&file_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to delete old vLog file: {}",
                e
            )))
        })?;

        Ok(GcStats {
            file_id,
            live_count,
            dead_count,
            reclaimed_bytes: file_size
                - new_writer
                    .get_ref()
                    .metadata()
                    .map_err(|e| {
                        AmateRSError::IoError(ErrorContext::new(format!(
                            "Failed to get new file size: {}",
                            e
                        )))
                    })?
                    .len(),
        })
    }

    /// Calculate garbage ratio for a vLog file
    ///
    /// Returns the ratio of dead values to total values.
    /// This can be used to determine if GC is needed.
    pub fn calculate_garbage_ratio<F>(&self, file_id: u64, is_live_fn: F) -> Result<f64>
    where
        F: Fn(&Key) -> bool,
    {
        let file_path = Self::vlog_file_path(&self.config.vlog_dir, file_id);

        let file = File::open(&file_path).map_err(|e| {
            AmateRSError::IoError(ErrorContext::new(format!(
                "Failed to open vLog file: {}",
                e
            )))
        })?;

        let file_size = file
            .metadata()
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to get file size: {}", e)))
            })?
            .len();

        let mut reader = BufReader::new(file);
        let mut offset = 0u64;

        let mut live_bytes = 0u64;
        let mut dead_bytes = 0u64;

        while offset < file_size {
            let _start_offset = offset;

            // Try to read entry
            let mut magic_bytes = [0u8; 4];
            match reader.read_exact(&mut magic_bytes) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    return Err(AmateRSError::IoError(ErrorContext::new(format!(
                        "Failed to read magic: {}",
                        e
                    ))));
                }
            }

            let magic = u32::from_le_bytes(magic_bytes);
            if magic != 0x564C4F47 {
                break;
            }

            // Read key length
            let mut key_len_bytes = [0u8; 4];
            reader.read_exact(&mut key_len_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read key length: {}",
                    e
                )))
            })?;
            let key_len = u32::from_le_bytes(key_len_bytes) as usize;

            // Read key
            let mut key_bytes = vec![0u8; key_len];
            reader.read_exact(&mut key_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read key: {}", e)))
            })?;
            let key = Key::from_slice(&key_bytes);

            // Read value length
            let mut value_len_bytes = [0u8; 4];
            reader.read_exact(&mut value_len_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to read value length: {}",
                    e
                )))
            })?;
            let value_len = u32::from_le_bytes(value_len_bytes) as usize;

            // Skip value
            let mut value_bytes = vec![0u8; value_len];
            reader.read_exact(&mut value_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read value: {}", e)))
            })?;

            // Skip checksum
            let mut checksum_bytes = [0u8; 4];
            reader.read_exact(&mut checksum_bytes).map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Failed to read checksum: {}", e)))
            })?;

            let entry_size = 4 + 4 + key_len + 4 + value_len + 4;
            offset += entry_size as u64;

            if is_live_fn(&key) {
                live_bytes += entry_size as u64;
            } else {
                dead_bytes += entry_size as u64;
            }
        }

        let total_bytes = live_bytes + dead_bytes;
        if total_bytes == 0 {
            Ok(0.0)
        } else {
            Ok(dead_bytes as f64 / total_bytes as f64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::value_log::{ValueLog, ValueLogConfig, ValuePointer};
    use std::env;
    use std::path::PathBuf;

    /// Helper to create a unique temp directory for each test
    fn make_test_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir()
            .join("amaters_vlog_gc_tests")
            .join(name)
            .join(format!("{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        // Clean any leftover files from prior runs
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                std::fs::remove_file(entry.path()).ok();
            }
        }
        dir
    }

    #[test]
    fn test_segment_stats_tracking() -> Result<()> {
        let temp_dir = make_test_dir("segment_stats");

        let vlog = ValueLog::new(&temp_dir)?;
        let file_id = vlog.current_file_id();

        // Write some entries
        let mut pointers = Vec::new();
        for i in 0..5 {
            let key = Key::from_str(&format!("stats_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 500]);
            let ptr = vlog.append(key, value)?;
            pointers.push(ptr);
        }
        vlog.flush()?;

        // Check stats
        let stats = vlog
            .segment_stats(file_id)
            .expect("stats should exist for current segment");
        assert_eq!(stats.entry_count, 5);
        assert_eq!(stats.live_count, 5);
        assert!(stats.total_bytes > 0);
        assert_eq!(stats.dead_bytes, 0);
        assert!((stats.dead_ratio() - 0.0).abs() < f64::EPSILON);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_mark_dead_and_dead_ratio() -> Result<()> {
        let temp_dir = make_test_dir("mark_dead");

        let vlog = ValueLog::new(&temp_dir)?;
        let file_id = vlog.current_file_id();

        // Write 4 entries of equal size
        let mut pointers = Vec::new();
        for i in 0..4 {
            let key = Key::from_str(&format!("dead_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 200]);
            let ptr = vlog.append(key, value)?;
            pointers.push(ptr);
        }
        vlog.flush()?;

        // Initially, dead ratio should be 0
        let ratio = vlog.dead_ratio(file_id);
        assert!((ratio - 0.0).abs() < f64::EPSILON);

        // Mark 2 of 4 entries dead
        vlog.mark_dead(&pointers[0]);
        vlog.mark_dead(&pointers[1]);

        let ratio = vlog.dead_ratio(file_id);
        // Each entry is same size, so marking 2 of 4 dead ~ 0.5
        assert!(ratio > 0.45 && ratio < 0.55, "Expected ~0.5, got {}", ratio);

        // Check stats reflect the change
        let stats = vlog.segment_stats(file_id).expect("stats should exist");
        assert_eq!(stats.live_count, 2);
        assert_eq!(stats.entry_count, 4);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_total_reclaimable_bytes() -> Result<()> {
        let temp_dir = make_test_dir("reclaimable");

        let vlog = ValueLog::new(&temp_dir)?;

        let mut pointers = Vec::new();
        for i in 0..6 {
            let key = Key::from_str(&format!("reclaim_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 300]);
            let ptr = vlog.append(key, value)?;
            pointers.push(ptr);
        }
        vlog.flush()?;

        // Initially no reclaimable bytes
        assert_eq!(vlog.total_reclaimable_bytes(), 0);

        // Mark 3 entries dead
        for ptr in &pointers[0..3] {
            vlog.mark_dead(ptr);
        }

        let reclaimable = vlog.total_reclaimable_bytes();
        assert!(
            reclaimable > 0,
            "Expected reclaimable bytes > 0, got {}",
            reclaimable
        );

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_gc_correctness() -> Result<()> {
        let temp_dir = make_test_dir("gc_correctness");

        // Use small file size so writes go to a single file, then rotate
        let config = ValueLogConfig {
            vlog_dir: temp_dir.clone(),
            max_file_size: 100_000,
            sync_on_write: true,
            ..Default::default()
        };
        let gc_config = GcConfig {
            trigger_threshold: 0.3,
            min_segment_age: Duration::from_secs(0), // No age limit for test
            max_gc_bytes_per_run: 1024 * 1024,
        };

        let vlog = ValueLog::with_config_and_gc(config, gc_config)?;

        // Write values to segment 0
        let mut pointers = Vec::new();
        let mut values = Vec::new();
        for i in 0..10 {
            let key = Key::from_str(&format!("gc_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 500]);
            let ptr = vlog.append(key, value.clone())?;
            pointers.push(ptr);
            values.push(value);
        }
        vlog.flush()?;

        let old_file_id = vlog.current_file_id();

        // Force rotation so old segment is eligible for GC
        vlog.rotate()?;

        // Mark entries 5-9 as dead
        for ptr in &pointers[5..10] {
            vlog.mark_dead(ptr);
        }

        // Run GC - entries 0-4 are "live"
        let result = vlog.collect_garbage(|key| {
            let key_str = String::from_utf8_lossy(key.as_bytes());
            if let Some(num_str) = key_str.strip_prefix("gc_key_") {
                if let Ok(num) = num_str.parse::<usize>() {
                    return num < 5;
                }
            }
            false
        })?;

        assert_eq!(result.segments_collected, 1);
        assert!(result.bytes_reclaimed > 0);
        assert_eq!(result.entries_rewritten, 5);

        // Verify old segment file is deleted
        let old_path = ValueLog::vlog_file_path(&temp_dir, old_file_id);
        assert!(
            !old_path.exists(),
            "Old segment file should have been deleted"
        );

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_gc_threshold_respected() -> Result<()> {
        let temp_dir = make_test_dir("gc_threshold");

        let config = ValueLogConfig {
            vlog_dir: temp_dir.clone(),
            max_file_size: 100_000,
            sync_on_write: true,
            ..Default::default()
        };
        let gc_config = GcConfig {
            trigger_threshold: 0.8, // High threshold
            min_segment_age: Duration::from_secs(0),
            max_gc_bytes_per_run: 1024 * 1024,
        };

        let vlog = ValueLog::with_config_and_gc(config, gc_config)?;

        // Write entries
        let mut pointers = Vec::new();
        for i in 0..10 {
            let key = Key::from_str(&format!("thresh_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 300]);
            let ptr = vlog.append(key, value)?;
            pointers.push(ptr);
        }
        vlog.flush()?;

        let old_file_id = vlog.current_file_id();
        vlog.rotate()?;

        // Mark only 3 of 10 dead (~30% dead ratio, below 80% threshold)
        for ptr in &pointers[0..3] {
            vlog.mark_dead(ptr);
        }

        // GC should not collect this segment (ratio too low)
        let result = vlog.collect_garbage(|_| true)?;
        assert_eq!(
            result.segments_collected, 0,
            "GC should not trigger below threshold"
        );

        // Verify old segment still exists
        let old_path = ValueLog::vlog_file_path(&temp_dir, old_file_id);
        assert!(old_path.exists(), "Segment should still exist");

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_gc_empty_segment() -> Result<()> {
        let temp_dir = make_test_dir("gc_empty");

        let config = ValueLogConfig {
            vlog_dir: temp_dir.clone(),
            max_file_size: 100_000,
            sync_on_write: true,
            ..Default::default()
        };
        let gc_config = GcConfig {
            trigger_threshold: 0.3,
            min_segment_age: Duration::from_secs(0),
            max_gc_bytes_per_run: 1024 * 1024,
        };

        let vlog = ValueLog::with_config_and_gc(config, gc_config)?;

        // Just rotate to create an empty old segment
        let first_id = vlog.current_file_id();
        vlog.rotate()?;

        // GC should handle empty segment gracefully (no entries, no dead ratio)
        let result = vlog.collect_garbage(|_| false)?;
        // Empty segment has 0 dead ratio, so it won't be collected
        assert_eq!(result.segments_collected, 0);

        std::fs::remove_dir_all(&temp_dir).ok();
        let _ = first_id;
        Ok(())
    }

    #[test]
    fn test_gc_all_dead_segment() -> Result<()> {
        let temp_dir = make_test_dir("gc_all_dead");

        let config = ValueLogConfig {
            vlog_dir: temp_dir.clone(),
            max_file_size: 100_000,
            sync_on_write: true,
            ..Default::default()
        };
        let gc_config = GcConfig {
            trigger_threshold: 0.5,
            min_segment_age: Duration::from_secs(0),
            max_gc_bytes_per_run: 1024 * 1024,
        };

        let vlog = ValueLog::with_config_and_gc(config, gc_config)?;

        // Write entries
        let mut pointers = Vec::new();
        for i in 0..5 {
            let key = Key::from_str(&format!("alldead_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 200]);
            let ptr = vlog.append(key, value)?;
            pointers.push(ptr);
        }
        vlog.flush()?;

        let old_file_id = vlog.current_file_id();
        vlog.rotate()?;

        // Mark ALL entries dead
        for ptr in &pointers {
            vlog.mark_dead(ptr);
        }

        // Dead ratio should be 1.0
        let ratio = vlog.dead_ratio(old_file_id);
        assert!(
            (ratio - 1.0).abs() < 0.01,
            "Expected ratio ~1.0, got {}",
            ratio
        );

        // GC should collect and rewrite 0 entries
        let result = vlog.collect_garbage(|_| false)?;
        assert_eq!(result.segments_collected, 1);
        assert_eq!(result.entries_rewritten, 0);
        assert!(result.bytes_reclaimed > 0);

        // Old segment should be deleted
        let old_path = ValueLog::vlog_file_path(&temp_dir, old_file_id);
        assert!(!old_path.exists());

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_gc_all_live_segment() -> Result<()> {
        let temp_dir = make_test_dir("gc_all_live");

        let config = ValueLogConfig {
            vlog_dir: temp_dir.clone(),
            max_file_size: 100_000,
            sync_on_write: true,
            ..Default::default()
        };
        let gc_config = GcConfig {
            trigger_threshold: 0.3,
            min_segment_age: Duration::from_secs(0),
            max_gc_bytes_per_run: 1024 * 1024,
        };

        let vlog = ValueLog::with_config_and_gc(config, gc_config)?;

        // Write entries but mark none dead
        for i in 0..5 {
            let key = Key::from_str(&format!("alllive_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 200]);
            vlog.append(key, value)?;
        }
        vlog.flush()?;
        vlog.rotate()?;

        // No entries marked dead, so dead ratio = 0, below threshold
        let result = vlog.collect_garbage(|_| true)?;
        assert_eq!(
            result.segments_collected, 0,
            "All-live segment should not be collected"
        );

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_gc_result_stats_accuracy() -> Result<()> {
        let temp_dir = make_test_dir("gc_stats_accuracy");

        let config = ValueLogConfig {
            vlog_dir: temp_dir.clone(),
            max_file_size: 100_000,
            sync_on_write: true,
            ..Default::default()
        };
        let gc_config = GcConfig {
            trigger_threshold: 0.3,
            min_segment_age: Duration::from_secs(0),
            max_gc_bytes_per_run: 1024 * 1024,
        };

        let vlog = ValueLog::with_config_and_gc(config, gc_config)?;

        let mut pointers = Vec::new();
        for i in 0..8 {
            let key = Key::from_str(&format!("acc_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 400]);
            let ptr = vlog.append(key, value)?;
            pointers.push(ptr);
        }
        vlog.flush()?;
        vlog.rotate()?;

        // Mark 6 of 8 dead (75% dead)
        for ptr in &pointers[0..6] {
            vlog.mark_dead(ptr);
        }

        let result = vlog.collect_garbage(|key| {
            let key_str = String::from_utf8_lossy(key.as_bytes());
            if let Some(num_str) = key_str.strip_prefix("acc_key_") {
                if let Ok(num) = num_str.parse::<usize>() {
                    return num >= 6;
                }
            }
            false
        })?;

        assert_eq!(result.segments_collected, 1);
        assert_eq!(result.entries_rewritten, 2);
        assert!(result.bytes_reclaimed > 0);
        assert!(result.duration.as_nanos() > 0);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_is_gc_running_flag() -> Result<()> {
        let temp_dir = make_test_dir("gc_running_flag");

        let vlog = ValueLog::new(&temp_dir)?;
        assert!(!vlog.is_gc_running());

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_concurrent_reads_during_gc() -> Result<()> {
        use std::sync::Arc;

        let temp_dir = make_test_dir("concurrent_gc");

        let config = ValueLogConfig {
            vlog_dir: temp_dir.clone(),
            max_file_size: 100_000,
            sync_on_write: true,
            ..Default::default()
        };
        let gc_config = GcConfig {
            trigger_threshold: 0.3,
            min_segment_age: Duration::from_secs(0),
            max_gc_bytes_per_run: 1024 * 1024,
        };

        let vlog = Arc::new(ValueLog::with_config_and_gc(config, gc_config)?);

        // Write entries to first segment
        let mut first_segment_pointers = Vec::new();
        for i in 0..10 {
            let key = Key::from_str(&format!("conc_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 300]);
            let ptr = vlog.append(key, value)?;
            first_segment_pointers.push(ptr);
        }
        vlog.flush()?;

        // Rotate to new segment
        vlog.rotate()?;

        // Write entries to second segment (these will be read concurrently)
        let mut second_segment_pointers = Vec::new();
        for i in 0..5 {
            let key = Key::from_str(&format!("conc2_key_{}", i));
            let value = CipherBlob::new(vec![(i + 100) as u8; 300]);
            let ptr = vlog.append(key, value)?;
            second_segment_pointers.push(ptr);
        }
        vlog.flush()?;

        // Mark first segment entries 5-9 as dead
        for ptr in &first_segment_pointers[5..10] {
            vlog.mark_dead(ptr);
        }

        // Spawn reader threads that read from the second segment
        let handles: Vec<_> = second_segment_pointers
            .iter()
            .enumerate()
            .map(|(i, ptr)| {
                let vlog_clone = Arc::clone(&vlog);
                let ptr_clone = ptr.clone();
                let expected = (i + 100) as u8;
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        let val = vlog_clone
                            .read(&ptr_clone)
                            .expect("read should succeed during GC");
                        assert_eq!(val.as_bytes()[0], expected);
                        std::thread::yield_now();
                    }
                })
            })
            .collect();

        // Run GC on first segment concurrently with reads on second
        let gc_result = vlog.collect_garbage(|key| {
            let key_str = String::from_utf8_lossy(key.as_bytes());
            if let Some(num_str) = key_str.strip_prefix("conc_key_") {
                if let Ok(num) = num_str.parse::<usize>() {
                    return num < 5;
                }
            }
            // second segment entries are always live
            true
        })?;

        // Wait for all reader threads
        for handle in handles {
            handle.join().expect("reader thread should not panic");
        }

        assert!(gc_result.segments_collected >= 1);

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_space_reclamation_preserves_live_data() -> Result<()> {
        let temp_dir = make_test_dir("reclaim_preserves");

        let config = ValueLogConfig {
            vlog_dir: temp_dir.clone(),
            max_file_size: 100_000,
            sync_on_write: true,
            ..Default::default()
        };
        let gc_config = GcConfig {
            trigger_threshold: 0.2,
            min_segment_age: Duration::from_secs(0),
            max_gc_bytes_per_run: 1024 * 1024,
        };

        let vlog = ValueLog::with_config_and_gc(config, gc_config)?;

        // Write entries
        let mut pointers = Vec::new();
        let mut expected_values = Vec::new();
        for i in 0..6 {
            let key = Key::from_str(&format!("reclaim_key_{}", i));
            let value = CipherBlob::new(vec![i as u8; 250]);
            let ptr = vlog.append(key, value.clone())?;
            pointers.push(ptr);
            expected_values.push(value);
        }
        vlog.flush()?;

        let old_file_id = vlog.current_file_id();
        vlog.rotate()?;

        // Mark entries 0, 2, 4 as dead (keep 1, 3, 5 live)
        vlog.mark_dead(&pointers[0]);
        vlog.mark_dead(&pointers[2]);
        vlog.mark_dead(&pointers[4]);

        // Reclaim the old segment
        let is_live = |key: &Key| -> bool {
            let key_str = String::from_utf8_lossy(key.as_bytes());
            if let Some(num_str) = key_str.strip_prefix("reclaim_key_") {
                if let Ok(num) = num_str.parse::<usize>() {
                    return num % 2 == 1; // odd keys are live
                }
            }
            false
        };
        let (reclaimed, rewritten) = vlog.reclaim_segment(old_file_id, &is_live)?;

        assert_eq!(rewritten, 3);
        assert!(reclaimed > 0);

        // Old segment file should be gone
        let old_path = ValueLog::vlog_file_path(&temp_dir, old_file_id);
        assert!(!old_path.exists());

        std::fs::remove_dir_all(&temp_dir).ok();
        Ok(())
    }

    #[test]
    fn test_dead_ratio_nonexistent_segment() {
        let temp_dir = make_test_dir("dead_ratio_noexist");
        let vlog = ValueLog::new(&temp_dir).expect("should create vlog");

        // Non-existent segment should return 0.0
        let ratio = vlog.dead_ratio(9999);
        assert!((ratio - 0.0).abs() < f64::EPSILON);

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_gc_config_defaults() {
        let gc = GcConfig::default();
        assert!((gc.trigger_threshold - 0.5).abs() < f64::EPSILON);
        assert_eq!(gc.min_segment_age, Duration::from_secs(3600));
        assert_eq!(gc.max_gc_bytes_per_run, 256 * 1024 * 1024);
    }

    #[test]
    fn test_segment_stats_new() {
        let stats = SegmentStats::new();
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(stats.live_bytes, 0);
        assert_eq!(stats.dead_bytes, 0);
        assert_eq!(stats.entry_count, 0);
        assert_eq!(stats.live_count, 0);
        assert!((stats.dead_ratio() - 0.0).abs() < f64::EPSILON);
    }
}
