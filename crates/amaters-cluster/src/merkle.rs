//! Merkle tree for batch log integrity verification.
//!
//! Builds a binary Merkle tree from a slice of 32-byte leaves using
//! [`blake3`] as the hash function.  Provides single-leaf inclusion proofs
//! that verify against the root in `O(log N)` time and space.
//!
//! ## Domain separation
//!
//! Internal nodes and leaves are hashed with disjoint domain-separation
//! prefixes to prevent second-preimage attacks where an attacker could
//! present an internal-node hash as if it were a leaf:
//!
//! - Leaves: `blake3(0x00 || leaf)` (single-byte prefix `0x00`)
//! - Internal nodes: `blake3(0x01 || left || right)` (prefix `0x01`)
//!
//! This is the standard "RFC 6962-style" domain separation — Certificate
//! Transparency uses the same scheme.
//!
//! ## Empty / odd-arity handling
//!
//! - Empty tree: root is the constant `blake3(b"amaters-merkle-empty-v1")`.
//!   This makes empty-leaf trees produce a stable, well-known root.
//! - Single leaf: root is the leaf-hashed form `blake3(0x00 || leaf)`.
//! - Odd levels: the last node is duplicated (paired with itself) when
//!   forming the parent.  This is the most common convention; document it
//!   so verifiers compute proofs consistently.
//!
//! ## Example
//!
//! ```rust
//! use amaters_cluster::merkle::MerkleTree;
//!
//! let leaves: Vec<[u8; 32]> = (0u8..4).map(|i| [i; 32]).collect();
//! let tree = MerkleTree::new(leaves.clone());
//! let root = tree.root();
//!
//! let proof = tree.proof(2).expect("index in range");
//! assert!(MerkleTree::verify(leaves[2], &proof, root));
//! ```

use crate::error::{RaftError, RaftResult};

/// Domain-separation prefix for leaf nodes.
const LEAF_PREFIX: u8 = 0x00;

/// Domain-separation prefix for internal nodes.
const INTERNAL_PREFIX: u8 = 0x01;

/// Stable root hash for an empty Merkle tree.
///
/// Computed as `blake3(b"amaters-merkle-empty-v1")` so that two empty trees
/// hash to the same root and an empty tree never collides with a non-empty
/// tree (no leaf can hash to this value because the leaf hash always
/// includes the [`LEAF_PREFIX`] byte).
fn empty_root() -> [u8; 32] {
    *blake3::hash(b"amaters-merkle-empty-v1").as_bytes()
}

/// Hash a single leaf with the leaf-domain prefix.
fn hash_leaf(leaf: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[LEAF_PREFIX]);
    hasher.update(leaf);
    *hasher.finalize().as_bytes()
}

/// Hash two child node hashes into a parent node hash.
fn hash_internal(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[INTERNAL_PREFIX]);
    hasher.update(left);
    hasher.update(right);
    *hasher.finalize().as_bytes()
}

/// Build a list of layers from leaf-hashed nodes upward to a single root.
///
/// `layers[0]` is the leaf-hash layer (length `leaves.len()`).
/// `layers[i + 1]` is computed from `layers[i]` by pairing adjacent nodes
/// (duplicating the last on odd-arity levels).  The final layer always has
/// length 1 — that's the root.
fn build_layers(leaves: &[[u8; 32]]) -> Vec<Vec<[u8; 32]>> {
    let mut layers: Vec<Vec<[u8; 32]>> = Vec::new();
    let leaf_layer: Vec<[u8; 32]> = leaves.iter().map(hash_leaf).collect();
    layers.push(leaf_layer);

    loop {
        let next: Vec<[u8; 32]> = match layers.last() {
            Some(layer) if layer.len() > 1 => layer
                .chunks(2)
                .map(|pair| {
                    let left = &pair[0];
                    // Odd arity: duplicate the last node.
                    let right = pair.get(1).unwrap_or(&pair[0]);
                    hash_internal(left, right)
                })
                .collect(),
            _ => break,
        };
        layers.push(next);
    }

    layers
}

// ──────────────────────────────────────────────
// MerkleProof
// ──────────────────────────────────────────────

/// Inclusion proof for a single leaf in a [`MerkleTree`].
///
/// `siblings[i]` is the sibling hash at level `i` of the tree (level 0 is
/// the leaf-hashed level).  `index` is the original leaf index, used
/// during verification to determine whether each sibling sits to the left
/// or right of the running hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleProof {
    /// Sibling hashes at each level, leaf-side first.
    pub siblings: Vec<[u8; 32]>,
    /// Original leaf index this proof corresponds to.
    pub index: usize,
}

// ──────────────────────────────────────────────
// MerkleTree
// ──────────────────────────────────────────────

/// A binary Merkle tree over 32-byte leaves.
///
/// Builds the tree at construction time so subsequent calls to [`root`] and
/// [`proof`] are inexpensive lookups.
///
/// [`root`]: MerkleTree::root
/// [`proof`]: MerkleTree::proof
#[derive(Debug, Clone)]
pub struct MerkleTree {
    /// Original leaves in the order they were passed at construction.
    leaves: Vec<[u8; 32]>,
    /// Root hash; cached at construction time.
    root: [u8; 32],
    /// Per-level node hashes; `layers[0]` is leaf-hashed, last layer has length 1.
    /// Empty trees have no layers — `root` is the empty-tree constant.
    layers: Vec<Vec<[u8; 32]>>,
}

impl MerkleTree {
    /// Build a new [`MerkleTree`] from the given leaves.
    ///
    /// - Empty input → root is the constant [`empty_root`].
    /// - Single-leaf input → root is `hash_leaf(leaf)`.
    /// - Multi-leaf input → balanced tree with last-leaf duplication on
    ///   odd levels, as described at the module level.
    pub fn new(leaves: Vec<[u8; 32]>) -> Self {
        if leaves.is_empty() {
            return Self {
                leaves,
                root: empty_root(),
                layers: Vec::new(),
            };
        }

        let layers = build_layers(&leaves);
        let root = match layers.last().and_then(|l| l.first()) {
            Some(r) => *r,
            None => empty_root(),
        };

        Self {
            leaves,
            root,
            layers,
        }
    }

    /// Number of leaves the tree was built over.
    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    /// `true` when the tree was built over zero leaves.
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// Root hash of the tree.
    pub fn root(&self) -> [u8; 32] {
        self.root
    }

    /// Compute the inclusion proof for the leaf at `index`.
    ///
    /// # Errors
    ///
    /// Returns [`RaftError::Other`] when `index >= self.len()`, including
    /// the empty-tree case.
    pub fn proof(&self, index: usize) -> RaftResult<MerkleProof> {
        if index >= self.leaves.len() {
            return Err(RaftError::Other {
                message: format!(
                    "MerkleTree::proof: index {} out of range (len = {})",
                    index,
                    self.leaves.len()
                ),
            });
        }

        // Single leaf: the proof is empty — the leaf hash itself is the root.
        if self.leaves.len() == 1 {
            return Ok(MerkleProof {
                siblings: Vec::new(),
                index,
            });
        }

        let mut siblings = Vec::new();
        let mut current = index;

        // Walk every layer except the topmost (root) gathering the sibling
        // at each level.
        for layer in self.layers.iter().take(self.layers.len().saturating_sub(1)) {
            let sibling_idx = if current % 2 == 0 {
                // We sit on the left; sibling is to our right (or duplicate of self).
                if current + 1 < layer.len() {
                    current + 1
                } else {
                    current
                }
            } else {
                // We sit on the right; sibling is to our left.
                current - 1
            };
            siblings.push(layer[sibling_idx]);
            current /= 2;
        }

        Ok(MerkleProof { siblings, index })
    }

    /// Verify a single-leaf inclusion proof against `root`.
    ///
    /// Returns `true` iff the supplied `leaf`, when combined with each
    /// sibling in `proof` according to its index, hashes up to `root`.
    pub fn verify(leaf: [u8; 32], proof: &MerkleProof, root: [u8; 32]) -> bool {
        let mut current = hash_leaf(&leaf);
        let mut idx = proof.index;

        for sibling in &proof.siblings {
            current = if idx % 2 == 0 {
                hash_internal(&current, sibling)
            } else {
                hash_internal(sibling, &current)
            };
            idx /= 2;
        }

        current == root
    }
}

// ──────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves_from_seed(n: usize) -> Vec<[u8; 32]> {
        (0..n)
            .map(|i| {
                let mut leaf = [0u8; 32];
                leaf[0] = i as u8;
                leaf[1] = (i >> 8) as u8;
                // Fill rest deterministically via blake3 of the index, so
                // leaves look "real" rather than trivially patterned.
                let h = blake3::hash(&i.to_le_bytes());
                leaf[2..].copy_from_slice(&h.as_bytes()[..30]);
                leaf
            })
            .collect()
    }

    #[test]
    fn test_merkle_tree_root_deterministic() {
        let leaves = leaves_from_seed(7);
        let tree_a = MerkleTree::new(leaves.clone());
        let tree_b = MerkleTree::new(leaves);
        assert_eq!(
            tree_a.root(),
            tree_b.root(),
            "two trees built from identical leaves must have identical roots"
        );
    }

    #[test]
    fn test_merkle_tree_proof_verifies() {
        let leaves = leaves_from_seed(8);
        let tree = MerkleTree::new(leaves.clone());
        let root = tree.root();

        for (i, leaf) in leaves.iter().enumerate() {
            let proof = tree.proof(i).expect("proof must be available");
            assert!(
                MerkleTree::verify(*leaf, &proof, root),
                "proof for leaf index {} must verify against the root",
                i
            );
        }
    }

    #[test]
    fn test_merkle_tree_proof_fails_on_tampered_leaf() {
        let leaves = leaves_from_seed(6);
        let tree = MerkleTree::new(leaves.clone());
        let root = tree.root();

        let proof = tree.proof(3).expect("proof at index 3");

        let mut tampered = leaves[3];
        tampered[0] ^= 0xff;

        assert!(
            !MerkleTree::verify(tampered, &proof, root),
            "tampered leaf must not verify against the original root"
        );
    }

    #[test]
    fn test_merkle_tree_empty_leaves_root() {
        let tree = MerkleTree::new(Vec::new());
        assert!(tree.is_empty(), "empty leaves yield is_empty = true");
        assert_eq!(tree.len(), 0);
        assert_eq!(
            tree.root(),
            empty_root(),
            "empty tree root must equal the well-known empty constant"
        );

        // proof on empty tree must error out cleanly.
        assert!(tree.proof(0).is_err(), "proof of empty tree must error");
    }

    #[test]
    fn test_merkle_tree_single_leaf_root() {
        let leaf = [0xa5u8; 32];
        let tree = MerkleTree::new(vec![leaf]);
        assert_eq!(tree.len(), 1);
        assert_eq!(
            tree.root(),
            hash_leaf(&leaf),
            "single-leaf tree root must equal the leaf hash"
        );

        // Proof is empty but verifies against the root.
        let proof = tree.proof(0).expect("proof at index 0");
        assert!(proof.siblings.is_empty());
        assert!(MerkleTree::verify(leaf, &proof, tree.root()));
    }

    #[test]
    fn test_merkle_tree_proof_odd_arity() {
        // 5 leaves exercises the odd-arity duplication path.
        let leaves = leaves_from_seed(5);
        let tree = MerkleTree::new(leaves.clone());
        let root = tree.root();

        for (i, leaf) in leaves.iter().enumerate() {
            let proof = tree.proof(i).expect("proof must be available");
            assert!(
                MerkleTree::verify(*leaf, &proof, root),
                "odd-arity tree: proof for leaf {} must verify",
                i
            );
        }
    }

    #[test]
    fn test_merkle_tree_proof_out_of_range() {
        let leaves = leaves_from_seed(3);
        let tree = MerkleTree::new(leaves);
        assert!(tree.proof(99).is_err());
    }

    #[test]
    fn test_merkle_tree_verify_wrong_root_fails() {
        let leaves = leaves_from_seed(4);
        let tree = MerkleTree::new(leaves.clone());

        let proof = tree.proof(1).expect("valid proof");
        let bogus_root = [0xffu8; 32];
        assert!(
            !MerkleTree::verify(leaves[1], &proof, bogus_root),
            "verification against a wrong root must fail"
        );
    }

    #[test]
    fn test_merkle_tree_domain_separation_distinguishes_layers() {
        // A 2-leaf tree with leaves [A, B] must have a root different from
        // the same A and B presented as if they were already-internal hashes.
        let a = [0x11u8; 32];
        let b = [0x22u8; 32];
        let tree = MerkleTree::new(vec![a, b]);
        let root = tree.root();

        // If we lacked domain separation, an attacker could pass
        // pre-leaf-hashed values and produce the same root from
        // hash_internal(a, b).  With prefixes, this must differ.
        let attacker_internal = hash_internal(&a, &b);
        assert_ne!(
            root, attacker_internal,
            "domain separation must distinguish leaf-input from internal-input"
        );
    }
}
