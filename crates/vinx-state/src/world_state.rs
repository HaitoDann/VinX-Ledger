use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use vinx_core::{
    amount::{
        cumulative_emission_atoms, Amount, BPS_DENOM, DEFAULT_FEE_FLOOR_ATOMS, MIN_STAKE_ATOMS,
        MIN_VALIDATOR_BOND_ATOMS, SLASH_BOUNTY_BPS, SLASH_EQUIVOCATION_BPS, UNBONDING_SECS,
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
    /// La Fonderie — the emission reserve. At genesis it holds the entire supply; it
    /// drains *only* to reward block production (work emission) and grows only when a
    /// slashed bond melts back in. The invariant `circulating_supply + foundry ==
    /// MAX_SUPPLY` holds at every block: nothing is created or destroyed, only moved.
    #[serde(default)]
    pub foundry: Amount,
    /// Timestamp (unix seconds) of the first block — the emission epoch. Established
    /// lazily on the first block; `emission_started` guards initialization (so a
    /// genesis timestamp of 0 does not collide with an "unset" sentinel).
    #[serde(default)]
    pub emission_epoch_ts: u64,
    /// Whether the emission epoch has been established yet.
    #[serde(default)]
    emission_started: bool,
    /// Cumulative atoms already emitted as work rewards. Tracks the emission curve so
    /// each block forges exactly `curve(now) - emitted` and never double-emits.
    #[serde(default)]
    pub emitted_atoms: u128,
    /// Bonds in their unbonding delay: `(address, amount, unlock_ts)`. The amount left
    /// `staked` at unstake time and returns to the balance once `unlock_ts` is reached.
    /// It stays slashable until then. Part of circulation the whole time.
    #[serde(default)]
    pub pending_unbonds: Vec<PendingUnbond>,
    /// Timestamp of the block currently being applied — set before draining txs so
    /// `apply_unstake` can compute a real-time unlock. Not persisted.
    #[serde(skip)]
    current_block_ts: u64,
    /// Fees collected from the transactions of the block currently being applied.
    /// Credited in full to the block producer by `settle_block`. Not persisted
    /// (transient within a single block).
    #[serde(skip)]
    block_fees: Amount,
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

/// A bond amount in its unbonding delay, waiting to return to `address`'s balance
/// at `unlock_ts` (unix seconds). Slashable until it matures.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingUnbond {
    pub address: Address,
    pub amount: Amount,
    pub unlock_ts: u64,
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
            emission_epoch_ts: 0,
            emission_started: false,
            emitted_atoms: 0,
            pending_unbonds: Vec::new(),
            current_block_ts: 0,
            block_fees: Amount::ZERO,
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
        // Apply the congestion multiplier to the *governable* fee floor (not the
        // compile-time constant), so raising the floor also raises surge pricing.
        let floor = self.fee_floor.atoms();
        let new_atoms = floor * multiplier_bps / 10_000;
        self.base_fee = Amount::from_atoms(new_atoms.max(floor));
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

    /// Records the timestamp of the block currently being applied. Must be called
    /// before draining/applying that block's transactions so `apply_unstake` can
    /// compute a real-time unlock. Idempotent per block.
    pub fn set_block_context(&mut self, block_ts: u64) {
        self.current_block_ts = block_ts;
    }

    /// Settles block-level rewards to the producer and matures due unbonds. Call once
    /// per block, after applying all transactions and before `compute_state_root`, in
    /// every path that builds/replays a block (producer, P2P apply, sync). Returns
    /// `(fees, emission)` for logging.
    ///
    /// Fees stay in circulation (they move sender → producer). Emission is forged from
    /// the Foundry into circulation. Both are computed deterministically from the block
    /// (its transactions, its `producer`, its `block_ts`), so validators re-applying
    /// the block reach the identical state.
    pub fn settle_block(&mut self, producer: &Address, block_ts: u64) -> (Amount, Amount) {
        // 1. Collected transaction fees → producer (circulation-neutral).
        let fees = std::mem::replace(&mut self.block_fees, Amount::ZERO);
        if fees > Amount::ZERO {
            self.credit(producer, fees);
        }
        // 2. Mature any unbonds whose delay has elapsed (real time).
        self.mature_unbonds(block_ts);
        // 3. Work emission forged from the Foundry → producer.
        let emission = self.emit_work_reward(producer, block_ts);
        (fees, emission)
    }

    /// Checks the founding invariant `circulating_supply + foundry == MAX_SUPPLY`
    /// (ADR 0004). Cheap (two reads + one add). The block paths (producer, P2P apply,
    /// sync) treat `false` as a critical fault and reject/roll back the block rather
    /// than committing a corrupted supply — the enforcement point, since the
    /// state_root covers only accounts, not the Foundry.
    pub fn supply_invariant_holds(&self) -> bool {
        self.circulating_supply
            .atoms()
            .checked_add(self.foundry.atoms())
            == Some(vinx_core::amount::MAX_SUPPLY_ATOMS)
    }

    /// Returns matured bonds to their owners' balances. Circulation-neutral: the funds
    /// were already counted as circulating throughout the unbonding delay.
    fn mature_unbonds(&mut self, block_ts: u64) {
        if self.pending_unbonds.is_empty() {
            return;
        }
        let mut matured: Vec<(Address, Amount)> = Vec::new();
        self.pending_unbonds.retain(|u| {
            if u.unlock_ts <= block_ts {
                matured.push((u.address, u.amount));
                false
            } else {
                true
            }
        });
        for (addr, amount) in matured {
            self.credit(&addr, amount);
        }
    }

    /// Forges this block's work-emission reward out of the Foundry and credits it to
    /// the producer. Emission follows the discrete halving curve, integrated over real
    /// time since the emission epoch — so a quiet network doesn't stall or accelerate
    /// it. The first block establishes the epoch and emits nothing.
    fn emit_work_reward(&mut self, producer: &Address, block_ts: u64) -> Amount {
        if !self.emission_started {
            self.emission_started = true;
            self.emission_epoch_ts = block_ts;
            return Amount::ZERO;
        }
        let elapsed = block_ts.saturating_sub(self.emission_epoch_ts);
        let target = cumulative_emission_atoms(elapsed);
        let to_emit = target.saturating_sub(self.emitted_atoms);
        if to_emit == 0 {
            return Amount::ZERO;
        }
        let forged = self.forge_from_foundry(Amount::from_atoms(to_emit));
        if forged > Amount::ZERO {
            self.credit(producer, forged);
            self.emitted_atoms = self.emitted_atoms.saturating_add(forged.atoms());
        }
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
        // A scheduled upgrade (height-triggered) or a pending unbond (time-triggered)
        // both need the chain to keep advancing so their trigger can be reached.
        self.pending_upgrade.is_some() || !self.pending_unbonds.is_empty()
    }

    pub fn credit_for_test(&mut self, address: Address, amount: Amount) {
        self.mark_dirty(&address);
        let acc = self
            .accounts
            .entry(address)
            .or_insert_with(|| Account::new(address));
        acc.balance = acc.balance.saturating_add(amount);
    }

    /// Test helper that funds an account **while preserving the supply invariant**:
    /// it forges from the Foundry (like real emission) instead of conjuring balance.
    /// Prefer this over `credit_for_test` when the test then produces a block (which
    /// enforces the invariant in `settle_block`). Requires a genesis-funded Foundry.
    pub fn credit_from_foundry_for_test(&mut self, address: Address, amount: Amount) {
        let forged = self.forge_from_foundry(amount);
        self.credit(&address, forged);
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
        Self::verify_tx_signature_pure(tx)
    }

    /// Pure, state-independent signature verification for a single transaction.
    ///
    /// Depends only on the transaction's own bytes — it reads no `WorldState` — so it is
    /// safe to call from multiple threads at once (ADR 0015): verifying a block's
    /// signatures is embarrassingly parallel and, being a pure pass/fail conjunction, is
    /// fully deterministic regardless of the order in which the checks run. The parallel
    /// batch driver lives at the block-validation call sites in `vinx-node`, which then
    /// apply state sequentially via [`WorldState::apply_transaction_trusted`].
    pub fn verify_tx_signature_pure(tx: &Transaction) -> Result<(), CoreError> {
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

        // The fee stays in circulation: it is collected into the block's fee pool and
        // credited in full to the producer by `settle_block`. No melt — it just changes
        // hands, so `circulating_supply` is unchanged (sender −fee, producer +fee).
        self.block_fees = self.block_fees.saturating_add(tx.fee);

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
        // Bond posting: move balance → staked. No yield, no warm-up — the bond is
        // pure security collateral. Circulation-neutral (balance −a, staked +a).
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
        let bond_floor = Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS);
        let is_active_validator = self.validator_set.contains(&tx.from);
        let unlock_ts = self.current_block_ts.saturating_add(UNBONDING_SECS);

        // ADR 0009: cap concurrent unbonding entries per account (anti-spam on
        // `pending_unbonds`, a stronger bound than a negligible flat fee would be).
        let pending_for_sender = self
            .pending_unbonds
            .iter()
            .filter(|u| u.address == tx.from)
            .count();
        if pending_for_sender >= vinx_core::amount::MAX_PENDING_UNBONDS_PER_ACCOUNT {
            return Err(CoreError::InvalidTransaction(
                "too many pending unbonds — wait for one to mature".to_string(),
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
        if account.staked < tx.amount {
            return Err(CoreError::InsufficientBalance);
        }
        let remaining = account.staked.checked_sub(tx.amount).unwrap();
        // A properly bonded, active validator may not drop below the minimum bond
        // while in the set — it must be removed from the validator set first.
        // (A grandfathered genesis validator holding less than the bond is exempt.)
        if is_active_validator && account.staked >= bond_floor && remaining < bond_floor {
            return Err(CoreError::InvalidTransaction(
                "active validator cannot unstake below the minimum bond".to_string(),
            ));
        }
        account.staked = remaining;
        account.nonce += 1;
        // The withdrawn amount does NOT return to the balance now: it enters the
        // unbonding delay and stays slashable until `unlock_ts`. Circulation-neutral.
        self.pending_unbonds.push(PendingUnbond {
            address: tx.from,
            amount: tx.amount,
            unlock_ts,
        });
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

        let (new_version, activation_ts) = tx
            .decode_upgrade_payload()
            .ok_or_else(|| CoreError::UpgradeViolation("malformed upgrade payload".to_string()))?;

        // ADR 0006: the notice window is measured in real seconds against block
        // timestamps, not block heights (height is not a clock under adaptive cadence).
        let upgrade_type = self.current_version.upgrade_type(&new_version);
        let min_notice = upgrade_type.min_notice_secs();
        let announcement_ts = self.current_block_ts;

        if activation_ts < announcement_ts.saturating_add(min_notice) {
            return Err(CoreError::UpgradeViolation(format!(
                "{:?} upgrade requires {} s notice (announced at ts {}, requested activation at ts {})",
                upgrade_type, min_notice, announcement_ts, activation_ts
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
            activation_ts,
            announced_at: announcement_ts,
        });

        tracing::info!(
            version = %self.pending_upgrade.as_ref().unwrap().version,
            activation_ts,
            "Protocol upgrade scheduled"
        );

        Ok(())
    }

    // ADR 0007: `apply_add_validator` / `apply_remove_validator` (dedicated tx types
    // 0x05 / 0x06) were removed. Validator-set changes now go through the single
    // governance path — `apply_admin_action` → `GovernanceAction::AddValidator` /
    // `RemoveValidator` — which carries the same (now strict) validation.

    /// Checks whether a pending upgrade should activate at the current block time
    /// (ADR 0006) and applies the version change if so.
    pub fn check_upgrade_activation(&mut self) {
        let should_activate = self
            .pending_upgrade
            .as_ref()
            .map(|u| self.current_block_ts >= u.activation_ts)
            .unwrap_or(false);

        if should_activate {
            let upgrade = self.pending_upgrade.take().unwrap();
            let old = self.current_version.clone();
            self.current_version = upgrade.version.clone();
            tracing::info!(
                from = %old,
                to = %upgrade.version,
                ts = self.current_block_ts,
                "Protocol upgrade activated"
            );
        }
    }

    // Staking rewards no longer exist: validators are paid for *work* (block
    // production) via `emit_work_reward` + collected fees, not for holding a stake.
    // See `settle_block`.

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

        // 1. Same height, different blocks — the definition of equivocation.
        if evidence.header_a.height != evidence.header_b.height {
            return Err(CoreError::InvalidTransaction(
                "evidence headers are at different heights".to_string(),
            ));
        }
        let hash_a = evidence.header_a.hash();
        let hash_b = evidence.header_b.hash();
        if hash_a == hash_b {
            return Err(CoreError::InvalidTransaction(
                "evidence headers are identical — not equivocation".to_string(),
            ));
        }

        // 2. Both signatures must name the target validator, with matching pubkeys.
        if evidence.sig_a.validator != *target || evidence.sig_b.validator != *target {
            return Err(CoreError::InvalidTransaction(
                "evidence validator mismatch".to_string(),
            ));
        }
        if Address::from_public_key(&evidence.sig_a.pub_key) != *target
            || Address::from_public_key(&evidence.sig_b.pub_key) != *target
        {
            return Err(CoreError::InvalidTransaction(
                "evidence pubkey/address mismatch".to_string(),
            ));
        }

        // 3. THE crucial check: both signatures must be cryptographically valid over
        //    their respective block-header hashes. Only the target could have produced
        //    both — this is what makes a slash provable and forgery impossible.
        if evidence
            .sig_a
            .pub_key
            .verify(&hash_a, &evidence.sig_a.signature)
            .is_err()
            || evidence
                .sig_b
                .pub_key
                .verify(&hash_b, &evidence.sig_b.signature)
                .is_err()
        {
            return Err(CoreError::InvalidTransaction(
                "invalid equivocation signature".to_string(),
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

        // Slashable = current bond + any amounts still in the unbonding delay (the
        // whole reason for the delay is that exiting funds stay slashable).
        let staked = self
            .accounts
            .get(target)
            .map(|a| a.staked)
            .unwrap_or(Amount::ZERO);
        let unbonding: u128 = self
            .pending_unbonds
            .iter()
            .filter(|u| &u.address == target)
            .map(|u| u.amount.atoms())
            .sum();
        let slashable = staked.atoms().saturating_add(unbonding);

        if slashable > 0 {
            if let Some(acc) = self.accounts.get_mut(target) {
                acc.staked = Amount::ZERO;
            }
            self.pending_unbonds.retain(|u| &u.address != target);

            // slashed = slashable × equivocation rate (100%); any remainder returns.
            let slashed = slashable * SLASH_EQUIVOCATION_BPS / BPS_DENOM;
            let returned = slashable.saturating_sub(slashed);
            let bounty = slashed * SLASH_BOUNTY_BPS / BPS_DENOM;
            let to_melt = slashed.saturating_sub(bounty);

            self.mark_dirty(target);
            if returned > 0 {
                self.credit(target, Amount::from_atoms(returned));
            }
            self.credit(&tx.from, Amount::from_atoms(bounty)); // reporter bounty
            self.melt_to_foundry(Amount::from_atoms(to_melt)); // rest → Foundry
        }

        self.mark_dirty(&tx.from);

        // Remove from validator set (can't produce blocks anymore).
        if self.validator_set.len() > 1 {
            self.validator_set.remove(target);
            tracing::warn!(validator = %target, slashed = slashable, "Validator slashed for equivocation");
        } else {
            tracing::warn!(validator = %target, "Slash accounting applied — last validator kept in set");
        }

        Ok(())
    }

    fn apply_admin_action(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        // Require sender to be the admin
        self.check_admin(tx)?;

        let action: GovernanceAction = bincode::deserialize(&tx.payload).map_err(|_| {
            CoreError::InvalidTransaction("malformed governance action payload".to_string())
        })?;

        // ADR 0007: check the nonce but do NOT consume it yet — a governance action
        // that fails validation (e.g. duplicate validator, missing bond) must leave the
        // nonce untouched, matching the old dedicated handlers. The nonce is only bumped
        // once the action has been applied successfully.
        let cur_nonce = self
            .accounts
            .get(&tx.from)
            .ok_or(CoreError::InsufficientBalance)?
            .nonce;
        if cur_nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: cur_nonce,
                got: tx.nonce,
            });
        }

        match action {
            GovernanceAction::AddValidator(addr) => {
                // ADR 0007: single, strict validation path (was previously duplicated in
                // the dedicated 0x05 handler). Reject a duplicate rather than ignoring it.
                if self.validator_set.contains(&addr) {
                    return Err(CoreError::InvalidTransaction(
                        "address is already a validator".to_string(),
                    ));
                }
                // Skin in the game: a new validator must have posted the minimum bond.
                // (The genesis validator enters via create_genesis_state, so it is
                // grandfathered and exempt.)
                if self.account_staked(&addr).atoms() < MIN_VALIDATOR_BOND_ATOMS {
                    return Err(CoreError::InvalidTransaction(
                        "candidate validator has not posted the minimum bond".to_string(),
                    ));
                }
                self.validator_set.add(addr);
                tracing::info!(%addr, "Admin: validator added");
            }
            GovernanceAction::RemoveValidator(addr) => {
                if self.validator_set.len() <= 1 {
                    return Err(CoreError::InvalidTransaction(
                        "cannot remove the last validator".to_string(),
                    ));
                }
                if !self.validator_set.contains(&addr) {
                    return Err(CoreError::InvalidTransaction(
                        "address is not a validator".to_string(),
                    ));
                }
                self.validator_set.remove(&addr);
                tracing::info!(%addr, "Admin: validator removed");
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
                activation_ts,
            } => {
                // ADR 0006: activation is a wall-clock timestamp, announced at the
                // current block time.
                if self.pending_upgrade.is_none() {
                    self.pending_upgrade = Some(vinx_core::ScheduledUpgrade {
                        version: version.clone(),
                        activation_ts,
                        announced_at: self.current_block_ts,
                    });
                    tracing::info!(activation_ts, "Admin: upgrade scheduled");
                }
            }
            GovernanceAction::RotateAdmin(new_admin) => {
                self.admin_address = Some(new_admin);
                tracing::info!(%new_admin, "Admin: admin key rotated");
            }
        }

        // Success: consume the nonce.
        self.accounts
            .get_mut(&tx.from)
            .expect("sender existence checked above")
            .nonce += 1;
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
    }
}

fn hash_account(account: &Account) -> Hash32 {
    let addr = account.address.as_bytes();
    let mut buf = Vec::with_capacity(addr.len() + 16 + 8 + 16);
    buf.extend_from_slice(addr);
    buf.extend_from_slice(&account.balance.atoms().to_be_bytes());
    buf.extend_from_slice(&account.nonce.to_be_bytes());
    buf.extend_from_slice(&account.staked.atoms().to_be_bytes());
    sha256(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::{protocol::ProtocolVersion, Transaction};
    use vinx_crypto::{Address, KeyPair};

    fn kp_addr() -> (KeyPair, Address) {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        (kp, addr)
    }

    fn admin_state() -> (WorldState, KeyPair, Address) {
        let admin_kp = KeyPair::generate();
        let admin_addr = Address::from_public_key(&admin_kp.public_key());
        let mut state = WorldState::new();
        state.admin_address = Some(admin_addr.clone());
        state.credit_for_test(admin_addr.clone(), Amount::from_vinx(1_000));
        (state, admin_kp, admin_addr)
    }

    // ─── Fair launch: emission, fees, bond, unbonding, slashing ──────────────
    use vinx_core::amount::{cumulative_emission_atoms, HALVING_PERIOD_SECS, MAX_SUPPLY_ATOMS};
    use vinx_core::block::GENESIS_PREV_HASH;
    use vinx_core::{BlockHeader, BlockSignature, SlashEvidence};

    #[test]
    fn test_supply_invariant_detects_corruption() {
        let mut s = WorldState::new();
        s.foundry = Amount::from_atoms(MAX_SUPPLY_ATOMS);
        assert!(s.supply_invariant_holds());
        // Corrupt the reserve → the invariant must report a violation.
        s.foundry = Amount::from_atoms(MAX_SUPPLY_ATOMS - 1);
        assert!(!s.supply_invariant_holds());
    }

    #[test]
    fn test_first_block_sets_emission_epoch_and_emits_nothing() {
        let mut s = WorldState::new();
        s.foundry = Amount::from_atoms(MAX_SUPPLY_ATOMS);
        let (_, producer) = kp_addr();
        let (fees, emission) = s.settle_block(&producer, 1_000);
        assert_eq!(fees, Amount::ZERO);
        assert_eq!(emission, Amount::ZERO);
        assert_eq!(s.emission_epoch_ts, 1_000);
        assert_eq!(s.foundry.atoms(), MAX_SUPPLY_ATOMS);
    }

    #[test]
    fn test_emission_rewards_producer_and_conserves_supply() {
        let mut s = WorldState::new();
        s.foundry = Amount::from_atoms(MAX_SUPPLY_ATOMS);
        let (_, producer) = kp_addr();
        s.settle_block(&producer, 0); // establish epoch at t=0
                                      // One full era later: half the supply is emitted to the producer.
        let (_, emission) = s.settle_block(&producer, HALVING_PERIOD_SECS);
        let expected = cumulative_emission_atoms(HALVING_PERIOD_SECS);
        assert_eq!(emission.atoms(), expected);
        assert_eq!(s.accounts[&producer].balance.atoms(), expected);
        assert_eq!(s.foundry.atoms(), MAX_SUPPLY_ATOMS - expected);
        assert_eq!(
            s.circulating_supply.atoms() + s.foundry.atoms(),
            MAX_SUPPLY_ATOMS
        );
    }

    #[test]
    fn test_emission_is_not_weighted_by_bond() {
        // A producer with zero stake still earns the full block emission.
        let mut s = WorldState::new();
        s.foundry = Amount::from_atoms(MAX_SUPPLY_ATOMS);
        let (_, producer) = kp_addr();
        s.settle_block(&producer, 0);
        let (_, emission) = s.settle_block(&producer, HALVING_PERIOD_SECS / 8); // ~1 year
        assert!(emission > Amount::ZERO);
        assert_eq!(s.accounts[&producer].balance, emission);
    }

    #[test]
    fn test_fee_goes_to_producer_not_foundry() {
        let mut s = WorldState::new();
        let (sender_kp, sender) = kp_addr();
        let (_, receiver) = kp_addr();
        let (_, producer) = kp_addr();
        s.credit_for_test(sender, Amount::from_vinx(1_000));
        s.circulating_supply = Amount::from_vinx(1_000);
        let amount = Amount::from_vinx(100);
        let fee = amount.calculate_fee(s.base_fee);
        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 0);
        let foundry_before = s.foundry;
        s.apply_transaction(&tx).unwrap();
        // Fee is collected, not melted — the Foundry does not change.
        assert_eq!(s.foundry, foundry_before);
        // Settling credits the producer with the fee (epoch established, no emission).
        s.settle_block(&producer, 100);
        assert_eq!(s.accounts[&producer].balance, fee);
        // The fee just changed hands: circulation is unchanged.
        assert_eq!(s.circulating_supply, Amount::from_vinx(1_000));
    }

    #[test]
    fn test_add_validator_requires_bond() {
        // ADR 0007: validator admission goes through the unified AdminAction path.
        use vinx_core::GovernanceAction;
        let (mut state, admin_kp, _) = admin_state();
        let (_, candidate) = kp_addr();
        let add = |nonce| {
            Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::AddValidator(candidate),
                nonce,
            )
        };
        // No bond → rejected.
        assert!(state.apply_transaction(&add(0)).is_err());
        // Post the minimum bond, then admission succeeds.
        state.set_staked_for_test(&candidate, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
        state.apply_transaction(&add(0)).unwrap();
        assert!(state.validator_set.contains(&candidate));
        // Adding the same validator again is now a hard error (strict semantics).
        assert!(state.apply_transaction(&add(1)).is_err());
    }

    #[test]
    fn test_unstake_enters_unbonding_then_matures() {
        let mut s = WorldState::new();
        let (kp, addr) = kp_addr();
        s.credit_for_test(addr, Amount::from_vinx(1_000));
        s.apply_transaction(&Transaction::new_stake(
            &kp,
            Amount::from_vinx(500),
            Amount::ZERO,
            0,
        ))
        .unwrap();
        assert_eq!(s.accounts[&addr].staked, Amount::from_vinx(500));

        // Unstake at ts = 1000 → enters the unbonding delay, NOT credited yet.
        s.set_block_context(1_000);
        s.apply_transaction(&Transaction::new_unstake(
            &kp,
            Amount::from_vinx(500),
            Amount::ZERO,
            1,
        ))
        .unwrap();
        assert_eq!(s.accounts[&addr].staked, Amount::ZERO);
        assert_eq!(s.accounts[&addr].balance, Amount::from_vinx(500)); // still not back
        assert_eq!(s.pending_unbonds.len(), 1);

        // Just before unlock: nothing matures.
        s.settle_block(&addr, 1_000 + UNBONDING_SECS - 1);
        assert_eq!(s.pending_unbonds.len(), 1);
        // At unlock: the bond returns to the balance.
        s.settle_block(&addr, 1_000 + UNBONDING_SECS);
        assert!(s.pending_unbonds.is_empty());
        assert_eq!(s.accounts[&addr].balance, Amount::from_vinx(1_000));
    }

    #[test]
    fn test_unstake_pending_cap_enforced() {
        use vinx_core::amount::MAX_PENDING_UNBONDS_PER_ACCOUNT;
        let mut s = WorldState::new();
        let (kp, addr) = kp_addr();
        s.credit_for_test(addr, Amount::from_vinx(1_000));
        s.apply_transaction(&Transaction::new_stake(
            &kp,
            Amount::from_vinx(100),
            Amount::ZERO,
            0,
        ))
        .unwrap();
        s.set_block_context(1_000);
        // Unstake 1 VINX up to the cap — all accepted.
        for i in 0..MAX_PENDING_UNBONDS_PER_ACCOUNT {
            let tx =
                Transaction::new_unstake(&kp, Amount::from_vinx(1), Amount::ZERO, (i + 1) as u64);
            s.apply_transaction(&tx).unwrap();
        }
        assert_eq!(s.pending_unbonds.len(), MAX_PENDING_UNBONDS_PER_ACCOUNT);
        // One more → rejected (anti-spam).
        let over = Transaction::new_unstake(
            &kp,
            Amount::from_vinx(1),
            Amount::ZERO,
            (MAX_PENDING_UNBONDS_PER_ACCOUNT + 1) as u64,
        );
        assert!(s.apply_transaction(&over).is_err());
    }

    #[test]
    fn test_active_validator_cannot_unstake_below_bond() {
        let mut s = WorldState::new();
        let (kp, addr) = kp_addr();
        let bond = MIN_VALIDATOR_BOND_ATOMS;
        s.credit_for_test(addr, Amount::from_atoms(bond * 2));
        s.apply_transaction(&Transaction::new_stake(
            &kp,
            Amount::from_atoms(bond),
            Amount::ZERO,
            0,
        ))
        .unwrap();
        s.validator_set = ValidatorSet::single(addr);
        s.set_block_context(1_000);
        // Unstaking any of the bond would drop below the minimum → rejected.
        let bad = Transaction::new_unstake(&kp, Amount::from_atoms(bond / 2), Amount::ZERO, 1);
        assert!(s.apply_transaction(&bad).is_err());
    }

    // Builds a signed header at `height` with a distinguishing `state_root`.
    fn signed_header(
        kp: &KeyPair,
        validator: Address,
        height: u64,
        tag: u8,
    ) -> (BlockHeader, BlockSignature) {
        let header = BlockHeader {
            height,
            prev_hash: GENESIS_PREV_HASH,
            timestamp: 0,
            validator,
            tx_count: 0,
            state_root: [tag; 32],
            base_fee: 0,
            receipts_root: [0u8; 32],
        };
        let sig = BlockSignature {
            validator,
            pub_key: kp.public_key(),
            signature: kp.sign(&header.hash()),
        };
        (header, sig)
    }

    #[test]
    fn test_slash_rejects_forged_evidence() {
        // An attacker who does not hold the victim's key cannot fabricate evidence:
        // the signatures won't verify against the victim's public key.
        let (reporter_kp, reporter) = kp_addr();
        let (victim_kp, victim) = kp_addr();
        let attacker_kp = KeyPair::generate();
        let mut s = WorldState::new();
        s.credit_for_test(reporter, Amount::from_vinx(10));
        s.set_staked_for_test(&victim, Amount::from_vinx(1_000));
        s.validator_set = ValidatorSet::new(vec![victim, reporter]);

        let (header_a, _) = signed_header(&victim_kp, victim, 5, 0xAA);
        let (header_b, _) = signed_header(&victim_kp, victim, 5, 0xBB);
        // Forged: claim the victim's pubkey but sign with the attacker's key.
        let forge = |h: &BlockHeader| BlockSignature {
            validator: victim,
            pub_key: victim_kp.public_key(),
            signature: attacker_kp.sign(&h.hash()),
        };
        let evidence = SlashEvidence {
            header_a: header_a.clone(),
            header_b: header_b.clone(),
            sig_a: forge(&header_a),
            sig_b: forge(&header_b),
        };
        let tx = Transaction::new_slash_validator(&reporter_kp, victim, &evidence, 0);
        assert!(s.apply_transaction(&tx).is_err());
        // Victim keeps its bond and its seat.
        assert_eq!(s.accounts[&victim].staked, Amount::from_vinx(1_000));
        assert!(s.validator_set.contains(&victim));
    }

    #[test]
    fn test_slash_valid_equivocation_burns_bond() {
        let (reporter_kp, reporter) = kp_addr();
        let (victim_kp, victim) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(reporter, Amount::from_vinx(10));
        s.set_staked_for_test(&victim, Amount::from_vinx(1_000));
        s.validator_set = ValidatorSet::new(vec![victim, reporter]);

        // Two genuinely-signed, different headers at the same height = equivocation.
        let (header_a, sig_a) = signed_header(&victim_kp, victim, 5, 0xAA);
        let (header_b, sig_b) = signed_header(&victim_kp, victim, 5, 0xBB);
        let evidence = SlashEvidence {
            header_a,
            header_b,
            sig_a,
            sig_b,
        };
        let tx = Transaction::new_slash_validator(&reporter_kp, victim, &evidence, 0);
        s.apply_transaction(&tx).unwrap();

        // 100% of the bond slashed: victim loses everything and its seat.
        assert_eq!(s.accounts[&victim].staked, Amount::ZERO);
        assert!(!s.validator_set.contains(&victim));
        // 10% bounty to the reporter (100 VINX), 90% (900 VINX) melted to the Foundry.
        assert_eq!(s.accounts[&reporter].balance, Amount::from_vinx(110));
        assert_eq!(s.foundry, Amount::from_vinx(900));
    }

    // ─── protocol upgrades ───────────────────────────────────────────────────

    #[test]
    fn test_admin_can_schedule_patch_upgrade() {
        let (mut state, admin_kp, _) = admin_state();
        let notice = vinx_core::amount::UPGRADE_NOTICE_PATCH_SECS;
        let activation = state.current_block_ts + notice + 100;

        let tx = Transaction::new_announce_upgrade(
            &admin_kp,
            ProtocolVersion::new(1, 0, 1),
            activation,
            0,
        );
        state.apply_transaction(&tx).unwrap();

        let upgrade = state.pending_upgrade.as_ref().unwrap();
        assert_eq!(upgrade.version, ProtocolVersion::new(1, 0, 1));
        assert_eq!(upgrade.activation_ts, activation);
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
    fn test_upgrade_activates_at_correct_time() {
        let (mut state, admin_kp, _) = admin_state();
        let notice = vinx_core::amount::UPGRADE_NOTICE_PATCH_SECS;
        let activation = notice + 1;

        state
            .apply_transaction(&Transaction::new_announce_upgrade(
                &admin_kp,
                ProtocolVersion::new(1, 0, 1),
                activation,
                0,
            ))
            .unwrap();

        // Not yet activated (block time still before activation_ts)
        state.set_block_context(activation - 1);
        state.check_upgrade_activation();
        assert_eq!(state.current_version, ProtocolVersion::GENESIS);
        assert!(state.pending_upgrade.is_some());

        // Activates exactly at activation_ts
        state.set_block_context(activation);
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

        let notice = vinx_core::amount::UPGRADE_NOTICE_MAJOR_SECS;
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
