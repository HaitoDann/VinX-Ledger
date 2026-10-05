//! Sparse Merkle tree over 256-bit keys (ADR 0083, L1).
//!
//! A binary tree of depth 256 in which a subtree holding a single leaf is replaced by that
//! leaf and an empty subtree hashes to zero — the compressed sparse Merkle tree of
//! Diem/Aptos, whose Jellyfish variant stores the same tree 16-ary on disk. Consequences:
//!
//! - insert, update and delete touch O(log n) nodes on average (no re-indexing when an
//!   account appears, unlike a sorted-leaf tree);
//! - the root depends only on the set of `(key, value)` pairs, not on insertion order;
//! - every key has a short proof of **inclusion** (its value) or **non-inclusion**;
//! - nodes are shared (`Arc`): cloning a tree is O(1) and a clone diverges path by path.
//!
//! Hashing (BLAKE3, domain-separated):
//! - empty subtree: `[0; 32]`
//! - leaf:          `H("VINX_SMT_LEAF" ‖ key ‖ value_hash)`
//! - internal:      `H("VINX_SMT_NODE" ‖ left ‖ right)`

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{hash256, Hash32};

const LEAF_DST: &[u8] = b"VINX_SMT_LEAF";
const NODE_DST: &[u8] = b"VINX_SMT_NODE";

/// Hash of an empty subtree.
pub const SMT_EMPTY: Hash32 = [0u8; 32];

fn leaf_hash(key: &Hash32, value: &Hash32) -> Hash32 {
    let mut buf = Vec::with_capacity(LEAF_DST.len() + 64);
    buf.extend_from_slice(LEAF_DST);
    buf.extend_from_slice(key);
    buf.extend_from_slice(value);
    hash256(&buf)
}

fn node_hash(left: &Hash32, right: &Hash32) -> Hash32 {
    let mut buf = Vec::with_capacity(NODE_DST.len() + 64);
    buf.extend_from_slice(NODE_DST);
    buf.extend_from_slice(left);
    buf.extend_from_slice(right);
    hash256(&buf)
}

/// Bit `depth` of `key`, most significant first (0 = left, 1 = right).
fn bit(key: &Hash32, depth: usize) -> bool {
    (key[depth / 8] >> (7 - depth % 8)) & 1 == 1
}

#[derive(Debug)]
enum Node {
    Leaf {
        key: Hash32,
        value: Hash32,
        hash: Hash32,
    },
    Internal {
        left: Option<Arc<Node>>,
        right: Option<Arc<Node>>,
        hash: Hash32,
    },
}

impl Node {
    fn leaf(key: Hash32, value: Hash32) -> Arc<Node> {
        Arc::new(Node::Leaf {
            key,
            value,
            hash: leaf_hash(&key, &value),
        })
    }

    fn internal(left: Option<Arc<Node>>, right: Option<Arc<Node>>) -> Arc<Node> {
        let hash = node_hash(&hash_of(&left), &hash_of(&right));
        Arc::new(Node::Internal { left, right, hash })
    }

    fn hash(&self) -> Hash32 {
        match self {
            Node::Leaf { hash, .. } | Node::Internal { hash, .. } => *hash,
        }
    }
}

fn hash_of(n: &Option<Arc<Node>>) -> Hash32 {
    n.as_ref().map(|n| n.hash()).unwrap_or(SMT_EMPTY)
}

/// Two distinct leaves under a common subtree rooted at `depth`.
fn split(a: Arc<Node>, a_key: &Hash32, b: Arc<Node>, b_key: &Hash32, depth: usize) -> Arc<Node> {
    let (ba, bb) = (bit(a_key, depth), bit(b_key, depth));
    if ba != bb {
        return if ba {
            Node::internal(Some(b), Some(a))
        } else {
            Node::internal(Some(a), Some(b))
        };
    }
    let child = split(a, a_key, b, b_key, depth + 1);
    if ba {
        Node::internal(None, Some(child))
    } else {
        Node::internal(Some(child), None)
    }
}

fn insert(node: &Option<Arc<Node>>, key: Hash32, value: Hash32, depth: usize) -> Arc<Node> {
    match node.as_deref() {
        None => Node::leaf(key, value),
        Some(Node::Leaf { key: k, .. }) if *k == key => Node::leaf(key, value),
        Some(Node::Leaf { key: k, .. }) => {
            let existing = node.clone().expect("matched Some");
            let k = *k;
            split(Node::leaf(key, value), &key, existing, &k, depth)
        }
        Some(Node::Internal { left, right, .. }) => {
            if bit(&key, depth) {
                Node::internal(left.clone(), Some(insert(right, key, value, depth + 1)))
            } else {
                Node::internal(Some(insert(left, key, value, depth + 1)), right.clone())
            }
        }
    }
}

/// Removes `key`; collapses a subtree left holding a single leaf into that leaf.
fn remove(node: &Option<Arc<Node>>, key: &Hash32, depth: usize) -> Option<Arc<Node>> {
    match node.as_deref() {
        None => None,
        Some(Node::Leaf { key: k, .. }) => {
            if k == key {
                None
            } else {
                node.clone()
            }
        }
        Some(Node::Internal { left, right, .. }) => {
            let (l, r) = if bit(key, depth) {
                (left.clone(), remove(right, key, depth + 1))
            } else {
                (remove(left, key, depth + 1), right.clone())
            };
            match (&l, &r) {
                (None, None) => None,
                (Some(n), None) | (None, Some(n)) if matches!(**n, Node::Leaf { .. }) => {
                    Some(n.clone())
                }
                _ => Some(Node::internal(l, r)),
            }
        }
    }
}

/// Builds the canonical subtree of sorted, distinct `entries` sharing a prefix of `depth`.
fn build(entries: &[(Hash32, Hash32)], depth: usize) -> Option<Arc<Node>> {
    match entries {
        [] => None,
        [(k, v)] => Some(Node::leaf(*k, *v)),
        _ => {
            let split = entries.partition_point(|(k, _)| !bit(k, depth));
            let (l, r) = entries.split_at(split);
            Some(Node::internal(build(l, depth + 1), build(r, depth + 1)))
        }
    }
}

/// Proof that a key maps to a value, or to nothing, under a root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmtProof {
    /// The leaf found where the key's path ends: the key itself (inclusion), another key
    /// sharing the path (non-inclusion), or `None` for an empty subtree (non-inclusion).
    pub leaf: Option<(Hash32, Hash32)>,
    /// Sibling hashes from the root down to the leaf's depth.
    pub siblings: Vec<Hash32>,
}

impl SmtProof {
    /// Checks that `key` maps to `value` (`None` = absent) under `root`.
    pub fn verify(&self, root: &Hash32, key: &Hash32, value: Option<&Hash32>) -> bool {
        if self.siblings.len() > 256 {
            return false;
        }
        let depth = self.siblings.len();
        let mut current = match (&self.leaf, value) {
            (Some((k, v)), Some(expected)) => {
                if k != key || v != expected {
                    return false;
                }
                leaf_hash(k, v)
            }
            (Some((k, v)), None) => {
                // Another key occupies the subtree: it must share the path, and differ.
                if k == key || (0..depth).any(|d| bit(k, d) != bit(key, d)) {
                    return false;
                }
                leaf_hash(k, v)
            }
            (None, Some(_)) => return false,
            (None, None) => SMT_EMPTY,
        };
        for d in (0..depth).rev() {
            let sib = &self.siblings[d];
            current = if bit(key, d) {
                node_hash(sib, &current)
            } else {
                node_hash(&current, sib)
            };
        }
        current == *root
    }
}

/// A sparse Merkle tree mapping 256-bit keys to 256-bit value hashes.
#[derive(Clone, Debug, Default)]
pub struct SparseMerkleTree {
    root: Option<Arc<Node>>,
    len: usize,
}

impl SparseMerkleTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a tree from `(key, value)` pairs in one pass, O(n · depth) hashing with no
    /// intermediate copies. Duplicate keys keep the last value.
    pub fn from_entries(mut entries: Vec<(Hash32, Hash32)>) -> Self {
        entries.sort_by_key(|e| e.0);
        entries.reverse();
        entries.dedup_by(|a, b| a.0 == b.0); // keeps the first of each run = the last inserted
        entries.reverse();
        let len = entries.len();
        Self {
            root: build(&entries, 0),
            len,
        }
    }

    pub fn root(&self) -> Hash32 {
        hash_of(&self.root)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, key: &Hash32) -> Option<Hash32> {
        let mut node = self.root.as_deref();
        let mut depth = 0;
        while let Some(n) = node {
            match n {
                Node::Leaf { key: k, value, .. } => return (k == key).then_some(*value),
                Node::Internal { left, right, .. } => {
                    node = if bit(key, depth) { right } else { left }.as_deref();
                    depth += 1;
                }
            }
        }
        None
    }

    /// Inserts or updates `key`.
    pub fn insert(&mut self, key: Hash32, value: Hash32) {
        if self.get(&key).is_none() {
            self.len += 1;
        }
        self.root = Some(insert(&self.root, key, value, 0));
    }

    /// Removes `key` if present.
    pub fn remove(&mut self, key: &Hash32) {
        if self.get(key).is_some() {
            self.len -= 1;
            self.root = remove(&self.root, key, 0);
        }
    }

    /// Proof of the value (or absence) of `key`.
    pub fn prove(&self, key: &Hash32) -> SmtProof {
        let mut siblings = Vec::new();
        let mut node = self.root.as_deref();
        let mut depth = 0;
        loop {
            match node {
                None => {
                    return SmtProof {
                        leaf: None,
                        siblings,
                    }
                }
                Some(Node::Leaf { key: k, value, .. }) => {
                    return SmtProof {
                        leaf: Some((*k, *value)),
                        siblings,
                    }
                }
                Some(Node::Internal { left, right, .. }) => {
                    let (next, sib) = if bit(key, depth) {
                        (right, left)
                    } else {
                        (left, right)
                    };
                    siblings.push(hash_of(sib));
                    node = next.as_deref();
                    depth += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(i: u32) -> Hash32 {
        hash256(&i.to_be_bytes())
    }

    #[test]
    fn empty_tree() {
        let t = SparseMerkleTree::new();
        assert_eq!(t.root(), SMT_EMPTY);
        assert!(t.prove(&k(1)).verify(&t.root(), &k(1), None));
    }

    #[test]
    fn root_is_order_independent_and_delete_restores() {
        let mut a = SparseMerkleTree::new();
        let mut b = SparseMerkleTree::new();
        for i in 0..200 {
            a.insert(k(i), k(i + 1000));
        }
        for i in (0..200).rev() {
            b.insert(k(i), k(i + 1000));
        }
        assert_eq!(a.root(), b.root());
        assert_eq!(a.len(), 200);

        let before = a.root();
        a.insert(k(999), k(1));
        assert_ne!(a.root(), before);
        a.remove(&k(999));
        assert_eq!(
            a.root(),
            before,
            "delete must collapse back to the same tree"
        );

        for i in 0..200 {
            a.remove(&k(i));
        }
        assert_eq!(a.root(), SMT_EMPTY);
        assert!(a.is_empty());
    }

    #[test]
    fn bulk_build_matches_inserts() {
        let mut t = SparseMerkleTree::new();
        let mut entries = vec![];
        for i in 0..500 {
            t.insert(k(i), k(i + 3));
            entries.push((k(i), k(i + 3)));
        }
        entries.push((k(7), k(1))); // later duplicate wins
        t.insert(k(7), k(1));
        let b = SparseMerkleTree::from_entries(entries);
        assert_eq!(b.root(), t.root());
        assert_eq!(b.len(), t.len());
        assert_eq!(SparseMerkleTree::from_entries(vec![]).root(), SMT_EMPTY);
    }

    #[test]
    fn single_leaf_root_is_leaf_hash() {
        let mut t = SparseMerkleTree::new();
        t.insert(k(1), k(2));
        assert_eq!(t.root(), leaf_hash(&k(1), &k(2)));
    }

    #[test]
    fn proofs_of_inclusion_and_non_inclusion() {
        let mut t = SparseMerkleTree::new();
        for i in 0..100 {
            t.insert(k(i), k(i + 7));
        }
        let root = t.root();
        for i in 0..100 {
            let p = t.prove(&k(i));
            assert!(p.verify(&root, &k(i), Some(&k(i + 7))));
            assert!(!p.verify(&root, &k(i), Some(&k(i + 8))), "wrong value");
            assert!(
                !p.verify(&root, &k(i), None),
                "present key cannot be proven absent"
            );
        }
        for i in 100..200 {
            let p = t.prove(&k(i));
            assert!(p.verify(&root, &k(i), None));
            assert!(!p.verify(&root, &k(i), Some(&k(0))));
        }
        // A proof does not transfer to another root.
        let p = t.prove(&k(3));
        t.insert(k(3), k(0));
        assert!(!p.verify(&t.root(), &k(3), Some(&k(10))));
    }

    #[test]
    fn clone_is_independent() {
        let mut a = SparseMerkleTree::new();
        for i in 0..50 {
            a.insert(k(i), k(i));
        }
        let snapshot = a.clone();
        let r = a.root();
        a.insert(k(1), k(99));
        assert_eq!(snapshot.root(), r);
        assert_eq!(snapshot.get(&k(1)), Some(k(1)));
        assert_eq!(a.get(&k(1)), Some(k(99)));
    }
}
