// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Mandats de prélèvement et frais payés par un parrain (protocole 4).

mod common;

use common::*;
use vinx_core::{PolicyConfig, SignedTransaction, Transaction, TxBody};
use vinx_crypto::{Address, KeyPair};
use vinx_state::{BlockError, MandateError, PolicyError, TxError};

const FEE: u64 = VINX / 1000;
const DAY: u64 = 86_400;
const MONTH: u64 = 30 * DAY;

fn tx_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidTx { error, .. }) => error,
        other => panic!("attendu une transaction invalide, obtenu {other:?}"),
    }
}

fn grant(
    payer: &KeyPair,
    nonce: u64,
    payee: Address,
    max: u64,
    expires_at: Option<u64>,
    first: bool,
) -> SignedTransaction {
    user_tx(
        payer,
        nonce,
        TxBody::GrantMandate {
            payee,
            max_per_period: max,
            period: MONTH,
            expires_at,
        },
        first,
    )
}

fn collect(
    payee: &KeyPair,
    nonce: u64,
    payer: Address,
    amount: u64,
    first: bool,
) -> SignedTransaction {
    user_tx(
        payee,
        nonce,
        TxBody::Collect {
            payer,
            amount,
            memo: Some(b"abonnement".to_vec()),
        },
        first,
    )
}

#[test]
fn subscription_is_collected_within_the_monthly_limit() {
    let mut net = Net::current();
    let alice = net.funded(100 * VINX);
    let shop = net.funded(VINX);
    net.block(
        T0,
        &[],
        &[grant(&alice, 1, addr(&shop), 10 * VINX, None, true)],
    )
    .unwrap();
    net.block(
        T0 + DAY,
        &[],
        &[collect(&shop, 1, addr(&alice), 6 * VINX, true)],
    )
    .unwrap();
    // 6 + 5 > 10 dans le même mois : refusé.
    assert_eq!(
        tx_err(net.block(
            T0 + 2 * DAY,
            &[],
            &[collect(&shop, 2, addr(&alice), 5 * VINX, false)]
        )),
        TxError::Mandate(MandateError::Exceeded)
    );
    net.block(
        T0 + 2 * DAY,
        &[],
        &[collect(&shop, 2, addr(&alice), 4 * VINX, false)],
    )
    .unwrap();
    // Le mois suivant, la limite repart.
    net.block(
        T0 + MONTH,
        &[],
        &[collect(&shop, 3, addr(&alice), 10 * VINX, false)],
    )
    .unwrap();
    assert_eq!(
        net.state.balance(&addr(&alice)),
        100 * VINX - 20 * VINX - FEE
    );
    assert_eq!(net.state.balance(&addr(&shop)), VINX + 20 * VINX - 3 * FEE);

    // Un autre bénéficiaire n'a aucun droit.
    let thief = net.funded(VINX);
    assert_eq!(
        tx_err(net.block(
            T0 + MONTH,
            &[],
            &[collect(&thief, 1, addr(&alice), VINX, true)]
        )),
        TxError::Mandate(MandateError::Unknown)
    );
    // Révoqué : plus de prélèvement.
    let revoke = user_tx(
        &alice,
        2,
        TxBody::RevokeMandate { payee: addr(&shop) },
        false,
    );
    net.block(T0 + MONTH, &[], &[revoke]).unwrap();
    assert_eq!(
        tx_err(net.block(
            T0 + 2 * MONTH,
            &[],
            &[collect(&shop, 4, addr(&alice), VINX, false)]
        )),
        TxError::Mandate(MandateError::Unknown)
    );
    net.state.check_invariant().unwrap();
}

#[test]
fn mandate_rules() {
    let mut net = Net::current();
    let alice = net.funded(10 * VINX);
    let shop = net.funded(VINX);
    let bad_period = user_tx(
        &alice,
        1,
        TxBody::GrantMandate {
            payee: addr(&shop),
            max_per_period: VINX,
            period: 60,
            expires_at: None,
        },
        true,
    );
    assert_eq!(
        tx_err(net.block(T0, &[], &[bad_period])),
        TxError::Mandate(MandateError::Invalid)
    );
    assert_eq!(
        tx_err(net.block(T0, &[], &[grant(&alice, 1, addr(&alice), VINX, None, true)])),
        TxError::Mandate(MandateError::Invalid)
    );
    // Échéance : plus rien après.
    net.block(
        T0,
        &[],
        &[grant(
            &alice,
            1,
            addr(&shop),
            100 * VINX,
            Some(T0 + DAY),
            true,
        )],
    )
    .unwrap();
    assert_eq!(
        tx_err(net.block(
            T0 + DAY,
            &[],
            &[collect(&shop, 1, addr(&alice), VINX, true)]
        )),
        TxError::Mandate(MandateError::Expired)
    );
    // Le prélèvement ne peut pas laisser le payeur sous la réserve.
    let drain = collect(&shop, 1, addr(&alice), 10 * VINX - FEE - 1, true);
    assert_eq!(
        tx_err(net.block(T0 + 10, &[], &[drain])),
        TxError::BelowReserve
    );
}

#[test]
fn granting_a_mandate_needs_approval_when_the_account_has_rules() {
    let mut net = Net::current();
    let phone = net.funded(100 * VINX);
    let policy = PolicyConfig {
        signers: vec![KeyPair::generate().public_key()],
        threshold: 1,
        daily_limit: Some(50 * VINX),
    };
    net.block(
        T0,
        &[],
        &[user_tx(&phone, 1, TxBody::SetPolicy { policy }, true)],
    )
    .unwrap();
    let g = grant(&phone, 2, addr(&KeyPair::generate()), VINX, None, false);
    assert_eq!(
        tx_err(net.block(T0, &[], &[g])),
        TxError::Policy(PolicyError::ApprovalsMissing)
    );
}

fn sponsored(
    from: &KeyPair,
    nonce: u64,
    body: TxBody,
    sponsor: &KeyPair,
    signer: &KeyPair,
    first: bool,
) -> SignedTransaction {
    let draft = Transaction {
        chain_id: CHAIN.into(),
        sender: addr(from),
        nonce,
        fee: FEE,
        body: body.clone(),
    };
    let signature = signer.sign(&draft.sponsor_message(&addr(sponsor)));
    Transaction {
        body: TxBody::Sponsored {
            body: Box::new(body),
            sponsor: addr(sponsor),
            signature,
        },
        ..draft
    }
    .sign(from, first)
}

#[test]
fn sponsor_pays_the_fee() {
    let mut net = Net::current();
    let alice = net.funded(10 * VINX);
    let shop = net.funded(10 * VINX);
    // La boutique doit avoir sa clé en état : une première transaction.
    net.block(
        T0,
        &[],
        &[user_tx(
            &shop,
            1,
            TxBody::SetMemoRequired { required: false },
            true,
        )],
    )
    .unwrap();
    let pay = TxBody::Transfer {
        to: addr(&shop),
        amount: 10 * VINX,
        memo: None,
    };
    // Alice paie exactement 10 VINX : sans parrain, les frais manqueraient.
    let tx = sponsored(&alice, 1, pay.clone(), &shop, &shop, true);
    net.block(T0, &[], &[tx]).unwrap();
    assert_eq!(net.state.balance(&addr(&alice)), 0);
    assert_eq!(net.state.balance(&addr(&shop)), 20 * VINX - 2 * FEE);
    net.state.check_invariant().unwrap();

    // Signature du parrain falsifiée : refusé.
    let bob = net.funded(10 * VINX);
    let forged = sponsored(&bob, 1, pay, &shop, &KeyPair::generate(), true);
    assert_eq!(tx_err(net.block(T0, &[], &[forged])), TxError::BadSponsor);
}

#[test]
fn envelopes_must_be_in_order() {
    let mut net = Net::current();
    let alice = net.funded(10 * VINX);
    let nested = TxBody::Approved {
        body: Box::new(TxBody::Sponsored {
            body: Box::new(TxBody::Attest { digest: [1; 32] }),
            sponsor: Address::treasury(),
            signature: alice.sign(b"x"),
        }),
        approvals: vec![],
    };
    assert_eq!(
        tx_err(net.block(T0, &[], &[user_tx(&alice, 1, nested, true)])),
        TxError::BadEnvelope
    );
}

#[test]
fn mandates_survive_storage_round_trip() {
    let mut net = Net::current();
    let alice = net.funded(10 * VINX);
    net.block(
        T0,
        &[],
        &[grant(
            &alice,
            1,
            addr(&KeyPair::generate()),
            VINX,
            None,
            true,
        )],
    )
    .unwrap();
    let back =
        vinx_state::State::from_parts(&net.state.globals_bytes(), net.state.accounts.clone())
            .unwrap();
    assert_eq!(back, net.state);
    assert_eq!(back.state_root(), net.state.state_root());
}
