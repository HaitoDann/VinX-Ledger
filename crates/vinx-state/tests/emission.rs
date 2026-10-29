// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Émission vers les validateurs et frais aux validateurs (protocole 3).

mod common;

use common::*;
use vinx_crypto::{Address, KeyPair};
use vinx_state::BlockError;
use vinx_state::emission::GENESIS_EMISSION;

const FEE: u64 = VINX / 1000;

fn validator_balance(net: &Net, i: usize) -> u64 {
    net.state
        .balance(&Address::from_public_key(&net.validators[i].public_key()))
}

#[test]
fn fees_go_to_the_epoch_pot_not_the_treasury() {
    let mut net = Net::current();
    let alice = net.funded(10 * VINX);
    let treasury = net.state.balance(&Address::treasury());
    let pot = net.state.emission.pot;
    let tx = transfer(&alice, 1, addr(&KeyPair::generate()), VINX, true);
    net.block(T0, &[], &[tx]).unwrap();
    assert_eq!(net.state.balance(&Address::treasury()), treasury);
    assert_eq!(net.state.emission.pot, pot + FEE);
    net.state.check_invariant().unwrap();
}

#[test]
fn epoch_rewards_follow_proposed_blocks() {
    let mut net = Net::current();
    net.state.emission.epoch_blocks = 4;
    let alice = net.funded(10 * VINX); // hauteur 1, sans proposeur
    let tx = transfer(&alice, 1, addr(&KeyPair::generate()), VINX, true);
    // Hauteurs 2, 3, 4 : le validateur 0 propose deux blocs, le 1 un seul.
    let day = 86_400;
    net.block_by(T0 + day / 3, 0, &[], &[tx]).unwrap();
    net.block_by(T0 + 2 * day / 3, 1, &[], &[]).unwrap();
    assert_eq!(
        validator_balance(&net, 0),
        0,
        "rien avant la fin de l'époque"
    );
    net.block_by(T0 + day, 0, &[], &[]).unwrap();
    assert_eq!(net.state.height, 4);

    let emitted = net.state.emission.emitted;
    assert_eq!(emitted, net.state.emission.target(T0 + day));
    // ~18 977 VINX émis le premier jour, plus les frais.
    assert!(
        (18_970 * VINX..18_980 * VINX).contains(&emitted),
        "{emitted}"
    );
    let (v0, v1) = (validator_balance(&net, 0), validator_balance(&net, 1));
    assert_eq!(v0 + v1 + net.state.emission.pot, emitted + FEE);
    assert!(v0.abs_diff(2 * v1) <= 2, "{v0} / {v1}");
    assert_eq!(
        validator_balance(&net, 2),
        0,
        "un validateur absent ne gagne rien"
    );
    assert!(net.state.emission.proposed.is_empty());
    assert_eq!(net.state.emission.remaining(), GENESIS_EMISSION - emitted);
    net.state.check_invariant().unwrap();
}

#[test]
fn proposer_must_belong_to_the_set() {
    let mut net = Net::current();
    assert_eq!(
        net.block_by(T0, 9, &[], &[]),
        Err(BlockError::UnknownProposer)
    );
}

#[test]
fn legacy_chains_keep_fees_in_the_treasury() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let treasury = net.state.balance(&Address::treasury());
    let tx = transfer(&alice, 1, addr(&KeyPair::generate()), VINX, true);
    net.block_by(T0, 0, &[], &[tx]).unwrap();
    assert_eq!(net.state.balance(&Address::treasury()), treasury + FEE);
    assert_eq!(net.state.emission.pot, 0);
}

#[test]
fn emission_survives_storage_round_trip() {
    let mut net = Net::current();
    net.block_by(T0, 1, &[], &[]).unwrap();
    let back =
        vinx_state::State::from_parts(&net.state.globals_bytes(), net.state.accounts.clone())
            .unwrap();
    assert_eq!(back, net.state);
    assert_eq!(back.state_root(), net.state.state_root());
}
