use crate::WorldState;
use vinx_core::{
    amount::{ADMIN_ALLOCATION_ATOMS, COFFRE_MATURITY_ATOMS},
    Account, Amount, ValidatorSet,
};
use vinx_crypto::Address;

pub struct GenesisConfig {
    /// Receives 21M VinX immediately at block 0 (Sandbox Phase 1).
    pub admin_address: Address,
    /// Initial PoA validator — the node that proposes block 1 and beyond.
    pub validator_address: Address,
}

/// Builds the initial chain state from the genesis configuration.
pub fn create_genesis_state(config: &GenesisConfig) -> WorldState {
    let mut state = WorldState::new();

    // 21M VinX to admin — Sandbox allocation, immediately usable
    state.insert_account(Account::new_with_balance(
        config.admin_address.clone(),
        Amount::from_atoms(ADMIN_ALLOCATION_ATOMS),
    ));

    // Coffre Maturité: tracked in WorldState, cryptographically locked
    // Unlockable only when 3 cumulative conditions are met (MiCA CASP, audit, public policy)
    state.coffre_maturity = Amount::from_atoms(COFFRE_MATURITY_ATOMS);

    // Circulating supply at genesis = only the admin Sandbox allocation
    state.circulating_supply = Amount::from_atoms(ADMIN_ALLOCATION_ATOMS);
    state.block_height = 0;
    // Admin address is stored on-chain for governance operations (freeze, upgrades)
    state.admin_address = Some(config.admin_address.clone());
    // Initial validator set — admin can add/remove validators via governance transactions
    state.validator_set = ValidatorSet::single(config.validator_address.clone());

    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::amount::{ADMIN_ALLOCATION_ATOMS, COFFRE_MATURITY_ATOMS, MAX_SUPPLY_ATOMS};
    use vinx_crypto::KeyPair;

    fn genesis() -> (WorldState, Address) {
        let admin_kp = KeyPair::generate();
        let admin_addr = Address::from_public_key(&admin_kp.public_key());
        let validator_kp = KeyPair::generate();
        let validator_addr = Address::from_public_key(&validator_kp.public_key());
        let state = create_genesis_state(&GenesisConfig {
            admin_address: admin_addr.clone(),
            validator_address: validator_addr,
        });
        (state, admin_addr)
    }

    #[test]
    fn test_admin_receives_21m_vinx() {
        let (state, admin) = genesis();
        assert_eq!(
            state.account_balance(&admin).atoms(),
            ADMIN_ALLOCATION_ATOMS
        );
    }

    #[test]
    fn test_coffre_maturity_is_tracked_in_state() {
        let (state, _) = genesis();
        assert_eq!(state.coffre_maturity.atoms(), COFFRE_MATURITY_ATOMS);
    }

    #[test]
    fn test_circulating_supply_at_genesis() {
        let (state, _) = genesis();
        assert_eq!(state.circulating_supply.atoms(), ADMIN_ALLOCATION_ATOMS);
    }

    #[test]
    fn test_admin_plus_coffre_equals_max_supply() {
        assert_eq!(ADMIN_ALLOCATION_ATOMS + COFFRE_MATURITY_ATOMS, MAX_SUPPLY_ATOMS);
    }

    #[test]
    fn test_genesis_block_height_is_zero() {
        let (state, _) = genesis();
        assert_eq!(state.block_height, 0);
    }

    #[test]
    fn test_staking_pool_and_treasury_are_zero_at_genesis() {
        let (state, _) = genesis();
        assert_eq!(state.staking_pool, Amount::ZERO);
        assert_eq!(state.treasury, Amount::ZERO);
    }

    #[test]
    fn test_validator_set_initialized_at_genesis() {
        let validator_kp = KeyPair::generate();
        let validator_addr = Address::from_public_key(&validator_kp.public_key());
        let admin_kp = KeyPair::generate();
        let admin_addr = Address::from_public_key(&admin_kp.public_key());
        let state = create_genesis_state(&GenesisConfig {
            admin_address: admin_addr,
            validator_address: validator_addr.clone(),
        });
        assert!(state.validator_set.contains(&validator_addr));
        assert_eq!(state.validator_set.len(), 1);
    }
}
