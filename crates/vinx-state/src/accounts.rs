//! Table des comptes : copie quasi gratuite, racine incrémentale, suivi des
//! comptes modifiés.
//!
//! - Les comptes sont dans une table persistante (`imbl::OrdMap`) : copier
//!   l'état pour exécuter un bloc ne copie pas le million de comptes, seulement
//!   ce qui change ensuite.
//! - Le cache de l'arbre des comptes n'est invalidé que sur le chemin des
//!   comptes modifiés.
//! - Les adresses modifiées sont notées, pour n'écrire qu'elles sur disque.

use crate::smt::{self, SmtCache};
use borsh::{BorshDeserialize, BorshSerialize};
use imbl::OrdMap;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::sync::Mutex;
use vinx_core::Account;
use vinx_crypto::{Address, Hash};

/// Cache de l'arbre et chemins à invalider avant le prochain calcul de racine.
/// Différer l'invalidation évite de la répéter pour un compte touché plusieurs
/// fois dans un bloc (la trésorerie reçoit tous les frais).
#[derive(Clone, Default)]
struct Tree {
    cache: SmtCache,
    stale: BTreeSet<Address>,
}

pub struct Accounts {
    map: OrdMap<Address, Account>,
    /// Derrière un verrou pour que la racine se calcule depuis `&self`.
    tree: Mutex<Tree>,
    dirty: BTreeSet<Address>,
    /// Somme des soldes, tenue à jour à chaque écriture.
    total: u128,
}

impl Accounts {
    pub fn new() -> Self {
        Self {
            map: OrdMap::new(),
            tree: Mutex::new(Tree::default()),
            dirty: BTreeSet::new(),
            total: 0,
        }
    }

    /// Somme de tous les soldes.
    pub fn total_balance(&self) -> u128 {
        self.total
    }

    pub fn get(&self, address: &Address) -> Option<&Account> {
        self.map.get(address)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Address, &Account)> {
        self.map.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &Account> {
        self.map.values()
    }

    /// Écrit un compte : invalide son chemin dans l'arbre et le note comme modifié.
    pub fn insert(&mut self, address: Address, account: Account) {
        self.tree
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .stale
            .insert(address);
        self.dirty.insert(address);
        self.total = self.total.saturating_add(u128::from(account.balance));
        if let Some(old) = self.map.insert(address, account) {
            self.total = self.total.saturating_sub(u128::from(old.balance));
        }
    }

    /// Racine de l'arbre des comptes (SPEC §4.1).
    pub fn root(&self) -> Hash {
        let mut tree = self.tree.lock().unwrap_or_else(|e| e.into_inner());
        let Tree { cache, stale } = &mut *tree;
        for address in std::mem::take(stale) {
            smt::invalidate(cache, &address);
        }
        smt::root(&self.map, cache)
    }

    /// Adresses modifiées depuis le dernier appel, avec leur compte actuel.
    pub fn take_dirty(&mut self) -> Vec<(Address, Account)> {
        std::mem::take(&mut self.dirty)
            .into_iter()
            .filter_map(|a| Some((a, self.map.get(&a)?.clone())))
            .collect()
    }
}

impl Default for Accounts {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Accounts {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
            tree: Mutex::new(self.tree.lock().unwrap_or_else(|e| e.into_inner()).clone()),
            dirty: self.dirty.clone(),
            total: self.total,
        }
    }
}

/// Deux tables sont égales si elles contiennent les mêmes comptes.
impl PartialEq for Accounts {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl Eq for Accounts {}

impl std::fmt::Debug for Accounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Accounts({} comptes)", self.map.len())
    }
}

impl std::ops::Index<&Address> for Accounts {
    type Output = Account;

    #[allow(clippy::panic)] // même contrat que `BTreeMap::index` : réservé aux tests
    fn index(&self, address: &Address) -> &Account {
        self.map
            .get(address)
            .unwrap_or_else(|| panic!("compte inconnu : {address}"))
    }
}

impl FromIterator<(Address, Account)> for Accounts {
    fn from_iter<I: IntoIterator<Item = (Address, Account)>>(iter: I) -> Self {
        let mut accounts = Self::new();
        for (a, acc) in iter {
            accounts.insert(a, acc);
        }
        accounts
    }
}

/// Même encodage qu'une table triée : nombre d'entrées, puis les paires dans
/// l'ordre des adresses.
impl BorshSerialize for Accounts {
    fn serialize<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let len =
            u32::try_from(self.map.len()).map_err(|_| std::io::Error::other("trop de comptes"))?;
        len.serialize(writer)?;
        for (a, acc) in &self.map {
            a.serialize(writer)?;
            acc.serialize(writer)?;
        }
        Ok(())
    }
}

impl BorshDeserialize for Accounts {
    fn deserialize_reader<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        let len = u32::deserialize_reader(reader)?;
        let mut map = OrdMap::new();
        let mut total: u128 = 0;
        let mut previous: Option<Address> = None;
        for _ in 0..len {
            let a = Address::deserialize_reader(reader)?;
            if previous.is_some_and(|p| a <= p) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "comptes non triés",
                ));
            }
            previous = Some(a);
            let account = Account::deserialize_reader(reader)?;
            total = total.saturating_add(u128::from(account.balance));
            map.insert(a, account);
        }
        Ok(Self {
            map,
            tree: Mutex::new(Tree::default()),
            dirty: BTreeSet::new(),
            total,
        })
    }
}
