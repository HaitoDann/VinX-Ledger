use crate::WorldState;
use vinx_core::{Account, Amount, ValidatorSet, CHAIN_ID_MAINNET};
use vinx_crypto::Address;

pub struct GenesisConfig {
    /// Admin account — holds the governance key. It receives **no** genesis allocation
    /// (fair launch: no pre-mine); it may hold a zero balance and still govern, since
    /// governance transactions are fee-exempt.
    pub admin_address: Address,
    /// Initial PoA validator — proposes block 1 and beyond. Grandfathered past the
    /// bond requirement (it bootstraps with no balance and earns its first VinX by
    /// producing blocks).
    pub validator_address: Address,
    /// Chain ID for replay protection (CHAIN_ID_MAINNET / TESTNET / DEVNET).
    pub chain_id: u32,
}

/// Builds the initial chain state from the genesis configuration.
///
/// **Fair launch — no pre-mine.** At genesis `emitted_atoms = 0` and circulation = 0.
/// Tokens are minted progressively as block producers earn work emission. Once the
/// emission curve reaches dust-level, transaction fees become the sole reward (ADR 0040).
pub fn create_genesis_state(config: &GenesisConfig) -> WorldState {
    let mut state = WorldState::new();

    // Progressive minting: nothing is pre-allocated. emitted_atoms = 0, circulating = 0.
    state.circulating_supply = Amount::ZERO;

    state.block_height = 0;
    // Admin address is stored on-chain for governance operations (validators, upgrades).
    state.admin_address = Some(config.admin_address);
    // Initial validator set — admin can add/remove validators via governance transactions.
    state.validator_set = ValidatorSet::single(config.validator_address);
    state.chain_id = config.chain_id;

    state
}

/// **DEV/OPS uniquement — jamais sur mainnet.** Construit la genèse puis pré-finance le
/// validateur de genèse en mintant `prefund_atoms` directement dans son compte. Sert à
/// amorcer un testnet multi-validateurs sans attendre l'émission (les candidats doivent bonder
/// 100k VINX ; le validateur de genèse les leur transfère).
///
/// **Garde-fou fair launch :** si `chain_id == CHAIN_ID_MAINNET`, le pré-financement est
/// **ignoré** — la genèse mainnet reste à 0 en circulation, sans pre-mine. L'invariant de masse
/// `circ + pot + détruits = émis ≤ MAX` est préservé (ADR 0040).
pub fn create_genesis_state_with_dev_prefund(
    config: &GenesisConfig,
    prefund_atoms: u128,
) -> WorldState {
    let mut state = create_genesis_state(config);
    if prefund_atoms == 0 || config.chain_id == CHAIN_ID_MAINNET {
        return state; // no-op : mainnet ou pas de pré-financement demandé
    }
    let amount = Amount::from_atoms(prefund_atoms);
    // Progressive minting (ADR 0040): mint directly into the genesis validator's account.
    // emitted_atoms tracks the total supply minted; circulating_supply matches it here.
    state.accounts.insert(
        config.validator_address,
        Account::new_with_balance(config.validator_address, amount),
    );
    state.emitted_atoms = prefund_atoms;
    state.circulating_supply = amount;
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::amount::MAX_SUPPLY_ATOMS;
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
    fn test_no_premine_admin_balance_is_zero() {
        // Fair launch: the admin/founder receives nothing at genesis.
        let (state, admin) = genesis();
        assert_eq!(state.account_balance(&admin), Amount::ZERO);
    }

    #[test]
    fn test_nothing_emitted_at_genesis() {
        // ADR 0040: progressive minting — emitted_atoms = 0, no pre-allocation.
        let (state, _) = genesis();
        assert_eq!(state.emitted_atoms, 0);
        assert_eq!(state.circulating_supply, Amount::ZERO);
        assert_eq!(state.remaining_supply(), MAX_SUPPLY_ATOMS);
    }

    #[test]
    fn test_genesis_supply_invariant_holds() {
        // circulating(0) + epoch_pot(0) + destroyed(0) == emitted(0) ≤ MAX_SUPPLY.
        let (state, _) = genesis();
        assert!(state.supply_invariant_holds());
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
