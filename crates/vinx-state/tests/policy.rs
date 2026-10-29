// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Règles de compte : multisig et plafond journalier (protocole 4).

mod common;

use common::*;
use vinx_core::{PolicyConfig, SignedTransaction, Transaction, TxBody};
use vinx_crypto::{Address, KeyPair};
use vinx_state::{BlockError, PolicyError, TxError};

const FEE: u64 = VINX / 1000;
const DAY: u64 = 86_400;

fn tx_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidTx { error, .. }) => error,
        other => panic!("attendu une transaction invalide, obtenu {other:?}"),
    }
}

fn unsigned(from: &KeyPair, nonce: u64, body: TxBody) -> Transaction {
    Transaction {
        chain_id: CHAIN.into(),
        sender: addr(from),
        nonce,
        fee: FEE,
        body,
    }
}

/// `body` enveloppé avec les approbations des signataires `(indice, clé)`.
fn approved(
    from: &KeyPair,
    nonce: u64,
    body: TxBody,
    signers: &[(u8, &KeyPair)],
) -> SignedTransaction {
    let draft = unsigned(from, nonce, body.clone());
    let approvals = signers.iter().map(|(i, k)| draft.approve(*i, k)).collect();
    unsigned(
        from,
        nonce,
        TxBody::Approved {
            body: Box::new(body),
            approvals,
        },
    )
    .sign(from, false)
}

fn pay(to: Address, amount: u64) -> TxBody {
    TxBody::Transfer {
        to,
        amount,
        memo: None,
    }
}

#[test]
fn multisig_account_needs_its_approvals() {
    let mut net = Net::current();
    let asso = net.funded(100 * VINX);
    let members: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
    let policy = PolicyConfig {
        signers: members.iter().map(KeyPair::public_key).collect(),
        threshold: 2,
        daily_limit: None,
    };
    let set = user_tx(&asso, 1, TxBody::SetPolicy { policy }, true);
    net.block(T0, &[], &[set]).unwrap();
    let bob = addr(&KeyPair::generate());

    // La clé du compte seule ne suffit plus.
    let alone = user_tx(&asso, 2, pay(bob, VINX), false);
    assert_eq!(
        tx_err(net.block(T0, &[], &[alone])),
        TxError::Policy(PolicyError::ApprovalsMissing)
    );
    // Une approbation sur deux : refusé.
    let one = approved(&asso, 2, pay(bob, VINX), &[(0, &members[0])]);
    assert_eq!(
        tx_err(net.block(T0, &[], &[one])),
        TxError::Policy(PolicyError::ApprovalsMissing)
    );
    // Signature d'une clé étrangère : refusé.
    let forged = approved(
        &asso,
        2,
        pay(bob, VINX),
        &[(0, &members[0]), (1, &KeyPair::generate())],
    );
    assert_eq!(
        tx_err(net.block(T0, &[], &[forged])),
        TxError::Policy(PolicyError::InvalidApprovals)
    );
    // Deux approbations : accepté.
    let ok = approved(
        &asso,
        2,
        pay(bob, VINX),
        &[(0, &members[0]), (2, &members[2])],
    );
    net.block(T0, &[], &[ok]).unwrap();
    assert_eq!(net.state.balance(&bob), VINX);

    // Changer la clé du compte exige aussi les approbations.
    let rotate = user_tx(
        &asso,
        3,
        TxBody::RotateKey {
            new_key: KeyPair::generate().public_key(),
        },
        false,
    );
    assert_eq!(
        tx_err(net.block(T0, &[], &[rotate])),
        TxError::Policy(PolicyError::ApprovalsMissing)
    );
}

#[test]
fn daily_limit_with_a_vault_key() {
    let mut net = Net::current();
    let phone = net.funded(1_000 * VINX);
    let vault = KeyPair::generate();
    let policy = PolicyConfig {
        signers: vec![vault.public_key()],
        threshold: 1,
        daily_limit: Some(50 * VINX),
    };
    net.block(
        T0,
        &[],
        &[user_tx(&phone, 1, TxBody::SetPolicy { policy }, true)],
    )
    .unwrap();
    let shop = addr(&KeyPair::generate());

    // 30 + 20 VINX dans la journée : permis.
    net.block(
        T0 + 10,
        &[],
        &[user_tx(&phone, 2, pay(shop, 30 * VINX), false)],
    )
    .unwrap();
    net.block(
        T0 + 20,
        &[],
        &[user_tx(&phone, 3, pay(shop, 20 * VINX), false)],
    )
    .unwrap();
    // Un VINX de plus : refusé.
    assert_eq!(
        tx_err(net.block(T0 + 30, &[], &[user_tx(&phone, 4, pay(shop, VINX), false)])),
        TxError::Policy(PolicyError::DailyLimitExceeded)
    );
    // Avec la clé coffre, pas de plafond.
    let big = approved(&phone, 4, pay(shop, 500 * VINX), &[(0, &vault)]);
    net.block(T0 + 40, &[], &[big]).unwrap();
    // Le lendemain, le plafond repart à zéro.
    net.block(
        T0 + DAY,
        &[],
        &[user_tx(&phone, 5, pay(shop, 40 * VINX), false)],
    )
    .unwrap();
    assert_eq!(net.state.balance(&shop), 590 * VINX);

    // Le téléphone seul ne peut pas lever le plafond.
    let lift = user_tx(
        &phone,
        6,
        TxBody::SetPolicy {
            policy: PolicyConfig::default(),
        },
        false,
    );
    assert_eq!(
        tx_err(net.block(T0 + DAY, &[], &[lift])),
        TxError::Policy(PolicyError::ApprovalsMissing)
    );
    // Avec le coffre, la politique est retirée.
    let lift = approved(
        &phone,
        6,
        TxBody::SetPolicy {
            policy: PolicyConfig::default(),
        },
        &[(0, &vault)],
    );
    net.block(T0 + DAY, &[], &[lift]).unwrap();
    assert!(net.state.policies.get(&addr(&phone)).is_none());
    net.state.check_invariant().unwrap();
}

#[test]
fn invalid_policies_and_stray_approvals_are_refused() {
    let mut net = Net::current();
    let alice = net.funded(100 * VINX);
    let k = KeyPair::generate().public_key();
    for policy in [
        PolicyConfig {
            signers: vec![k.clone()],
            threshold: 2,
            daily_limit: None,
        },
        PolicyConfig {
            signers: vec![k.clone(), k.clone()],
            threshold: 1,
            daily_limit: None,
        },
        PolicyConfig {
            signers: vec![],
            threshold: 0,
            daily_limit: Some(VINX),
        },
        PolicyConfig {
            signers: (0..11).map(|_| KeyPair::generate().public_key()).collect(),
            threshold: 1,
            daily_limit: None,
        },
    ] {
        assert_eq!(
            tx_err(net.block(
                T0,
                &[],
                &[user_tx(&alice, 1, TxBody::SetPolicy { policy }, true)]
            )),
            TxError::Policy(PolicyError::InvalidConfig)
        );
    }
    // Des approbations sur un compte sans politique n'ont pas de sens.
    let mut stray = approved(
        &alice,
        1,
        pay(addr(&KeyPair::generate()), VINX),
        &[(0, &KeyPair::generate())],
    );
    stray = stray.tx.sign(&alice, true);
    assert_eq!(
        tx_err(net.block(T0, &[], &[stray])),
        TxError::Policy(PolicyError::InvalidApprovals)
    );
}

#[test]
fn policies_survive_storage_round_trip() {
    let mut net = Net::current();
    let alice = net.funded(100 * VINX);
    let policy = PolicyConfig {
        signers: vec![KeyPair::generate().public_key()],
        threshold: 1,
        daily_limit: Some(VINX),
    };
    net.block(
        T0,
        &[],
        &[user_tx(&alice, 1, TxBody::SetPolicy { policy }, true)],
    )
    .unwrap();
    let back =
        vinx_state::State::from_parts(&net.state.globals_bytes(), net.state.accounts.clone())
            .unwrap();
    assert_eq!(back, net.state);
    assert_eq!(back.state_root(), net.state.state_root());
}
