use crate::WorldState;
use vinx_core::{
    amount::{FOUNDER_ALLOCATION_ATOMS, FOUNDRY_GENESIS_ATOMS},
    Account, Amount, ValidatorSet,
};
use vinx_crypto::Address;

pub struct GenesisConfig {
    /// Founder account — forged 1% of supply (1 billion VinX) at block 0 to bootstrap
    /// circulation, and holds the admin key for governance.
    pub admin_address: Address,
    /// Initial PoA validator — the node that proposes block 1 and beyond.
    pub validator_address: Address,
    /// Chain ID for replay protection (CHAIN_ID_MAINNET / TESTNET / DEVNET).
    pub chain_id: u32,
}

/// Builds the initial chain state from the genesis configuration.
///
/// The 100 billion VinX are forged once: 1 billion (1%) into the founder's account
/// to seed circulation, and 99 billion (99%) sealed in the Foundry. From then on the
/// supply only cycles — fees melt into the Foundry, staking rewards are forged out.
pub fn create_genesis_state(config: &GenesisConfig) -> WorldState {
    let mut state = WorldState::new();

    // 1 billion VinX forged to the founder — circulating from block 0.
    state.insert_account(Account::new_with_balance(
        config.admin_address,
        Amount::from_atoms(FOUNDER_ALLOCATION_ATOMS),
    ));
    state.circulating_supply = Amount::from_atoms(FOUNDER_ALLOCATION_ATOMS);

    // 99 billion VinX sealed in the Foundry — forged into circulation over time.
    state.foundry = Amount::from_atoms(FOUNDRY_GENESIS_ATOMS);

    state.block_height = 0;
    // Admin address is stored on-chain for governance operations (validators, upgrades).
    state.admin_address = Some(config.admin_address);
    // Initial validator set — admin can add/remove validators via governance transactions.
    state.validator_set = ValidatorSet::single(config.validator_address);
    state.chain_id = config.chain_id;

    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::amount::{FOUNDER_ALLOCATION_ATOMS, FOUNDRY_GENESIS_ATOMS, MAX_SUPPLY_ATOMS};
    use vinx_core::CHAIN_ID_DEVNET;
    use vinx_crypto::KeyPair;

    fn genesis() -> (WorldState, Address) {
        let admin_kp = KeyPair::generate();
        let admin_addr = Address::from_public_key(&admin_kp.public_key());
        let validator_kp = KeyPair::generate();
        let validator_addr = Address::from_public_key(&validator_kp.public_key());
        let state = create_genesis_state(&GenesisConfig {
            admin_address: admin_addr.clone(),
            validator_address: validator_addr,
            chain_id: CHAIN_ID_DEVNET,
        });
        (state, admin_addr)
    }

    #[test]
    fn test_founder_receives_1_billion_vinx() {
        let (state, admin) = genesis();
        assert_eq!(
            state.account_balance(&admin).atoms(),
            FOUNDER_ALLOCATION_ATOMS
        );
    }

    #[test]
    fn test_foundry_holds_99_billion_at_genesis() {
        let (state, _) = genesis();
        assert_eq!(state.foundry.atoms(), FOUNDRY_GENESIS_ATOMS);
    }

    #[test]
    fn test_circulating_supply_at_genesis() {
        let (state, _) = genesis();
        assert_eq!(state.circulating_supply.atoms(), FOUNDER_ALLOCATION_ATOMS);
    }

    #[test]
    fn test_circulation_plus_foundry_equals_max_supply() {
        // The founding invariant of the melt/forge cycle.
        let (state, _) = genesis();
        assert_eq!(
            state.circulating_supply.atoms() + state.foundry.atoms(),
            MAX_SUPPLY_ATOMS
        );
        assert_eq!(
            FOUNDER_ALLOCATION_ATOMS + FOUNDRY_GENESIS_ATOMS,
            MAX_SUPPLY_ATOMS
        );
    }

    #[test]
    fn test_genesis_block_height_is_zero() {
        let (state, _) = genesis();
        assert_eq!(state.block_height, 0);
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
            chain_id: CHAIN_ID_DEVNET,
        });
        assert!(state.validator_set.contains(&validator_addr));
        assert_eq!(state.validator_set.len(), 1);
    }
}
