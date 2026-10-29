//! Tables de l'état hors comptes (noms, séquestres, règles, mandats) :
//! copie quasi gratuite, racine incrémentale, suivi des lignes modifiées
//! (protocole 4, SPEC §4.5).
//!
//! Comme pour les comptes, chaque table est une `imbl::OrdMap` : exécuter un
//! bloc ne copie que ce qu'il modifie. Son engagement est un arbre de Merkle
//! creux indexé par `H("vinx/table/key" ‖ étiquette ‖ borsh(clé))[0..20]`,
//! dont chaque feuille porte `H(borsh(clé, valeur))` ; seules les branches
//! modifiées sont recalculées. Les lignes modifiées sont notées pour n'écrire
//! qu'elles sur disque.
//!
//! L'encodage borsh d'une table est celui d'une `BTreeMap` : nombre de lignes
//! puis paires triées. Les racines d'état des protocoles 2 et 3, qui encodent
//! les tables en entier, sont donc inchangées.

use crate::smt::{self, SmtCache};
use borsh::{BorshDeserialize, BorshSerialize};
use imbl::OrdMap;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::sync::Mutex;
use vinx_core::encode;
use vinx_crypto::{ADDRESS_LEN, Address, Domain, Hash, hash_tagged_parts};

/// Valeur rangée dans une table.
pub trait Row: Clone + PartialEq + std::fmt::Debug + BorshSerialize + BorshDeserialize {
    /// Étiquette de la table : sépare les clés de tables différentes.
    const TAG: u8;
    /// Unités de supply immobilisées par la ligne (invariant §4.3).
    fn locked(&self) -> u128 {
        0
    }
}

/// Clé d'une table.
pub trait Key: Clone + Ord + std::fmt::Debug + BorshSerialize + BorshDeserialize {}
impl<T: Clone + Ord + std::fmt::Debug + BorshSerialize + BorshDeserialize> Key for T {}

#[derive(Clone, Default)]
struct Tree {
    cache: SmtCache,
    stale: BTreeSet<Address>,
}

pub struct Table<K: Key, V: Row> {
    map: OrdMap<K, V>,
    /// Clé de l'arbre → empreinte de la ligne.
    digests: OrdMap<Address, Hash>,
    tree: Mutex<Tree>,
    dirty: BTreeSet<K>,
    locked: u128,
}

/// Clé de 20 octets d'une ligne dans l'arbre de sa table.
pub fn tree_key<K: Key>(tag: u8, key: &K) -> Address {
    let h = hash_tagged_parts(Domain::TableKey, &[&[tag], &encode(key)]);
    let mut out = [0u8; ADDRESS_LEN];
    for (o, b) in out.iter_mut().zip(h.iter()) {
        *o = *b;
    }
    Address::from_bytes(out)
}

impl<K: Key, V: Row> Table<K, V> {
    pub fn new() -> Self {
        Self {
            map: OrdMap::new(),
            digests: OrdMap::new(),
            tree: Mutex::new(Tree::default()),
            dirty: BTreeSet::new(),
            locked: 0,
        }
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: Ord + ?Sized,
        K: std::borrow::Borrow<Q>,
    {
        self.map.get(key)
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Ord + ?Sized,
        K: std::borrow::Borrow<Q>,
    {
        self.map.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.map.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.map.values()
    }

    /// Somme des [`Row::locked`] des lignes, tenue à jour à chaque écriture.
    pub fn locked(&self) -> u128 {
        self.locked
    }

    fn touch(&mut self, key: &K) -> Address {
        let tk = tree_key(V::TAG, key);
        self.tree
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .stale
            .insert(tk);
        tk
    }

    /// Écrit une ligne sans la noter comme modifiée (chargement depuis le disque).
    fn put(&mut self, key: K, value: V) -> Address {
        let tk = self.touch(&key);
        self.digests
            .insert(tk, blake3::hash(&encode(&(&key, &value))).into());
        self.locked = self.locked.saturating_add(value.locked());
        if let Some(old) = self.map.insert(key, value) {
            self.locked = self.locked.saturating_sub(old.locked());
        }
        tk
    }

    pub fn insert(&mut self, key: K, value: V) {
        self.dirty.insert(key.clone());
        self.put(key, value);
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let old = self.map.remove(key)?;
        let tk = self.touch(key);
        self.digests.remove(&tk);
        self.locked = self.locked.saturating_sub(old.locked());
        self.dirty.insert(key.clone());
        Some(old)
    }

    /// Racine de l'arbre de la table.
    pub fn root(&self) -> Hash {
        let mut tree = self.tree.lock().unwrap_or_else(|e| e.into_inner());
        let Tree { cache, stale } = &mut *tree;
        for address in std::mem::take(stale) {
            smt::invalidate(cache, &address);
        }
        smt::root_of_digests(&self.digests, cache)
    }

    /// Lignes modifiées depuis le dernier appel : `None` pour une ligne supprimée.
    pub fn take_dirty(&mut self) -> Vec<(K, Option<V>)> {
        std::mem::take(&mut self.dirty)
            .into_iter()
            .map(|k| {
                let v = self.map.get(&k).cloned();
                (k, v)
            })
            .collect()
    }

    /// Ligne lue sur le disque : ajoutée sans être notée comme modifiée.
    pub fn load(&mut self, key: K, value: V) {
        self.put(key, value);
    }

    /// Note toutes les lignes comme modifiées, pour les écrire toutes sur disque.
    pub fn mark_all_dirty(&mut self) {
        self.dirty = self.map.keys().cloned().collect();
    }
}

impl<K: Key, V: Row> Default for Table<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Key, V: Row> Clone for Table<K, V> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
            digests: self.digests.clone(),
            tree: Mutex::new(self.tree.lock().unwrap_or_else(|e| e.into_inner()).clone()),
            dirty: self.dirty.clone(),
            locked: self.locked,
        }
    }
}

/// Deux tables sont égales si elles contiennent les mêmes lignes.
impl<K: Key, V: Row> PartialEq for Table<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl<K: Key, V: Row> Eq for Table<K, V> {}

impl<K: Key, V: Row> std::fmt::Debug for Table<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Table({} lignes)", self.map.len())
    }
}

impl<K: Key, V: Row> FromIterator<(K, V)> for Table<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut t = Self::new();
        for (k, v) in iter {
            t.insert(k, v);
        }
        t
    }
}

/// Même encodage qu'une `BTreeMap<K, V>`.
impl<K: Key, V: Row> BorshSerialize for Table<K, V> {
    fn serialize<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let len = u32::try_from(self.map.len())
            .map_err(|_| std::io::Error::other("table trop grande"))?;
        len.serialize(writer)?;
        for (k, v) in &self.map {
            k.serialize(writer)?;
            v.serialize(writer)?;
        }
        Ok(())
    }
}

/// Toutes les lignes lues sont notées comme modifiées : un état reçu d'un
/// instantané ou relu d'une ancienne base est ainsi entièrement écrit.
impl<K: Key, V: Row> BorshDeserialize for Table<K, V> {
    fn deserialize_reader<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        let len = u32::deserialize_reader(reader)?;
        let mut t = Self::new();
        let mut previous: Option<K> = None;
        for _ in 0..len {
            let k = K::deserialize_reader(reader)?;
            if previous.as_ref().is_some_and(|p| k <= *p) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "lignes de table non triées",
                ));
            }
            previous = Some(k.clone());
            let v = V::deserialize_reader(reader)?;
            t.insert(k, v);
        }
        Ok(t)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::arithmetic_side_effects)] // code de test
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
    struct Note(u64);
    impl Row for Note {
        const TAG: u8 = 99;
        fn locked(&self) -> u128 {
            u128::from(self.0)
        }
    }

    #[test]
    fn root_is_incremental_and_order_independent() {
        let mut a: Table<String, Note> = Table::new();
        let mut b: Table<String, Note> = Table::new();
        for i in 0..50u64 {
            a.insert(format!("k{i}"), Note(i));
        }
        for i in (0..50u64).rev() {
            b.insert(format!("k{i}"), Note(i));
        }
        assert_eq!(a.root(), b.root());
        let before = a.root();
        a.insert("k7".into(), Note(1_000));
        assert_ne!(a.root(), before);
        a.insert("k7".into(), Note(7));
        assert_eq!(a.root(), before);
        // Un arbre recalculé de zéro donne la même racine que l'incrémental.
        let fresh: Table<String, Note> = a.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        assert_eq!(fresh.root(), a.root());
        assert_eq!(a.locked(), (0..50u128).sum::<u128>());
        a.remove(&"k3".to_string());
        assert_eq!(a.locked(), (0..50u128).sum::<u128>() - 3);
        assert_eq!(Table::<String, Note>::new().root(), smt::EMPTY);
    }

    #[test]
    fn borsh_matches_a_btreemap_and_dirty_is_tracked() {
        let mut t: Table<String, Note> = Table::new();
        t.insert("b".into(), Note(2));
        t.insert("a".into(), Note(1));
        let map: BTreeMap<String, Note> = [("a".into(), Note(1)), ("b".into(), Note(2))].into();
        assert_eq!(encode(&t), encode(&map));
        let back: Table<String, Note> = vinx_core::decode(&encode(&t)).unwrap();
        assert_eq!(back, t);
        assert_eq!(back.root(), t.root());

        let dirty = t.take_dirty();
        assert_eq!(dirty.len(), 2);
        assert!(t.take_dirty().is_empty());
        t.remove(&"a".to_string());
        assert_eq!(t.take_dirty(), vec![("a".to_string(), None)]);
        let mut loaded: Table<String, Note> = Table::new();
        loaded.load("x".into(), Note(9));
        assert!(loaded.take_dirty().is_empty());
    }
}
