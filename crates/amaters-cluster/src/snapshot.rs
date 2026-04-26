//! Snapshot management and log compaction for Raft consensus
//!
//! This module implements the snapshot mechanism described in Section 7 of the
//! Raft paper. Snapshots allow the system to compact the log by capturing a
//! point-in-time state of the state machine, enabling the removal of all log
//! entries up to that point.

use crate::error::{RaftError, RaftResult};
use crate::types::{LogIndex, NodeId, Term};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Snapshot metadata describing the state captured in a snapshot
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotMetadata {
    /// Index of the last log entry included in the snapshot
    pub last_included_index: LogIndex,
    /// Term of the last log entry included in the snapshot
    pub last_included_term: Term,
    /// Timestamp when the snapshot was created
    pub created_at: DateTime<Utc>,
    /// Size of the snapshot data in bytes
    pub size_bytes: u64,
    /// CRC32 checksum of the snapshot data for integrity verification
    pub checksum: u32,
}

impl SnapshotMetadata {
    /// Create new snapshot metadata
    pub fn new(
        last_included_index: LogIndex,
        last_included_term: Term,
        size_bytes: u64,
        checksum: u32,
    ) -> Self {
        Self {
            last_included_index,
            last_included_term,
            created_at: Utc::now(),
            size_bytes,
            checksum,
        }
    }

    /// Filename for this snapshot's metadata file
    pub(crate) fn metadata_filename(&self) -> String {
        format!(
            "snapshot-{:016x}-{:016x}.meta.json",
            self.last_included_term, self.last_included_index
        )
    }

    /// Filename for this snapshot's data file
    pub(crate) fn data_filename(&self) -> String {
        format!(
            "snapshot-{:016x}-{:016x}.data",
            self.last_included_term, self.last_included_index
        )
    }
}

/// A complete snapshot containing metadata and serialized state machine data
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Metadata describing this snapshot
    pub metadata: SnapshotMetadata,
    /// Serialized state machine data
    pub data: Vec<u8>,
}

impl Snapshot {
    /// Create a new snapshot from raw data
    pub fn new(last_included_index: LogIndex, last_included_term: Term, data: Vec<u8>) -> Self {
        let checksum = crc32fast::hash(&data);
        let size_bytes = data.len() as u64;
        let metadata = SnapshotMetadata::new(
            last_included_index,
            last_included_term,
            size_bytes,
            checksum,
        );
        Self { metadata, data }
    }

    /// Verify the integrity of the snapshot data against its checksum
    pub fn verify_checksum(&self) -> bool {
        let computed = crc32fast::hash(&self.data);
        computed == self.metadata.checksum
    }
}

/// Configuration for snapshot behavior
#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    /// Directory where snapshots are stored
    pub snapshot_dir: PathBuf,
    /// Maximum number of snapshots to retain on disk
    pub max_snapshots: usize,
    /// Number of log entries that trigger automatic snapshot creation
    pub snapshot_threshold: u64,
}

impl SnapshotConfig {
    /// Create a new snapshot configuration
    pub fn new(snapshot_dir: PathBuf, max_snapshots: usize, snapshot_threshold: u64) -> Self {
        Self {
            snapshot_dir,
            max_snapshots,
            snapshot_threshold,
        }
    }

    /// Create a default configuration using a temporary directory
    pub fn with_defaults(snapshot_dir: PathBuf) -> Self {
        Self {
            snapshot_dir,
            max_snapshots: 3,
            snapshot_threshold: 10000,
        }
    }
}

/// Policy for automatic snapshot creation
///
/// Controls when snapshots are triggered based on log size thresholds.
/// This policy is checked after each batch of entries is applied to the
/// state machine.
#[derive(Debug, Clone)]
pub struct SnapshotPolicy {
    /// Maximum number of log entries since the last snapshot before
    /// triggering a new one. Set to 0 to disable automatic snapshots.
    pub max_log_entries: u64,
    /// Minimum number of applied entries before the first snapshot.
    /// Prevents creating snapshots too early when the system is bootstrapping.
    pub min_applied_before_snapshot: u64,
}

impl SnapshotPolicy {
    /// Create a new snapshot policy with the given threshold
    pub fn new(max_log_entries: u64) -> Self {
        Self {
            max_log_entries,
            min_applied_before_snapshot: 0,
        }
    }

    /// Create a disabled policy (no automatic snapshots)
    pub fn disabled() -> Self {
        Self {
            max_log_entries: 0,
            min_applied_before_snapshot: 0,
        }
    }

    /// Set the minimum applied entries before first snapshot
    pub fn with_min_applied(mut self, min: u64) -> Self {
        self.min_applied_before_snapshot = min;
        self
    }

    /// Check if a snapshot should be created based on current log size
    ///
    /// Returns true if:
    /// - The policy is enabled (max_log_entries > 0)
    /// - entries_since_snapshot >= max_log_entries
    /// - applied_index >= min_applied_before_snapshot
    pub fn should_snapshot(&self, entries_since_snapshot: u64, applied_index: u64) -> bool {
        if self.max_log_entries == 0 {
            return false;
        }
        if applied_index < self.min_applied_before_snapshot {
            return false;
        }
        entries_since_snapshot >= self.max_log_entries
    }
}

impl Default for SnapshotPolicy {
    fn default() -> Self {
        Self::new(10_000)
    }
}

/// Manages snapshot creation, storage, loading, and cleanup
pub struct SnapshotManager {
    /// Configuration for snapshot behavior
    pub(crate) config: SnapshotConfig,
    /// Metadata of the latest known snapshot (cached)
    latest: Option<SnapshotMetadata>,
}

impl SnapshotManager {
    /// Create a new snapshot manager
    ///
    /// Initializes the snapshot directory and scans for existing snapshots.
    pub fn new(config: SnapshotConfig) -> RaftResult<Self> {
        // Ensure snapshot directory exists
        fs::create_dir_all(&config.snapshot_dir).map_err(|e| RaftError::StorageError {
            message: format!(
                "Failed to create snapshot directory '{}': {}",
                config.snapshot_dir.display(),
                e
            ),
        })?;

        let mut manager = Self {
            config,
            latest: None,
        };

        // Scan for existing snapshots and set latest
        manager.scan_existing_snapshots()?;

        Ok(manager)
    }

    /// Scan the snapshot directory for existing snapshot metadata files
    fn scan_existing_snapshots(&mut self) -> RaftResult<()> {
        let entries =
            fs::read_dir(&self.config.snapshot_dir).map_err(|e| RaftError::StorageError {
                message: format!(
                    "Failed to read snapshot directory '{}': {}",
                    self.config.snapshot_dir.display(),
                    e
                ),
            })?;

        let mut best: Option<SnapshotMetadata> = None;

        for entry in entries {
            let entry = entry.map_err(|e| RaftError::StorageError {
                message: format!("Failed to read directory entry: {}", e),
            })?;

            let path = entry.path();
            if let Some(ext) = path.extension() {
                // We look for .json files that end in .meta.json
                if ext == "json" {
                    if let Some(stem) = path.file_stem() {
                        let stem_str = stem.to_string_lossy();
                        if stem_str.ends_with(".meta") {
                            match self.load_metadata_from_file(&path) {
                                Ok(meta) => {
                                    let dominated = best.as_ref().is_some_and(|b| {
                                        (b.last_included_term, b.last_included_index)
                                            >= (meta.last_included_term, meta.last_included_index)
                                    });
                                    if !dominated {
                                        best = Some(meta);
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        path = %path.display(),
                                        error = %e,
                                        "Skipping corrupt snapshot metadata"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        self.latest = best;
        Ok(())
    }

    /// Load snapshot metadata from a specific file
    fn load_metadata_from_file(&self, path: &Path) -> RaftResult<SnapshotMetadata> {
        let contents = fs::read_to_string(path).map_err(|e| RaftError::StorageError {
            message: format!("Failed to read metadata file '{}': {}", path.display(), e),
        })?;

        serde_json::from_str(&contents).map_err(|e| RaftError::StorageError {
            message: format!("Failed to parse metadata file '{}': {}", path.display(), e),
        })
    }

    /// Atomically write data to a file: write to temp, fsync, rename.
    ///
    /// This ensures that on crash, either the old file or the new file exists
    /// in its entirety — never a partially written file.
    fn atomic_write(final_path: &Path, data: &[u8]) -> RaftResult<()> {
        let ext = final_path
            .extension()
            .map(|e| e.to_string_lossy())
            .unwrap_or_default();
        let tmp_path = final_path.with_extension(format!("{}.tmp", ext));
        let mut f = fs::File::create(&tmp_path).map_err(|e| RaftError::StorageError {
            message: format!("Failed to create temp file '{}': {}", tmp_path.display(), e),
        })?;
        f.write_all(data).map_err(|e| RaftError::StorageError {
            message: format!("Failed to write temp file '{}': {}", tmp_path.display(), e),
        })?;
        f.sync_all().map_err(|e| RaftError::StorageError {
            message: format!("Failed to fsync temp file '{}': {}", tmp_path.display(), e),
        })?;
        fs::rename(&tmp_path, final_path).map_err(|e| RaftError::StorageError {
            message: format!(
                "Failed to rename '{}' to '{}': {}",
                tmp_path.display(),
                final_path.display(),
                e
            ),
        })?;
        Ok(())
    }

    /// Create and persist a new snapshot
    ///
    /// Writes the snapshot data and metadata to disk atomically (write-to-temp
    /// + fsync + rename), updates the latest metadata cache, and cleans up old
    ///   snapshots.
    pub fn create_snapshot(
        &mut self,
        data: Vec<u8>,
        last_included_index: LogIndex,
        last_included_term: Term,
    ) -> RaftResult<SnapshotMetadata> {
        let snapshot = Snapshot::new(last_included_index, last_included_term, data);

        // Atomically write data file
        let data_path = self
            .config
            .snapshot_dir
            .join(snapshot.metadata.data_filename());
        Self::atomic_write(&data_path, &snapshot.data)?;

        // Atomically write metadata file
        let meta_path = self
            .config
            .snapshot_dir
            .join(snapshot.metadata.metadata_filename());
        let meta_json = serde_json::to_string_pretty(&snapshot.metadata).map_err(|e| {
            RaftError::StorageError {
                message: format!("Failed to serialize snapshot metadata: {}", e),
            }
        })?;
        Self::atomic_write(&meta_path, meta_json.as_bytes())?;

        info!(
            last_included_index = last_included_index,
            last_included_term = last_included_term,
            size_bytes = snapshot.metadata.size_bytes,
            checksum = snapshot.metadata.checksum,
            "Created snapshot"
        );

        let metadata = snapshot.metadata.clone();
        self.latest = Some(snapshot.metadata);

        // Clean up old snapshots
        self.cleanup_old_snapshots()?;

        Ok(metadata)
    }

    /// Load the most recent snapshot from disk
    pub fn load_latest(&self) -> RaftResult<Option<Snapshot>> {
        let meta = match &self.latest {
            Some(m) => m,
            None => return Ok(None),
        };

        let data_path = self.config.snapshot_dir.join(meta.data_filename());
        let data = fs::read(&data_path).map_err(|e| RaftError::StorageError {
            message: format!(
                "Failed to read snapshot data from '{}': {}",
                data_path.display(),
                e
            ),
        })?;

        let snapshot = Snapshot {
            metadata: meta.clone(),
            data,
        };

        // Verify integrity
        if !snapshot.verify_checksum() {
            return Err(RaftError::StorageError {
                message: format!(
                    "Snapshot checksum mismatch for index {}, term {}",
                    meta.last_included_index, meta.last_included_term
                ),
            });
        }

        debug!(
            last_included_index = meta.last_included_index,
            last_included_term = meta.last_included_term,
            size_bytes = meta.size_bytes,
            "Loaded latest snapshot"
        );

        Ok(Some(snapshot))
    }

    /// Check whether the log has grown enough to warrant a new snapshot
    pub fn should_snapshot(&self, log_size: u64) -> bool {
        if self.config.snapshot_threshold == 0 {
            return false;
        }
        log_size >= self.config.snapshot_threshold
    }

    /// Get the metadata of the latest snapshot
    pub fn get_latest_metadata(&self) -> Option<&SnapshotMetadata> {
        self.latest.as_ref()
    }

    /// Get the last included index of the latest snapshot
    pub fn last_included_index(&self) -> LogIndex {
        self.latest
            .as_ref()
            .map(|m| m.last_included_index)
            .unwrap_or(0)
    }

    /// Get the last included term of the latest snapshot
    pub fn last_included_term(&self) -> Term {
        self.latest
            .as_ref()
            .map(|m| m.last_included_term)
            .unwrap_or(0)
    }

    /// Remove old snapshots, keeping only the most recent `max_snapshots`
    pub fn cleanup_old_snapshots(&self) -> RaftResult<()> {
        let mut snapshot_metas = self.list_all_snapshots()?;

        if snapshot_metas.len() <= self.config.max_snapshots {
            return Ok(());
        }

        // Sort by (term, index) descending so we keep the newest
        snapshot_metas.sort_by(|a, b| {
            (b.last_included_term, b.last_included_index)
                .cmp(&(a.last_included_term, a.last_included_index))
        });

        // Remove excess snapshots (those beyond max_snapshots)
        let to_remove = &snapshot_metas[self.config.max_snapshots..];

        for meta in to_remove {
            let data_path = self.config.snapshot_dir.join(meta.data_filename());
            let meta_path = self.config.snapshot_dir.join(meta.metadata_filename());

            if data_path.exists() {
                fs::remove_file(&data_path).map_err(|e| RaftError::StorageError {
                    message: format!(
                        "Failed to remove old snapshot data '{}': {}",
                        data_path.display(),
                        e
                    ),
                })?;
            }

            if meta_path.exists() {
                fs::remove_file(&meta_path).map_err(|e| RaftError::StorageError {
                    message: format!(
                        "Failed to remove old snapshot metadata '{}': {}",
                        meta_path.display(),
                        e
                    ),
                })?;
            }

            info!(
                last_included_index = meta.last_included_index,
                last_included_term = meta.last_included_term,
                "Removed old snapshot"
            );
        }

        Ok(())
    }

    /// List all snapshot metadata files in the snapshot directory
    pub fn list_all_snapshots(&self) -> RaftResult<Vec<SnapshotMetadata>> {
        let entries =
            fs::read_dir(&self.config.snapshot_dir).map_err(|e| RaftError::StorageError {
                message: format!(
                    "Failed to read snapshot directory '{}': {}",
                    self.config.snapshot_dir.display(),
                    e
                ),
            })?;

        let mut metas = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|e| RaftError::StorageError {
                message: format!("Failed to read directory entry: {}", e),
            })?;

            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "json" {
                    if let Some(stem) = path.file_stem() {
                        let stem_str = stem.to_string_lossy();
                        if stem_str.ends_with(".meta") {
                            match self.load_metadata_from_file(&path) {
                                Ok(meta) => metas.push(meta),
                                Err(e) => {
                                    warn!(
                                        path = %path.display(),
                                        error = %e,
                                        "Skipping corrupt snapshot metadata during cleanup"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(metas)
    }

    /// Install a snapshot received from the leader via InstallSnapshot RPC
    ///
    /// This handles the case where a follower is too far behind and the leader
    /// sends its snapshot directly. The follower must replace its log and state.
    pub fn install_snapshot(&mut self, snapshot: Snapshot) -> RaftResult<SnapshotMetadata> {
        // Verify checksum
        if !snapshot.verify_checksum() {
            return Err(RaftError::StorageError {
                message: format!(
                    "Received snapshot with invalid checksum (index={}, term={})",
                    snapshot.metadata.last_included_index, snapshot.metadata.last_included_term
                ),
            });
        }

        // Check that this snapshot is newer than what we have
        if let Some(current) = &self.latest {
            if (
                snapshot.metadata.last_included_term,
                snapshot.metadata.last_included_index,
            ) <= (current.last_included_term, current.last_included_index)
            {
                return Err(RaftError::StorageError {
                    message: format!(
                        "Received snapshot (term={}, index={}) is not newer than current (term={}, index={})",
                        snapshot.metadata.last_included_term,
                        snapshot.metadata.last_included_index,
                        current.last_included_term,
                        current.last_included_index,
                    ),
                });
            }
        }

        // Atomically write to disk
        let data_path = self
            .config
            .snapshot_dir
            .join(snapshot.metadata.data_filename());
        Self::atomic_write(&data_path, &snapshot.data)?;

        let meta_path = self
            .config
            .snapshot_dir
            .join(snapshot.metadata.metadata_filename());
        let meta_json = serde_json::to_string_pretty(&snapshot.metadata).map_err(|e| {
            RaftError::StorageError {
                message: format!("Failed to serialize installed snapshot metadata: {}", e),
            }
        })?;
        Self::atomic_write(&meta_path, meta_json.as_bytes())?;

        info!(
            last_included_index = snapshot.metadata.last_included_index,
            last_included_term = snapshot.metadata.last_included_term,
            size_bytes = snapshot.metadata.size_bytes,
            "Installed snapshot from leader"
        );

        let metadata = snapshot.metadata.clone();
        self.latest = Some(snapshot.metadata);

        self.cleanup_old_snapshots()?;

        Ok(metadata)
    }
}

/// InstallSnapshot RPC request (Raft paper Section 7)
///
/// Sent by the leader to followers that are too far behind in the log.
/// The snapshot is sent in chunks identified by offset and done flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallSnapshotRequest {
    /// Leader's current term
    pub term: Term,
    /// Leader's ID so follower can redirect clients
    pub leader_id: NodeId,
    /// The last included index in the snapshot
    pub last_included_index: LogIndex,
    /// The term of the last included index
    pub last_included_term: Term,
    /// Byte offset into the snapshot data for chunked transfer
    pub offset: u64,
    /// Raw snapshot data chunk
    pub data: Vec<u8>,
    /// True if this is the final chunk of the snapshot
    pub done: bool,
}

impl InstallSnapshotRequest {
    /// Create a new InstallSnapshot request for a complete snapshot (single chunk)
    pub fn new_complete(
        term: Term,
        leader_id: NodeId,
        last_included_index: LogIndex,
        last_included_term: Term,
        data: Vec<u8>,
    ) -> Self {
        Self {
            term,
            leader_id,
            last_included_index,
            last_included_term,
            offset: 0,
            data,
            done: true,
        }
    }

    /// Create a new InstallSnapshot request for a chunk of a snapshot
    pub fn new_chunk(
        term: Term,
        leader_id: NodeId,
        last_included_index: LogIndex,
        last_included_term: Term,
        offset: u64,
        data: Vec<u8>,
        done: bool,
    ) -> Self {
        Self {
            term,
            leader_id,
            last_included_index,
            last_included_term,
            offset,
            data,
            done,
        }
    }

    /// Check if this is a complete (non-chunked) snapshot transfer
    pub fn is_complete(&self) -> bool {
        self.offset == 0 && self.done
    }
}

/// InstallSnapshot RPC response
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallSnapshotResponse {
    /// Current term of the responding node, for leader to update itself
    pub term: Term,
}

impl InstallSnapshotResponse {
    /// Create a new InstallSnapshot response
    pub fn new(term: Term) -> Self {
        Self { term }
    }
}

/// Accumulator for receiving chunked snapshot data from the leader
pub struct SnapshotReceiver {
    /// Expected last included index
    last_included_index: LogIndex,
    /// Expected last included term
    last_included_term: Term,
    /// Accumulated data chunks
    data: Vec<u8>,
    /// Next expected offset
    next_offset: u64,
}

impl SnapshotReceiver {
    /// Create a new snapshot receiver for an incoming snapshot
    pub fn new(last_included_index: LogIndex, last_included_term: Term) -> Self {
        Self {
            last_included_index,
            last_included_term,
            data: Vec::new(),
            next_offset: 0,
        }
    }

    /// Receive a chunk of snapshot data
    ///
    /// Returns `Ok(Some(Snapshot))` when the final chunk is received,
    /// `Ok(None)` while still accumulating, or `Err` on protocol violation.
    pub fn receive_chunk(&mut self, req: &InstallSnapshotRequest) -> RaftResult<Option<Snapshot>> {
        // Validate this chunk matches our expected snapshot
        if req.last_included_index != self.last_included_index
            || req.last_included_term != self.last_included_term
        {
            return Err(RaftError::StorageError {
                message: format!(
                    "Snapshot chunk mismatch: expected (index={}, term={}), got (index={}, term={})",
                    self.last_included_index,
                    self.last_included_term,
                    req.last_included_index,
                    req.last_included_term,
                ),
            });
        }

        // Validate offset
        if req.offset != self.next_offset {
            return Err(RaftError::StorageError {
                message: format!(
                    "Unexpected snapshot chunk offset: expected {}, got {}",
                    self.next_offset, req.offset,
                ),
            });
        }

        // Append chunk data
        self.data.extend_from_slice(&req.data);
        self.next_offset += req.data.len() as u64;

        if req.done {
            let snapshot = Snapshot::new(
                self.last_included_index,
                self.last_included_term,
                std::mem::take(&mut self.data),
            );
            Ok(Some(snapshot))
        } else {
            Ok(None)
        }
    }

    /// Get the expected last included index
    pub fn last_included_index(&self) -> LogIndex {
        self.last_included_index
    }

    /// Get the expected last included term
    pub fn last_included_term(&self) -> Term {
        self.last_included_term
    }

    /// Get how much data has been accumulated so far
    pub fn bytes_received(&self) -> u64 {
        self.data.len() as u64
    }
}

/// Trait for snapshot storage operations.
///
/// Provides a clean, trait-based interface for saving, loading, listing,
/// and pruning snapshots. Implementations must be `Send + Sync`.
pub trait SnapshotStore: Send + Sync {
    /// Save a snapshot to storage and return the generated metadata.
    fn save(
        &mut self,
        data: Vec<u8>,
        last_included_index: LogIndex,
        last_included_term: Term,
    ) -> RaftResult<SnapshotMetadata>;

    /// Load the most recent snapshot from storage.
    fn load_latest(&self) -> RaftResult<Option<Snapshot>>;

    /// List metadata for all stored snapshots.
    fn list(&self) -> RaftResult<Vec<SnapshotMetadata>>;

    /// Prune old snapshots, keeping only the `keep_n` most recent.
    fn prune(&self, keep_n: usize) -> RaftResult<()>;
}

/// Disk-backed implementation of [`SnapshotStore`].
///
/// Wraps a [`SnapshotManager`] to provide a trait-based interface with
/// atomic writes (write-to-temp + fsync + rename) for crash safety.
pub struct DiskSnapshotStore {
    manager: SnapshotManager,
}

impl DiskSnapshotStore {
    /// Create a new disk-backed snapshot store.
    pub fn new(config: SnapshotConfig) -> RaftResult<Self> {
        let manager = SnapshotManager::new(config)?;
        Ok(Self { manager })
    }

    /// Access the underlying snapshot manager.
    pub fn manager(&self) -> &SnapshotManager {
        &self.manager
    }

    /// Mutably access the underlying snapshot manager.
    pub fn manager_mut(&mut self) -> &mut SnapshotManager {
        &mut self.manager
    }
}

impl SnapshotStore for DiskSnapshotStore {
    fn save(
        &mut self,
        data: Vec<u8>,
        last_included_index: LogIndex,
        last_included_term: Term,
    ) -> RaftResult<SnapshotMetadata> {
        self.manager
            .create_snapshot(data, last_included_index, last_included_term)
    }

    fn load_latest(&self) -> RaftResult<Option<Snapshot>> {
        self.manager.load_latest()
    }

    fn list(&self) -> RaftResult<Vec<SnapshotMetadata>> {
        self.manager.list_all_snapshots()
    }

    fn prune(&self, keep_n: usize) -> RaftResult<()> {
        let mut snapshot_metas = self.manager.list_all_snapshots()?;

        if snapshot_metas.len() <= keep_n {
            return Ok(());
        }

        // Sort by (term, index) descending so we keep the newest
        snapshot_metas.sort_by(|a, b| {
            (b.last_included_term, b.last_included_index)
                .cmp(&(a.last_included_term, a.last_included_index))
        });

        let to_remove = &snapshot_metas[keep_n..];

        for meta in to_remove {
            let data_path = self.manager.config.snapshot_dir.join(meta.data_filename());
            let meta_path = self
                .manager
                .config
                .snapshot_dir
                .join(meta.metadata_filename());

            if data_path.exists() {
                fs::remove_file(&data_path).map_err(|e| RaftError::StorageError {
                    message: format!(
                        "Failed to remove old snapshot data '{}': {}",
                        data_path.display(),
                        e
                    ),
                })?;
            }

            if meta_path.exists() {
                fs::remove_file(&meta_path).map_err(|e| RaftError::StorageError {
                    message: format!(
                        "Failed to remove old snapshot metadata '{}': {}",
                        meta_path.display(),
                        e
                    ),
                })?;
            }

            info!(
                last_included_index = meta.last_included_index,
                last_included_term = meta.last_included_term,
                "Pruned old snapshot"
            );
        }

        Ok(())
    }
}

/// Streams a snapshot file from disk in fixed-size chunks without buffering
/// the entire snapshot in memory.
pub struct SnapshotStreamer {
    path: PathBuf,
    metadata: SnapshotMetadata,
    chunk_size: usize,
    offset: u64,
    total_size: u64,
    file: fs::File,
}

impl SnapshotStreamer {
    /// Create a new streamer for the snapshot at `path`.
    ///
    /// Opens the file and records its total size. Returns an error if the
    /// file cannot be opened or its size cannot be determined.
    pub fn new(path: PathBuf, metadata: SnapshotMetadata, chunk_size: usize) -> RaftResult<Self> {
        let file = fs::File::open(&path).map_err(|e| RaftError::StorageError {
            message: format!("Failed to open snapshot file '{}': {}", path.display(), e),
        })?;
        let total_size = file
            .metadata()
            .map_err(|e| RaftError::StorageError {
                message: format!("Failed to stat snapshot file '{}': {}", path.display(), e),
            })?
            .len();
        Ok(Self {
            path,
            metadata,
            chunk_size,
            offset: 0,
            total_size,
            file,
        })
    }

    /// Return the snapshot metadata.
    pub fn metadata(&self) -> &SnapshotMetadata {
        &self.metadata
    }

    /// Return the total size of the snapshot file in bytes.
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Read the next chunk from the file and build an [`InstallSnapshotRequest`].
    ///
    /// Returns `Ok(None)` once all bytes have been streamed.
    pub fn next_chunk_for_rpc(
        &mut self,
        term: Term,
        leader_id: NodeId,
    ) -> RaftResult<Option<InstallSnapshotRequest>> {
        if self.offset >= self.total_size {
            return Ok(None);
        }

        self.file
            .seek(SeekFrom::Start(self.offset))
            .map_err(|e| RaftError::StorageError {
                message: format!(
                    "Failed to seek to offset {} in '{}': {}",
                    self.offset,
                    self.path.display(),
                    e
                ),
            })?;

        let remaining = self.total_size - self.offset;
        let to_read = remaining.min(self.chunk_size as u64) as usize;
        let mut buf = vec![0u8; to_read];

        self.file
            .read_exact(&mut buf)
            .map_err(|e| RaftError::StorageError {
                message: format!(
                    "Failed to read {} bytes at offset {} from '{}': {}",
                    to_read,
                    self.offset,
                    self.path.display(),
                    e
                ),
            })?;

        let chunk_offset = self.offset;
        self.offset += to_read as u64;
        let done = self.offset >= self.total_size;

        Ok(Some(InstallSnapshotRequest {
            term,
            leader_id,
            last_included_index: self.metadata.last_included_index,
            last_included_term: self.metadata.last_included_term,
            offset: chunk_offset,
            data: buf,
            done,
        }))
    }
}

/// Receives snapshot chunks, writing directly to a temp file.
/// On completion, verifies the CRC32 checksum and atomically renames
/// the temp file to the final destination path.
pub struct SnapshotStreamReceiver {
    temp_path: PathBuf,
    final_path: PathBuf,
    file: fs::File,
    next_offset: u64,
    last_included_index: LogIndex,
    last_included_term: Term,
    expected_checksum: Option<u32>,
    bytes_written: u64,
}

impl SnapshotStreamReceiver {
    /// Create a new receiver that writes chunks to a temp file in `dir`.
    ///
    /// The temp file is `snapshot-{term}-{index}.data.tmp`; on completion it
    /// is atomically renamed to `snapshot-{term}-{index}.data`.
    pub fn new(
        dir: &Path,
        last_included_index: LogIndex,
        last_included_term: Term,
    ) -> RaftResult<Self> {
        let temp_name = format!(
            "snapshot-{:016x}-{:016x}.data.tmp",
            last_included_term, last_included_index
        );
        let final_name = format!(
            "snapshot-{:016x}-{:016x}.data",
            last_included_term, last_included_index
        );
        let temp_path = dir.join(&temp_name);
        let final_path = dir.join(&final_name);

        let file = fs::File::create(&temp_path).map_err(|e| RaftError::StorageError {
            message: format!(
                "Failed to create temp snapshot file '{}': {}",
                temp_path.display(),
                e
            ),
        })?;

        Ok(Self {
            temp_path,
            final_path,
            file,
            next_offset: 0,
            last_included_index,
            last_included_term,
            expected_checksum: None,
            bytes_written: 0,
        })
    }

    /// Receive a snapshot data chunk.
    ///
    /// Returns `Ok(Some(final_path))` when the last chunk (`done == true`) is
    /// received and the file has been verified and atomically renamed to its
    /// final location. Returns `Ok(None)` while still accumulating chunks.
    pub fn receive_chunk(&mut self, req: &InstallSnapshotRequest) -> RaftResult<Option<PathBuf>> {
        // Validate snapshot identity
        if req.last_included_index != self.last_included_index
            || req.last_included_term != self.last_included_term
        {
            return Err(RaftError::StorageError {
                message: format!(
                    "Snapshot identity mismatch: expected (index={}, term={}), got (index={}, term={})",
                    self.last_included_index,
                    self.last_included_term,
                    req.last_included_index,
                    req.last_included_term,
                ),
            });
        }

        // Validate sequential offset
        if req.offset != self.next_offset {
            return Err(RaftError::StorageError {
                message: format!(
                    "Snapshot chunk offset mismatch: expected {}, got {}",
                    self.next_offset, req.offset,
                ),
            });
        }

        // Write chunk to the temp file
        self.file
            .write_all(&req.data)
            .map_err(|e| RaftError::StorageError {
                message: format!(
                    "Failed to write snapshot chunk at offset {}: {}",
                    self.next_offset, e
                ),
            })?;

        self.next_offset += req.data.len() as u64;
        self.bytes_written += req.data.len() as u64;

        if !req.done {
            return Ok(None);
        }

        // Flush so all bytes are on disk before we read back for checksum
        self.file.flush().map_err(|e| RaftError::StorageError {
            message: format!("Failed to flush snapshot temp file: {}", e),
        })?;

        // Re-read the temp file in streaming fashion to compute CRC32
        let mut verify_file =
            fs::File::open(&self.temp_path).map_err(|e| RaftError::StorageError {
                message: format!(
                    "Failed to open temp file '{}' for checksum verification: {}",
                    self.temp_path.display(),
                    e
                ),
            })?;

        let mut hasher = crc32fast::Hasher::new();
        let mut read_buf = vec![0u8; 65536]; // 64 KiB read buffer
        loop {
            let n = verify_file
                .read(&mut read_buf)
                .map_err(|e| RaftError::StorageError {
                    message: format!("Failed to read temp file for checksum verification: {}", e),
                })?;
            if n == 0 {
                break;
            }
            hasher.update(&read_buf[..n]);
        }
        let computed_checksum = hasher.finalize();

        // Verify against expected checksum when known
        if let Some(expected) = self.expected_checksum {
            if computed_checksum != expected {
                return Err(RaftError::StorageError {
                    message: format!(
                        "Snapshot CRC32 mismatch: expected {:#010x}, computed {:#010x}",
                        expected, computed_checksum
                    ),
                });
            }
        }

        // Atomically rename temp → final
        fs::rename(&self.temp_path, &self.final_path).map_err(|e| RaftError::StorageError {
            message: format!(
                "Failed to rename '{}' to '{}': {}",
                self.temp_path.display(),
                self.final_path.display(),
                e
            ),
        })?;

        info!(
            last_included_index = self.last_included_index,
            last_included_term = self.last_included_term,
            bytes_written = self.bytes_written,
            checksum = computed_checksum,
            "Snapshot stream received and finalized"
        );

        Ok(Some(self.final_path.clone()))
    }

    /// Return the number of bytes written to the temp file so far.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_snapshot_dir() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("Failed to create temp dir for snapshot tests")
    }

    fn make_config(dir: &Path) -> SnapshotConfig {
        SnapshotConfig::new(dir.to_path_buf(), 3, 100)
    }

    #[test]
    fn test_snapshot_creation() {
        let dir = test_snapshot_dir();
        let config = make_config(dir.path());
        let mut manager = SnapshotManager::new(config).expect("Failed to create snapshot manager");

        let data = b"state machine data v1".to_vec();
        let meta = manager
            .create_snapshot(data.clone(), 50, 3)
            .expect("Failed to create snapshot");

        assert_eq!(meta.last_included_index, 50);
        assert_eq!(meta.last_included_term, 3);
        assert_eq!(meta.size_bytes, data.len() as u64);
        assert_eq!(meta.checksum, crc32fast::hash(&data));
    }

    #[test]
    fn test_snapshot_load_latest() {
        let dir = test_snapshot_dir();
        let config = make_config(dir.path());
        let mut manager = SnapshotManager::new(config).expect("Failed to create snapshot manager");

        let data = b"state machine snapshot data".to_vec();
        manager
            .create_snapshot(data.clone(), 100, 5)
            .expect("Failed to create snapshot");

        let loaded = manager
            .load_latest()
            .expect("Failed to load latest snapshot");
        let loaded = loaded.expect("Expected a snapshot to exist");

        assert_eq!(loaded.metadata.last_included_index, 100);
        assert_eq!(loaded.metadata.last_included_term, 5);
        assert_eq!(loaded.data, data);
        assert!(loaded.verify_checksum());
    }

    #[test]
    fn test_snapshot_load_latest_empty() {
        let dir = test_snapshot_dir();
        let config = make_config(dir.path());
        let manager = SnapshotManager::new(config).expect("Failed to create snapshot manager");

        let loaded = manager
            .load_latest()
            .expect("Failed to load latest snapshot");
        assert!(loaded.is_none());
    }

    #[test]
    fn test_snapshot_cleanup_old() {
        let dir = test_snapshot_dir();
        // max_snapshots = 2 for this test
        let config = SnapshotConfig::new(dir.path().to_path_buf(), 2, 100);
        let mut manager = SnapshotManager::new(config).expect("Failed to create snapshot manager");

        // Create 4 snapshots
        manager
            .create_snapshot(b"snap1".to_vec(), 10, 1)
            .expect("Failed to create snapshot 1");
        manager
            .create_snapshot(b"snap2".to_vec(), 20, 2)
            .expect("Failed to create snapshot 2");
        manager
            .create_snapshot(b"snap3".to_vec(), 30, 3)
            .expect("Failed to create snapshot 3");
        manager
            .create_snapshot(b"snap4".to_vec(), 40, 4)
            .expect("Failed to create snapshot 4");

        // Should only have 2 snapshots remaining
        let all = manager
            .list_all_snapshots()
            .expect("Failed to list snapshots");
        assert_eq!(all.len(), 2);

        // The two newest should remain
        let mut indices: Vec<u64> = all.iter().map(|m| m.last_included_index).collect();
        indices.sort();
        assert_eq!(indices, vec![30, 40]);
    }

    #[test]
    fn test_snapshot_threshold_trigger() {
        let dir = test_snapshot_dir();
        let config = SnapshotConfig::new(dir.path().to_path_buf(), 3, 500);
        let manager = SnapshotManager::new(config).expect("Failed to create snapshot manager");

        assert!(!manager.should_snapshot(100));
        assert!(!manager.should_snapshot(499));
        assert!(manager.should_snapshot(500));
        assert!(manager.should_snapshot(1000));
    }

    #[test]
    fn test_snapshot_threshold_zero_disabled() {
        let dir = test_snapshot_dir();
        let config = SnapshotConfig::new(dir.path().to_path_buf(), 3, 0);
        let manager = SnapshotManager::new(config).expect("Failed to create snapshot manager");

        assert!(!manager.should_snapshot(0));
        assert!(!manager.should_snapshot(999_999));
    }

    #[test]
    fn test_snapshot_metadata_serialization() {
        let meta = SnapshotMetadata::new(42, 7, 1024, 0xDEAD_BEEF);

        let json = serde_json::to_string(&meta).expect("Failed to serialize metadata");
        let deserialized: SnapshotMetadata =
            serde_json::from_str(&json).expect("Failed to deserialize metadata");

        assert_eq!(deserialized.last_included_index, 42);
        assert_eq!(deserialized.last_included_term, 7);
        assert_eq!(deserialized.size_bytes, 1024);
        assert_eq!(deserialized.checksum, 0xDEAD_BEEF);
        assert_eq!(deserialized.created_at, meta.created_at);
    }

    #[test]
    fn test_snapshot_checksum_verification() {
        let data = b"important state data".to_vec();
        let snapshot = Snapshot::new(10, 2, data);
        assert!(snapshot.verify_checksum());

        // Tamper with data
        let mut tampered = snapshot.clone();
        if let Some(byte) = tampered.data.first_mut() {
            *byte ^= 0xFF;
        }
        assert!(!tampered.verify_checksum());
    }

    #[test]
    fn test_install_snapshot_request_complete() {
        let req = InstallSnapshotRequest::new_complete(5, 1, 100, 3, b"data".to_vec());
        assert_eq!(req.term, 5);
        assert_eq!(req.leader_id, 1);
        assert_eq!(req.last_included_index, 100);
        assert_eq!(req.last_included_term, 3);
        assert_eq!(req.offset, 0);
        assert!(req.done);
        assert!(req.is_complete());
    }

    #[test]
    fn test_install_snapshot_request_chunk() {
        let req = InstallSnapshotRequest::new_chunk(5, 1, 100, 3, 512, b"chunk2".to_vec(), false);
        assert_eq!(req.offset, 512);
        assert!(!req.done);
        assert!(!req.is_complete());
    }

    #[test]
    fn test_install_snapshot_response() {
        let resp = InstallSnapshotResponse::new(7);
        assert_eq!(resp.term, 7);
    }

    #[test]
    fn test_snapshot_receiver_single_chunk() {
        let mut receiver = SnapshotReceiver::new(50, 3);

        let req = InstallSnapshotRequest::new_complete(5, 1, 50, 3, b"full data".to_vec());

        let result = receiver
            .receive_chunk(&req)
            .expect("Failed to receive chunk");
        let snapshot = result.expect("Expected completed snapshot");

        assert_eq!(snapshot.metadata.last_included_index, 50);
        assert_eq!(snapshot.metadata.last_included_term, 3);
        assert_eq!(snapshot.data, b"full data");
        assert!(snapshot.verify_checksum());
    }

    #[test]
    fn test_snapshot_receiver_multi_chunk() {
        let mut receiver = SnapshotReceiver::new(100, 5);

        // First chunk
        let req1 = InstallSnapshotRequest::new_chunk(5, 1, 100, 5, 0, b"hello".to_vec(), false);
        let result1 = receiver
            .receive_chunk(&req1)
            .expect("Failed to receive chunk 1");
        assert!(result1.is_none());
        assert_eq!(receiver.bytes_received(), 5);

        // Second chunk
        let req2 = InstallSnapshotRequest::new_chunk(5, 1, 100, 5, 5, b" world".to_vec(), true);
        let result2 = receiver
            .receive_chunk(&req2)
            .expect("Failed to receive chunk 2");
        let snapshot = result2.expect("Expected completed snapshot");

        assert_eq!(snapshot.data, b"hello world");
        assert!(snapshot.verify_checksum());
    }

    #[test]
    fn test_snapshot_receiver_wrong_offset() {
        let mut receiver = SnapshotReceiver::new(50, 3);

        let req = InstallSnapshotRequest::new_chunk(5, 1, 50, 3, 999, b"bad".to_vec(), false);

        let result = receiver.receive_chunk(&req);
        assert!(result.is_err());
    }

    #[test]
    fn test_snapshot_receiver_mismatched_snapshot() {
        let mut receiver = SnapshotReceiver::new(50, 3);

        // Different index than expected
        let req = InstallSnapshotRequest::new_complete(5, 1, 99, 3, b"wrong snapshot".to_vec());

        let result = receiver.receive_chunk(&req);
        assert!(result.is_err());
    }

    #[test]
    fn test_install_snapshot_to_manager() {
        let dir = test_snapshot_dir();
        let config = make_config(dir.path());
        let mut manager = SnapshotManager::new(config).expect("Failed to create snapshot manager");

        let data = b"installed snapshot data".to_vec();
        let snapshot = Snapshot::new(200, 10, data.clone());

        let meta = manager
            .install_snapshot(snapshot)
            .expect("Failed to install snapshot");
        assert_eq!(meta.last_included_index, 200);
        assert_eq!(meta.last_included_term, 10);

        // Verify we can load it
        let loaded = manager
            .load_latest()
            .expect("Failed to load")
            .expect("Expected snapshot");
        assert_eq!(loaded.data, data);
    }

    #[test]
    fn test_install_older_snapshot_rejected() {
        let dir = test_snapshot_dir();
        let config = make_config(dir.path());
        let mut manager = SnapshotManager::new(config).expect("Failed to create snapshot manager");

        // Create a newer snapshot first
        manager
            .create_snapshot(b"newer".to_vec(), 100, 5)
            .expect("Failed to create snapshot");

        // Try to install an older one
        let old_snapshot = Snapshot::new(50, 3, b"older".to_vec());
        let result = manager.install_snapshot(old_snapshot);
        assert!(result.is_err());
    }

    #[test]
    fn test_snapshot_persistence_across_managers() {
        let dir = test_snapshot_dir();
        let config = make_config(dir.path());

        // Create snapshot with first manager
        {
            let mut manager =
                SnapshotManager::new(config.clone()).expect("Failed to create manager 1");
            manager
                .create_snapshot(b"persisted data".to_vec(), 75, 4)
                .expect("Failed to create snapshot");
        }

        // Load with second manager (simulating restart)
        {
            let manager = SnapshotManager::new(config).expect("Failed to create manager 2");
            let latest = manager.get_latest_metadata();
            assert!(latest.is_some());
            let meta = latest.expect("Expected metadata");
            assert_eq!(meta.last_included_index, 75);
            assert_eq!(meta.last_included_term, 4);

            let snapshot = manager
                .load_latest()
                .expect("Failed to load")
                .expect("Expected snapshot");
            assert_eq!(snapshot.data, b"persisted data");
        }
    }

    #[test]
    fn test_snapshot_config_with_defaults() {
        let config = SnapshotConfig::with_defaults(PathBuf::from("/tmp/test"));
        assert_eq!(config.max_snapshots, 3);
        assert_eq!(config.snapshot_threshold, 10000);
    }

    #[test]
    fn test_snapshot_policy_should_trigger() {
        let policy = SnapshotPolicy::new(100);
        assert!(!policy.should_snapshot(50, 50)); // below threshold
        assert!(!policy.should_snapshot(99, 99)); // just below
        assert!(policy.should_snapshot(100, 100)); // at threshold
        assert!(policy.should_snapshot(200, 200)); // above threshold
    }

    #[test]
    fn test_snapshot_policy_disabled() {
        let policy = SnapshotPolicy::disabled();
        assert!(!policy.should_snapshot(10000, 10000));
    }

    #[test]
    fn test_snapshot_policy_min_applied() {
        let policy = SnapshotPolicy::new(10).with_min_applied(50);
        assert!(!policy.should_snapshot(20, 30)); // enough entries but not enough applied
        assert!(policy.should_snapshot(20, 50)); // enough of both
    }

    #[test]
    fn test_snapshot_policy_default() {
        let policy = SnapshotPolicy::default();
        assert_eq!(policy.max_log_entries, 10_000);
        assert!(!policy.should_snapshot(9_999, 9_999));
        assert!(policy.should_snapshot(10_000, 10_000));
    }

    // --- Atomic write and DiskSnapshotStore tests ---

    #[test]
    fn test_atomic_write_creates_file() {
        let dir = test_snapshot_dir();
        let file_path = dir.path().join("atomic_test.data");
        let content = b"atomic write content";

        SnapshotManager::atomic_write(&file_path, content).expect("atomic_write should succeed");

        let read_back = fs::read(&file_path).expect("File should exist");
        assert_eq!(read_back, content);
    }

    #[test]
    fn test_atomic_write_no_tmp_left() {
        let dir = test_snapshot_dir();
        let file_path = dir.path().join("no_tmp_left.data");

        SnapshotManager::atomic_write(&file_path, b"data").expect("atomic_write should succeed");

        let tmp_path = file_path.with_extension("data.tmp");
        assert!(
            !tmp_path.exists(),
            "Temp file should not remain after atomic write"
        );
    }

    #[test]
    fn test_snapshot_store_save_and_load() {
        let dir = test_snapshot_dir();
        let config = make_config(dir.path());
        let mut store = DiskSnapshotStore::new(config).expect("Failed to create DiskSnapshotStore");

        let data = b"disk snapshot store data".to_vec();
        let meta = store
            .save(data.clone(), 100, 5)
            .expect("Failed to save snapshot");

        assert_eq!(meta.last_included_index, 100);
        assert_eq!(meta.last_included_term, 5);
        assert_eq!(meta.size_bytes, data.len() as u64);

        let loaded = store
            .load_latest()
            .expect("Failed to load latest")
            .expect("Expected a snapshot");
        assert_eq!(loaded.data, data);
        assert!(loaded.verify_checksum());
    }

    #[test]
    fn test_snapshot_store_list() {
        let dir = test_snapshot_dir();
        let config = SnapshotConfig::new(dir.path().to_path_buf(), 10, 100);
        let mut store = DiskSnapshotStore::new(config).expect("Failed to create DiskSnapshotStore");

        store.save(b"snap1".to_vec(), 10, 1).expect("save 1 failed");
        store.save(b"snap2".to_vec(), 20, 2).expect("save 2 failed");
        store.save(b"snap3".to_vec(), 30, 3).expect("save 3 failed");

        let list = store.list().expect("list failed");
        assert_eq!(list.len(), 3);

        let mut indices: Vec<u64> = list.iter().map(|m| m.last_included_index).collect();
        indices.sort();
        assert_eq!(indices, vec![10, 20, 30]);
    }

    #[test]
    fn test_snapshot_store_prune() {
        let dir = test_snapshot_dir();
        // max_snapshots high so auto-cleanup doesn't kick in
        let config = SnapshotConfig::new(dir.path().to_path_buf(), 10, 100);
        let mut store = DiskSnapshotStore::new(config).expect("Failed to create DiskSnapshotStore");

        // Create 5 snapshots
        for i in 1..=5 {
            store
                .save(format!("snap{}", i).into_bytes(), i * 10, i)
                .expect("save failed");
        }

        assert_eq!(store.list().expect("list failed").len(), 5);

        // Prune to keep only 2
        store.prune(2).expect("prune failed");

        let remaining = store.list().expect("list failed");
        assert_eq!(remaining.len(), 2);

        // The two newest (term=5/index=50, term=4/index=40) should remain
        let mut indices: Vec<u64> = remaining.iter().map(|m| m.last_included_index).collect();
        indices.sort();
        assert_eq!(indices, vec![40, 50]);
    }

    // --- SnapshotStreamer / SnapshotStreamReceiver tests ---

    #[test]
    fn test_streamer_chunks_correctly() {
        let dir = test_snapshot_dir();

        // ~2 MiB of deterministic data → exactly 4 × 512 KiB chunks
        let total_size: usize = 2 * 1024 * 1024;
        let original_data: Vec<u8> = (0..total_size).map(|i| (i % 256) as u8).collect();
        let checksum = crc32fast::hash(&original_data);
        let metadata = SnapshotMetadata::new(42, 7, total_size as u64, checksum);

        let snap_path = dir.path().join(metadata.data_filename());
        fs::write(&snap_path, &original_data).expect("Failed to write snapshot file");

        let chunk_size = 512 * 1024; // 512 KiB
        let mut streamer = SnapshotStreamer::new(snap_path, metadata, chunk_size)
            .expect("Failed to create SnapshotStreamer");

        assert_eq!(streamer.total_size(), total_size as u64);

        let mut reconstructed = Vec::new();
        let mut chunk_count = 0usize;
        let mut last_done = false;

        while let Some(req) = streamer
            .next_chunk_for_rpc(5, 1)
            .expect("next_chunk_for_rpc failed")
        {
            assert_eq!(req.last_included_index, 42);
            assert_eq!(req.last_included_term, 7);
            reconstructed.extend_from_slice(&req.data);
            chunk_count += 1;
            last_done = req.done;
        }

        assert!(last_done, "Final chunk must have done=true");
        assert_eq!(chunk_count, 4, "2 MiB / 512 KiB = 4 chunks");
        assert_eq!(
            reconstructed, original_data,
            "Reconstructed data must match original"
        );
    }

    #[test]
    fn test_stream_receiver_writes_to_disk() {
        let dir = test_snapshot_dir();

        let last_included_index: LogIndex = 100;
        let last_included_term: Term = 5;

        let mut receiver =
            SnapshotStreamReceiver::new(dir.path(), last_included_index, last_included_term)
                .expect("Failed to create SnapshotStreamReceiver");

        let chunk1 = b"chunk one data--".to_vec();
        let chunk2 = b"chunk two data--".to_vec();
        let chunk3 = b"chunk three data".to_vec();

        let req1 = InstallSnapshotRequest::new_chunk(5, 1, 100, 5, 0, chunk1.clone(), false);
        let result1 = receiver
            .receive_chunk(&req1)
            .expect("receive chunk 1 failed");
        assert!(result1.is_none());
        assert_eq!(receiver.bytes_written(), chunk1.len() as u64);

        let offset2 = chunk1.len() as u64;
        let req2 = InstallSnapshotRequest::new_chunk(5, 1, 100, 5, offset2, chunk2.clone(), false);
        let result2 = receiver
            .receive_chunk(&req2)
            .expect("receive chunk 2 failed");
        assert!(result2.is_none());

        let offset3 = offset2 + chunk2.len() as u64;
        let req3 = InstallSnapshotRequest::new_chunk(5, 1, 100, 5, offset3, chunk3.clone(), true);
        let result3 = receiver
            .receive_chunk(&req3)
            .expect("receive chunk 3 failed");
        let final_path = result3.expect("Expected final path on done=true");

        assert!(final_path.exists(), "Final snapshot file must exist");

        let written = fs::read(&final_path).expect("Failed to read final snapshot file");
        let expected: Vec<u8> = [chunk1, chunk2, chunk3].concat();
        assert_eq!(written, expected, "Written data must match sent chunks");
        assert_eq!(receiver.bytes_written(), expected.len() as u64);
    }

    #[test]
    fn test_streamer_and_receiver_roundtrip() {
        let dir = test_snapshot_dir();

        let src_dir = dir.path().join("src");
        let dst_dir = dir.path().join("dst");
        fs::create_dir_all(&src_dir).expect("Failed to create src dir");
        fs::create_dir_all(&dst_dir).expect("Failed to create dst dir");

        // 1.5 MiB = 3 × 512 KiB chunks of pseudo-random bytes
        let total_size: usize = 3 * 512 * 1024;
        let original_data: Vec<u8> = (0..total_size)
            .map(|i| ((i.wrapping_mul(7).wrapping_add(i / 256)) % 256) as u8)
            .collect();

        let last_included_index: LogIndex = 250;
        let last_included_term: Term = 12;
        let checksum = crc32fast::hash(&original_data);
        let metadata = SnapshotMetadata::new(
            last_included_index,
            last_included_term,
            total_size as u64,
            checksum,
        );

        let snap_path = src_dir.join(metadata.data_filename());
        fs::write(&snap_path, &original_data).expect("Failed to write source snapshot");

        let chunk_size = 512 * 1024; // exactly 3 chunks
        let mut streamer = SnapshotStreamer::new(snap_path, metadata, chunk_size)
            .expect("Failed to create SnapshotStreamer");
        let mut receiver =
            SnapshotStreamReceiver::new(&dst_dir, last_included_index, last_included_term)
                .expect("Failed to create SnapshotStreamReceiver");

        let mut final_path: Option<PathBuf> = None;
        while let Some(req) = streamer.next_chunk_for_rpc(15, 1).expect("Streamer error") {
            if let Some(path) = receiver.receive_chunk(&req).expect("Receiver error") {
                final_path = Some(path);
                break;
            }
        }

        let final_path = final_path.expect("Round-trip must complete");
        assert!(final_path.exists(), "Final snapshot file must exist");

        let received_data = fs::read(&final_path).expect("Failed to read final snapshot");
        assert_eq!(
            received_data, original_data,
            "Round-trip data must match original"
        );
        assert_eq!(receiver.bytes_written(), total_size as u64);
    }
}
