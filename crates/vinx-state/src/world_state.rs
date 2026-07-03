use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use vinx_core::{
    amount::{
        Amount, DEFAULT_FEE_FLOOR_ATOMS, FORGE_RATE_BPS, FORGE_RATE_DENOM, MIN_STAKE_ATOMS,
        STAKING_DISTRIBUTION_INTERVAL,
    },
    block::SlashEvidence,
    chain_id::CHAIN_ID_DEVNET,
    governance::GovernanceAction,
    protocol::{ProtocolVersion, ScheduledUpgrade},
    Account, CoreError, Transaction, TransactionType, ValidatorSet,
};
use vinx_crypto::{sha256, Address, Hash32, IncrementalMerkleTree};

/// In-memory representation of the full chain state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldState {
    /// Accounts keyed by bech32 address. A `BTreeMap` (not `HashMap`) so iteration
    /// is already sorted by address — the Merkle leaf order — avoiding an O(n log n)
    /// sort on every state-root rebuild and every inclusion-proof lookup.
    pub(crate) accounts: BTreeMap<Address, Account>,
    /// Tokens held by accounts (Σ balances + staked). Always equals `MAX_SUPPLY - foundry`:
    /// the supply is conserved forever (no burn), it only cycles between accounts and the Foundry.
    pub circulating_supply: Amount,
    pub block_height: u64,
    /// La Fonderie — the melt/forge reserve. Transaction fees *melt* into it; staking
    /// rewards are *forged* out of it. The invariant `circulating_supply + foundry ==
    /// MAX_SUPPLY` holds at every block: nothing is ever created or destroyed, only recycled.
    #[serde(default)]
    pub foundry: Amount,
    /// Static minimum fee floor; dynamic base_fee is always >= this.
    pub fee_floor: Amount,
    /// Dynamic fee floor, updated each block from mempool pressure.
    /// 1x at normal load, up to 3x at 100% mempool capacity.
    #[serde(default = "default_fee_floor")]
    pub base_fee: Amount,
    /// Address that may issue admin transactions (validator set, upgrades, admin rotation).
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
    leaf_index: HashMap<Address, usize>,
    /// Accounts modified since the last `compute_state_root` call.
    #[serde(skip)]
    dirty_addrs: HashSet<Address>,
    /// True when an account was added/removed — requires a full O(n) rebuild.
    #[serde(skip)]
    needs_rebuild: bool,
    /// Accounts modified since the last persistence flush. Distinct from
    /// `dirty_addrs` (which is consumed by `compute_state_root`): this set survives
    /// until `take_persist_dirty` drains it, so incremental persistence can write
    /// only the accounts that actually changed instead of the whole map.
    #[serde(skip)]
    persist_dirty: HashSet<Address>,
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
            accounts: BTreeMap::new(),
            circulating_supply: Amount::ZERO,
            block_height: 0,
            foundry: Amount::ZERO,
            fee_floor,
            base_fee: fee_floor,
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
            persist_dirty: HashSet::new(),
        }
    }

    /// Marks an account address as dirty.
    /// If the address is not yet in the leaf index (new account), triggers a full rebuild.
    #[inline]
    fn mark_dirty(&mut self, addr: &Address) {
        if !self.leaf_index.contains_key(addr) {
            self.needs_rebuild = true;
        }
        self.dirty_addrs.insert(*addr);
        self.persist_dirty.insert(*addr);
    }

    /// O(n) full rebuild of the incremental tree — sorts all accounts, hashes each leaf,
    /// rebuilds the `leaf_index` map and all internal tree levels.
    fn full_rebuild(&mut self) {
        // `accounts` is a BTreeMap, so `values()` already yields address-sorted order.
        let entries: Vec<&Account> = self.accounts.values().collect();
        self.leaf_index.clear();
        let leaves: Vec<Hash32> = entries
            .iter()
            .enumerate()
            .map(|(i, a)| {
                self.leaf_index.insert(a.address, i);
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
            let dirty: Vec<Address> = std::mem::take(&mut self.dirty_addrs).into_iter().collect();
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

    /// *Melt*: moves `amount` from circulation into the Foundry (fees, slashing).
    /// Preserves the invariant `circulating_supply + foundry == MAX_SUPPLY`.
    fn melt_to_foundry(&mut self, amount: Amount) {
        self.foundry = self.foundry.saturating_add(amount);
        self.circulating_supply = self
            .circulating_supply
            .checked_sub(amount)
            .unwrap_or(Amount::ZERO);
    }

    /// *Forge*: draws up to `amount` out of the Foundry back into circulation
    /// (staking rewards). Returns the amount actually forged (capped by the reserve).
    fn forge_from_foundry(&mut self, amount: Amount) -> Amount {
        let forged = if self.foundry >= amount {
            amount
        } else {
            self.foundry
        };
        self.foundry = self.foundry.checked_sub(forged).unwrap_or(Amount::ZERO);
        self.circulating_supply = self.circulating_supply.saturating_add(forged);
        forged
    }

    /// Credits `amount` to `address`, creating the account if necessary.
    pub fn credit(&mut self, addr: &Address, amount: Amount) {
        if amount == Amount::ZERO {
            return;
        }
        self.mark_dirty(addr);
        let acc = self
            .accounts
            .entry(*addr)
            .or_insert_with(|| Account::new(*addr));
        acc.balance = acc.balance.saturating_add(amount);
    }

    pub fn get_account(&self, address: &Address) -> Option<&Account> {
        self.accounts.get(address)
    }

    /// Returns all accounts sorted by address (for Merkle proof computation).
    /// `accounts` is a BTreeMap, so `values()` is already in address order.
    pub fn accounts_sorted(&self) -> Vec<&Account> {
        self.accounts.values().collect()
    }

    pub fn account_balance(&self, address: &Address) -> Amount {
        self.accounts
            .get(address)
            .map(|a| a.balance)
            .unwrap_or(Amount::ZERO)
    }

    pub fn account_staked(&self, address: &Address) -> Amount {
        self.accounts
            .get(address)
            .map(|a| a.staked)
            .unwrap_or(Amount::ZERO)
    }

    /// La Fonderie balance — the melt/forge reserve.
    pub fn foundry_balance(&self) -> Amount {
        self.foundry
    }

    // ---- Incremental persistence support ----------------------------------

    /// Serializes every state field **except** the `accounts` map, which is
    /// persisted separately per key. The map is temporarily moved out (no clone)
    /// and restored before returning, so `self` is left unchanged.
    ///
    /// The resulting blob deserializes back into a `WorldState` whose `accounts`
    /// map is empty; the caller repopulates it via [`WorldState::load_account`].
    pub fn serialize_meta(&mut self) -> Result<Vec<u8>, bincode::Error> {
        let accounts = std::mem::take(&mut self.accounts);
        let result = bincode::serialize(&*self);
        self.accounts = accounts;
        result
    }

    /// Drains and returns the set of account addresses modified since the last
    /// drain. Used by incremental persistence to write only changed rows.
    pub fn take_persist_dirty(&mut self) -> Vec<Address> {
        std::mem::take(&mut self.persist_dirty)
            .into_iter()
            .collect()
    }

    /// Marks every current account for persistence — used before a full save
    /// (genesis bootstrap, snapshot import) so the next flush writes the whole set.
    pub fn mark_all_persist_dirty(&mut self) {
        self.persist_dirty = self.accounts.keys().copied().collect();
    }

    /// Looks up an account by its address (persistence row source).
    pub fn account_by_addr(&self, addr: &Address) -> Option<&Account> {
        self.accounts.get(addr)
    }

    /// Iterates over all `(address, account)` pairs — full-snapshot persistence.
    pub fn accounts_iter(&self) -> impl Iterator<Item = (&Address, &Account)> {
        self.accounts.iter()
    }

    /// Inserts an account loaded from storage **without** marking it dirty.
    /// The Merkle tree and indexes are rebuilt lazily on the first state-root call.
    pub fn load_account(&mut self, account: Account) {
        self.accounts.insert(account.address, account);
    }

    /// Returns true when the chain must keep advancing even with an empty mempool.
    ///
    /// A periodic heartbeat block is only required when a protocol upgrade is
    /// scheduled (its activation is triggered by reaching a block height). Otherwise
    /// the node can sleep until the next transaction — no wasted storage.
    pub fn has_pending_time_sensitive_ops(&self) -> bool {
        self.pending_upgrade.is_some()
    }

    pub(crate) fn insert_account(&mut self, account: Account) {
        self.mark_dirty(&account.address);
        self.accounts.insert(account.address, account);
    }

    pub fn credit_for_test(&mut self, address: Address, amount: Amount) {
        self.mark_dirty(&address);
        let acc = self
            .accounts
            .entry(address)
            .or_insert_with(|| Account::new(address));
        acc.balance = acc.balance.saturating_add(amount);
    }

    /// Applies a transaction with full verification: chain-id/TTL replay checks
    /// **and** Ed25519 signature verification. Use this for transactions from any
    /// untrusted source (P2P gossip, chain sync, direct replay of a received block).
    pub fn apply_transaction(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        self.check_replay_and_ttl(tx)?;
        self.verify_tx_signatures(tx)?;
        self.dispatch_tx(tx)
    }

    /// Applies a transaction **without** re-verifying its Ed25519 signatures.
    ///
    /// The caller MUST guarantee the signatures were already verified — e.g. the
    /// transaction was drained from the mempool's verified `queues`, whose invariant
    /// is that every entry has a valid signature. Chain-id and TTL checks are still
    /// enforced because the chain height may have advanced since admission (a tx can
    /// expire between mempool entry and block inclusion).
    ///
    /// NEVER call this on transactions from an untrusted source (P2P, sync) — signature
    /// verification is the security boundary there. Use `apply_transaction` instead.
    pub fn apply_transaction_trusted(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        self.check_replay_and_ttl(tx)?;
        self.dispatch_tx(tx)
    }

    /// Cheap replay-protection guards: chain-id binding and height-based TTL.
    fn check_replay_and_ttl(&self, tx: &Transaction) -> Result<(), CoreError> {
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
        Ok(())
    }

    /// Verifies the sender's (and optional sponsor's) public-key binding and Ed25519
    /// signature. This is the expensive part of applying a transaction.
    fn verify_tx_signatures(&self, tx: &Transaction) -> Result<(), CoreError> {
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
        Ok(())
    }

    /// Dispatches a (pre-verified) transaction to its type-specific handler.
    fn dispatch_tx(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        match &tx.tx_type {
            TransactionType::Transfer => self.apply_transfer(tx),
            TransactionType::Stake => self.apply_stake(tx),
            TransactionType::Unstake => self.apply_unstake(tx),
            TransactionType::AnnounceUpgrade => self.apply_announce_upgrade(tx),
            TransactionType::AddValidator => self.apply_add_validator(tx),
            TransactionType::RemoveValidator => self.apply_remove_validator(tx),
            TransactionType::SlashValidator => self.apply_slash_validator(tx),
            TransactionType::AdminAction => self.apply_admin_action(tx),
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
            (tx.amount, *sponsor_addr)
        } else {
            let total = tx
                .amount
                .checked_add(tx.fee)
                .ok_or(CoreError::AmountOverflow)?;
            (total, tx.from)
        };

        {
            let sender = self
                .accounts
                .get_mut(&tx.from)
                .ok_or(CoreError::InsufficientBalance)?;
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
                .get_mut(&fee_payer)
                .ok_or(CoreError::InsufficientBalance)?;
            if sponsor_acc.balance < tx.fee {
                return Err(CoreError::InsufficientBalance);
            }
            sponsor_acc.balance = sponsor_acc.balance.checked_sub(tx.fee).unwrap();
        }

        let receiver = self
            .accounts
            .entry(tx.to)
            .or_insert_with(|| Account::new(tx.to));
        receiver.balance = receiver
            .balance
            .checked_add(tx.amount)
            .ok_or(CoreError::AmountOverflow)?;

        // Melt: 100% of the fee flows back into the Foundry and leaves circulation.
        // The supply is conserved — nothing is burned, it will be re-forged as rewards.
        self.melt_to_foundry(tx.fee);

        self.mark_dirty(&tx.from);
        self.mark_dirty(&tx.to);
        if fee_payer != tx.from {
            self.mark_dirty(&fee_payer);
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
            .get_mut(&tx.from)
            .ok_or(CoreError::InsufficientBalance)?;
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
        self.mark_dirty(&tx.from);
        Ok(())
    }

    fn apply_unstake(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let account = self
            .accounts
            .get_mut(&tx.from)
            .ok_or(CoreError::InsufficientBalance)?;
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
        self.mark_dirty(&tx.from);
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
            .get_mut(&tx.from)
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;
        self.mark_dirty(&tx.from);

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
            .get_mut(&tx.from)
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;
        self.validator_set.add(tx.to);
        self.mark_dirty(&tx.from);
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
            .get_mut(&tx.from)
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;
        self.validator_set.remove(&tx.to);
        self.mark_dirty(&tx.from);
        tracing::info!(validator = %tx.to, "Validator removed from set");
        Ok(())
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

    /// Forges staking rewards out of the Foundry every STAKING_DISTRIBUTION_INTERVAL
    /// blocks and distributes them to stakers proportionally to their stake.
    ///
    /// The reward pool is a fraction (`FORGE_RATE_BPS`) of the current Foundry, so it
    /// shrinks as the Foundry drains and grows again as fees melt back in — the
    /// Foundry never empties (the "infinite cycle"). Returns the amount forged.
    pub fn distribute_staking_rewards(&mut self) -> Amount {
        if self.block_height == 0
            || !self
                .block_height
                .is_multiple_of(STAKING_DISTRIBUTION_INTERVAL)
        {
            return Amount::ZERO;
        }
        if self.foundry == Amount::ZERO {
            return Amount::ZERO;
        }

        let total_staked: u128 = self.accounts.values().map(|a| a.staked.atoms()).sum();
        if total_staked == 0 {
            return Amount::ZERO;
        }

        // Forge a fraction of the Foundry as this epoch's reward pool.
        let pool = self.foundry.atoms() * FORGE_RATE_BPS / FORGE_RATE_DENOM;
        if pool == 0 {
            return Amount::ZERO;
        }
        let mut distributed = 0u128;

        let mut rewarded: Vec<Address> = Vec::new();
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
                rewarded.push(account.address);
            }
        }
        for addr in rewarded {
            self.mark_dirty(&addr);
        }

        // Move the forged tokens from the Foundry into circulation (accounting).
        self.forge_from_foundry(Amount::from_atoms(distributed))
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
            .get_mut(&tx.from)
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;

        // Slash: melt the validator's stake back into the Foundry (minus a bounty)
        let slashed = self
            .accounts
            .get(target)
            .map(|a| a.staked)
            .unwrap_or(Amount::ZERO);
        if slashed > Amount::ZERO {
            // 10% bounty to the reporter
            let bounty = Amount::from_atoms(slashed.atoms() / 10);
            // Remainder melts back into the Foundry (recycled, never burned).
            let to_melt = slashed.checked_sub(bounty).unwrap_or(Amount::ZERO);

            if let Some(acc) = self.accounts.get_mut(target) {
                acc.staked = Amount::ZERO;
                acc.stake_since = 0;
            }
            self.mark_dirty(target);
            self.credit(&tx.from, bounty); // credit() also calls mark_dirty(tx.from)
            self.melt_to_foundry(to_melt);
        }

        self.mark_dirty(&tx.from);

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
            .get_mut(&tx.from)
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
                    self.validator_set.add(addr);
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
            GovernanceAction::RotateAdmin(new_admin) => {
                self.admin_address = Some(new_admin);
                tracing::info!(%new_admin, "Admin: admin key rotated");
            }
        }

        self.mark_dirty(&tx.from);
        Ok(())
    }

    #[cfg(test)]
    fn set_staked_for_test(&mut self, address: &Address, staked: Amount) {
        let acc = self
            .accounts
            .entry(*address)
            .or_insert_with(|| vinx_core::Account::new(*address));
        acc.balance = Amount::ZERO;
        acc.staked = staked;
        acc.stake_since = 0;
    }
}

fn hash_account(account: &Account) -> Hash32 {
    let addr = account.address.as_bytes();
    let mut buf = Vec::with_capacity(addr.len() + 16 + 8 + 16 + 8);
    buf.extend_from_slice(addr);
    buf.extend_from_slice(&account.balance.atoms().to_be_bytes());
    buf.extend_from_slice(&account.nonce.to_be_bytes());
    buf.extend_from_slice(&account.staked.atoms().to_be_bytes());
    buf.extend_from_slice(&account.stake_since.to_be_bytes());
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

    // ─── Fonderie: melt / forge ──────────────────────────────────────────────

    // At FORGE_RATE_BPS = 10 (0.1%), a Foundry of 1_000_000 VINX forges 1_000 VINX.
    const FORGE_DIVISOR: u64 = 1_000; // 10_000 / FORGE_RATE_BPS

    #[test]
    fn test_no_stakers_foundry_unchanged() {
        let mut s = WorldState::new();
        s.foundry = Amount::from_vinx(1_000_000);
        s.block_height = 100;
        let forged = s.distribute_staking_rewards();
        assert_eq!(forged, Amount::ZERO);
        assert_eq!(s.foundry, Amount::from_vinx(1_000_000));
    }

    #[test]
    fn test_empty_foundry_forges_nothing() {
        let mut s = WorldState::new();
        let addr = staker(&mut s, Amount::from_vinx(1_000));
        s.foundry = Amount::ZERO;
        s.block_height = 100;
        assert_eq!(s.distribute_staking_rewards(), Amount::ZERO);
        assert_eq!(s.accounts[&addr].balance, Amount::ZERO);
    }

    #[test]
    fn test_single_staker_receives_forged_fraction() {
        let mut s = WorldState::new();
        let addr = staker(&mut s, Amount::from_vinx(1_000));
        s.foundry = Amount::from_vinx(1_000_000);
        s.block_height = 100;
        let forged = s.distribute_staking_rewards();
        let expected = Amount::from_vinx(1_000_000 / FORGE_DIVISOR); // 1_000 VINX
        assert_eq!(forged, expected);
        assert_eq!(s.accounts[&addr].balance, expected);
        assert_eq!(s.foundry, Amount::from_vinx(1_000_000 - 1_000));
    }

    #[test]
    fn test_two_stakers_forge_proportional() {
        let mut s = WorldState::new();
        let alice = staker(&mut s, Amount::from_vinx(1_000));
        let bob = staker(&mut s, Amount::from_vinx(3_000));
        s.foundry = Amount::from_vinx(400_000); // forges 400 VINX
        s.block_height = 100;
        s.distribute_staking_rewards();
        assert_eq!(s.accounts[&alice].balance, Amount::from_vinx(100));
        assert_eq!(s.accounts[&bob].balance, Amount::from_vinx(300));
    }

    #[test]
    fn test_no_forge_outside_interval() {
        let mut s = WorldState::new();
        let addr = staker(&mut s, Amount::from_vinx(1_000));
        s.foundry = Amount::from_vinx(1_000_000);
        s.block_height = 99;
        assert_eq!(s.distribute_staking_rewards(), Amount::ZERO);
        assert_eq!(s.accounts[&addr].balance, Amount::ZERO);
    }

    #[test]
    fn test_forge_conserves_supply() {
        let mut s = WorldState::new();
        let _a = staker(&mut s, Amount::from_vinx(1_000));
        s.foundry = Amount::from_vinx(1_000_000);
        s.circulating_supply = Amount::from_vinx(1_000); // the staked tokens
        let before = s.circulating_supply.atoms() + s.foundry.atoms();
        s.block_height = 100;
        let forged = s.distribute_staking_rewards();
        assert!(forged > Amount::ZERO);
        assert_eq!(s.circulating_supply.atoms() + s.foundry.atoms(), before);
    }

    #[test]
    fn test_fee_melts_into_foundry() {
        let mut s = WorldState::new();
        let sender_kp = KeyPair::generate();
        let sender = Address::from_public_key(&sender_kp.public_key());
        s.credit_for_test(sender.clone(), Amount::from_vinx(1_000));
        s.circulating_supply = Amount::from_vinx(1_000);
        s.foundry = Amount::from_vinx(99_000);
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(100);
        let fee = amount.calculate_fee(s.base_fee);
        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 0);
        let foundry_before = s.foundry;
        s.apply_transaction(&tx).unwrap();
        assert_eq!(s.foundry, foundry_before.saturating_add(fee));
        assert_eq!(
            s.circulating_supply.atoms() + s.foundry.atoms(),
            Amount::from_vinx(100_000).atoms()
        );
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
