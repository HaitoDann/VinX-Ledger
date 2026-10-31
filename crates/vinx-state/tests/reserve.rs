// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Réserve par objet (protocole 6).

mod common;

use common::*;
use vinx_core::{AdminAction, AssetPowers, ParamId, PolicyConfig, SignedTransaction, TxBody};
use vinx_crypto::{Address, KeyPair};
use vinx_state::{BlockError, State, TxError};

/// Réserve minimale d'un compte (0,1 VINX) et réserve par objet (0,1 VINX).
const MIN: u64 = VINX / 10;
const PER: u64 = VINX / 10;
const FEE: u64 = VINX / 1000;

fn tx_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidTx { error, .. }) => error,
        other => panic!("attendu une transaction invalide, obtenu {other:?}"),
    }
}

fn accept(who: &KeyPair, nonce: u64, symbol: &str, yes: bool, first: bool) -> SignedTransaction {
    user_tx(
        who,
        nonce,
        TxBody::AcceptAsset {
            symbol: symbol.into(),
            accept: yes,
        },
        first,
    )
}

/// Un émetteur qui a créé `symbols` (supply 1 000, pouvoir d'émettre).
fn issuer_of(net: &mut Net, symbols: &[&str]) -> KeyPair {
    let issuer = net.funded(10_000 * VINX);
    let mut nonce = 0;
    for s in symbols {
        nonce += 1;
        let reg = TxBody::RegisterName {
            name: s.to_lowercase(),
        };
        net.block(T0, &[], &[user_tx(&issuer, nonce, reg, nonce == 1)])
            .unwrap();
        nonce += 1;
        let create = TxBody::CreateAsset {
            symbol: (*s).into(),
            name: format!("Jeton {s}"),
            decimals: 0,
            initial_supply: 1_000,
            max_supply: None,
            powers: AssetPowers {
                mint: true,
                ..AssetPowers::default()
            },
        };
        net.block(T0, &[], &[user_tx(&issuer, nonce, create, false)])
            .unwrap();
    }
    issuer
}

fn count(s: &State, who: &KeyPair) -> u32 {
    s.reserves.count(&addr(who))
}

#[test]
fn accepting_assets_locks_a_reserve_per_object() {
    let mut net = Net::current();
    issuer_of(&mut net, &["AAA", "BBB", "CCC"]);
    // De quoi couvrir la réserve du compte, deux objets et 4 transactions.
    let alice = net.funded(MIN + 2 * PER + 4 * FEE);
    net.block(T0, &[], &[accept(&alice, 1, "AAA", true, true)])
        .unwrap();
    net.block(T0, &[], &[accept(&alice, 2, "BBB", true, false)])
        .unwrap();
    assert_eq!(count(&net.state, &alice), 2);
    // Un troisième objet passerait sous la réserve : refusé.
    assert_eq!(
        tx_err(net.block(T0, &[], &[accept(&alice, 3, "CCC", true, false)])),
        TxError::ObjectReserve
    );
    // Retirer un jeton libère sa réserve ; le troisième passe alors.
    net.block(T0, &[], &[accept(&alice, 3, "AAA", false, false)])
        .unwrap();
    assert_eq!(count(&net.state, &alice), 1);
    net.block(T0, &[], &[accept(&alice, 4, "CCC", true, false)])
        .unwrap();
    assert_eq!(count(&net.state, &alice), 2);
}

#[test]
fn reserved_funds_cannot_be_spent_but_fees_can() {
    let mut net = Net::current();
    issuer_of(&mut net, &["AAA"]);
    let alice = net.funded(VINX);
    let bob = net.funded(VINX);
    net.block(T0, &[], &[accept(&alice, 1, "AAA", true, true)])
        .unwrap();
    // Alice a 1 VINX moins les frais ; elle doit garder 0,2 VINX.
    let balance = net.state.balance(&addr(&alice));
    let too_much = transfer(&alice, 2, addr(&bob), balance - MIN - PER + 1 - FEE, false);
    assert_eq!(
        tx_err(net.block(T0, &[], &[too_much])),
        TxError::ObjectReserve
    );
    let ok = transfer(&alice, 2, addr(&bob), balance - MIN - PER - FEE, false);
    net.block(T0, &[], &[ok]).unwrap();
    assert_eq!(net.state.balance(&addr(&alice)), MIN + PER);
    // On ne peut pas vider un compte qui possède des objets.
    let empty = transfer(&alice, 3, addr(&bob), MIN + PER - FEE, false);
    assert_eq!(tx_err(net.block(T0, &[], &[empty])), TxError::ObjectReserve);
    // Payer des frais reste possible, même sous la réserve, pour libérer
    // des objets : la gouvernance relève la réserve, Alice retire son jeton.
    let raise = net.admin(AdminAction::SetParam {
        param: ParamId::ObjectReserve,
        value: VINX,
    });
    net.block(T0, &[raise], &[]).unwrap();
    net.block(T0, &[], &[accept(&alice, 3, "AAA", false, false)])
        .unwrap();
    assert_eq!(count(&net.state, &alice), 0);
}

#[test]
fn mandates_and_policies_are_objects_too() {
    let mut net = Net::current();
    let alice = net.funded(10 * VINX);
    let shop = net.funded(VINX);
    let grant = user_tx(
        &alice,
        1,
        TxBody::GrantMandate {
            payee: addr(&shop),
            max_per_period: 10 * VINX,
            period: 30 * 86_400,
            expires_at: None,
        },
        true,
    );
    let policy = user_tx(
        &alice,
        2,
        TxBody::SetPolicy {
            policy: PolicyConfig {
                signers: vec![KeyPair::generate().public_key()],
                threshold: 1,
                daily_limit: Some(100 * VINX),
            },
        },
        false,
    );
    net.block(T0, &[], &[grant, policy]).unwrap();
    assert_eq!(count(&net.state, &alice), 2);
    // Renouveler un mandat ne crée pas d'objet de plus.
    let again = user_tx(
        &alice,
        3,
        TxBody::GrantMandate {
            payee: addr(&shop),
            max_per_period: 5 * VINX,
            period: 30 * 86_400,
            expires_at: None,
        },
        false,
    );
    let r = net.block(T0, &[], &[again]);
    // Avec des règles, accorder un mandat exige les approbations : c'est
    // l'erreur attendue ; le compte d'objets est inchangé.
    assert!(r.is_err());
    assert_eq!(count(&net.state, &alice), 2);
    // Le bénéficiaire ne peut pas prélever jusqu'à entamer la réserve.
    let collect = |amount| {
        user_tx(
            &shop,
            1,
            TxBody::Collect {
                payer: addr(&alice),
                amount,
                memo: None,
            },
            true,
        )
    };
    let left = net.state.balance(&addr(&alice));
    assert_eq!(
        tx_err(net.block(T0, &[], &[collect(left - MIN - 2 * PER + 1)])),
        TxError::ObjectReserve
    );
    net.block(T0, &[], &[collect(left - MIN - 2 * PER)])
        .unwrap();
}

#[test]
fn upgrade_to_protocol_6_counts_existing_objects() {
    let mut net = Net::current();
    net.state.protocol_version = 5;
    issuer_of(&mut net, &["AAA", "BBB"]);
    let alice = net.funded(VINX);
    let shop = net.funded(VINX);
    let grant = user_tx(
        &alice,
        3,
        TxBody::GrantMandate {
            payee: addr(&shop),
            max_per_period: VINX,
            period: 30 * 86_400,
            expires_at: None,
        },
        false,
    );
    net.block(
        T0,
        &[],
        &[
            accept(&alice, 1, "AAA", true, true),
            accept(&alice, 2, "BBB", true, false),
            grant,
        ],
    )
    .unwrap();
    // Protocole 5 : pas encore de compte d'objets, ni de paramètre.
    assert_eq!(count(&net.state, &alice), 0);
    let early = net.admin(AdminAction::SetParam {
        param: ParamId::ObjectReserve,
        value: 1,
    });
    assert!(net.block(T0, &[early], &[]).is_err());
    net.admin_nonce -= 1;
    let h = net.state.height;
    let up = net.admin(AdminAction::ScheduleUpgrade {
        version: 6,
        activation_height: h + 1 + 17_280,
    });
    net.block(T0, &[up], &[]).unwrap();
    net.state.height = h + 17_280;
    net.block(T0, &[], &[]).unwrap();
    assert_eq!(net.state.protocol_version, 6);
    // Deux jetons et un mandat ; chaque émetteur garde son propre jeton.
    assert_eq!(count(&net.state, &alice), 3);
    assert_eq!(net.state.reserves.owned.len(), 2);
}

#[test]
fn reserve_rows_reload_to_the_same_state_root() {
    let mut net = Net::current();
    issuer_of(&mut net, &["AAA"]);
    let alice = net.funded(VINX);
    net.block(T0, &[], &[accept(&alice, 1, "AAA", true, true)])
        .unwrap();
    let root = net.state.state_root();
    let mut state = net.state.clone();
    let rows = state.take_dirty_rows();
    let mut reloaded =
        State::from_parts(&state.globals_bytes_light(), state.accounts.clone()).unwrap();
    for (key, value) in rows {
        if let Some(v) = value {
            reloaded.load_row(&key, &v).unwrap();
        }
    }
    assert_eq!(reloaded.state_root(), root);
    assert_eq!(reloaded.reserves.count(&addr(&alice)), 1);
    let _ = Address::treasury();
}
