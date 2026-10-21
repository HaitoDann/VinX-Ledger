//! Tests de propriétés : quelle que soit la séquence d'opérations, la supply
//! est conservée et une opération refusée ne modifie pas l'état.

// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

mod common;

use common::*;
use proptest::prelude::*;
use vinx_core::{AdminAction, SignedAdminTransaction, SignedTransaction};
use vinx_crypto::{Address, KeyPair};
use vinx_state::BlockError;
use vinx_state::escrow::GENESIS_TRANCHE_AMOUNT;

#[derive(Clone, Debug)]
enum Op {
    Transfer {
        from: usize,
        to: usize,
        amount: u64,
        nonce_skew: u8,
    },
    Fund {
        to: usize,
        amount: u64,
    },
    Release {
        amount: u64,
        months: u8,
    },
    Return {
        amount: u64,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..5usize, 0..6usize, 0..20 * VINX, 0..4u8)
            .prop_map(|(from, to, amount, nonce_skew)| Op::Transfer { from, to, amount, nonce_skew }),
        2 => (0..5usize, 0..50 * VINX).prop_map(|(to, amount)| Op::Fund { to, amount }),
        1 => (0..=GENESIS_TRANCHE_AMOUNT + 1, 0..3u8)
            .prop_map(|(amount, months)| Op::Release { amount, months }),
        1 => (0..100 * VINX).prop_map(|amount| Op::Return { amount }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn supply_is_conserved_and_failures_leave_no_trace(ops in prop::collection::vec(op(), 1..60)) {
        let mut net = Net::new();
        let users: Vec<KeyPair> = (0..5).map(|_| KeyPair::generate()).collect();
        let mut nonces = [0u64; 5];
        let mut now = T0;

        for op in ops {
            let (admin, user): (Vec<SignedAdminTransaction>, Vec<SignedTransaction>) = match op {
                Op::Transfer { from, to, amount, nonce_skew } => {
                    let dest = users.get(to).map_or(Address::treasury(), addr);
                    let nonce = nonces[from] + 1 + u64::from(nonce_skew == 3);
                    let first = net.state.accounts.get(&addr(&users[from])).is_none_or(|a| a.key.is_none());
                    (vec![], vec![transfer(&users[from], nonce, dest, amount, first)])
                }
                Op::Fund { to, amount } => (
                    vec![net.admin(AdminAction::TreasuryTransfer { to: addr(&users[to]), amount, memo: None })],
                    vec![],
                ),
                Op::Release { amount, months } => {
                    now += u64::from(months) * PERIOD;
                    (vec![net.admin(AdminAction::EscrowRelease { amount })], vec![])
                }
                Op::Return { amount } => (vec![net.admin(AdminAction::EscrowReturn { amount })], vec![]),
            };
            let before = net.state.clone();
            match net.block(now, &admin, &user) {
                Ok(()) => {
                    if let Op::Transfer { from, .. } = op { nonces[from] += 1; }
                }
                Err(e) => {
                    // Un refus doit venir d'une règle, jamais de l'invariant : sinon une
                    // transaction a bien été appliquée de travers.
                    prop_assert_ne!(e, BlockError::InvariantViolated);
                    prop_assert_eq!(&net.state, &before);
                    if !admin.is_empty() { net.admin_nonce -= 1; }
                }
            }
            prop_assert!(net.state.check_invariant().is_ok());
        }
    }
}
