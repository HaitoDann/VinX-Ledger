// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Paiements groupés et horodatage natif (protocole 4).

mod common;

use common::*;
use vinx_core::{Payment, TxBody};
use vinx_crypto::{Address, KeyPair};
use vinx_state::{BlockError, TxError};

const FEE: u64 = VINX / 1000;

fn tx_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidTx { error, .. }) => error,
        other => panic!("attendu une transaction invalide, obtenu {other:?}"),
    }
}

fn pay(to: Address, amount: u64) -> Payment {
    Payment {
        to,
        amount,
        memo: None,
    }
}

fn batch(
    from: &KeyPair,
    nonce: u64,
    payments: Vec<Payment>,
    fee: u64,
    first: bool,
) -> vinx_core::SignedTransaction {
    vinx_core::Transaction {
        chain_id: CHAIN.into(),
        sender: addr(from),
        nonce,
        fee,
        body: TxBody::BatchTransfer { payments },
    }
    .sign(from, first)
}

#[test]
fn batch_pays_everyone_at_once() {
    let mut net = Net::current();
    let alice = net.funded(100 * VINX);
    let people: Vec<Address> = (0..12).map(|_| addr(&KeyPair::generate())).collect();
    let payments = people.iter().map(|a| pay(*a, VINX)).collect();
    // 12 paiements : deux frais minimaux (un par tranche de 10).
    let tx = batch(&alice, 1, payments, 2 * FEE, true);
    net.block(T0, &[], &[tx]).unwrap();
    for a in &people {
        assert_eq!(net.state.balance(a), VINX);
    }
    assert_eq!(
        net.state.balance(&addr(&alice)),
        100 * VINX - 12 * VINX - 2 * FEE
    );
    net.state.check_invariant().unwrap();
}

#[test]
fn batch_rules() {
    let mut net = Net::current();
    let alice = net.funded(1_000 * VINX);
    let bob = addr(&KeyPair::generate());
    let cases = [
        (vec![], FEE, TxError::InvalidBatch),
        (vec![pay(bob, VINX); 101], 11 * FEE, TxError::InvalidBatch),
        (vec![pay(bob, VINX); 11], FEE, TxError::FeeTooLow),
        (
            vec![pay(bob, VINX), pay(addr(&alice), VINX)],
            FEE,
            TxError::SelfTransfer,
        ),
        (vec![pay(bob, VINX), pay(bob, 0)], FEE, TxError::ZeroAmount),
        // Un nouveau compte doit recevoir au moins la réserve.
        (vec![pay(bob, 1)], FEE, TxError::BelowReserve),
        (
            vec![pay(bob, 600 * VINX), pay(bob, 600 * VINX)],
            FEE,
            TxError::InsufficientBalance,
        ),
    ];
    for (payments, fee, expected) in cases {
        let tx = batch(&alice, 1, payments, fee, true);
        assert_eq!(tx_err(net.block(T0, &[], &[tx])), expected);
    }
    // Rien n'a été appliqué : le compte n'a pas bougé.
    assert_eq!(net.state.balance(&addr(&alice)), 1_000 * VINX);
}

#[test]
fn attest_costs_only_the_fee() {
    let mut net = Net::current();
    let alice = net.funded(10 * VINX);
    let tx = user_tx(&alice, 1, TxBody::Attest { digest: [7; 32] }, true);
    net.block(T0, &[], &[tx]).unwrap();
    assert_eq!(net.state.balance(&addr(&alice)), 10 * VINX - FEE);
    net.state.check_invariant().unwrap();
}

#[test]
fn new_transactions_wait_for_protocol_4() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let tx = user_tx(&alice, 1, TxBody::Attest { digest: [7; 32] }, true);
    assert_eq!(tx_err(net.block(T0, &[], &[tx])), TxError::NotYetActive);
}
