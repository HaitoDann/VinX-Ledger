//! Genèse (SPEC §12).

use crate::emission::{DEFAULT_EPOCH_BLOCKS, Emission, GENESIS_EMISSION};
use crate::escrow::Escrow;
use crate::state::{AdminSet, State};
use crate::validators::ValidatorSet;
use thiserror::Error;
use vinx_core::{ADMIN_SIGNERS, Account, Amount, Params, TOTAL_SUPPLY, UNITS_PER_VINX};
use vinx_crypto::{Address, PublicKey};

/// Solde de la trésorerie à la genèse des versions 0.1 à 0.9 : 100 000 000 VINX.
/// Depuis le protocole 3, ces 100 M forment le budget d'émission des validateurs
/// et la trésorerie démarre à 0 (ADR 0002).
pub const LEGACY_GENESIS_TREASURY: Amount = 100_000_000 * UNITS_PER_VINX;

/// Version du protocole d'une genèse standard.
pub const GENESIS_PROTOCOL: u32 = 3;

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
    /// Budget d'émission des validateurs ; 0 pour une genèse d'avant le protocole 3.
    pub emission_budget: Amount,
    /// Durée d'une époque d'émission, en blocs.
    pub epoch_blocks: u64,
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
    #[error("robinet de testnet plus grand que la dernière tranche d'escrow")]
    FaucetTooLarge,
}

impl Genesis {
    /// Genèse conforme aux ADR 0001 et 0002 : 90 % en 120 tranches d'escrow,
    /// 10 % en budget d'émission des validateurs, trésorerie vide.
    pub fn standard(
        chain_id: impl Into<String>,
        genesis_time: u64,
        admin: [PublicKey; ADMIN_SIGNERS],
        validators: Vec<PublicKey>,
    ) -> Option<Self> {
        Some(Self {
            chain_id: chain_id.into(),
            genesis_time,
            protocol_version: GENESIS_PROTOCOL,
            admin,
            validators,
            treasury_balance: 0,
            escrow: Escrow::genesis(genesis_time)?,
            params: Params::default(),
            emission_budget: GENESIS_EMISSION,
            epoch_blocks: DEFAULT_EPOCH_BLOCKS,
        })
    }

    /// Genèse des versions 0.1 à 0.9 : protocole 1, 100 M en trésorerie, pas
    /// d'émission. Sert aux réseaux existants et aux tests des règles anciennes.
    pub fn legacy(
        chain_id: impl Into<String>,
        genesis_time: u64,
        admin: [PublicKey; ADMIN_SIGNERS],
        validators: Vec<PublicKey>,
    ) -> Option<Self> {
        Some(Self {
            protocol_version: 1,
            treasury_balance: LEGACY_GENESIS_TREASURY,
            emission_budget: 0,
            ..Self::standard(chain_id, genesis_time, admin, validators)?
        })
    }

    /// Réseau de test : dote la trésorerie de `amount` (le robinet), pris sur
    /// la dernière tranche de l'escrow. Le budget des validateurs et la supply
    /// ne changent pas.
    pub fn with_faucet(mut self, amount: Amount) -> Result<Self, GenesisError> {
        let last = self
            .escrow
            .tranches
            .back_mut()
            .ok_or(GenesisError::FaucetTooLarge)?;
        last.amount = last
            .amount
            .checked_sub(amount)
            .ok_or(GenesisError::FaucetTooLarge)?;
        self.treasury_balance = self
            .treasury_balance
            .checked_add(amount)
            .ok_or(GenesisError::SupplyMismatch)?;
        Ok(self)
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
        let total = u128::from(self.treasury_balance)
            .checked_add(self.escrow.total())
            .and_then(|t| t.checked_add(u128::from(self.emission_budget)));
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
            registry: crate::Registry::default(),
            emission: Emission {
                start: self.genesis_time,
                budget: self.emission_budget,
                epoch_blocks: self.epoch_blocks.max(1),
                ..Emission::default()
            },
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
        assert_eq!(s.accounts[&Address::treasury()].balance, 0);
        assert_eq!(s.escrow.tranches.len(), 120);
        assert_eq!(s.emission.remaining(), GENESIS_EMISSION);
        assert_eq!(s.protocol_version, 3);
        assert_eq!(s.check_invariant(), Ok(()));
    }

    #[test]
    fn testnet_faucet_comes_from_the_last_tranche() {
        let faucet = 1_000_000 * UNITS_PER_VINX;
        let s = standard()
            .with_faucet(faucet)
            .unwrap()
            .into_state()
            .unwrap();
        assert_eq!(s.accounts[&Address::treasury()].balance, faucet);
        assert_eq!(s.emission.remaining(), GENESIS_EMISSION);
        assert_eq!(
            s.escrow.tranches.back().unwrap().amount,
            crate::escrow::GENESIS_TRANCHE_AMOUNT - faucet
        );
        assert_eq!(s.check_invariant(), Ok(()));
        assert_eq!(
            standard().with_faucet(8_000_000 * UNITS_PER_VINX),
            Err(GenesisError::FaucetTooLarge)
        );
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
