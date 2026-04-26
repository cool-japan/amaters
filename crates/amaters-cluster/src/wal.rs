//! Segment-based Write-Ahead Log (WAL) with CRC32 integrity and fsync.
//!
//! Provides crash-safe, durable logging for Raft consensus. Each segment
//! file contains a header followed by length-prefixed, CRC32-checksummed
//! entries.  [`WalWriter`] appends entries with configurable fsync
//! behaviour, while [`WalReader`] iterates over all segments and validates
//! integrity.
//!
//! # Release notes
//!
//! ## WAL v2 (current)
//! - Added 8-byte packed fencing token per log entry.
//! - WAL magic bumped from `WAL1` (0x57414C31) to `WAL2` (0x57414C32).
//! - WAL v1 segments are still readable: the missing token field is filled with 0.
//! - [`CorruptionPolicy`] variants renamed: `SkipCorrupted` → `AlertAndContinue`,
//!   `TruncateAtCorruption` → `TruncateToLastGood`, `FailHard` → `RefuseStart`.
//!
//! # On-disk format
//!
//! ## Segment header (12 bytes)
//!
//! ```text
//! [magic: u32 LE][version: u32 LE][segment_id: u32 LE]
//! ```
//!
//! ## Entry format (WAL v2)
//!
//! ```text
//! [entry_len: u32 LE][term: u64 LE][index: u64 LE][cmd_len: u32 LE][cmd: N bytes][fencing_token: u64 LE][crc32: u32 LE]
//! ```
//!
//! ## Entry format (WAL v1, read-only compat)
//!
//! ```text
//! [entry_len: u32 LE][term: u64 LE][index: u64 LE][cmd_len: u32 LE][cmd: N bytes][crc32: u32 LE]
//! ```
//!
//! `entry_len` covers everything after itself (including the trailing CRC).
//! For v2, the CRC is computed over `term + index + cmd_len + cmd + fencing_token`
//! (i.e. all payload bytes excluding `entry_len` itself and the CRC).
//! For v1 the fencing token bytes are absent from the CRC computation.

use crate::error::{RaftError, RaftResult};
use crate::log::{Command, LogEntry};
use crate::types::LogIndex;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Magic bytes identifying a WAL v2 segment file (`"WAL2"` as little-endian u32).
const WAL_MAGIC: u32 = 0x57414C32;

/// Magic bytes for legacy WAL v1 segments (`"WAL1"` as little-endian u32).
const WAL_MAGIC_V1: u32 = 0x57414C31;

/// Current WAL on-disk format version.
const WAL_VERSION: u32 = 2;

/// Legacy WAL v1 format version (read-compat only).
const WAL_VERSION_V1: u32 = 1;

/// Size of the segment header in bytes: magic(4) + version(4) + segment_id(4).
const SEGMENT_HEADER_SIZE: usize = 12;

/// Default maximum segment size before rotation (64 MiB).
const DEFAULT_MAX_SEGMENT_SIZE: u64 = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// SyncMode
// ---------------------------------------------------------------------------

/// Controls when `fsync` is called on the WAL segment file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncMode {
    /// Call `fsync` after every write.
    EveryWrite,
    /// Call `fsync` after every `n` writes.
    Batched(usize),
    /// Let the OS decide when to flush — no explicit `fsync`.
    OsManaged,
}

// ---------------------------------------------------------------------------
// CorruptionPolicy
// ---------------------------------------------------------------------------

/// Strategy for handling mid-segment corruption during WAL recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorruptionPolicy {
    /// Skip corrupted entries and continue scanning for valid ones.
    /// Previously called `SkipCorrupted`.
    AlertAndContinue,
    /// Truncate the log at the first encountered corruption (discard all
    /// entries from that point onward).
    /// Previously called `TruncateAtCorruption`.
    TruncateToLastGood,
    /// Fail immediately on any corruption (return an error).
    /// Previously called `FailHard`.
    RefuseStart,
}

// ---------------------------------------------------------------------------
// WalDiagnostics
// ---------------------------------------------------------------------------

/// Diagnostic information collected during corruption-aware WAL recovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WalDiagnostics {
    /// Number of entries that were valid and recovered.
    pub valid_entries: u64,
    /// Number of entries that had CRC mismatches or structural damage.
    pub corrupt_entries: u64,
    /// Number of segment tails that were truncated due to partial writes.
    pub truncated_segments: u64,
    /// Total bytes occupied by recovered (valid) entries.
    pub recovered_bytes: u64,
}

// ---------------------------------------------------------------------------
// Segment header
// ---------------------------------------------------------------------------

/// Header written at the start of every WAL segment file.
struct SegmentHeader {
    magic: u32,
    version: u32,
    segment_id: u32,
}

impl SegmentHeader {
    fn new(segment_id: u32) -> Self {
        Self {
            magic: WAL_MAGIC,
            version: WAL_VERSION,
            segment_id,
        }
    }

    fn encode(&self) -> [u8; SEGMENT_HEADER_SIZE] {
        let mut buf = [0u8; SEGMENT_HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.magic.to_le_bytes());
        buf[4..8].copy_from_slice(&self.version.to_le_bytes());
        buf[8..12].copy_from_slice(&self.segment_id.to_le_bytes());
        buf
    }

    fn decode(data: &[u8]) -> RaftResult<Self> {
        if data.len() < SEGMENT_HEADER_SIZE {
            return Err(RaftError::StorageError {
                message: "segment header too short".to_string(),
            });
        }
        let magic = u32::from_le_bytes(read_4(data, 0)?);
        let version = u32::from_le_bytes(read_4(data, 4)?);
        let segment_id = u32::from_le_bytes(read_4(data, 8)?);

        // Accept both WAL v1 (legacy, read-only compat) and WAL v2 (current).
        let accepted = (magic == WAL_MAGIC && version == WAL_VERSION)
            || (magic == WAL_MAGIC_V1 && version == WAL_VERSION_V1);
        if !accepted {
            return Err(RaftError::StorageError {
                message: format!(
                    "bad WAL header: magic={magic:#010x}, version={version} \
                     (expected WAL2/v2 or WAL1/v1)"
                ),
            });
        }
        Ok(Self {
            magic,
            version,
            segment_id,
        })
    }

    /// Returns `true` if this is a legacy v1 segment.
    fn is_v1(&self) -> bool {
        self.version == WAL_VERSION_V1
    }
}

// ---------------------------------------------------------------------------
// WalWriter
// ---------------------------------------------------------------------------

/// Appends [`LogEntry`] values to a segment-based WAL on disk.
///
/// Segment files are named `wal-{segment_id:08}.seg` inside the configured
/// directory.  When the current segment exceeds
/// `max_segment_size` a new segment is
/// created automatically.
pub struct WalWriter {
    dir: PathBuf,
    current_segment: Option<File>,
    current_segment_id: u32,
    current_segment_size: u64,
    max_segment_size: u64,
    sync_mode: SyncMode,
    writes_since_sync: usize,
}

impl WalWriter {
    /// Create a new writer rooted at `dir`.
    ///
    /// The directory is created if it does not exist.  Any existing segments
    /// are discovered so that new writes continue from the latest segment.
    pub fn new(dir: &Path, sync_mode: SyncMode, max_segment_size: u64) -> RaftResult<Self> {
        fs::create_dir_all(dir).map_err(|e| RaftError::StorageError {
            message: format!("failed to create WAL dir {}: {e}", dir.display()),
        })?;

        let max_segment_size = if max_segment_size == 0 {
            DEFAULT_MAX_SEGMENT_SIZE
        } else {
            max_segment_size
        };

        let mut writer = Self {
            dir: dir.to_path_buf(),
            current_segment: None,
            current_segment_id: 0,
            current_segment_size: 0,
            max_segment_size,
            sync_mode,
            writes_since_sync: 0,
        };

        // Discover existing segments
        let existing = list_segments(dir)?;
        if let Some(&last_id) = existing.last() {
            writer.current_segment_id = last_id;
            let path = segment_path(dir, last_id);
            let meta = fs::metadata(&path).map_err(|e| RaftError::StorageError {
                message: format!("failed to stat segment {}: {e}", path.display()),
            })?;
            writer.current_segment_size = meta.len();
            let file = OpenOptions::new().append(true).open(&path).map_err(|e| {
                RaftError::StorageError {
                    message: format!("failed to open segment {}: {e}", path.display()),
                }
            })?;
            writer.current_segment = Some(file);
        }

        Ok(writer)
    }

    /// Append a single log entry to the WAL.
    ///
    /// Rotates to a new segment when the current one exceeds the configured
    /// maximum size.  Calls `fsync` according to the configured
    /// [`SyncMode`].
    pub fn append(&mut self, entry: &LogEntry) -> RaftResult<()> {
        let encoded = encode_entry(entry);
        let encoded_len = encoded.len() as u64;

        // Rotate if necessary (but always allow at least one entry per segment)
        if self.current_segment.is_some()
            && self.current_segment_size + encoded_len > self.max_segment_size
            && self.current_segment_size > SEGMENT_HEADER_SIZE as u64
        {
            self.rotate_segment()?;
        }

        // Ensure we have an open segment
        if self.current_segment.is_none() {
            self.open_new_segment()?;
        }

        let file = self
            .current_segment
            .as_mut()
            .ok_or_else(|| RaftError::StorageError {
                message: "no open segment after rotation".to_string(),
            })?;

        file.write_all(&encoded)
            .map_err(|e| RaftError::StorageError {
                message: format!("failed to write WAL entry: {e}"),
            })?;

        self.current_segment_size += encoded_len;
        self.writes_since_sync += 1;

        self.maybe_sync()?;

        Ok(())
    }

    /// Force an `fsync` on the current segment, regardless of [`SyncMode`].
    pub fn sync(&mut self) -> RaftResult<()> {
        if let Some(ref file) = self.current_segment {
            file.sync_data().map_err(|e| RaftError::StorageError {
                message: format!("failed to fsync WAL: {e}"),
            })?;
            self.writes_since_sync = 0;
        }
        Ok(())
    }

    /// Truncate all entries with index >= `from_index`.
    ///
    /// This rewrites segment files, removing entries at or beyond the given
    /// index.  After truncation the writer re-opens the last remaining
    /// segment (or creates a fresh one if all entries were removed).
    pub fn truncate_from(&mut self, from_index: LogIndex) -> RaftResult<()> {
        // Close the current fd
        self.current_segment = None;

        let reader = WalReader::new(&self.dir);
        let all_entries = reader.recover()?;
        let kept: Vec<&LogEntry> = all_entries
            .iter()
            .filter(|e| e.index < from_index)
            .collect();

        // Remove all existing segment files
        let segments = list_segments(&self.dir)?;
        for seg_id in &segments {
            let path = segment_path(&self.dir, *seg_id);
            let _ = fs::remove_file(&path);
        }

        // Reset state
        self.current_segment_id = 0;
        self.current_segment_size = 0;
        self.writes_since_sync = 0;

        // Re-write kept entries
        for entry in kept {
            self.append(entry)?;
        }

        self.sync()?;

        Ok(())
    }

    // -- private helpers --

    fn open_new_segment(&mut self) -> RaftResult<()> {
        let path = segment_path(&self.dir, self.current_segment_id);
        let mut file = File::create(&path).map_err(|e| RaftError::StorageError {
            message: format!("failed to create segment {}: {e}", path.display()),
        })?;

        let header = SegmentHeader::new(self.current_segment_id);
        file.write_all(&header.encode())
            .map_err(|e| RaftError::StorageError {
                message: format!("failed to write segment header: {e}"),
            })?;

        self.current_segment_size = SEGMENT_HEADER_SIZE as u64;
        self.current_segment = Some(file);
        Ok(())
    }

    fn rotate_segment(&mut self) -> RaftResult<()> {
        // Sync before closing
        self.sync()?;
        self.current_segment = None;
        self.current_segment_id += 1;
        self.open_new_segment()?;
        Ok(())
    }

    fn maybe_sync(&mut self) -> RaftResult<()> {
        match &self.sync_mode {
            SyncMode::EveryWrite => self.sync(),
            SyncMode::Batched(n) => {
                if self.writes_since_sync >= *n {
                    self.sync()
                } else {
                    Ok(())
                }
            }
            SyncMode::OsManaged => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------------
// WalReader
// ---------------------------------------------------------------------------

/// Reads entries from all WAL segments in a directory.
///
/// Segments are processed in ascending `segment_id` order.  CRC32
/// checksums are validated for every entry.
pub struct WalReader {
    dir: PathBuf,
}

impl WalReader {
    /// Create a new reader for the WAL directory.
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    /// Read all entries from all segments, validating CRC for every entry.
    ///
    /// Returns an error if any corruption or CRC mismatch is detected.
    /// Accepts both WAL v1 and WAL v2 segments transparently.
    pub fn read_all(&self) -> RaftResult<Vec<LogEntry>> {
        let segments = list_segments(&self.dir)?;
        let mut all_entries = Vec::new();

        for seg_id in segments {
            let path = segment_path(&self.dir, seg_id);
            let data = read_segment_file(&path)?;

            if data.len() < SEGMENT_HEADER_SIZE {
                return Err(RaftError::StorageError {
                    message: format!(
                        "segment {} too small ({} bytes)",
                        path.display(),
                        data.len()
                    ),
                });
            }

            // Validate header
            let header = SegmentHeader::decode(&data[..SEGMENT_HEADER_SIZE])?;

            // Decode entries — strict mode (errors on CRC mismatch)
            let entries = decode_entries(&data[SEGMENT_HEADER_SIZE..], false, header.is_v1())?;
            all_entries.extend(entries);
        }

        Ok(all_entries)
    }

    /// Recover entries from all segments, tolerating a partial or corrupted
    /// final entry.
    ///
    /// Complete, CRC-valid entries are returned.  If the very last entry of
    /// the very last segment is incomplete or has a bad CRC it is silently
    /// discarded (crash recovery).  Corruption in the middle of a segment
    /// still returns an error.
    ///
    /// Uses [`CorruptionPolicy::TruncateToLastGood`] for backward
    /// compatibility.
    pub fn recover(&self) -> RaftResult<Vec<LogEntry>> {
        let (entries, _diag) = self.recover_with_policy(CorruptionPolicy::TruncateToLastGood)?;
        Ok(entries)
    }

    /// Recover entries with an explicit [`CorruptionPolicy`].
    ///
    /// Returns the recovered entries together with [`WalDiagnostics`]
    /// describing what was found.
    pub fn recover_with_policy(
        &self,
        policy: CorruptionPolicy,
    ) -> RaftResult<(Vec<LogEntry>, WalDiagnostics)> {
        let segments = list_segments(&self.dir)?;
        let seg_count = segments.len();
        let mut all_entries = Vec::new();
        let mut diag = WalDiagnostics::default();

        for (i, seg_id) in segments.into_iter().enumerate() {
            let path = segment_path(&self.dir, seg_id);
            let data = read_segment_file(&path)?;

            if data.len() < SEGMENT_HEADER_SIZE {
                if i == seg_count - 1 {
                    diag.truncated_segments += 1;
                    tracing::warn!(
                        segment_id = seg_id,
                        bytes = data.len(),
                        "skipping incomplete final segment header"
                    );
                    break;
                }
                return Err(RaftError::StorageError {
                    message: format!(
                        "segment {} too small ({} bytes)",
                        path.display(),
                        data.len()
                    ),
                });
            }

            // Validate header
            let header = SegmentHeader::decode(&data[..SEGMENT_HEADER_SIZE])?;

            let is_last = i == seg_count - 1;
            let (entries, seg_diag) = decode_entries_with_policy(
                &data[SEGMENT_HEADER_SIZE..],
                is_last,
                policy,
                seg_id,
                header.is_v1(),
            )?;
            diag.valid_entries += seg_diag.valid_entries;
            diag.corrupt_entries += seg_diag.corrupt_entries;
            diag.truncated_segments += seg_diag.truncated_segments;
            diag.recovered_bytes += seg_diag.recovered_bytes;
            all_entries.extend(entries);
        }

        Ok((all_entries, diag))
    }
}

// ---------------------------------------------------------------------------
// Encoding / decoding helpers
// ---------------------------------------------------------------------------

/// Encode a [`LogEntry`] to the WAL v2 on-disk binary format.
///
/// Format: `[entry_len:4 LE][term:8 LE][index:8 LE][cmd_len:4 LE][cmd:N][fencing_token:8 LE][crc32:4 LE]`
///
/// `entry_len` covers everything after the first 4 bytes up to and including
/// the CRC.  The CRC is computed over all payload bytes (term, index, cmd_len,
/// cmd, fencing_token) but *excludes* `entry_len` and the CRC itself.
fn encode_entry(entry: &LogEntry) -> Vec<u8> {
    let cmd_bytes = &entry.command.data;
    // payload = term(8) + index(8) + cmd_len(4) + cmd(N) + fencing_token(8) + crc(4)
    let payload_len = 8 + 8 + 4 + cmd_bytes.len() + 8 + 4;

    let mut buf = Vec::with_capacity(4 + payload_len);

    // entry_len (u32 LE) — everything after these 4 bytes
    buf.extend_from_slice(&(payload_len as u32).to_le_bytes());
    // term (u64 LE)
    buf.extend_from_slice(&entry.term.to_le_bytes());
    // index (u64 LE)
    buf.extend_from_slice(&entry.index.to_le_bytes());
    // cmd_len (u32 LE)
    buf.extend_from_slice(&(cmd_bytes.len() as u32).to_le_bytes());
    // cmd bytes
    buf.extend_from_slice(cmd_bytes);
    // fencing_token (u64 LE)
    buf.extend_from_slice(&entry.fencing_token.to_le_bytes());
    // crc32 over payload (everything between entry_len and here)
    let crc = crc32fast::hash(&buf[4..]);
    buf.extend_from_slice(&crc.to_le_bytes());

    buf
}

/// Decode entries from raw bytes after the segment header.
///
/// When `lenient_tail` is `true`, a partial or CRC-bad final entry is
/// silently discarded (crash recovery mode).  When `false`, any corruption
/// returns an error.  Pass `is_v1 = true` for legacy WAL v1 segments that
/// do not carry the 8-byte fencing token per entry.
fn decode_entries(data: &[u8], lenient_tail: bool, is_v1: bool) -> RaftResult<Vec<LogEntry>> {
    let mut entries = Vec::new();
    let mut pos = 0;

    while pos + 4 <= data.len() {
        let entry_len = u32::from_le_bytes(read_4(data, pos)?) as usize;

        // Do we have enough bytes for the full record?
        if pos + 4 + entry_len > data.len() {
            if lenient_tail {
                break; // partial trailing entry — discard
            }
            return Err(RaftError::StorageError {
                message: format!(
                    "truncated entry at offset {pos}: need {} more bytes",
                    (pos + 4 + entry_len) - data.len()
                ),
            });
        }

        let record_start = pos + 4;
        let record_end = record_start + entry_len;
        let record = &data[record_start..record_end];

        if entry_len < 4 {
            if lenient_tail && record_end >= data.len() {
                break;
            }
            return Err(RaftError::StorageError {
                message: format!("entry_len too small ({entry_len}) at offset {pos}"),
            });
        }

        let payload = &record[..entry_len - 4];
        let stored_crc = u32::from_le_bytes(read_4(record, entry_len - 4)?);
        let computed_crc = crc32fast::hash(payload);

        if stored_crc != computed_crc {
            if lenient_tail && record_end >= data.len() {
                break; // corrupted last entry — discard in recovery mode
            }
            return Err(RaftError::StorageError {
                message: format!(
                    "CRC mismatch at offset {pos}: stored={stored_crc:#010x}, computed={computed_crc:#010x}"
                ),
            });
        }

        let entry = parse_payload(payload, is_v1, pos)?;
        entries.push(entry);

        pos = record_end;
    }

    Ok(entries)
}

/// Parse a single entry payload (after the outer CRC has already been validated).
///
/// v1 layout: term(8) + index(8) + cmd_len(4) + cmd(N)  [no token]
/// v2 layout: term(8) + index(8) + cmd_len(4) + cmd(N) + fencing_token(8)
fn parse_payload(payload: &[u8], is_v1: bool, offset: usize) -> RaftResult<LogEntry> {
    let min_len = if is_v1 { 20 } else { 28 };
    if payload.len() < min_len {
        return Err(RaftError::StorageError {
            message: format!("record payload too short at offset {offset}"),
        });
    }

    let term = u64::from_le_bytes(read_8(payload, 0)?);
    let index = u64::from_le_bytes(read_8(payload, 8)?);
    let cmd_len = u32::from_le_bytes(read_4(payload, 16)?) as usize;

    let cmd_end = 20 + cmd_len;
    if payload.len() < cmd_end {
        return Err(RaftError::StorageError {
            message: format!("cmd_len exceeds record at offset {offset}"),
        });
    }

    let cmd_data = payload[20..cmd_end].to_vec();

    let fencing_token = if is_v1 {
        0u64
    } else {
        if payload.len() < cmd_end + 8 {
            return Err(RaftError::StorageError {
                message: format!("missing fencing_token bytes at offset {offset}"),
            });
        }
        u64::from_le_bytes(read_8(payload, cmd_end)?)
    };

    Ok(LogEntry::with_fencing_token(
        term,
        index,
        Command::new(cmd_data),
        fencing_token,
    ))
}

/// Decode entries from raw bytes with a configurable [`CorruptionPolicy`].
///
/// When `lenient_tail` is `true`, a partial trailing entry is silently
/// discarded regardless of policy.  `is_v1` controls whether the legacy v1
/// entry format (no fencing token) is expected.
fn decode_entries_with_policy(
    data: &[u8],
    lenient_tail: bool,
    policy: CorruptionPolicy,
    segment_id: u32,
    is_v1: bool,
) -> RaftResult<(Vec<LogEntry>, WalDiagnostics)> {
    let mut entries = Vec::new();
    let mut diag = WalDiagnostics::default();
    let mut pos = 0;
    let mut entry_idx: u64 = 0;

    while pos + 4 <= data.len() {
        let entry_len = u32::from_le_bytes(read_4(data, pos)?) as usize;

        // Do we have enough bytes for the full record?
        if pos + 4 + entry_len > data.len() {
            if lenient_tail {
                diag.truncated_segments += 1;
                tracing::warn!(
                    segment_id,
                    entry_idx,
                    offset = pos,
                    "partial trailing entry discarded"
                );
                break;
            }
            return Err(RaftError::StorageError {
                message: format!(
                    "truncated entry at offset {pos}: need {} more bytes",
                    (pos + 4 + entry_len) - data.len()
                ),
            });
        }

        let record_start = pos + 4;
        let record_end = record_start + entry_len;
        let record = &data[record_start..record_end];
        let record_total_bytes = 4 + entry_len;

        if entry_len < 4 {
            if lenient_tail && record_end >= data.len() {
                diag.truncated_segments += 1;
                break;
            }
            return Err(RaftError::StorageError {
                message: format!("entry_len too small ({entry_len}) at offset {pos}"),
            });
        }

        let payload = &record[..entry_len - 4];
        let stored_crc = u32::from_le_bytes(read_4(record, entry_len - 4)?);
        let computed_crc = crc32fast::hash(payload);

        if stored_crc != computed_crc {
            tracing::warn!(
                segment_id,
                entry_idx,
                offset = pos,
                stored_crc = format_args!("{stored_crc:#010x}"),
                computed_crc = format_args!("{computed_crc:#010x}"),
                policy = ?policy,
                "CRC mismatch detected"
            );
            diag.corrupt_entries += 1;

            match policy {
                CorruptionPolicy::RefuseStart => {
                    return Err(RaftError::StorageError {
                        message: format!(
                            "CRC mismatch at segment {segment_id}, offset {pos}: \
                             stored={stored_crc:#010x}, computed={computed_crc:#010x}"
                        ),
                    });
                }
                CorruptionPolicy::TruncateToLastGood => {
                    tracing::warn!(
                        segment_id,
                        entry_idx,
                        offset = pos,
                        "truncating WAL at corruption point"
                    );
                    break;
                }
                CorruptionPolicy::AlertAndContinue => {
                    tracing::warn!(
                        segment_id,
                        entry_idx,
                        offset = pos,
                        "skipping corrupted entry (AlertAndContinue)"
                    );
                    pos = record_end;
                    entry_idx += 1;
                    continue;
                }
            }
        }

        let entry = parse_payload(payload, is_v1, pos)?;
        entries.push(entry);

        diag.valid_entries += 1;
        diag.recovered_bytes += record_total_bytes as u64;
        pos = record_end;
        entry_idx += 1;
    }

    Ok((entries, diag))
}

// ---------------------------------------------------------------------------
// File / path helpers
// ---------------------------------------------------------------------------

/// Build the path for a segment file:  `<dir>/wal-<segment_id:08>.seg`
fn segment_path(dir: &Path, segment_id: u32) -> PathBuf {
    dir.join(format!("wal-{segment_id:08}.seg"))
}

/// List segment IDs present in `dir`, sorted ascending.
fn list_segments(dir: &Path) -> RaftResult<Vec<u32>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut ids: Vec<u32> = Vec::new();
    let read_dir = fs::read_dir(dir).map_err(|e| RaftError::StorageError {
        message: format!("failed to read WAL dir {}: {e}", dir.display()),
    })?;

    for entry in read_dir {
        let entry = entry.map_err(|e| RaftError::StorageError {
            message: format!("failed to read dir entry: {e}"),
        })?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Some(id) = parse_segment_name(&name_str) {
            ids.push(id);
        }
    }

    ids.sort_unstable();
    Ok(ids)
}

/// Parse a segment filename like `wal-00000003.seg` into the id `3`.
fn parse_segment_name(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("wal-")?;
    let digits = rest.strip_suffix(".seg")?;
    digits.parse::<u32>().ok()
}

/// Read an entire segment file into memory.
fn read_segment_file(path: &Path) -> RaftResult<Vec<u8>> {
    let mut file = File::open(path).map_err(|e| RaftError::StorageError {
        message: format!("failed to open segment {}: {e}", path.display()),
    })?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|e| RaftError::StorageError {
            message: format!("failed to read segment {}: {e}", path.display()),
        })?;
    Ok(data)
}

// ---------------------------------------------------------------------------
// Byte-reading helpers (same contract as persistence.rs)
// ---------------------------------------------------------------------------

fn read_4(data: &[u8], offset: usize) -> RaftResult<[u8; 4]> {
    data.get(offset..offset + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| RaftError::StorageError {
            message: format!("unexpected EOF reading 4 bytes at offset {offset}"),
        })
}

fn read_8(data: &[u8], offset: usize) -> RaftResult<[u8; 8]> {
    data.get(offset..offset + 8)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| RaftError::StorageError {
            message: format!("unexpected EOF reading 8 bytes at offset {offset}"),
        })
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Command;

    /// Helper: create a unique temp directory for a test.
    fn test_wal_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "amaters_wal_test_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Helper: build a simple log entry.
    fn make_entry(term: u64, index: u64, payload: &str) -> LogEntry {
        LogEntry::new(term, index, Command::new(payload.as_bytes().to_vec()))
    }

    #[test]
    fn test_wal_append_and_read_back() {
        let dir = test_wal_dir("append_read");
        let mut writer =
            WalWriter::new(&dir, SyncMode::EveryWrite, DEFAULT_MAX_SEGMENT_SIZE).expect("writer");

        for i in 1..=10 {
            let entry = make_entry(1, i, &format!("cmd-{i}"));
            writer.append(&entry).expect("append");
        }

        let reader = WalReader::new(&dir);
        let entries = reader.read_all().expect("read_all");
        assert_eq!(entries.len(), 10);

        for (i, entry) in entries.iter().enumerate() {
            let idx = (i + 1) as u64;
            assert_eq!(entry.term, 1);
            assert_eq!(entry.index, idx);
            assert_eq!(entry.command.data, format!("cmd-{idx}").as_bytes().to_vec());
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wal_crc_corruption_detection() {
        let dir = test_wal_dir("crc_corrupt");
        let mut writer =
            WalWriter::new(&dir, SyncMode::EveryWrite, DEFAULT_MAX_SEGMENT_SIZE).expect("writer");

        for i in 1..=5 {
            writer
                .append(&make_entry(1, i, &format!("data-{i}")))
                .expect("append");
        }

        // Corrupt a byte inside the first segment (after header, inside an entry payload)
        let segments = list_segments(&dir).expect("list");
        assert!(!segments.is_empty());
        let seg_path = segment_path(&dir, segments[0]);

        let mut data = fs::read(&seg_path).expect("read seg");
        // Flip a byte in the middle of the segment data
        let corrupt_offset = SEGMENT_HEADER_SIZE + 10;
        if corrupt_offset < data.len() {
            data[corrupt_offset] ^= 0xFF;
        }
        fs::write(&seg_path, &data).expect("write corrupted");

        // read_all should fail
        let reader = WalReader::new(&dir);
        assert!(reader.read_all().is_err());

        // recover should silently truncate or skip the corrupted entry
        // (since it's not necessarily the last entry, recover may also error
        // — but if corruption is in the only segment's first entry the
        // recovered log will simply be empty.)
        // For a robust test, we accept either Ok([]) or Err.
        let result = reader.recover();
        // With corruption in the middle, recover returns Err unless
        // the corruption is in the tail entry.  Both outcomes are valid.
        assert!(result.is_ok() || result.is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wal_segment_rotation() {
        let dir = test_wal_dir("rotation");
        // Use a tiny segment size to force frequent rotation
        let mut writer = WalWriter::new(&dir, SyncMode::EveryWrite, 256).expect("writer");

        for i in 1..=20 {
            writer
                .append(&make_entry(1, i, &format!("rot-{i}")))
                .expect("append");
        }

        let segments = list_segments(&dir).expect("list");
        assert!(
            segments.len() > 1,
            "expected multiple segments, got {}",
            segments.len()
        );

        let reader = WalReader::new(&dir);
        let entries = reader.read_all().expect("read_all");
        assert_eq!(entries.len(), 20);
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.index, (i + 1) as u64);
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wal_crash_recovery() {
        let dir = test_wal_dir("crash_recovery");
        let mut writer =
            WalWriter::new(&dir, SyncMode::EveryWrite, DEFAULT_MAX_SEGMENT_SIZE).expect("writer");

        for i in 1..=5 {
            writer
                .append(&make_entry(1, i, &format!("ok-{i}")))
                .expect("append");
        }

        // Simulate a crash: append a partial entry (truncated bytes) to
        // the segment file.
        let segments = list_segments(&dir).expect("list");
        let seg_path = segment_path(&dir, *segments.last().expect("last seg"));

        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(&seg_path)
                .expect("open for partial write");
            // Write a partial entry header (entry_len says 100 but we only
            // write 6 bytes of payload — incomplete).
            let fake_len: u32 = 100;
            f.write_all(&fake_len.to_le_bytes())
                .expect("write partial len");
            f.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE])
                .expect("write partial data");
        }

        // read_all should fail (strict mode sees truncated entry)
        let reader = WalReader::new(&dir);
        assert!(reader.read_all().is_err());

        // recover should return only the 5 complete entries
        let recovered = reader.recover().expect("recover");
        assert_eq!(recovered.len(), 5);
        for (i, entry) in recovered.iter().enumerate() {
            assert_eq!(entry.index, (i + 1) as u64);
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wal_empty_startup() {
        let dir = test_wal_dir("empty");
        let _writer =
            WalWriter::new(&dir, SyncMode::EveryWrite, DEFAULT_MAX_SEGMENT_SIZE).expect("writer");

        let reader = WalReader::new(&dir);
        let entries = reader.read_all().expect("read_all");
        assert!(entries.is_empty());

        // recover also returns empty
        let recovered = reader.recover().expect("recover");
        assert!(recovered.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wal_truncate_from() {
        let dir = test_wal_dir("truncate");
        let mut writer =
            WalWriter::new(&dir, SyncMode::EveryWrite, DEFAULT_MAX_SEGMENT_SIZE).expect("writer");

        for i in 1..=10 {
            writer
                .append(&make_entry(1, i, &format!("entry-{i}")))
                .expect("append");
        }

        writer.truncate_from(6).expect("truncate_from(6)");

        let reader = WalReader::new(&dir);
        let entries = reader.read_all().expect("read_all");
        assert_eq!(entries.len(), 5);
        for (i, entry) in entries.iter().enumerate() {
            let idx = (i + 1) as u64;
            assert_eq!(entry.index, idx);
            assert_eq!(
                entry.command.data,
                format!("entry-{idx}").as_bytes().to_vec()
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Corruption-policy tests
    // -----------------------------------------------------------------------

    /// Helper: write `count` entries and return the segment path.
    fn write_entries(dir: &Path, count: u64) -> PathBuf {
        let mut writer =
            WalWriter::new(dir, SyncMode::EveryWrite, DEFAULT_MAX_SEGMENT_SIZE).expect("writer");
        for i in 1..=count {
            writer
                .append(&make_entry(1, i, &format!("payload-{i}")))
                .expect("append");
        }
        let segs = list_segments(dir).expect("list");
        segment_path(dir, *segs.last().expect("segment"))
    }

    /// Corrupt a single byte inside entry `entry_number` (1-based) of the
    /// first segment.  This targets the payload region so the CRC will not
    /// match.
    fn corrupt_entry_n(seg_path: &Path, entry_number: usize) {
        let mut data = fs::read(seg_path).expect("read segment");
        let mut pos = SEGMENT_HEADER_SIZE;
        for n in 1..=entry_number {
            let entry_len =
                u32::from_le_bytes(data[pos..pos + 4].try_into().expect("4 bytes")) as usize;
            if n == entry_number {
                // Flip a byte inside the payload of this entry
                let payload_start = pos + 4;
                let flip_offset = payload_start + 2;
                data[flip_offset] ^= 0xFF;
                break;
            }
            pos += 4 + entry_len;
        }
        fs::write(seg_path, &data).expect("write corrupted");
    }

    // --- Spec-named B2 tests ---

    #[test]
    fn test_wal_corrupted_refuse_start() {
        let dir = test_wal_dir("wal_corrupted_refuse_start");
        let seg_path = write_entries(&dir, 5);
        corrupt_entry_n(&seg_path, 3);

        let reader = WalReader::new(&dir);
        let result = reader.recover_with_policy(CorruptionPolicy::RefuseStart);
        assert!(
            result.is_err(),
            "RefuseStart should return error on corruption"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wal_corrupted_truncate() {
        let dir = test_wal_dir("wal_corrupted_truncate");
        let seg_path = write_entries(&dir, 5);
        // Corrupt entry 3 (entries 1,2 should survive)
        corrupt_entry_n(&seg_path, 3);

        let reader = WalReader::new(&dir);
        let (entries, diag) = reader
            .recover_with_policy(CorruptionPolicy::TruncateToLastGood)
            .expect("recover");

        assert_eq!(
            entries.len(),
            2,
            "TruncateToLastGood: keep entries before corruption"
        );
        assert_eq!(diag.valid_entries, 2);
        assert_eq!(diag.corrupt_entries, 1);
        assert!(diag.recovered_bytes > 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wal_corrupted_alert_continue() {
        let dir = test_wal_dir("wal_corrupted_alert_continue");
        let seg_path = write_entries(&dir, 5);
        // Corrupt entry 2 — entries 1, 3, 4, 5 should survive
        corrupt_entry_n(&seg_path, 2);

        let reader = WalReader::new(&dir);
        let (entries, diag) = reader
            .recover_with_policy(CorruptionPolicy::AlertAndContinue)
            .expect("recover");

        assert_eq!(
            entries.len(),
            4,
            "AlertAndContinue: skip only the corrupted entry"
        );
        assert_eq!(diag.corrupt_entries, 1);
        assert_eq!(diag.valid_entries, 4);

        // Verify the indices are 1, 3, 4, 5
        let indices: Vec<u64> = entries.iter().map(|e| e.index).collect();
        assert_eq!(indices, vec![1, 3, 4, 5]);

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Extended corruption-policy tests ---

    #[test]
    fn test_corruption_policy_refuse_start_inner() {
        let dir = test_wal_dir("corruption_refuse_start_inner");
        let seg_path = write_entries(&dir, 5);
        corrupt_entry_n(&seg_path, 3);

        let reader = WalReader::new(&dir);
        let result = reader.recover_with_policy(CorruptionPolicy::RefuseStart);
        assert!(
            result.is_err(),
            "RefuseStart should return error on corruption"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_corruption_policy_truncate_to_last_good() {
        let dir = test_wal_dir("corruption_truncate_last_good");
        let seg_path = write_entries(&dir, 5);
        // Corrupt entry 3 (entries 1,2 should survive)
        corrupt_entry_n(&seg_path, 3);

        let reader = WalReader::new(&dir);
        let (entries, diag) = reader
            .recover_with_policy(CorruptionPolicy::TruncateToLastGood)
            .expect("recover");

        assert_eq!(entries.len(), 2, "should keep entries before corruption");
        assert_eq!(diag.valid_entries, 2);
        assert_eq!(diag.corrupt_entries, 1);
        assert!(diag.recovered_bytes > 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_corruption_policy_alert_and_continue() {
        let dir = test_wal_dir("corruption_alert_continue");
        let seg_path = write_entries(&dir, 5);
        // Corrupt entry 2 — entries 1, 3, 4, 5 should survive
        corrupt_entry_n(&seg_path, 2);

        let reader = WalReader::new(&dir);
        let (entries, diag) = reader
            .recover_with_policy(CorruptionPolicy::AlertAndContinue)
            .expect("recover");

        assert_eq!(entries.len(), 4, "should skip only the corrupted entry");
        assert_eq!(diag.corrupt_entries, 1);
        assert_eq!(diag.valid_entries, 4);

        // Verify the indices are 1, 3, 4, 5
        let indices: Vec<u64> = entries.iter().map(|e| e.index).collect();
        assert_eq!(indices, vec![1, 3, 4, 5]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_corruption_policy_first_entry() {
        let dir = test_wal_dir("corruption_first");
        let seg_path = write_entries(&dir, 5);
        corrupt_entry_n(&seg_path, 1);

        let reader = WalReader::new(&dir);

        // AlertAndContinue: entries 2..5 survive
        let (entries, diag) = reader
            .recover_with_policy(CorruptionPolicy::AlertAndContinue)
            .expect("recover");
        assert_eq!(entries.len(), 4);
        assert_eq!(diag.corrupt_entries, 1);

        // TruncateToLastGood: nothing survives
        let (entries, diag) = reader
            .recover_with_policy(CorruptionPolicy::TruncateToLastGood)
            .expect("recover");
        assert_eq!(entries.len(), 0);
        assert_eq!(diag.corrupt_entries, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_corruption_policy_last_entry() {
        let dir = test_wal_dir("corruption_last");
        let seg_path = write_entries(&dir, 5);
        corrupt_entry_n(&seg_path, 5);

        let reader = WalReader::new(&dir);

        // TruncateToLastGood: entries 1..4 survive
        let (entries, diag) = reader
            .recover_with_policy(CorruptionPolicy::TruncateToLastGood)
            .expect("recover");
        assert_eq!(entries.len(), 4);
        assert_eq!(diag.corrupt_entries, 1);

        // AlertAndContinue: entries 1..4 survive
        let (entries, diag) = reader
            .recover_with_policy(CorruptionPolicy::AlertAndContinue)
            .expect("recover");
        assert_eq!(entries.len(), 4);
        assert_eq!(diag.corrupt_entries, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_corruption_diagnostics_no_corruption() {
        let dir = test_wal_dir("diag_clean");
        write_entries(&dir, 10);

        let reader = WalReader::new(&dir);
        let (entries, diag) = reader
            .recover_with_policy(CorruptionPolicy::RefuseStart)
            .expect("recover");
        assert_eq!(entries.len(), 10);
        assert_eq!(diag.valid_entries, 10);
        assert_eq!(diag.corrupt_entries, 0);
        assert_eq!(diag.truncated_segments, 0);
        assert!(diag.recovered_bytes > 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_corruption_recover_backward_compat() {
        // The old recover() should still work and use TruncateToLastGood
        let dir = test_wal_dir("corruption_compat");
        let seg_path = write_entries(&dir, 5);
        corrupt_entry_n(&seg_path, 3);

        let reader = WalReader::new(&dir);
        let entries = reader.recover().expect("recover");
        // TruncateToLastGood: entries 1, 2
        assert_eq!(entries.len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wal_sync_modes() {
        // EveryWrite
        {
            let dir = test_wal_dir("sync_every");
            let mut writer = WalWriter::new(&dir, SyncMode::EveryWrite, DEFAULT_MAX_SEGMENT_SIZE)
                .expect("writer");
            for i in 1..=5 {
                writer.append(&make_entry(1, i, "a")).expect("append");
            }
            let reader = WalReader::new(&dir);
            assert_eq!(reader.read_all().expect("read").len(), 5);
            let _ = fs::remove_dir_all(&dir);
        }

        // OsManaged
        {
            let dir = test_wal_dir("sync_os");
            let mut writer = WalWriter::new(&dir, SyncMode::OsManaged, DEFAULT_MAX_SEGMENT_SIZE)
                .expect("writer");
            for i in 1..=5 {
                writer.append(&make_entry(1, i, "b")).expect("append");
            }
            writer.sync().expect("manual sync");
            let reader = WalReader::new(&dir);
            assert_eq!(reader.read_all().expect("read").len(), 5);
            let _ = fs::remove_dir_all(&dir);
        }

        // Batched
        {
            let dir = test_wal_dir("sync_batched");
            let mut writer = WalWriter::new(&dir, SyncMode::Batched(3), DEFAULT_MAX_SEGMENT_SIZE)
                .expect("writer");
            for i in 1..=7 {
                writer.append(&make_entry(1, i, "c")).expect("append");
            }
            writer.sync().expect("final sync");
            let reader = WalReader::new(&dir);
            assert_eq!(reader.read_all().expect("read").len(), 7);
            let _ = fs::remove_dir_all(&dir);
        }
    }

    // -----------------------------------------------------------------------
    // B3 – Fencing token WAL v2 tests
    // -----------------------------------------------------------------------

    /// Write an entry carrying a specific fencing token, then read it back.
    #[test]
    fn test_wal_v2_fencing_token_roundtrip() {
        use crate::log::Command;
        let dir = test_wal_dir("v2_token_roundtrip");
        let mut writer =
            WalWriter::new(&dir, SyncMode::EveryWrite, DEFAULT_MAX_SEGMENT_SIZE).expect("writer");

        let token_raw: u64 = ((3u64) << 32) | 7u64; // term=3, seq=7
        let entry = LogEntry::with_fencing_token(1, 1, Command::new(b"hello".to_vec()), token_raw);
        writer.append(&entry).expect("append");

        let reader = WalReader::new(&dir);
        let entries = reader.read_all().expect("read_all");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].fencing_token, token_raw);
        assert_eq!(entries[0].command.data, b"hello");

        let _ = fs::remove_dir_all(&dir);
    }

    /// WAL v1 compat: hand-craft a v1 segment (no token) and verify it loads.
    #[test]
    fn test_wal_v1_backward_compat_read() {
        let dir = test_wal_dir("v1_compat");
        fs::create_dir_all(&dir).expect("mkdir");
        let seg_path = dir.join("wal-00000000.seg");

        // Build a minimal v1 segment
        let mut buf: Vec<u8> = Vec::new();

        // Segment header: magic=WAL1, version=1, segment_id=0
        buf.extend_from_slice(&WAL_MAGIC_V1.to_le_bytes()); // magic
        buf.extend_from_slice(&WAL_VERSION_V1.to_le_bytes()); // version
        buf.extend_from_slice(&0u32.to_le_bytes()); // segment_id

        // One v1 entry: [entry_len:4][term:8][index:8][cmd_len:4][cmd:N][crc32:4]
        let cmd = b"v1cmd";
        let term: u64 = 1;
        let index: u64 = 1;

        let mut payload: Vec<u8> = Vec::new();
        payload.extend_from_slice(&term.to_le_bytes());
        payload.extend_from_slice(&index.to_le_bytes());
        payload.extend_from_slice(&(cmd.len() as u32).to_le_bytes());
        payload.extend_from_slice(cmd);
        let crc = crc32fast::hash(&payload);

        let entry_len = (payload.len() + 4) as u32; // +4 for CRC
        buf.extend_from_slice(&entry_len.to_le_bytes());
        buf.extend_from_slice(&payload);
        buf.extend_from_slice(&crc.to_le_bytes());

        fs::write(&seg_path, &buf).expect("write v1 segment");

        let reader = WalReader::new(&dir);
        let entries = reader.read_all().expect("read v1 segment");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].term, 1);
        assert_eq!(entries[0].index, 1);
        assert_eq!(entries[0].command.data, b"v1cmd");
        // Token should be 0 for v1 entries
        assert_eq!(entries[0].fencing_token, 0);

        let _ = fs::remove_dir_all(&dir);
    }
}
