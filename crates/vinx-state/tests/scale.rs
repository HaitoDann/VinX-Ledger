//! Banc d'essai à grande échelle (ignoré par défaut) :
//! `cargo test --release -p vinx-state --test scale -- --ignored --nocapture`

// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use std::time::Instant;
use vinx_core::{Account, Transaction, TxBody, UNITS_PER_VINX};
use vinx_crypto::{Address, KeyPair};
use vinx_state::Genesis;

const ACCOUNTS: u32 = 1_000_000;
const TXS: u64 = 1_000;

#[test]
#[ignore]
fn one_million_accounts() {
    let k = || KeyPair::generate().public_key();
    let mut state = Genesis::legacy("vinx-scale", 1_800_000_000, [k(), k(), k()], vec![k()])
        .unwrap()
        .into_state()
        .unwrap();
    // Comptes synthétiques financés par la trésorerie, pour garder l'invariant.
    let treasury = Address::treasury();
    let per = UNITS_PER_VINX;
    let senders: Vec<KeyPair> = (0..TXS).map(|_| KeyPair::generate()).collect();
    let t = Instant::now();
    for i in 0..ACCOUNTS {
        let mut a = [0u8; 20];
        a[..4].copy_from_slice(&i.to_be_bytes());
        a[4..8].copy_from_slice(&i.wrapping_mul(2_654_435_761).to_be_bytes());
        state.set_account_for_tests(
            Address::from_bytes(a),
            Account {
                balance: per,
                ..Account::default()
            },
        );
    }
    for s in &senders {
        state.set_account_for_tests(
            Address::from_public_key(&s.public_key()),
            Account {
                balance: 10 * per,
                ..Account::default()
            },
        );
    }
    let moved = per * u64::from(ACCOUNTS) + 10 * per * TXS;
    let mut tr = state.account(&treasury).unwrap().clone();
    tr.balance -= moved;
    state.set_account_for_tests(treasury, tr);
    state.check_invariant().unwrap();
    println!("création de {ACCOUNTS} comptes : {:?}", t.elapsed());

    let t = Instant::now();
    let root = state.state_root();
    println!("racine d'état (à froid) : {:?}", t.elapsed());

    let txs: Vec<_> = senders
        .iter()
        .map(|s| {
            Transaction {
                chain_id: "vinx-scale".into(),
                sender: Address::from_public_key(&s.public_key()),
                nonce: 1,
                fee: UNITS_PER_VINX / 1000,
                body: TxBody::Transfer {
                    to: treasury,
                    amount: per,
                    memo: None,
                },
            }
            .sign(s, true)
        })
        .collect();
    let t = Instant::now();
    let next = state.execute(1, 1_800_000_000_000, &[], &txs).unwrap();
    println!(
        "exécution d'un bloc de {TXS} transactions : {:?}",
        t.elapsed()
    );
    let t = Instant::now();
    let root2 = next.state_root();
    println!("racine d'état après le bloc : {:?}", t.elapsed());
    assert_ne!(root, root2);
    let full = borsh::to_vec(&next).unwrap().len();
    // Les comptes créés pour le banc sont marqués modifiés : on les oublie pour
    // mesurer ce qu'un bloc seul fait écrire sur disque.
    state.accounts.take_dirty();
    let mut after = state.execute(1, 1_800_000_000_000, &[], &txs).unwrap();
    let written: usize = after
        .accounts
        .take_dirty()
        .iter()
        .map(|(_, a)| 20 + borsh::to_vec(a).unwrap().len())
        .sum();
    println!(
        "écriture disque par bloc : {} Ko, au lieu de l'état complet : {} Mo",
        written / 1_000,
        full / 1_000_000
    );
}
