//! Réserve par objet (protocole 6, SPEC §4.6).
//!
//! Chaque objet durable qu'un compte possède (jeton accepté, mandat accordé,
//! règles de compte) immobilise `per_object` VINX sur ce compte, en plus de la
//! réserve minimale. L'argent reste au compte ; il redevient disponible quand
//! l'objet est supprimé. Remplir l'état de la chaîne d'objets inutiles devient
//! ainsi coûteux, au lieu de ne coûter que des frais de transaction.

use crate::table::{Row, Table};
use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;
use vinx_core::Amount;
use vinx_crypto::Address;

/// Réserve initiale par objet : 0,1 VINX.
pub const DEFAULT_OBJECT_RESERVE: Amount = 100_000_000;

/// Nombre d'objets possédés par un compte.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Owned {
    pub count: u32,
}

impl Row for Owned {
    const TAG: u8 = 9;
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Reserves {
    /// Montant immobilisé par objet (paramètre `object_reserve`).
    pub per_object: Amount,
    /// Objets possédés, par compte ; un compte sans objet n'a pas de ligne.
    pub owned: Table<Address, Owned>,
}

impl Default for Reserves {
    fn default() -> Self {
        Self {
            per_object: DEFAULT_OBJECT_RESERVE,
            owned: Table::new(),
        }
    }
}

impl Reserves {
    pub fn count(&self, who: &Address) -> u32 {
        self.owned.get(who).map_or(0, |o| o.count)
    }

    /// Solde que le compte doit garder : 0 sans objet (un compte peut être
    /// vidé), sinon la réserve minimale plus `per_object` par objet.
    pub fn required(&self, count: u32, min_reserve: Amount) -> Amount {
        if count == 0 {
            return 0;
        }
        self.per_object
            .saturating_mul(Amount::from(count))
            .saturating_add(min_reserve)
    }

    /// Applique des variations de nombre d'objets.
    pub fn apply(&mut self, deltas: &BTreeMap<Address, i64>) {
        for (who, delta) in deltas {
            let count = i64::from(self.count(who)).saturating_add(*delta).max(0);
            match u32::try_from(count).unwrap_or(u32::MAX) {
                0 => {
                    self.owned.remove(who);
                }
                c => self.owned.insert(*who, Owned { count: c }),
            }
        }
    }
}
