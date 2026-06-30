use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use vinx_core::{
    amount::{
        Amount, DEFAULT_FEE_FLOOR_ATOMS, FREEZE_DURATION_BLOCKS, MIN_STAKE_ATOMS,
        STAKING_DISTRIBUTION_INTERVAL,
    },
    block::SlashEvidence,
    chain_id::CHAIN_ID_DEVNET,
    governance::{CoffreCondition, GovernanceAction},
    protocol::{ProtocolVersion, ScheduledUpgrade},
    Account, CoreError, Transaction, TransactionType, ValidatorSet,
};
use vinx_crypto::{sha256, Address, Hash32, IncrementalMerkleTree};

/// In-memory representation of the full chain state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldState {
    pub(crate) accounts: HashMap<String, Account>,
    pub circulating_supply: Amount,
    pub block_height: u64,
    pub staking_pool: Amount,
    /// Accumulated melt fees waiting to be redistributed — 30% of every fee goes here.
    pub melt_pool: Amount,
    /// Pool des jetons à distribuer — tokens released from melt via ReleaseMeltToDistribution,
    /// and the Coffre Maturité when unlocked. Not staking rewards.
    #[serde(default)]
    pub distribution_pool: Amount,
    /// Static minimum fee floor; dynamic base_fee is always >= this.
    pub fee_floor: Amount,
    /// Dynamic fee floor, updated each block from mempool pressure.
    /// 1x at normal load, up to 3x at 100% mempool capacity.
    #[serde(default = "default_fee_floor")]
    pub base_fee: Amount,
    /// Pending validator reward for the current block (flushed to proposer at block end).
    /// 30% of every transaction fee accumulates here.
    #[serde(default)]
    pub validator_fee_pool: Amount,
    /// Protocol treasury: accumulates 20% of every transaction fee.
    /// Governed by admin — funds protocol development, audits, public policy.
    #[serde(default)]
    pub treasury: Amount,
    /// Coffre Maturité: locked until 3 governance conditions are met.
    pub coffre_maturity: Amount,
    #[serde(default)]
    pub coffre_mica_casp: bool,
    #[serde(default)]
    pub coffre_external_audit: bool,
    #[serde(default)]
    pub coffre_public_policy: bool,
    /// Address that may issue admin transactions (freeze, upgrade announcements).
    /// None = no admin restrictions (dev/test mode).
    pub admin_address: Option<Address>,
    /// Currently running protocol version.
    pub current_version: ProtocolVersion,
    /// Upgrade scheduled but not yet activated.
    pub pending_upgrade: Option<ScheduledUpgrade>,
    /// Active PoA validator set. Admin can add/remove validators via governance txs.
    pub validator_set: ValidatorSet,
    /// Chain ID for replay protection — transactions must match this value.
    #[serde(default = "default_chain_id")]
    pub chain_id: u32,
    /// Incremental Merkle tree over sorted account leaf hashes.
    /// Not persisted — rebuilt lazily on the first `compute_state_root` call after load.
    #[serde(skip)]
    merkle_tree: IncrementalMerkleTree,
    /// Maps address string → leaf index in `merkle_tree.leaves()`.
    #[serde(skip)]
    leaf_index: HashMap<String, usize>,
    /// Accounts modified since the last `compute_state_root` call.
    #[serde(skip)]
    dirty_addrs: HashSet<String>,
    /// True when an account was added/removed — requires a full O(n) rebuild.
    #[serde(skip)]
    needs_rebuild: bool,
}

fn default_fee_floor() -> Amount {
    Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS)
}

fn default_chain_id() -> u32 {
    CHAIN_ID_DEVNET
}

impl Default for WorldState {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldState {
    pub fn new() -> Self {
        let fee_floor = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        Self {
            accounts: HashMap::new(),
            circulating_supply: Amount::ZERO,
            block_height: 0,
            staking_pool: Amount::ZERO,
            melt_pool: Amount::ZERO,
            distribution_pool: Amount::ZERO,
            fee_floor,
            base_fee: fee_floor,
            validator_fee_pool: Amount::ZERO,
            treasury: Amount::ZERO,
            coffre_maturity: Amount::ZERO,
            coffre_mica_casp: false,
            coffre_external_audit: false,
            coffre_public_policy: false,
            admin_address: None,
            current_version: ProtocolVersion::GENESIS,
            pending_upgrade: None,
            // Placeholder — always overwritten by create_genesis_state before use.
            validator_set: ValidatorSet::single(Address::zero()),
            chain_id: CHAIN_ID_DEVNET,
            merkle_tree: IncrementalMerkleTree::new(),
            leaf_index: HashMap::new(),
            dirty_addrs: HashSet::new(),
            needs_rebuild: false,
        }
    }

    /// Marks an account address as dirty.
    /// If the address is not yet in the leaf index (new account), triggers a full rebuild.
    #[inline]
    fn mark_dirty(&mut self, addr: &str) {
        if !self.leaf_index.contains_key(addr) {
            self.needs_rebuild = true;
        }
        self.dirty_addrs.insert(addr.to_string());
    }

    /// O(n) full rebuild of the incremental tree — sorts all accounts, hashes each leaf,
    /// rebuilds the `leaf_index` map and all internal tree levels.
    fn full_rebuild(&mut self) {
        let mut entries: Vec<&Account> = self.accounts.values().collect();
        entries.sort_by_key(|a| a.address.as_str());
        self.leaf_index.clear();
        let leaves: Vec<Hash32> = entries
            .iter()
            .enumerate()
            .map(|(i, a)| {
                self.leaf_index.insert(a.address.to_string(), i);
                hash_account(a)
            })
            .collect();
        self.merkle_tree.rebuild(&leaves);
        self.dirty_addrs.clear();
        self.needs_rebuild = false;
    }

    /// Applies pending dirty-leaf updates to the tree, rebuilding fully if needed.
    fn flush_dirty(&mut self) {
        // Lazy full build on first call after deserialization or genesis.
        if self.leaf_index.is_empty() && !self.accounts.is_empty() {
            self.full_rebuild();
            return;
        }
        if self.dirty_addrs.is_empty() && !self.needs_rebuild {
            return;
        }
        if self.needs_rebuild {
            self.full_rebuild();
        } else {
            // O(|dirty| × log n) — only update changed leaf paths.
            let dirty: Vec<String> = std::mem::take(&mut self.dirty_addrs).into_iter().collect();
            for addr in dirty {
                if let (Some(&idx), Some(account)) =
                    (self.leaf_index.get(&addr), self.accounts.get(&addr))
                {
                    self.merkle_tree.update_leaf(idx, hash_account(account));
                }
            }
        }
    }

    /// Updates the dynamic base fee based on current mempool pressure.
    /// Applies EIP-1559-style surge pricing: 1× at ≤80% load, up to 3× at 100%.
    pub fn update_base_fee(&mut self, mempool_pending: usize, max_block_txs: usize) {
        let load_bps = if max_block_txs == 0 {
            0usize
        } else {
            (mempool_pending * 10_000) / max_block_txs
        };
        let multiplier_bps: u128 = if load_bps > 8_000 {
            // 1× + up to 2× extra at full load (linear: 3× at 100%)
            10_000 + (load_bps as u128 - 8_000) * 10
        } else {
            10_000
        };
        let new_atoms = DEFAULT_FEE_FLOOR_ATOMS * multiplier_bps / 10_000;
        self.base_fee = Amount::from_atoms(new_atoms.max(self.fee_floor.atoms()));
    }

    /// Drains and returns accumulated validator fee rewards for the current block.
    pub fn flush_validator_fee_pool(&mut self) -> Amount {
        let reward = self.validator_fee_pool;
        self.validator_fee_pool = Amount::ZERO;
        reward
    }

    /// Credits `amount` to `address`, creating the account if necessary.
    pub fn credit(&mut self, addr: &Address, amount: Amount) {
        if amount == Amount::ZERO {
            return;
        }
        self.mark_dirty(addr.as_str());
        let acc = self
            .accounts
            .entry(addr.as_str().to_string())
            .or_insert_with(|| Account::new(addr.clone()));
        acc.balance = acc.balance.saturating_add(amount);
    }

    pub fn get_account(&self, address: &Address) -> Option<&Account> {
        self.accounts.get(address.as_str())
    }

    /// Returns all accounts sorted by address (for Merkle proof computation).
    pub fn accounts_sorted(&self) -> Vec<&Account> {
        let mut entries: Vec<&Account> = self.accounts.values().collect();
        entries.sort_by_key(|a| a.address.as_str());
        entries
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

    pub fn treasury_balance(&self) -> Amount {
        self.treasury
    }

    /// Returns true when the chain must keep advancing even with an empty mempool.
    ///
    /// Two conditions require a periodic heartbeat block:
    /// - A protocol upgrade is scheduled (activation triggered by reaching a block height).
    /// - At least one account is frozen (auto-unfreeze triggered by block height elapsed).
    ///
    /// When neither condition holds, the node can sleep indefinitely until the next
    /// transaction arrives — no heartbeat needed, no wasted storage.
    pub fn has_pending_time_sensitive_ops(&self) -> bool {
        if self.pending_upgrade.is_some() {
            return true;
        }
        self.accounts.values().any(|a| a.frozen)
    }

    pub fn is_frozen(&self, address: &Address) -> bool {
        self.accounts
            .get(address.as_str())
            .map(|a| a.frozen)
            .unwrap_or(false)
    }

    pub(crate) fn insert_account(&mut self, account: Account) {
        self.mark_dirty(account.address.as_str());
        self.accounts
            .insert(account.address.as_str().to_string(), account);
    }

    pub fn credit_for_test(&mut self, address: Address, amount: Amount) {
        self.mark_dirty(address.as_str());
        let acc = self
            .accounts
            .entry(address.as_str().to_string())
            .or_insert_with(|| Account::new(address));
        acc.balance = acc.balance.saturating_add(amount);
    }

    pub fn apply_transaction(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        if tx.tx_type != TransactionType::Emission {
            // Chain-ID replay protection
            if tx.chain_id != self.chain_id {
                return Err(CoreError::InvalidTransaction(format!(
                    "chain_id mismatch: tx={} state={}",
                    tx.chain_id, self.chain_id
                )));
            }
            // TTL check
            if let Some(expires) = tx.expires_at_height {
                if self.block_height >= expires {
                    return Err(CoreError::InvalidTransaction(format!(
                        "transaction expired at height {} (current {})",
                        expires, self.block_height
                    )));
                }
            }
            let pk = tx.pub_key.as_ref().ok_or(CoreError::InvalidSignature)?;
            let derived = Address::from_public_key(pk);
            if derived != tx.from {
                return Err(CoreError::PubKeyMismatch);
            }
            let sig = tx.signature.as_ref().ok_or(CoreError::InvalidSignature)?;
            pk.verify(&tx.signing_bytes(), sig)?;

            // Sponsored transaction: validate sponsor's key and signature
            if let Some(ref sponsor_addr) = tx.sponsor {
                let spk = tx
                    .sponsor_pub_key
                    .as_ref()
                    .ok_or(CoreError::InvalidSignature)?;
                if &Address::from_public_key(spk) != sponsor_addr {
                    return Err(CoreError::PubKeyMismatch);
                }
                let ssig = tx
                    .sponsor_signature
                    .as_ref()
                    .ok_or(CoreError::InvalidSignature)?;
                spk.verify(&tx.signing_bytes(), ssig)?;
            }
        }

        match &tx.tx_type {
            TransactionType::Transfer => self.apply_transfer(tx),
            TransactionType::Stake => self.apply_stake(tx),
            TransactionType::Unstake => self.apply_unstake(tx),
            TransactionType::FreezeAccount => self.apply_freeze(tx),
            TransactionType::UnfreezeAccount => self.apply_unfreeze(tx),
            TransactionType::AnnounceUpgrade => self.apply_announce_upgrade(tx),
            TransactionType::AddValidator => self.apply_add_validator(tx),
            TransactionType::RemoveValidator => self.apply_remove_validator(tx),
            TransactionType::SlashValidator => self.apply_slash_validator(tx),
            TransactionType::AdminAction => self.apply_admin_action(tx),
            TransactionType::Emission => Err(CoreError::InvalidTransaction(
                "emission disabled in Phase 1".to_string(),
            )),
        }
    }

    fn check_admin(&self, tx: &Transaction) -> Result<(), CoreError> {
        if let Some(ref admin) = self.admin_address {
            if &tx.from != admin {
                return Err(CoreError::Unauthorized);
            }
        }
        Ok(())
    }

    fn apply_transfer(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        // Use dynamic base_fee (may be higher than fee_floor during network congestion)
        let expected_fee = tx.amount.calculate_fee(self.base_fee);
        if tx.fee < expected_fee {
            return Err(CoreError::InvalidTransaction(format!(
                "fee {} is below minimum {}",
                tx.fee, expected_fee
            )));
        }

        // Sponsored tx: sender pays only the transfer amount; sponsor pays the fee separately
        let (sender_debit, fee_payer) = if let Some(ref sponsor_addr) = tx.sponsor {
            (tx.amount, sponsor_addr.clone())
        } else {
            let total = tx
                .amount
                .checked_add(tx.fee)
                .ok_or(CoreError::AmountOverflow)?;
            (total, tx.from.clone())
        };

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
            if sender.balance < sender_debit {
                return Err(CoreError::InsufficientBalance);
            }
            sender.balance = sender.balance.checked_sub(sender_debit).unwrap();
            sender.nonce += 1;
        }

        // Debit fee from sponsor (if different from sender)
        if fee_payer != tx.from {
            let sponsor_acc = self
                .accounts
                .get_mut(fee_payer.as_str())
                .ok_or(CoreError::InsufficientBalance)?;
            if sponsor_acc.frozen {
                return Err(CoreError::AccountFrozen);
            }
            if sponsor_acc.balance < tx.fee {
                return Err(CoreError::InsufficientBalance);
            }
            sponsor_acc.balance = sponsor_acc.balance.checked_sub(tx.fee).unwrap();
        }

        let receiver = self
            .accounts
            .entry(tx.to.as_str().to_string())
            .or_insert_with(|| Account::new(tx.to.clone()));
        receiver.balance = receiver
            .balance
            .checked_add(tx.amount)
            .ok_or(CoreError::AmountOverflow)?;

        // Fee split: 80% validator / 20% treasury
        let validator_cut = Amount::validator_share(tx.fee);
        let treasury_cut = Amount::treasury_share(tx.fee);
        self.validator_fee_pool = self
            .validator_fee_pool
            .checked_add(validator_cut)
            .ok_or(CoreError::AmountOverflow)?;
        self.treasury = self
            .treasury
            .checked_add(treasury_cut)
            .ok_or(CoreError::AmountOverflow)?;

        self.mark_dirty(tx.from.as_str());
        self.mark_dirty(tx.to.as_str());
        if fee_payer != tx.from {
            self.mark_dirty(fee_payer.as_str());
        }

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
        if account.staked == Amount::ZERO {
            account.stake_since = self.block_height;
        }
        account.balance = account.balance.checked_sub(tx.amount).unwrap();
        account.staked = account
            .staked
            .checked_add(tx.amount)
            .ok_or(CoreError::AmountOverflow)?;
        account.nonce += 1;
        self.mark_dirty(tx.from.as_str());
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
        if account.staked == Amount::ZERO {
            account.stake_since = 0;
        }
        account.nonce += 1;
        self.mark_dirty(tx.from.as_str());
        Ok(())
    }

    fn apply_freeze(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        self.check_admin(tx)?;
        let sender = self
            .accounts
            .get_mut(tx.from.as_str())
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;

        let target = self
            .accounts
            .get_mut(tx.to.as_str())
            .ok_or(CoreError::InvalidTransaction(
                "target account does not exist".to_string(),
            ))?;
        if target.frozen {
            return Err(CoreError::InvalidTransaction(
                "account is already frozen".to_string(),
            ));
        }
        target.frozen = true;
        target.frozen_since = self.block_height;
        self.mark_dirty(tx.from.as_str());
        self.mark_dirty(tx.to.as_str());
        Ok(())
    }

    fn apply_unfreeze(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        self.check_admin(tx)?;
        let sender = self
            .accounts
            .get_mut(tx.from.as_str())
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;

        let target = self
            .accounts
            .get_mut(tx.to.as_str())
            .ok_or(CoreError::InvalidTransaction(
                "target account does not exist".to_string(),
            ))?;
        target.frozen = false;
        target.frozen_since = 0;
        self.mark_dirty(tx.from.as_str());
        self.mark_dirty(tx.to.as_str());
        Ok(())
    }

    fn apply_announce_upgrade(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        self.check_admin(tx)?;

        if self.pending_upgrade.is_some() {
            return Err(CoreError::UpgradeViolation(
                "an upgrade is already scheduled".to_string(),
            ));
        }

        let (new_version, activation_height) = tx
            .decode_upgrade_payload()
            .ok_or_else(|| CoreError::UpgradeViolation("malformed upgrade payload".to_string()))?;

        let upgrade_type = self.current_version.upgrade_type(&new_version);
        let min_notice = upgrade_type.min_notice_blocks();
        let announcement_height = self.block_height;

        if activation_height < announcement_height.saturating_add(min_notice) {
            return Err(CoreError::UpgradeViolation(format!(
                "{:?} upgrade requires {} blocks notice (announcement at {}, requested activation at {})",
                upgrade_type, min_notice, announcement_height, activation_height
            )));
        }

        let sender = self
            .accounts
            .get_mut(tx.from.as_str())
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;
        self.mark_dirty(tx.from.as_str());

        self.pending_upgrade = Some(ScheduledUpgrade {
            version: new_version,
            activation_height,
            announced_at: announcement_height,
        });

        tracing::info!(
            version = %self.pending_upgrade.as_ref().unwrap().version,
            activation_height,
            "Protocol upgrade scheduled"
        );

        Ok(())
    }

    fn apply_add_validator(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        self.check_admin(tx)?;
        if self.validator_set.contains(&tx.to) {
            return Err(CoreError::InvalidTransaction(
                "address is already a validator".to_string(),
            ));
        }
        let sender = self
            .accounts
            .get_mut(tx.from.as_str())
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;
        self.validator_set.add(tx.to.clone());
        self.mark_dirty(tx.from.as_str());
        tracing::info!(validator = %tx.to, "Validator added to set");
        Ok(())
    }

    fn apply_remove_validator(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        self.check_admin(tx)?;
        if self.validator_set.len() <= 1 {
            return Err(CoreError::InvalidTransaction(
                "cannot remove the last validator".to_string(),
            ));
        }
        if !self.validator_set.contains(&tx.to) {
            return Err(CoreError::InvalidTransaction(
                "address is not a validator".to_string(),
            ));
        }
        let sender = self
            .accounts
            .get_mut(tx.from.as_str())
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;
        self.validator_set.remove(&tx.to);
        self.mark_dirty(tx.from.as_str());
        tracing::info!(validator = %tx.to, "Validator removed from set");
        Ok(())
    }

    /// Checks for accounts that have been frozen longer than FREEZE_DURATION_BLOCKS
    /// and automatically unfreezes them. Called by the block producer on every block.
    pub fn check_auto_unfreeze(&mut self) {
        let height = self.block_height;
        let mut unfrozen: Vec<String> = Vec::new();
        for account in self.accounts.values_mut() {
            if account.frozen
                && height.saturating_sub(account.frozen_since) >= FREEZE_DURATION_BLOCKS
            {
                account.frozen = false;
                account.frozen_since = 0;
                unfrozen.push(account.address.to_string());
                tracing::info!(
                    address = %account.address,
                    height,
                    "Account automatically unfrozen (12-month limit reached)"
                );
            }
        }
        for addr in unfrozen {
            self.mark_dirty(&addr);
        }
    }

    /// Checks whether a pending upgrade should activate at the current block height
    /// and applies the version change if so.
    pub fn check_upgrade_activation(&mut self) {
        let should_activate = self
            .pending_upgrade
            .as_ref()
            .map(|u| self.block_height >= u.activation_height)
            .unwrap_or(false);

        if should_activate {
            let upgrade = self.pending_upgrade.take().unwrap();
            let old = self.current_version.clone();
            self.current_version = upgrade.version.clone();
            tracing::info!(
                from = %old,
                to = %upgrade.version,
                height = self.block_height,
                "Protocol upgrade activated"
            );
        }
    }

    /// Distributes staking pool every STAKING_DISTRIBUTION_INTERVAL blocks.
    pub fn distribute_staking_rewards(&mut self) -> Amount {
        if self.block_height == 0 || self.block_height % STAKING_DISTRIBUTION_INTERVAL != 0 {
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

        let mut rewarded: Vec<String> = Vec::new();
        for account in self.accounts.values_mut() {
            if account.staked == Amount::ZERO {
                continue;
            }
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
                rewarded.push(account.address.to_string());
            }
        }
        for addr in rewarded {
            self.mark_dirty(&addr);
        }

        self.staking_pool = self
            .staking_pool
            .checked_sub(Amount::from_atoms(distributed))
            .unwrap_or(Amount::ZERO);

        Amount::from_atoms(distributed)
    }

    /// Merkle root of the account state after sorting accounts by address.
    ///
    /// On a cold start (after deserialization) or when new accounts are added: O(n) full rebuild.
    /// When only existing accounts changed: O(|dirty| × log n) incremental update — typically
    /// O(txs_per_block × log n), orders of magnitude faster than O(n) for large account sets.
    pub fn compute_state_root(&mut self) -> Hash32 {
        self.flush_dirty();
        self.merkle_tree.root()
    }

    fn apply_slash_validator(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let evidence: SlashEvidence = bincode::deserialize(&tx.payload)
            .map_err(|_| CoreError::InvalidTransaction("malformed slash evidence".to_string()))?;

        let target = &tx.to;

        // Both signatures must be from the same validator (the target)
        if evidence.sig_a.validator != *target || evidence.sig_b.validator != *target {
            return Err(CoreError::InvalidTransaction(
                "evidence validator mismatch".to_string(),
            ));
        }

        if Address::from_public_key(&evidence.sig_a.pub_key) != *target {
            return Err(CoreError::InvalidTransaction(
                "sig_a pubkey/address mismatch".to_string(),
            ));
        }
        if Address::from_public_key(&evidence.sig_b.pub_key) != *target {
            return Err(CoreError::InvalidTransaction(
                "sig_b pubkey/address mismatch".to_string(),
            ));
        }

        // Signatures must be different (they signed different things)
        if evidence.sig_a.signature == evidence.sig_b.signature {
            return Err(CoreError::InvalidTransaction(
                "signatures are identical — not equivocation".to_string(),
            ));
        }

        if !self.validator_set.contains(target) {
            return Err(CoreError::InvalidTransaction(
                "target is not a validator".to_string(),
            ));
        }

        let sender = self
            .accounts
            .get_mut(tx.from.as_str())
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;

        // Slash: redirect the validator's stake to the redistribution pool
        let slashed = self
            .accounts
            .get(target.as_str())
            .map(|a| a.staked)
            .unwrap_or(Amount::ZERO);
        if slashed > Amount::ZERO {
            // 10% bounty to the reporter
            let bounty = Amount::from_atoms(slashed.atoms() / 10);
            // Remainder goes to the melt pool (redistribution reserve, not a burn)
            let to_melt = slashed.checked_sub(bounty).unwrap_or(Amount::ZERO);

            if let Some(acc) = self.accounts.get_mut(target.as_str()) {
                acc.staked = Amount::ZERO;
                acc.stake_since = 0;
            }
            self.mark_dirty(target.as_str());
            self.credit(&tx.from, bounty); // credit() also calls mark_dirty(tx.from)
            self.melt_pool = self.melt_pool.saturating_add(to_melt);
        }

        self.mark_dirty(tx.from.as_str());

        // Remove from validator set (can't produce blocks anymore)
        if self.validator_set.len() > 1 {
            self.validator_set.remove(target);
            tracing::warn!(validator = %target, slashed = %slashed, "Validator slashed for equivocation");
        } else {
            tracing::warn!(validator = %target, "Slash skipped — last validator");
        }

        Ok(())
    }

    fn apply_admin_action(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        // Require sender to be the admin
        self.check_admin(tx)?;

        let action: GovernanceAction = bincode::deserialize(&tx.payload).map_err(|_| {
            CoreError::InvalidTransaction("malformed governance action payload".to_string())
        })?;

        let sender = self
            .accounts
            .get_mut(tx.from.as_str())
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;

        match action {
            GovernanceAction::AddValidator(addr) => {
                if !self.validator_set.contains(&addr) {
                    self.validator_set.add(addr.clone());
                    tracing::info!(%addr, "Admin: validator added");
                }
            }
            GovernanceAction::RemoveValidator(addr) => {
                if self.validator_set.len() > 1 && self.validator_set.contains(&addr) {
                    self.validator_set.remove(&addr);
                    tracing::info!(%addr, "Admin: validator removed");
                }
            }
            GovernanceAction::UpdateFeeFloor { atoms } => {
                self.fee_floor = Amount::from_atoms(atoms as u128);
                if self.base_fee < self.fee_floor {
                    self.base_fee = self.fee_floor;
                }
                tracing::info!(atoms, "Admin: fee floor updated");
            }
            GovernanceAction::ScheduleUpgrade {
                version,
                activation_height,
            } => {
                if self.pending_upgrade.is_none() {
                    self.pending_upgrade = Some(vinx_core::ScheduledUpgrade {
                        version: version.clone(),
                        activation_height,
                        announced_at: self.block_height,
                    });
                    tracing::info!(activation_height, "Admin: upgrade scheduled");
                }
            }
            GovernanceAction::ReleaseMeltToDistribution { amount } => {
                if self.melt_pool >= amount {
                    self.melt_pool = self.melt_pool.checked_sub(amount).unwrap_or(Amount::ZERO);
                    self.distribution_pool = self.distribution_pool.saturating_add(amount);
                    tracing::info!(%amount, "Admin: melt released to distribution pool");
                }
            }
            GovernanceAction::RotateAdmin(new_admin) => {
                self.admin_address = Some(new_admin.clone());
                tracing::info!(%new_admin, "Admin: admin key rotated");
            }
            GovernanceAction::MarkCoffreCondition(condition) => {
                match condition {
                    CoffreCondition::MicaCasp => self.coffre_mica_casp = true,
                    CoffreCondition::ExternalAudit => self.coffre_external_audit = true,
                    CoffreCondition::PublicPolicy => self.coffre_public_policy = true,
                }
                tracing::info!(?condition, "Admin: Coffre condition marked");
            }
            GovernanceAction::UnlockCoffre => {
                if self.coffre_mica_casp && self.coffre_external_audit && self.coffre_public_policy
                {
                    let amount = self.coffre_maturity;
                    self.distribution_pool = self.distribution_pool.saturating_add(amount);
                    self.circulating_supply = self.circulating_supply.saturating_add(amount);
                    self.coffre_maturity = Amount::ZERO;
                    tracing::info!(%amount, "Admin: Coffre Maturité unlocked → distribution pool");
                } else {
                    return Err(CoreError::InvalidTransaction(
                        "UnlockCoffre rejected: not all 3 conditions are met".to_string(),
                    ));
                }
            }
        }

        self.mark_dirty(tx.from.as_str());
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

fn hash_account(account: &Account) -> Hash32 {
    let addr = account.address.as_str().as_bytes();
    let mut buf = Vec::with_capacity(addr.len() + 16 + 8 + 16 + 1 + 8 + 8);
    buf.extend_from_slice(addr);
    buf.extend_from_slice(&account.balance.atoms().to_be_bytes());
    buf.extend_from_slice(&account.nonce.to_be_bytes());
    buf.extend_from_slice(&account.staked.atoms().to_be_bytes());
    buf.push(account.frozen as u8);
    buf.extend_from_slice(&account.stake_since.to_be_bytes());
    buf.extend_from_slice(&account.frozen_since.to_be_bytes());
    sha256(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::{protocol::ProtocolVersion, Transaction};
    use vinx_crypto::{Address, KeyPair};

    fn staker(state: &mut WorldState, stake: Amount) -> Address {
        let addr = Address::from_public_key(&KeyPair::generate().public_key());
        state.set_staked_for_test(&addr, stake);
        addr
    }

    fn admin_state() -> (WorldState, KeyPair, Address) {
        let admin_kp = KeyPair::generate();
        let admin_addr = Address::from_public_key(&admin_kp.public_key());
        let mut state = WorldState::new();
        state.admin_address = Some(admin_addr.clone());
        state.credit_for_test(admin_addr.clone(), Amount::from_vinx(1_000));
        (state, admin_kp, admin_addr)
    }

    // ─── staking distribution ────────────────────────────────────────────────

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
        s.block_height = 99;
        let distributed = s.distribute_staking_rewards();
        assert_eq!(distributed, Amount::ZERO);
        assert_eq!(s.staking_pool, Amount::from_vinx(50));
        assert_eq!(s.accounts[addr.as_str()].balance, Amount::ZERO);
    }

    // ─── freeze / unfreeze via transaction ──────────────────────────────────

    #[test]
    fn test_admin_can_freeze_account() {
        let (mut state, admin_kp, admin_addr) = admin_state();
        let target_kp = KeyPair::generate();
        let target_addr = Address::from_public_key(&target_kp.public_key());
        state.credit_for_test(target_addr.clone(), Amount::from_vinx(100));

        let tx = Transaction::new_freeze(&admin_kp, target_addr.clone(), 0);
        state.apply_transaction(&tx).unwrap();

        assert!(state.is_frozen(&target_addr));
        assert_eq!(state.accounts[target_addr.as_str()].frozen_since, 0); // block_height=0
        assert_eq!(state.accounts[admin_addr.as_str()].nonce, 1);
    }

    #[test]
    fn test_non_admin_cannot_freeze() {
        let (mut state, _, _) = admin_state();
        let attacker_kp = KeyPair::generate();
        let attacker_addr = Address::from_public_key(&attacker_kp.public_key());
        let target_addr = Address::from_public_key(&KeyPair::generate().public_key());
        state.credit_for_test(attacker_addr, Amount::from_vinx(100));
        state.credit_for_test(target_addr.clone(), Amount::from_vinx(100));

        let tx = Transaction::new_freeze(&attacker_kp, target_addr.clone(), 0);
        assert_eq!(state.apply_transaction(&tx), Err(CoreError::Unauthorized));
        assert!(!state.is_frozen(&target_addr));
    }

    #[test]
    fn test_admin_can_unfreeze_account() {
        let (mut state, admin_kp, _) = admin_state();
        let target_kp = KeyPair::generate();
        let target_addr = Address::from_public_key(&target_kp.public_key());
        state.credit_for_test(target_addr.clone(), Amount::from_vinx(100));

        state
            .apply_transaction(&Transaction::new_freeze(&admin_kp, target_addr.clone(), 0))
            .unwrap();
        assert!(state.is_frozen(&target_addr));

        state
            .apply_transaction(&Transaction::new_unfreeze(
                &admin_kp,
                target_addr.clone(),
                1,
            ))
            .unwrap();
        assert!(!state.is_frozen(&target_addr));
        assert_eq!(state.accounts[target_addr.as_str()].frozen_since, 0);
    }

    #[test]
    fn test_auto_unfreeze_after_duration() {
        let (mut state, _, _) = admin_state();
        let addr = Address::from_public_key(&KeyPair::generate().public_key());
        state.credit_for_test(addr.clone(), Amount::from_vinx(100));

        // Freeze at block 0
        state.accounts.get_mut(addr.as_str()).unwrap().frozen = true;
        state.accounts.get_mut(addr.as_str()).unwrap().frozen_since = 0;

        // Not yet expired at block FREEZE_DURATION_BLOCKS - 1
        state.block_height = FREEZE_DURATION_BLOCKS - 1;
        state.check_auto_unfreeze();
        assert!(state.is_frozen(&addr));

        // Expires exactly at FREEZE_DURATION_BLOCKS
        state.block_height = FREEZE_DURATION_BLOCKS;
        state.check_auto_unfreeze();
        assert!(!state.is_frozen(&addr));
        assert_eq!(state.accounts[addr.as_str()].frozen_since, 0);
    }

    #[test]
    fn test_freeze_nonexistent_account_rejected() {
        let (mut state, admin_kp, _) = admin_state();
        let ghost = Address::from_public_key(&KeyPair::generate().public_key());
        let tx = Transaction::new_freeze(&admin_kp, ghost, 0);
        assert!(state.apply_transaction(&tx).is_err());
    }

    // ─── protocol upgrades ───────────────────────────────────────────────────

    #[test]
    fn test_admin_can_schedule_patch_upgrade() {
        let (mut state, admin_kp, _) = admin_state();
        let notice = vinx_core::amount::UPGRADE_NOTICE_PATCH_BLOCKS;
        let activation = state.block_height + notice + 100;

        let tx = Transaction::new_announce_upgrade(
            &admin_kp,
            ProtocolVersion::new(1, 0, 1),
            activation,
            0,
        );
        state.apply_transaction(&tx).unwrap();

        let upgrade = state.pending_upgrade.as_ref().unwrap();
        assert_eq!(upgrade.version, ProtocolVersion::new(1, 0, 1));
        assert_eq!(upgrade.activation_height, activation);
    }

    #[test]
    fn test_upgrade_too_soon_rejected() {
        let (mut state, admin_kp, _) = admin_state();
        // 1 block is way too short
        let tx = Transaction::new_announce_upgrade(&admin_kp, ProtocolVersion::new(2, 0, 0), 1, 0);
        assert!(matches!(
            state.apply_transaction(&tx),
            Err(CoreError::UpgradeViolation(_))
        ));
    }

    #[test]
    fn test_upgrade_activates_at_correct_height() {
        let (mut state, admin_kp, _) = admin_state();
        let notice = vinx_core::amount::UPGRADE_NOTICE_PATCH_BLOCKS;
        let activation = notice + 1;

        state
            .apply_transaction(&Transaction::new_announce_upgrade(
                &admin_kp,
                ProtocolVersion::new(1, 0, 1),
                activation,
                0,
            ))
            .unwrap();

        // Not yet activated
        state.block_height = activation - 1;
        state.check_upgrade_activation();
        assert_eq!(state.current_version, ProtocolVersion::GENESIS);
        assert!(state.pending_upgrade.is_some());

        // Activates exactly at activation_height
        state.block_height = activation;
        state.check_upgrade_activation();
        assert_eq!(state.current_version, ProtocolVersion::new(1, 0, 1));
        assert!(state.pending_upgrade.is_none());
    }

    #[test]
    fn test_non_admin_cannot_schedule_upgrade() {
        let (mut state, _, _) = admin_state();
        let attacker = KeyPair::generate();
        let attacker_addr = Address::from_public_key(&attacker.public_key());
        state.credit_for_test(attacker_addr, Amount::from_vinx(100));

        let notice = vinx_core::amount::UPGRADE_NOTICE_MAJOR_BLOCKS;
        let tx = Transaction::new_announce_upgrade(
            &attacker,
            ProtocolVersion::new(2, 0, 0),
            notice + 1,
            0,
        );
        assert_eq!(state.apply_transaction(&tx), Err(CoreError::Unauthorized));
    }

    // ─── Merkle state root ───────────────────────────────────────────────────

    #[test]
    fn test_state_root_empty_is_zero() {
        let mut s = WorldState::new();
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
        assert_ne!(root_before, s.compute_state_root());
    }

    #[test]
    fn test_state_root_order_independent_of_insertion() {
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
        use vinx_core::amount::DECIMAL_FACTOR;
        let mut s = WorldState::new();
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        s.credit_for_test(addr.clone(), Amount::from_vinx(10));
        let below_min = Amount::from_atoms(DECIMAL_FACTOR - 1);
        let tx = Transaction::new_stake(&kp, below_min, Amount::ZERO, 0);
        assert_eq!(
            s.apply_transaction(&tx),
            Err(CoreError::InvalidTransaction(
                "stake amount below minimum 1 VINX".to_string()
            ))
        );
    }
}
