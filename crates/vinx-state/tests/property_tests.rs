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

/// Sum of all account balances + staked amounts + protocol pools.
fn total_tracked(state: &WorldState) -> u128 {
    let accounts: u128 = state
        .accounts_sorted()
        .iter()
        .map(|a| a.balance.atoms() + a.staked.atoms())
        .sum();
    accounts
        + state.staking_pool.atoms()
        + state.melt_pool.atoms()
        + state.distribution_pool.atoms()
        + state.validator_fee_pool.atoms()
        + state.treasury_balance().atoms()
}

// ─── Fee arithmetic ───────────────────────────────────────────────────────────

proptest! {
    /// The two fee shares must always sum to the exact fee (no atoms lost to rounding).
    #[test]
    fn fee_split_sums_to_fee(atoms in 1u128..=1_000_000_000 * DECIMAL_FACTOR) {
        let fee = Amount::from_atoms(atoms);
        let validator = Amount::validator_share(fee);
        let treasury  = Amount::treasury_share(fee);
        prop_assert_eq!(
            validator.atoms() + treasury.atoms(),
            fee.atoms(),
            "fee split lost atoms: validator={} treasury={} fee={}",
            validator.atoms(), treasury.atoms(), fee.atoms()
        );
    }

    /// Each individual share must never exceed the total fee.
    #[test]
    fn fee_parts_never_exceed_total(atoms in 1u128..=1_000_000_000 * DECIMAL_FACTOR) {
        let fee = Amount::from_atoms(atoms);
        prop_assert!(Amount::validator_share(fee) <= fee);
        prop_assert!(Amount::treasury_share(fee)  <= fee);
    }

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
    /// After a valid transfer, circulating_supply must not change.
    #[test]
    fn transfer_preserves_circulating_supply(
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
                let supply_before = state.circulating_supply;
                state.apply_transaction(&tx).unwrap();
                prop_assert_eq!(state.circulating_supply, supply_before,
                    "circulating_supply changed after transfer");
            }
        }
    }

    /// After a valid transfer, the total of all tracked pools + account balances
    /// must equal circulating_supply (conservation law).
    #[test]
    fn transfer_conservation_law(
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
                    total_tracked(&state),
                    state.circulating_supply.atoms(),
                    "conservation law violated after transfer"
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

    /// The exact fee must flow entirely into the three pools with no leakage.
    #[test]
    fn fee_fully_distributed_to_pools(
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

        let validator_before = state.validator_fee_pool.atoms();
        let treasury_before  = state.treasury_balance().atoms();

        let tx = Transaction::new_transfer(&sender_kp, receiver_addr, amount, fee, 0);
        state.apply_transaction(&tx).unwrap();

        let validator_gained = state.validator_fee_pool.atoms() - validator_before;
        let treasury_gained  = state.treasury_balance().atoms() - treasury_before;

        prop_assert_eq!(
            validator_gained + treasury_gained,
            fee.atoms(),
            "fee not fully distributed: distributed={} expected={}",
            validator_gained + treasury_gained,
            fee.atoms()
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
