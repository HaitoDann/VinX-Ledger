// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Protocole 7, lot P7a : validateurs nommés, caution, diversité, délai des
//! actions admin (docs/PROTOCOLE-7.md §§1–3, §7).

mod common;

use common::*;
use vinx_core::{AdminAction, ParamId, TxBody, ValidatorIdentity};
use vinx_crypto::{Address, KeyPair};
use vinx_state::staking::{BOND_GRACE, DEFAULT_VALIDATOR_BOND, StakingError, ValidatorStatus};
use vinx_state::{BlockError, State, TxError};

const DAY: u64 = 86_400;
const DELAY: u64 = 2 * DAY;

fn id(name: &str, operator: &str) -> ValidatorIdentity {
    ValidatorIdentity {
        name: name.into(),
        operator: operator.into(),
        country: *b"FR",
    }
}

fn admin_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidAdminTx { error, .. }) => error,
        other => panic!("attendu une action admin refusée, obtenu {other:?}"),
    }
}

fn tx_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidTx { error, .. }) => error,
        other => panic!("attendu une transaction refusée, obtenu {other:?}"),
    }
}

/// Réseau au protocole 7, activé par une vraie mise à jour à chaud. `funded` :
/// comptes à financer avant l'activation (les paiements de la trésorerie sont
/// ensuite soumis au délai).
fn net7(funded: &[&KeyPair]) -> Net {
    let mut net = Net::current();
    for kp in funded {
        let pay = net.admin(AdminAction::TreasuryTransfer {
            to: addr(kp),
            amount: 50_000 * VINX,
            memo: None,
        });
        net.block(T0, &[pay], &[]).unwrap();
    }
    let h = net.state.height;
    let up = net.admin(AdminAction::ScheduleUpgrade {
        version: 7,
        activation_height: h + 1 + 17_280,
    });
    net.block(T0, &[up], &[]).unwrap();
    net.state.height = h + 17_280;
    net.block(T0, &[], &[]).unwrap();
    assert_eq!(net.state.protocol_version, 7);
    net
}

fn first(kp: &KeyPair, state: &State) -> bool {
    state.account(&addr(kp)).is_none_or(|a| a.key.is_none())
}

fn send(net: &mut Net, kp: &KeyPair, body: TxBody, at: u64) -> Result<(), BlockError> {
    let nonce = net.state.account(&addr(kp)).map_or(0, |a| a.nonce) + 1;
    let tx = user_tx(kp, nonce, body, first(kp, &net.state));
    net.block(at, &[], &[tx])
}

#[test]
fn activation_gives_the_validators_provisional_records() {
    let net = net7(&[]);
    let s = &net.state;
    assert_eq!(s.staking.records.len(), 4);
    for (i, v) in net.validators.iter().enumerate() {
        let r = s.staking.records.get(&v.public_key()).unwrap();
        assert_eq!(r.identity.name, format!("validateur-{}", i + 1));
        assert_eq!(r.identity.operator, "VinX Labs");
        assert_eq!(r.status, ValidatorStatus::Active);
        assert_eq!(r.grace_until, T0 + BOND_GRACE);
    }
    assert!(s.check_invariant().is_ok());
}

#[test]
fn admin_actions_wait_48_hours_and_can_be_cancelled() {
    let mut net = net7(&[]);
    let raise = net.admin(AdminAction::SetParam {
        param: ParamId::MinFee,
        value: 2 * VINX / 1000,
    });
    let raise_id = raise.id();
    net.block(T0, &[raise], &[]).unwrap();
    assert_eq!(net.state.params.min_fee, VINX / 1000, "pas encore appliqué");
    assert!(net.state.staking.pending.contains_key(&raise_id));
    net.block(T0 + DELAY - 1, &[], &[]).unwrap();
    assert_eq!(net.state.params.min_fee, VINX / 1000);
    net.block(T0 + DELAY, &[], &[]).unwrap();
    assert_eq!(
        net.state.params.min_fee,
        2 * VINX / 1000,
        "appliqué après 48 h"
    );
    assert!(net.state.staking.pending.is_empty());

    // Une action annulée pendant le délai ne s'applique jamais.
    let lower = net.admin(AdminAction::SetParam {
        param: ParamId::MinFee,
        value: VINX / 1000,
    });
    let lower_id = lower.id();
    net.block(T0 + DELAY, &[lower], &[]).unwrap();
    let cancel = net.admin(AdminAction::CancelPending { id: lower_id });
    net.block(T0 + DELAY + 10, &[cancel], &[]).unwrap();
    net.block(T0 + 3 * DELAY, &[], &[]).unwrap();
    assert_eq!(net.state.params.min_fee, 2 * VINX / 1000, "annulée");

    // Une action invalide est refusée tout de suite, pas après le délai.
    let bad = net.admin(AdminAction::SetParam {
        param: ParamId::MinAvailability,
        value: 10_001,
    });
    assert_eq!(
        admin_err(net.block(T0 + 3 * DELAY, &[bad], &[])),
        TxError::InvalidParam
    );
}

#[test]
fn a_named_validator_enters_with_its_bond() {
    let cand = KeyPair::generate();
    let mut net = net7(&[&cand]);
    let key = cand.public_key();
    let t = T0 + 10;

    // L'ancienne action est remplacée.
    let old = net.admin(AdminAction::AddValidator { key: key.clone() });
    assert_eq!(
        admin_err(net.block(t, &[old], &[])),
        TxError::Staking(StakingError::Obsolete)
    );
    net.admin_nonce -= 1;

    // Sans caution : refusé.
    let add = |net: &mut Net, name: &str| {
        net.admin(AdminAction::AddNamedValidator {
            key: key.clone(),
            identity: id(name, "Saffron SAS"),
        })
    };
    let a = add(&mut net, "Saffron");
    assert_eq!(
        admin_err(net.block(t, &[a], &[])),
        TxError::Staking(StakingError::BondTooLow)
    );
    net.admin_nonce -= 1;

    // Caution déposée depuis une autre adresse que celle de la clé : refusée.
    let other = KeyPair::generate();
    let mut n2 = net7(&[&other]);
    assert_eq!(
        tx_err(send(
            &mut n2,
            &other,
            TxBody::DepositBond {
                key: key.clone(),
                amount: DEFAULT_VALIDATOR_BOND
            },
            t
        )),
        TxError::Staking(StakingError::NotValidatorAddress)
    );

    send(
        &mut net,
        &cand,
        TxBody::DepositBond {
            key: key.clone(),
            amount: DEFAULT_VALIDATOR_BOND,
        },
        t,
    )
    .unwrap();
    assert_eq!(net.state.staking.bond(&key), DEFAULT_VALIDATOR_BOND);
    assert!(
        net.state.check_invariant().is_ok(),
        "caution dans l'invariant"
    );

    // Nom déjà pris (insensible à la casse) : refusé.
    let taken = add(&mut net, "VALIDATEUR-1");
    assert_eq!(
        admin_err(net.block(t, &[taken], &[])),
        TxError::Staking(StakingError::NameTaken)
    );
    net.admin_nonce -= 1;

    let a = add(&mut net, "Saffron");
    net.block(t, &[a], &[]).unwrap();
    assert!(
        !net.state.staking.records.contains_key(&key),
        "en attente du délai"
    );
    net.block(t + DELAY, &[], &[]).unwrap();
    let r = net.state.staking.records.get(&key).unwrap();
    assert_eq!(r.identity.name, "Saffron");
    net.block(t + DELAY, &[], &[]).unwrap();
    net.block(t + DELAY, &[], &[]).unwrap();
    assert!(
        net.state.validators.active.contains(&key),
        "actif 2 blocs après"
    );

    // Le validateur tient lui-même son site et son contact à jour.
    send(
        &mut net,
        &cand,
        TxBody::UpdateValidatorInfo {
            website: "https://saffron.example".into(),
            contact: "ops@saffron.example".into(),
        },
        t + DELAY,
    )
    .unwrap();
    let r = net.state.staking.records.get(&key).unwrap();
    assert_eq!(r.website, "https://saffron.example");

    // Retiré : il sort, et son nom ne sera jamais réattribué.
    let rm = net.admin(AdminAction::RemoveValidator { key: key.clone() });
    net.block(t + DELAY, &[rm], &[]).unwrap();
    net.block(t + 2 * DELAY, &[], &[]).unwrap();
    assert!(matches!(
        net.state.staking.records.get(&key).unwrap().status,
        ValidatorStatus::Leaving { .. }
    ));
    let newcomer = KeyPair::generate();
    let reuse = net.admin(AdminAction::AddNamedValidator {
        key: newcomer.public_key(),
        identity: id("saffron", "Autre"),
    });
    assert_eq!(
        admin_err(net.block(t + 2 * DELAY, &[reuse], &[])),
        TxError::Staking(StakingError::NameTaken)
    );
}

#[test]
fn diversity_rule_caps_one_operator() {
    let cand = KeyPair::generate();
    let mut net = net7(&[&cand]);
    let key = cand.public_key();
    send(
        &mut net,
        &cand,
        TxBody::DepositBond {
            key: key.clone(),
            amount: DEFAULT_VALIDATOR_BOND,
        },
        T0,
    )
    .unwrap();
    let on = net.admin(AdminAction::SetParam {
        param: ParamId::Diversity,
        value: 1,
    });
    net.block(T0, &[on], &[]).unwrap();
    net.block(T0 + DELAY, &[], &[]).unwrap();
    assert_eq!(net.state.staking.params.diversity, 1);
    // Les 4 validateurs en place sont « VinX Labs » : un 5e du même opérateur
    // est refusé (au plus ⌊(5 − 1)/3⌋ = 1 siège).
    let same = net.admin(AdminAction::AddNamedValidator {
        key: key.clone(),
        identity: id("Saffron", "  vinx   LABS "),
    });
    assert_eq!(
        admin_err(net.block(T0 + DELAY, &[same], &[])),
        TxError::Staking(StakingError::Diversity)
    );
}

#[test]
fn staking_rows_reload_to_the_same_state_root() {
    let cand = KeyPair::generate();
    let mut net = net7(&[&cand]);
    let key = cand.public_key();
    send(
        &mut net,
        &cand,
        TxBody::DepositBond {
            key: key.clone(),
            amount: DEFAULT_VALIDATOR_BOND,
        },
        T0,
    )
    .unwrap();
    let pending = net.admin(AdminAction::SetParam {
        param: ParamId::MinFee,
        value: 7,
    });
    net.block(T0, &[pending], &[]).unwrap();
    let root = net.state.state_root();
    let mut state = net.state.clone();
    state.mark_all_rows_dirty();
    let rows = state.take_dirty_rows();
    let mut reloaded =
        State::from_parts(&state.globals_bytes_light(), state.accounts.clone()).unwrap();
    for (k, v) in rows {
        if let Some(v) = v {
            reloaded.load_row(&k, &v).unwrap();
        }
    }
    assert_eq!(reloaded.state_root(), root);
    assert_eq!(reloaded.staking.bond(&key), DEFAULT_VALIDATOR_BOND);
    assert_eq!(reloaded.staking.pending.len(), 1);
    assert!(reloaded.check_invariant().is_ok());
    let _ = Address::treasury();
}
