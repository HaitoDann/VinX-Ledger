//! Scale check (ADR 0083): per-block cost with many accounts. Run with
//! `cargo test --release -p vinx-state --test scale_bench -- --ignored --nocapture`.

use std::time::Instant;
use vinx_core::Amount;
use vinx_crypto::Address;
use vinx_state::WorldState;

#[test]
#[ignore]
fn per_block_cost_with_many_accounts() {
    let n: u32 = 200_000;
    let mut s = WorldState::new();
    for i in 0..n {
        let mut b = [0u8; 20];
        b[..4].copy_from_slice(&i.to_be_bytes());
        s.credit_for_test(Address::from_bytes(b), Amount::from_atoms(1_000));
    }
    let t = Instant::now();
    s.compute_state_root();
    println!("initial tree build ({n} accounts): {:?}", t.elapsed());

    let t = Instant::now();
    let rounds = 20;
    for r in 0..rounds {
        let mut copy = s.clone(); // what every proposal / validation does
        for j in 0..1000u32 {
            let mut b = [0u8; 20];
            b[..4].copy_from_slice(&((j * 97 + r) % n).to_be_bytes());
            copy.credit_for_test(Address::from_bytes(b), Amount::from_atoms(1));
        }
        copy.compute_state_root();
        s = copy;
    }
    println!(
        "per block (clone + 1000 account updates + root): {:?}",
        t.elapsed() / rounds
    );

    // Same, but each block also creates 100 new accounts (new payees).
    let t = Instant::now();
    for r in 0..rounds {
        let mut copy = s.clone();
        for j in 0..1000u32 {
            let mut b = [0u8; 20];
            b[..4].copy_from_slice(&((j * 97 + r) % n).to_be_bytes());
            copy.credit_for_test(Address::from_bytes(b), Amount::from_atoms(1));
        }
        for j in 0..100u32 {
            let mut b = [0xffu8; 20];
            b[..4].copy_from_slice(&(r * 1000 + j).to_be_bytes());
            copy.credit_for_test(Address::from_bytes(b), Amount::from_atoms(1));
        }
        copy.compute_state_root();
        s = copy;
    }
    println!(
        "per block with 100 new accounts: {:?}",
        t.elapsed() / rounds
    );
}
