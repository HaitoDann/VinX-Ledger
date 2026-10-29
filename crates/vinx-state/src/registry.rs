//! Noms `.vinx` et séquestres entre utilisateurs (protocole 2, SPEC §5.4, §5.5).
//!
//! Les tables sont des [`Table`] : copie quasi gratuite, racine incrémentale
//! à partir du protocole 4 (SPEC §4.5).

use crate::table::{Row, Table};
use borsh::{BorshDeserialize, BorshSerialize};
use vinx_core::{Amount, UNITS_PER_VINX};
use vinx_crypto::{Address, Hash};

/// Durée d'un enregistrement : une année de 365,25 jours, en secondes.
pub const NAME_PERIOD: u64 = 31_557_600;

/// Délai après l'expiration pendant lequel seul l'ancien titulaire peut renouveler.
pub const NAME_GRACE: u64 = 30 * 86_400;

/// Échéance maximale d'un séquestre : un an après son ouverture.
pub const MAX_DEAL_DURATION: u64 = NAME_PERIOD;

/// Prix initial d'un an de nom.
pub const DEFAULT_NAME_FEE: Amount = 10 * UNITS_PER_VINX;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct NameRecord {
    pub owner: Address,
    /// Horodatage Unix, en secondes.
    pub expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Deal {
    pub buyer: Address,
    pub seller: Address,
    pub arbiter: Option<Address>,
    pub amount: Amount,
    pub deadline: u64,
    pub memo: Option<Vec<u8>>,
}

impl Row for NameRecord {
    const TAG: u8 = 1;
}

impl Row for Deal {
    const TAG: u8 = 2;
    fn locked(&self) -> u128 {
        u128::from(self.amount)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Registry {
    /// Horodatage (secondes) du bloc en cours, ou du dernier bloc appliqué.
    pub now: u64,
    pub name_fee: Amount,
    pub names: Table<String, NameRecord>,
    pub deals: Table<Hash, Deal>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            now: 0,
            name_fee: DEFAULT_NAME_FEE,
            names: Table::new(),
            deals: Table::new(),
        }
    }
}

/// 3 à 32 caractères parmi `a-z`, `0-9` et `-`, sans tiret au début ni à la fin.
pub fn valid_name(name: &str) -> bool {
    let ok_len = (3..=32).contains(&name.len());
    let ok_chars = name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok_len && ok_chars && !name.starts_with('-') && !name.ends_with('-')
}

impl Registry {
    /// Montant total bloqué en séquestre.
    pub fn locked(&self) -> u128 {
        self.deals.locked()
    }

    /// Titulaire actuel d'un nom, s'il n'a pas expiré à `now`.
    pub fn resolve(&self, name: &str, now: u64) -> Option<Address> {
        self.names
            .get(name)
            .filter(|r| now < r.expires_at)
            .map(|r| r.owner)
    }

    /// Noms non expirés d'une adresse.
    pub fn names_of(&self, owner: &Address, now: u64) -> Vec<(String, u64)> {
        self.names
            .iter()
            .filter(|(_, r)| r.owner == *owner && now < r.expires_at)
            .map(|(n, r)| (n.clone(), r.expires_at))
            .collect()
    }

    /// Séquestres où l'adresse est acheteur, vendeur ou arbitre.
    pub fn deals_of(&self, who: &Address) -> Vec<(Hash, Deal)> {
        self.deals
            .iter()
            .filter(|(_, d)| d.buyer == *who || d.seller == *who || d.arbiter == Some(*who))
            .map(|(id, d)| (*id, d.clone()))
            .collect()
    }
}
