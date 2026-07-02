use proptest::prelude::*;
use vinx_core::{
    amount::{Amount, DECIMAL_FACTOR, DEFAULT_FEE_FLOOR_ATOMS},
    Transaction,
};
use vinx_crypto::{
    merkle_proof_for, merkle_root, sha256, verify_merkle_proof, Address, Hash32, KeyPair,
};
use vinx_state::WorldState;

// ─── Helper ──────────────────────────────────────────────────────────────────

/// Sum of all tokens held by accounts (Σ balances + staked).
fn total_in_accounts(state: &WorldState) -> u128 {
    state
        .accounts_sorted()
        .iter()
        .map(|a| a.balance.atoms() + a.staked.atoms())
        .sum()
}

// ─── Fee arithmetic ───────────────────────────────────────────────────────────

proptest! {
    /// calculate_fee must always return at least the floor.
    #[test]
    fn fee_respects_floor(amount_atoms in DECIMAL_FACTOR..=1_000_000 * DECIMAL_FACTOR) {
        let amount = Amount::from_atoms(amount_atoms);
        let floor  = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let fee    = amount.calculate_fee(floor);
        prop_assert!(fee >= floor, "fee {} < floor {}", fee.atoms(), floor.atoms());
    }
}

// ─── Merkle determinism & proof soundness ────────────────────────────────────

proptest! {
    /// Same leaves always produce the same Merkle root.
    #[test]
    fn merkle_root_is_deterministic(
        n    in 1usize..=16usize,
        seed in any::<u64>(),
    ) {
        let leaves: Vec<Hash32> = (0..n)
            .map(|i| sha256(&[seed.wrapping_shr((i % 64) as u32) as u8, i as u8]))
            .collect();
        prop_assert_eq!(merkle_root(&leaves), merkle_root(&leaves));
    }

    /// A proof generated for leaf i must verify against the root of those leaves.
    #[test]
    fn merkle_proof_is_valid_for_any_leaf(
        n    in 1usize..=16usize,
        seed in any::<u64>(),
    ) {
        let leaves: Vec<Hash32> = (0..n)
            .map(|i| sha256(&[seed.wrapping_shr((i % 64) as u32) as u8, i as u8]))
            .collect();
        let root = merkle_root(&leaves);
        for idx in 0..n {
            if let Some(proof) = merkle_proof_for(&leaves, idx) {
                prop_assert!(
                    verify_merkle_proof(&leaves[idx], &proof, &root),
                    "proof failed for leaf {} of {} (seed={})", idx, n, seed
                );
            }
        }
    }

    /// A proof from one tree must not verify against a different root.
    #[test]
    fn merkle_proof_rejects_wrong_root(
        n    in 2usize..=8usize,
        seed in any::<u64>(),
    ) {
        let leaves: Vec<Hash32> = (0..n)
            .map(|i| sha256(&[seed.wrapping_shr((i % 64) as u32) as u8, i as u8]))
            .collect();
        let correct_root = merkle_root(&leaves);
        // Build a wrong root by flipping a byte
        let mut wrong_root = correct_root;
        wrong_root[0] ^= 0xFF;

        if let Some(proof) = merkle_proof_for(&leaves, 0) {
            prop_assert!(
                !verify_merkle_proof(&leaves[0], &proof, &wrong_root),
                "proof accepted a wrong root"
            );
        }
    }
}

// ─── WorldState transfer invariants ──────────────────────────────────────────

proptest! {
    /// A transfer melts its fee, so `circulating + foundry` (the total supply) is
    /// conserved even though circulating_supply alone decreases by the fee.
    #[test]
    fn transfer_conserves_supply(
        amount_vinx  in 1u64..=500u64,
        initial_vinx in 1_000u64..=10_000u64,
    ) {
        let sender_kp   = KeyPair::generate();
        let sender_addr = Address::from_public_key(&sender_kp.public_key());
        let receiver_kp = KeyPair::generate();
        let receiver_addr = Address::from_public_key(&receiver_kp.public_key());

        let mut state = WorldState::new();
        let initial   = Amount::from_vinx(initial_vinx);
        state.credit_for_test(sender_addr.clone(), initial);
        state.circulating_supply = initial;

        let amount = Amount::from_vinx(amount_vinx);
        let fee    = amount.calculate_fee(state.base_fee);

        if let Some(total_cost) = amount.checked_add(fee) {
            if total_cost <= initial {
                let tx = Transaction::new_transfer(&sender_kp, receiver_addr, amount, fee, 0);
                let total_before = state.circulating_supply.atoms() + state.foundry_balance().atoms();
                state.apply_transaction(&tx).unwrap();
                prop_assert_eq!(
                    state.circulating_supply.atoms() + state.foundry_balance().atoms(),
                    total_before,
                    "supply (circulating + foundry) changed after transfer"
                );
            }
        }
    }

    /// After a valid transfer, `circulating_supply` must still equal the exact sum
    /// of tokens held by accounts (the field tracks the accounts precisely).
    #[test]
    fn circulating_tracks_accounts(
        amount_vinx  in 1u64..=500u64,
        initial_vinx in 1_000u64..=10_000u64,
    ) {
        let sender_kp   = KeyPair::generate();
        let sender_addr = Address::from_public_key(&sender_kp.public_key());
        let receiver_kp = KeyPair::generate();
        let receiver_addr = Address::from_public_key(&receiver_kp.public_key());

        let mut state = WorldState::new();
        let initial   = Amount::from_vinx(initial_vinx);
        state.credit_for_test(sender_addr.clone(), initial);
        state.circulating_supply = initial;

        let amount = Amount::from_vinx(amount_vinx);
        let fee    = amount.calculate_fee(state.base_fee);

        if let Some(total_cost) = amount.checked_add(fee) {
            if total_cost <= initial {
                let tx = Transaction::new_transfer(&sender_kp, receiver_addr, amount, fee, 0);
                state.apply_transaction(&tx).unwrap();
                prop_assert_eq!(
                    total_in_accounts(&state),
                    state.circulating_supply.atoms(),
                    "circulating_supply diverged from account balances"
                );
            }
        }
    }

    /// A valid transfer must increment the sender's nonce by exactly 1.
    #[test]
    fn transfer_increments_nonce(initial_vinx in 1_000u64..=10_000u64) {
        let sender_kp   = KeyPair::generate();
        let sender_addr = Address::from_public_key(&sender_kp.public_key());
        let receiver_kp = KeyPair::generate();
        let receiver_addr = Address::from_public_key(&receiver_kp.public_key());

        let mut state = WorldState::new();
        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(initial_vinx));

        let amount = Amount::from_vinx(1);
        let fee    = amount.calculate_fee(state.base_fee);
        let nonce_before = state.get_account(&sender_addr).map(|a| a.nonce).unwrap_or(0);

        let tx = Transaction::new_transfer(&sender_kp, receiver_addr, amount, fee, nonce_before);
        state.apply_transaction(&tx).unwrap();

        let nonce_after = state.get_account(&sender_addr).map(|a| a.nonce).unwrap();
        prop_assert_eq!(nonce_after, nonce_before + 1);
    }

    /// The exact fee must melt entirely into the Foundry with no leakage.
    #[test]
    fn fee_fully_melts_to_foundry(
        amount_vinx  in 1u64..=1_000u64,
        initial_vinx in 2_000u64..=20_000u64,
    ) {
        let sender_kp   = KeyPair::generate();
        let sender_addr = Address::from_public_key(&sender_kp.public_key());
        let receiver_kp = KeyPair::generate();
        let receiver_addr = Address::from_public_key(&receiver_kp.public_key());

        let mut state = WorldState::new();
        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(initial_vinx));

        let amount = Amount::from_vinx(amount_vinx);
        let fee    = amount.calculate_fee(state.base_fee);

        let foundry_before = state.foundry_balance().atoms();

        let tx = Transaction::new_transfer(&sender_kp, receiver_addr, amount, fee, 0);
        state.apply_transaction(&tx).unwrap();

        prop_assert_eq!(
            state.foundry_balance().atoms() - foundry_before,
            fee.atoms(),
            "fee did not fully melt into the Foundry"
        );
    }

    /// A transaction with a wrong nonce must always be rejected.
    #[test]
    fn wrong_nonce_always_rejected(
        initial_vinx in 1_000u64..=10_000u64,
        wrong_nonce  in 1u64..=100u64,
    ) {
        let sender_kp   = KeyPair::generate();
        let sender_addr = Address::from_public_key(&sender_kp.public_key());
        let receiver_kp = KeyPair::generate();
        let receiver_addr = Address::from_public_key(&receiver_kp.public_key());

        let mut state = WorldState::new();
        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(initial_vinx));

        // Sender's confirmed nonce is 0; use wrong_nonce (always >= 1) to trigger rejection.
        let amount = Amount::from_vinx(1);
        let fee    = amount.calculate_fee(state.base_fee);
        let tx     = Transaction::new_transfer(&sender_kp, receiver_addr, amount, fee, wrong_nonce);
        prop_assert!(state.apply_transaction(&tx).is_err(), "wrong nonce was accepted");
    }
}
