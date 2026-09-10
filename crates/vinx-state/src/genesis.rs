use crate::WorldState;
use vinx_core::{Account, Amount, PoolStatus, ValidatorPoolEntry, ValidatorSet, CHAIN_ID_MAINNET};
use vinx_crypto::{Address, BlsPubKey, BlsSignature};

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
    /// BLS12-381 G1 public key (48 bytes) of the genesis validator, with its
    /// Proof-of-Possession (96 bytes).
    ///
    /// Since blocks are authenticated against the on-chain BLS registry
    /// (`consensus::verify_proposer_authenticated`), a proposer whose key is not
    /// registered cannot be authenticated and its blocks are refused by peers. The
    /// genesis validator has no way to register one itself — that would require a
    /// transaction in a block peers accept, which is exactly what it cannot produce.
    /// Registering it here breaks that bootstrap deadlock.
    ///
    /// `None` leaves the registry empty, which is only usable for a single-node chain
    /// (a lone producer appends to its own chain without the P2P check). Any
    /// multi-node network must set it.
    pub validator_bls: Option<GenesisBlsKey>,
}

/// The genesis validator's BLS key material, verified before it is written to state.
#[derive(Clone, Debug)]
pub struct GenesisBlsKey {
    /// G1 compressed public key, 48 bytes.
    pub pub_key: [u8; 48],
    /// Proof-of-Possession over `pub_key`, G2 compressed, 96 bytes.
    pub pop: [u8; 96],
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

    // Register the genesis validator's BLS key so its blocks can be authenticated from
    // height 1. The genesis validator is grandfathered past the bond requirement, so it
    // has no pool entry yet; create one with a zero bond purely to carry the key.
    // `indexed_bls_keys` reads `validator_pool`, so without this entry the registry is
    // empty and every block it produces is refused by peers.
    if let Some(ref bls) = config.validator_bls {
        let pop_ok = BlsPubKey::from_bytes(&bls.pub_key)
            .and_then(|pk| {
                pk.verify_pop(
                    &BlsSignature(bls.pop),
                    config.validator_address.as_bytes(),
                    config.chain_id,
                )
            })
            .is_ok();
        assert!(
            pop_ok,
            "genesis validator BLS Proof-of-Possession is invalid — refusing to write an \
             unverifiable key into genesis state"
        );
        let mut entry = ValidatorPoolEntry::new(0, 0);
        // The genesis validator produces from height 1; it does not serve a warm-up.
        entry.status = PoolStatus::Active;
        entry.bls_pub_key = Some(bls.pub_key.to_vec());
        entry.bls_pop = Some(bls.pop.to_vec());
        state.validator_pool.insert(config.validator_address, entry);
    }

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
            validator_bls: None,
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
            validator_bls: None,
        });
        assert!(state.validator_set.contains(&validator_addr));
        assert_eq!(state.validator_set.len(), 1);
    }
}
