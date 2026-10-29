// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Noms `.vinx` et séquestres entre utilisateurs (protocole 2).

mod common;

use common::*;
use vinx_core::{AdminAction, ParamId, TxBody, deal_id};
use vinx_crypto::{Address, KeyPair};
use vinx_state::registry::{DEFAULT_NAME_FEE, NAME_GRACE, NAME_PERIOD};
use vinx_state::{BlockError, TxError};

const FEE: u64 = VINX / 1000;

fn tx_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidTx { error, .. } | BlockError::InvalidAdminTx { error, .. }) => {
            error
        }
        other => panic!("attendu une transaction invalide, obtenu {other:?}"),
    }
}

/// Réseau passé au protocole 2.
fn v2() -> Net {
    let mut net = Net::new();
    net.state.protocol_version = 2;
    net
}

fn register(kp: &KeyPair, nonce: u64, name: &str, first: bool) -> vinx_core::SignedTransaction {
    user_tx(kp, nonce, TxBody::RegisterName { name: name.into() }, first)
}

#[test]
fn new_transactions_wait_for_protocol_2() {
    let mut net = Net::new();
    let alice = net.funded(100 * VINX);
    assert_eq!(
        tx_err(net.block(T0, &[], &[register(&alice, 1, "alice", true)])),
        TxError::NotYetActive
    );
    let fee = net.admin(AdminAction::SetParam {
        param: ParamId::NameFee,
        value: 1,
    });
    assert_eq!(tx_err(net.block(T0, &[fee], &[])), TxError::NotYetActive);
    // Avant le protocole 2, le registre ne change pas la racine d'état.
    let root = net.state.state_root();
    net.state.registry.name_fee = 1;
    assert_eq!(net.state.state_root(), root);
}

#[test]
fn register_renew_and_expire() {
    let mut net = v2();
    let alice = net.funded(100 * VINX);
    let bob = net.funded(100 * VINX);
    let treasury = net.state.balance(&Address::treasury());
    net.block(T0, &[], &[register(&alice, 1, "alice", true)])
        .unwrap();
    assert_eq!(
        net.state.balance(&addr(&alice)),
        100 * VINX - FEE - DEFAULT_NAME_FEE
    );
    assert_eq!(
        net.state.balance(&Address::treasury()),
        treasury + FEE + DEFAULT_NAME_FEE
    );
    let reg = &net.state.registry;
    assert_eq!(reg.resolve("alice", T0), Some(addr(&alice)));
    assert_eq!(reg.names.get("alice").unwrap().expires_at, T0 + NAME_PERIOD);

    // Pris par Alice, même pendant le délai de grâce après expiration.
    let late = T0 + NAME_PERIOD + NAME_GRACE - 1;
    assert_eq!(
        tx_err(net.block(late, &[], &[register(&bob, 1, "alice", true)])),
        TxError::NameTaken
    );
    assert_eq!(net.state.registry.resolve("alice", late), None);

    // Le renouvellement s'ajoute à l'échéance en cours.
    net.block(T0 + 10, &[], &[register(&alice, 2, "alice", false)])
        .unwrap();
    assert_eq!(
        net.state.registry.names.get("alice").unwrap().expires_at,
        T0 + 2 * NAME_PERIOD
    );

    // Libre pour tous après expiration et délai de grâce.
    let free = T0 + 2 * NAME_PERIOD + NAME_GRACE;
    net.block(free, &[], &[register(&bob, 1, "alice", true)])
        .unwrap();
    assert_eq!(net.state.registry.resolve("alice", free), Some(addr(&bob)));
}

#[test]
fn names_must_be_well_formed() {
    let mut net = v2();
    let alice = net.funded(100 * VINX);
    for bad in [
        "ab",
        "Alice",
        "-alice",
        "alice-",
        "al ice",
        "été",
        &"a".repeat(33),
    ] {
        assert_eq!(
            tx_err(net.block(T0, &[], &[register(&alice, 1, bad, true)])),
            TxError::InvalidName,
            "{bad}"
        );
    }
    net.block(T0, &[], &[register(&alice, 1, "cave-42", true)])
        .unwrap();
}

#[test]
fn name_transfer_and_fee_change() {
    let mut net = v2();
    let alice = net.funded(100 * VINX);
    let bob = KeyPair::generate();
    let fee = net.admin(AdminAction::SetParam {
        param: ParamId::NameFee,
        value: 0,
    });
    net.block(T0, &[fee], &[register(&alice, 1, "alice", true)])
        .unwrap();
    assert_eq!(net.state.balance(&addr(&alice)), 100 * VINX - FEE);
    let give = |nonce| {
        user_tx(
            &alice,
            nonce,
            TxBody::TransferName {
                name: "alice".into(),
                to: addr(&bob),
            },
            false,
        )
    };
    net.block(T0, &[], &[give(2)]).unwrap();
    assert_eq!(net.state.registry.resolve("alice", T0), Some(addr(&bob)));
    assert_eq!(
        tx_err(net.block(T0, &[], &[give(3)])),
        TxError::NotNameOwner
    );
}

fn open(
    buyer: &KeyPair,
    nonce: u64,
    seller: Address,
    arbiter: Option<Address>,
    deadline: u64,
    first: bool,
) -> vinx_core::SignedTransaction {
    user_tx(
        buyer,
        nonce,
        TxBody::OpenDeal {
            seller,
            arbiter,
            amount: 10 * VINX,
            deadline,
            memo: None,
        },
        first,
    )
}

#[test]
fn deal_release_by_buyer_or_arbiter() {
    let mut net = v2();
    let buyer = net.funded(100 * VINX);
    let seller = net.funded(VINX);
    let arbiter = net.funded(VINX);
    net.block(
        T0,
        &[],
        &[open(
            &buyer,
            1,
            addr(&seller),
            Some(addr(&arbiter)),
            T0 + 100,
            true,
        )],
    )
    .unwrap();
    let id = deal_id(&addr(&buyer), 1);
    assert_eq!(net.state.balance(&addr(&buyer)), 90 * VINX - FEE);
    assert_eq!(net.state.registry.locked(), u128::from(10 * VINX));
    net.state.check_invariant().unwrap();

    // Le vendeur ne peut pas se payer lui-même.
    let steal = user_tx(&seller, 1, TxBody::ReleaseDeal { id }, true);
    assert_eq!(tx_err(net.block(T0, &[], &[steal])), TxError::DealForbidden);

    let release = user_tx(&arbiter, 1, TxBody::ReleaseDeal { id }, true);
    net.block(T0, &[], &[release]).unwrap();
    assert_eq!(net.state.balance(&addr(&seller)), 11 * VINX);
    assert!(net.state.registry.deals.is_empty());
    let again = user_tx(&buyer, 2, TxBody::ReleaseDeal { id }, false);
    assert_eq!(tx_err(net.block(T0, &[], &[again])), TxError::UnknownDeal);
}

#[test]
fn deal_refund_rules() {
    let mut net = v2();
    let buyer = net.funded(100 * VINX);
    let seller = net.funded(VINX);
    net.block(
        T0,
        &[],
        &[open(&buyer, 1, addr(&seller), None, T0 + 100, true)],
    )
    .unwrap();
    let id = deal_id(&addr(&buyer), 1);
    let early = user_tx(&buyer, 2, TxBody::RefundDeal { id }, false);
    assert_eq!(
        tx_err(net.block(T0 + 99, &[], std::slice::from_ref(&early))),
        TxError::DealForbidden
    );
    net.block(T0 + 100, &[], &[early]).unwrap();
    assert_eq!(net.state.balance(&addr(&buyer)), 100 * VINX - 2 * FEE);

    // Le vendeur peut rembourser à tout moment.
    net.block(
        T0,
        &[],
        &[open(&buyer, 3, addr(&seller), None, T0 + 100, false)],
    )
    .unwrap();
    let id = deal_id(&addr(&buyer), 3);
    net.block(
        T0,
        &[],
        &[user_tx(&seller, 1, TxBody::RefundDeal { id }, true)],
    )
    .unwrap();
    assert!(net.state.registry.deals.is_empty());
    net.state.check_invariant().unwrap();
}

#[test]
fn deal_parameters_are_checked() {
    let mut net = v2();
    let buyer = net.funded(100 * VINX);
    let seller = addr(&KeyPair::generate());
    for (arbiter, deadline) in [
        (Some(addr(&buyer)), T0 + 100),
        (Some(seller), T0 + 100),
        (None, T0),
        (None, T0 + NAME_PERIOD + 1),
    ] {
        assert_eq!(
            tx_err(net.block(T0, &[], &[open(&buyer, 1, seller, arbiter, deadline, true)])),
            TxError::InvalidDeal
        );
    }
}

#[test]
fn registry_survives_storage_round_trip() {
    let mut net = v2();
    let alice = net.funded(100 * VINX);
    net.block(T0, &[], &[register(&alice, 1, "alice", true)])
        .unwrap();
    let back =
        vinx_state::State::from_parts(&net.state.globals_bytes(), net.state.accounts.clone())
            .unwrap();
    assert_eq!(back, net.state);
}

#[test]
fn state_written_by_0_7_still_loads() {
    let mut net = Net::new();
    net.funded(VINX);
    let s = &net.state;
    // Disposition de l'état global avant le registre.
    let old = vinx_core::encode(&(
        &s.chain_id,
        s.protocol_version,
        s.height,
        &s.escrow,
        &s.admin,
        &s.validators,
        &s.params,
        &s.upgrade,
    ));
    let back = vinx_state::State::from_parts(&old, s.accounts.clone()).unwrap();
    assert_eq!(back.state_root(), s.state_root());
    assert!(back.registry.names.is_empty());
}
