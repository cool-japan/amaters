//! Snapshot creation and restore for server-level state.
//!
//! Snapshots are compressed with LZ4 (via oxiarc-lz4) and stored as flat
//! binary files under a configurable directory.  Each snapshot file is named
//! `snapshot-{id:020}.bin` so that lexicographic directory listing already
//! yields the snapshots in chronological order.
//!
//! A fast FNV-64 checksum is embedded in the header of every file so that a
//! corrupted file can be detected at read time before the decompressed bytes
//! are handed to the caller.

use std::io::{Read as IoRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::server::{ServerError, ServerResult};

// ─── Header constants ─────────────────────────────────────────────────────────

/// Magic bytes written at the start of every snapshot file.
const MAGIC: &[u8; 8] = b"AMSNAP\x01\x00";
/// Fixed header length in bytes:
/// - 8 B  magic
/// - 8 B  snapshot id (little-endian u64)
/// - 8 B  timestamp_ms (little-endian u64)
/// - 8 B  original (uncompressed) size (little-endian u64)
/// - 8 B  FNV-64 checksum of the *compressed* payload
const HEADER_LEN: usize = 40;

// ─── FNV-64 ──────────────────────────────────────────────────────────────────

const FNV_PRIME: u64 = 0x00000100_000001B3;
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce4_84222325;

fn fnv64(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in data {
        hash = hash.wrapping_mul(FNV_PRIME);
        hash ^= u64::from(*byte);
    }
    hash
}

// ─── Public types ─────────────────────────────────────────────────────────────

/// Metadata describing a snapshot stored on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMeta {
    /// Monotonically increasing identifier (caller-assigned).
    pub id: u64,
    /// Unix timestamp in milliseconds at which the snapshot was written.
    pub timestamp_ms: u64,
    /// Size of the *compressed* bytes stored in the file (excluding header).
    pub size_bytes: u64,
    /// FNV-64 checksum of the compressed payload.
    pub checksum: u64,
}

/// Manages a directory of numbered snapshot files.
///
/// All operations are synchronous (no async I/O) to keep things simple and
/// testable.  In production the caller is responsible for spawning these calls
/// onto a blocking thread via `tokio::task::spawn_blocking`.
pub struct SnapshotManager {
    snapshot_dir: PathBuf,
    uploader: Option<Arc<dyn SnapshotUploader>>,
}

impl SnapshotManager {
    /// Create a [`SnapshotManager`] that stores files in `snapshot_dir`.
    ///
    /// The directory is created if it does not yet exist.
    pub fn new(snapshot_dir: impl Into<PathBuf>) -> ServerResult<Self> {
        let dir: PathBuf = snapshot_dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            snapshot_dir: dir,
            uploader: None,
        })
    }

    /// Compress `data` with LZ4 and write it to disk as snapshot `id`.
    ///
    /// Returns [`SnapshotMeta`] describing the written file.
    pub fn write_snapshot(&self, id: u64, data: &[u8]) -> ServerResult<SnapshotMeta> {
        let compressed = oxiarc_lz4::compress(data)
            .map_err(|e| ServerError::Storage(format!("Snapshot LZ4 compress failed: {}", e)))?;

        let timestamp_ms = current_timestamp_ms();
        let checksum = fnv64(&compressed);
        let size_bytes = compressed.len() as u64;

        let path = self.snapshot_path(id);
        let mut file = std::fs::File::create(&path)?;

        // Write header
        let mut header = [0u8; HEADER_LEN];
        header[0..8].copy_from_slice(MAGIC);
        header[8..16].copy_from_slice(&id.to_le_bytes());
        header[16..24].copy_from_slice(&timestamp_ms.to_le_bytes());
        header[24..32].copy_from_slice(&(data.len() as u64).to_le_bytes());
        header[32..40].copy_from_slice(&checksum.to_le_bytes());

        file.write_all(&header)?;
        file.write_all(&compressed)?;
        file.sync_all()?;

        Ok(SnapshotMeta {
            id,
            timestamp_ms,
            size_bytes,
            checksum,
        })
    }

    /// Read back the snapshot with the given `id` and decompress it.
    ///
    /// Returns an error if the file is missing, the header is corrupted, the
    /// checksum does not match, or LZ4 decompression fails.
    pub fn read_snapshot(&self, id: u64) -> ServerResult<Vec<u8>> {
        let path = self.snapshot_path(id);
        let mut file = std::fs::File::open(&path)
            .map_err(|e| ServerError::Storage(format!("Cannot open snapshot {}: {}", id, e)))?;

        // Read and validate header
        let mut header = [0u8; HEADER_LEN];
        file.read_exact(&mut header).map_err(|e| {
            ServerError::Storage(format!("Snapshot {} header read error: {}", id, e))
        })?;

        if &header[0..8] != MAGIC {
            return Err(ServerError::Storage(format!(
                "Snapshot {} has invalid magic bytes",
                id
            )));
        }

        let stored_id = u64::from_le_bytes(header[8..16].try_into().unwrap_or_default());
        if stored_id != id {
            return Err(ServerError::Storage(format!(
                "Snapshot file id mismatch: expected {}, got {}",
                id, stored_id
            )));
        }

        let original_size =
            u64::from_le_bytes(header[24..32].try_into().unwrap_or_default()) as usize;
        let expected_checksum = u64::from_le_bytes(header[32..40].try_into().unwrap_or_default());

        // Read compressed payload
        let mut compressed = Vec::new();
        file.read_to_end(&mut compressed).map_err(|e| {
            ServerError::Storage(format!("Snapshot {} payload read error: {}", id, e))
        })?;

        // Verify checksum before decompressing
        let actual_checksum = fnv64(&compressed);
        if actual_checksum != expected_checksum {
            return Err(ServerError::Storage(format!(
                "Snapshot {} checksum mismatch: expected {:016x}, got {:016x}",
                id, expected_checksum, actual_checksum
            )));
        }

        // Decompress
        let data = oxiarc_lz4::decompress(&compressed, original_size).map_err(|e| {
            ServerError::Storage(format!("Snapshot {} LZ4 decompress failed: {}", id, e))
        })?;

        Ok(data)
    }

    /// List all available snapshots, sorted by ID descending (newest first).
    pub fn list_snapshots(&self) -> ServerResult<Vec<SnapshotMeta>> {
        let entries = std::fs::read_dir(&self.snapshot_dir)?;

        let mut metas = Vec::new();
        for entry in entries {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            // Only process files matching our naming convention
            if !name.starts_with("snapshot-") || !name.ends_with(".bin") {
                continue;
            }

            let meta = self.read_meta_from_path(&entry.path())?;
            metas.push(meta);
        }

        // Sort descending by id (newest first)
        metas.sort_by_key(|m| std::cmp::Reverse(m.id));
        Ok(metas)
    }

    /// Delete the snapshot file for `id`.
    ///
    /// Returns an error if the file does not exist.
    pub fn delete_snapshot(&self, id: u64) -> ServerResult<()> {
        let path = self.snapshot_path(id);
        std::fs::remove_file(&path)
            .map_err(|e| ServerError::Storage(format!("Failed to delete snapshot {}: {}", id, e)))
    }

    /// Build the canonical path for a given snapshot `id`.
    fn snapshot_path(&self, id: u64) -> PathBuf {
        self.snapshot_dir.join(format!("snapshot-{:020}.bin", id))
    }

    /// Read only the header bytes from an on-disk file to populate
    /// [`SnapshotMeta`] without decompressing the payload.
    fn read_meta_from_path(&self, path: &Path) -> ServerResult<SnapshotMeta> {
        let mut file = std::fs::File::open(path).map_err(|e| {
            ServerError::Storage(format!(
                "Cannot open snapshot file {}: {}",
                path.display(),
                e
            ))
        })?;

        let mut header = [0u8; HEADER_LEN];
        file.read_exact(&mut header).map_err(|e| {
            ServerError::Storage(format!("Cannot read header from {}: {}", path.display(), e))
        })?;

        if &header[0..8] != MAGIC {
            return Err(ServerError::Storage(format!(
                "Invalid magic in {}",
                path.display()
            )));
        }

        let id = u64::from_le_bytes(header[8..16].try_into().unwrap_or_default());
        let timestamp_ms = u64::from_le_bytes(header[16..24].try_into().unwrap_or_default());
        let checksum = u64::from_le_bytes(header[32..40].try_into().unwrap_or_default());

        // Size = file size - header length
        let file_len = std::fs::metadata(path)?.len();
        let size_bytes = file_len.saturating_sub(HEADER_LEN as u64);

        Ok(SnapshotMeta {
            id,
            timestamp_ms,
            size_bytes,
            checksum,
        })
    }
}

/// Returns current Unix time in milliseconds, falling back to 0 on error.
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ─── Remote snapshot upload trait ────────────────────────────────────────────

/// Trait for uploading snapshots to remote object storage.
///
/// The `LocalSnapshotUploader` is a reference implementation suitable for
/// testing and single-node deployments.  An S3-backed implementation would
/// live behind a feature flag and satisfy the same interface.
pub trait SnapshotUploader: Send + Sync {
    /// Upload raw `data` as snapshot `id` and return the remote URI.
    fn upload_snapshot(&self, id: u64, data: &[u8]) -> ServerResult<String>;
    /// Download a snapshot previously uploaded to `uri`.
    fn download_snapshot(&self, uri: &str) -> ServerResult<Vec<u8>>;
    /// List all snapshots in remote storage, sorted by id ascending.
    fn list_remote_snapshots(&self) -> ServerResult<Vec<(u64, String)>>;
}

/// Local filesystem implementation of [`SnapshotUploader`].
///
/// URIs produced by this uploader have the form `local://<absolute_path>`.
pub struct LocalSnapshotUploader {
    base_path: PathBuf,
}

impl LocalSnapshotUploader {
    /// Create a new uploader that stores remote snapshots under `base_path`.
    ///
    /// The directory is created if it does not already exist.
    pub fn new(base_path: impl Into<PathBuf>) -> ServerResult<Self> {
        let base_path = base_path.into();
        std::fs::create_dir_all(&base_path)?;
        Ok(Self { base_path })
    }

    /// Parse the snapshot id from a remote filename such as
    /// `remote-snapshot-{id:020}.bin`.
    fn parse_id_from_name(name: &str) -> Option<u64> {
        let stem = name
            .strip_prefix("remote-snapshot-")?
            .strip_suffix(".bin")?;
        stem.parse::<u64>().ok()
    }
}

impl SnapshotUploader for LocalSnapshotUploader {
    fn upload_snapshot(&self, id: u64, data: &[u8]) -> ServerResult<String> {
        let path = self
            .base_path
            .join(format!("remote-snapshot-{:020}.bin", id));
        std::fs::write(&path, data)?;
        Ok(format!("local://{}", path.display()))
    }

    fn download_snapshot(&self, uri: &str) -> ServerResult<Vec<u8>> {
        let path_str = uri
            .strip_prefix("local://")
            .ok_or_else(|| ServerError::Storage(format!("Unsupported URI scheme in '{}'", uri)))?;
        std::fs::read(path_str)
            .map_err(|e| ServerError::Storage(format!("Failed to download '{}': {}", uri, e)))
    }

    fn list_remote_snapshots(&self) -> ServerResult<Vec<(u64, String)>> {
        let entries = std::fs::read_dir(&self.base_path)?;
        let mut snapshots = Vec::new();
        for entry in entries {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if let Some(id) = Self::parse_id_from_name(&name) {
                let uri = format!("local://{}", entry.path().display());
                snapshots.push((id, uri));
            }
        }
        snapshots.sort_by_key(|(id, _)| *id);
        Ok(snapshots)
    }
}

impl SnapshotManager {
    /// Attach an uploader to this manager.
    pub fn set_uploader(&mut self, uploader: Arc<dyn SnapshotUploader>) {
        self.uploader = Some(uploader);
    }

    /// Upload the local snapshot with `id` via the configured uploader.
    ///
    /// Returns the remote URI on success, or an error if no uploader is set
    /// or the snapshot does not exist locally.
    pub fn upload(&self, id: u64) -> ServerResult<String> {
        let uploader = self.uploader.as_ref().ok_or_else(|| {
            ServerError::Storage("No uploader configured on SnapshotManager".to_string())
        })?;
        let data = self.read_snapshot(id)?;
        uploader.upload_snapshot(id, &data)
    }

    /// Download a snapshot from `uri` and store it locally under `local_id`.
    ///
    /// Returns the [`SnapshotMeta`] of the newly written local snapshot.
    pub fn restore_from_remote(&self, uri: &str, local_id: u64) -> ServerResult<SnapshotMeta> {
        let uploader = self.uploader.as_ref().ok_or_else(|| {
            ServerError::Storage("No uploader configured on SnapshotManager".to_string())
        })?;
        let data = uploader.download_snapshot(uri)?;
        self.write_snapshot(local_id, &data)
    }
}

// ─── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("amaters_snapshot_test_{}", suffix));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn test_write_and_read_snapshot() {
        let dir = temp_dir("write_read");
        let sm = SnapshotManager::new(&dir).expect("create manager");

        // 1 MiB of test data
        let payload: Vec<u8> = (0u8..=255).cycle().take(1024 * 1024).collect();
        let meta = sm.write_snapshot(42, &payload).expect("write");
        assert_eq!(meta.id, 42);
        assert!(meta.size_bytes > 0);

        let restored = sm.read_snapshot(42).expect("read");
        assert_eq!(restored, payload);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_list_snapshots_sorted() {
        let dir = temp_dir("list_sorted");
        let sm = SnapshotManager::new(&dir).expect("create manager");

        sm.write_snapshot(10, b"alpha").expect("write 10");
        sm.write_snapshot(30, b"gamma").expect("write 30");
        sm.write_snapshot(20, b"beta").expect("write 20");

        let list = sm.list_snapshots().expect("list");
        assert_eq!(list.len(), 3);
        // Newest (highest id) first
        assert_eq!(list[0].id, 30);
        assert_eq!(list[1].id, 20);
        assert_eq!(list[2].id, 10);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_delete_snapshot() {
        let dir = temp_dir("delete");
        let sm = SnapshotManager::new(&dir).expect("create manager");

        sm.write_snapshot(1, b"to be deleted").expect("write");
        sm.write_snapshot(2, b"to be kept").expect("write");

        sm.delete_snapshot(1).expect("delete");

        let list = sm.list_snapshots().expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, 2);

        // Attempting to read the deleted snapshot should fail
        assert!(sm.read_snapshot(1).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_snapshot_checksum_corruption_detected() {
        let dir = temp_dir("checksum");
        let sm = SnapshotManager::new(&dir).expect("create manager");

        sm.write_snapshot(99, b"important data").expect("write");

        // Corrupt the payload bytes of the snapshot file
        let path = dir.join("snapshot-00000000000000000099.bin");
        let mut contents = std::fs::read(&path).expect("read file");
        // Flip a byte well past the header
        if contents.len() > HEADER_LEN + 4 {
            contents[HEADER_LEN + 4] ^= 0xFF;
        }
        std::fs::write(&path, &contents).expect("write corrupted file");

        let result = sm.read_snapshot(99);
        assert!(
            result.is_err(),
            "Should detect checksum mismatch after corruption"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_fnv64_deterministic() {
        let a = fnv64(b"hello");
        let b = fnv64(b"hello");
        assert_eq!(a, b);
        assert_ne!(fnv64(b"hello"), fnv64(b"world"));
    }

    #[test]
    fn test_local_uploader_round_trip() {
        let local_dir = temp_dir("uploader_local");
        let remote_dir = temp_dir("uploader_remote");

        let mut sm = SnapshotManager::new(&local_dir).expect("create manager");
        let uploader = LocalSnapshotUploader::new(&remote_dir).expect("create uploader");
        sm.set_uploader(std::sync::Arc::new(uploader));

        let payload = b"round-trip test data 1234567890";
        sm.write_snapshot(777, payload)
            .expect("write local snapshot");

        let uri = sm.upload(777).expect("upload");
        assert!(uri.starts_with("local://"), "URI: {}", uri);

        // Restore into a fresh manager pointing at same local dir
        let meta = sm
            .restore_from_remote(&uri, 888)
            .expect("restore from remote");
        assert_eq!(meta.id, 888);

        let restored = sm.read_snapshot(888).expect("read restored");
        assert_eq!(restored.as_slice(), payload.as_ref());

        std::fs::remove_dir_all(&local_dir).ok();
        std::fs::remove_dir_all(&remote_dir).ok();
    }

    #[test]
    fn test_upload_requires_uploader_set() {
        let dir = temp_dir("no_uploader");
        let sm = SnapshotManager::new(&dir).expect("create manager");
        sm.write_snapshot(1, b"data").expect("write");
        let result = sm.upload(1);
        assert!(result.is_err(), "upload without uploader should fail");
        std::fs::remove_dir_all(&dir).ok();
    }
}
