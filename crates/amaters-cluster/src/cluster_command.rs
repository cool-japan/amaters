//! Typed cluster command encoding for Raft log entries.
//!
//! All commands written into the Raft log by AmateRS components (data writes,
//! placement actions, membership changes) are encoded as a [`ClusterCommand`]
//! and stored in `Command::data`.  The encoding is:
//!
//! ```text
//! [tag: u8] [json: UTF-8 bytes]
//! ```
//!
//! The tag byte allows O(1) dispatch without deserialising the payload.
//!
//! # Variant tags
//!
//! | Tag  | Variant                    |
//! |------|---------------------------|
//! | 0x01 | `DataPut`                 |
//! | 0x02 | `DataDelete`              |
//! | 0x10 | `PlaceSplit`              |
//! | 0x11 | `PlaceMerge`              |
//! | 0x12 | `PlaceTransfer`           |
//! | 0x20 | `MembershipAdd`           |
//! | 0x21 | `MembershipRemove`        |
//!
//! # State machine boundary
//!
//! This module is responsible for *encoding* commands into the Raft log only.
//! The state machine that *applies* committed `PlaceSplit`, `PlaceMerge`, and
//! `PlaceTransfer` entries — updating the [`crate::shard::ShardRegistry`] and
//! migrating data — is a separate concern implemented in future phases.

use crate::error::{RaftError, RaftResult};
use crate::placement::PlacementAction;
use crate::shard::ShardId;
use crate::types::NodeId;

// ── Tag constants ─────────────────────────────────────────────────────────────

/// Raft log tag for a KV put (data plane).
pub const TAG_DATA_PUT: u8 = 0x01;
/// Raft log tag for a KV delete (data plane).
pub const TAG_DATA_DELETE: u8 = 0x02;
/// Raft log tag for a shard split action.
pub const TAG_PLACE_SPLIT: u8 = 0x10;
/// Raft log tag for a shard merge action.
pub const TAG_PLACE_MERGE: u8 = 0x11;
/// Raft log tag for a shard transfer action.
pub const TAG_PLACE_TRANSFER: u8 = 0x12;
/// Raft log tag for adding a cluster member.
pub const TAG_MEMBERSHIP_ADD: u8 = 0x20;
/// Raft log tag for removing a cluster member.
pub const TAG_MEMBERSHIP_REMOVE: u8 = 0x21;

// ── ClusterCommand ────────────────────────────────────────────────────────────

/// A typed, serialisable view over the raw `Command::data` bytes in a Raft log
/// entry.
///
/// Use [`ClusterCommand::encode`] to produce bytes suitable for
/// [`crate::log::Command::new`], and [`ClusterCommand::decode`] (or the
/// `TryFrom<&[u8]>` impl) to reconstruct the command on the receiver side.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ClusterCommand {
    /// A KV put (data plane, future use).
    DataPut {
        /// The key to insert or overwrite.
        key: Vec<u8>,
        /// The value to associate with the key.
        value: Vec<u8>,
    },
    /// A KV delete (data plane, future use).
    DataDelete {
        /// The key to delete.
        key: Vec<u8>,
    },
    /// Split a hot shard at `split_key`.
    PlaceSplit {
        /// The shard to split.
        shard_id: ShardId,
        /// The key at which to split, serialised as raw bytes.
        split_key: Vec<u8>,
    },
    /// Merge two adjacent cold shards into one.
    PlaceMerge {
        /// The left (lower-range) shard.
        left_shard_id: ShardId,
        /// The right (higher-range) shard.
        right_shard_id: ShardId,
    },
    /// Transfer a shard from one node to another to rebalance load.
    PlaceTransfer {
        /// The shard to move.
        shard_id: ShardId,
        /// The node currently hosting the shard.
        from_node: NodeId,
        /// The node that should receive the shard.
        to_node: NodeId,
    },
    /// Add a cluster member (membership plane).
    MembershipAdd {
        /// The node ID to admit.
        node_id: NodeId,
        /// The network address of the new node.
        address: String,
    },
    /// Remove a cluster member (membership plane).
    MembershipRemove {
        /// The node ID to evict.
        node_id: NodeId,
    },
}

impl ClusterCommand {
    /// Return the single-byte tag that identifies this variant.
    ///
    /// The tag is stored as the first byte of the encoded form and enables
    /// O(1) dispatch without deserialising the JSON payload.
    pub fn tag(&self) -> u8 {
        match self {
            ClusterCommand::DataPut { .. } => TAG_DATA_PUT,
            ClusterCommand::DataDelete { .. } => TAG_DATA_DELETE,
            ClusterCommand::PlaceSplit { .. } => TAG_PLACE_SPLIT,
            ClusterCommand::PlaceMerge { .. } => TAG_PLACE_MERGE,
            ClusterCommand::PlaceTransfer { .. } => TAG_PLACE_TRANSFER,
            ClusterCommand::MembershipAdd { .. } => TAG_MEMBERSHIP_ADD,
            ClusterCommand::MembershipRemove { .. } => TAG_MEMBERSHIP_REMOVE,
        }
    }

    /// Encode the command as `[tag_byte][json_bytes]`.
    ///
    /// The result is suitable for use as the `data` field of a
    /// [`crate::log::Command`].
    pub fn encode(&self) -> Vec<u8> {
        let tag = self.tag();
        let json = serde_json::to_vec(self)
            .expect("ClusterCommand serialization must not fail for well-formed data");
        let mut out = Vec::with_capacity(1 + json.len());
        out.push(tag);
        out.extend_from_slice(&json);
        out
    }

    /// Decode a command from `[tag_byte][json_bytes]`.
    ///
    /// Returns an error if the byte slice is empty, the tag is unknown, or the
    /// JSON body cannot be deserialised.
    pub fn decode(bytes: &[u8]) -> RaftResult<Self> {
        let (&tag, json_tail) = bytes.split_first().ok_or_else(|| RaftError::Other {
            message: "ClusterCommand::decode: empty byte slice".to_owned(),
        })?;

        // Verify that the tag byte is one we know about before paying the cost
        // of JSON deserialisation.
        match tag {
            TAG_DATA_PUT
            | TAG_DATA_DELETE
            | TAG_PLACE_SPLIT
            | TAG_PLACE_MERGE
            | TAG_PLACE_TRANSFER
            | TAG_MEMBERSHIP_ADD
            | TAG_MEMBERSHIP_REMOVE => {}
            other => {
                return Err(RaftError::Other {
                    message: format!("ClusterCommand::decode: unknown tag byte 0x{:02x}", other),
                });
            }
        }

        let cmd: ClusterCommand =
            serde_json::from_slice(json_tail).map_err(|e| RaftError::Other {
                message: format!("ClusterCommand::decode: JSON deserialisation failed: {}", e),
            })?;

        // Consistency check: the tag embedded in the JSON must match the
        // leading byte.  A mismatch indicates data corruption.
        if cmd.tag() != tag {
            return Err(RaftError::Other {
                message: format!(
                    "ClusterCommand::decode: tag mismatch — header byte 0x{:02x} but JSON \
                     deserialised to variant with tag 0x{:02x}",
                    tag,
                    cmd.tag(),
                ),
            });
        }

        Ok(cmd)
    }

    /// Convert a [`crate::placement::PlacementAction`] into the corresponding [`ClusterCommand`].
    ///
    /// | `PlacementAction` variant | `ClusterCommand` variant |
    /// |--------------------------|--------------------------|
    /// | `Split`                  | `PlaceSplit`             |
    /// | `Merge`                  | `PlaceMerge`             |
    /// | `Transfer`               | `PlaceTransfer`          |
    pub fn from_placement_action(action: &PlacementAction) -> Self {
        match action {
            PlacementAction::Split {
                shard_id,
                split_key,
            } => ClusterCommand::PlaceSplit {
                shard_id: *shard_id,
                // Serialise the `Key` as its raw byte representation.
                split_key: split_key.as_bytes().to_vec(),
            },
            PlacementAction::Merge {
                left_shard_id,
                right_shard_id,
            } => ClusterCommand::PlaceMerge {
                left_shard_id: *left_shard_id,
                right_shard_id: *right_shard_id,
            },
            PlacementAction::Transfer {
                shard_id,
                from_node,
                to_node,
            } => ClusterCommand::PlaceTransfer {
                shard_id: *shard_id,
                from_node: *from_node,
                to_node: *to_node,
            },
        }
    }
}

// ── TryFrom<&[u8]> ───────────────────────────────────────────────────────────

impl TryFrom<&[u8]> for ClusterCommand {
    type Error = RaftError;

    /// Delegate to [`ClusterCommand::decode`].
    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        ClusterCommand::decode(bytes)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard::KeyRange;
    use amaters_core::Key;

    // ── round-trip helpers ────────────────────────────────────────────────────

    fn assert_round_trip(cmd: ClusterCommand) {
        let encoded = cmd.encode();
        // Verify the leading tag byte matches.
        assert_eq!(
            encoded[0],
            cmd.tag(),
            "leading tag byte must match cmd.tag()"
        );
        // Decode and compare.
        let decoded = ClusterCommand::decode(&encoded)
            .expect("decode must succeed for a freshly encoded command");
        assert_eq!(cmd, decoded, "decoded command must equal original");
        // Also test TryFrom.
        let via_try_from = ClusterCommand::try_from(encoded.as_slice())
            .expect("TryFrom must succeed for a freshly encoded command");
        assert_eq!(cmd, via_try_from, "TryFrom result must equal original");
    }

    // ── individual variant round-trips ────────────────────────────────────────

    #[test]
    fn test_encode_decode_place_split() {
        let cmd = ClusterCommand::PlaceSplit {
            shard_id: 42,
            split_key: vec![0x80, 0x00, 0xFF],
        };
        assert_round_trip(cmd);
    }

    #[test]
    fn test_encode_decode_place_merge() {
        let cmd = ClusterCommand::PlaceMerge {
            left_shard_id: 7,
            right_shard_id: 8,
        };
        assert_round_trip(cmd);
    }

    #[test]
    fn test_encode_decode_place_transfer() {
        let cmd = ClusterCommand::PlaceTransfer {
            shard_id: 99,
            from_node: 1,
            to_node: 3,
        };
        assert_round_trip(cmd);
    }

    #[test]
    fn test_encode_decode_data_put() {
        let cmd = ClusterCommand::DataPut {
            key: b"hello".to_vec(),
            value: b"world".to_vec(),
        };
        assert_round_trip(cmd);
    }

    #[test]
    fn test_encode_decode_data_delete() {
        let cmd = ClusterCommand::DataDelete {
            key: b"goodbye".to_vec(),
        };
        assert_round_trip(cmd);
    }

    #[test]
    fn test_encode_decode_membership_add() {
        let cmd = ClusterCommand::MembershipAdd {
            node_id: 5,
            address: "192.168.1.10:7878".to_owned(),
        };
        assert_round_trip(cmd);
    }

    #[test]
    fn test_encode_decode_membership_remove() {
        let cmd = ClusterCommand::MembershipRemove { node_id: 5 };
        assert_round_trip(cmd);
    }

    // ── from_placement_action — all three variants ─────────────────────────────

    #[test]
    fn test_from_placement_action_all_variants() {
        // Split
        let split_key = Key::from_slice(&[0x80u8]);
        let split_action = PlacementAction::Split {
            shard_id: 11,
            split_key: split_key.clone(),
        };
        let split_cmd = ClusterCommand::from_placement_action(&split_action);
        assert_eq!(
            split_cmd,
            ClusterCommand::PlaceSplit {
                shard_id: 11,
                split_key: split_key.as_bytes().to_vec(),
            },
            "Split action must map to PlaceSplit with key bytes"
        );
        // Must also encode/decode cleanly.
        assert_round_trip(split_cmd);

        // Merge
        let merge_action = PlacementAction::Merge {
            left_shard_id: 3,
            right_shard_id: 4,
        };
        let merge_cmd = ClusterCommand::from_placement_action(&merge_action);
        assert_eq!(
            merge_cmd,
            ClusterCommand::PlaceMerge {
                left_shard_id: 3,
                right_shard_id: 4,
            },
            "Merge action must map to PlaceMerge"
        );
        assert_round_trip(merge_cmd);

        // Transfer
        let transfer_action = PlacementAction::Transfer {
            shard_id: 17,
            from_node: 2,
            to_node: 5,
        };
        let transfer_cmd = ClusterCommand::from_placement_action(&transfer_action);
        assert_eq!(
            transfer_cmd,
            ClusterCommand::PlaceTransfer {
                shard_id: 17,
                from_node: 2,
                to_node: 5,
            },
            "Transfer action must map to PlaceTransfer"
        );
        assert_round_trip(transfer_cmd);
    }

    // ── error handling ────────────────────────────────────────────────────────

    #[test]
    fn test_decode_empty_bytes_is_error() {
        let result = ClusterCommand::decode(&[]);
        assert!(result.is_err(), "decoding empty bytes must return an error");
    }

    #[test]
    fn test_decode_unknown_tag_is_error() {
        // Tag 0xFF is not assigned to any variant.
        let bytes = [0xFF, b'{', b'}'];
        let result = ClusterCommand::decode(&bytes);
        assert!(result.is_err(), "unknown tag byte must return an error");
    }

    #[test]
    fn test_tag_bytes_are_unique() {
        // Construct one of every variant and verify tags are all distinct.
        let range =
            KeyRange::new(Key::from_slice(&[0u8]), Key::from_slice(&[255u8])).expect("valid range");
        let _ = range; // used to prove KeyRange construction; actual tags come from enum

        let variants: Vec<ClusterCommand> = vec![
            ClusterCommand::DataPut {
                key: vec![1],
                value: vec![2],
            },
            ClusterCommand::DataDelete { key: vec![3] },
            ClusterCommand::PlaceSplit {
                shard_id: 1,
                split_key: vec![0x80],
            },
            ClusterCommand::PlaceMerge {
                left_shard_id: 1,
                right_shard_id: 2,
            },
            ClusterCommand::PlaceTransfer {
                shard_id: 1,
                from_node: 1,
                to_node: 2,
            },
            ClusterCommand::MembershipAdd {
                node_id: 1,
                address: "a:1".to_owned(),
            },
            ClusterCommand::MembershipRemove { node_id: 1 },
        ];

        let mut tags = std::collections::HashSet::new();
        for v in &variants {
            let inserted = tags.insert(v.tag());
            assert!(
                inserted,
                "duplicate tag 0x{:02x} found for {:?}",
                v.tag(),
                v
            );
        }
        assert_eq!(
            tags.len(),
            variants.len(),
            "each variant must have a unique tag"
        );
    }
}
