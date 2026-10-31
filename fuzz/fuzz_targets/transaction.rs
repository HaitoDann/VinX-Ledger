//! Une transaction décodée de n'importe quels octets est validée puis
//! appliquée, ou refusée proprement, sans planter ni violer l'invariant de
//! supply, sur un état au dernier protocole.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;
use vinx_core::{SignedAdminTransaction, SignedTransaction};
use vinx_crypto::KeyPair;
use vinx_state::{Genesis, State};

fn base() -> &'static State {
    static STATE: OnceLock<State> = OnceLock::new();
    STATE.get_or_init(|| {
        let k = |i: u8| KeyPair::from_secret_bytes(&[i; 32]).public_key();
        Genesis::standard("vinx-fuzz", 1_800_000_000, [k(1), k(2), k(3)], vec![k(4)])
            .expect("genèse de fuzzing")
            .with_faucet(1_000_000_000_000_000)
            .expect("robinet")
            .into_state()
            .expect("état de genèse")
    })
}

fuzz_target!(|bytes: &[u8]| {
    if let Ok(tx) = borsh::from_slice::<SignedTransaction>(bytes) {
        let mut state = base().clone();
        let _ = state.apply_tx(&tx);
        assert!(state.check_invariant().is_ok());
        let _ = base().execute(1, 1_800_000_000_000, &[], &[tx]);
    }
    if let Ok(tx) = borsh::from_slice::<SignedAdminTransaction>(bytes) {
        let _ = base().execute(1, 1_800_000_000_000, &[tx], &[]);
    }
});
