//! Une transaction décodée de n'importe quels octets est validée puis
//! appliquée, ou refusée proprement, sans planter ni violer l'invariant de
//! supply, sur un état au dernier protocole. Les mêmes octets servent aussi de
//! preuve d'équivoque (protocole 7), envoyée par un compte financé contre le
//! validateur cautionné de la genèse.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;
use vinx_core::{Account, SignedAdminTransaction, SignedTransaction, Transaction, TxBody};
use vinx_crypto::{Address, KeyPair};
use vinx_state::{Genesis, State};

/// Compte financé qui signe les signalements.
fn reporter() -> KeyPair {
    KeyPair::from_secret_bytes(&[5; 32])
}

fn base() -> &'static State {
    static STATE: OnceLock<State> = OnceLock::new();
    STATE.get_or_init(|| {
        let k = |i: u8| KeyPair::from_secret_bytes(&[i; 32]).public_key();
        let mut genesis =
            Genesis::standard("vinx-fuzz", 1_800_000_000, [k(1), k(2), k(3)], vec![k(4)])
                .expect("genèse de fuzzing")
                .with_faucet(1_000_000_000_000_000)
                .expect("robinet");
        genesis.validator_bond = vinx_state::staking::DEFAULT_VALIDATOR_BOND;
        let mut state = genesis.into_state().expect("état de genèse");
        // Le signaleur reçoit une part du robinet (l'invariant reste juste).
        let treasury = Address::treasury();
        let mut t = state.account(&treasury).cloned().expect("trésorerie");
        let amount = 1_000_000_000_000;
        t.balance -= amount;
        state.set_account_for_tests(treasury, t);
        let who = Address::from_public_key(&reporter().public_key());
        state.set_account_for_tests(
            who,
            Account {
                balance: amount,
                ..Account::default()
            },
        );
        state
    })
}

/// Même état au modèle réserve (protocole 8) : demandes et votes.
fn reserve_base() -> &'static State {
    static STATE: OnceLock<State> = OnceLock::new();
    STATE.get_or_init(|| {
        let k = |i: u8| KeyPair::from_secret_bytes(&[i; 32]).public_key();
        let mut state =
            Genesis::standard("vinx-fuzz", 1_800_000_000, [k(1), k(2), k(3)], vec![k(4)])
                .expect("genèse de fuzzing")
                .with_reserve()
                .expect("modèle réserve")
                .into_state()
                .expect("état de genèse");
        let amount = 1_000_000_000_000_000;
        state.governance.reserve -= amount;
        let who = Address::from_public_key(&reporter().public_key());
        state.set_account_for_tests(
            who,
            Account {
                balance: amount,
                ..Account::default()
            },
        );
        state
    })
}

fuzz_target!(|bytes: &[u8]| {
    if let Ok(tx) = borsh::from_slice::<SignedTransaction>(bytes) {
        let mut state = base().clone();
        let _ = state.apply_tx(&tx);
        assert!(state.check_invariant().is_ok());
        let _ = base().execute(1, 1_800_000_000_000, &[], &[tx.clone()]);
        let mut reserve = reserve_base().clone();
        let _ = reserve.apply_tx(&tx);
        assert!(reserve.check_invariant().is_ok());
        let _ = reserve_base().execute(1, 1_800_000_000_000, &[], &[tx]);
    }
    if let Ok(tx) = borsh::from_slice::<SignedAdminTransaction>(bytes) {
        let _ = base().execute(1, 1_800_000_000_000, &[tx.clone()], &[]);
        let _ = reserve_base().execute(1, 1_800_000_000_000, &[tx], &[]);
    }
    let kp = reporter();
    let report = Transaction {
        chain_id: "vinx-fuzz".into(),
        sender: Address::from_public_key(&kp.public_key()),
        nonce: 1,
        fee: 1_000_000,
        body: TxBody::ReportEquivocation {
            key: KeyPair::from_secret_bytes(&[4; 32]).public_key(),
            evidence: bytes.to_vec(),
        },
    }
    .sign(&kp, true);
    let mut state = base().clone();
    let _ = state.apply_tx(&report);
    assert!(state.check_invariant().is_ok());
});
