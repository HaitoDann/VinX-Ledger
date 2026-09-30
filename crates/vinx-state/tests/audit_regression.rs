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
        validator_bls: vinx_state::GenesisBlsKey::from_secret(
            &vinx_crypto::BlsSecretKey::generate(),
            &validator,
            vinx_core::CHAIN_ID_DEVNET,
        ),
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

/// VINX-11 — deux protections distinctes, à vérifier séparément.
///
/// 1. Une PoP recopiée depuis l'état public d'un autre validateur échoue à la
///    **vérification** : elle est liée à l'adresse de son propriétaire (`pop_message`).
/// 2. Même avec une PoP correctement forgée pour lui-même, un validateur ne peut pas
///    revendiquer une clé G1 déjà enregistrée — l'unicité reste nécessaire, sinon deux bits
///    du bitmap résoudraient vers la même clé et une signature réelle compterait deux fois.
#[test]
fn bls_key_cannot_be_registered_by_two_validators() {
    use vinx_crypto::BlsSecretKey;

    let v1 = KeyPair::generate();
    let v1_addr = Address::from_public_key(&v1.public_key());
    let v2 = KeyPair::generate();
    let v2_addr = Address::from_public_key(&v2.public_key());

    let mut state = genesis_with(v1_addr);
    let chain_id = state.chain_id;
    let bond = Amount::from_vinx(200_000);

    // v1 entre au pool avec SA clé BLS (ADR 0075 §3.1 : le bond la porte).
    let v1_bls = BlsSecretKey::generate();
    state.credit_for_test(v1_addr, bond.saturating_add(Amount::from_vinx(1_000)));
    state
        .apply_transaction(&Transaction::new_stake_with_bls(
            &v1,
            bond,
            Amount::ZERO,
            0,
            &v1_bls,
            chain_id,
        ))
        .expect("v1 bonds with its own BLS key");

    // v2 entre au pool avec une clé différente.
    let v2_bls = BlsSecretKey::generate();
    state.credit_for_test(v2_addr, bond.saturating_add(Amount::from_vinx(1_000)));
    state
        .apply_transaction(&Transaction::new_stake_with_bls(
            &v2,
            bond,
            Amount::ZERO,
            0,
            &v2_bls,
            chain_id,
        ))
        .expect("v2 bonds with its own BLS key");

    // (1) v2 recopie la clé ET la PoP de v1, telles qu'elles figurent dans l'état.
    let copied = vinx_core::transaction::RegisterBlsKeyPayload {
        bls_pub_key: v1_bls.public_key().0.to_vec(),
        bls_pop: v1_bls
            .proof_of_possession(v1_addr.as_bytes(), chain_id)
            .0
            .to_vec(),
        operator: None,
    };
    assert!(
        state
            .apply_transaction(&Transaction::new_register_bls_key(&v2, &copied, 1))
            .is_err(),
        "une PoP liée à v1 ne doit pas valider pour v2"
    );

    // (2) v2 forge une PoP correcte pour lui-même sur la clé de v1 — impossible sans la
    //     clé secrète, simulé ici pour isoler le contrôle d'unicité.
    let own_pop = vinx_core::transaction::RegisterBlsKeyPayload {
        bls_pub_key: v1_bls.public_key().0.to_vec(),
        bls_pop: v1_bls
            .proof_of_possession(v2_addr.as_bytes(), chain_id)
            .0
            .to_vec(),
        operator: None,
    };
    assert!(
        state
            .apply_transaction(&Transaction::new_register_bls_key(&v2, &own_pop, 1))
            .is_err(),
        "la même clé G1 ne doit pas être revendiquée par un second validateur"
    );
}

/// ADR 0075 §3.1 — un bond qui fait entrer au pool doit porter la clé BLS. Sans elle,
/// « tout validateur actif est authentifiable » n'est qu'une convergence, et `quorum()`
/// comptant tous les membres, un set peuplé de membres sans clé fige la finalité.
#[test]
fn bond_entering_the_pool_must_carry_a_bls_key() {
    use vinx_crypto::BlsSecretKey;

    let v = KeyPair::generate();
    let addr = Address::from_public_key(&v.public_key());
    let mut state = genesis_with(addr);
    let chain_id = state.chain_id;
    let bond = Amount::from_vinx(200_000);
    state.credit_for_test(addr, bond.saturating_add(Amount::from_vinx(2_000)));

    // Bond nu : refusé, et le nonce ne doit pas être consommé.
    let nonce_before = state.get_account(&addr).map(|a| a.nonce).unwrap_or(0);
    assert!(
        state
            .apply_transaction(&Transaction::new_stake(&v, bond, Amount::ZERO, 0))
            .is_err(),
        "un bond sans clé BLS ne doit pas faire entrer au pool"
    );
    assert!(!state.validator_pool.contains_key(&addr));
    assert_eq!(
        state.get_account(&addr).map(|a| a.nonce).unwrap_or(0),
        nonce_before,
        "un bond refusé ne doit pas consommer le nonce"
    );

    // Bond portant la clé : accepté, et la clé est enregistrée d'emblée.
    let bls = BlsSecretKey::generate();
    state
        .apply_transaction(&Transaction::new_stake_with_bls(
            &v,
            bond,
            Amount::ZERO,
            0,
            &bls,
            chain_id,
        ))
        .expect("un bond portant la clé BLS entre au pool");
    assert_eq!(
        state
            .validator_pool
            .get(&addr)
            .and_then(|e| e.bls_pub_key.clone()),
        Some(bls.public_key().0.to_vec()),
        "la clé doit être enregistrée dès l'entrée au pool"
    );

    // Un bond portant une PoP d'une autre chaîne est refusé (VINX-11).
    let other = KeyPair::generate();
    let other_addr = Address::from_public_key(&other.public_key());
    state.credit_for_test(other_addr, bond.saturating_add(Amount::from_vinx(2_000)));
    assert!(
        state
            .apply_transaction(&Transaction::new_stake_with_bls(
                &other,
                bond,
                Amount::ZERO,
                0,
                &BlsSecretKey::generate(),
                chain_id.wrapping_add(1),
            ))
            .is_err(),
        "une PoP d'une autre chaîne ne doit pas être acceptée au bonding"
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

    // The key is mandatory (ADR 0082): the genesis validator is authenticable from block 1.
    let state = create_genesis_state(&GenesisConfig {
        chain_id: vinx_core::CHAIN_ID_DEVNET,
        admin_address: admin,
        validator_address: validator,
        validator_bls: vinx_state::GenesisBlsKey::from_secret(
            &sk,
            &validator,
            vinx_core::CHAIN_ID_DEVNET,
        ),
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
        validator_bls: vinx_state::GenesisBlsKey {
            pub_key: sk.public_key().0,
            // PoP d'une autre clé — la genèse doit refuser d'écrire une clé invérifiable.
            pop: other
                .proof_of_possession(validator.as_bytes(), vinx_core::CHAIN_ID_DEVNET)
                .0,
        },
    });
}

/// VINX-13 — `payload` was unbounded while the fee derives from `amount` alone, so a
/// zero-value transaction carrying megabytes cost only the fee floor: free block bloat.
/// The bound must hold on the consensus path, not just at mempool admission, or an
/// oversized payload still enters state inside a block.
#[test]
fn oversized_payload_is_rejected_on_every_path() {
    let sender = KeyPair::generate();
    let sender_addr = Address::from_public_key(&sender.public_key());
    let recipient = Address::from_public_key(&KeyPair::generate().public_key());

    let mut state = genesis_with(sender_addr);
    state.credit_for_test(sender_addr, Amount::from_vinx(1_000));

    let amount = Amount::from_atoms(0);
    let mut tx = Transaction::new_transfer(&sender, recipient, amount, floor(), 0);
    // A transfer carries at most a memo (ADR 0085).
    tx.payload = vec![0u8; vinx_core::amount::MAX_MEMO_BYTES + 1];
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
    ok.payload = vec![0u8; vinx_core::amount::MAX_MEMO_BYTES];
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

/// Passe adversariale (auto-audit) — VINX-20 n'était corrigé qu'à moitié. Le premier
/// correctif avait traité `apply_admin_action` et laissé `check_admin`, qui garde
/// `AnnounceUpgrade`, sur l'ancien motif permissif : sans admin configuré, n'importe qui
/// pouvait planifier une mise à jour de protocole.
#[test]
fn announce_upgrade_is_refused_when_no_authority_is_configured() {
    let attacker = KeyPair::generate();
    let attacker_addr = Address::from_public_key(&attacker.public_key());

    let mut state = genesis_with(attacker_addr);
    state.admin_address = None;
    state.admin_policy = None;
    state.credit_for_test(attacker_addr, Amount::from_vinx(1_000));

    let tx = Transaction::new_announce_upgrade(
        &attacker,
        vinx_core::protocol::ProtocolVersion {
            major: 9,
            minor: 9,
            patch: 9,
        },
        u64::MAX,
        0,
    );
    assert_eq!(
        state.apply_transaction(&tx),
        Err(vinx_core::CoreError::Unauthorized),
        "planifier une mise à jour doit échouer fermé sans autorité admin"
    );
}

/// Passe adversariale — `emission_started` gouverne l'émission *et* la clôture d'époque,
/// et n'était engagé qu'indirectement via `emission_epoch_ts`, donc invisible quand
/// celui-ci vaut 0. Il est désormais engagé.
#[test]
fn state_root_commits_emission_flag() {
    let admin = Address::from_public_key(&KeyPair::generate().public_key());

    let base = genesis_with(admin);
    let baseline = base.clone().compute_consensus_root();

    // Un état identique sauf `emission_started` doit avoir une racine différente.
    let mut started = base.clone();
    started.settle_block(&admin, 0); // établit l'époque à t=0 → emission_epoch_ts reste 0
    assert_ne!(
        started.compute_consensus_root(),
        baseline,
        "emission_started doit être engagé même quand emission_epoch_ts vaut 0"
    );
}
