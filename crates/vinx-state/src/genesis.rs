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
    /// Proof-of-Possession (96 bytes). Required (ADR 0082): every block is committed by a
    /// certificate of BLS-signed precommits verified against the on-chain registry, so
    /// even a single-node chain needs its validator's key registered from genesis.
    pub validator_bls: GenesisBlsKey,
}

/// The genesis validator's BLS key material, verified before it is written to state.
#[derive(Clone, Debug)]
pub struct GenesisBlsKey {
    /// G1 compressed public key, 48 bytes.
    pub pub_key: [u8; 48],
    /// Proof-of-Possession over `pub_key`, G2 compressed, 96 bytes.
    pub pop: [u8; 96],
}

impl GenesisBlsKey {
    /// Key material of `sk` for `validator` on `chain_id` (PoP bound to both).
    pub fn from_secret(
        sk: &vinx_crypto::BlsSecretKey,
        validator: &vinx_crypto::Address,
        chain_id: u32,
    ) -> Self {
        Self {
            pub_key: sk.public_key().0,
            pop: sk.proof_of_possession(validator.as_bytes(), chain_id).0,
        }
    }
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
    {
        let bls = &config.validator_bls;
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
    // The admin's governance transactions (upgrade announcements, admissions) are
    // fee-free but need an account for their nonce: 1 VINX of the prefund opens it.
    // On mainnet (no prefund), anyone sending the admin a payment opens it.
    let admin_atoms = vinx_core::amount::DECIMAL_FACTOR.min(prefund_atoms);
    let (amount, admin_amount) = if config.admin_address != config.validator_address {
        (
            Amount::from_atoms(prefund_atoms - admin_atoms),
            Amount::from_atoms(admin_atoms),
        )
    } else {
        (amount, Amount::ZERO)
    };
    state.accounts.insert(
        config.validator_address,
        Account::new_with_balance(config.validator_address, amount),
    );
    if admin_amount > Amount::ZERO {
        state.accounts.insert(
            config.admin_address,
            Account::new_with_balance(config.admin_address, admin_amount),
        );
    }
    state.emitted_atoms = prefund_atoms;
    state.genesis_prefund_atoms = prefund_atoms;
    state.circulating_supply = Amount::from_atoms(prefund_atoms);
    state
}

/// Adds validators to a genesis state (multi-validator launch, ADR 0082). Each key's
/// Proof-of-Possession is verified against its address and the chain id; voting powers
/// are recomputed over the whole genesis set. Panics on an invalid PoP or a duplicate,
/// since an unverifiable genesis must never be written.
pub fn add_genesis_validators(state: &mut WorldState, extra: &[(Address, GenesisBlsKey)]) {
    if extra.is_empty() {
        return;
    }
    let mut addrs: Vec<Address> = state.validator_set.validators().to_vec();
    for (addr, bls) in extra {
        assert!(!addrs.contains(addr), "duplicate genesis validator {addr}");
        let pop_ok = BlsPubKey::from_bytes(&bls.pub_key)
            .and_then(|pk| pk.verify_pop(&BlsSignature(bls.pop), addr.as_bytes(), state.chain_id))
            .is_ok();
        assert!(
            pop_ok,
            "genesis validator {addr}: invalid BLS Proof-of-Possession"
        );
        let mut entry = ValidatorPoolEntry::new(0, 0);
        entry.status = PoolStatus::Active;
        entry.bls_pub_key = Some(bls.pub_key.to_vec());
        entry.bls_pop = Some(bls.pop.to_vec());
        state.validator_pool.insert(*addr, entry);
        addrs.push(*addr);
    }
    state.validator_set = state.weighted_validator_set(addrs);
}

#[cfg(test)]
mod tests {

    #[test]
    fn prefund_opens_the_admin_account_and_keeps_supply_exact() {
        let sk = vinx_crypto::BlsSecretKey::generate();
        let val = vinx_crypto::KeyPair::generate();
        let adm = vinx_crypto::KeyPair::generate();
        let validator = vinx_crypto::Address::from_public_key(&val.public_key());
        let admin = vinx_crypto::Address::from_public_key(&adm.public_key());
        let cfg = GenesisConfig {
            admin_address: admin,
            validator_address: validator,
            chain_id: 7,
            validator_bls: crate::GenesisBlsKey::from_secret(&sk, &validator, 7),
        };
        let prefund = 1_000 * vinx_core::amount::DECIMAL_FACTOR;
        let s = create_genesis_state_with_dev_prefund(&cfg, prefund);
        assert_eq!(
            s.get_account(&admin).map(|a| a.balance.atoms()),
            Some(vinx_core::amount::DECIMAL_FACTOR)
        );
        assert!(
            s.supply_invariant_holds(),
            "circulating == emitted at genesis"
        );
    }
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
            admin_address: admin_addr,
            validator_address: validator_addr,
            chain_id: CHAIN_ID_DEVNET,
            validator_bls: crate::genesis::GenesisBlsKey::from_secret(
                &vinx_crypto::BlsSecretKey::generate(),
                &validator_addr,
                CHAIN_ID_DEVNET,
            ),
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
            validator_address: validator_addr,
            chain_id: CHAIN_ID_DEVNET,
            validator_bls: crate::genesis::GenesisBlsKey::from_secret(
                &vinx_crypto::BlsSecretKey::generate(),
                &validator_addr,
                CHAIN_ID_DEVNET,
            ),
        });
        assert!(state.validator_set.contains(&validator_addr));
        assert_eq!(state.validator_set.len(), 1);
    }
}
