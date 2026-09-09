//! Security regression tests — state layer.
//!
//! Each test carries the attack from the audit proof-of-concept and asserts it is now
//! refused. All of them demonstrated a live vulnerability on the audited commit `8774806`.

use vinx_core::amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS};
use vinx_core::Transaction;
use vinx_crypto::{Address, KeyPair};
use vinx_state::{create_genesis_state, GenesisConfig, WorldState};

fn genesis_with(admin: Address) -> WorldState {
    let validator = Address::from_public_key(&KeyPair::generate().public_key());
    create_genesis_state(&GenesisConfig {
        chain_id: vinx_core::CHAIN_ID_DEVNET,
        admin_address: admin,
        validator_address: validator,
        validator_bls: None,
    })
}

fn floor() -> Amount {
    Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS)
}

/// VINX-03 / VX-RED-005 — naming an account as `sponsor` must not let an attacker spend
/// it. On the audited commit this drained the victim's entire balance: the mempool paths
/// verified the sender's signature only, and the producer applied the transaction with
/// `apply_transaction_trusted`, which skips crypto by contract.
#[test]
fn sponsor_without_signature_cannot_drain_an_account() {
    let attacker = KeyPair::generate();
    let attacker_addr = Address::from_public_key(&attacker.public_key());
    let victim = Address::from_public_key(&KeyPair::generate().public_key());

    let mut state = genesis_with(attacker_addr);
    state.credit_for_test(attacker_addr, Amount::from_vinx(1_000));
    let victim_balance = Amount::from_vinx(5_000);
    state.credit_for_test(victim, victim_balance);

    // Transfer of 0 VINX to self, fee = the victim's whole balance, sponsor = victim,
    // with no sponsor key or signature at all.
    let mut tx = Transaction::new_transfer(
        &attacker,
        attacker_addr,
        Amount::from_atoms(0),
        victim_balance,
        0,
    );
    tx.sponsor = Some(victim);
    tx.sponsor_pub_key = None;
    tx.sponsor_signature = None;
    tx.sign(&attacker);

    assert!(
        WorldState::verify_tx_signature_pure(&tx).is_err(),
        "canonical verification must reject a missing sponsor signature"
    );
    assert!(
        state.admission_check(&tx).is_err(),
        "mempool admission must reject it too — the queues must hold only valid txs"
    );
    assert_eq!(
        state.account_balance(&victim),
        victim_balance,
        "the victim's balance must be untouched"
    );
}

/// VINX-03 — a *forged* sponsor signature (right shape, wrong key) must also be refused.
#[test]
fn sponsor_with_forged_signature_is_rejected() {
    let attacker = KeyPair::generate();
    let attacker_addr = Address::from_public_key(&attacker.public_key());
    let victim_kp = KeyPair::generate();
    let victim = Address::from_public_key(&victim_kp.public_key());

    let mut tx = Transaction::new_transfer(
        &attacker,
        attacker_addr,
        Amount::from_atoms(0),
        Amount::from_vinx(5_000),
        0,
    );
    tx.sponsor = Some(victim);
    // Attacker supplies their own key/signature while naming the victim as sponsor.
    tx.sponsor_pub_key = Some(attacker.public_key());
    tx.sponsor_signature = Some(attacker.sign(&tx.signing_bytes()));
    tx.sign(&attacker);

    assert!(
        WorldState::verify_tx_signature_pure(&tx).is_err(),
        "the sponsor's key must derive to the sponsor's address"
    );
}

/// VINX-03 — a genuinely sponsored transaction still works: the fix must not break the
/// feature it protects.
#[test]
fn genuinely_sponsored_transaction_still_applies() {
    let sender = KeyPair::generate();
    let sender_addr = Address::from_public_key(&sender.public_key());
    let sponsor = KeyPair::generate();
    let sponsor_addr = Address::from_public_key(&sponsor.public_key());
    let recipient = Address::from_public_key(&KeyPair::generate().public_key());

    let mut state = genesis_with(sender_addr);
    state.credit_for_test(sender_addr, Amount::from_vinx(1_000));
    state.credit_for_test(sponsor_addr, Amount::from_vinx(1_000));

    let amount = Amount::from_vinx(10);
    let fee = amount.calculate_fee(floor());
    // NB: `with_sponsor` changes `signing_bytes` (it appends the sponsor marker), which
    // invalidates the sender signature produced by `new_transfer`. The sender must sign
    // last. See the ordering note on `Transaction::with_sponsor`.
    let mut tx =
        Transaction::new_transfer(&sender, recipient, amount, fee, 0).with_sponsor(&sponsor);
    tx.sign(&sender);

    WorldState::verify_tx_signature_pure(&tx).expect("a properly sponsored tx is valid");
    state.admission_check(&tx).expect("and is admissible");
    state.apply_transaction(&tx).expect("and applies");

    assert_eq!(state.account_balance(&recipient), amount);
    // The sponsor paid the fee, the sender only the amount.
    assert_eq!(
        state.account_balance(&sponsor_addr),
        Amount::from_vinx(1_000).checked_sub(fee).unwrap()
    );
}

/// VINX-11 — the same BLS G1 key must not be registrable by two different validators,
/// or one real signature counts twice in the bitmap.
#[test]
fn bls_key_cannot_be_registered_by_two_validators() {
    use vinx_crypto::BlsSecretKey;

    let v1 = KeyPair::generate();
    let v1_addr = Address::from_public_key(&v1.public_key());
    let v2 = KeyPair::generate();
    let v2_addr = Address::from_public_key(&v2.public_key());

    let mut state = genesis_with(v1_addr);
    let bond = Amount::from_vinx(200_000);
    for (kp, a) in [(&v1, v1_addr), (&v2, v2_addr)] {
        state.credit_for_test(a, bond.saturating_add(Amount::from_vinx(1_000)));
        let fee = bond.calculate_fee(floor());
        let stake = Transaction::new_stake(kp, bond, fee, 0);
        state.apply_transaction(&stake).expect("bond must succeed");
    }

    let bls_sk = BlsSecretKey::generate();
    let payload = vinx_core::transaction::RegisterBlsKeyPayload {
        bls_pub_key: bls_sk.public_key().0.to_vec(),
        bls_pop: bls_sk.proof_of_possession().0.to_vec(),
    };

    let reg1 = Transaction::new_register_bls_key(&v1, &payload, 1);
    state
        .apply_transaction(&reg1)
        .expect("first registration succeeds");

    // v2 copies v1's published key and PoP straight out of the state.
    let reg2 = Transaction::new_register_bls_key(&v2, &payload, 1);
    assert!(
        state.apply_transaction(&reg2).is_err(),
        "the same G1 key must not be claimed by a second validator"
    );
}

/// Bootstrap regression: the genesis validator's BLS key must be in the registry from
/// height 0, otherwise it cannot produce a block its peers accept — and it cannot
/// register the key either, because doing so requires a block peers accept.
#[test]
fn genesis_registers_the_validator_bls_key() {
    use vinx_crypto::BlsSecretKey;

    let admin = Address::from_public_key(&KeyPair::generate().public_key());
    let validator = Address::from_public_key(&KeyPair::generate().public_key());
    let sk = BlsSecretKey::generate();

    // Without the key, the registry is empty — the deadlock this fixes.
    let bare = create_genesis_state(&GenesisConfig {
        chain_id: vinx_core::CHAIN_ID_DEVNET,
        admin_address: admin,
        validator_address: validator,
        validator_bls: None,
    });
    assert_eq!(
        bare.indexed_bls_keys(&bare.validator_set),
        vec![None],
        "no key configured ⇒ empty registry (single-node use only)"
    );

    // With it, the genesis validator is authenticable from block 1.
    let state = create_genesis_state(&GenesisConfig {
        chain_id: vinx_core::CHAIN_ID_DEVNET,
        admin_address: admin,
        validator_address: validator,
        validator_bls: Some(vinx_state::GenesisBlsKey {
            pub_key: sk.public_key().0,
            pop: sk.proof_of_possession().0,
        }),
    });
    assert_eq!(
        state.indexed_bls_keys(&state.validator_set),
        vec![Some(sk.public_key().0)],
        "the genesis validator's key must be registered at height 0"
    );
}

/// The genesis writer must not accept an unverifiable key: a bad PoP would put a key
/// into state that no block can ever verify against.
#[test]
#[should_panic(expected = "Proof-of-Possession")]
fn genesis_rejects_an_invalid_bls_pop() {
    use vinx_crypto::BlsSecretKey;

    let admin = Address::from_public_key(&KeyPair::generate().public_key());
    let validator = Address::from_public_key(&KeyPair::generate().public_key());
    let sk = BlsSecretKey::generate();
    let other = BlsSecretKey::generate();

    let _ = create_genesis_state(&GenesisConfig {
        chain_id: vinx_core::CHAIN_ID_DEVNET,
        admin_address: admin,
        validator_address: validator,
        validator_bls: Some(vinx_state::GenesisBlsKey {
            pub_key: sk.public_key().0,
            pop: other.proof_of_possession().0, // PoP of a different key
        }),
    });
}

/// VINX-13 — `payload` was unbounded while the fee derives from `amount` alone, so a
/// zero-value transaction carrying megabytes cost only the fee floor: free block bloat.
/// The bound must hold on the consensus path, not just at mempool admission, or an
/// oversized payload still enters state inside a block.
#[test]
fn oversized_payload_is_rejected_on_every_path() {
    use vinx_core::amount::MAX_TX_PAYLOAD_BYTES;

    let sender = KeyPair::generate();
    let sender_addr = Address::from_public_key(&sender.public_key());
    let recipient = Address::from_public_key(&KeyPair::generate().public_key());

    let mut state = genesis_with(sender_addr);
    state.credit_for_test(sender_addr, Amount::from_vinx(1_000));

    let amount = Amount::from_atoms(0);
    let mut tx = Transaction::new_transfer(&sender, recipient, amount, floor(), 0);
    tx.payload = vec![0u8; MAX_TX_PAYLOAD_BYTES + 1];
    tx.sign(&sender);

    assert!(
        state.admission_check(&tx).is_err(),
        "mempool admission must reject an oversized payload"
    );
    assert!(
        state.clone().apply_transaction(&tx).is_err(),
        "the verified apply path must reject it"
    );
    assert!(
        state.apply_transaction_trusted(&tx).is_err(),
        "the block-application path must reject it too, or blocks disagree"
    );

    // Exactly at the limit is still accepted — the bound must not be off by one.
    let mut ok = Transaction::new_transfer(&sender, recipient, amount, floor(), 0);
    ok.payload = vec![0u8; MAX_TX_PAYLOAD_BYTES];
    ok.sign(&sender);
    state
        .admission_check(&ok)
        .expect("a payload at the limit is valid");
}

/// VINX-20 — an absent admin authority is not a permissive one. With no admin
/// configured, governance actions used to be executed for anyone.
#[test]
fn admin_action_is_refused_when_no_authority_is_configured() {
    use vinx_core::governance::GovernanceAction;

    let attacker = KeyPair::generate();
    let attacker_addr = Address::from_public_key(&attacker.public_key());
    let victim_validator = Address::from_public_key(&KeyPair::generate().public_key());

    let mut state = genesis_with(attacker_addr);
    state.admin_address = None; // "dev mode" — previously wide open
    state.admin_policy = None;
    state.credit_for_test(attacker_addr, Amount::from_vinx(1_000));

    let tx = Transaction::new_admin_action(
        &attacker,
        &GovernanceAction::RemoveValidator(victim_validator),
        0,
    );
    assert_eq!(
        state.apply_transaction(&tx),
        Err(vinx_core::CoreError::Unauthorized),
        "governance must fail closed when no admin authority exists"
    );
}
