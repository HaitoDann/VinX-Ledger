fn hash_account(account: &Account) -> Hash32 {
    let addr = account.address.as_str().as_bytes();
    let mut buf = Vec::with_capacity(addr.len() + 16 + 8 + 16 + 1 + 8);
    buf.extend_from_slice(addr);
    buf.extend_from_slice(&account.balance.atoms().to_be_bytes());
    buf.extend_from_slice(&account.nonce.to_be_bytes());
    buf.extend_from_slice(&account.staked.atoms().to_be_bytes());
    buf.push(account.frozen as u8);
    buf.extend_from_slice(&account.stake_since.to_be_bytes());
    sha256(&buf)
}

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
        s.block_height = 100;
        let distributed = s.distribute_staking_rewards();
        assert_eq!(distributed, Amount::ZERO);
        assert_eq!(s.staking_pool, Amount::from_vinx(100));
    }

    #[test]
    fn test_empty_pool_distributes_nothing() {
        let mut s = WorldState::new();
        let addr = staker(&mut s, Amount::from_vinx(1_000));
        s.staking_pool = Amount::ZERO;
        s.block_height = 100;
        let distributed = s.distribute_staking_rewards();
        assert_eq!(distributed, Amount::ZERO);
        assert_eq!(s.accounts[addr.as_str()].balance, Amount::ZERO);
    }

    #[test]
    fn test_single_staker_receives_full_pool() {
        let mut s = WorldState::new();
        let addr = staker(&mut s, Amount::from_vinx(1_000));
        s.staking_pool = Amount::from_vinx(50);
        s.block_height = 100;
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
        s.block_height = 100;
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
        s.block_height = 100;
        s.distribute_staking_rewards();
        assert_eq!(s.accounts[a.as_str()].balance, Amount::from_atoms(1));
        assert_eq!(s.accounts[b.as_str()].balance, Amount::from_atoms(1));
        assert_eq!(s.staking_pool, Amount::from_atoms(1));
    }

    #[test]
    fn test_non_stakers_receive_nothing() {
        let mut s = WorldState::new();
        let _staker_addr = staker(&mut s, Amount::from_vinx(1_000));
        let idle = Address::from_public_key(&KeyPair::generate().public_key());
        s.credit_for_test(idle.clone(), Amount::from_vinx(5_000));
        s.staking_pool = Amount::from_vinx(100);
        s.block_height = 100;
        s.distribute_staking_rewards();
        assert_eq!(s.accounts[idle.as_str()].balance, Amount::from_vinx(5_000));
    }

    #[test]
    fn test_no_distribution_outside_interval() {
        let mut s = WorldState::new();
        let addr = staker(&mut s, Amount::from_vinx(1_000));
        s.staking_pool = Amount::from_vinx(50);
        s.block_height = 99; // not a multiple of STAKING_DISTRIBUTION_INTERVAL
        let distributed = s.distribute_staking_rewards();
        assert_eq!(distributed, Amount::ZERO);
        assert_eq!(s.staking_pool, Amount::from_vinx(50)); // pool unchanged
        assert_eq!(s.accounts[addr.as_str()].balance, Amount::ZERO);
    }

    #[test]
    fn test_state_root_empty_is_zero() {
        let s = WorldState::new();
        assert_eq!(s.compute_state_root(), [0u8; 32]);
    }

    #[test]
    fn test_state_root_is_deterministic() {
        let mut s = WorldState::new();
        let addr = Address::from_public_key(&KeyPair::generate().public_key());
        s.credit_for_test(addr, Amount::from_vinx(100));
        assert_eq!(s.compute_state_root(), s.compute_state_root());
    }

    #[test]
    fn test_state_root_changes_on_balance_change() {
        let mut s = WorldState::new();
        let addr = Address::from_public_key(&KeyPair::generate().public_key());
        s.credit_for_test(addr.clone(), Amount::from_vinx(100));
        let root_before = s.compute_state_root();
        s.credit_for_test(addr, Amount::from_vinx(1));
        let root_after = s.compute_state_root();
        assert_ne!(root_before, root_after);
    }

    #[test]
    fn test_state_root_order_independent_of_insertion() {
        // Same accounts inserted in different order → same root
        let kp1 = KeyPair::generate();
        let kp2 = KeyPair::generate();
        let addr1 = Address::from_public_key(&kp1.public_key());
        let addr2 = Address::from_public_key(&kp2.public_key());

        let mut s1 = WorldState::new();
        s1.credit_for_test(addr1.clone(), Amount::from_vinx(50));
        s1.credit_for_test(addr2.clone(), Amount::from_vinx(200));

        let mut s2 = WorldState::new();
        s2.credit_for_test(addr2, Amount::from_vinx(200));
        s2.credit_for_test(addr1, Amount::from_vinx(50));

        assert_eq!(s1.compute_state_root(), s2.compute_state_root());
    }

    #[test]
    fn test_min_stake_enforced() {
        use vinx_core::{amount::DECIMAL_FACTOR, Transaction};
        let mut s = WorldState::new();
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        s.credit_for_test(addr.clone(), Amount::from_vinx(10));
        // Stake less than 1 VINX should fail
        let below_min = Amount::from_atoms(DECIMAL_FACTOR - 1);
        let tx = Transaction::new_stake(&kp, below_min, Amount::ZERO, 0);
        assert_eq!(
            s.apply_transaction(&tx),
            Err(vinx_core::CoreError::InvalidTransaction(
                "stake amount below minimum 1 VINX".to_string()
            ))
        );
    }
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use vinx_core::{
    amount::{
        Amount, DEFAULT_FEE_FLOOR_ATOMS, MIN_STAKE_ATOMS, STAKING_DISTRIBUTION_INTERVAL,
    },
    Account, CoreError, Transaction, TransactionType,
};
use vinx_crypto::{merkle_root, sha256, Address, Hash32};

/// In-memory representation of the full chain state.
/// Each call to `apply_transaction` mutates the state atomically.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldState {
    pub(crate) accounts: HashMap<String, Account>,
    /// Tokens in circulation (admin allocation + any future unlocks).
    pub circulating_supply: Amount,
    pub block_height: u64,
    /// Accumulated staking rewards waiting to be distributed every 100 blocks.
    pub staking_pool: Amount,
    /// VinX Labs treasury from fees.
    pub treasury: Amount,
    /// Minimum fee per transaction; adjustable by VinX Labs governance.
    pub fee_floor: Amount,
    /// Coffre Maturité: locked until 3 governance conditions are met.
    pub coffre_maturity: Amount,
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
            coffre_maturity: Amount::ZERO,
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
            TransactionType::Emission => Err(CoreError::InvalidTransaction(
                "emission disabled in Phase 1".to_string(),
            )),
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
        if tx.amount.atoms() < MIN_STAKE_ATOMS {
            return Err(CoreError::InvalidTransaction(
                "stake amount below minimum 1 VINX".to_string(),
            ));
        }
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
        // Record when staking began (only on first stake; adding to existing stake keeps original timestamp)
        if account.staked == Amount::ZERO {
            account.stake_since = self.block_height;
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
        // Reset stake timestamp when fully unstaked
        if account.staked == Amount::ZERO {
            account.stake_since = 0;
        }
        account.nonce += 1;
        Ok(())
    }

    /// Distributes the accumulated staking pool to all accounts with `staked > 0`,
    /// proportionally to their staked share. Only fires every STAKING_DISTRIBUTION_INTERVAL
    /// blocks. The integer-division remainder stays in the pool and rolls over.
    /// Returns the total amount distributed.
    pub fn distribute_staking_rewards(&mut self) -> Amount {
        if self.block_height == 0
            || self.block_height % STAKING_DISTRIBUTION_INTERVAL != 0
        {
            return Amount::ZERO;
        }
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

    /// Computes the Merkle root of the account state.
    ///
    /// Each account is hashed as:
    ///   SHA-256(address_utf8 || balance_be128 || nonce_be64 || staked_be128 || frozen_u8 || stake_since_be64)
    ///
    /// Accounts are sorted deterministically by address string before building the tree.
    pub fn compute_state_root(&self) -> Hash32 {
        let mut entries: Vec<&Account> = self.accounts.values().collect();
        entries.sort_by_key(|a| a.address.as_str());

        let leaves: Vec<Hash32> = entries.iter().map(|a| hash_account(a)).collect();
        merkle_root(&leaves)
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
        acc.stake_since = 0;
    }
}
