#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::{Address, KeyPair};

    fn staker(state: &mut WorldState, stake: Amount) -> Address {
        let addr = Address::from_public_key(&KeyPair::generate().public_key());
        state.set_staked_for_test(&addr, stake);
        addr
    }

    #[test]
    fn test_no_stakers_pool_unchanged() {
        let mut s = WorldState::new();
        s.staking_pool = Amount::from_vinx(100);
        let distributed = s.distribute_staking_rewards();
        assert_eq!(distributed, Amount::ZERO);
        assert_eq!(s.staking_pool, Amount::from_vinx(100));
    }

    #[test]
    fn test_empty_pool_distributes_nothing() {
        let mut s = WorldState::new();
        let addr = staker(&mut s, Amount::from_vinx(1_000));
        s.staking_pool = Amount::ZERO;
        let distributed = s.distribute_staking_rewards();
        assert_eq!(distributed, Amount::ZERO);
        assert_eq!(s.accounts[addr.as_str()].balance, Amount::ZERO);
    }

    #[test]
    fn test_single_staker_receives_full_pool() {
        let mut s = WorldState::new();
        let addr = staker(&mut s, Amount::from_vinx(1_000));
        s.staking_pool = Amount::from_vinx(50);
        let distributed = s.distribute_staking_rewards();
        assert_eq!(distributed, Amount::from_vinx(50));
        assert_eq!(s.staking_pool, Amount::ZERO);
        assert_eq!(s.accounts[addr.as_str()].balance, Amount::from_vinx(50));
    }

    #[test]
    fn test_two_stakers_proportional() {
        // Alice: 1 000 staked, Bob: 3 000 staked, pool: 400 → Alice 100, Bob 300
        let mut s = WorldState::new();
        let alice = staker(&mut s, Amount::from_vinx(1_000));
        let bob = staker(&mut s, Amount::from_vinx(3_000));
        s.staking_pool = Amount::from_vinx(400);
        s.distribute_staking_rewards();
        assert_eq!(s.accounts[alice.as_str()].balance, Amount::from_vinx(100));
        assert_eq!(s.accounts[bob.as_str()].balance, Amount::from_vinx(300));
        assert_eq!(s.staking_pool, Amount::ZERO);
    }

    #[test]
    fn test_remainder_stays_in_pool() {
        // 2 equal stakers, 3 atoms in pool → each gets 1, 1 stays in pool
        let mut s = WorldState::new();
        let a = staker(&mut s, Amount::from_vinx(1_000));
        let b = staker(&mut s, Amount::from_vinx(1_000));
        s.staking_pool = Amount::from_atoms(3);
        s.distribute_staking_rewards();
        assert_eq!(s.accounts[a.as_str()].balance, Amount::from_atoms(1));
        assert_eq!(s.accounts[b.as_str()].balance, Amount::from_atoms(1));
        assert_eq!(s.staking_pool, Amount::from_atoms(1));
    }

    #[test]
    fn test_non_stakers_receive_nothing() {
        let mut s = WorldState::new();
        let _staker_addr = staker(&mut s, Amount::from_vinx(1_000));
        // A second account with balance but no stake
        let idle = Address::from_public_key(&KeyPair::generate().public_key());
        s.credit_for_test(idle.clone(), Amount::from_vinx(5_000));
        s.staking_pool = Amount::from_vinx(100);
        s.distribute_staking_rewards();
        // Idle account's balance should be unchanged at 5_000 VINX
        assert_eq!(s.accounts[idle.as_str()].balance, Amount::from_vinx(5_000));
    }
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use vinx_core::{
    amount::{
        Amount, DEFAULT_FEE_FLOOR_ATOMS, EMISSION_PER_BLOCK_ATOMS, EMISSION_TOTAL_BLOCKS,
        RESERVE_ALLOCATION_ATOMS,
    },
    Account, CoreError, Transaction, TransactionType,
};
use vinx_crypto::Address;

/// In-memory representation of the full chain state.
/// Each call to `apply_transaction` mutates the state atomically.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldState {
    pub(crate) accounts: HashMap<String, Account>,
    /// Tokens in circulation (admin allocation + total emitted so far).
    pub circulating_supply: Amount,
    pub block_height: u64,
    /// Accumulated staking rewards waiting to be distributed.
    pub staking_pool: Amount,
    /// VinX Labs treasury from fees.
    pub treasury: Amount,
    /// Minimum fee per transaction; adjustable by VinX Labs governance.
    pub fee_floor: Amount,
    /// Remaining protocol reserve (decreases by EMISSION_PER_BLOCK_ATOMS each block).
    pub protocol_reserve: Amount,
}

impl Default for WorldState {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldState {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            circulating_supply: Amount::ZERO,
            block_height: 0,
            staking_pool: Amount::ZERO,
            treasury: Amount::ZERO,
            fee_floor: Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS),
            protocol_reserve: Amount::ZERO,
        }
    }

    pub fn get_account(&self, address: &Address) -> Option<&Account> {
        self.accounts.get(address.as_str())
    }

    pub fn account_balance(&self, address: &Address) -> Amount {
        self.accounts
            .get(address.as_str())
            .map(|a| a.balance)
            .unwrap_or(Amount::ZERO)
    }

    pub fn account_staked(&self, address: &Address) -> Amount {
        self.accounts
            .get(address.as_str())
            .map(|a| a.staked)
            .unwrap_or(Amount::ZERO)
    }

    pub fn is_frozen(&self, address: &Address) -> bool {
        self.accounts
            .get(address.as_str())
            .map(|a| a.frozen)
            .unwrap_or(false)
    }

    pub(crate) fn insert_account(&mut self, account: Account) {
        self.accounts
            .insert(account.address.as_str().to_string(), account);
    }

    /// Credits an address directly — for genesis setup and test scaffolding only.
    pub fn credit_for_test(&mut self, address: Address, amount: Amount) {
        let acc = self
            .accounts
            .entry(address.as_str().to_string())
            .or_insert_with(|| Account::new(address));
        acc.balance = acc.balance.saturating_add(amount);
    }

    /// Applies a single transaction, validating signature and business rules.
    pub fn apply_transaction(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        if tx.tx_type != TransactionType::Emission {
            let pk = tx.pub_key.as_ref().ok_or(CoreError::InvalidSignature)?;
            let derived = Address::from_public_key(pk);
            if derived != tx.from {
                return Err(CoreError::PubKeyMismatch);
            }
            let sig = tx.signature.as_ref().ok_or(CoreError::InvalidSignature)?;
            pk.verify(&tx.signing_bytes(), sig)?;
        }

        match &tx.tx_type {
            TransactionType::Transfer => self.apply_transfer(tx),
            TransactionType::Stake => self.apply_stake(tx),
            TransactionType::Unstake => self.apply_unstake(tx),
            TransactionType::FreezeAccount => self.apply_freeze(tx),
            TransactionType::UnfreezeAccount => self.apply_unfreeze(tx),
            TransactionType::Emission => self.apply_emission(tx),
        }
    }

    /// Returns the emission amount for the given block height.
    /// The last block emits whatever atoms remain in the reserve.
    pub fn emission_amount_for_block(block_height: u64) -> Amount {
        if block_height == EMISSION_TOTAL_BLOCKS {
            // Last block: emit the remainder to avoid leaving dust in the reserve
            let emitted_so_far = EMISSION_PER_BLOCK_ATOMS * (EMISSION_TOTAL_BLOCKS - 1) as u128;
            Amount::from_atoms(RESERVE_ALLOCATION_ATOMS - emitted_so_far)
        } else if block_height < EMISSION_TOTAL_BLOCKS {
            Amount::from_atoms(EMISSION_PER_BLOCK_ATOMS)
        } else {
            Amount::ZERO // emission period is over
        }
    }

    fn apply_transfer(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let expected_fee = tx.amount.calculate_fee(self.fee_floor);
        if tx.fee < expected_fee {
            return Err(CoreError::InvalidTransaction(format!(
                "fee {} is below minimum {}",
                tx.fee, expected_fee
            )));
        }
        let total_debit = tx
            .amount
            .checked_add(tx.fee)
            .ok_or(CoreError::AmountOverflow)?;

        // Validate and debit sender
        {
            let sender = self
                .accounts
                .get_mut(tx.from.as_str())
                .ok_or(CoreError::InsufficientBalance)?;
            if sender.frozen {
                return Err(CoreError::AccountFrozen);
            }
            if sender.nonce != tx.nonce {
                return Err(CoreError::InvalidNonce {
                    expected: sender.nonce,
                    got: tx.nonce,
                });
            }
            if sender.balance < total_debit {
                return Err(CoreError::InsufficientBalance);
            }
            sender.balance = sender.balance.checked_sub(total_debit).unwrap();
            sender.nonce += 1;
        }

        // Credit receiver (create account if new)
        let receiver = self
            .accounts
            .entry(tx.to.as_str().to_string())
            .or_insert_with(|| Account::new(tx.to.clone()));
        receiver.balance = receiver
            .balance
            .checked_add(tx.amount)
            .ok_or(CoreError::AmountOverflow)?;

        // Distribute fee: 80% staking pool, 20% treasury
        let staking = Amount::staking_share(tx.fee);
        let treasury = Amount::treasury_share(tx.fee);
        self.staking_pool = self
            .staking_pool
            .checked_add(staking)
            .ok_or(CoreError::AmountOverflow)?;
        self.treasury = self
            .treasury
            .checked_add(treasury)
            .ok_or(CoreError::AmountOverflow)?;

        Ok(())
    }

    fn apply_stake(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let account = self
            .accounts
            .get_mut(tx.from.as_str())
            .ok_or(CoreError::InsufficientBalance)?;
        if account.frozen {
            return Err(CoreError::AccountFrozen);
        }
        if account.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: account.nonce,
                got: tx.nonce,
            });
        }
        if account.balance < tx.amount {
            return Err(CoreError::InsufficientBalance);
        }
        account.balance = account.balance.checked_sub(tx.amount).unwrap();
        account.staked = account
            .staked
            .checked_add(tx.amount)
            .ok_or(CoreError::AmountOverflow)?;
        account.nonce += 1;
        Ok(())
    }

    fn apply_unstake(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let account = self
            .accounts
            .get_mut(tx.from.as_str())
            .ok_or(CoreError::InsufficientBalance)?;
        if account.frozen {
            return Err(CoreError::AccountFrozen);
        }
        if account.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: account.nonce,
                got: tx.nonce,
            });
        }
        if account.staked < tx.amount {
            return Err(CoreError::InsufficientBalance);
        }
        account.staked = account.staked.checked_sub(tx.amount).unwrap();
        account.balance = account
            .balance
            .checked_add(tx.amount)
            .ok_or(CoreError::AmountOverflow)?;
        account.nonce += 1;
        Ok(())
    }

    /// Distributes the accumulated staking pool to all accounts with `staked > 0`,
    /// proportionally to their staked share. The integer-division remainder stays in
    /// the pool and rolls over to the next block.
    /// Returns the total amount distributed.
    pub fn distribute_staking_rewards(&mut self) -> Amount {
        if self.staking_pool == Amount::ZERO {
            return Amount::ZERO;
        }

        let total_staked: u128 = self.accounts.values().map(|a| a.staked.atoms()).sum();
        if total_staked == 0 {
            return Amount::ZERO;
        }

        let pool = self.staking_pool.atoms();
        let mut distributed = 0u128;

        for account in self.accounts.values_mut() {
            if account.staked == Amount::ZERO {
                continue;
            }
            // reward = floor(pool * staked / total_staked)
            // checked_mul guards against overflow; fallback scales down by 10^9
            // (loses < 1 VINX precision) for extreme values.
            let reward = pool
                .checked_mul(account.staked.atoms())
                .map(|p| p / total_staked)
                .unwrap_or_else(|| {
                    let p = pool / 1_000_000_000;
                    let t = (total_staked / 1_000_000_000).max(1);
                    p.saturating_mul(account.staked.atoms()) / t
                });
            if reward > 0 {
                account.balance = account.balance.saturating_add(Amount::from_atoms(reward));
                distributed = distributed.saturating_add(reward);
            }
        }

        self.staking_pool = self
            .staking_pool
            .checked_sub(Amount::from_atoms(distributed))
            .unwrap_or(Amount::ZERO);

        Amount::from_atoms(distributed)
    }

    fn apply_freeze(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let account = self
            .accounts
            .entry(tx.to.as_str().to_string())
            .or_insert_with(|| Account::new(tx.to.clone()));
        account.frozen = true;
        Ok(())
    }

    fn apply_unfreeze(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        if let Some(account) = self.accounts.get_mut(tx.to.as_str()) {
            account.frozen = false;
        }
        Ok(())
    }

    #[cfg(test)]
    fn set_staked_for_test(&mut self, address: &Address, staked: Amount) {
        let acc = self
            .accounts
            .entry(address.as_str().to_string())
            .or_insert_with(|| vinx_core::Account::new(address.clone()));
        acc.balance = Amount::ZERO;
        acc.staked = staked;
    }

    fn apply_emission(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let new_supply = self
            .circulating_supply
            .checked_add(tx.amount)
            .ok_or(CoreError::AmountOverflow)?;
        if new_supply > Amount::MAX_SUPPLY {
            return Err(CoreError::SupplyCapExceeded);
        }
        self.protocol_reserve = self
            .protocol_reserve
            .checked_sub(tx.amount)
            .ok_or(CoreError::InsufficientBalance)?;

        let pool = self
            .accounts
            .entry(tx.to.as_str().to_string())
            .or_insert_with(|| Account::new(tx.to.clone()));
        pool.balance = pool
            .balance
            .checked_add(tx.amount)
            .ok_or(CoreError::AmountOverflow)?;

        self.circulating_supply = new_supply;
        Ok(())
    }
}
