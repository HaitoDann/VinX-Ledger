//! Arbre de Merkle binaire, utilisé pour `tx_root` (SPEC §6).
//!
//! - Feuille : `H("vinx/merkle/leaf" ‖ valeur)`.
//! - Nœud : `H("vinx/merkle/node" ‖ gauche ‖ droite)`.
//! - Un nœud sans voisin à un niveau remonte tel quel au niveau supérieur. Il n'est
//!   jamais dupliqué : la duplication de Bitcoin permet à deux listes différentes
//!   d'avoir la même racine (CVE-2012-2459).
//! - Liste vide : `H("vinx/merkle/empty")`.
//!
//! Les étiquettes distinctes pour les feuilles et les nœuds empêchent de présenter
//! un nœud interne comme une feuille.

use crate::hash::{Domain, Hash, hash_tagged, hash_tagged_parts};

fn leaf(value: &Hash) -> Hash {
    hash_tagged(Domain::MerkleLeaf, value)
}

fn node(left: &Hash, right: &Hash) -> Hash {
    hash_tagged_parts(Domain::MerkleNode, &[left, right])
}

fn next_level(level: &[Hash]) -> Vec<Hash> {
    level
        .chunks(2)
        .map(|pair| match pair {
            [l, r] => node(l, r),
            // Nœud sans voisin : remonte tel quel.
            _ => pair.first().copied().unwrap_or_default(),
        })
        .collect()
}

/// Racine de Merkle des `values`, dans l'ordre.
pub fn merkle_root(values: &[Hash]) -> Hash {
    if values.is_empty() {
        return hash_tagged(Domain::MerkleEmpty, &[]);
    }
    let mut level: Vec<Hash> = values.iter().map(leaf).collect();
    while level.len() > 1 {
        level = next_level(&level);
    }
    level.first().copied().unwrap_or_default()
}

/// Côté où se trouve le voisin dans une étape de preuve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Preuve d'inclusion : les voisins rencontrés de la feuille à la racine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerkleProof {
    pub steps: Vec<(Side, Hash)>,
}

/// Preuve d'inclusion de `values[index]`, ou `None` si l'indice est hors limites.
pub fn merkle_proof(values: &[Hash], index: usize) -> Option<MerkleProof> {
    values.get(index)?;
    let mut level: Vec<Hash> = values.iter().map(leaf).collect();
    let mut idx = index;
    let mut steps = Vec::new();
    while level.len() > 1 {
        let sibling = idx ^ 1;
        if let Some(h) = level.get(sibling) {
            let side = if sibling < idx {
                Side::Left
            } else {
                Side::Right
            };
            steps.push((side, *h));
        }
        level = next_level(&level);
        idx >>= 1;
    }
    Some(MerkleProof { steps })
}

/// Vérifie que `value` est incluse sous `root`.
pub fn verify_merkle_proof(root: &Hash, value: &Hash, proof: &MerkleProof) -> bool {
    let computed = proof
        .steps
        .iter()
        .fold(leaf(value), |acc, (side, sibling)| match side {
            Side::Left => node(sibling, &acc),
            Side::Right => node(&acc, sibling),
        });
    computed == *root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(n: u8) -> Vec<Hash> {
        (0..n).map(|i| [i; 32]).collect()
    }

    #[test]
    fn empty_and_single() {
        assert_eq!(merkle_root(&[]), hash_tagged(Domain::MerkleEmpty, &[]));
        assert_eq!(merkle_root(&[[9; 32]]), leaf(&[9; 32]));
    }

    #[test]
    fn three_leaves_layout() {
        let v = values(3);
        let expected = node(&node(&leaf(&v[0]), &leaf(&v[1])), &leaf(&v[2]));
        assert_eq!(merkle_root(&v), expected);
    }

    /// CVE-2012-2459 : [a, b, c] et [a, b, c, c] doivent avoir des racines différentes.
    #[test]
    fn odd_node_is_not_duplicated() {
        let mut v = values(3);
        let r3 = merkle_root(&v);
        v.push(v[2]);
        assert_ne!(r3, merkle_root(&v));
    }

    #[test]
    fn order_matters() {
        let v = values(4);
        let mut w = v.clone();
        w.swap(0, 1);
        assert_ne!(merkle_root(&v), merkle_root(&w));
    }

    #[test]
    fn inner_node_cannot_pose_as_leaf() {
        let v = values(2);
        let inner = node(&leaf(&v[0]), &leaf(&v[1]));
        assert_ne!(merkle_root(&[inner]), merkle_root(&v));
    }

    #[test]
    fn proofs_verify_for_every_index_and_size() {
        for n in 1..=17u8 {
            let v = values(n);
            let root = merkle_root(&v);
            for (i, value) in v.iter().enumerate() {
                let proof = merkle_proof(&v, i).unwrap();
                assert!(verify_merkle_proof(&root, value, &proof), "n={n} i={i}");
                assert!(!verify_merkle_proof(&root, &[0xee; 32], &proof));
            }
            assert!(merkle_proof(&v, v.len()).is_none());
        }
    }

    #[test]
    fn tampered_proof_fails() {
        let v = values(5);
        let root = merkle_root(&v);
        let mut proof = merkle_proof(&v, 1).unwrap();
        proof.steps[0].0 = Side::Right;
        assert!(!verify_merkle_proof(&root, &v[1], &proof));
    }
}
