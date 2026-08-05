use crate::WorldState;
use vinx_core::{amount::FOUNDRY_GENESIS_ATOMS, Account, Amount, ValidatorSet, CHAIN_ID_MAINNET};
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
/// **Fair launch — no pre-mine.** All 100 billion VinX sit in the Foundry at genesis
/// and circulation starts at zero. Tokens enter circulation only by rewarding the work
/// of block producers (work emission), and once the Foundry is drained, transaction
/// fees become the validators' only reward.
pub fn create_genesis_state(config: &GenesisConfig) -> WorldState {
    let mut state = WorldState::new();

    // The entire supply is sealed in the Foundry; nothing circulates yet.
    state.foundry = Amount::from_atoms(FOUNDRY_GENESIS_ATOMS);
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
/// validateur de genèse en déplaçant `prefund_atoms` de la Fonderie vers son compte. Sert à
/// amorcer un testnet multi-validateurs sans attendre l'émission (les candidats doivent bonder
/// 100k VINX ; le validateur de genèse les leur transfère).
///
/// **Garde-fou fair launch :** si `chain_id == CHAIN_ID_MAINNET`, le pré-financement est
/// **ignoré** — la genèse mainnet reste à 0 en circulation, sans pre-mine. L'invariant de masse
/// `circulation + Fonderie == MAX` est préservé (on déplace, on ne crée pas).
pub fn create_genesis_state_with_dev_prefund(
    config: &GenesisConfig,
    prefund_atoms: u128,
) -> WorldState {
    let mut state = create_genesis_state(config);
    if prefund_atoms == 0 || config.chain_id == CHAIN_ID_MAINNET {
        return state; // no-op : mainnet ou pas de pré-financement demandé
    }
    let amount = Amount::from_atoms(prefund_atoms);
    state.accounts.insert(
        config.validator_address,
        Account::new_with_balance(config.validator_address, amount),
    );
    // Déplacement Fonderie → circulation (invariant de masse préservé, ADR 0004).
    state.foundry = Amount::from_atoms(FOUNDRY_GENESIS_ATOMS - prefund_atoms);
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
    fn test_foundry_holds_entire_supply_at_genesis() {
        let (state, _) = genesis();
        assert_eq!(state.foundry.atoms(), MAX_SUPPLY_ATOMS);
    }

    #[test]
    fn test_circulating_supply_is_zero_at_genesis() {
        let (state, _) = genesis();
        assert_eq!(state.circulating_supply, Amount::ZERO);
    }

    #[test]
    fn test_circulation_plus_foundry_equals_max_supply() {
        // The founding invariant: 0 circulating + 100 Md Foundry == the whole supply.
        let (state, _) = genesis();
        assert_eq!(
            state.circulating_supply.atoms() + state.foundry.atoms(),
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
