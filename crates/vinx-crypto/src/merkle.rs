use crate::hash::{hash256, Hash32};
use serde::{Deserialize, Serialize};

// ── Incremental Merkle tree ───────────────────────────────────────────────────

/// Merkle tree that recomputes only the O(log n) path from a changed leaf to the root.
/// Full rebuild is O(n) and only required when the leaf count changes (new/removed account).
///
/// Layout: `levels[0]` = leaf hashes (sorted by address), `levels[k]` = k-th internal level.
/// `levels.last()` is always `[root]`. Odd levels pad the last leaf to its right neighbour
/// (same convention as `merkle_root`), so roots are identical for the same leaf set.
#[derive(Clone, Debug, Default)]
pub struct IncrementalMerkleTree {
    levels: Vec<Vec<Hash32>>,
}

impl IncrementalMerkleTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// O(n) full (re)build from a sorted leaf slice.
    pub fn build(leaves: &[Hash32]) -> Self {
        let mut tree = Self::new();
        tree.rebuild(leaves);
        tree
    }

    /// O(n) full rebuild — call when the leaf count changes (account added/removed).
    pub fn rebuild(&mut self, leaves: &[Hash32]) {
        self.levels.clear();
        if leaves.is_empty() {
            return;
        }
        self.levels.push(leaves.to_vec());
        loop {
            let prev = self.levels.last().unwrap();
            if prev.len() == 1 {
                break;
            }
            let len = prev.len();
            let mut next = Vec::with_capacity(len.div_ceil(2));
            for i in (0..len).step_by(2) {
                let l = prev[i];
                let r = if i + 1 < len { prev[i + 1] } else { prev[i] };
                next.push(hash_pair(l, r));
            }
            self.levels.push(next);
        }
    }

    /// O(log n) single-leaf update for an existing leaf.
    /// Propagates the change through all internal levels to the root.
    pub fn update_leaf(&mut self, idx: usize, new_hash: Hash32) {
        let Some(leaves) = self.levels.first() else {
            return;
        };
        if idx >= leaves.len() {
            return;
        }
        self.levels[0][idx] = new_hash;
        let mut node_idx = idx;
        for lvl in 0..self.levels.len() - 1 {
            let len = self.levels[lvl].len();
            let parent = node_idx / 2;
            let li = parent * 2;
            let ri = li + 1;
            let l = self.levels[lvl][li];
            let r = if ri < len {
                self.levels[lvl][ri]
            } else {
                self.levels[lvl][li]
            };
            self.levels[lvl + 1][parent] = hash_pair(l, r);
            node_idx = parent;
        }
    }

    /// Current Merkle root. Returns `[0u8; 32]` for an empty tree.
    pub fn root(&self) -> Hash32 {
        self.levels.last().map_or([0u8; 32], |top| top[0])
    }

    /// Number of leaves in the tree.
    pub fn len(&self) -> usize {
        self.levels.first().map_or(0, |l| l.len())
    }

    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    /// Leaf hashes (sorted), compatible with `merkle_proof_for`.
    pub fn leaves(&self) -> &[Hash32] {
        self.levels.first().map_or(&[], |l| l.as_slice())
    }
}

fn hash_pair(l: Hash32, r: Hash32) -> Hash32 {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(&l);
    buf[32..].copy_from_slice(&r);
    hash256(&buf)
}

/// Computes the Merkle root of a list of 32-byte leaf hashes.
///
/// - Empty list  → `[0u8; 32]`
/// - Single leaf → that leaf (no hashing)
/// - Odd count   → last leaf duplicated to make the level even
/// - Each parent → `SHA-256(left_child || right_child)`
pub fn merkle_root(leaves: &[Hash32]) -> Hash32 {
    match leaves.len() {
        0 => [0u8; 32],
        1 => leaves[0],
        _ => {
            let mut level = leaves.to_vec();
            while level.len() > 1 {
                if level.len() % 2 == 1 {
                    level.push(*level.last().unwrap());
                }
                level = level
                    .chunks_exact(2)
                    .map(|pair| {
                        let mut buf = [0u8; 64];
                        buf[..32].copy_from_slice(&pair[0]);
                        buf[32..].copy_from_slice(&pair[1]);
                        hash256(&buf)
                    })
                    .collect();
            }
            level[0]
        }
    }
}

/// A single step in a Merkle inclusion proof.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MerkleProofStep {
    pub sibling: Hash32,
    /// true = the sibling is the RIGHT child (i.e., current node is left).
    pub sibling_is_right: bool,
}

/// Generates an inclusion proof for the leaf at `index` in `leaves`.
/// Returns None if leaves is empty or index is out of bounds.
pub fn merkle_proof_for(leaves: &[Hash32], index: usize) -> Option<Vec<MerkleProofStep>> {
    if leaves.is_empty() || index >= leaves.len() {
        return None;
    }
    if leaves.len() == 1 {
        return Some(vec![]); // root IS the leaf
    }
    let mut proof = Vec::new();
    let mut level = leaves.to_vec();
    let mut idx = index;
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            level.push(*level.last().unwrap());
        }
        let sibling_idx = if idx.is_multiple_of(2) {
            idx + 1
        } else {
            idx - 1
        };
        proof.push(MerkleProofStep {
            sibling: level[sibling_idx],
            sibling_is_right: idx.is_multiple_of(2),
        });
        level = level
            .chunks_exact(2)
            .map(|p| {
                let mut buf = [0u8; 64];
                buf[..32].copy_from_slice(&p[0]);
                buf[32..].copy_from_slice(&p[1]);
                hash256(&buf)
            })
            .collect();
        idx /= 2;
    }
    Some(proof)
}

/// Verifies an inclusion proof: reconstructs root from leaf + proof steps,
/// returns true iff it matches `expected_root`.
pub fn verify_merkle_proof(
    leaf: &Hash32,
    proof: &[MerkleProofStep],
    expected_root: &Hash32,
) -> bool {
    let mut current = *leaf;
    for step in proof {
        let (left, right) = if step.sibling_is_right {
            (current, step.sibling)
        } else {
            (step.sibling, current)
        };
        let mut buf = [0u8; 64];
        buf[..32].copy_from_slice(&left);
        buf[32..].copy_from_slice(&right);
        current = hash256(&buf);
    }
    &current == expected_root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_returns_zero() {
        assert_eq!(merkle_root(&[]), [0u8; 32]);
    }

    #[test]
    fn test_single_leaf_returns_itself() {
        let leaf = hash256(b"vinx");
        assert_eq!(merkle_root(&[leaf]), leaf);
    }

    #[test]
    fn test_two_leaves() {
        let l = hash256(b"left");
        let r = hash256(b"right");
        let mut buf = [0u8; 64];
        buf[..32].copy_from_slice(&l);
        buf[32..].copy_from_slice(&r);
        let expected = hash256(&buf);
        assert_eq!(merkle_root(&[l, r]), expected);
    }

    #[test]
    fn test_deterministic() {
        let leaves: Vec<Hash32> = (0u8..6).map(|i| hash256(&[i])).collect();
        assert_eq!(merkle_root(&leaves), merkle_root(&leaves));
    }

    #[test]
    fn test_order_matters() {
        let a = hash256(b"a");
        let b = hash256(b"b");
        assert_ne!(merkle_root(&[a, b]), merkle_root(&[b, a]));
    }

    #[test]
    fn test_odd_count_is_deterministic() {
        let leaves: Vec<Hash32> = (0u8..5).map(|i| hash256(&[i])).collect();
        let r1 = merkle_root(&leaves);
        let r2 = merkle_root(&leaves);
        assert_eq!(r1, r2);
        let leaves6: Vec<Hash32> = (0u8..6).map(|i| hash256(&[i])).collect();
        assert_ne!(r1, merkle_root(&leaves6));
    }

    #[test]
    fn test_different_leaves_different_root() {
        let a: Vec<Hash32> = (0u8..4).map(|i| hash256(&[i])).collect();
        let mut b = a.clone();
        b[2] = hash256(b"changed");
        assert_ne!(merkle_root(&a), merkle_root(&b));
    }

    // ─── Merkle proof tests ──────────────────────────────────────────────────

    #[test]
    fn test_proof_empty_returns_none() {
        assert!(merkle_proof_for(&[], 0).is_none());
    }

    #[test]
    fn test_proof_out_of_bounds_returns_none() {
        let leaves: Vec<Hash32> = vec![hash256(b"a")];
        assert!(merkle_proof_for(&leaves, 1).is_none());
    }

    #[test]
    fn test_proof_single_leaf_empty_proof() {
        let leaf = hash256(b"single");
        let proof = merkle_proof_for(&[leaf], 0).unwrap();
        assert!(proof.is_empty());
        // Verify: root = leaf
        assert!(verify_merkle_proof(&leaf, &proof, &leaf));
    }

    #[test]
    fn test_proof_two_leaves_index_0() {
        let l = hash256(b"left");
        let r = hash256(b"right");
        let root = merkle_root(&[l, r]);
        let proof = merkle_proof_for(&[l, r], 0).unwrap();
        assert!(verify_merkle_proof(&l, &proof, &root));
    }

    #[test]
    fn test_proof_two_leaves_index_1() {
        let l = hash256(b"left");
        let r = hash256(b"right");
        let root = merkle_root(&[l, r]);
        let proof = merkle_proof_for(&[l, r], 1).unwrap();
        assert!(verify_merkle_proof(&r, &proof, &root));
    }

    #[test]
    fn test_proof_four_leaves_all_indices() {
        let leaves: Vec<Hash32> = (0u8..4).map(|i| hash256(&[i])).collect();
        let root = merkle_root(&leaves);
        for idx in 0..4 {
            let proof = merkle_proof_for(&leaves, idx).unwrap();
            assert!(
                verify_merkle_proof(&leaves[idx], &proof, &root),
                "proof failed for index {}",
                idx
            );
        }
    }

    #[test]
    fn test_proof_five_leaves_odd_count() {
        let leaves: Vec<Hash32> = (0u8..5).map(|i| hash256(&[i])).collect();
        let root = merkle_root(&leaves);
        for idx in 0..5 {
            let proof = merkle_proof_for(&leaves, idx).unwrap();
            assert!(
                verify_merkle_proof(&leaves[idx], &proof, &root),
                "proof failed for index {}",
                idx
            );
        }
    }

    #[test]
    fn test_proof_wrong_root_fails() {
        let leaves: Vec<Hash32> = (0u8..4).map(|i| hash256(&[i])).collect();
        let proof = merkle_proof_for(&leaves, 0).unwrap();
        let wrong_root = hash256(b"wrong");
        assert!(!verify_merkle_proof(&leaves[0], &proof, &wrong_root));
    }

    #[test]
    fn test_proof_wrong_leaf_fails() {
        let leaves: Vec<Hash32> = (0u8..4).map(|i| hash256(&[i])).collect();
        let root = merkle_root(&leaves);
        let proof = merkle_proof_for(&leaves, 0).unwrap();
        let wrong_leaf = hash256(b"wrong");
        assert!(!verify_merkle_proof(&wrong_leaf, &proof, &root));
    }

    // ─── IncrementalMerkleTree tests ─────────────────────────────────────────

    #[test]
    fn test_incremental_empty_root_is_zero() {
        let tree = IncrementalMerkleTree::new();
        assert_eq!(tree.root(), [0u8; 32]);
        assert!(tree.is_empty());
    }

    #[test]
    fn test_incremental_root_matches_batch_for_various_counts() {
        for n in 1u8..=10 {
            let leaves: Vec<Hash32> = (0..n).map(|i| hash256(&[i])).collect();
            let tree = IncrementalMerkleTree::build(&leaves);
            assert_eq!(tree.root(), merkle_root(&leaves), "root mismatch for n={n}");
        }
    }

    #[test]
    fn test_incremental_update_leaf_matches_full_rebuild() {
        let mut leaves: Vec<Hash32> = (0u8..6).map(|i| hash256(&[i])).collect();
        let mut tree = IncrementalMerkleTree::build(&leaves);
        let new_hash = hash256(b"updated");
        leaves[2] = new_hash;
        tree.update_leaf(2, new_hash);
        assert_eq!(tree.root(), merkle_root(&leaves));
    }

    #[test]
    fn test_incremental_update_all_leaves() {
        let n = 7usize;
        let mut leaves: Vec<Hash32> = (0..n).map(|i| hash256(&[i as u8])).collect();
        let mut tree = IncrementalMerkleTree::build(&leaves);
        for i in 0..n {
            let h = hash256(&[i as u8, 0xff]);
            leaves[i] = h;
            tree.update_leaf(i, h);
            assert_eq!(
                tree.root(),
                merkle_root(&leaves),
                "mismatch after update {i}"
            );
        }
    }

    #[test]
    fn test_incremental_rebuild_resets_correctly() {
        let leaves_a: Vec<Hash32> = (0u8..4).map(|i| hash256(&[i])).collect();
        let mut tree = IncrementalMerkleTree::build(&leaves_a);
        assert_eq!(tree.root(), merkle_root(&leaves_a));
        let leaves_b: Vec<Hash32> = (0u8..5).map(|i| hash256(&[i, 1])).collect();
        tree.rebuild(&leaves_b);
        assert_eq!(tree.root(), merkle_root(&leaves_b));
        assert_eq!(tree.len(), 5);
    }

    #[test]
    fn test_incremental_proof_compatible_with_batch() {
        let leaves: Vec<Hash32> = (0u8..5).map(|i| hash256(&[i])).collect();
        let tree = IncrementalMerkleTree::build(&leaves);
        assert_eq!(tree.leaves(), leaves.as_slice());
        let root = tree.root();
        for idx in 0..leaves.len() {
            let proof = merkle_proof_for(tree.leaves(), idx).unwrap();
            assert!(
                verify_merkle_proof(&leaves[idx], &proof, &root),
                "proof failed for index {idx}"
            );
        }
    }
}
