use crate::WorldState;
use vinx_core::{
    amount::{ADMIN_ALLOCATION_ATOMS, RESERVE_ALLOCATION_ATOMS},
    Account, Amount,
};
use vinx_crypto::Address;

pub struct GenesisConfig {
    /// Receives 500M VinX immediately at block 0.
    pub admin_address: Address,
    /// Protocol-internal address holding the 99.5B reserve until linear emission.
    pub reserve_address: Address,
}

/// Builds the initial chain state from the genesis configuration.
pub fn create_genesis_state(config: &GenesisConfig) -> WorldState {
    let mut state = WorldState::new();

    // 500M VinX to admin — immediately usable
    state.insert_account(Account::new_with_balance(
        config.admin_address.clone(),
        Amount::from_atoms(ADMIN_ALLOCATION_ATOMS),
    ));

    // 99.5B tracked in the protocol reserve field, not as an account balance
    state.protocol_reserve = Amount::from_atoms(RESERVE_ALLOCATION_ATOMS);

    // Circulating supply at genesis = only the admin allocation
    state.circulating_supply = Amount::from_atoms(ADMIN_ALLOCATION_ATOMS);
    state.block_height = 0;

    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::amount::{
        ADMIN_ALLOCATION_ATOMS, MAX_SUPPLY_ATOMS, RESERVE_ALLOCATION_ATOMS,
    };
    use vinx_crypto::KeyPair;

    fn genesis() -> (WorldState, Address, Address) {
        let admin_kp = KeyPair::generate();
        let reserve_kp = KeyPair::generate();
        let admin_addr = Address::from_public_key(&admin_kp.public_key());
        let reserve_addr = Address::from_public_key(&reserve_kp.public_key());
        let state = create_genesis_state(&GenesisConfig {
            admin_address: admin_addr.clone(),
            reserve_address: reserve_addr.clone(),
        });
        (state, admin_addr, reserve_addr)
    }

    #[test]
    fn test_admin_receives_500m_vinx() {
        let (state, admin, _) = genesis();
        assert_eq!(
            state.account_balance(&admin).atoms(),
            ADMIN_ALLOCATION_ATOMS
        );
    }

    #[test]
    fn test_reserve_is_tracked_in_state() {
        let (state, _, _) = genesis();
        assert_eq!(
            state.protocol_reserve.atoms(),
            RESERVE_ALLOCATION_ATOMS
        );
    }

    #[test]
    fn test_circulating_supply_at_genesis() {
        let (state, _, _) = genesis();
        assert_eq!(
            state.circulating_supply.atoms(),
            ADMIN_ALLOCATION_ATOMS
        );
    }

    #[test]
    fn test_admin_plus_reserve_equals_max_supply() {
        assert_eq!(ADMIN_ALLOCATION_ATOMS + RESERVE_ALLOCATION_ATOMS, MAX_SUPPLY_ATOMS);
    }

    #[test]
    fn test_genesis_block_height_is_zero() {
        let (state, _, _) = genesis();
        assert_eq!(state.block_height, 0);
    }

    #[test]
    fn test_staking_pool_and_treasury_are_zero_at_genesis() {
        let (state, _, _) = genesis();
        assert_eq!(state.staking_pool, Amount::ZERO);
        assert_eq!(state.treasury, Amount::ZERO);
    }
}
