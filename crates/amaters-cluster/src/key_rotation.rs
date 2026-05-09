//! Key rotation for [`LogEncryptionKey`]s.
//!
//! Each [`crate::encryption::EncryptedPayload`] carries the [`KeyVersion`] it
//! was encrypted under so that decryption can look up the historical key
//! even after the master key has been rotated.  The current key (highest
//! version) is always used for new encryptions; older keys are retained
//! for `retention` rotations and then dropped.
//!
//! ## Background rotation task
//!
//! Automatic, time-based rotation via a `tokio` task is **deferred** to a
//! future cycle.  The [`KeyManager::rotate`] API is wired so an external
//! scheduler (for example, an admin RPC handler or a timer driven from the
//! cluster event loop) can call it directly.  See `cluster.encryption`
//! config fields in [`crate::config::NodeConfig`] for the configuration
//! surface that already exists for when the task is added.

use std::collections::BTreeMap;

use crate::encryption::LogEncryptionKey;

/// Monotonic version number for a [`LogEncryptionKey`].
///
/// Encoded into [`crate::encryption::EncryptedPayload::key_version`] so
/// that decryption can find the right historical key.  The first key has
/// version 1; version 0 is reserved as a "legacy / unset" sentinel for
/// any pre-rotation payload.
pub type KeyVersion = u32;

/// The legacy sentinel used by payloads that pre-date key rotation
/// (i.e. were serialized before the `key_version` field existed).
pub const LEGACY_KEY_VERSION: KeyVersion = 0;

// ──────────────────────────────────────────────
// KeyManager
// ──────────────────────────────────────────────

/// Manages the rolling window of [`LogEncryptionKey`]s used by an
/// [`crate::encryption::EntryEncryptor`].
///
/// The "current" key is always used for encryption; on rotation, the old
/// current key is moved into `history` and a new key takes its place.
/// `history` is bounded by `retention` (oldest entries are dropped first
/// once the bound is exceeded).  Decryption looks up the right key by the
/// [`KeyVersion`] embedded in the payload.
///
/// `retention` of `1` means only the current key is kept; rotating then
/// immediately invalidates the previous key.  `retention` of `N` means at
/// most `N - 1` historical keys plus the current key are retained at any
/// time (so we can decrypt entries from the most recent `N` versions).
///
/// `retention` is silently clamped to `>= 1` at construction time.
pub struct KeyManager {
    current_version: KeyVersion,
    current: LogEncryptionKey,
    /// Map from version → historical key (does **not** include the
    /// current key).  Bounded by `retention - 1`.
    history: BTreeMap<KeyVersion, LogEncryptionKey>,
    /// Maximum total versions kept (current + history); always `>= 1`.
    retention: usize,
}

impl KeyManager {
    /// Build a new [`KeyManager`] with `initial` as the current key at
    /// [`KeyVersion`] `1`.
    ///
    /// `retention` is clamped to `>= 1`; that is, at minimum the current
    /// key is always kept.  `retention = 3` means current + 2 historical
    /// keys are retained.
    pub fn new(initial: LogEncryptionKey, retention: usize) -> Self {
        let retention = retention.max(1);
        Self {
            current_version: 1,
            current: initial,
            history: BTreeMap::new(),
            retention,
        }
    }

    /// Rotate to a new master key, returning the new current version.
    ///
    /// The previous current key is moved into `history`.  When the
    /// combined size of (current + history) exceeds `retention`, the
    /// oldest historical entry is dropped.
    pub fn rotate(&mut self, new_key: LogEncryptionKey) -> KeyVersion {
        // Move the current key (and its version) into history.
        let prev_version = self.current_version;
        let prev_key = std::mem::replace(&mut self.current, new_key);
        self.history.insert(prev_version, prev_key);

        // Advance to the new version.
        self.current_version = self
            .current_version
            .checked_add(1)
            .unwrap_or(KeyVersion::MAX);

        // Bound history so total kept versions == retention.  We always
        // count the current key as one of the retained slots, so we keep
        // at most `retention - 1` historical entries.
        let max_history = self.retention.saturating_sub(1);
        while self.history.len() > max_history {
            // Drop the oldest entry (smallest version).
            if let Some((&oldest_version, _)) = self.history.iter().next() {
                self.history.remove(&oldest_version);
            } else {
                break;
            }
        }

        self.current_version
    }

    /// The current key, paired with its version.
    pub fn current(&self) -> (KeyVersion, &LogEncryptionKey) {
        (self.current_version, &self.current)
    }

    /// Look up the key with `version`, falling back to historical entries.
    ///
    /// Returns `None` if `version` is older than the retained window
    /// (already pruned) or has never existed.
    pub fn lookup(&self, version: KeyVersion) -> Option<&LogEncryptionKey> {
        if version == self.current_version {
            return Some(&self.current);
        }
        self.history.get(&version)
    }

    /// Number of versions currently retained (current + history).
    ///
    /// Always `>= 1` because the current key is always present, so
    /// `KeyManager` does not expose an `is_empty` method.
    pub fn version_count(&self) -> usize {
        1 + self.history.len()
    }

    /// Configured retention (always `>= 1`).
    pub fn retention(&self) -> usize {
        self.retention
    }
}

// ──────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> LogEncryptionKey {
        LogEncryptionKey::new([byte; 32])
    }

    #[test]
    fn test_key_manager_rotation_advances_version() {
        let mut mgr = KeyManager::new(key(0x01), 3);
        assert_eq!(mgr.current().0, 1, "initial current version is 1");

        let v2 = mgr.rotate(key(0x02));
        assert_eq!(v2, 2, "rotation increments to version 2");
        assert_eq!(mgr.current().0, 2);

        let v3 = mgr.rotate(key(0x03));
        assert_eq!(v3, 3);
        assert_eq!(mgr.current().0, 3);
    }

    #[test]
    fn test_key_manager_lookup_returns_current_and_history() {
        let mut mgr = KeyManager::new(key(0xaa), 3);
        let _ = mgr.rotate(key(0xbb));
        let _ = mgr.rotate(key(0xcc));

        // Current is version 3, holding 0xcc.
        // History holds versions 1 (0xaa) and 2 (0xbb).
        assert!(mgr.lookup(3).is_some());
        assert!(mgr.lookup(2).is_some());
        assert!(mgr.lookup(1).is_some());
        assert!(
            mgr.lookup(99).is_none(),
            "non-existent version returns None"
        );
    }

    #[test]
    fn test_key_manager_retention_drops_oldest() {
        // retention = 2 means current + 1 historical entry max.
        let mut mgr = KeyManager::new(key(0xaa), 2);
        let _ = mgr.rotate(key(0xbb)); // current = v2(0xbb), history = {v1(0xaa)}
        let _ = mgr.rotate(key(0xcc)); // current = v3(0xcc), history = {v2(0xbb)}; v1 dropped.

        assert!(mgr.lookup(3).is_some(), "current v3 retained");
        assert!(mgr.lookup(2).is_some(), "previous v2 retained");
        assert!(mgr.lookup(1).is_none(), "oldest v1 dropped past retention");

        let _ = mgr.rotate(key(0xdd)); // current = v4, history = {v3}; v2 dropped.
        assert!(mgr.lookup(4).is_some());
        assert!(mgr.lookup(3).is_some());
        assert!(mgr.lookup(2).is_none());
    }

    #[test]
    fn test_key_manager_retention_clamped_to_one() {
        let mut mgr = KeyManager::new(key(0x10), 0); // 0 → clamped to 1.
        assert_eq!(mgr.retention(), 1);

        // With retention 1 only the current key is kept.
        let _ = mgr.rotate(key(0x20));
        assert!(mgr.lookup(2).is_some(), "current v2 retained");
        assert!(mgr.lookup(1).is_none(), "v1 dropped immediately");
    }

    #[test]
    fn test_key_manager_version_count_grows_then_caps() {
        let mut mgr = KeyManager::new(key(0x01), 3);
        assert_eq!(mgr.version_count(), 1, "single key after construction");

        let _ = mgr.rotate(key(0x02));
        assert_eq!(mgr.version_count(), 2);

        let _ = mgr.rotate(key(0x03));
        assert_eq!(mgr.version_count(), 3);

        // Beyond retention, version_count stays capped.
        let _ = mgr.rotate(key(0x04));
        assert_eq!(mgr.version_count(), 3);

        let _ = mgr.rotate(key(0x05));
        assert_eq!(mgr.version_count(), 3);
    }
}
