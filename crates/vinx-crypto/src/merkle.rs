use crate::hash::{sha256, Hash32};

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
}
