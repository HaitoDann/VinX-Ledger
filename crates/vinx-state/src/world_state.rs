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
