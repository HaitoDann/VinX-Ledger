//! Genèse (SPEC §12).

use crate::escrow::Escrow;
use crate::state::{AdminSet, State};
use crate::validators::ValidatorSet;
use thiserror::Error;
use vinx_core::{ADMIN_SIGNERS, Account, Amount, Params, TOTAL_SUPPLY, UNITS_PER_VINX};
use vinx_crypto::{Address, PublicKey};

/// Solde de la trésorerie à la genèse : 100 000 000 VINX.
pub const GENESIS_TREASURY: Amount = 100_000_000 * UNITS_PER_VINX;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Genesis {
    pub chain_id: String,
    /// Horodatage Unix, en secondes.
    pub genesis_time: u64,
    pub protocol_version: u32,
    pub admin: [PublicKey; ADMIN_SIGNERS],
    pub validators: Vec<PublicKey>,
    pub treasury_balance: Amount,
    pub escrow: Escrow,
    pub params: Params,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GenesisError {
    #[error("identifiant de chaîne vide")]
    EmptyChainId,
    #[error("aucun validateur")]
    NoValidators,
    #[error("validateur en double")]
    DuplicateValidator,
    #[error("signataire admin en double")]
    DuplicateSigner,
    #[error("calendrier d'escrow invalide")]
    InvalidEscrow,
    #[error("la supply ne vaut pas exactement 10¹⁸ unités")]
    SupplyMismatch,
}

impl Genesis {
    /// Genèse conforme à l'ADR 0001 : 10 % en trésorerie, 90 % en 120 tranches.
    pub fn standard(
        chain_id: impl Into<String>,
        genesis_time: u64,
        admin: [PublicKey; ADMIN_SIGNERS],
        validators: Vec<PublicKey>,
    ) -> Option<Self> {
        Some(Self {
            chain_id: chain_id.into(),
            genesis_time,
            protocol_version: 1,
            admin,
            validators,
            treasury_balance: GENESIS_TREASURY,
            escrow: Escrow::genesis(genesis_time)?,
            params: Params::default(),
        })
    }

    /// Construit l'état initial, après vérification de toutes les règles.
    pub fn into_state(self) -> Result<State, GenesisError> {
        if self.chain_id.is_empty() {
            return Err(GenesisError::EmptyChainId);
        }
        if self.validators.is_empty() {
            return Err(GenesisError::NoValidators);
        }
        if has_duplicates(&self.validators) {
            return Err(GenesisError::DuplicateValidator);
        }
        if has_duplicates(&self.admin) {
            return Err(GenesisError::DuplicateSigner);
        }
        let ordered = self
            .escrow
            .tranches
            .iter()
            .zip(self.escrow.tranches.iter().skip(1))
            .all(|(a, b)| a.matures_at < b.matures_at);
        if self.escrow.period == 0 || !ordered {
            return Err(GenesisError::InvalidEscrow);
        }
        let total = u128::from(self.treasury_balance).checked_add(self.escrow.total());
        if total != Some(u128::from(TOTAL_SUPPLY)) {
            return Err(GenesisError::SupplyMismatch);
        }
        let treasury = Account {
            balance: self.treasury_balance,
            ..Account::default()
        };
        let state = State {
            chain_id: self.chain_id,
            protocol_version: self.protocol_version,
            height: 0,
            accounts: [(Address::treasury(), treasury)].into_iter().collect(),
            escrow: self.escrow,
            admin: AdminSet {
                signers: self.admin,
                nonce: 0,
            },
            validators: ValidatorSet::new(self.validators),
            params: self.params,
            upgrade: None,
        };
        state
            .check_invariant()
            .map_err(|_| GenesisError::SupplyMismatch)?;
        Ok(state)
    }
}

fn has_duplicates(keys: &[PublicKey]) -> bool {
    keys.iter()
        .enumerate()
        .any(|(i, k)| keys.iter().skip(i.saturating_add(1)).any(|o| o == k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn keys(n: usize) -> Vec<PublicKey> {
        (0..n).map(|_| KeyPair::generate().public_key()).collect()
    }

    fn standard() -> Genesis {
        let a = keys(3);
        Genesis::standard(
            "vinx-test",
            1_800_000_000,
            [a[0].clone(), a[1].clone(), a[2].clone()],
            keys(4),
        )
        .unwrap()
    }

    #[test]
    fn standard_genesis_is_valid() {
        let s = standard().into_state().unwrap();
        assert_eq!(s.accounts[&Address::treasury()].balance, GENESIS_TREASURY);
        assert_eq!(s.escrow.tranches.len(), 120);
        assert_eq!(s.check_invariant(), Ok(()));
    }

    #[test]
    fn invalid_genesis_is_rejected() {
        let mut g = standard();
        g.treasury_balance += 1;
        assert_eq!(g.into_state().unwrap_err(), GenesisError::SupplyMismatch);

        let mut g = standard();
        g.validators.clear();
        assert_eq!(g.into_state().unwrap_err(), GenesisError::NoValidators);

        let mut g = standard();
        g.validators.push(g.validators[0].clone());
        assert_eq!(
            g.into_state().unwrap_err(),
            GenesisError::DuplicateValidator
        );

        let mut g = standard();
        g.admin[2] = g.admin[0].clone();
        assert_eq!(g.into_state().unwrap_err(), GenesisError::DuplicateSigner);

        let mut g = standard();
        g.escrow.tranches.swap(0, 1);
        assert_eq!(g.into_state().unwrap_err(), GenesisError::InvalidEscrow);
    }
}
