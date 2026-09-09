//! Security regression tests — node / consensus layer.
//!
//! Each test carries the **exact attack** from the audit proof-of-concept and asserts it
//! is now refused. Every one of them passed (i.e. the attack succeeded) on the audited
//! commit `8774806`; they fail if any fix here is reverted.

use vinx_core::{Block, BlockHeader, ValidatorSet};
use vinx_crypto::{bls_aggregate, Address, BlsSecretKey, Hash32, KeyPair};
use vinx_node::chain::Chain;
use vinx_node::consensus;

fn addr() -> Address {
    Address::from_public_key(&KeyPair::generate().public_key())
}

fn make_block(height: u64, prev: Hash32, proposer: Address) -> Block {
    Block {
        header: BlockHeader {
            height,
            prev_hash: prev,
            timestamp: height,
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
    }
}

/// Deterministic BLS key for validator slot `idx`, so a registry can be built to match.
fn bls_sk(idx: usize) -> BlsSecretKey {
    let mut seed = [0u8; 32];
    seed[0] = idx as u8 + 1;
    BlsSecretKey::from_bytes(&seed).expect("valid BLS scalar")
}

fn registry(n: usize) -> Vec<Option<[u8; 48]>> {
    (0..n).map(|i| Some(bls_sk(i).public_key().0)).collect()
}

/// The attack: sign with `n` keys belonging to no validator, advertise them in
/// `bls_cosigner_pks`, and leave `bls_bitmap` empty to select the legacy path.
fn forge_quorum_with_foreign_keys(block: &mut Block, n: usize) {
    let sks: Vec<BlsSecretKey> = (0..n).map(|_| BlsSecretKey::generate()).collect();
    let msg = block.header.hash();
    let sigs: Vec<_> = sks.iter().map(|sk| sk.sign(&msg)).collect();
    let agg = bls_aggregate(&sigs).expect("aggregate");
    block.bls_aggregate = Some(agg.0.to_vec());
    block.bls_cosigner_pks = sks.iter().map(|sk| sk.public_key().0.to_vec()).collect();
    block.bls_bitmap = vec![];
}

/// VINX-02 / VX-RED-001 — a quorum of keys registered by nobody must not validate.
#[test]
fn forged_quorum_via_empty_bitmap_is_rejected() {
    let validators: Vec<Address> = (0..7).map(|_| addr()).collect();
    let vs = ValidatorSet::new(validators.clone());
    assert_eq!(vs.quorum(), 5, "7 validators → quorum 5");

    // No validator has registered a BLS key.
    let indexed_pks: Vec<Option<[u8; 48]>> = vec![None; 7];

    let mut block = make_block(1, [0u8; 32], validators[0]);
    forge_quorum_with_foreign_keys(&mut block, 5);

    assert!(
        consensus::validate_block_with_registry(&block, &vs, &indexed_pks).is_err(),
        "a quorum of unregistered keys must be refused"
    );
    assert!(
        !block.is_finalized(&vs, &indexed_pks),
        "the forged block must not be considered finalized"
    );

    // Even against a fully populated registry, an empty bitmap attributes the aggregate
    // to no validator and must not count.
    assert!(
        !block.is_finalized(&vs, &registry(7)),
        "an empty bitmap must never reach quorum"
    );
}

/// VINX-05 — fork-choice weight must count registered co-signers only, so padding a block
/// with self-generated keys cannot win the race or drive finality.
#[test]
fn fork_choice_ignores_unregistered_cosigners() {
    let validators: Vec<Address> = (0..3).map(|_| addr()).collect();
    let vs = ValidatorSet::new(validators.clone());
    let reg = registry(3);

    let (mut chain, genesis) = Chain::new_with_genesis(validators[0], 0);

    // Honest block: two genuinely registered co-signers.
    let mut honest = make_block(1, genesis.hash(), validators[1]);
    consensus::sign_block_bls(&mut honest, &bls_sk(0), 0).unwrap();
    consensus::sign_block_bls(&mut honest, &bls_sk(1), 1).unwrap();

    // Attacker's competing block: 50 foreign co-signers, no bitmap.
    let mut attacker = make_block(1, genesis.hash(), validators[2]);
    attacker.header.timestamp = 99;
    forge_quorum_with_foreign_keys(&mut attacker, 50);

    let candidates = vec![honest.clone(), attacker.clone()];
    let winner = consensus::canonical_head(&candidates, &vs, &reg).expect("a head");
    assert_eq!(
        winner.hash(),
        honest.hash(),
        "fork-choice must prefer real registered co-signatures over 50 forged ones"
    );

    // And the forged block must not finalize.
    chain.push(attacker);
    chain.advance_finality(&vs, &reg);
    assert_eq!(
        chain.finalized_height(),
        0,
        "a block with no registered co-signers must never finalize"
    );
}

/// VINX-01 / VX-RED-002 — naming a validator in `header.validator` is not authorship.
#[test]
fn proposer_impersonation_is_rejected() {
    let validators: Vec<Address> = (0..3).map(|_| addr()).collect();
    let vs = ValidatorSet::new(validators.clone());
    let reg = registry(3);

    // An attacker with no validator key names an honest validator as proposer.
    let block = make_block(1, [0u8; 32], validators[1]);
    assert!(
        consensus::verify_proposer_authenticated(&block, &vs, &reg).is_err(),
        "a block with no proposer co-signature must be refused"
    );

    // Padding it with foreign keys does not help either.
    let mut forged = make_block(1, [0u8; 32], validators[1]);
    forge_quorum_with_foreign_keys(&mut forged, 3);
    assert!(
        consensus::verify_proposer_authenticated(&forged, &vs, &reg).is_err(),
        "foreign keys must not authenticate a proposer"
    );

    // Co-signing at someone else's index does not impersonate the named proposer:
    // validators[1] is at index 1, but only index 0 signed.
    let mut wrong_slot = make_block(1, [0u8; 32], validators[1]);
    consensus::sign_block_bls(&mut wrong_slot, &bls_sk(0), 0).unwrap();
    assert!(
        consensus::verify_proposer_authenticated(&wrong_slot, &vs, &reg).is_err(),
        "the proposer's own bitmap bit must be set"
    );

    // The genuine proposer, signing with its registered key, is accepted.
    let mut honest = make_block(1, [0u8; 32], validators[1]);
    consensus::sign_block_bls(&mut honest, &bls_sk(1), 1).unwrap();
    consensus::verify_proposer_authenticated(&honest, &vs, &reg)
        .expect("a genuinely signed block must be accepted");
}

/// VINX-08 — `reorg_replace` must respect `height_base`, or every snapshot-synced node
/// panics on the first competing block it receives.
#[test]
fn reorg_replace_is_height_base_aware() {
    let v = addr();
    let snap_block = make_block(1000, [7u8; 32], v);
    let mut chain = Chain::new_from_snapshot(snap_block.clone());

    let b1001 = make_block(1001, snap_block.hash(), v);
    chain.push(b1001);
    assert_eq!(chain.tip_height(), 1001);

    let mut competing = make_block(1001, snap_block.hash(), v);
    competing.header.timestamp = 12345;
    let competing_hash = competing.hash();

    // Used to panic with "start index out of range" — a remote node kill.
    let removed = chain.reorg_replace(1001, competing);
    assert_eq!(removed.len(), 1, "exactly the replaced block is removed");
    assert_eq!(chain.tip_height(), 1001, "tip stays at the reorged height");
    assert_eq!(
        chain.tip_hash(),
        competing_hash,
        "the candidate is installed"
    );
    assert_eq!(chain.height_base, 1000, "height_base is preserved");

    // A height above the tip is out of range: handled by returning nothing, not by
    // panicking. (Heights at or below finality are excluded by the caller's contract.)
    assert!(chain
        .reorg_replace(5000, make_block(5000, [0u8; 32], v))
        .is_empty());
}

/// Deployment invariant surfaced by the VINX-01 fix: a proposer whose BLS key is NOT
/// registered on-chain cannot be authenticated, so its blocks are refused. This is the
/// intended (and only possible) secure behaviour — you cannot verify authorship against
/// a key you do not have — but it makes on-chain BLS registration a hard prerequisite
/// for block acceptance. See audit/post-fix/FINDINGS_STATUS.md "Deployment prerequisite".
#[test]
fn unregistered_proposer_is_refused_and_registration_is_a_prerequisite() {
    let validators: Vec<Address> = (0..3).map(|_| addr()).collect();
    let vs = ValidatorSet::new(validators.clone());

    // A genuine proposer signing with its real key, exactly as `producer.rs` does.
    let mut block = make_block(1, [0u8; 32], validators[1]);
    consensus::sign_block_bls(&mut block, &bls_sk(1), 1).unwrap();

    // Registry empty (the state of a fresh chain: genesis registers no BLS keys).
    let empty_registry: Vec<Option<[u8; 48]>> = vec![None; 3];
    assert!(
        consensus::verify_proposer_authenticated(&block, &vs, &empty_registry).is_err(),
        "an unregistered proposer cannot be authenticated"
    );

    // Once the key is registered, the same block authenticates.
    consensus::verify_proposer_authenticated(&block, &vs, &registry(3))
        .expect("a registered proposer's block is accepted");
}
