//! Chronomètre les étapes de l'application d'un bloc plein.
//! `cargo run --release -p vinx-node --example profil_bloc [transactions]`
#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use std::time::Instant;
use vinx_core::{Account, Transaction, TxBody};
use vinx_crypto::{Address, KeyPair};

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(5000);
    let k = || KeyPair::generate().public_key();
    let mut state = vinx_state::Genesis::legacy("vinx-profil", 1, [k(), k(), k()], vec![k()])
        .unwrap()
        .into_state()
        .unwrap();
    state.params.max_block_txs = n as u64;
    let keys: Vec<KeyPair> = (0..n).map(|_| KeyPair::generate()).collect();
    let fund = 10_000_000_000u64;
    let mut treasury = state.account(&Address::treasury()).unwrap().clone();
    for kp in &keys {
        let a = Address::from_public_key(&kp.public_key());
        state.set_account_for_tests(
            a,
            Account {
                balance: fund,
                nonce: 0,
                key: Some(kp.public_key()),
                memo_required: false,
            },
        );
        treasury.balance -= fund;
    }
    state.set_account_for_tests(Address::treasury(), treasury);
    state.state_root();

    let txs: Vec<_> = keys
        .iter()
        .enumerate()
        .map(|(i, kp)| {
            let to = Address::from_public_key(&keys[(i + 1) % n].public_key());
            Transaction {
                chain_id: state.chain_id.clone(),
                sender: Address::from_public_key(&kp.public_key()),
                nonce: 1,
                fee: state.params.min_fee,
                body: TxBody::Transfer {
                    to,
                    amount: 1,
                    memo: None,
                },
            }
            .sign(kp, false)
        })
        .collect();

    let t = Instant::now();
    for stx in &txs {
        stx.verify_signature(stx.public_key.as_ref().unwrap_or(&keys[0].public_key()))
            .ok();
    }
    let verify = t.elapsed();
    let t = Instant::now();
    for (stx, kp) in txs.iter().zip(&keys) {
        stx.verify_signature(&kp.public_key()).unwrap();
    }
    let verify = verify.max(t.elapsed());

    let t = Instant::now();
    let next = state.execute(state.height + 1, 2_000, &[], &txs).unwrap();
    let execute = t.elapsed();
    let t = Instant::now();
    let _ = state.execute(state.height + 1, 2_000, &[], &txs).unwrap();
    let execute_warm = t.elapsed();

    let t = Instant::now();
    let root = next.state_root();
    let root_time = t.elapsed();

    let t = Instant::now();
    let _ = vinx_state::state::tx_root(&[], &txs);
    let txroot = t.elapsed();

    let t = Instant::now();
    let _ = state.clone();
    let clone = t.elapsed();

    let dir = std::env::temp_dir().join(format!("profil-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = vinx_node::store::Store::open(&dir.join("db"), [7; 32]).unwrap();
    let mut next2 = next.clone();
    let block = vinx_core::Block {
        header: vinx_core::BlockHeader {
            protocol_version: 1,
            chain_id: state.chain_id.clone(),
            height: 1,
            timestamp: 2_000,
            prev_hash: [0; 32],
            tx_root: [0; 32],
            state_root: root,
            validators_hash: [0; 32],
            proposer: 0,
        },
        admin_txs: vec![],
        txs: txs.clone(),
    };
    let cert = vinx_core::CommitCertificate {
        height: 1,
        round: 0,
        block_hash: block.hash(),
        signatures: vec![],
    };
    let t = Instant::now();
    store.commit_block(&block, &cert, &mut next2).unwrap();
    let commit = t.elapsed();
    let _ = std::fs::remove_dir_all(&dir);

    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    println!("{n} transactions");
    println!("  signatures seules      {:8.1} ms", ms(verify));
    println!("  execute, 1re fois      {:8.1} ms", ms(execute));
    println!(
        "  execute, 2e fois       {:8.1} ms  (signatures en cache)",
        ms(execute_warm)
    );
    println!("  state_root             {:8.1} ms", ms(root_time));
    println!("  tx_root                {:8.1} ms", ms(txroot));
    println!("  clone de l'état        {:8.1} ms", ms(clone));
    println!("  écriture disque        {:8.1} ms", ms(commit));
}
