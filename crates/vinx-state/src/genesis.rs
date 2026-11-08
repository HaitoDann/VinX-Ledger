//! Genèse (SPEC §12).

use crate::emission::{DEFAULT_EPOCH_BLOCKS, Emission, GENESIS_EMISSION};
use crate::escrow::Escrow;
use crate::staking::{
    BOND_GRACE, Bond, NameOwner, Staking, ValidatorRecord, ValidatorStatus, name_key,
};
use crate::state::{AdminSet, State};
use crate::validators::ValidatorSet;
use thiserror::Error;
use vinx_core::{
    ADMIN_SIGNERS, Account, Amount, Params, TOTAL_SUPPLY, UNITS_PER_VINX, ValidatorIdentity,
};
use vinx_crypto::{Address, PublicKey};

/// Solde de la trésorerie à la genèse des versions 0.1 à 0.9 : 100 000 000 VINX.
/// Depuis le protocole 3, ces 100 M forment le budget d'émission des validateurs
/// et la trésorerie démarre à 0 (ADR 0002).
pub const LEGACY_GENESIS_TREASURY: Amount = 100_000_000 * UNITS_PER_VINX;

/// Version du protocole d'une genèse standard.
pub const GENESIS_PROTOCOL: u32 = 8;

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
    /// Protocole 7 : identité de chaque validateur, dans l'ordre de
    /// `validators`. Vide : fiches provisoires « validateur-N », « VinX Labs ».
    pub identities: Vec<ValidatorIdentity>,
    /// Protocole 7 : caution de chaque validateur, prise sur la trésorerie.
    /// Sous la caution exigée, le validateur a 30 jours de grâce.
    pub validator_bond: Amount,
    /// Protocole 7 : caution exigée de chaque validateur (`validator_bond`
    /// des paramètres). `None` : la valeur par défaut, 10 000 VINX.
    pub required_bond: Option<Amount>,
    /// Protocole 8 : modèle réserve (réseau principal, pré-production). Les
    /// 900 M forment la réserve du protocole au lieu de l'escrow ; pas de
    /// trésorerie ni de robinet.
    pub reserve_model: bool,
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
    #[error("identités de validateurs invalides, en double ou en nombre incorrect")]
    InvalidIdentities,
    #[error("trésorerie insuffisante pour les cautions des validateurs")]
    BondsTooLarge,
    #[error("modèle réserve : protocole 8 au moins, sans escrow, sans trésorerie ni robinet")]
    InvalidReserveModel,
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
            identities: Vec::new(),
            validator_bond: 0,
            required_bond: None,
            reserve_model: false,
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

    /// Modèle réserve (protocole 8) : les 900 M de l'escrow deviennent la
    /// réserve du protocole. À appeler sur une genèse sans robinet.
    pub fn with_reserve(mut self) -> Result<Self, GenesisError> {
        if self.treasury_balance > 0 {
            return Err(GenesisError::InvalidReserveModel);
        }
        self.escrow.tranches.clear();
        self.protocol_version = self.protocol_version.max(8);
        self.reserve_model = true;
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
        let reserve = if self.reserve_model {
            if self.protocol_version < 8
                || self.treasury_balance > 0
                || !self.escrow.tranches.is_empty()
            {
                return Err(GenesisError::InvalidReserveModel);
            }
            crate::governance::GENESIS_RESERVE
        } else {
            0
        };
        let total = u128::from(self.treasury_balance)
            .checked_add(u128::from(reserve))
            .and_then(|t| t.checked_add(self.escrow.total()))
            .and_then(|t| t.checked_add(u128::from(self.emission_budget)));
        if total != Some(u128::from(TOTAL_SUPPLY)) {
            return Err(GenesisError::SupplyMismatch);
        }
        let mut treasury_balance = self.treasury_balance;
        let mut staking = Staking::default();
        if self.protocol_version >= 7 {
            staking = genesis_staking(
                &self.validators,
                &self.identities,
                self.validator_bond,
                self.required_bond,
                self.genesis_time,
            )?;
            let n = Amount::try_from(self.validators.len()).unwrap_or(Amount::MAX);
            let bonds = self
                .validator_bond
                .checked_mul(n)
                .ok_or(GenesisError::BondsTooLarge)?;
            treasury_balance = treasury_balance
                .checked_sub(bonds)
                .ok_or(GenesisError::BondsTooLarge)?;
        } else if !self.identities.is_empty()
            || self.validator_bond > 0
            || self.required_bond.is_some()
        {
            return Err(GenesisError::InvalidIdentities);
        }
        let treasury = Account {
            balance: treasury_balance,
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
            policies: Default::default(),
            mandates: Default::default(),
            assets: Default::default(),
            reserves: Default::default(),
            staking,
            governance: crate::governance::Governance {
                enabled: self.reserve_model,
                reserve,
                month_start: self.genesis_time,
                ..Default::default()
            },
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

/// Fiches et cautions des validateurs d'une genèse au protocole 7.
fn genesis_staking(
    validators: &[PublicKey],
    identities: &[ValidatorIdentity],
    bond: Amount,
    required_bond: Option<Amount>,
    genesis_time: u64,
) -> Result<Staking, GenesisError> {
    if !identities.is_empty() && identities.len() != validators.len() {
        return Err(GenesisError::InvalidIdentities);
    }
    let mut staking = Staking::default();
    if let Some(required) = required_bond {
        staking.params.validator_bond = required;
    }
    let grace_until = if bond >= staking.params.validator_bond {
        0
    } else {
        genesis_time.saturating_add(BOND_GRACE)
    };
    for (i, key) in validators.iter().enumerate() {
        let identity = identities
            .get(i)
            .cloned()
            .unwrap_or_else(|| ValidatorIdentity {
                name: format!("validateur-{}", i.saturating_add(1)),
                operator: "VinX Labs".into(),
                country: *b"FR",
            });
        if !identity.is_valid() || staking.names.contains_key(&name_key(&identity.name)) {
            return Err(GenesisError::InvalidIdentities);
        }
        staking
            .names
            .insert(name_key(&identity.name), NameOwner { key: key.clone() });
        if bond > 0 {
            staking.bonds.insert(key.clone(), Bond { amount: bond });
        }
        staking.records.insert(
            key.clone(),
            ValidatorRecord {
                identity,
                website: String::new(),
                contact: String::new(),
                status: ValidatorStatus::Active,
                joined_at: 0,
                grace_until,
            },
        );
    }
    let set = validators.to_vec();
    if !staking.diversity_ok(&set, |k| {
        staking
            .records
            .get(k)
            .map(|r| r.identity.operator.clone())
            .unwrap_or_default()
    }) {
        return Err(GenesisError::InvalidIdentities);
    }
    Ok(staking)
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
        assert_eq!(s.protocol_version, GENESIS_PROTOCOL);
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
    fn protocol_7_genesis_has_records_and_bonds() {
        let bond = crate::staking::DEFAULT_VALIDATOR_BOND;
        let mut g = standard().with_faucet(1_000_000 * UNITS_PER_VINX).unwrap();
        g.validator_bond = bond;
        g.identities = ["Cinnamon", "Amber", "Obsidian", "Pearl"]
            .iter()
            .map(|n| ValidatorIdentity {
                name: (*n).into(),
                operator: "VinX Labs".into(),
                country: *b"FR",
            })
            .collect();
        let s = g.clone().into_state().unwrap();
        assert_eq!(s.staking.records.len(), 4);
        assert_eq!(s.staking.bond(&g.validators[0]), bond);
        let r = s.staking.records.get(&g.validators[1]).unwrap();
        assert_eq!(r.identity.name, "Amber");
        assert_eq!(r.grace_until, 0);
        assert_eq!(
            s.accounts[&Address::treasury()].balance,
            1_000_000 * UNITS_PER_VINX - 4 * bond
        );
        assert_eq!(s.check_invariant(), Ok(()));

        // Sans caution : fiches provisoires et délai de grâce.
        let s = standard().into_state().unwrap();
        let r = s.staking.records.get(&standard().validators[0]);
        assert!(r.is_none(), "clés différentes à chaque appel");
        assert_eq!(s.staking.records.len(), 4);
        assert!(
            s.staking
                .records
                .iter()
                .all(|(_, r)| r.grace_until == 1_800_000_000 + BOND_GRACE)
        );

        let mut dup = g.clone();
        dup.identities[1].name = "cinnamon".into();
        assert_eq!(dup.into_state(), Err(GenesisError::InvalidIdentities));
        let mut big = g;
        big.validator_bond = 1_000_000 * UNITS_PER_VINX;
        assert_eq!(big.into_state(), Err(GenesisError::BondsTooLarge));
    }

    #[test]
    fn required_bond_can_be_set_at_genesis() {
        // Pré-production : pas de robinet, caution exigée basse, un validateur.
        let mut g = standard();
        g.validators.truncate(1);
        g.required_bond = Some(1_000 * UNITS_PER_VINX);
        let s = g.clone().into_state().unwrap();
        assert_eq!(s.staking.params.validator_bond, 1_000 * UNITS_PER_VINX);
        assert_eq!(s.accounts[&Address::treasury()].balance, 0);
        let r = s.staking.records.get(&g.validators[0]).unwrap();
        assert_eq!(
            r.grace_until,
            1_800_000_000 + BOND_GRACE,
            "caution à déposer"
        );
        // Une caution de genèse au niveau exigé : pas de délai de grâce.
        let mut g = g.with_faucet(1_000 * UNITS_PER_VINX).unwrap();
        g.validator_bond = 1_000 * UNITS_PER_VINX;
        let s = g.clone().into_state().unwrap();
        assert_eq!(
            s.staking.records.get(&g.validators[0]).unwrap().grace_until,
            0
        );
        assert_eq!(s.check_invariant(), Ok(()));
        // Réservé au protocole 7.
        g.protocol_version = 6;
        g.validator_bond = 0;
        assert_eq!(g.into_state(), Err(GenesisError::InvalidIdentities));
    }

    #[test]
    fn reserve_model_genesis() {
        let s = standard().with_reserve().unwrap().into_state().unwrap();
        assert!(s.governance.enabled);
        assert_eq!(s.governance.reserve, crate::governance::GENESIS_RESERVE);
        assert!(s.escrow.tranches.is_empty());
        assert_eq!(s.accounts[&Address::treasury()].balance, 0);
        assert_eq!(s.protocol_version, 8);
        assert_eq!(s.check_invariant(), Ok(()));
        // Pas de robinet au modèle réserve.
        let g = standard().with_faucet(UNITS_PER_VINX).unwrap();
        assert_eq!(g.with_reserve(), Err(GenesisError::InvalidReserveModel));
        let mut g = standard().with_reserve().unwrap();
        g.protocol_version = 7;
        assert_eq!(g.into_state(), Err(GenesisError::InvalidReserveModel));
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
