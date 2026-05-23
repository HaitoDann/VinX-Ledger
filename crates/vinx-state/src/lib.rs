pub mod genesis;
pub mod world_state;

pub use genesis::{create_genesis_state, GenesisConfig};
pub use world_state::WorldState;

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::{
        amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS, EMISSION_PER_BLOCK_ATOMS, RESERVE_ALLOCATION_ATOMS},
        Transaction, TransactionType,
    };
    use vinx_crypto::{Address, KeyPair};

    // ─── helpers ────────────────────────────────────────────────────────────────

    fn funded_state() -> (WorldState, KeyPair, Address) {
        let sender_kp = KeyPair::generate();
        let sender_addr = Address::from_public_key(&sender_kp.public_key());
        let reserve_addr = Address::from_public_key(&KeyPair::generate().public_key());

        let state = create_genesis_state(&GenesisConfig {
            admin_address: sender_addr.clone(),
            reserve_address: reserve_addr,
        });
        (state, sender_kp, sender_addr)
    }

    fn floor() -> Amount {
        Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS)
    }

    /// Computes the correct protocol fee for a given transfer amount.
    fn fee_for(amount: Amount) -> Amount {
        amount.calculate_fee(floor())
    }

    // ─── transfer ───────────────────────────────────────────────────────────────

    #[test]
    fn test_transfer_happy_path() {
        let (mut state, sender_kp, _sender_addr) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1_000);
        let tx = Transaction::new_transfer(&sender_kp, receiver.clone(), amount, fee_for(amount), 0);

        state.apply_transaction(&tx).unwrap();

        assert_eq!(state.account_balance(&receiver), amount);
    }

    #[test]
    fn test_transfer_debits_sender_correctly() {
        let (mut state, sender_kp, sender_addr) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let initial = state.account_balance(&sender_addr);
        let amount = Amount::from_vinx(1_000);
        let fee = fee_for(amount);
        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 0);

        state.apply_transaction(&tx).unwrap();

        let expected = initial
            .checked_sub(amount)
            .unwrap()
            .checked_sub(fee)
            .unwrap();
        assert_eq!(state.account_balance(&sender_addr), expected);
    }

    #[test]
    fn test_transfer_insufficient_balance() {
        let (mut state, sender_kp, _) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        // Try to send more than total admin allocation (500M)
        let too_much = Amount::from_vinx(600_000_000);
        let tx = Transaction::new_transfer(&sender_kp, receiver, too_much, fee_for(too_much), 0);

        let result = state.apply_transaction(&tx);
        assert_eq!(result, Err(vinx_core::CoreError::InsufficientBalance));
    }

    #[test]
    fn test_transfer_frozen_account() {
        let (mut state, sender_kp, sender_addr) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());

        // Manually mark as frozen (admin multi-sig logic lives in governance layer)
        state.accounts.get_mut(sender_addr.as_str()).unwrap().frozen = true;

        let amount = Amount::from_vinx(1);
        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee_for(amount), 0);
        assert_eq!(
            state.apply_transaction(&tx),
            Err(vinx_core::CoreError::AccountFrozen)
        );
    }

    #[test]
    fn test_transfer_invalid_nonce() {
        let (mut state, sender_kp, _) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1);
        // Use nonce 1 but account nonce is 0
        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee_for(amount), 1);

        assert!(matches!(
            state.apply_transaction(&tx),
            Err(vinx_core::CoreError::InvalidNonce { expected: 0, got: 1 })
        ));
    }

    #[test]
    fn test_transfer_nonce_increments() {
        let (mut state, sender_kp, sender_addr) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1);

        let tx0 = Transaction::new_transfer(&sender_kp, receiver.clone(), amount, fee_for(amount), 0);
        state.apply_transaction(&tx0).unwrap();
        assert_eq!(state.accounts[sender_addr.as_str()].nonce, 1);

        let tx1 = Transaction::new_transfer(&sender_kp, receiver, amount, fee_for(amount), 1);
        state.apply_transaction(&tx1).unwrap();
        assert_eq!(state.accounts[sender_addr.as_str()].nonce, 2);
    }

    #[test]
    fn test_transfer_fee_split() {
        let (mut state, sender_kp, _) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(10_000); // 0.05% of 10_000 = 5 VinX fee
        let fee = fee_for(amount);

        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 0);
        state.apply_transaction(&tx).unwrap();

        let staking = Amount::staking_share(fee);
        let treasury = Amount::treasury_share(fee);
        assert_eq!(state.staking_pool, staking);
        assert_eq!(state.treasury, treasury);
        assert_eq!(staking.checked_add(treasury).unwrap(), fee);
    }

    #[test]
    fn test_tampered_signature_rejected() {
        let (mut state, sender_kp, _) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1);
        let mut tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee_for(amount), 0);
        // Tamper with the amount after signing
        tx.amount = Amount::from_vinx(999_999_999);

        assert_eq!(
            state.apply_transaction(&tx),
            Err(vinx_core::CoreError::Crypto(
                vinx_crypto::CryptoError::InvalidSignature.to_string()
            ))
        );
    }

    #[test]
    fn test_wrong_pubkey_rejected() {
        let (mut state, sender_kp, _) = funded_state();
        let attacker_kp = KeyPair::generate();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1);
        let mut tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee_for(amount), 0);
        // Replace pub_key with attacker's key
        tx.pub_key = Some(attacker_kp.public_key());

        assert_eq!(
            state.apply_transaction(&tx),
            Err(vinx_core::CoreError::PubKeyMismatch)
        );
    }

    // ─── stake / unstake ────────────────────────────────────────────────────────

    #[test]
    fn test_stake_moves_balance_to_staked() {
        let (mut state, sender_kp, sender_addr) = funded_state();
        let initial_balance = state.account_balance(&sender_addr);
        let stake_amount = Amount::from_vinx(1_000);

        let tx = Transaction::new_stake(&sender_kp, stake_amount, Amount::ZERO, 0);
        state.apply_transaction(&tx).unwrap();

        assert_eq!(
            state.account_balance(&sender_addr),
            initial_balance.checked_sub(stake_amount).unwrap()
        );
        assert_eq!(state.account_staked(&sender_addr), stake_amount);
    }

    #[test]
    fn test_unstake_moves_staked_back_to_balance() {
        let (mut state, sender_kp, sender_addr) = funded_state();
        let stake_amount = Amount::from_vinx(500);

        state
            .apply_transaction(&Transaction::new_stake(&sender_kp, stake_amount, Amount::ZERO, 0))
            .unwrap();
        let balance_after_stake = state.account_balance(&sender_addr);

        state
            .apply_transaction(&Transaction::new_unstake(&sender_kp, stake_amount, Amount::ZERO, 1))
            .unwrap();

        assert_eq!(
            state.account_balance(&sender_addr),
            balance_after_stake.checked_add(stake_amount).unwrap()
        );
        assert_eq!(state.account_staked(&sender_addr), Amount::ZERO);
    }

    #[test]
    fn test_cannot_unstake_more_than_staked() {
        let (mut state, sender_kp, _) = funded_state();
        let tx = Transaction::new_unstake(&sender_kp, Amount::from_vinx(1), Amount::ZERO, 0);
        assert_eq!(
            state.apply_transaction(&tx),
            Err(vinx_core::CoreError::InsufficientBalance)
        );
    }

    // ─── emission ───────────────────────────────────────────────────────────────

    #[test]
    fn test_emission_increases_circulating_supply() {
        let (mut state, _, _) = funded_state();
        let before = state.circulating_supply;
        let pool = Address::from_public_key(&KeyPair::generate().public_key());
        let reserve = Address::from_public_key(&KeyPair::generate().public_key());
        let emit_amount = Amount::from_atoms(EMISSION_PER_BLOCK_ATOMS);

        let tx = Transaction::new_emission(pool, emit_amount, reserve);
        state.apply_transaction(&tx).unwrap();

        assert_eq!(
            state.circulating_supply,
            before.checked_add(emit_amount).unwrap()
        );
    }

    #[test]
    fn test_emission_credits_pool_account() {
        let (mut state, _, _) = funded_state();
        let pool = Address::from_public_key(&KeyPair::generate().public_key());
        let reserve = Address::from_public_key(&KeyPair::generate().public_key());
        let emit_amount = Amount::from_atoms(EMISSION_PER_BLOCK_ATOMS);

        let tx = Transaction::new_emission(pool.clone(), emit_amount, reserve);
        state.apply_transaction(&tx).unwrap();

        assert_eq!(state.account_balance(&pool), emit_amount);
    }

    #[test]
    fn test_emission_decreases_reserve() {
        let (mut state, _, _) = funded_state();
        let before_reserve = state.protocol_reserve;
        let pool = Address::from_public_key(&KeyPair::generate().public_key());
        let reserve = Address::from_public_key(&KeyPair::generate().public_key());
        let emit_amount = Amount::from_atoms(EMISSION_PER_BLOCK_ATOMS);

        let tx = Transaction::new_emission(pool, emit_amount, reserve);
        state.apply_transaction(&tx).unwrap();

        assert_eq!(
            state.protocol_reserve,
            before_reserve.checked_sub(emit_amount).unwrap()
        );
    }

    #[test]
    fn test_emission_cannot_exceed_supply_cap() {
        let (mut state, _, _) = funded_state();
        // Force circulating supply near the cap
        state.circulating_supply = Amount::MAX_SUPPLY;
        let pool = Address::from_public_key(&KeyPair::generate().public_key());
        let reserve = Address::from_public_key(&KeyPair::generate().public_key());
        let tx = Transaction::new_emission(
            pool,
            Amount::from_atoms(EMISSION_PER_BLOCK_ATOMS),
            reserve,
        );
        assert_eq!(
            state.apply_transaction(&tx),
            Err(vinx_core::CoreError::SupplyCapExceeded)
        );
    }

    #[test]
    fn test_emission_amount_per_block() {
        let amount = WorldState::emission_amount_for_block(1);
        assert_eq!(amount.atoms(), EMISSION_PER_BLOCK_ATOMS);
    }

    #[test]
    fn test_emission_zero_after_period() {
        let amount = WorldState::emission_amount_for_block(40_000_000);
        assert_eq!(amount, Amount::ZERO);
    }

    // ─── freeze / unfreeze ──────────────────────────────────────────────────────

    #[test]
    fn test_freeze_and_unfreeze() {
        let (mut state, sender_kp, sender_addr) = funded_state();
        assert!(!state.is_frozen(&sender_addr));

        state.accounts.get_mut(sender_addr.as_str()).unwrap().frozen = true;
        assert!(state.is_frozen(&sender_addr));

        state.accounts.get_mut(sender_addr.as_str()).unwrap().frozen = false;
        assert!(!state.is_frozen(&sender_addr));
    }
}
