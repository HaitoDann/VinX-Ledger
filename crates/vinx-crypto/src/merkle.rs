use crate::hash::{sha256, Hash32};
use serde::{Deserialize, Serialize};

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
                        sha256(&buf)
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
        let sibling_idx = if idx % 2 == 0 { idx + 1 } else { idx - 1 };
        proof.push(MerkleProofStep {
            sibling: level[sibling_idx],
            sibling_is_right: idx % 2 == 0,
        });
        level = level.chunks_exact(2).map(|p| {
            let mut buf = [0u8; 64];
            buf[..32].copy_from_slice(&p[0]);
            buf[32..].copy_from_slice(&p[1]);
            sha256(&buf)
        }).collect();
        idx /= 2;
    }
    Some(proof)
}

/// Verifies an inclusion proof: reconstructs root from leaf + proof steps,
/// returns true iff it matches `expected_root`.
pub fn verify_merkle_proof(leaf: &Hash32, proof: &[MerkleProofStep], expected_root: &Hash32) -> bool {
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
        current = sha256(&buf);
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
        let leaf = sha256(b"vinx");
        assert_eq!(merkle_root(&[leaf]), leaf);
    }

    #[test]
    fn test_two_leaves() {
        let l = sha256(b"left");
        let r = sha256(b"right");
        let mut buf = [0u8; 64];
        buf[..32].copy_from_slice(&l);
        buf[32..].copy_from_slice(&r);
        let expected = sha256(&buf);
        assert_eq!(merkle_root(&[l, r]), expected);
    }

    #[test]
    fn test_deterministic() {
        let leaves: Vec<Hash32> = (0u8..6).map(|i| sha256(&[i])).collect();
        assert_eq!(merkle_root(&leaves), merkle_root(&leaves));
    }

    #[test]
    fn test_order_matters() {
        let a = sha256(b"a");
        let b = sha256(b"b");
        assert_ne!(merkle_root(&[a, b]), merkle_root(&[b, a]));
    }

    #[test]
    fn test_odd_count_is_deterministic() {
        let leaves: Vec<Hash32> = (0u8..5).map(|i| sha256(&[i])).collect();
        let r1 = merkle_root(&leaves);
        let r2 = merkle_root(&leaves);
        assert_eq!(r1, r2);
        let leaves6: Vec<Hash32> = (0u8..6).map(|i| sha256(&[i])).collect();
        assert_ne!(r1, merkle_root(&leaves6));
    }

    #[test]
    fn test_different_leaves_different_root() {
        let a: Vec<Hash32> = (0u8..4).map(|i| sha256(&[i])).collect();
        let mut b = a.clone();
        b[2] = sha256(b"changed");
        assert_ne!(merkle_root(&a), merkle_root(&b));
    }

    // ─── Merkle proof tests ──────────────────────────────────────────────────

    #[test]
    fn test_proof_empty_returns_none() {
        assert!(merkle_proof_for(&[], 0).is_none());
    }

    #[test]
    fn test_proof_out_of_bounds_returns_none() {
        let leaves: Vec<Hash32> = vec![sha256(b"a")];
        assert!(merkle_proof_for(&leaves, 1).is_none());
    }

    #[test]
    fn test_proof_single_leaf_empty_proof() {
        let leaf = sha256(b"single");
        let proof = merkle_proof_for(&[leaf], 0).unwrap();
        assert!(proof.is_empty());
        // Verify: root = leaf
        assert!(verify_merkle_proof(&leaf, &proof, &leaf));
    }

    #[test]
    fn test_proof_two_leaves_index_0() {
        let l = sha256(b"left");
        let r = sha256(b"right");
        let root = merkle_root(&[l, r]);
        let proof = merkle_proof_for(&[l, r], 0).unwrap();
        assert!(verify_merkle_proof(&l, &proof, &root));
    }

    #[test]
    fn test_proof_two_leaves_index_1() {
        let l = sha256(b"left");
        let r = sha256(b"right");
        let root = merkle_root(&[l, r]);
        let proof = merkle_proof_for(&[l, r], 1).unwrap();
        assert!(verify_merkle_proof(&r, &proof, &root));
    }

    #[test]
    fn test_proof_four_leaves_all_indices() {
        let leaves: Vec<Hash32> = (0u8..4).map(|i| sha256(&[i])).collect();
        let root = merkle_root(&leaves);
        for idx in 0..4 {
            let proof = merkle_proof_for(&leaves, idx).unwrap();
            assert!(verify_merkle_proof(&leaves[idx], &proof, &root),
                "proof failed for index {}", idx);
        }
    }

    #[test]
    fn test_proof_five_leaves_odd_count() {
        let leaves: Vec<Hash32> = (0u8..5).map(|i| sha256(&[i])).collect();
        let root = merkle_root(&leaves);
        for idx in 0..5 {
            let proof = merkle_proof_for(&leaves, idx).unwrap();
            assert!(verify_merkle_proof(&leaves[idx], &proof, &root),
                "proof failed for index {}", idx);
        }
    }

    #[test]
    fn test_proof_wrong_root_fails() {
        let leaves: Vec<Hash32> = (0u8..4).map(|i| sha256(&[i])).collect();
        let proof = merkle_proof_for(&leaves, 0).unwrap();
        let wrong_root = sha256(b"wrong");
        assert!(!verify_merkle_proof(&leaves[0], &proof, &wrong_root));
    }

    #[test]
    fn test_proof_wrong_leaf_fails() {
        let leaves: Vec<Hash32> = (0u8..4).map(|i| sha256(&[i])).collect();
        let root = merkle_root(&leaves);
        let proof = merkle_proof_for(&leaves, 0).unwrap();
        let wrong_leaf = sha256(b"wrong");
        assert!(!verify_merkle_proof(&wrong_leaf, &proof, &root));
    }
}
