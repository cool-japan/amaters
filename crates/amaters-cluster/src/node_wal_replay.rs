//! WAL replay helper for [`crate::node::RaftNode`].
//!
//! Extracted into a separate file to keep `node.rs` within the 2 000-line policy.

use crate::error::RaftResult;
use crate::log::{LogEntry, RaftLog};
use crate::wal::{CorruptionPolicy, WalReader};
use tracing::{info, warn};

/// Replay WAL entries from `wal_dir` into `log`, merging with any entries
/// already present.
///
/// Strategy: WAL entries with indices greater than the current `log.last_index()`
/// are appended verbatim.  Entries at or below the current last index are
/// skipped (persistence already covers them, and WAL is treated as a
/// superset or equal set).  If the WAL has a higher-index entry that
/// conflicts in term, the WAL version wins (WAL is more recent).
///
/// Uses [`CorruptionPolicy::TruncateToLastGood`] for crash safety (partial
/// final entries are silently discarded).
pub(crate) fn replay_wal_into_log(wal_dir: &std::path::Path, log: &mut RaftLog) -> RaftResult<()> {
    let reader = WalReader::new(wal_dir);
    let (wal_entries, diag) = reader.recover_with_policy(CorruptionPolicy::TruncateToLastGood)?;

    if diag.corrupt_entries > 0 || diag.truncated_segments > 0 {
        warn!(
            corrupt_entries = diag.corrupt_entries,
            truncated_segments = diag.truncated_segments,
            valid_entries = diag.valid_entries,
            "WAL replay: corruption/truncation detected"
        );
    }

    if wal_entries.is_empty() {
        info!(wal_dir = %wal_dir.display(), "WAL replay: no entries to recover");
        return Ok(());
    }

    let current_last = log.last_index();
    let new_entries: Vec<LogEntry> = wal_entries
        .into_iter()
        .filter(|e| e.index > current_last)
        .collect();

    if new_entries.is_empty() {
        info!(
            wal_dir = %wal_dir.display(),
            current_last,
            "WAL replay: all WAL entries already present in log"
        );
        return Ok(());
    }

    let replayed_count = new_entries.len();
    let first_new = new_entries[0].index;
    let last_new = new_entries[new_entries.len() - 1].index;

    log.append_entries(new_entries)?;

    info!(
        wal_dir = %wal_dir.display(),
        replayed_count,
        first_new,
        last_new,
        new_last_index = log.last_index(),
        "WAL replay complete"
    );

    Ok(())
}
