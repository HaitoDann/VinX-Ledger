//! Actifs natifs : jetons créés par les utilisateurs (protocole 5, SPEC §5.11).
//!
//! Un actif porte le symbole d'un nom `.vinx` que possédait son émetteur à la
//! création (`eurv.vinx` → `EURV`) : un symbole ne sert qu'une fois. Les
//! pouvoirs de l'émetteur (émission, gel, reprise forcée) sont choisis à la
//! création et ne peuvent ensuite qu'être abandonnés, jamais ajoutés : chaque
//! détenteur sait d'avance ce que l'émetteur a le droit de faire.
//!
//! On ne reçoit un actif qu'après l'avoir accepté (`AcceptAsset`) : personne
//! ne peut remplir un compte, ni l'état, de jetons non sollicités.

use crate::table::{Row, Table};
use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;
use vinx_core::{Amount, AssetPowers, UNITS_PER_VINX};
use vinx_crypto::Address;

/// Prix initial de la création d'un actif.
pub const DEFAULT_ASSET_FEE: Amount = 100 * UNITS_PER_VINX;

/// Décimales maximales d'un actif : comme le VINX.
pub const MAX_ASSET_DECIMALS: u8 = 9;

/// Longueur maximale du nom affiché d'un actif, en octets.
pub const MAX_ASSET_NAME: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Asset {
    pub issuer: Address,
    /// Nom affiché, par exemple « Euro VinX ».
    pub name: String,
    pub decimals: u8,
    /// En circulation, en unités de l'actif.
    pub supply: Amount,
    /// Plafond de `supply`, s'il y en a un.
    pub max_supply: Option<Amount>,
    /// Pouvoirs encore détenus par l'émetteur.
    pub powers: AssetPowers,
    /// Gel de tout l'actif : plus aucun transfert ni destruction.
    pub frozen: bool,
}

/// Solde d'un compte dans un actif.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Holding {
    pub amount: Amount,
    pub frozen: bool,
}

impl Row for Asset {
    const TAG: u8 = 5;
}

impl Row for Holding {
    const TAG: u8 = 6;
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Assets {
    /// Prix de la création d'un actif, versé à la trésorerie.
    pub fee: Amount,
    /// Actifs par symbole.
    pub assets: Table<String, Asset>,
    /// Soldes par (symbole, compte). Une ligne, même vide, vaut accord du
    /// compte pour recevoir l'actif : on ne reçoit que ce qu'on a accepté.
    pub holdings: Table<(String, Address), Holding>,
}

impl Default for Assets {
    fn default() -> Self {
        Self {
            fee: DEFAULT_ASSET_FEE,
            assets: Table::new(),
            holdings: Table::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AssetError {
    #[error("actif invalide : nom affiché, décimales (0 à 9) ou supply")]
    Invalid,
    #[error("le symbole doit être un nom .vinx de l'émetteur, non expiré")]
    NotSymbolOwner,
    #[error("ce symbole est déjà utilisé par un actif")]
    SymbolTaken,
    #[error("actif inconnu")]
    Unknown,
    #[error("réservé à l'émetteur de l'actif")]
    NotIssuer,
    #[error("l'émetteur n'a pas (ou plus) ce pouvoir sur l'actif")]
    PowerMissing,
    #[error("au-delà de la supply maximale de l'actif")]
    MaxSupply,
    #[error("solde de l'actif insuffisant")]
    InsufficientBalance,
    #[error("actif ou compte gelé")]
    Frozen,
    #[error("ce compte n'a pas accepté cet actif")]
    NotAccepted,
    #[error("retirer un actif exige un solde nul")]
    NotEmpty,
}

/// Symbole d'un actif : le nom `.vinx` en majuscules.
pub fn symbol_of(name: &str) -> String {
    name.to_ascii_uppercase()
}

impl Assets {
    pub fn balance(&self, symbol: &str, who: &Address) -> Amount {
        self.holdings
            .get(&(symbol.to_owned(), *who))
            .map_or(0, |h| h.amount)
    }

    /// Actifs détenus par une adresse, ou gelés pour elle.
    pub fn holdings_of(&self, who: &Address) -> Vec<(String, Holding)> {
        self.holdings
            .iter()
            .filter(|((_, a), _)| a == who)
            .map(|((s, _), h)| (s.clone(), h.clone()))
            .collect()
    }
}

/// Modifications d'actifs d'une transaction, appliquées seulement si elle
/// réussit, comme celles des comptes. `None` : pas de ligne, l'actif n'est
/// pas accepté par ce compte.
#[derive(Default)]
pub struct AssetChanges {
    pub assets: Vec<(String, Asset)>,
    holdings: BTreeMap<(String, Address), Option<Holding>>,
}

impl AssetChanges {
    fn holding(&mut self, base: &Assets, symbol: &str, who: Address) -> &mut Option<Holding> {
        self.holdings
            .entry((symbol.to_owned(), who))
            .or_insert_with_key(|k| base.holdings.get(k).cloned())
    }

    /// Le compte a-t-il accepté l'actif ?
    pub fn accepted(&mut self, base: &Assets, symbol: &str, who: Address) -> bool {
        self.holding(base, symbol, who).is_some()
    }

    /// Accepte l'actif : crée une ligne vide si le compte n'en a pas.
    pub fn accept(&mut self, base: &Assets, symbol: &str, who: Address) {
        self.holding(base, symbol, who)
            .get_or_insert_with(Holding::default);
    }

    /// Retire l'actif : seulement avec un solde nul et un compte non gelé.
    pub fn leave(&mut self, base: &Assets, symbol: &str, who: Address) -> Result<(), AssetError> {
        let slot = self.holding(base, symbol, who);
        match slot {
            Some(h) if h.frozen => Err(AssetError::Frozen),
            Some(h) if h.amount > 0 => Err(AssetError::NotEmpty),
            _ => {
                *slot = None;
                Ok(())
            }
        }
    }

    pub fn is_frozen(&mut self, base: &Assets, symbol: &str, who: Address) -> bool {
        self.holding(base, symbol, who)
            .as_ref()
            .is_some_and(|h| h.frozen)
    }

    pub fn set_frozen(
        &mut self,
        base: &Assets,
        symbol: &str,
        who: Address,
        frozen: bool,
    ) -> Result<(), AssetError> {
        let h = self
            .holding(base, symbol, who)
            .as_mut()
            .ok_or(AssetError::NotAccepted)?;
        h.frozen = frozen;
        Ok(())
    }

    pub fn debit(
        &mut self,
        base: &Assets,
        symbol: &str,
        who: Address,
        amount: Amount,
    ) -> Result<(), AssetError> {
        let h = self
            .holding(base, symbol, who)
            .as_mut()
            .ok_or(AssetError::InsufficientBalance)?;
        h.amount = h
            .amount
            .checked_sub(amount)
            .ok_or(AssetError::InsufficientBalance)?;
        Ok(())
    }

    /// Crédite un compte qui a accepté l'actif.
    pub fn credit(
        &mut self,
        base: &Assets,
        symbol: &str,
        who: Address,
        amount: Amount,
    ) -> Result<(), AssetError> {
        let h = self
            .holding(base, symbol, who)
            .as_mut()
            .ok_or(AssetError::NotAccepted)?;
        // Les soldes sont bornés par la supply, elle-même un `Amount`.
        h.amount = h.amount.checked_add(amount).ok_or(AssetError::MaxSupply)?;
        Ok(())
    }

    pub fn commit(self, base: &mut Assets) {
        for (symbol, asset) in self.assets {
            base.assets.insert(symbol, asset);
        }
        for (key, h) in self.holdings {
            let unchanged = base.holdings.get(&key) == h.as_ref();
            match h {
                _ if unchanged => {}
                Some(h) => base.holdings.insert(key, h),
                None => {
                    base.holdings.remove(&key);
                }
            }
        }
    }
}
