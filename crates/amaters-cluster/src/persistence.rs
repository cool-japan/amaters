//! Persistent storage backends for Raft consensus
//!
//! Provides trait-based persistence abstraction and two implementations:
//! - [`FilePersistence`]: File-based storage with CRC32 checksums and atomic writes
//! - [`MemoryPersistence`]: In-memory storage for testing

use crate::error::{RaftError, RaftResult};
use crate::log::{Command, LogEntry};
use crate::types::{LogIndex, NodeId, Term};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Trait for persistent storage backends
pub trait RaftPersistence: Send + Sync {
    /// Save persistent state (current_term, voted_for)
    fn save_state(&self, term: Term, voted_for: Option<NodeId>) -> RaftResult<()>;

    /// Load persistent state; returns (term, voted_for)
    fn load_state(&self) -> RaftResult<(Term, Option<NodeId>)>;

    /// Append log entries to persistent storage
    fn append_entries(&self, entries: &[LogEntry]) -> RaftResult<()>;

    /// Load all log entries from persistent storage
    fn load_log(&self) -> RaftResult<Vec<LogEntry>>;

    /// Truncate log from index (inclusive) onward
    fn truncate_log_from(&self, index: LogIndex) -> RaftResult<()>;

    /// Get the last persisted log index (0 if empty)
    fn last_log_index(&self) -> RaftResult<LogIndex>;

    /// Save the applied index to durable storage.
    ///
    /// Persisting the applied index avoids replaying already-applied entries
    /// on crash recovery, so the state machine stays consistent.
    fn save_applied_index(&self, index: LogIndex) -> RaftResult<()>;

    /// Load the previously persisted applied index (0 if not set).
    fn load_applied_index(&self) -> RaftResult<LogIndex>;

    /// Sync all data to durable storage
    fn sync(&self) -> RaftResult<()>;
}

// ---------------------------------------------------------------------------
// FilePersistence
// ---------------------------------------------------------------------------

/// File-based persistence with CRC32 integrity checks.
///
/// Layout on disk:
/// - `<dir>/raft_state.json` — serialised term + voted_for (atomic write via
///   rename)
/// - `<dir>/raft_log.bin` — append-only binary log where each record is:
///   `[len:4][term:8][index:8][cmd_len:4][cmd:N][crc32:4]`
pub struct FilePersistence {
    state_path: PathBuf,
    log_path: PathBuf,
    sync_on_write: bool,
}

/// Serialisable state written to `raft_state.json`.
#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedState {
    current_term: Term,
    voted_for: Option<NodeId>,
    #[serde(default)]
    applied_index: LogIndex,
}

impl FilePersistence {
    /// Create a new file-based persistence rooted at `dir`.
    ///
    /// Creates the directory if it does not exist.
    pub fn new(dir: &Path, sync_on_write: bool) -> RaftResult<Self> {
        std::fs::create_dir_all(dir).map_err(|e| RaftError::StorageError {
            message: format!("failed to create persistence dir {}: {e}", dir.display()),
        })?;

        Ok(Self {
            state_path: dir.join("raft_state.json"),
            log_path: dir.join("raft_log.bin"),
            sync_on_write,
        })
    }

    /// Atomic write: write to `.tmp` then rename.
    fn atomic_write_state(&self, data: &[u8]) -> RaftResult<()> {
        let tmp_path = self.state_path.with_extension("json.tmp");

        let mut f = std::fs::File::create(&tmp_path).map_err(|e| RaftError::StorageError {
            message: format!("failed to create tmp state file: {e}"),
        })?;

        f.write_all(data).map_err(|e| RaftError::StorageError {
            message: format!("failed to write tmp state file: {e}"),
        })?;

        if self.sync_on_write {
            f.sync_all().map_err(|e| RaftError::StorageError {
                message: format!("failed to sync tmp state file: {e}"),
            })?;
        }

        std::fs::rename(&tmp_path, &self.state_path).map_err(|e| RaftError::StorageError {
            message: format!("failed to rename tmp state file: {e}"),
        })?;

        Ok(())
    }

    /// Encode a single log entry into the on-disk binary format.
    ///
    /// Format: `[total_len:4 LE][term:8 LE][index:8 LE][cmd_len:4 LE][cmd bytes][crc32:4 LE]`
    ///
    /// `total_len` is the number of bytes that follow (everything after the
    /// first 4 bytes, including the trailing CRC).
    fn encode_entry(entry: &LogEntry) -> Vec<u8> {
        let cmd_bytes = &entry.command.data;
        // payload = term(8) + index(8) + cmd_len(4) + cmd + crc(4)
        let payload_len = 8 + 8 + 4 + cmd_bytes.len() + 4;

        let mut buf = Vec::with_capacity(4 + payload_len);

        // total_len (u32 LE)
        buf.extend_from_slice(&(payload_len as u32).to_le_bytes());
        // term (u64 LE)
        buf.extend_from_slice(&entry.term.to_le_bytes());
        // index (u64 LE)
        buf.extend_from_slice(&entry.index.to_le_bytes());
        // cmd_len (u32 LE)
        buf.extend_from_slice(&(cmd_bytes.len() as u32).to_le_bytes());
        // cmd bytes
        buf.extend_from_slice(cmd_bytes);
        // crc32 over everything before this point (after total_len)
        let crc = crc32fast::hash(&buf[4..]);
        buf.extend_from_slice(&crc.to_le_bytes());

        buf
    }

    /// Decode log entries from raw bytes, skipping any trailing partial /
    /// corrupted records.
    fn decode_entries(data: &[u8]) -> RaftResult<Vec<LogEntry>> {
        let mut entries = Vec::new();
        let mut pos = 0;

        while pos + 4 <= data.len() {
            // Read total_len
            let total_len = u32::from_le_bytes(read_4(data, pos)?) as usize;

            // Check we have enough bytes for the full record
            if pos + 4 + total_len > data.len() {
                // Partial record at end — stop (crash recovery truncation)
                break;
            }

            let record_start = pos + 4;
            let record_end = record_start + total_len;
            let record = &data[record_start..record_end];

            // The last 4 bytes of record are the CRC
            if total_len < 4 {
                break; // definitely corrupted
            }
            let payload = &record[..total_len - 4];
            let stored_crc = u32::from_le_bytes(read_4(record, total_len - 4)?);
            let computed_crc = crc32fast::hash(payload);

            if stored_crc != computed_crc {
                return Err(RaftError::StorageError {
                    message: format!(
                        "CRC mismatch at offset {pos}: stored={stored_crc:#010x}, computed={computed_crc:#010x}"
                    ),
                });
            }

            // Parse payload: term(8) + index(8) + cmd_len(4) + cmd(N)
            if payload.len() < 20 {
                return Err(RaftError::StorageError {
                    message: format!("record too short at offset {pos}"),
                });
            }

            let term = u64::from_le_bytes(read_8(payload, 0)?);
            let index = u64::from_le_bytes(read_8(payload, 8)?);
            let cmd_len = u32::from_le_bytes(read_4(payload, 16)?) as usize;

            if payload.len() < 20 + cmd_len {
                return Err(RaftError::StorageError {
                    message: format!("cmd_len exceeds record at offset {pos}"),
                });
            }

            let cmd_data = payload[20..20 + cmd_len].to_vec();
            entries.push(LogEntry::new(term, index, Command::new(cmd_data)));

            pos = record_end;
        }

        Ok(entries)
    }

    /// Rewrite the log file keeping only entries with index < `from_index`.
    fn rewrite_log_without(&self, from_index: LogIndex) -> RaftResult<()> {
        let entries = self.load_log()?;
        let kept: Vec<&LogEntry> = entries.iter().filter(|e| e.index < from_index).collect();

        let tmp_path = self.log_path.with_extension("bin.tmp");
        let mut f = std::fs::File::create(&tmp_path).map_err(|e| RaftError::StorageError {
            message: format!("failed to create tmp log file: {e}"),
        })?;

        for entry in &kept {
            let encoded = Self::encode_entry(entry);
            f.write_all(&encoded).map_err(|e| RaftError::StorageError {
                message: format!("failed to write entry to tmp log: {e}"),
            })?;
        }

        if self.sync_on_write {
            f.sync_all().map_err(|e| RaftError::StorageError {
                message: format!("failed to sync tmp log: {e}"),
            })?;
        }

        std::fs::rename(&tmp_path, &self.log_path).map_err(|e| RaftError::StorageError {
            message: format!("failed to rename tmp log: {e}"),
        })?;

        Ok(())
    }
}

impl RaftPersistence for FilePersistence {
    fn save_state(&self, term: Term, voted_for: Option<NodeId>) -> RaftResult<()> {
        // Preserve any previously persisted applied_index
        let applied_index = if self.state_path.exists() {
            self.load_applied_index().unwrap_or(0)
        } else {
            0
        };
        let state = PersistedState {
            current_term: term,
            voted_for,
            applied_index,
        };
        let json = serde_json::to_vec_pretty(&state).map_err(|e| RaftError::StorageError {
            message: format!("failed to serialize state: {e}"),
        })?;
        self.atomic_write_state(&json)
    }

    fn load_state(&self) -> RaftResult<(Term, Option<NodeId>)> {
        if !self.state_path.exists() {
            return Ok((0, None));
        }

        let mut f = std::fs::File::open(&self.state_path).map_err(|e| RaftError::StorageError {
            message: format!("failed to open state file: {e}"),
        })?;

        let mut data = Vec::new();
        f.read_to_end(&mut data)
            .map_err(|e| RaftError::StorageError {
                message: format!("failed to read state file: {e}"),
            })?;

        let state: PersistedState =
            serde_json::from_slice(&data).map_err(|e| RaftError::StorageError {
                message: format!("failed to parse state file: {e}"),
            })?;

        Ok((state.current_term, state.voted_for))
    }

    fn append_entries(&self, entries: &[LogEntry]) -> RaftResult<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .map_err(|e| RaftError::StorageError {
                message: format!("failed to open log file for append: {e}"),
            })?;

        for entry in entries {
            let encoded = Self::encode_entry(entry);
            f.write_all(&encoded).map_err(|e| RaftError::StorageError {
                message: format!("failed to append entry: {e}"),
            })?;
        }

        if self.sync_on_write {
            f.sync_all().map_err(|e| RaftError::StorageError {
                message: format!("failed to sync log file: {e}"),
            })?;
        }

        Ok(())
    }

    fn load_log(&self) -> RaftResult<Vec<LogEntry>> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }

        let mut f = std::fs::File::open(&self.log_path).map_err(|e| RaftError::StorageError {
            message: format!("failed to open log file: {e}"),
        })?;

        let mut data = Vec::new();
        f.read_to_end(&mut data)
            .map_err(|e| RaftError::StorageError {
                message: format!("failed to read log file: {e}"),
            })?;

        Self::decode_entries(&data)
    }

    fn truncate_log_from(&self, index: LogIndex) -> RaftResult<()> {
        if !self.log_path.exists() {
            return Ok(());
        }
        self.rewrite_log_without(index)
    }

    fn last_log_index(&self) -> RaftResult<LogIndex> {
        let entries = self.load_log()?;
        Ok(entries.last().map_or(0, |e| e.index))
    }

    fn save_applied_index(&self, index: LogIndex) -> RaftResult<()> {
        // Load current state and overwrite with updated applied_index
        let (current_term, voted_for) = if self.state_path.exists() {
            self.load_state()?
        } else {
            (0, None)
        };
        let state = PersistedState {
            current_term,
            voted_for,
            applied_index: index,
        };
        let json = serde_json::to_vec_pretty(&state).map_err(|e| RaftError::StorageError {
            message: format!("failed to serialize state (applied_index update): {e}"),
        })?;
        self.atomic_write_state(&json)
    }

    fn load_applied_index(&self) -> RaftResult<LogIndex> {
        if !self.state_path.exists() {
            return Ok(0);
        }
        let mut f = std::fs::File::open(&self.state_path).map_err(|e| RaftError::StorageError {
            message: format!("failed to open state file: {e}"),
        })?;
        let mut data = Vec::new();
        f.read_to_end(&mut data)
            .map_err(|e| RaftError::StorageError {
                message: format!("failed to read state file: {e}"),
            })?;
        let state: PersistedState =
            serde_json::from_slice(&data).map_err(|e| RaftError::StorageError {
                message: format!("failed to parse state file (applied_index): {e}"),
            })?;
        Ok(state.applied_index)
    }

    fn sync(&self) -> RaftResult<()> {
        // Opening and syncing the directory is the most portable way to flush
        // metadata on POSIX.  On macOS/Windows this is best-effort.
        if let Ok(dir) =
            std::fs::File::open(self.state_path.parent().unwrap_or_else(|| Path::new(".")))
        {
            let _ = dir.sync_all();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MemoryPersistence (for testing)
// ---------------------------------------------------------------------------

/// In-memory persistence backend, useful for tests.
pub struct MemoryPersistence {
    state: parking_lot::RwLock<(Term, Option<NodeId>)>,
    log: parking_lot::RwLock<Vec<LogEntry>>,
    applied_index: parking_lot::RwLock<LogIndex>,
}

impl MemoryPersistence {
    /// Create a new empty in-memory persistence backend.
    pub fn new() -> Self {
        Self {
            state: parking_lot::RwLock::new((0, None)),
            log: parking_lot::RwLock::new(Vec::new()),
            applied_index: parking_lot::RwLock::new(0),
        }
    }
}

impl Default for MemoryPersistence {
    fn default() -> Self {
        Self::new()
    }
}

impl RaftPersistence for MemoryPersistence {
    fn save_state(&self, term: Term, voted_for: Option<NodeId>) -> RaftResult<()> {
        *self.state.write() = (term, voted_for);
        Ok(())
    }

    fn load_state(&self) -> RaftResult<(Term, Option<NodeId>)> {
        Ok(*self.state.read())
    }

    fn append_entries(&self, entries: &[LogEntry]) -> RaftResult<()> {
        self.log.write().extend(entries.iter().cloned());
        Ok(())
    }

    fn load_log(&self) -> RaftResult<Vec<LogEntry>> {
        Ok(self.log.read().clone())
    }

    fn truncate_log_from(&self, index: LogIndex) -> RaftResult<()> {
        self.log.write().retain(|e| e.index < index);
        Ok(())
    }

    fn last_log_index(&self) -> RaftResult<LogIndex> {
        Ok(self.log.read().last().map_or(0, |e| e.index))
    }

    fn save_applied_index(&self, index: LogIndex) -> RaftResult<()> {
        *self.applied_index.write() = index;
        Ok(())
    }

    fn load_applied_index(&self) -> RaftResult<LogIndex> {
        Ok(*self.applied_index.read())
    }

    fn sync(&self) -> RaftResult<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Helper: create a temp dir for file persistence tests.
    fn temp_persistence_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "amaters_test_{prefix}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // ensure clean start
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn make_entry(term: Term, index: LogIndex, data: &str) -> LogEntry {
        LogEntry::new(term, index, Command::from_str(data))
    }

    // ---- FilePersistence: state ----

    #[test]
    fn test_file_persistence_save_load_state() {
        let dir = temp_persistence_dir("state_save_load");
        let fp = FilePersistence::new(&dir, true).expect("create persistence");

        // Default state
        let (term, voted) = fp.load_state().expect("load default");
        assert_eq!(term, 0);
        assert_eq!(voted, None);

        // Save and reload
        fp.save_state(5, Some(42)).expect("save");
        let (term, voted) = fp.load_state().expect("load after save");
        assert_eq!(term, 5);
        assert_eq!(voted, Some(42));

        // Overwrite
        fp.save_state(10, None).expect("overwrite");
        let (term, voted) = fp.load_state().expect("load overwritten");
        assert_eq!(term, 10);
        assert_eq!(voted, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- FilePersistence: log ----

    #[test]
    fn test_file_persistence_append_load_log() {
        let dir = temp_persistence_dir("log_append_load");
        let fp = FilePersistence::new(&dir, true).expect("create");

        let entries = vec![
            make_entry(1, 1, "cmd1"),
            make_entry(1, 2, "cmd2"),
            make_entry(2, 3, "cmd3"),
        ];

        fp.append_entries(&entries).expect("append");

        let loaded = fp.load_log().expect("load");
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].term, 1);
        assert_eq!(loaded[0].index, 1);
        assert_eq!(loaded[0].command.data, b"cmd1");
        assert_eq!(loaded[2].term, 2);
        assert_eq!(loaded[2].index, 3);

        // Append more
        fp.append_entries(&[make_entry(2, 4, "cmd4")])
            .expect("append more");
        let loaded = fp.load_log().expect("load 2");
        assert_eq!(loaded.len(), 4);

        assert_eq!(fp.last_log_index().expect("last idx"), 4);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- FilePersistence: truncate ----

    #[test]
    fn test_file_persistence_truncate_log() {
        let dir = temp_persistence_dir("log_truncate");
        let fp = FilePersistence::new(&dir, true).expect("create");

        let entries = vec![
            make_entry(1, 1, "a"),
            make_entry(1, 2, "b"),
            make_entry(2, 3, "c"),
            make_entry(2, 4, "d"),
        ];
        fp.append_entries(&entries).expect("append");

        // Truncate from index 3 onward
        fp.truncate_log_from(3).expect("truncate");
        let loaded = fp.load_log().expect("load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].index, 1);
        assert_eq!(loaded[1].index, 2);

        assert_eq!(fp.last_log_index().expect("last idx"), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- FilePersistence: crash recovery (drop + reopen) ----

    #[test]
    fn test_file_persistence_crash_recovery() {
        let dir = temp_persistence_dir("crash_recovery");

        // "Session 1" — write state + log, then drop.
        {
            let fp = FilePersistence::new(&dir, true).expect("create");
            fp.save_state(7, Some(99)).expect("save state");
            fp.append_entries(&[
                make_entry(5, 1, "hello"),
                make_entry(6, 2, "world"),
                make_entry(7, 3, "!"),
            ])
            .expect("append");
            fp.sync().expect("sync");
        }
        // fp is dropped — simulates crash.

        // "Session 2" — reopen and verify.
        {
            let fp = FilePersistence::new(&dir, true).expect("reopen");

            let (term, voted) = fp.load_state().expect("load state");
            assert_eq!(term, 7);
            assert_eq!(voted, Some(99));

            let log = fp.load_log().expect("load log");
            assert_eq!(log.len(), 3);
            assert_eq!(log[0].command.data, b"hello");
            assert_eq!(log[2].index, 3);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- FilePersistence: atomic state write ----

    #[test]
    fn test_file_persistence_atomic_state_write() {
        let dir = temp_persistence_dir("atomic_state");
        let fp = FilePersistence::new(&dir, true).expect("create");

        // Write initial state
        fp.save_state(1, Some(10)).expect("save 1");

        // Write second state (atomic overwrite)
        fp.save_state(2, Some(20)).expect("save 2");

        // Verify no leftover .tmp file
        let tmp = fp.state_path.with_extension("json.tmp");
        assert!(!tmp.exists(), "tmp file should have been renamed away");

        let (term, voted) = fp.load_state().expect("load");
        assert_eq!(term, 2);
        assert_eq!(voted, Some(20));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- FilePersistence: corrupted entry detection ----

    #[test]
    fn test_file_persistence_corrupted_entry() {
        let dir = temp_persistence_dir("corrupted");
        let fp = FilePersistence::new(&dir, true).expect("create");

        fp.append_entries(&[make_entry(1, 1, "good")])
            .expect("append");

        // Corrupt the log file by flipping a byte in the middle
        let mut data = std::fs::read(&fp.log_path).expect("read raw");
        // Flip a byte in the payload area (after the 4-byte length header)
        if data.len() > 10 {
            data[10] ^= 0xFF;
        }
        std::fs::write(&fp.log_path, &data).expect("write corrupted");

        let result = fp.load_log();
        assert!(result.is_err(), "should detect CRC mismatch");
        let err_msg = format!("{}", result.expect_err("expected error"));
        assert!(
            err_msg.contains("CRC mismatch"),
            "error should mention CRC: {err_msg}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- MemoryPersistence ----

    #[test]
    fn test_memory_persistence_basic() {
        let mp = MemoryPersistence::new();

        // State
        let (t, v) = mp.load_state().expect("load default");
        assert_eq!(t, 0);
        assert_eq!(v, None);

        mp.save_state(3, Some(7)).expect("save");
        let (t, v) = mp.load_state().expect("load");
        assert_eq!(t, 3);
        assert_eq!(v, Some(7));

        // Log
        mp.append_entries(&[make_entry(1, 1, "x"), make_entry(1, 2, "y")])
            .expect("append");
        assert_eq!(mp.last_log_index().expect("last"), 2);

        mp.truncate_log_from(2).expect("truncate");
        assert_eq!(mp.last_log_index().expect("last after trunc"), 1);

        mp.sync().expect("sync");
    }

    // ---- Integration: persistence is Send + Sync + object safe ----

    #[test]
    fn test_persistence_trait_object() {
        let mp: Arc<dyn RaftPersistence> = Arc::new(MemoryPersistence::new());
        mp.save_state(1, None).expect("save via trait object");
        let (t, _) = mp.load_state().expect("load via trait object");
        assert_eq!(t, 1);
    }
}
