//! Log management for Raft consensus

use crate::error::{RaftError, RaftResult};
use crate::types::{LogIndex, Term};
use std::collections::VecDeque;

/// Trait for state machines that can be driven by the Raft log.
///
/// Implementors receive committed log entries in order and produce
/// deterministic outputs.  They must also support snapshotting and
/// restoring from a snapshot so that log compaction is possible.
pub trait StateMachine: Send + Sync {
    /// Apply a single committed log entry to the state machine.
    ///
    /// Returns the output bytes produced by the command, or an error
    /// if application fails.
    fn apply(&mut self, entry: &LogEntry) -> RaftResult<Vec<u8>>;

    /// Capture a point-in-time snapshot of the state machine.
    fn snapshot(&self) -> RaftResult<Vec<u8>>;

    /// Restore the state machine from a previously captured snapshot.
    fn restore(&mut self, snapshot: &[u8]) -> RaftResult<()>;
}

/// The result of applying a single committed log entry.
#[derive(Debug, Clone)]
pub struct ApplyResult {
    /// The log index of the applied entry.
    pub index: LogIndex,
    /// The term of the applied entry.
    pub term: Term,
    /// The output produced by the state machine (or empty if no callback).
    pub output: Vec<u8>,
}

/// A point-in-time snapshot of applied state, suitable for transfer to
/// followers or for local log compaction.
#[derive(Debug, Clone)]
pub struct SnapshotData {
    /// The index of the last entry included in this snapshot.
    pub last_included_index: LogIndex,
    /// The term of the last entry included in this snapshot.
    pub last_included_term: Term,
    /// The serialised state machine data.
    pub data: Vec<u8>,
}

/// A command to be replicated in the log
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The command data
    pub data: Vec<u8>,
}

impl Command {
    /// Create a new command
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create a command from a string
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        Self::new(s.as_bytes().to_vec())
    }
}

/// A log entry in the Raft log
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// The term when this entry was created
    pub term: Term,
    /// The index of this entry in the log (1-indexed)
    pub index: LogIndex,
    /// The command to be applied to the state machine
    pub command: Command,
    /// Packed fencing token (see [`crate::types::FencingToken`]).
    ///
    /// Serialised as a little-endian `u64` in WAL v2 entries.
    /// WAL v1 entries do not carry a token; zero is used as a sentinel.
    pub fencing_token: u64,
}

impl LogEntry {
    /// Create a new log entry with no fencing token (token = 0).
    pub fn new(term: Term, index: LogIndex, command: Command) -> Self {
        Self {
            term,
            index,
            command,
            fencing_token: 0,
        }
    }

    /// Create a new log entry with an explicit packed fencing token.
    pub fn with_fencing_token(
        term: Term,
        index: LogIndex,
        command: Command,
        fencing_token: u64,
    ) -> Self {
        Self {
            term,
            index,
            command,
            fencing_token,
        }
    }
}

/// In-memory log with persistent backing
pub struct RaftLog {
    /// In-memory cache of log entries
    entries: VecDeque<LogEntry>,
    /// Index of the first entry in the cache (1-indexed)
    /// If cache is empty, this is last_index + 1
    first_index: LogIndex,
    /// Index of the last entry in the log
    last_index: LogIndex,
    /// Term of the last entry in the log
    last_term: Term,
    /// Index of the highest log entry known to be committed
    commit_index: LogIndex,
    /// Index of the highest log entry applied to state machine
    applied_index: LogIndex,
    /// Snapshot metadata (index and term of last included entry)
    snapshot_index: LogIndex,
    snapshot_term: Term,
    /// Optional callback invoked when an entry is applied.
    /// Wrapped in a `Mutex` so that `RaftLog` remains `Sync` (required
    /// by `parking_lot::RwLock` in `RaftNode`).
    apply_callback:
        std::sync::Mutex<Option<Box<dyn FnMut(&LogEntry) -> RaftResult<Vec<u8>> + Send>>>,
}

impl RaftLog {
    /// Create a new empty log
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            first_index: 1,
            last_index: 0,
            last_term: 0,
            commit_index: 0,
            applied_index: 0,
            snapshot_index: 0,
            snapshot_term: 0,
            apply_callback: std::sync::Mutex::new(None),
        }
    }

    /// Append a new entry to the log
    pub fn append(&mut self, term: Term, command: Command) -> LogIndex {
        let index = self.last_index + 1;
        let entry = LogEntry::new(term, index, command);

        self.entries.push_back(entry);
        self.last_index = index;
        self.last_term = term;

        index
    }

    /// Append multiple entries to the log
    pub fn append_entries(&mut self, entries: Vec<LogEntry>) -> RaftResult<()> {
        if entries.is_empty() {
            return Ok(());
        }

        // Verify entries are sequential
        for (expected_index, entry) in (self.last_index + 1..).zip(entries.iter()) {
            if entry.index != expected_index {
                return Err(RaftError::LogInconsistency {
                    reason: format!("Expected index {}, got {}", expected_index, entry.index),
                });
            }
        }

        // Append all entries
        for entry in entries {
            self.last_index = entry.index;
            self.last_term = entry.term;
            self.entries.push_back(entry);
        }

        Ok(())
    }

    /// Get an entry by index
    pub fn get(&self, index: LogIndex) -> Option<&LogEntry> {
        if index < self.first_index || index > self.last_index {
            return None;
        }

        let offset = (index - self.first_index) as usize;
        self.entries.get(offset)
    }

    /// Get entries starting from a given index
    pub fn get_entries_from(&self, start_index: LogIndex, max_count: usize) -> Vec<LogEntry> {
        if start_index < self.first_index || start_index > self.last_index {
            return Vec::new();
        }

        let offset = (start_index - self.first_index) as usize;
        self.entries
            .iter()
            .skip(offset)
            .take(max_count)
            .cloned()
            .collect()
    }

    /// Get the term of an entry by index
    pub fn get_term(&self, index: LogIndex) -> Option<Term> {
        if index == 0 {
            return Some(0);
        }

        if index == self.snapshot_index {
            return Some(self.snapshot_term);
        }

        self.get(index).map(|entry| entry.term)
    }

    /// Get the index of the last entry
    pub fn last_index(&self) -> LogIndex {
        self.last_index
    }

    /// Get the term of the last entry
    pub fn last_term(&self) -> Term {
        self.last_term
    }

    /// Delete entries from a given index onwards
    pub fn truncate_from(&mut self, from_index: LogIndex) -> RaftResult<()> {
        if from_index <= self.snapshot_index {
            return Err(RaftError::LogInconsistency {
                reason: format!(
                    "Cannot truncate before snapshot index {}",
                    self.snapshot_index
                ),
            });
        }

        if from_index > self.last_index {
            return Ok(());
        }

        // Calculate how many entries to remove
        let offset = (from_index - self.first_index) as usize;
        self.entries.truncate(offset);

        // Update last index and term
        if let Some(last_entry) = self.entries.back() {
            self.last_index = last_entry.index;
            self.last_term = last_entry.term;
        } else {
            self.last_index = self.snapshot_index;
            self.last_term = self.snapshot_term;
        }

        Ok(())
    }

    /// Check if the log contains an entry at the given index with the given term
    pub fn matches(&self, index: LogIndex, term: Term) -> bool {
        if index == 0 {
            return term == 0;
        }

        if index == self.snapshot_index {
            return term == self.snapshot_term;
        }

        match self.get_term(index) {
            Some(t) => t == term,
            None => false,
        }
    }

    /// Get the commit index
    pub fn commit_index(&self) -> LogIndex {
        self.commit_index
    }

    /// Set the commit index (must be monotonically increasing)
    pub fn set_commit_index(&mut self, index: LogIndex) -> RaftResult<()> {
        if index < self.commit_index {
            return Err(RaftError::LogInconsistency {
                reason: format!(
                    "Cannot decrease commit index from {} to {}",
                    self.commit_index, index
                ),
            });
        }

        if index > self.last_index {
            return Err(RaftError::LogInconsistency {
                reason: format!(
                    "Cannot commit beyond last index {} (tried to commit {})",
                    self.last_index, index
                ),
            });
        }

        self.commit_index = index;
        Ok(())
    }

    /// Get the applied index
    pub fn applied_index(&self) -> LogIndex {
        self.applied_index
    }

    /// Set the applied index (must be monotonically increasing)
    pub fn set_applied_index(&mut self, index: LogIndex) -> RaftResult<()> {
        if index < self.applied_index {
            return Err(RaftError::LogInconsistency {
                reason: format!(
                    "Cannot decrease applied index from {} to {}",
                    self.applied_index, index
                ),
            });
        }

        if index > self.commit_index {
            return Err(RaftError::LogInconsistency {
                reason: format!(
                    "Cannot apply beyond commit index {} (tried to apply {})",
                    self.commit_index, index
                ),
            });
        }

        self.applied_index = index;
        Ok(())
    }

    /// Get entries that are committed but not yet applied
    pub fn get_uncommitted_entries(&self) -> Vec<LogEntry> {
        if self.applied_index >= self.commit_index {
            return Vec::new();
        }

        self.get_entries_from(self.applied_index + 1, usize::MAX)
            .into_iter()
            .take_while(|entry| entry.index <= self.commit_index)
            .collect()
    }

    /// Compact the log up to and including the given index
    ///
    /// This removes all log entries up to `index`, setting the snapshot
    /// metadata to reflect the compacted state. The compacted entries are
    /// permanently discarded.
    ///
    /// Preconditions:
    /// - `index` must be at or below the applied index (already applied to state machine)
    /// - `index` must be at or above the current snapshot_index
    pub fn compact_until(&mut self, index: LogIndex, term: Term) -> RaftResult<()> {
        if index == 0 {
            return Ok(());
        }

        if index <= self.snapshot_index {
            // Already compacted past this point
            return Ok(());
        }

        if index > self.applied_index {
            return Err(RaftError::LogInconsistency {
                reason: format!(
                    "Cannot compact beyond applied index {} (tried to compact until {})",
                    self.applied_index, index
                ),
            });
        }

        // Verify the term matches what we have
        if let Some(entry_term) = self.get_term(index) {
            if entry_term != term {
                return Err(RaftError::LogInconsistency {
                    reason: format!(
                        "Term mismatch at index {}: expected {}, found {}",
                        index, term, entry_term
                    ),
                });
            }
        }

        // Remove entries from the front of the deque
        let entries_to_remove = if index >= self.first_index {
            ((index - self.first_index) + 1) as usize
        } else {
            0
        };

        let drain_count = entries_to_remove.min(self.entries.len());
        self.entries.drain(..drain_count);

        // Update snapshot metadata
        self.snapshot_index = index;
        self.snapshot_term = term;
        self.first_index = index + 1;

        Ok(())
    }

    /// Get the current snapshot point (last compacted index)
    pub fn get_snapshot_point(&self) -> (LogIndex, Term) {
        (self.snapshot_index, self.snapshot_term)
    }

    /// Get the snapshot index
    pub fn snapshot_index(&self) -> LogIndex {
        self.snapshot_index
    }

    /// Get the snapshot term
    pub fn snapshot_term(&self) -> Term {
        self.snapshot_term
    }

    /// Reset the log state after installing a snapshot from a leader
    ///
    /// This discards all existing log entries and sets the log's base
    /// to the snapshot's last included entry.
    pub fn install_snapshot(&mut self, last_included_index: LogIndex, last_included_term: Term) {
        self.entries.clear();
        self.snapshot_index = last_included_index;
        self.snapshot_term = last_included_term;
        self.first_index = last_included_index + 1;
        self.last_index = last_included_index;
        self.last_term = last_included_term;

        // Advance commit and applied indices if needed
        if self.commit_index < last_included_index {
            self.commit_index = last_included_index;
        }
        if self.applied_index < last_included_index {
            self.applied_index = last_included_index;
        }
    }

    /// Get the number of entries since the last snapshot (useful for threshold checks)
    pub fn entries_since_snapshot(&self) -> u64 {
        self.last_index.saturating_sub(self.snapshot_index)
    }

    /// Check if the log is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the number of entries in the log
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    // ── State machine application ──────────────────────────────────

    /// Register a callback that is invoked for every entry applied to the
    /// state machine.  The callback receives the [`LogEntry`] and must
    /// return output bytes or an error.
    pub fn set_apply_callback<F>(&mut self, callback: F)
    where
        F: FnMut(&LogEntry) -> RaftResult<Vec<u8>> + Send + 'static,
    {
        let mut guard = self
            .apply_callback
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(Box::new(callback));
    }

    /// Apply all committed-but-not-yet-applied entries, invoking the
    /// registered callback (if any) for each one.
    ///
    /// Returns an [`ApplyResult`] per applied entry.  If no entries are
    /// pending, an empty vector is returned.
    pub fn apply_committed_entries(&mut self) -> RaftResult<Vec<ApplyResult>> {
        let entries = self.get_uncommitted_entries();
        let mut results = Vec::with_capacity(entries.len());
        for entry in &entries {
            let output = {
                let mut guard = self
                    .apply_callback
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(ref mut cb) = *guard {
                    cb(entry)?
                } else {
                    Vec::new()
                }
            };
            self.applied_index = entry.index;
            results.push(ApplyResult {
                index: entry.index,
                term: entry.term,
                output,
            });
        }
        Ok(results)
    }

    /// Apply up to `max_entries` committed-but-not-yet-applied entries.
    ///
    /// If the callback returns an error mid-batch the `applied_index` is
    /// rolled back to its value before the call and the error is
    /// propagated.
    pub fn apply_batch(&mut self, max_entries: usize) -> RaftResult<Vec<ApplyResult>> {
        let entries = self.get_uncommitted_entries();
        let batch: Vec<_> = entries.into_iter().take(max_entries).collect();
        let saved_applied = self.applied_index;
        let mut results = Vec::new();
        for entry in &batch {
            let invoke_result = {
                let mut guard = self
                    .apply_callback
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(ref mut cb) = *guard {
                    cb(entry)
                } else {
                    Ok(Vec::new())
                }
            };
            match invoke_result {
                Ok(output) => {
                    self.applied_index = entry.index;
                    results.push(ApplyResult {
                        index: entry.index,
                        term: entry.term,
                        output,
                    });
                }
                Err(e) => {
                    self.applied_index = saved_applied;
                    return Err(e);
                }
            }
        }
        Ok(results)
    }

    /// Create a [`SnapshotData`] reflecting the current applied state.
    ///
    /// The `data` field is left empty; callers should fill it via
    /// [`StateMachine::snapshot()`].
    pub fn create_snapshot(&self) -> RaftResult<SnapshotData> {
        let term = self
            .entries
            .iter()
            .find(|e| e.index == self.applied_index)
            .map(|e| e.term)
            .unwrap_or(self.snapshot_term);
        Ok(SnapshotData {
            last_included_index: self.applied_index,
            last_included_term: term,
            data: Vec::new(),
        })
    }

    /// Number of committed entries that have not yet been applied.
    pub fn pending_apply_count(&self) -> usize {
        if self.commit_index <= self.applied_index {
            0
        } else {
            (self.commit_index - self.applied_index) as usize
        }
    }

    /// Returns `true` when the applied index has caught up with the
    /// commit index.
    pub fn is_fully_applied(&self) -> bool {
        self.applied_index >= self.commit_index
    }
}

impl Default for RaftLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_log() {
        let log = RaftLog::new();
        assert_eq!(log.last_index(), 0);
        assert_eq!(log.last_term(), 0);
        assert_eq!(log.commit_index(), 0);
        assert_eq!(log.applied_index(), 0);
        assert!(log.is_empty());
    }

    #[test]
    fn test_append_entry() {
        let mut log = RaftLog::new();
        let cmd = Command::from_str("test");

        let index = log.append(1, cmd.clone());
        assert_eq!(index, 1);
        assert_eq!(log.last_index(), 1);
        assert_eq!(log.last_term(), 1);
        assert_eq!(log.len(), 1);

        let entry = log.get(1).expect("Entry should exist");
        assert_eq!(entry.index, 1);
        assert_eq!(entry.term, 1);
        assert_eq!(entry.command, cmd);
    }

    #[test]
    fn test_append_multiple_entries() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));
        log.append(2, Command::from_str("cmd3"));

        assert_eq!(log.last_index(), 3);
        assert_eq!(log.last_term(), 2);
        assert_eq!(log.len(), 3);
    }

    #[test]
    fn test_get_entries_from() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));
        log.append(2, Command::from_str("cmd3"));

        let entries = log.get_entries_from(2, 10);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].index, 2);
        assert_eq!(entries[1].index, 3);
    }

    #[test]
    fn test_get_entries_from_with_limit() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));
        log.append(2, Command::from_str("cmd3"));

        let entries = log.get_entries_from(1, 2);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].index, 1);
        assert_eq!(entries[1].index, 2);
    }

    #[test]
    fn test_truncate_from() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));
        log.append(2, Command::from_str("cmd3"));

        log.truncate_from(2).expect("Truncate should succeed");

        assert_eq!(log.last_index(), 1);
        assert_eq!(log.last_term(), 1);
        assert_eq!(log.len(), 1);
        assert!(log.get(2).is_none());
        assert!(log.get(3).is_none());
    }

    #[test]
    fn test_matches() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));
        log.append(2, Command::from_str("cmd3"));

        assert!(log.matches(1, 1));
        assert!(log.matches(2, 1));
        assert!(log.matches(3, 2));
        assert!(!log.matches(3, 1));
        assert!(!log.matches(4, 2));
    }

    #[test]
    fn test_commit_index() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));
        log.append(2, Command::from_str("cmd3"));

        assert_eq!(log.commit_index(), 0);

        log.set_commit_index(2).expect("Set commit should succeed");
        assert_eq!(log.commit_index(), 2);

        // Cannot decrease commit index
        let result = log.set_commit_index(1);
        assert!(result.is_err());
    }

    #[test]
    fn test_applied_index() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));
        log.set_commit_index(2).expect("Set commit should succeed");

        assert_eq!(log.applied_index(), 0);

        log.set_applied_index(1)
            .expect("Set applied should succeed");
        assert_eq!(log.applied_index(), 1);

        // Cannot apply beyond commit index
        let result = log.set_applied_index(3);
        assert!(result.is_err());
    }

    #[test]
    fn test_compact_until() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));
        log.append(2, Command::from_str("cmd3"));
        log.append(2, Command::from_str("cmd4"));
        log.append(3, Command::from_str("cmd5"));

        // Commit and apply first 3 entries
        log.set_commit_index(3).expect("Set commit should succeed");
        log.set_applied_index(3)
            .expect("Set applied should succeed");

        // Compact up to index 2
        log.compact_until(2, 1).expect("Compact should succeed");

        assert_eq!(log.snapshot_index(), 2);
        assert_eq!(log.snapshot_term(), 1);
        assert_eq!(log.len(), 3); // entries 3, 4, 5 remain
        assert!(log.get(1).is_none());
        assert!(log.get(2).is_none());
        assert!(log.get(3).is_some());
        assert_eq!(log.last_index(), 5);
    }

    #[test]
    fn test_compact_until_beyond_applied_fails() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));
        log.set_commit_index(1).expect("Set commit should succeed");
        log.set_applied_index(1)
            .expect("Set applied should succeed");

        // Try to compact beyond applied index
        let result = log.compact_until(2, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_compact_preserves_snapshot_metadata() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(2, Command::from_str("cmd2"));
        log.append(3, Command::from_str("cmd3"));
        log.set_commit_index(3).expect("Set commit should succeed");
        log.set_applied_index(3)
            .expect("Set applied should succeed");

        log.compact_until(2, 2).expect("Compact should succeed");

        let (snap_idx, snap_term) = log.get_snapshot_point();
        assert_eq!(snap_idx, 2);
        assert_eq!(snap_term, 2);

        // get_term should still work for snapshot index
        assert_eq!(log.get_term(2), Some(2));
    }

    #[test]
    fn test_entries_since_snapshot() {
        let mut log = RaftLog::new();
        for i in 1..=10 {
            log.append(1, Command::from_str(&format!("cmd{}", i)));
        }
        assert_eq!(log.entries_since_snapshot(), 10);

        log.set_commit_index(5).expect("Set commit should succeed");
        log.set_applied_index(5)
            .expect("Set applied should succeed");
        log.compact_until(5, 1).expect("Compact should succeed");

        assert_eq!(log.entries_since_snapshot(), 5);
    }

    #[test]
    fn test_install_snapshot_resets_log() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));

        log.install_snapshot(100, 5);

        assert_eq!(log.last_index(), 100);
        assert_eq!(log.last_term(), 5);
        assert_eq!(log.snapshot_index(), 100);
        assert_eq!(log.snapshot_term(), 5);
        assert_eq!(log.commit_index(), 100);
        assert_eq!(log.applied_index(), 100);
        assert!(log.is_empty());
    }

    #[test]
    fn test_get_uncommitted_entries() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("cmd1"));
        log.append(1, Command::from_str("cmd2"));
        log.append(2, Command::from_str("cmd3"));
        log.set_commit_index(2).expect("Set commit should succeed");

        let entries = log.get_uncommitted_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].index, 1);
        assert_eq!(entries[1].index, 2);

        log.set_applied_index(1)
            .expect("Set applied should succeed");
        let entries = log.get_uncommitted_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].index, 2);
    }

    // ── State machine application tests ─────────────────────────────

    #[test]
    fn test_apply_committed_sequential() {
        let mut log = RaftLog::new();
        for i in 1..=5 {
            log.append(1, Command::from_str(&format!("cmd{}", i)));
        }
        log.set_commit_index(5).expect("commit");

        let results = log.apply_committed_entries().expect("apply");
        assert_eq!(results.len(), 5);
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r.index, (i + 1) as u64);
            assert_eq!(r.term, 1);
        }
        assert_eq!(log.applied_index(), 5);
    }

    #[test]
    fn test_apply_committed_partial() {
        let mut log = RaftLog::new();
        for i in 1..=5 {
            log.append(1, Command::from_str(&format!("cmd{}", i)));
        }
        log.set_commit_index(3).expect("commit");

        let results = log.apply_committed_entries().expect("apply");
        assert_eq!(results.len(), 3);
        assert_eq!(log.applied_index(), 3);
    }

    #[test]
    fn test_apply_with_callback() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("hello"));
        log.append(1, Command::from_str("world"));
        log.set_commit_index(2).expect("commit");

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_clone = seen.clone();
        log.set_apply_callback(move |entry| {
            seen_clone
                .lock()
                .expect("lock")
                .push(entry.command.data.clone());
            Ok(Vec::new())
        });

        log.apply_committed_entries().expect("apply");
        let data = seen.lock().expect("lock");
        assert_eq!(data.len(), 2);
        assert_eq!(data[0], b"hello");
        assert_eq!(data[1], b"world");
    }

    #[test]
    fn test_apply_callback_output() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("ping"));
        log.set_commit_index(1).expect("commit");

        log.set_apply_callback(|_entry| Ok(b"pong".to_vec()));

        let results = log.apply_committed_entries().expect("apply");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].output, b"pong");
    }

    #[test]
    fn test_apply_callback_error() {
        let mut log = RaftLog::new();
        for i in 1..=5 {
            log.append(1, Command::from_str(&format!("cmd{}", i)));
        }
        log.set_commit_index(5).expect("commit");

        let mut count = 0u64;
        log.set_apply_callback(move |entry| {
            count += 1;
            if entry.index == 3 {
                return Err(RaftError::StateMachineError {
                    message: "boom".into(),
                });
            }
            let _ = count; // keep count alive
            Ok(Vec::new())
        });

        let err = log.apply_committed_entries().expect_err("should fail");
        assert!(matches!(err, RaftError::StateMachineError { .. }));
        // applied_index advanced up to entry 2 (last successful)
        assert_eq!(log.applied_index(), 2);
    }

    #[test]
    fn test_apply_batch_limited() {
        let mut log = RaftLog::new();
        for i in 1..=5 {
            log.append(1, Command::from_str(&format!("cmd{}", i)));
        }
        log.set_commit_index(5).expect("commit");

        let results = log.apply_batch(2).expect("batch");
        assert_eq!(results.len(), 2);
        assert_eq!(log.applied_index(), 2);
    }

    #[test]
    fn test_apply_batch_rollback() {
        let mut log = RaftLog::new();
        for i in 1..=5 {
            log.append(1, Command::from_str(&format!("cmd{}", i)));
        }
        log.set_commit_index(5).expect("commit");

        log.set_apply_callback(|entry| {
            if entry.index == 3 {
                return Err(RaftError::StateMachineError {
                    message: "fail".into(),
                });
            }
            Ok(Vec::new())
        });

        let err = log.apply_batch(5).expect_err("should fail");
        assert!(matches!(err, RaftError::StateMachineError { .. }));
        // Rollback: applied_index should be back to 0
        assert_eq!(log.applied_index(), 0);
    }

    #[test]
    fn test_apply_no_callback() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("x"));
        log.append(1, Command::from_str("y"));
        log.set_commit_index(2).expect("commit");

        let results = log.apply_committed_entries().expect("apply");
        assert_eq!(results.len(), 2);
        assert!(results[0].output.is_empty());
        assert!(results[1].output.is_empty());
        assert_eq!(log.applied_index(), 2);
    }

    #[test]
    fn test_apply_empty() {
        let mut log = RaftLog::new();
        // Nothing committed
        let results = log.apply_committed_entries().expect("apply");
        assert!(results.is_empty());
    }

    #[test]
    fn test_apply_idempotent() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("a"));
        log.set_commit_index(1).expect("commit");

        let r1 = log.apply_committed_entries().expect("first apply");
        assert_eq!(r1.len(), 1);

        let r2 = log.apply_committed_entries().expect("second apply");
        assert!(r2.is_empty());
    }

    #[test]
    fn test_pending_apply_count() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("a"));
        log.append(1, Command::from_str("b"));
        log.append(1, Command::from_str("c"));
        log.set_commit_index(3).expect("commit");

        assert_eq!(log.pending_apply_count(), 3);

        log.set_applied_index(1).expect("apply");
        assert_eq!(log.pending_apply_count(), 2);

        log.set_applied_index(3).expect("apply");
        assert_eq!(log.pending_apply_count(), 0);
    }

    #[test]
    fn test_is_fully_applied() {
        let mut log = RaftLog::new();
        assert!(log.is_fully_applied()); // 0 == 0

        log.append(1, Command::from_str("a"));
        log.set_commit_index(1).expect("commit");
        assert!(!log.is_fully_applied());

        log.set_applied_index(1).expect("apply");
        assert!(log.is_fully_applied());
    }

    #[test]
    fn test_create_snapshot() {
        let mut log = RaftLog::new();
        log.append(1, Command::from_str("a"));
        log.append(2, Command::from_str("b"));
        log.append(2, Command::from_str("c"));
        log.set_commit_index(3).expect("commit");
        log.set_applied_index(3).expect("apply");

        let snap = log.create_snapshot().expect("snapshot");
        assert_eq!(snap.last_included_index, 3);
        assert_eq!(snap.last_included_term, 2);
        assert!(snap.data.is_empty());
    }

    #[test]
    fn test_state_machine_trait() {
        use std::collections::HashMap;

        /// A simple key-value state machine for testing.
        struct KvStateMachine {
            store: HashMap<String, String>,
        }

        impl KvStateMachine {
            fn new() -> Self {
                Self {
                    store: HashMap::new(),
                }
            }
        }

        impl StateMachine for KvStateMachine {
            fn apply(&mut self, entry: &LogEntry) -> RaftResult<Vec<u8>> {
                let text = std::str::from_utf8(&entry.command.data).map_err(|e| {
                    RaftError::StateMachineError {
                        message: format!("invalid utf8: {}", e),
                    }
                })?;
                let parts: Vec<&str> = text.splitn(2, '=').collect();
                if parts.len() == 2 {
                    self.store
                        .insert(parts[0].to_string(), parts[1].to_string());
                    Ok(b"OK".to_vec())
                } else {
                    // GET
                    let val = self.store.get(parts[0]).cloned().unwrap_or_default();
                    Ok(val.into_bytes())
                }
            }

            fn snapshot(&self) -> RaftResult<Vec<u8>> {
                let mut buf = Vec::new();
                for (k, v) in &self.store {
                    buf.extend_from_slice(k.as_bytes());
                    buf.push(b'=');
                    buf.extend_from_slice(v.as_bytes());
                    buf.push(b'\n');
                }
                Ok(buf)
            }

            fn restore(&mut self, snapshot: &[u8]) -> RaftResult<()> {
                self.store.clear();
                let text =
                    std::str::from_utf8(snapshot).map_err(|e| RaftError::StateMachineError {
                        message: format!("invalid utf8: {}", e),
                    })?;
                for line in text.lines() {
                    let parts: Vec<&str> = line.splitn(2, '=').collect();
                    if parts.len() == 2 {
                        self.store
                            .insert(parts[0].to_string(), parts[1].to_string());
                    }
                }
                Ok(())
            }
        }

        // Apply entries through the trait
        let mut sm = KvStateMachine::new();
        let entry1 = LogEntry::new(1, 1, Command::from_str("foo=bar"));
        let entry2 = LogEntry::new(1, 2, Command::from_str("baz=qux"));

        let out1 = sm.apply(&entry1).expect("apply1");
        assert_eq!(out1, b"OK");
        let out2 = sm.apply(&entry2).expect("apply2");
        assert_eq!(out2, b"OK");

        // Snapshot
        let snap = sm.snapshot().expect("snapshot");
        assert!(!snap.is_empty());

        // Restore into a fresh machine
        let mut sm2 = KvStateMachine::new();
        sm2.restore(&snap).expect("restore");
        assert_eq!(sm2.store.get("foo").map(|s| s.as_str()), Some("bar"));
        assert_eq!(sm2.store.get("baz").map(|s| s.as_str()), Some("qux"));

        // GET via apply
        let entry3 = LogEntry::new(1, 3, Command::from_str("foo"));
        let out3 = sm2.apply(&entry3).expect("apply3");
        assert_eq!(out3, b"bar");
    }
}
