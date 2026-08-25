pub mod genesis;
pub mod world_state;

pub use genesis::{create_genesis_state, create_genesis_state_with_dev_prefund, GenesisConfig};
pub use world_state::{
    v8_meta_suffix, v9_meta_suffix, v10_meta_suffix, v11_meta_suffix, v12_meta_suffix,
    v13_meta_suffix, v16_meta_suffix, v17_meta_suffix, v18_meta_suffix, AdminPolicy,
    GovernanceProposal, ModuleEntry, WorldState,
};

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::{
        amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS},
        Transaction,
    };
    use vinx_crypto::{Address, KeyPair};

    // ─── helpers ────────────────────────────────────────────────────────────────

    fn funded_state() -> (WorldState, KeyPair, Address) {
        let sender_kp = KeyPair::generate();
        let sender_addr = Address::from_public_key(&sender_kp.public_key());

        let validator_kp = KeyPair::generate();
        let validator_addr = Address::from_public_key(&validator_kp.public_key());
        let mut state = create_genesis_state(&GenesisConfig {
            chain_id: vinx_core::CHAIN_ID_DEVNET,
            admin_address: sender_addr.clone(),
            validator_address: validator_addr,
        });
        // Fair launch grants nothing at genesis — fund the sender for these unit tests.
        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(1_000_000_000));
        (state, sender_kp, sender_addr)
    }

    fn floor() -> Amount {
        Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS)
    }

    fn fee_for(amount: Amount) -> Amount {
        amount.calculate_fee(floor())
    }

    // ─── transfer ───────────────────────────────────────────────────────────────

    #[test]
    fn test_transfer_happy_path() {
        let (mut state, sender_kp, _sender_addr) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1_000);
        let tx =
            Transaction::new_transfer(&sender_kp, receiver.clone(), amount, fee_for(amount), 0);

        state.apply_transaction(&tx).unwrap();

        assert_eq!(state.account_balance(&receiver), amount);
    }

    #[test]
    fn test_apply_trusted_skips_signature_but_enforces_state() {
        let (mut state, sender_kp, _sender_addr) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1_000);
        let mut tx =
            Transaction::new_transfer(&sender_kp, receiver.clone(), amount, fee_for(amount), 0);

        // Tamper a signed field *after* signing so the Ed25519 signature no longer
        // verifies, without changing any economic field (TTL is far in the future).
        tx.expires_at_height = Some(1_000_000);

        // Full verification path rejects the now-invalid signature.
        let mut full = state.clone();
        assert!(
            full.apply_transaction(&tx).is_err(),
            "full path must reject a tampered signature"
        );

        // Trusted path skips signature verification and applies the transfer.
        state.apply_transaction_trusted(&tx).unwrap();
        assert_eq!(state.account_balance(&receiver), amount);

        // Trusted still enforces nonce — replaying nonce 0 is rejected.
        assert!(matches!(
            state.apply_transaction_trusted(&tx),
            Err(vinx_core::CoreError::InvalidNonce { .. })
        ));
    }

    #[test]
    fn test_apply_trusted_still_enforces_chain_id_and_ttl() {
        let (mut state, sender_kp, _) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1);

        // Wrong chain-id is rejected even on the trusted path (cheap replay guard).
        let mut wrong_chain =
            Transaction::new_transfer(&sender_kp, receiver.clone(), amount, fee_for(amount), 0);
        wrong_chain.chain_id = vinx_core::CHAIN_ID_MAINNET;
        assert!(state.apply_transaction_trusted(&wrong_chain).is_err());

        // Expired TTL is rejected even on the trusted path.
        state.block_height = 100;
        let mut expired =
            Transaction::new_transfer(&sender_kp, receiver, amount, fee_for(amount), 0);
        expired.expires_at_height = Some(50);
        assert!(state.apply_transaction_trusted(&expired).is_err());
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
        // Try to send more than total admin allocation (21B)
        let too_much = Amount::from_vinx(22_000_000_000);
        let tx = Transaction::new_transfer(&sender_kp, receiver, too_much, fee_for(too_much), 0);

        let result = state.apply_transaction(&tx);
        assert_eq!(result, Err(vinx_core::CoreError::InsufficientBalance));
    }

    #[test]
    fn test_transfer_invalid_nonce() {
        let (mut state, sender_kp, _) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1);
        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee_for(amount), 1);

        assert!(matches!(
            state.apply_transaction(&tx),
            Err(vinx_core::CoreError::InvalidNonce {
                expected: 0,
                got: 1
            })
        ));
    }

    #[test]
    fn test_transfer_nonce_increments() {
        let (mut state, sender_kp, sender_addr) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1);

        let tx0 =
            Transaction::new_transfer(&sender_kp, receiver.clone(), amount, fee_for(amount), 0);
        state.apply_transaction(&tx0).unwrap();
        assert_eq!(state.accounts[&sender_addr].nonce, 1);

        let tx1 = Transaction::new_transfer(&sender_kp, receiver, amount, fee_for(amount), 1);
        state.apply_transaction(&tx1).unwrap();
        assert_eq!(state.accounts[&sender_addr].nonce, 2);
    }

    #[test]
    fn test_transfer_fee_goes_to_producer() {
        let (mut state, sender_kp, _) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let producer = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(10_000);
        let fee = fee_for(amount);

        let pot_before = state.epoch_dist_emission_pot;
        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 0);
        state.apply_transaction(&tx).unwrap();
        state.settle_block(&producer, 1, 1);

        // 100% of the fee goes to the block producer; the epoch pot is untouched.
        assert_eq!(state.account_balance(&producer), fee);
        assert_eq!(state.epoch_dist_emission_pot, pot_before);
    }

    #[test]
    fn test_tampered_signature_rejected() {
        let (mut state, sender_kp, _) = funded_state();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(1);
        let mut tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee_for(amount), 0);
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
    fn test_unstake_enters_unbonding_then_returns_after_delay() {
        use vinx_core::amount::UNBONDING_SECS;
        let (mut state, sender_kp, sender_addr) = funded_state();
        let stake_amount = Amount::from_vinx(500);

        state
            .apply_transaction(&Transaction::new_stake(
                &sender_kp,
                stake_amount,
                Amount::ZERO,
                0,
            ))
            .unwrap();
        let balance_after_stake = state.account_balance(&sender_addr);

        // Unstake at ts = 1000: the bond leaves `staked` but does NOT return yet.
        state.set_block_context(1_000);
        state
            .apply_transaction(&Transaction::new_unstake(
                &sender_kp,
                stake_amount,
                Amount::ZERO,
                1,
            ))
            .unwrap();
        assert_eq!(state.account_staked(&sender_addr), Amount::ZERO);
        assert_eq!(state.account_balance(&sender_addr), balance_after_stake); // not yet back

        // After the unbonding delay, settling matures it back to the balance.
        state.settle_block(&sender_addr, 1, 1_000 + UNBONDING_SECS);
        assert_eq!(
            state.account_balance(&sender_addr),
            balance_after_stake.checked_add(stake_amount).unwrap()
        );
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

    #[test]
    fn test_stake_below_minimum_rejected() {
        let (mut state, sender_kp, _) = funded_state();
        use vinx_core::amount::DECIMAL_FACTOR;
        let below_min = Amount::from_atoms(DECIMAL_FACTOR - 1);
        let tx = Transaction::new_stake(&sender_kp, below_min, Amount::ZERO, 0);
        assert_eq!(
            state.apply_transaction(&tx),
            Err(vinx_core::CoreError::InvalidTransaction(
                "stake amount below minimum 1 VINX".to_string()
            ))
        );
    }
}
