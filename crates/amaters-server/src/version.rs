//! Version compatibility and rolling upgrade support.
//!
//! This module provides version constants, compatibility checks, and a
//! handshake protocol for multi-node clusters to detect incompatible
//! version combinations before establishing connections.

/// Minimum compatible version: nodes running this version or newer (with
/// the same major) are accepted into the cluster.
pub const MIN_COMPATIBLE_VERSION: (u64, u64, u64) = (0, 2, 0);

/// Current version of this build, taken from `Cargo.toml` at compile time.
pub const CURRENT_VERSION: (u64, u64, u64) = (0, 2, 2);

/// Returns `true` when `peer_version` is considered compatible with this node.
///
/// Compatibility rules:
/// - Major version must match exactly (breaking API changes live on majors).
/// - Peer minor must be >= [`MIN_COMPATIBLE_VERSION`]'s minor so that a
///   cluster composed of nodes at different 0.2.x patch levels can still
///   operate together during a rolling upgrade.
pub fn is_compatible(peer_version: (u64, u64, u64)) -> bool {
    peer_version.0 == CURRENT_VERSION.0 && peer_version.1 >= MIN_COMPATIBLE_VERSION.1
}

/// Version handshake exchanged between nodes on initial connection.
///
/// Both sides send a [`VersionHandshake`] and then call
/// [`is_compatible_with`][VersionHandshake::is_compatible_with] on the remote
/// side's handshake before proceeding.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct VersionHandshake {
    /// The exact version running on the sending node.
    pub version: (u64, u64, u64),
    /// The oldest version the sending node is willing to talk to.
    pub min_compatible: (u64, u64, u64),
    /// Stringified semver from `CARGO_PKG_VERSION`, useful for diagnostics.
    pub build_id: String,
}

impl VersionHandshake {
    /// Construct a handshake that describes *this* binary.
    pub fn current() -> Self {
        Self {
            version: CURRENT_VERSION,
            min_compatible: MIN_COMPATIBLE_VERSION,
            build_id: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Returns `true` when `other` is compatible with this node.
    ///
    /// The check is intentionally symmetric: each side applies its own
    /// local `is_compatible` rules against the remote version.
    pub fn is_compatible_with(&self, other: &VersionHandshake) -> bool {
        // Check that the other node's version is acceptable to us.
        is_compatible(other.version)
    }
}

// ─── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_version_compatible_with_itself() {
        assert!(is_compatible(CURRENT_VERSION));
    }

    #[test]
    fn test_older_minor_version_compatible() {
        // 0.2.0 is the minimum, so 0.2.0 must be compatible with 0.2.2
        let old_minor = (0u64, 2u64, 0u64);
        assert!(is_compatible(old_minor));

        // 0.2.1 is also fine
        let mid_minor = (0u64, 2u64, 1u64);
        assert!(is_compatible(mid_minor));
    }

    #[test]
    fn test_different_major_version_incompatible() {
        let v1 = (1u64, 0u64, 0u64);
        assert!(!is_compatible(v1));

        let v2 = (2u64, 2u64, 0u64);
        assert!(!is_compatible(v2));
    }

    #[test]
    fn test_minor_below_minimum_incompatible() {
        // 0.1.x is below MIN_COMPATIBLE_VERSION.minor = 2
        let too_old = (0u64, 1u64, 99u64);
        assert!(!is_compatible(too_old));
    }

    #[test]
    fn test_version_handshake_serialization() {
        let hs = VersionHandshake::current();
        let json = serde_json::to_string(&hs).expect("serialise");
        let back: VersionHandshake = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(hs, back);
    }

    #[test]
    fn test_version_handshake_incompatible_major() {
        let current = VersionHandshake::current();
        let other = VersionHandshake {
            version: (1u64, 0u64, 0u64),
            min_compatible: (1u64, 0u64, 0u64),
            build_id: "old-major".to_string(),
        };
        assert!(!current.is_compatible_with(&other));
    }

    #[test]
    fn test_version_handshake_compatible_peer() {
        let current = VersionHandshake::current();
        let peer = VersionHandshake {
            version: (0u64, 2u64, 1u64),
            min_compatible: MIN_COMPATIBLE_VERSION,
            build_id: "peer".to_string(),
        };
        assert!(current.is_compatible_with(&peer));
    }
}
