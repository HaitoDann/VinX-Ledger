//! Merkle tree of a block's transactions (ADR 0083, L6 — payment receipts).
//!
//! `receipts_root` in the block header commits to the ordered transaction hashes, so a
//! payment is provable with its index and O(log n) sibling hashes instead of the whole
//! block. Unlike the generic `merkle` module, the tree is domain-separated (a leaf can
//! never be passed off as an internal node) and an odd node is *promoted*, never
//! duplicated — so `[a, b, c]` and `[a, b, c, c]` have different roots (the malleability
//! of Bitcoin's CVE-2012-2459).
//!
//! - leaf:     `H("VINX_TX_LEAF" ‖ tx_hash)`
//! - internal: `H("VINX_TX_NODE" ‖ left ‖ right)`
//! - empty:    `[0; 32]`

use crate::{hash256, Hash32};

fn leaf(tx_hash: &Hash32) -> Hash32 {
    let mut buf = Vec::with_capacity(12 + 32);
    buf.extend_from_slice(b"VINX_TX_LEAF");
    buf.extend_from_slice(tx_hash);
    hash256(&buf)
}

fn node(l: &Hash32, r: &Hash32) -> Hash32 {
    let mut buf = Vec::with_capacity(12 + 64);
    buf.extend_from_slice(b"VINX_TX_NODE");
    buf.extend_from_slice(l);
    buf.extend_from_slice(r);
    hash256(&buf)
}

fn next_level(level: &[Hash32]) -> Vec<Hash32> {
    level
        .chunks(2)
        .map(|p| {
            if p.len() == 2 {
                node(&p[0], &p[1])
            } else {
                p[0]
            }
        })
        .collect()
}

/// Root of the ordered transaction hashes.
pub fn tx_root(tx_hashes: &[Hash32]) -> Hash32 {
    if tx_hashes.is_empty() {
        return [0u8; 32];
    }
    let mut level: Vec<Hash32> = tx_hashes.iter().map(leaf).collect();
    while level.len() > 1 {
        level = next_level(&level);
    }
    level[0]
}

/// Sibling hashes (bottom-up) proving `tx_hashes[index]`; `None` if out of range.
pub fn tx_proof(tx_hashes: &[Hash32], index: usize) -> Option<Vec<Hash32>> {
    if index >= tx_hashes.len() {
        return None;
    }
    let mut level: Vec<Hash32> = tx_hashes.iter().map(leaf).collect();
    let mut idx = index;
    let mut siblings = Vec::new();
    while level.len() > 1 {
        let sib = idx ^ 1;
        if sib < level.len() {
            siblings.push(level[sib]);
        } // else: promoted, no sibling at this level
        level = next_level(&level);
        idx /= 2;
    }
    Some(siblings)
}

/// Checks that `tx_hash` is transaction `index` of `count` under `root`. `count` must
/// come from the committed header (`tx_count`), not from the prover.
pub fn verify_tx_proof(
    tx_hash: &Hash32,
    index: usize,
    count: usize,
    siblings: &[Hash32],
    root: &Hash32,
) -> bool {
    if index >= count {
        return false;
    }
    let mut current = leaf(tx_hash);
    let (mut idx, mut len, mut used) = (index, count, 0);
    while len > 1 {
        if idx ^ 1 < len {
            let Some(sib) = siblings.get(used) else {
                return false;
            };
            used += 1;
            current = if idx % 2 == 0 {
                node(&current, sib)
            } else {
                node(sib, &current)
            };
        }
        idx /= 2;
        len = len.div_ceil(2);
    }
    used == siblings.len() && current == *root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(i: u32) -> Hash32 {
        hash256(&i.to_le_bytes())
    }

    #[test]
    fn every_leaf_proves_for_every_size() {
        for n in 1..40usize {
            let hs: Vec<Hash32> = (0..n as u32).map(h).collect();
            let root = tx_root(&hs);
            for i in 0..n {
                let p = tx_proof(&hs, i).unwrap();
                assert!(verify_tx_proof(&hs[i], i, n, &p, &root), "n={n} i={i}");
                assert!(!verify_tx_proof(&h(999), i, n, &p, &root));
                if n > 1 {
                    assert!(!verify_tx_proof(&hs[i], (i + 1) % n, n, &p, &root));
                }
            }
        }
    }

    #[test]
    fn no_duplication_malleability() {
        let abc = vec![h(1), h(2), h(3)];
        let abcc = vec![h(1), h(2), h(3), h(3)];
        assert_ne!(tx_root(&abc), tx_root(&abcc));
        assert_eq!(tx_root(&[]), [0u8; 32]);
        assert_ne!(tx_root(&[h(1)]), h(1), "a leaf is domain-separated");
    }
}
