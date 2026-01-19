//! Log management for Raft consensus

use crate::error::{RaftError, RaftResult};
use crate::types::{LogIndex, Term};
use std::collections::VecDeque;

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
}

impl LogEntry {
    /// Create a new log entry
    pub fn new(term: Term, index: LogIndex, command: Command) -> Self {
        Self {
            term,
            index,
            command,
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
        let mut expected_index = self.last_index + 1;
        for entry in &entries {
            if entry.index != expected_index {
                return Err(RaftError::LogInconsistency {
                    reason: format!("Expected index {}, got {}", expected_index, entry.index),
                });
            }
            expected_index += 1;
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

    /// Check if the log is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the number of entries in the log
    pub fn len(&self) -> usize {
        self.entries.len()
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
}
