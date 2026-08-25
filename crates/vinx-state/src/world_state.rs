use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use vinx_core::{
    amount::{
        cumulative_emission_atoms, Amount, ACTIVE_SET_COOLDOWN_SECS, ACTIVE_SET_STEP,
        BOND_COOLDOWN_SECS, BOND_STEP_BPS, BPS_DENOM, DEFAULT_ACTIVE_SET_SIZE,
        DEFAULT_FEE_FLOOR_ATOMS, EPOCH_DURATION_SECS, EXISTENTIAL_DEPOSIT_ATOMS, MAX_BOND_HARD_CAP,
        MAX_MODULES, MAX_NONCE_AHEAD, MIN_ACTIVE_SET_SIZE, MIN_BOND_HARD_FLOOR,
        MIN_MODULE_BOND_ATOMS, MIN_STAKE_ATOMS, MIN_VALIDATOR_BOND_ATOMS, PROPOSER_SHARE_BPS,
        SLASH_BOUNTY_BPS, SLASH_EQUIVOCATION_BPS, UNBONDING_SECS, VALIDATOR_SCORE_WINDOW_SECS,
    },
    block::SlashEvidence,
    chain_id::CHAIN_ID_DEVNET,
    governance::GovernanceAction,
    module::ModuleOp,
    protocol::{ProtocolVersion, ScheduledUpgrade},
    reliability::{self, ReliabilityMap},
    validator_pool::PoolStatus,
    Account, CoreError, RegisterBlsKeyPayload, Transaction, TransactionType, ValidatorSet,
};
use vinx_crypto::{sha256, Address, BlsPubKey, BlsSignature, Hash32, IncrementalMerkleTree};

/// In-memory representation of the full chain state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldState {
    /// Accounts keyed by bech32 address. A `BTreeMap` (not `HashMap`) so iteration
    /// is already sorted by address — the Merkle leaf order — avoiding an O(n log n)
    /// sort on every state-root rebuild and every inclusion-proof lookup.
    pub(crate) accounts: BTreeMap<Address, Account>,
    /// Tokens in circulation (Σ balances + staked + pending unbonds). Together with
    /// `epoch_dist_emission_pot` and `destroyed_atoms` this always equals `emitted_atoms`.
    pub circulating_supply: Amount,
    pub block_height: u64,
    /// Dormant — kept for bincode backward-compatibility (v9 on-disk layout).
    /// Not used in any logic after ADR 0040. Always `Amount::ZERO` on new chains.
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) foundry: Amount,
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
    /// Optional K-of-M admin committee (ADR 0011). When set, governance actions require
    /// `threshold` approvals among `signers`, superseding the single `admin_address`. When
    /// `None`, `admin_address` is the sole authority (legacy 1-of-1). `serde(default)` so
    /// pre-0011 state (bincode meta / JSON snapshot) loads with no committee.
    ///
    /// Declared after the `serde(skip)` fields so it is the last *serialized* field: a
    /// pre-0011 meta blob is a strict prefix of a current one, which the v7→v8 storage
    /// migration exploits by appending this field's default encoding.
    #[serde(default)]
    pub admin_policy: Option<AdminPolicy>,
    /// Governance proposals awaiting enough committee approvals to execute (ADR 0011).
    /// Empty under a single admin (actions execute immediately). Consensus meta, like
    /// `pending_unbonds`: derived deterministically from the same transaction history.
    #[serde(default)]
    pub pending_governance: Vec<GovernanceProposal>,
    /// Bonded module registry (ADR 0010): `module_id → ModuleEntry`. The L1 stores only the
    /// operator, its bond, and the latest committed anchor — never module logic. Appended
    /// after `pending_governance` so the v8→v9 storage migration can append its default
    /// (empty map). `serde(default)` for pre-0010 state.
    #[serde(default)]
    pub modules: BTreeMap<Hash32, ModuleEntry>,
    /// Slash proceeds awaiting distribution to honest validators (ADR 0040).
    /// 90 % of every slashed bond flows here; distributed at epoch close (ADR 0028).
    /// Appended after `modules` — the v9→v10 migration appends its default encoding.
    #[serde(default)]
    pub epoch_dist_emission_pot: Amount,
    /// Cumulative atoms permanently destroyed by reaping dust (ADR 0026 + ADR 0040).
    /// The only source of destruction on VinX — amounts are ≤ 0.001 VINX per account.
    /// Appended after `epoch_dist_emission_pot` — same append-only migration strategy.
    #[serde(default)]
    pub destroyed_atoms: u128,
    /// Fiabilité des validateurs (ADR 0027) : manquements de proposition + jailing, dérivés
    /// **déterministiquement** de la séquence de blocs (comme `pending_unbonds`, hors
    /// `state_root`). `serde(default)` pour l'état pré-0027.
    #[serde(default)]
    pub reliability: ReliabilityMap,
    // ── Open PoA (ADR 0038) ─────────────────────────────────────────────────────
    // All fields below are appended after `reliability` — the v11→v12 migration
    // appends their default encodings (empty map / empty set / default value).
    // The bincode prefix-append property requires these fields to stay LAST and
    // be individually serde(default)-gated so pre-v12 blobs load cleanly.
    //
    /// Pool of all bonded validators (ADR 0038). Keyed by validator signing address.
    /// Includes active, benched, warming-up, and unbonding entries.
    /// Appended after `reliability` — v11→v12 migration appends its default (empty).
    #[serde(default)]
    pub validator_pool: BTreeMap<Address, vinx_core::ValidatorPoolEntry>,
    /// Validator keys permanently banned after a proven equivocation (ADR 0038).
    /// `AddValidator`/bond transactions referencing a banned key are rejected.
    /// Appended after `validator_pool`.
    #[serde(default)]
    pub banned_validator_keys: std::collections::HashSet<Address>,
    /// Current governable active-set size N (ADR 0038). Default: DEFAULT_ACTIVE_SET_SIZE.
    /// Governable within [MIN_ACTIVE_SET_SIZE, MAX_ACTIVE_SET_SIZE] in steps of
    /// ACTIVE_SET_STEP with ACTIVE_SET_COOLDOWN_SECS between modifications.
    /// Appended after `banned_validator_keys`.
    #[serde(default = "default_active_set_size")]
    pub active_set_size: u32,
    /// Timestamp of the last governance modification to `active_set_size` (ADR 0038).
    /// Used to enforce the ACTIVE_SET_COOLDOWN_SECS between modifications.
    /// Appended after `active_set_size`.
    #[serde(default)]
    pub last_active_set_size_change_ts: u64,
    /// Timestamp of the last governance modification to `min_validator_bond` (ADR 0038).
    /// Used to enforce BOND_COOLDOWN_SECS between modifications.
    /// Appended after `last_active_set_size_change_ts`.
    #[serde(default)]
    pub last_bond_change_ts: u64,
    /// Timestamp of the most recent epoch close (ADR 0028 / ADR 0038).
    /// Epoch closes trigger score decay, active-set rotation, warmup ticks, and
    /// epoch-pot distribution. Zero until the first epoch close fires.
    /// Appended after `last_bond_change_ts` — v12→v13 migration appends `u64 = 0`.
    #[serde(default)]
    pub last_epoch_close_ts: u64,
    // ── ADR 0038 complement ─────────────────────────────────────────────────────
    /// Governable minimum bond to enter the validator pool (ADR 0038).
    /// Default: MIN_VALIDATOR_BOND_ATOMS (100 000 VinX). Adjustable within
    /// [MIN_BOND_HARD_FLOOR, MAX_BOND_HARD_CAP] in steps of ±BOND_STEP_BPS with
    /// BOND_COOLDOWN_SECS between modifications. Appended after `last_epoch_close_ts`
    /// — v15→v16 migration appends its default (u128 = MIN_VALIDATOR_BOND_ATOMS).
    #[serde(default = "default_min_validator_bond")]
    pub min_validator_bond_atoms: u128,
    // ── ADR 0029 Phase 2 — epoch beacon ─────────────────────────────────────────
    /// Epoch beacon for committee selection (ADR 0029 Phase 2, SHA-256 placeholder
    /// until ECVRF RFC 9381 is integrated). Updated at every epoch close.
    /// All-zeros until the first epoch close occurs.
    /// Appended after `min_validator_bond_atoms` — v16→v17 migration appends 32 zeros.
    #[serde(default)]
    pub epoch_beacon: Hash32,
}

/// A bond amount in its unbonding delay, waiting to return to `address`'s balance
/// at `unlock_ts` (unix seconds). Slashable until it matures.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingUnbond {
    pub address: Address,
    pub amount: Amount,
    pub unlock_ts: u64,
}

/// A K-of-M admin committee (ADR 0011). `threshold` signatures among the distinct
/// `signers` are required to enact any governance action.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AdminPolicy {
    pub signers: Vec<Address>,
    pub threshold: u16,
}

/// A governance action accumulating committee approvals until it reaches the threshold and
/// executes (ADR 0011). Identified by `action_hash = sha256(bincode(action))` so identical
/// actions proposed by different signers converge on the same tally.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GovernanceProposal {
    pub action_hash: Hash32,
    pub action: GovernanceAction,
    /// Distinct signer addresses that have approved, in first-seen order.
    pub approvals: Vec<Address>,
}

/// A registered module in the bonded anchor registry (ADR 0010). The L1 keeps only this
/// commitment — never the module's logic or full state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModuleEntry {
    /// Address that registered the module and alone may anchor or deregister it.
    pub operator: Address,
    /// Locked bond, returned to the operator on deregistration. Circulation-neutral while
    /// locked (the operator still owns it).
    pub bond: Amount,
    /// Latest committed state root of the off-chain module. All-zero until first anchored.
    pub anchor_head: Hash32,
    /// Number of successful anchors — a monotonic activity counter.
    pub anchored_count: u64,
}

/// Upper bound on committee size (ADR 0011) — bounds the signer set stored in state and the
/// work to authorize an action. Generous for a realistic governance council.
pub const MAX_ADMIN_SIGNERS: usize = 64;

/// Upper bound on concurrently pending governance proposals (ADR 0011) — bounds the state a
/// committee can accumulate from partially-approved actions.
pub const MAX_PENDING_GOVERNANCE: usize = 64;

/// Validates a proposed committee (ADR 0011): non-empty, no duplicate signers, size within
/// [`MAX_ADMIN_SIGNERS`], and `threshold` in `1..=signers.len()`.
fn validate_admin_policy(signers: Vec<Address>, threshold: u16) -> Result<AdminPolicy, CoreError> {
    if signers.is_empty() {
        return Err(CoreError::InvalidTransaction(
            "committee needs at least one signer".to_string(),
        ));
    }
    if signers.len() > MAX_ADMIN_SIGNERS {
        return Err(CoreError::InvalidTransaction(
            "committee exceeds the maximum signer count".to_string(),
        ));
    }
    let mut sorted = signers.clone();
    sorted.sort();
    sorted.dedup();
    if sorted.len() != signers.len() {
        return Err(CoreError::InvalidTransaction(
            "duplicate signer in committee".to_string(),
        ));
    }
    if threshold == 0 || threshold as usize > signers.len() {
        return Err(CoreError::InvalidTransaction(
            "threshold must be between 1 and the number of signers".to_string(),
        ));
    }
    Ok(AdminPolicy { signers, threshold })
}

/// The bincode bytes appended to a pre-0011 (v7) `WorldState` meta blob to bring it to v8
/// (ADR 0011): the default `admin_policy` (`None`) followed by the default
/// `pending_governance` (empty vec). Because bincode concatenates struct fields with no
/// framing and these are the last two *serialized* fields, appending exactly these bytes to
/// a v7 blob yields a valid v8 blob. Shared by the storage v7→v8 migration and its tests.
pub fn v8_meta_suffix() -> Vec<u8> {
    let mut out = bincode::serialize(&None::<AdminPolicy>).expect("serialize None");
    out.extend(bincode::serialize(&Vec::<GovernanceProposal>::new()).expect("serialize empty vec"));
    out
}

/// The bincode bytes appended to a v8 `WorldState` meta blob to bring it to v9 (ADR 0010):
/// the default `modules` registry (empty map). Same append-only rationale as
/// [`v8_meta_suffix`] — `modules` is the last serialized field. Shared by the storage
/// v8→v9 migration and its tests.
pub fn v9_meta_suffix() -> Vec<u8> {
    bincode::serialize(&BTreeMap::<Hash32, ModuleEntry>::new()).expect("serialize empty map")
}

/// The bincode bytes appended to a v9 `WorldState` meta blob to bring it to v10
/// (ADR 0040): the default `epoch_dist_emission_pot` (Amount::ZERO) followed by the
/// default `destroyed_atoms` (0u128). Same append-only rationale as prior suffixes.
pub fn v10_meta_suffix() -> Vec<u8> {
    let mut out = bincode::serialize(&Amount::ZERO).expect("serialize Amount::ZERO");
    out.extend(bincode::serialize(&0u128).expect("serialize 0u128"));
    out
}

/// The bincode bytes appended to a v10 `WorldState` meta blob to bring it to v11 (ADR 0027):
/// the default `reliability` table (empty map). Same append-only rationale as
/// [`v9_meta_suffix`] — `reliability` is the last serialized field. Shared by the storage
/// v10→v11 migration and its tests.
pub fn v11_meta_suffix() -> Vec<u8> {
    bincode::serialize(&ReliabilityMap::new()).expect("serialize empty map")
}

/// The bincode bytes appended to a v11 `WorldState` meta blob to bring it to v12 (ADR 0038
/// Open PoA). Appends the defaults of five new fields in declaration order:
///   1. `validator_pool`                  — empty `BTreeMap<Address, ValidatorPoolEntry>`
///   2. `banned_validator_keys`           — empty `HashSet<Address>`
///   3. `active_set_size`                 — `u32 = DEFAULT_ACTIVE_SET_SIZE` (21)
///   4. `last_active_set_size_change_ts`  — `u64 = 0`
///   5. `last_bond_change_ts`             — `u64 = 0`
pub fn v12_meta_suffix() -> Vec<u8> {
    use std::collections::{BTreeMap, HashSet};
    use vinx_core::ValidatorPoolEntry;
    let mut out =
        bincode::serialize(&BTreeMap::<Address, ValidatorPoolEntry>::new())
            .expect("serialize empty pool");
    out.extend(
        bincode::serialize(&HashSet::<Address>::new()).expect("serialize empty ban set"),
    );
    out.extend(
        bincode::serialize(&DEFAULT_ACTIVE_SET_SIZE).expect("serialize active_set_size"),
    );
    out.extend(bincode::serialize(&0u64).expect("serialize 0u64"));
    out.extend(bincode::serialize(&0u64).expect("serialize 0u64"));
    out
}

/// The bincode bytes appended to a v12 `WorldState` meta blob to bring it to v13 (ADR 0028
/// epoch close). Appends the default for one new field:
///   1. `last_epoch_close_ts` — `u64 = 0`
pub fn v13_meta_suffix() -> Vec<u8> {
    bincode::serialize(&0u64).expect("serialize 0u64")
}

/// The bincode bytes appended to a v15 `WorldState` meta blob to bring it to v16 (ADR 0038
/// governable bond floor). Appends the default for one new field:
///   1. `min_validator_bond_atoms` — `u128 = MIN_VALIDATOR_BOND_ATOMS`
pub fn v16_meta_suffix() -> Vec<u8> {
    bincode::serialize(&MIN_VALIDATOR_BOND_ATOMS).expect("serialize u128")
}

/// The bincode bytes appended to a v16 `WorldState` meta blob to bring it to v17 (ADR 0029
/// Phase 2 epoch beacon). Appends the default for one new field:
///   1. `epoch_beacon` — `Hash32 = [0u8; 32]`
pub fn v17_meta_suffix() -> Vec<u8> {
    bincode::serialize(&[0u8; 32]).expect("serialize Hash32")
}

/// SHA-256(epoch_number_le || address) — deterministic sort key for tiebreaking
/// validators with identical reliability scores at epoch rotation (ADR 0038).
fn epoch_tiebreaker(epoch: u64, addr: &Address) -> [u8; 32] {
    let mut buf = [0u8; 8 + 20];
    buf[..8].copy_from_slice(&epoch.to_le_bytes());
    buf[8..].copy_from_slice(addr.as_bytes());
    sha256(&buf)
}

fn default_fee_floor() -> Amount {
    Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS)
}

fn default_chain_id() -> u32 {
    CHAIN_ID_DEVNET
}

fn default_active_set_size() -> u32 {
    DEFAULT_ACTIVE_SET_SIZE
}

fn default_min_validator_bond() -> u128 {
    MIN_VALIDATOR_BOND_ATOMS
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
            admin_policy: None,
            pending_governance: Vec::new(),
            modules: BTreeMap::new(),
            epoch_dist_emission_pot: Amount::ZERO,
            destroyed_atoms: 0,
            reliability: ReliabilityMap::new(),
            validator_pool: BTreeMap::new(),
            banned_validator_keys: std::collections::HashSet::new(),
            active_set_size: DEFAULT_ACTIVE_SET_SIZE,
            last_active_set_size_change_ts: 0,
            last_bond_change_ts: 0,
            last_epoch_close_ts: 0,
            min_validator_bond_atoms: MIN_VALIDATOR_BOND_ATOMS,
            epoch_beacon: [0u8; 32],
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

    /// Moves `amount` from circulation into the epoch distribution pot (ADR 0040).
    /// Used for slash redistribution: removes tokens from a validator's bond and parks
    /// them in the pot until the epoch closes and they are paid to honest validators.
    fn move_to_epoch_pot(&mut self, amount: Amount) {
        self.epoch_dist_emission_pot = self.epoch_dist_emission_pot.saturating_add(amount);
        self.circulating_supply = self
            .circulating_supply
            .checked_sub(amount)
            .unwrap_or(Amount::ZERO);
    }

    /// Mints `amount` atoms into existence and credits the emission target account.
    /// Updates both `emitted_atoms` (the cumulative curve position) and
    /// `circulating_supply` (the supply-level tracker).
    fn mint_emission(&mut self, recipient: &Address, amount: Amount) {
        if amount == Amount::ZERO {
            return;
        }
        self.emitted_atoms = self
            .emitted_atoms
            .saturating_add(amount.atoms())
            .min(vinx_core::amount::MAX_SUPPLY_ATOMS);
        self.circulating_supply = self.circulating_supply.saturating_add(amount);
        self.credit(recipient, amount);
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
    /// Fees stay in circulation (they move sender → producer). Emission is newly minted
    /// (progressive minting, ADR 0040). Both are computed deterministically from the
    /// block so validators re-applying it reach the identical state.
    pub fn settle_block(
        &mut self,
        producer: &Address,
        height: u64,
        block_ts: u64,
    ) -> (Amount, Amount) {
        // 1. Collected transaction fees → producer (circulation-neutral).
        let fees = std::mem::replace(&mut self.block_fees, Amount::ZERO);
        if fees > Amount::ZERO {
            self.credit(producer, fees);
        }
        // 2. Mature any unbonds whose delay has elapsed (real time).
        self.mature_unbonds(block_ts);
        // 3. Work emission — mint new tokens → producer (ADR 0040).
        let emission = self.emit_work_reward(producer, block_ts);
        // 4. Fiabilité des validateurs (ADR 0027) : attributions + jailing.
        if height > 0 {
            let jailed = reliability::on_block_applied(
                &mut self.reliability,
                &self.validator_set,
                height,
                producer,
            );
            if jailed {
                tracing::warn!(
                    height,
                    "ADR 0027 : validateur jailé (manquements de proposition consécutifs)"
                );
            }
        }
        // 5. Epoch close (ADR 0028/0038) — triggered when EPOCH_DURATION_SECS have elapsed
        //    since the last close. Deterministic on block_ts so all nodes close the same epoch.
        if EPOCH_DURATION_SECS > 0 && self.emission_started {
            let since_last = block_ts.saturating_sub(
                if self.last_epoch_close_ts == 0 {
                    self.emission_epoch_ts
                } else {
                    self.last_epoch_close_ts
                },
            );
            if since_last >= EPOCH_DURATION_SECS {
                self.tick_epoch_close();
            }
        }
        (fees, emission)
    }

    /// Moves `amount` out of the epoch pot back into validator balances.
    /// Inverse of `move_to_epoch_pot`: epoch_pot decreases, circulating increases.
    fn distribute_from_epoch_pot(&mut self, addr: &Address, amount: Amount) {
        if amount == Amount::ZERO {
            return;
        }
        self.epoch_dist_emission_pot = self
            .epoch_dist_emission_pot
            .checked_sub(amount)
            .unwrap_or(Amount::ZERO);
        self.circulating_supply = self.circulating_supply.saturating_add(amount);
        self.credit(addr, amount);
    }

    /// Records which validators co-signed a finalized block.
    ///
    /// Updates `cosign_count_in_window` and `eligible_blocks_in_window` for every
    /// validator currently in the Active status. Call once per finalized block,
    /// after the block's co-signature set is known.
    pub fn record_block_cosigns(&mut self, cosigner_addrs: &[Address]) {
        let cosigners: HashSet<Address> = cosigner_addrs.iter().copied().collect();
        let active_addrs: Vec<Address> = self
            .validator_pool
            .iter()
            .filter(|(_, e)| matches!(e.status, PoolStatus::Active))
            .map(|(a, _)| *a)
            .collect();
        for addr in active_addrs {
            if let Some(entry) = self.validator_pool.get_mut(&addr) {
                entry.record_block(true, cosigners.contains(&addr));
            }
        }
    }

    /// Closes the current epoch: decays score windows, ticks warmup, rotates the active
    /// set by score (SHA-256 tiebreaker), distributes the epoch pot, and updates
    /// `validator_set` to the new active set (ADR 0028 + ADR 0038).
    ///
    /// Called automatically from `settle_block` when `EPOCH_DURATION_SECS` have elapsed.
    pub fn tick_epoch_close(&mut self) {
        let epoch_number = if EPOCH_DURATION_SECS > 0 {
            self.last_epoch_close_ts / EPOCH_DURATION_SECS
        } else {
            0
        };

        // 1. Decay all pool window counters (sliding-window approximation).
        let window_epochs = VALIDATOR_SCORE_WINDOW_SECS / EPOCH_DURATION_SECS.max(1);
        for entry in self.validator_pool.values_mut() {
            entry.decay_window(window_epochs);
        }

        // 2. Tick warmup counters — validators completing warm-up become Benched.
        for entry in self.validator_pool.values_mut() {
            entry.tick_warmup();
        }

        // 3. Rank eligible validators by score, SHA-256 tiebreaker.
        let n = self.active_set_size as usize;
        let mut eligible: Vec<(Address, u32)> = self
            .validator_pool
            .iter()
            .filter(|(_, e)| e.is_eligible())
            .map(|(a, e)| (*a, e.score_bps()))
            .collect();

        eligible.sort_by(|(a1, s1), (a2, s2)| {
            s2.cmp(s1).then_with(|| {
                epoch_tiebreaker(epoch_number, a1).cmp(&epoch_tiebreaker(epoch_number, a2))
            })
        });

        let new_active: HashSet<Address> = eligible.iter().take(n).map(|(a, _)| *a).collect();

        // 4. Update pool statuses to match selection.
        for (addr, entry) in self.validator_pool.iter_mut() {
            if matches!(entry.status, PoolStatus::Active | PoolStatus::Benched) {
                entry.status = if new_active.contains(addr) {
                    PoolStatus::Active
                } else {
                    PoolStatus::Benched
                };
            }
        }

        // 5. Distribute epoch pot proportionally by co-signature participation (ADR 0028).
        // Weight = cosign_count_in_window; equal fallback for validators with zero count.
        if !new_active.is_empty() {
            let pot = self.epoch_dist_emission_pot;
            if pot > Amount::ZERO {
                // Sorted for deterministic remainder recipient.
                let mut sorted: Vec<Address> = new_active.iter().copied().collect();
                sorted.sort();

                let weights: Vec<(Address, u64)> = sorted
                    .iter()
                    .map(|a| {
                        let w = self
                            .validator_pool
                            .get(a)
                            .map(|e| e.cosign_count_in_window)
                            .unwrap_or(0);
                        (*a, w)
                    })
                    .collect();
                let total_weight: u64 = weights.iter().map(|(_, w)| *w).sum();

                if total_weight == 0 {
                    // No participation data yet — equal split.
                    let count = sorted.len() as u128;
                    let share_atoms = pot.atoms() / count;
                    let remainder_atoms = pot.atoms() % count;
                    for &addr in &sorted {
                        if share_atoms > 0 {
                            self.distribute_from_epoch_pot(&addr, Amount::from_atoms(share_atoms));
                        }
                    }
                    if remainder_atoms > 0 {
                        self.distribute_from_epoch_pot(
                            &sorted[0],
                            Amount::from_atoms(remainder_atoms),
                        );
                    }
                } else {
                    let total_w = total_weight as u128;
                    let mut distributed = 0u128;
                    for (i, (addr, weight)) in weights.iter().enumerate() {
                        let share_atoms = if i + 1 < weights.len() {
                            pot.atoms() * (*weight as u128) / total_w
                        } else {
                            // Last recipient gets the remainder to avoid rounding loss.
                            pot.atoms() - distributed
                        };
                        if share_atoms > 0 {
                            self.distribute_from_epoch_pot(
                                addr,
                                Amount::from_atoms(share_atoms),
                            );
                            distributed += share_atoms;
                        }
                    }
                }
            }
        }

        // 6. Rebuild validator_set from the new active pool (sorted for determinism).
        if !new_active.is_empty() {
            let mut new_vs_addrs: Vec<Address> = new_active.iter().copied().collect();
            new_vs_addrs.sort();
            self.validator_set = ValidatorSet::new(new_vs_addrs);
        }
        // If the pool is empty (e.g. genesis before any bonds), leave validator_set as-is.

        // 7. Advance the epoch beacon (ADR 0029 Phase 2): chain-of-hashes accumulator.
        //    beacon' = sha256(beacon || epoch_number_le64 || block_ts_le64)
        //    SHA-256 placeholder for ECVRF RFC 9381 (integrated in Phase 2b).
        {
            let mut buf = [0u8; 32 + 8 + 8];
            buf[..32].copy_from_slice(&self.epoch_beacon);
            buf[32..40].copy_from_slice(&epoch_number.to_le_bytes());
            buf[40..48].copy_from_slice(&self.current_block_ts.to_le_bytes());
            self.epoch_beacon = sha256(&buf);
        }

        self.last_epoch_close_ts = self.current_block_ts;
    }

    /// Returns up to `k` validator addresses selected deterministically for `height`
    /// using the current epoch beacon (ADR 0029 Phase 2).
    ///
    /// Candidates are pool members in Active or Benched status (eligible for committee
    /// duty). Each is ranked by `sha256(beacon || height_le64 || addr_bytes)` — the k
    /// with the lexicographically smallest hash win the slot.
    ///
    /// This is a SHA-256 pseudo-VRF placeholder for ECVRF RFC 9381 (Phase 2b).
    pub fn committee_for_height(&self, height: u64, k: usize) -> Vec<Address> {
        let mut candidates: Vec<(Address, [u8; 32])> = self
            .validator_pool
            .iter()
            .filter(|(_, e)| matches!(e.status, PoolStatus::Active | PoolStatus::Benched))
            .map(|(addr, _)| {
                let mut buf = [0u8; 32 + 8 + 20];
                buf[..32].copy_from_slice(&self.epoch_beacon);
                buf[32..40].copy_from_slice(&height.to_le_bytes());
                buf[40..60].copy_from_slice(addr.as_bytes());
                (*addr, sha256(&buf))
            })
            .collect();
        candidates.sort_by_key(|(_, h)| *h);
        candidates.into_iter().take(k).map(|(a, _)| a).collect()
    }

    /// Checks the supply invariant (ADR 0040):
    /// `circulating_supply + epoch_dist_emission_pot + destroyed_atoms == emitted_atoms`
    /// and `emitted_atoms ≤ MAX_SUPPLY_ATOMS`.
    ///
    /// Cheap (a handful of reads + additions). The block paths (producer, P2P apply,
    /// sync) treat `false` as a critical fault and reject/roll back the block rather
    /// than committing a corrupted supply.
    pub fn supply_invariant_holds(&self) -> bool {
        let lhs = self
            .circulating_supply
            .atoms()
            .checked_add(self.epoch_dist_emission_pot.atoms())
            .and_then(|v| v.checked_add(self.destroyed_atoms));
        lhs == Some(self.emitted_atoms)
            && self.emitted_atoms <= vinx_core::amount::MAX_SUPPLY_ATOMS
    }

    /// Supply not yet emitted (`MAX_SUPPLY − emitted_atoms`).
    pub fn remaining_supply(&self) -> u128 {
        vinx_core::amount::MAX_SUPPLY_ATOMS.saturating_sub(self.emitted_atoms)
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

    /// Mints this block's work-emission reward and credits it to the producer.
    /// Emission follows the exponential decay curve, integrated over real time since
    /// the emission epoch — so a quiet network doesn't stall or accelerate it.
    /// The first block establishes the epoch and emits nothing (ADR 0040).
    fn emit_work_reward(&mut self, producer: &Address, block_ts: u64) -> Amount {
        if !self.emission_started {
            self.emission_started = true;
            self.emission_epoch_ts = block_ts;
            return Amount::ZERO;
        }
        let elapsed = block_ts.saturating_sub(self.emission_epoch_ts);
        let target = cumulative_emission_atoms(elapsed);
        let to_emit = target
            .saturating_sub(self.emitted_atoms)
            .min(vinx_core::amount::MAX_SUPPLY_ATOMS.saturating_sub(self.emitted_atoms));
        if to_emit == 0 {
            return Amount::ZERO;
        }
        // ADR 0028: 20% directly to the proposer (circulating), 80% into the epoch pot.
        let proposer_atoms = to_emit * PROPOSER_SHARE_BPS / BPS_DENOM;
        let pot_atoms = to_emit - proposer_atoms;
        let minted = Amount::from_atoms(to_emit);
        // Mint proposer share into the producer's balance (circulating).
        if proposer_atoms > 0 {
            self.mint_emission(producer, Amount::from_atoms(proposer_atoms));
        }
        // Mint pot share directly into emitted_atoms + epoch_dist_emission_pot (never circulating).
        if pot_atoms > 0 {
            self.emitted_atoms = self
                .emitted_atoms
                .saturating_add(pot_atoms)
                .min(vinx_core::amount::MAX_SUPPLY_ATOMS);
            self.epoch_dist_emission_pot = self
                .epoch_dist_emission_pot
                .saturating_add(Amount::from_atoms(pot_atoms));
        }
        minted
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

    /// Reaps an account that holds nothing worth 60 permanent bytes of state (ADR 0026):
    /// zero balance, zero stake, and no bond still unbonding. Removes it from the map —
    /// freeing its Merkle leaf — and marks the address for **deletion** from the
    /// persistence store (not just the in-memory map, or it would resurrect on reload).
    /// No-op if the account still holds value, has funds unbonding, or does not exist.
    ///
    /// Any residual dust (balance > 0 but below the existential deposit) is destroyed —
    /// the only source of token destruction in VinX (ADR 0040 §2.4). The transfer
    /// rules already prevent dust from forming (send that would leave < ED is rejected),
    /// so in practice `balance == 0` here; the check is a belt-and-suspenders safeguard.
    fn reap_if_empty(&mut self, addr: &Address) {
        let reapable_balance = match self.accounts.get(addr) {
            Some(a) if a.staked == Amount::ZERO => a.balance,
            _ => return,
        };
        if self.pending_unbonds.iter().any(|u| &u.address == addr) {
            return; // funds still unbonding — the account must stay to receive them
        }
        // Funded accounts (balance ≥ ED) are healthy — never reap them.
        if reapable_balance.atoms() >= EXISTENTIAL_DEPOSIT_ATOMS {
            return;
        }
        // Destroy any residual dust (normally zero; belt-and-suspenders for ED invariant).
        if reapable_balance > Amount::ZERO {
            self.circulating_supply = self
                .circulating_supply
                .checked_sub(reapable_balance)
                .unwrap_or(Amount::ZERO);
            self.destroyed_atoms = self
                .destroyed_atoms
                .saturating_add(reapable_balance.atoms());
        }
        self.accounts.remove(addr);
        self.leaf_index.remove(addr);
        // The leaf set shrank: the incremental tree needs a full rebuild, and the row
        // must be erased from persistence (mark_dirty only handles upserts).
        self.needs_rebuild = true;
        self.persist_dirty.insert(*addr);
    }

    /// Debug/test invariant for the existential deposit (ADR 0026 §Modèle): every account
    /// in the map holds either `balance >= ED`, or `staked > 0` (a bonded account is never
    /// dust), and no account with `balance == 0 && staked == 0` lingers in the map.
    pub fn existential_invariant_holds(&self) -> bool {
        let ed = Amount::from_atoms(EXISTENTIAL_DEPOSIT_ATOMS);
        self.accounts.values().all(|a| {
            if a.staked > Amount::ZERO {
                return true;
            }
            // No stake: balance must be a "real" balance (>= ED), never zero (should have
            // been reaped) and never dust (should have been rejected at write time).
            a.balance >= ed
        })
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

    /// Stateful mempool-admission checks (anti-spam). The signature must already be
    /// verified by the caller; this validates everything an attacker could otherwise
    /// claim for free:
    ///
    /// - `chain_id` matches (no cross-network replay filling the mempool),
    /// - the fee meets the current `base_fee` for fee-bearing types (Transfer,
    ///   AnchorState — stake/unstake/slash are exempt per ADR 0009),
    /// - the sender account **exists** and its balance covers the transaction's
    ///   worst-case debit (`admission_cost_atoms`) — so the fee used for mempool
    ///   priority is actually funded, not just declared,
    /// - a sponsored transaction's sponsor exists and covers the fee,
    /// - the nonce is not consumed and not absurdly far ahead
    ///   ([`MAX_NONCE_AHEAD`]) — bounds per-account nonce-gap parking,
    /// - an unstake does not exceed the sender's staked amount.
    ///
    /// Deliberately **conservative**: it must never reject a transaction the apply
    /// path would accept. Anything admitted can still fail at inclusion (state moved
    /// on) — this is a cheap gate, not a simulation.
    pub fn admission_check(&self, tx: &Transaction) -> Result<(), CoreError> {
        if tx.chain_id != self.chain_id {
            return Err(CoreError::InvalidTransaction(format!(
                "chain_id {} does not match this network ({})",
                tx.chain_id, self.chain_id
            )));
        }

        // Fee floor for the fee-bearing types (mirrors apply_transfer / apply_anchor_state).
        if matches!(
            tx.tx_type,
            TransactionType::Transfer | TransactionType::AnchorState
        ) && tx.fee < self.base_fee
        {
            return Err(CoreError::InvalidTransaction(format!(
                "fee {} is below the current base fee {}",
                tx.fee, self.base_fee
            )));
        }

        let Some(account) = self.accounts.get(&tx.from) else {
            return Err(CoreError::InvalidTransaction(
                "sender account does not exist (zero balance)".to_string(),
            ));
        };

        if tx.nonce < account.nonce {
            return Err(CoreError::InvalidNonce {
                expected: account.nonce,
                got: tx.nonce,
            });
        }
        if tx.nonce - account.nonce >= MAX_NONCE_AHEAD {
            return Err(CoreError::InvalidTransaction(format!(
                "nonce {} is too far ahead of account nonce {}",
                tx.nonce, account.nonce
            )));
        }

        if account.balance.atoms() < tx.admission_cost_atoms() {
            return Err(CoreError::InsufficientBalance);
        }
        if tx.tx_type == TransactionType::Unstake && account.staked < tx.amount {
            return Err(CoreError::InvalidTransaction(
                "unstake amount exceeds staked balance".to_string(),
            ));
        }

        // Sponsored fee: the sponsor must exist and cover the fee it signed for.
        if let Some(ref sponsor) = tx.sponsor {
            let Some(sponsor_acc) = self.accounts.get(sponsor) else {
                return Err(CoreError::InvalidTransaction(
                    "sponsor account does not exist".to_string(),
                ));
            };
            if sponsor_acc.balance < tx.fee {
                return Err(CoreError::InsufficientBalance);
            }
        }

        Ok(())
    }

    /// Returns true when a time-triggered operation is waiting on the chain to
    /// advance: a scheduled upgrade or a maturing unbond.
    ///
    /// Since ADR 0038 the block producer heartbeats unconditionally (at least one
    /// block per `HEARTBEAT_INTERVAL_SECS`), so this is no longer what gates the
    /// heartbeat — it remains useful for monitoring and tests.
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

    /// Test helper that mints tokens into an account while preserving the supply
    /// invariant (`circulating + pot + destroyed = emitted ≤ MAX_SUPPLY`). Prefer
    /// this over `credit_for_test` when the test then produces a block (which checks
    /// the invariant). Simulates work emission without going through the timing logic.
    pub fn credit_emit_for_test(&mut self, address: Address, amount: Amount) {
        self.mint_emission(&address, amount);
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
            TransactionType::AnchorState => self.apply_anchor_state(tx),
            TransactionType::RegisterBlsKey => self.apply_register_bls_key(tx),
            TransactionType::Unjail => self.apply_unjail(tx),
        }
    }

    fn check_admin(&self, tx: &Transaction) -> Result<(), CoreError> {
        // ADR 0011: once a K-of-M committee is installed, the legacy single-admin shortcuts
        // (e.g. the dedicated AnnounceUpgrade tx) are disabled — a lone key must not bypass
        // the threshold. Such actions must go through the committee AdminAction path.
        if self.admin_policy.is_some() {
            return Err(CoreError::Unauthorized);
        }
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

        // ── ADR 0026: existential deposit. Validate every party's resulting balance
        // BEFORE mutating any state: the block producer skips a rejected tx *without*
        // rolling back partial mutations, so a late rejection here would corrupt the block.
        // A party may land at exactly 0 (it gets reaped below) or at >= ED — never in the
        // forbidden ]0, ED[ band, unless it is staked (a bonded account is never dust).
        {
            let ed = EXISTENTIAL_DEPOSIT_ATOMS;
            let mut deltas: HashMap<Address, i128> = HashMap::new();
            *deltas.entry(tx.from).or_default() -= sender_debit.atoms() as i128;
            if fee_payer != tx.from {
                *deltas.entry(fee_payer).or_default() -= tx.fee.atoms() as i128;
            }
            *deltas.entry(tx.to).or_default() += tx.amount.atoms() as i128;
            for (addr, delta) in deltas {
                let cur = self.accounts.get(&addr);
                let bal = cur.map(|a| a.balance.atoms()).unwrap_or(0) as i128;
                let staked = cur.map(|a| a.staked.atoms()).unwrap_or(0);
                let after = bal + delta;
                // after < 0 (overspend) and after == 0 (exact drain → reaped) are both fine
                // here: overspend is surfaced with the precise error by the mutation block
                // below, and a zero balance is allowed. Only ]0, ED[ on an unstaked account
                // is forbidden.
                if after > 0 && staked == 0 && (after as u128) < ed {
                    return Err(CoreError::BelowExistentialDeposit);
                }
            }
        }

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

        // ADR 0026: reap any party the transfer left at exactly zero (no balance, no
        // stake, no bond unbonding), returning its 60 bytes to the free state.
        self.reap_if_empty(&tx.from);
        if fee_payer != tx.from {
            self.reap_if_empty(&fee_payer);
        }
        self.reap_if_empty(&tx.to);

        Ok(())
    }

    fn apply_stake(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        if tx.amount.atoms() < MIN_STAKE_ATOMS {
            return Err(CoreError::InvalidTransaction(
                "stake amount below minimum 1 VINX".to_string(),
            ));
        }
        // Compute new staked total in a scoped borrow so we can modify self.validator_pool after.
        let new_staked = {
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
            // Bond posting: move balance → staked. Circulation-neutral (balance −a, staked +a).
            account.balance = account.balance.checked_sub(tx.amount).unwrap();
            account.staked = account
                .staked
                .checked_add(tx.amount)
                .ok_or(CoreError::AmountOverflow)?;
            account.nonce += 1;
            account.staked
        };
        self.mark_dirty(&tx.from);
        // ADR 0038: auto-enter the validator pool once the bond floor is met.
        let bond = new_staked.atoms();
        if bond >= self.min_validator_bond_atoms
            && !self.banned_validator_keys.contains(&tx.from)
        {
            if let Some(entry) = self.validator_pool.get_mut(&tx.from) {
                entry.bond_atoms = bond;
            } else {
                self.validator_pool.insert(
                    tx.from,
                    vinx_core::ValidatorPoolEntry::new(bond, self.current_block_ts),
                );
                tracing::info!(addr = %tx.from, bond, "ADR 0038: validator auto-entered pool");
            }
        }
        Ok(())
    }

    fn apply_unstake(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let bond_floor = Amount::from_atoms(self.min_validator_bond_atoms);
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

        let remaining = {
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
            remaining
        };
        // The withdrawn amount does NOT return to the balance now: it enters the
        // unbonding delay and stays slashable until `unlock_ts`. Circulation-neutral.
        self.pending_unbonds.push(PendingUnbond {
            address: tx.from,
            amount: tx.amount,
            unlock_ts,
        });
        self.mark_dirty(&tx.from);
        // ADR 0038: update pool entry when bond drops below the governable floor.
        let new_bond = remaining.atoms();
        if let Some(entry) = self.validator_pool.get_mut(&tx.from) {
            entry.bond_atoms = new_bond;
            if new_bond < self.min_validator_bond_atoms {
                entry.status = vinx_core::validator_pool::PoolStatus::Unbonding { unlock_ts };
                tracing::info!(addr = %tx.from, new_bond, "ADR 0038: bond below floor, validator moved to Unbonding");
            }
        }
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

        // 2. Both headers must name the target as proposer.
        if evidence.header_a.validator != *target || evidence.header_b.validator != *target {
            return Err(CoreError::InvalidTransaction(
                "evidence headers do not name target as proposer".to_string(),
            ));
        }

        // 3. THE crucial check: both BLS signatures must verify against the target's
        //    registered BLS key (ADR 0046). Only the target could have produced both —
        //    forging evidence requires forging a BLS signature over a real header.
        let bls_pk_bytes = self
            .validator_pool
            .get(target)
            .and_then(|e| e.bls_pub_key.as_deref())
            .and_then(|b| <[u8; 48]>::try_from(b).ok())
            .ok_or_else(|| {
                CoreError::InvalidTransaction(
                    "target has no registered BLS key — cannot verify equivocation".to_string(),
                )
            })?;
        let bls_pk = BlsPubKey::from_bytes(&bls_pk_bytes).map_err(|_| {
            CoreError::InvalidTransaction("target BLS public key is malformed".to_string())
        })?;
        let sig_a_arr: [u8; 96] = evidence.bls_sig_a.as_slice().try_into().map_err(|_| {
            CoreError::InvalidTransaction("bls_sig_a must be 96 bytes".to_string())
        })?;
        let sig_b_arr: [u8; 96] = evidence.bls_sig_b.as_slice().try_into().map_err(|_| {
            CoreError::InvalidTransaction("bls_sig_b must be 96 bytes".to_string())
        })?;
        if vinx_crypto::bls_verify(&bls_pk, &BlsSignature(sig_a_arr), &hash_a).is_err()
            || vinx_crypto::bls_verify(&bls_pk, &BlsSignature(sig_b_arr), &hash_b).is_err()
        {
            return Err(CoreError::InvalidTransaction(
                "BLS equivocation proof does not verify".to_string(),
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
            self.credit(&tx.from, Amount::from_atoms(bounty)); // reporter bounty (10%)
            self.move_to_epoch_pot(Amount::from_atoms(to_melt)); // 90% → honest validators
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
        // ADR 0011: authorize against the effective admin authority — the K-of-M committee
        // if one is set, else the legacy single admin key, else dev mode (open).
        let (signers, threshold) = self.effective_admin();
        if let Some(ref signers) = signers {
            if !signers.contains(&tx.from) {
                return Err(CoreError::Unauthorized);
            }
        }

        // ADR 0007: check the nonce but do NOT consume it yet — a governance action that
        // fails validation (e.g. duplicate validator, missing bond) must leave the nonce
        // untouched. It is only bumped once the action has been applied (or an approval
        // recorded) successfully.
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

        let action: GovernanceAction = bincode::deserialize(&tx.payload).map_err(|_| {
            CoreError::InvalidTransaction("malformed governance action payload".to_string())
        })?;

        // Single admin (or dev mode) executes immediately; a committee accumulates approvals
        // and executes on the one that reaches the threshold. Either path mutates state only
        // after full validation, so a rejected action consumes no nonce (the producer skips
        // a failed tx without rolling back — ADR 0026).
        if threshold <= 1 {
            self.execute_governance_action(action)?;
        } else {
            self.record_governance_approval(action, tx.from, threshold)?;
        }

        // Success: consume the nonce.
        self.accounts
            .get_mut(&tx.from)
            .expect("sender existence checked above")
            .nonce += 1;
        self.mark_dirty(&tx.from);
        Ok(())
    }

    /// The effective admin authority (ADR 0011): `(Some(signers), threshold)` under a
    /// committee or a single admin key; `(None, 1)` in dev mode (no admin restriction).
    fn effective_admin(&self) -> (Option<Vec<Address>>, u16) {
        if let Some(ref p) = self.admin_policy {
            (Some(p.signers.clone()), p.threshold)
        } else if let Some(admin) = self.admin_address {
            (Some(vec![admin]), 1)
        } else {
            (None, 1)
        }
    }

    /// Records `approver`'s approval of `action` (ADR 0011). When the approval reaches
    /// `threshold` the action is executed and its proposal cleared; otherwise the approval
    /// is tallied. Re-approval by the same signer is rejected. Mutates nothing on failure.
    fn record_governance_approval(
        &mut self,
        action: GovernanceAction,
        approver: Address,
        threshold: u16,
    ) -> Result<(), CoreError> {
        let action_hash = sha256(&bincode::serialize(&action).map_err(|_| {
            CoreError::InvalidTransaction("cannot serialize governance action".to_string())
        })?);
        let existing = self
            .pending_governance
            .iter()
            .position(|p| p.action_hash == action_hash);
        if let Some(i) = existing {
            if self.pending_governance[i].approvals.contains(&approver) {
                return Err(CoreError::InvalidTransaction(
                    "signer has already approved this action".to_string(),
                ));
            }
        }
        let prospective = existing
            .map(|i| self.pending_governance[i].approvals.len())
            .unwrap_or(0)
            + 1;

        if prospective as u16 >= threshold {
            // Threshold reached — execute (validates before mutating). On failure nothing
            // is recorded. `retain` (not index removal) is safe even if the action cleared
            // the queue itself (SetAdminPolicy).
            self.execute_governance_action(action)?;
            self.pending_governance
                .retain(|p| p.action_hash != action_hash);
        } else if let Some(i) = existing {
            self.pending_governance[i].approvals.push(approver);
        } else {
            if self.pending_governance.len() >= MAX_PENDING_GOVERNANCE {
                return Err(CoreError::InvalidTransaction(
                    "too many pending governance proposals".to_string(),
                ));
            }
            self.pending_governance.push(GovernanceProposal {
                action_hash,
                action,
                approvals: vec![approver],
            });
        }
        Ok(())
    }

    /// Executes a fully-authorized governance action (ADR 0007 semantics; ADR 0011 for
    /// `SetAdminPolicy`). Each arm validates before mutating so a rejected action leaves
    /// state untouched.
    fn execute_governance_action(&mut self, action: GovernanceAction) -> Result<(), CoreError> {
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
                if self.account_staked(&addr).atoms() < self.min_validator_bond_atoms {
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
            GovernanceAction::SetAdminPolicy { signers, threshold } => {
                // ADR 0011: install (or replace) the K-of-M committee. A changed committee
                // invalidates in-flight approvals — the eligible signer set just changed.
                let policy = validate_admin_policy(signers, threshold)?;
                self.admin_policy = Some(policy);
                self.pending_governance.clear();
                tracing::info!(threshold, "Admin: committee policy set");
            }
            GovernanceAction::UpdateActiveSetSize { new_size } => {
                // ADR 0038: governable active-set N — must move by exactly ±ACTIVE_SET_STEP,
                // stay ≥ MIN_ACTIVE_SET_SIZE, and observe ACTIVE_SET_COOLDOWN_SECS.
                let current = self.active_set_size;
                let diff = (new_size as i64 - current as i64).unsigned_abs() as u32;
                if diff == 0 || diff != ACTIVE_SET_STEP {
                    return Err(CoreError::InvalidTransaction(format!(
                        "active_set_size must change by exactly ±{ACTIVE_SET_STEP} (current {current}, requested {new_size})"
                    )));
                }
                if new_size < MIN_ACTIVE_SET_SIZE {
                    return Err(CoreError::InvalidTransaction(format!(
                        "active_set_size {new_size} is below the minimum {MIN_ACTIVE_SET_SIZE}"
                    )));
                }
                if self.last_active_set_size_change_ts > 0 {
                    let elapsed = self
                        .current_block_ts
                        .saturating_sub(self.last_active_set_size_change_ts);
                    if elapsed < ACTIVE_SET_COOLDOWN_SECS {
                        return Err(CoreError::InvalidTransaction(format!(
                            "active_set_size was changed {} s ago; cooldown is {} s",
                            elapsed, ACTIVE_SET_COOLDOWN_SECS
                        )));
                    }
                }
                self.active_set_size = new_size;
                self.last_active_set_size_change_ts = self.current_block_ts;
                tracing::info!(new_size, "Admin: active_set_size updated (ADR 0038)");
            }
            GovernanceAction::UpdateMinValidatorBond { atoms } => {
                // ADR 0038: governable bond floor — within hard bounds, move by at most
                // BOND_STEP_BPS of the current value, observe BOND_COOLDOWN_SECS.
                if atoms < MIN_BOND_HARD_FLOOR {
                    return Err(CoreError::InvalidTransaction(format!(
                        "bond floor {atoms} is below the hard minimum {MIN_BOND_HARD_FLOOR}"
                    )));
                }
                if atoms > MAX_BOND_HARD_CAP {
                    return Err(CoreError::InvalidTransaction(format!(
                        "bond floor {atoms} exceeds the hard cap {MAX_BOND_HARD_CAP}"
                    )));
                }
                let current = self.min_validator_bond_atoms;
                let max_delta = current * BOND_STEP_BPS / BPS_DENOM;
                let delta = if atoms > current {
                    atoms - current
                } else {
                    current - atoms
                };
                if delta > max_delta {
                    return Err(CoreError::InvalidTransaction(format!(
                        "bond change {delta} exceeds BOND_STEP_BPS ({BOND_STEP_BPS} bps) limit {max_delta}"
                    )));
                }
                if self.last_bond_change_ts > 0 {
                    let elapsed = self
                        .current_block_ts
                        .saturating_sub(self.last_bond_change_ts);
                    if elapsed < BOND_COOLDOWN_SECS {
                        return Err(CoreError::InvalidTransaction(format!(
                            "bond floor was changed {} s ago; cooldown is {} s",
                            elapsed, BOND_COOLDOWN_SECS
                        )));
                    }
                }
                self.min_validator_bond_atoms = atoms;
                self.last_bond_change_ts = self.current_block_ts;
                tracing::info!(atoms, "Admin: min_validator_bond updated (ADR 0038)");
            }
        }
        Ok(())
    }

    /// Applies an `AnchorState` transaction: a bonded module-registry operation (ADR 0010).
    /// The L1 records the operator, bond, and latest anchor — never module logic.
    ///
    /// Every op pays the base fee (anti-spam; credited to the producer like a transfer) and
    /// leaves the operator with a live account (`balance >= ED`) — module operators are
    /// never dust and never reaped. Each arm validates fully before mutating, so a rejected
    /// op consumes no nonce (the producer skips a failed tx without rollback, cf. ADR 0026).
    fn apply_anchor_state(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let op: ModuleOp = bincode::deserialize(&tx.payload).map_err(|_| {
            CoreError::InvalidTransaction("malformed module operation payload".to_string())
        })?;

        // Fee floor (same anti-spam forfait as a transfer).
        if tx.fee < self.base_fee {
            return Err(CoreError::InvalidTransaction(format!(
                "fee {} is below minimum {}",
                tx.fee, self.base_fee
            )));
        }
        let ed = EXISTENTIAL_DEPOSIT_ATOMS;

        let operator = tx.from;
        let (nonce, balance) = {
            let acc = self
                .accounts
                .get(&operator)
                .ok_or(CoreError::InsufficientBalance)?;
            (acc.nonce, acc.balance.atoms())
        };
        if nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: nonce,
                got: tx.nonce,
            });
        }

        // Compute the operator's balance debit for this op, validating fully before any
        // mutation. Register locks a bond on top of the fee; Anchor/Deregister pay the fee
        // only (Deregister also refunds the bond, applied after removal below).
        let fee = tx.fee.atoms();
        match &op {
            ModuleOp::Register {
                module_id,
                bond_atoms,
            } => {
                if *bond_atoms < MIN_MODULE_BOND_ATOMS {
                    return Err(CoreError::InvalidTransaction(
                        "module bond is below the minimum".to_string(),
                    ));
                }
                if self.modules.contains_key(module_id) {
                    return Err(CoreError::InvalidTransaction(
                        "module id is already registered".to_string(),
                    ));
                }
                if self.modules.len() >= MAX_MODULES {
                    return Err(CoreError::InvalidTransaction(
                        "module registry is full".to_string(),
                    ));
                }
                let debit = bond_atoms
                    .checked_add(fee)
                    .ok_or(CoreError::AmountOverflow)?;
                if balance < debit {
                    return Err(CoreError::InsufficientBalance);
                }
                // Operators keep a live account (>= ED): they must exist to anchor later.
                if balance - debit < ed {
                    return Err(CoreError::BelowExistentialDeposit);
                }
                let acc = self.accounts.get_mut(&operator).expect("checked above");
                acc.balance = Amount::from_atoms(balance - debit);
                acc.nonce += 1;
                self.modules.insert(
                    *module_id,
                    ModuleEntry {
                        operator,
                        bond: Amount::from_atoms(*bond_atoms),
                        anchor_head: [0u8; 32],
                        anchored_count: 0,
                    },
                );
            }
            ModuleOp::Anchor {
                module_id,
                anchor_head,
            } => {
                let entry = self
                    .modules
                    .get(module_id)
                    .ok_or_else(|| CoreError::InvalidTransaction("unknown module".to_string()))?;
                if entry.operator != operator {
                    return Err(CoreError::Unauthorized);
                }
                if balance < fee || balance - fee < ed {
                    return Err(CoreError::InsufficientBalance);
                }
                let new_head = *anchor_head;
                let acc = self.accounts.get_mut(&operator).expect("checked above");
                acc.balance = Amount::from_atoms(balance - fee);
                acc.nonce += 1;
                let entry = self.modules.get_mut(module_id).expect("checked above");
                entry.anchor_head = new_head;
                entry.anchored_count += 1;
            }
            ModuleOp::Deregister { module_id } => {
                let entry = self
                    .modules
                    .get(module_id)
                    .ok_or_else(|| CoreError::InvalidTransaction("unknown module".to_string()))?;
                if entry.operator != operator {
                    return Err(CoreError::Unauthorized);
                }
                if balance < fee {
                    return Err(CoreError::InsufficientBalance);
                }
                let refund = entry.bond.atoms();
                let acc = self.accounts.get_mut(&operator).expect("checked above");
                // Net: −fee +refunded bond. Refund keeps the account well above ED.
                acc.balance = Amount::from_atoms(balance - fee + refund);
                acc.nonce += 1;
                self.modules.remove(module_id);
            }
        }

        // The fee changes hands into the block pool → credited to the producer at settle.
        self.block_fees = self.block_fees.saturating_add(tx.fee);
        self.mark_dirty(&operator);
        Ok(())
    }

    /// Applies a `RegisterBlsKey` transaction (ADR 0046): stores a validator's BLS12-381
    /// public key and Proof-of-Possession in the validator pool after verifying both.
    ///
    /// Self-authorized — any bonded validator may call this for themselves; no admin key
    /// required. Validates the PoP cryptographically before writing, so the pool never holds
    /// an unverified BLS key. Like `apply_admin_action`, validation happens before mutation,
    /// so a rejected payload does not consume the sender's nonce.
    fn apply_register_bls_key(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let account = self
            .accounts
            .get(&tx.from)
            .ok_or(CoreError::InsufficientBalance)?;
        if account.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: account.nonce,
                got: tx.nonce,
            });
        }

        if !self.validator_pool.contains_key(&tx.from) {
            return Err(CoreError::InvalidTransaction(
                "only bonded validators may register a BLS key".to_string(),
            ));
        }

        let payload: RegisterBlsKeyPayload =
            bincode::deserialize(&tx.payload).map_err(|_| {
                CoreError::InvalidTransaction("malformed RegisterBlsKey payload".to_string())
            })?;

        if payload.bls_pub_key.len() != 48 {
            return Err(CoreError::InvalidTransaction(
                "BLS public key must be 48 bytes (G1 compressed)".to_string(),
            ));
        }
        if payload.bls_pop.len() != 96 {
            return Err(CoreError::InvalidTransaction(
                "BLS Proof-of-Possession must be 96 bytes (G2 compressed)".to_string(),
            ));
        }

        let pk_arr: [u8; 48] = payload.bls_pub_key.as_slice().try_into().unwrap();
        let pop_arr: [u8; 96] = payload.bls_pop.as_slice().try_into().unwrap();

        let bls_pub = BlsPubKey::from_bytes(&pk_arr).map_err(|_| {
            CoreError::InvalidTransaction("BLS public key is not a valid G1 point".to_string())
        })?;
        let bls_pop_sig = BlsSignature(pop_arr);

        bls_pub.verify_pop(&bls_pop_sig).map_err(|_| {
            CoreError::InvalidTransaction(
                "BLS Proof-of-Possession verification failed".to_string(),
            )
        })?;

        // Verification passed — mutate.
        let entry = self
            .validator_pool
            .get_mut(&tx.from)
            .expect("existence checked above");
        entry.bls_pub_key = Some(payload.bls_pub_key);
        entry.bls_pop = Some(payload.bls_pop);

        self.accounts
            .get_mut(&tx.from)
            .expect("existence checked above")
            .nonce += 1;
        self.mark_dirty(&tx.from);
        tracing::info!(validator = %tx.from, "ADR 0046: BLS key registered");
        Ok(())
    }

    fn apply_unjail(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let account = self
            .accounts
            .get(&tx.from)
            .ok_or(CoreError::InsufficientBalance)?;
        if account.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: account.nonce,
                got: tx.nonce,
            });
        }
        let height = self.block_height;
        if !reliability::try_unjail(&mut self.reliability, &tx.from, height) {
            return Err(CoreError::InvalidTransaction(
                "unjail failed: validator is not jailed or cooldown has not elapsed".to_string(),
            ));
        }
        self.accounts
            .get_mut(&tx.from)
            .expect("existence checked above")
            .nonce += 1;
        self.mark_dirty(&tx.from);
        tracing::info!(validator = %tx.from, height, "ADR 0027: validator unjailed");
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
    use vinx_core::amount::{cumulative_emission_atoms, EMISSION_T_HALF_SECS};
    use vinx_core::block::GENESIS_PREV_HASH;
    use vinx_core::{BlockHeader, SlashEvidence};

    #[test]
    fn test_supply_invariant_detects_corruption() {
        let mut s = WorldState::new();
        // Fresh state: nothing emitted yet — invariant holds trivially.
        assert!(s.supply_invariant_holds());
        // Corrupt emitted_atoms so circ + pot + destroyed ≠ emitted → violation.
        s.emitted_atoms = 1;
        assert!(!s.supply_invariant_holds());
        // Repair: align circulating with emitted.
        s.circulating_supply = Amount::from_atoms(1);
        assert!(s.supply_invariant_holds());
    }

    #[test]
    fn test_first_block_sets_emission_epoch_and_emits_nothing() {
        let mut s = WorldState::new();
        let (_, producer) = kp_addr();
        let (fees, emission) = s.settle_block(&producer, 1, 1_000);
        assert_eq!(fees, Amount::ZERO);
        assert_eq!(emission, Amount::ZERO);
        assert_eq!(s.emission_epoch_ts, 1_000);
        assert_eq!(s.emitted_atoms, 0);
    }

    #[test]
    fn test_emission_rewards_producer_and_conserves_supply() {
        let mut s = WorldState::new();
        let (_, producer) = kp_addr();
        s.settle_block(&producer, 1, 0); // establish epoch at t=0
        // One full half-life later: ~50% of the supply has been minted.
        let (_, emission) = s.settle_block(&producer, 2, EMISSION_T_HALF_SECS);
        let expected = cumulative_emission_atoms(EMISSION_T_HALF_SECS);
        assert_eq!(emission.atoms(), expected);
        // ADR 0028: producer gets 20%, 80% goes to epoch pot.
        let producer_share = expected * PROPOSER_SHARE_BPS / BPS_DENOM;
        assert_eq!(s.accounts[&producer].balance.atoms(), producer_share);
        assert_eq!(s.emitted_atoms, expected);
        // Supply invariant: circulating + pot == emitted.
        assert!(s.supply_invariant_holds());
    }

    #[test]
    fn test_emission_is_not_weighted_by_bond() {
        // A producer with zero stake still earns the full block emission.
        let mut s = WorldState::new();
        let (_, producer) = kp_addr();
        s.settle_block(&producer, 1, 0);
        let (_, emission) = s.settle_block(&producer, 2, EMISSION_T_HALF_SECS / 20); // ~1 year
        assert!(emission > Amount::ZERO);
        // ADR 0028: producer receives PROPOSER_SHARE_BPS (20%) of the emission.
        let expected_balance = Amount::from_atoms(emission.atoms() * PROPOSER_SHARE_BPS / BPS_DENOM);
        assert_eq!(s.accounts[&producer].balance, expected_balance);
    }

    #[test]
    fn test_fee_goes_to_producer_not_epoch_pot() {
        let mut s = WorldState::new();
        let (sender_kp, sender) = kp_addr();
        let (_, receiver) = kp_addr();
        let (_, producer) = kp_addr();
        s.credit_emit_for_test(sender, Amount::from_vinx(1_000));
        let amount = Amount::from_vinx(100);
        let fee = amount.calculate_fee(s.base_fee);
        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 0);
        let pot_before = s.epoch_dist_emission_pot;
        s.apply_transaction(&tx).unwrap();
        // Fee is collected, not routed to the epoch pot.
        assert_eq!(s.epoch_dist_emission_pot, pot_before);
        // Settling credits the producer with the fee (epoch established, no emission).
        s.settle_block(&producer, 1, 100);
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
        let (_, producer) = kp_addr(); // separate producer so emission doesn't land on addr
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

        // Just before unlock: nothing matures. Use a separate producer so addr stays clean.
        s.settle_block(&producer, 1, 1_000 + UNBONDING_SECS - 1);
        assert_eq!(s.pending_unbonds.len(), 1);
        // At unlock: the bond returns to addr's balance.
        s.settle_block(&producer, 2, 1_000 + UNBONDING_SECS);
        assert!(s.pending_unbonds.is_empty());
        assert_eq!(s.accounts[&addr].balance, Amount::from_vinx(1_000));
    }

    // ─── ADR 0026: existential deposit & account reaping ─────────────────────
    use vinx_core::amount::EXISTENTIAL_DEPOSIT_ATOMS;

    #[test]
    fn test_transfer_creating_dust_account_is_rejected() {
        // A transfer that would leave a fresh receiver in ]0, ED[ is refused — no
        // dust account is ever materialized, and the sender is left untouched.
        let mut s = WorldState::new();
        let (sender_kp, sender) = kp_addr();
        let (_, receiver) = kp_addr();
        s.credit_for_test(sender, Amount::from_vinx(1_000));
        let dust = Amount::from_atoms(EXISTENTIAL_DEPOSIT_ATOMS / 2); // below ED
        let fee = dust.calculate_fee(s.base_fee);
        let tx = Transaction::new_transfer(&sender_kp, receiver, dust, fee, 0);
        assert_eq!(
            s.apply_transaction(&tx),
            Err(CoreError::BelowExistentialDeposit)
        );
        // Rejected before any mutation: receiver never created, sender nonce intact.
        assert!(s.get_account(&receiver).is_none());
        assert_eq!(s.accounts[&sender].nonce, 0);
        assert!(s.existential_invariant_holds());
    }

    #[test]
    fn test_transfer_of_exactly_ed_is_accepted() {
        let mut s = WorldState::new();
        let (sender_kp, sender) = kp_addr();
        let (_, receiver) = kp_addr();
        s.credit_for_test(sender, Amount::from_vinx(1_000));
        let ed = Amount::from_atoms(EXISTENTIAL_DEPOSIT_ATOMS);
        let fee = ed.calculate_fee(s.base_fee);
        let tx = Transaction::new_transfer(&sender_kp, receiver, ed, fee, 0);
        s.apply_transaction(&tx).unwrap();
        assert_eq!(s.accounts[&receiver].balance, ed);
        assert!(s.existential_invariant_holds());
    }

    #[test]
    fn test_sweep_to_zero_reaps_sender_and_conserves_supply() {
        // Draining an account to exactly 0 removes it from state (frees its 60 bytes)
        // without changing circulating supply (ADR 0004 mass invariant untouched).
        let mut s = WorldState::new();
        let (sender_kp, sender) = kp_addr();
        let (_, receiver) = kp_addr();
        let amount = Amount::from_vinx(100);
        let fee = amount.calculate_fee(s.base_fee);
        let total = amount.checked_add(fee).unwrap();
        s.credit_for_test(sender, total);
        s.circulating_supply = total;
        let supply_before = s.circulating_supply;

        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 0);
        s.apply_transaction(&tx).unwrap();

        // Sender drained to exactly 0 → reaped and gone from the map and the tree.
        assert!(s.get_account(&sender).is_none());
        assert!(!s.accounts_sorted().iter().any(|a| a.address == sender));
        // The receiver holds the funds; the fee is still in the block pool (circulation).
        assert_eq!(s.accounts[&receiver].balance, amount);
        assert_eq!(s.circulating_supply, supply_before);
        assert!(s.existential_invariant_holds());
        // The tree recomputes cleanly without the reaped leaf.
        let _ = s.compute_state_root();
    }

    #[test]
    fn test_transfer_leaving_sender_as_dust_is_rejected() {
        // Symmetric to the receiver rule: a transfer that would strand the *sender* in
        // ]0, ED[ is refused (they must land at exactly 0 or keep >= ED).
        let mut s = WorldState::new();
        let (sender_kp, sender) = kp_addr();
        let (_, receiver) = kp_addr();
        let amount = Amount::from_vinx(1);
        let fee = amount.calculate_fee(s.base_fee);
        // Fund the sender to end at ED/2 after amount + fee.
        let leftover = Amount::from_atoms(EXISTENTIAL_DEPOSIT_ATOMS / 2);
        let funded = amount
            .checked_add(fee)
            .unwrap()
            .checked_add(leftover)
            .unwrap();
        s.credit_for_test(sender, funded);
        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 0);
        assert_eq!(
            s.apply_transaction(&tx),
            Err(CoreError::BelowExistentialDeposit)
        );
        // Untouched: no partial mutation, receiver never created.
        assert_eq!(s.accounts[&sender].balance, funded);
        assert_eq!(s.accounts[&sender].nonce, 0);
        assert!(s.get_account(&receiver).is_none());
    }

    #[test]
    fn test_staked_account_is_exempt_from_ed_floor_and_not_reaped() {
        // A bonded account is never dust: it may hold a tiny balance (< ED) and must not
        // be reaped while it still has stake.
        let mut s = WorldState::new();
        let (sender_kp, sender) = kp_addr();
        let (_, receiver) = kp_addr();
        let ed = Amount::from_atoms(EXISTENTIAL_DEPOSIT_ATOMS);
        let amount = ed; // receiver ends exactly at ED — fine
        let fee = amount.calculate_fee(s.base_fee);
        let leftover = Amount::from_atoms(EXISTENTIAL_DEPOSIT_ATOMS / 2); // sender ends as sub-ED
        let funded = amount
            .checked_add(fee)
            .unwrap()
            .checked_add(leftover)
            .unwrap();
        // Stake first (set_staked_for_test zeroes the balance), then fund.
        s.set_staked_for_test(&sender, Amount::from_vinx(1));
        s.credit_for_test(sender, funded);

        let tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 0);
        s.apply_transaction(&tx).unwrap(); // allowed: sender is staked
        assert_eq!(s.accounts[&sender].balance, leftover);
        assert!(s.accounts[&sender].staked > Amount::ZERO);
        assert!(s.existential_invariant_holds());
    }

    #[test]
    fn test_unstake_leaving_zero_balance_is_not_reaped_until_matured() {
        // An account with everything unbonding (balance 0, staked 0, pending unbond) must
        // survive to receive the maturing funds — reaping it would burn them.
        let mut s = WorldState::new();
        let (kp, addr) = kp_addr();
        let stake = Amount::from_vinx(10);
        s.credit_for_test(addr, stake);
        s.apply_transaction(&Transaction::new_stake(&kp, stake, Amount::ZERO, 0))
            .unwrap();
        assert_eq!(s.accounts[&addr].balance, Amount::ZERO);
        s.set_block_context(1_000);
        s.apply_transaction(&Transaction::new_unstake(&kp, stake, Amount::ZERO, 1))
            .unwrap();
        // balance 0, staked 0, but a pending unbond exists → account must remain.
        assert!(s.get_account(&addr).is_some());
        assert_eq!(s.accounts[&addr].balance, Amount::ZERO);
        assert_eq!(s.accounts[&addr].staked, Amount::ZERO);
        // After maturation the funds return.
        s.settle_block(&addr, 1, 1_000 + UNBONDING_SECS);
        assert_eq!(s.accounts[&addr].balance, stake);
    }

    // ─── ADR 0011: K-of-M governance committee ───────────────────────────────

    fn committee(state: &mut WorldState, kps: &[KeyPair], threshold: u16) {
        let signers: Vec<Address> = kps
            .iter()
            .map(|k| Address::from_public_key(&k.public_key()))
            .collect();
        for a in &signers {
            state.credit_for_test(*a, Amount::from_vinx(1)); // materialize accounts (nonces)
        }
        state.admin_policy = Some(AdminPolicy { signers, threshold });
    }

    #[test]
    fn test_committee_requires_threshold_approvals() {
        use vinx_core::GovernanceAction;
        let mut s = WorldState::new();
        let kps: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
        committee(&mut s, &kps, 2); // 2-of-3
        let action = GovernanceAction::UpdateFeeFloor { atoms: 777 };

        // First approval: recorded, not yet executed.
        s.apply_transaction(&Transaction::new_admin_action(&kps[0], &action, 0))
            .unwrap();
        assert_eq!(s.pending_governance.len(), 1);
        assert_ne!(s.fee_floor.atoms(), 777);

        // Second, distinct approval: threshold reached → executed, proposal cleared.
        s.apply_transaction(&Transaction::new_admin_action(&kps[1], &action, 0))
            .unwrap();
        assert_eq!(s.fee_floor.atoms(), 777);
        assert!(s.pending_governance.is_empty());
    }

    #[test]
    fn test_committee_rejects_unauthorized_signer() {
        use vinx_core::GovernanceAction;
        let mut s = WorldState::new();
        let kps: Vec<KeyPair> = (0..2).map(|_| KeyPair::generate()).collect();
        committee(&mut s, &kps, 2);
        let outsider = KeyPair::generate();
        s.credit_for_test(
            Address::from_public_key(&outsider.public_key()),
            Amount::from_vinx(1),
        );
        let action = GovernanceAction::UpdateFeeFloor { atoms: 5 };
        assert_eq!(
            s.apply_transaction(&Transaction::new_admin_action(&outsider, &action, 0)),
            Err(CoreError::Unauthorized)
        );
        assert!(s.pending_governance.is_empty());
    }

    #[test]
    fn test_committee_rejects_duplicate_approval() {
        use vinx_core::GovernanceAction;
        let mut s = WorldState::new();
        let kps: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
        committee(&mut s, &kps, 2);
        let action = GovernanceAction::UpdateFeeFloor { atoms: 9 };
        s.apply_transaction(&Transaction::new_admin_action(&kps[0], &action, 0))
            .unwrap();
        // Same signer approving again is rejected; nonce not consumed.
        assert!(s
            .apply_transaction(&Transaction::new_admin_action(&kps[0], &action, 1))
            .is_err());
        let a0 = Address::from_public_key(&kps[0].public_key());
        assert_eq!(s.accounts[&a0].nonce, 1);
        assert_eq!(s.pending_governance[0].approvals.len(), 1);
    }

    #[test]
    fn test_single_admin_installs_committee_then_requires_it() {
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, admin) = admin_state();
        let member = KeyPair::generate();
        let member_addr = Address::from_public_key(&member.public_key());
        s.credit_for_test(member_addr, Amount::from_vinx(1));

        // Single admin installs a 2-of-2 committee — executes immediately (threshold 1 path).
        let set = GovernanceAction::SetAdminPolicy {
            signers: vec![admin, member_addr],
            threshold: 2,
        };
        s.apply_transaction(&Transaction::new_admin_action(&admin_kp, &set, 0))
            .unwrap();
        assert!(s.admin_policy.is_some());

        // Now a fee change needs both signers; one alone only tallies.
        let fee = GovernanceAction::UpdateFeeFloor { atoms: 42 };
        s.apply_transaction(&Transaction::new_admin_action(&admin_kp, &fee, 1))
            .unwrap();
        assert_ne!(s.fee_floor.atoms(), 42);
        s.apply_transaction(&Transaction::new_admin_action(&member, &fee, 0))
            .unwrap();
        assert_eq!(s.fee_floor.atoms(), 42);
    }

    #[test]
    fn test_set_admin_policy_validation() {
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, admin) = admin_state();
        // threshold greater than the signer count.
        let bad_threshold = GovernanceAction::SetAdminPolicy {
            signers: vec![admin],
            threshold: 2,
        };
        assert!(s
            .apply_transaction(&Transaction::new_admin_action(&admin_kp, &bad_threshold, 0))
            .is_err());
        // duplicate signer.
        let dup = GovernanceAction::SetAdminPolicy {
            signers: vec![admin, admin],
            threshold: 1,
        };
        assert!(s
            .apply_transaction(&Transaction::new_admin_action(&admin_kp, &dup, 0))
            .is_err());
        // A rejected policy leaves no committee and does not consume the nonce.
        assert!(s.admin_policy.is_none());
        assert_eq!(s.accounts[&admin].nonce, 0);
    }

    #[test]
    fn test_policy_change_clears_pending_proposals() {
        use vinx_core::GovernanceAction;
        let mut s = WorldState::new();
        let kps: Vec<KeyPair> = (0..2).map(|_| KeyPair::generate()).collect();
        committee(&mut s, &kps, 2); // 2-of-2
        let a0 = Address::from_public_key(&kps[0].public_key());
        let a1 = Address::from_public_key(&kps[1].public_key());

        // A fee proposal is pending (1 of 2 approvals).
        let fee = GovernanceAction::UpdateFeeFloor { atoms: 1 };
        s.apply_transaction(&Transaction::new_admin_action(&kps[0], &fee, 0))
            .unwrap();
        assert_eq!(s.pending_governance.len(), 1);

        // The committee replaces itself; on execution all in-flight proposals are cleared.
        let set = GovernanceAction::SetAdminPolicy {
            signers: vec![a0, a1],
            threshold: 1,
        };
        s.apply_transaction(&Transaction::new_admin_action(&kps[0], &set, 1))
            .unwrap(); // 1 of 2 for the policy change
        s.apply_transaction(&Transaction::new_admin_action(&kps[1], &set, 0))
            .unwrap(); // threshold → executes, clears pending
        assert_eq!(s.admin_policy.as_ref().unwrap().threshold, 1);
        assert!(s.pending_governance.is_empty());
    }

    // ─── ADR 0010: bonded module registry ────────────────────────────────────
    use vinx_core::amount::MIN_MODULE_BOND_ATOMS;
    use vinx_core::module::ModuleOp;

    fn operator_state() -> (WorldState, KeyPair, Address) {
        let mut s = WorldState::new();
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        s.credit_for_test(addr, Amount::from_vinx(5_000));
        s.circulating_supply = Amount::from_vinx(5_000);
        (s, kp, addr)
    }

    #[test]
    fn test_module_register_locks_bond_and_conserves_supply() {
        let (mut s, kp, addr) = operator_state();
        let fee = s.base_fee;
        let supply_before = s.circulating_supply;
        let id = [7u8; 32];
        let op = ModuleOp::Register {
            module_id: id,
            bond_atoms: MIN_MODULE_BOND_ATOMS,
        };
        s.apply_transaction(&Transaction::new_anchor_state(&kp, &op, fee, 0))
            .unwrap();

        let entry = s.modules.get(&id).unwrap();
        assert_eq!(entry.operator, addr);
        assert_eq!(entry.bond.atoms(), MIN_MODULE_BOND_ATOMS);
        assert_eq!(entry.anchor_head, [0u8; 32]);
        // Balance debited by bond + fee; bond is locked (not destroyed) so circulation holds.
        let expected = Amount::from_vinx(5_000).atoms() - MIN_MODULE_BOND_ATOMS - fee.atoms();
        assert_eq!(s.accounts[&addr].balance.atoms(), expected);
        assert_eq!(s.circulating_supply, supply_before);
    }

    #[test]
    fn test_module_register_rejects_low_bond_and_duplicate() {
        let (mut s, kp, _) = operator_state();
        let fee = s.base_fee;
        // Below the minimum bond.
        let low = ModuleOp::Register {
            module_id: [1u8; 32],
            bond_atoms: MIN_MODULE_BOND_ATOMS - 1,
        };
        assert!(s
            .apply_transaction(&Transaction::new_anchor_state(&kp, &low, fee, 0))
            .is_err());
        assert!(s.modules.is_empty());
        // Register, then a duplicate id is rejected.
        let id = [2u8; 32];
        let ok = ModuleOp::Register {
            module_id: id,
            bond_atoms: MIN_MODULE_BOND_ATOMS,
        };
        s.apply_transaction(&Transaction::new_anchor_state(&kp, &ok, fee, 0))
            .unwrap();
        let dup = ModuleOp::Register {
            module_id: id,
            bond_atoms: MIN_MODULE_BOND_ATOMS,
        };
        assert!(s
            .apply_transaction(&Transaction::new_anchor_state(&kp, &dup, fee, 1))
            .is_err());
    }

    #[test]
    fn test_module_anchor_is_operator_only() {
        let (mut s, kp, _) = operator_state();
        let fee = s.base_fee;
        let id = [3u8; 32];
        s.apply_transaction(&Transaction::new_anchor_state(
            &kp,
            &ModuleOp::Register {
                module_id: id,
                bond_atoms: MIN_MODULE_BOND_ATOMS,
            },
            fee,
            0,
        ))
        .unwrap();

        // Operator advances the anchor.
        let head = [9u8; 32];
        s.apply_transaction(&Transaction::new_anchor_state(
            &kp,
            &ModuleOp::Anchor {
                module_id: id,
                anchor_head: head,
            },
            fee,
            1,
        ))
        .unwrap();
        assert_eq!(s.modules[&id].anchor_head, head);
        assert_eq!(s.modules[&id].anchored_count, 1);

        // A non-operator cannot anchor.
        let outsider = KeyPair::generate();
        s.credit_for_test(
            Address::from_public_key(&outsider.public_key()),
            Amount::from_vinx(1),
        );
        let r = s.apply_transaction(&Transaction::new_anchor_state(
            &outsider,
            &ModuleOp::Anchor {
                module_id: id,
                anchor_head: [1u8; 32],
            },
            fee,
            0,
        ));
        assert_eq!(r, Err(CoreError::Unauthorized));
        assert_eq!(s.modules[&id].anchor_head, head); // unchanged
    }

    #[test]
    fn test_module_deregister_returns_bond() {
        let (mut s, kp, addr) = operator_state();
        let fee = s.base_fee;
        let id = [4u8; 32];
        s.apply_transaction(&Transaction::new_anchor_state(
            &kp,
            &ModuleOp::Register {
                module_id: id,
                bond_atoms: MIN_MODULE_BOND_ATOMS,
            },
            fee,
            0,
        ))
        .unwrap();
        let bal_after_register = s.accounts[&addr].balance.atoms();
        s.apply_transaction(&Transaction::new_anchor_state(
            &kp,
            &ModuleOp::Deregister { module_id: id },
            fee,
            1,
        ))
        .unwrap();
        assert!(s.modules.is_empty());
        // Bond refunded, minus the deregister fee.
        assert_eq!(
            s.accounts[&addr].balance.atoms(),
            bal_after_register + MIN_MODULE_BOND_ATOMS - fee.atoms()
        );
    }

    #[test]
    fn test_module_register_insufficient_balance_rejected() {
        let mut s = WorldState::new();
        let kp = KeyPair::generate();
        s.credit_for_test(
            Address::from_public_key(&kp.public_key()),
            Amount::from_vinx(500), // below the 1000 VINX bond
        );
        let fee = s.base_fee;
        let op = ModuleOp::Register {
            module_id: [5u8; 32],
            bond_atoms: MIN_MODULE_BOND_ATOMS,
        };
        assert!(s
            .apply_transaction(&Transaction::new_anchor_state(&kp, &op, fee, 0))
            .is_err());
        assert!(s.modules.is_empty());
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

    // Builds a BLS-signed header at `height` with a distinguishing `state_root`.
    // Returns (header, bls_sig_bytes) — the 96-byte G2 individual BLS signature.
    fn bls_signed_header(
        bls_sk: &vinx_crypto::BlsSecretKey,
        validator: Address,
        height: u64,
        tag: u8,
    ) -> (BlockHeader, Vec<u8>) {
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
        let sig = bls_sk.sign(&header.hash());
        (header, sig.0.to_vec())
    }

    // Registers a BLS key for `addr` in the validator_pool (bypasses PoP check for tests).
    fn register_bls_key_for_test(
        s: &mut WorldState,
        addr: Address,
        bls_sk: &vinx_crypto::BlsSecretKey,
    ) {
        use vinx_core::validator_pool::ValidatorPoolEntry;
        let bls_pk = bls_sk.public_key();
        let entry = s
            .validator_pool
            .entry(addr)
            .or_insert_with(|| ValidatorPoolEntry::new(MIN_VALIDATOR_BOND_ATOMS, 0));
        entry.bls_pub_key = Some(bls_pk.0.to_vec());
    }

    #[test]
    fn test_slash_rejects_forged_evidence() {
        // An attacker who does not hold the victim's BLS key cannot fabricate evidence.
        use vinx_crypto::BlsSecretKey;
        let (reporter_kp, reporter) = kp_addr();
        let (_, victim) = kp_addr();
        let attacker_bls_sk = BlsSecretKey::generate();
        let victim_bls_sk = BlsSecretKey::generate();
        let mut s = WorldState::new();
        s.credit_for_test(reporter, Amount::from_vinx(10));
        s.set_staked_for_test(&victim, Amount::from_vinx(1_000));
        // Register the victim's actual BLS key — attacker will forge with a different key.
        register_bls_key_for_test(&mut s, victim, &victim_bls_sk);
        s.validator_set = ValidatorSet::new(vec![victim, reporter]);

        let (header_a, _) = bls_signed_header(&victim_bls_sk, victim, 5, 0xAA);
        let (header_b, _) = bls_signed_header(&victim_bls_sk, victim, 5, 0xBB);
        // Forged: sign headers with the attacker's BLS key, not the victim's.
        let forge_sig_a = attacker_bls_sk.sign(&header_a.hash()).0.to_vec();
        let forge_sig_b = attacker_bls_sk.sign(&header_b.hash()).0.to_vec();
        let evidence = SlashEvidence {
            header_a,
            header_b,
            bls_sig_a: forge_sig_a,
            bls_sig_b: forge_sig_b,
        };
        let tx = Transaction::new_slash_validator(&reporter_kp, victim, &evidence, 0);
        assert!(s.apply_transaction(&tx).is_err());
        // Victim keeps its bond and its seat.
        assert_eq!(s.accounts[&victim].staked, Amount::from_vinx(1_000));
        assert!(s.validator_set.contains(&victim));
    }

    #[test]
    fn test_slash_valid_equivocation_burns_bond() {
        use vinx_crypto::BlsSecretKey;
        let (reporter_kp, reporter) = kp_addr();
        let (_, victim) = kp_addr();
        let victim_bls_sk = BlsSecretKey::generate();
        let mut s = WorldState::new();
        s.credit_for_test(reporter, Amount::from_vinx(10));
        s.set_staked_for_test(&victim, Amount::from_vinx(1_000));
        // Register victim's BLS key so apply_slash_validator can verify the proof.
        register_bls_key_for_test(&mut s, victim, &victim_bls_sk);
        s.validator_set = ValidatorSet::new(vec![victim, reporter]);

        // Two genuinely BLS-signed, different headers at the same height = equivocation.
        let (header_a, bls_sig_a) = bls_signed_header(&victim_bls_sk, victim, 5, 0xAA);
        let (header_b, bls_sig_b) = bls_signed_header(&victim_bls_sk, victim, 5, 0xBB);
        let evidence = SlashEvidence {
            header_a,
            header_b,
            bls_sig_a,
            bls_sig_b,
        };
        let tx = Transaction::new_slash_validator(&reporter_kp, victim, &evidence, 0);
        s.apply_transaction(&tx).unwrap();

        // 100% of the bond slashed: victim loses everything and its seat.
        assert_eq!(s.accounts[&victim].staked, Amount::ZERO);
        assert!(!s.validator_set.contains(&victim));
        // 10% bounty to the reporter (100 VINX), 90% (900 VINX) moved to the epoch pot.
        assert_eq!(s.accounts[&reporter].balance, Amount::from_vinx(110));
        assert_eq!(s.epoch_dist_emission_pot, Amount::from_vinx(900));
    }

    // ─── ADR 0027: unjail ────────────────────────────────────────────────────

    #[test]
    fn test_unjail_clears_jailed_status_after_cooldown() {
        use vinx_core::reliability::UNJAIL_COOLDOWN_HEIGHTS;
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_vinx(1));
        // Jail the validator by simulating missed proposals.
        let entry = s.reliability.entry(addr).or_default();
        entry.jailed_until = Some(s.block_height + UNJAIL_COOLDOWN_HEIGHTS + 1);
        // Before cooldown: unjail should fail.
        s.block_height = 0;
        let tx = Transaction::new_unjail(&kp, 0);
        assert!(s.apply_transaction(&tx).is_err(), "unjail before cooldown must fail");
        // After cooldown: unjail should succeed.
        s.block_height = UNJAIL_COOLDOWN_HEIGHTS + 2;
        let tx = Transaction::new_unjail(&kp, 0);
        s.apply_transaction(&tx).unwrap();
        assert!(!s.reliability.get(&addr).map(|r| r.is_jailed()).unwrap_or(false));
    }

    #[test]
    fn test_unjail_not_jailed_fails() {
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_vinx(1));
        // Validator was never jailed.
        let tx = Transaction::new_unjail(&kp, 0);
        assert!(s.apply_transaction(&tx).is_err(), "unjail when not jailed must fail");
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

    // ─── ADR 0046: BLS key registration ──────────────────────────────────────

    /// Build a minimal state where `kp`'s address is in the validator pool.
    fn validator_pool_state() -> (WorldState, KeyPair, Address) {
        use vinx_core::validator_pool::ValidatorPoolEntry;
        let mut s = WorldState::new();
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        s.credit_for_test(addr, Amount::from_vinx(1_000));
        // Insert directly into the pool (bypasses admission rules for test simplicity).
        s.validator_pool.insert(
            addr,
            ValidatorPoolEntry::new(MIN_VALIDATOR_BOND_ATOMS, 0),
        );
        (s, kp, addr)
    }

    #[test]
    fn test_register_bls_key_happy_path() {
        use vinx_core::RegisterBlsKeyPayload;
        use vinx_crypto::BlsSecretKey;

        let (mut s, kp, addr) = validator_pool_state();

        let bls_sk = BlsSecretKey::generate();
        let bls_pk = bls_sk.public_key();
        let pop = bls_sk.proof_of_possession();
        let payload = RegisterBlsKeyPayload {
            bls_pub_key: bls_pk.0.to_vec(),
            bls_pop: pop.0.to_vec(),
        };
        let tx = Transaction::new_register_bls_key(&kp, &payload, 0);
        s.apply_transaction(&tx).unwrap();

        let entry = &s.validator_pool[&addr];
        assert_eq!(entry.bls_pub_key.as_deref(), Some(bls_pk.0.as_slice()));
        assert_eq!(entry.bls_pop.as_deref(), Some(pop.0.as_slice()));
        // Nonce consumed.
        assert_eq!(s.accounts[&addr].nonce, 1);
    }

    #[test]
    fn test_register_bls_key_non_validator_rejected() {
        use vinx_core::RegisterBlsKeyPayload;
        use vinx_crypto::BlsSecretKey;

        let mut s = WorldState::new();
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        s.credit_for_test(addr, Amount::from_vinx(10));

        let bls_sk = BlsSecretKey::generate();
        let pop = bls_sk.proof_of_possession();
        let payload = RegisterBlsKeyPayload {
            bls_pub_key: bls_sk.public_key().0.to_vec(),
            bls_pop: pop.0.to_vec(),
        };
        let tx = Transaction::new_register_bls_key(&kp, &payload, 0);
        assert!(s.apply_transaction(&tx).is_err());
        // No pool entry created.
        assert!(!s.validator_pool.contains_key(&addr));
    }

    #[test]
    fn test_register_bls_key_invalid_pop_rejected() {
        use vinx_core::RegisterBlsKeyPayload;
        use vinx_crypto::BlsSecretKey;

        let (mut s, kp, addr) = validator_pool_state();

        let bls_sk = BlsSecretKey::generate();
        let wrong_sk = BlsSecretKey::generate();
        // PoP from a different key — cryptographic verification must reject this.
        let bad_pop = wrong_sk.proof_of_possession();
        let payload = RegisterBlsKeyPayload {
            bls_pub_key: bls_sk.public_key().0.to_vec(),
            bls_pop: bad_pop.0.to_vec(),
        };
        let tx = Transaction::new_register_bls_key(&kp, &payload, 0);
        assert!(s.apply_transaction(&tx).is_err());
        // Pool entry untouched: still no BLS key.
        assert!(s.validator_pool[&addr].bls_pub_key.is_none());
        // Nonce not consumed on rejected tx.
        assert_eq!(s.accounts[&addr].nonce, 0);
    }

    #[test]
    fn test_register_bls_key_wrong_sizes_rejected() {
        use vinx_core::RegisterBlsKeyPayload;
        use vinx_core::CoreError;

        let (mut s, kp, _addr) = validator_pool_state();

        // 47-byte pubkey (should be 48).
        let bad_pk = RegisterBlsKeyPayload {
            bls_pub_key: vec![0u8; 47],
            bls_pop: vec![0u8; 96],
        };
        assert!(matches!(
            s.apply_transaction(&Transaction::new_register_bls_key(&kp, &bad_pk, 0)),
            Err(CoreError::InvalidTransaction(_))
        ));

        // 95-byte PoP (should be 96).
        let bad_pop = RegisterBlsKeyPayload {
            bls_pub_key: vec![0u8; 48],
            bls_pop: vec![0u8; 95],
        };
        assert!(matches!(
            s.apply_transaction(&Transaction::new_register_bls_key(&kp, &bad_pop, 0)),
            Err(CoreError::InvalidTransaction(_))
        ));
    }

    #[test]
    fn test_register_bls_key_updates_existing() {
        use vinx_core::RegisterBlsKeyPayload;
        use vinx_crypto::BlsSecretKey;

        let (mut s, kp, addr) = validator_pool_state();

        // Register once with key A.
        let sk_a = BlsSecretKey::generate();
        let pop_a = sk_a.proof_of_possession();
        let payload_a = RegisterBlsKeyPayload {
            bls_pub_key: sk_a.public_key().0.to_vec(),
            bls_pop: pop_a.0.to_vec(),
        };
        s.apply_transaction(&Transaction::new_register_bls_key(&kp, &payload_a, 0))
            .unwrap();

        // Re-register with key B — should overwrite.
        let sk_b = BlsSecretKey::generate();
        let pop_b = sk_b.proof_of_possession();
        let payload_b = RegisterBlsKeyPayload {
            bls_pub_key: sk_b.public_key().0.to_vec(),
            bls_pop: pop_b.0.to_vec(),
        };
        s.apply_transaction(&Transaction::new_register_bls_key(&kp, &payload_b, 1))
            .unwrap();

        let entry = &s.validator_pool[&addr];
        assert_eq!(entry.bls_pub_key.as_deref(), Some(sk_b.public_key().0.as_slice()));
    }

    // ─── ADR 0038: open PoS admission (bond floor & active-set governance) ────

    #[test]
    fn test_stake_auto_enters_pool_at_bond_floor() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
        assert!(!s.validator_pool.contains_key(&addr));

        s.apply_transaction(&Transaction::new_stake(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
            Amount::ZERO,
            0,
        ))
        .unwrap();

        assert!(s.validator_pool.contains_key(&addr), "pool entry must be created on floor stake");
        assert_eq!(s.validator_pool[&addr].bond_atoms, MIN_VALIDATOR_BOND_ATOMS);
    }

    #[test]
    fn test_stake_below_floor_does_not_enter_pool() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        let below_floor = MIN_VALIDATOR_BOND_ATOMS - 1;
        s.credit_for_test(addr, Amount::from_atoms(below_floor));

        s.apply_transaction(&Transaction::new_stake(
            &kp,
            Amount::from_atoms(below_floor),
            Amount::ZERO,
            0,
        ))
        .unwrap();

        assert!(!s.validator_pool.contains_key(&addr), "sub-floor bond must not enter pool");
    }

    #[test]
    fn test_stake_banned_address_not_admitted_to_pool() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
        s.banned_validator_keys.insert(addr);

        s.apply_transaction(&Transaction::new_stake(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
            Amount::ZERO,
            0,
        ))
        .unwrap();

        assert!(!s.validator_pool.contains_key(&addr), "banned address must not enter pool");
    }

    #[test]
    fn test_unstake_below_floor_moves_pool_entry_to_unbonding() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        use vinx_core::validator_pool::PoolStatus;
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS * 2));

        // Stake enough to enter the pool.
        s.apply_transaction(&Transaction::new_stake(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
            Amount::ZERO,
            0,
        ))
        .unwrap();
        assert!(s.validator_pool.contains_key(&addr));

        // Unstake all — bond drops below floor → pool entry → Unbonding.
        s.set_block_context(1_000);
        s.apply_transaction(&Transaction::new_unstake(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
            Amount::ZERO,
            1,
        ))
        .unwrap();

        let entry = &s.validator_pool[&addr];
        assert!(
            matches!(entry.status, PoolStatus::Unbonding { .. }),
            "pool entry must be Unbonding after bond drops below floor"
        );
    }

    // ─── ADR 0038: UpdateActiveSetSize governance ─────────────────────────────

    #[test]
    fn test_update_active_set_size_happy_path() {
        use vinx_core::amount::{ACTIVE_SET_STEP, DEFAULT_ACTIVE_SET_SIZE};
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _) = admin_state();
        s.current_block_ts = 1_000;
        let new_size = DEFAULT_ACTIVE_SET_SIZE + ACTIVE_SET_STEP;

        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateActiveSetSize { new_size },
            0,
        ))
        .unwrap();

        assert_eq!(s.active_set_size, new_size);
        assert_eq!(s.last_active_set_size_change_ts, 1_000);
    }

    #[test]
    fn test_update_active_set_size_wrong_step_rejected() {
        use vinx_core::amount::DEFAULT_ACTIVE_SET_SIZE;
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _) = admin_state();

        // Step of 4 (not ACTIVE_SET_STEP=2) must be rejected.
        assert!(s
            .apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::UpdateActiveSetSize {
                    new_size: DEFAULT_ACTIVE_SET_SIZE + 4,
                },
                0,
            ))
            .is_err());
        assert_eq!(s.active_set_size, DEFAULT_ACTIVE_SET_SIZE);
    }

    #[test]
    fn test_update_active_set_size_below_minimum_rejected() {
        use vinx_core::amount::{ACTIVE_SET_STEP, MIN_ACTIVE_SET_SIZE};
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _) = admin_state();
        // Force the size down to MIN_ACTIVE_SET_SIZE + ACTIVE_SET_STEP so one more
        // decrement would cross the floor.
        s.active_set_size = MIN_ACTIVE_SET_SIZE + ACTIVE_SET_STEP;

        assert!(s
            .apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::UpdateActiveSetSize {
                    new_size: MIN_ACTIVE_SET_SIZE, // exactly the floor — still ok
                },
                0,
            ))
            .is_ok());

        // One more decrement would go below the floor.
        s.last_active_set_size_change_ts = 0; // bypass cooldown
        assert!(s
            .apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::UpdateActiveSetSize {
                    new_size: MIN_ACTIVE_SET_SIZE - ACTIVE_SET_STEP,
                },
                1,
            ))
            .is_err());
        assert_eq!(s.active_set_size, MIN_ACTIVE_SET_SIZE);
    }

    #[test]
    fn test_update_active_set_size_cooldown_enforced() {
        use vinx_core::amount::{ACTIVE_SET_COOLDOWN_SECS, ACTIVE_SET_STEP, DEFAULT_ACTIVE_SET_SIZE};
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _) = admin_state();
        // Use a non-zero baseline so last_change_ts > 0 after the first change.
        s.current_block_ts = 1_000;

        // First change succeeds.
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateActiveSetSize {
                new_size: DEFAULT_ACTIVE_SET_SIZE + ACTIVE_SET_STEP,
            },
            0,
        ))
        .unwrap();

        // Second change within cooldown must fail.
        s.current_block_ts = 1_000 + ACTIVE_SET_COOLDOWN_SECS - 1;
        assert!(s
            .apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::UpdateActiveSetSize {
                    new_size: DEFAULT_ACTIVE_SET_SIZE + ACTIVE_SET_STEP * 2,
                },
                1,
            ))
            .is_err());

        // After cooldown elapses it succeeds again.
        s.current_block_ts = 1_000 + ACTIVE_SET_COOLDOWN_SECS + 1;
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateActiveSetSize {
                new_size: DEFAULT_ACTIVE_SET_SIZE + ACTIVE_SET_STEP * 2,
            },
            1,
        ))
        .unwrap();
        assert_eq!(s.active_set_size, DEFAULT_ACTIVE_SET_SIZE + ACTIVE_SET_STEP * 2);
    }

    // ─── ADR 0038: UpdateMinValidatorBond governance ──────────────────────────

    #[test]
    fn test_update_min_validator_bond_happy_path() {
        use vinx_core::amount::{BOND_STEP_BPS, BPS_DENOM, MIN_VALIDATOR_BOND_ATOMS};
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _) = admin_state();
        // Increase by exactly BOND_STEP_BPS (25 %).
        let new_atoms = MIN_VALIDATOR_BOND_ATOMS + MIN_VALIDATOR_BOND_ATOMS * BOND_STEP_BPS / BPS_DENOM;

        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateMinValidatorBond { atoms: new_atoms },
            0,
        ))
        .unwrap();

        assert_eq!(s.min_validator_bond_atoms, new_atoms);
    }

    #[test]
    fn test_update_min_validator_bond_below_hard_floor_rejected() {
        use vinx_core::amount::MIN_BOND_HARD_FLOOR;
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _) = admin_state();
        s.min_validator_bond_atoms = MIN_BOND_HARD_FLOOR; // already at the floor

        assert!(s
            .apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::UpdateMinValidatorBond {
                    atoms: MIN_BOND_HARD_FLOOR - 1,
                },
                0,
            ))
            .is_err());
        assert_eq!(s.min_validator_bond_atoms, MIN_BOND_HARD_FLOOR);
    }

    #[test]
    fn test_update_min_validator_bond_above_hard_cap_rejected() {
        use vinx_core::amount::MAX_BOND_HARD_CAP;
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _) = admin_state();
        s.min_validator_bond_atoms = MAX_BOND_HARD_CAP;

        assert!(s
            .apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::UpdateMinValidatorBond {
                    atoms: MAX_BOND_HARD_CAP + 1,
                },
                0,
            ))
            .is_err());
        assert_eq!(s.min_validator_bond_atoms, MAX_BOND_HARD_CAP);
    }

    #[test]
    fn test_update_min_validator_bond_step_too_large_rejected() {
        use vinx_core::amount::{BOND_STEP_BPS, BPS_DENOM, MIN_VALIDATOR_BOND_ATOMS};
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _) = admin_state();
        // More than 25% in one go.
        let too_large = MIN_VALIDATOR_BOND_ATOMS + MIN_VALIDATOR_BOND_ATOMS * BOND_STEP_BPS / BPS_DENOM + 1;

        assert!(s
            .apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::UpdateMinValidatorBond { atoms: too_large },
                0,
            ))
            .is_err());
        assert_eq!(s.min_validator_bond_atoms, MIN_VALIDATOR_BOND_ATOMS);
    }

    #[test]
    fn test_update_min_validator_bond_cooldown_enforced() {
        use vinx_core::amount::{BOND_COOLDOWN_SECS, BOND_STEP_BPS, BPS_DENOM, MIN_VALIDATOR_BOND_ATOMS};
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _) = admin_state();
        // Non-zero baseline so last_bond_change_ts > 0 after the first change.
        s.current_block_ts = 1_000;
        let step = MIN_VALIDATOR_BOND_ATOMS * BOND_STEP_BPS / BPS_DENOM;

        // First change succeeds.
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateMinValidatorBond {
                atoms: MIN_VALIDATOR_BOND_ATOMS + step,
            },
            0,
        ))
        .unwrap();

        // Second change within cooldown must fail.
        s.current_block_ts = 1_000 + BOND_COOLDOWN_SECS - 1;
        assert!(s
            .apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::UpdateMinValidatorBond {
                    atoms: s.min_validator_bond_atoms + 1,
                },
                1,
            ))
            .is_err());

        // After cooldown it succeeds.
        s.current_block_ts = 1_000 + BOND_COOLDOWN_SECS + 1;
        let new_atoms = s.min_validator_bond_atoms + 1;
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateMinValidatorBond { atoms: new_atoms },
            1,
        ))
        .unwrap();
        assert_eq!(s.min_validator_bond_atoms, new_atoms);
    }

    // ─── ADR 0029 Phase 2: epoch beacon ──────────────────────────────────────

    #[test]
    fn test_tick_epoch_close_updates_beacon() {
        let mut s = WorldState::new();
        let beacon_before = s.epoch_beacon;
        s.tick_epoch_close();
        assert_ne!(s.epoch_beacon, beacon_before, "beacon must change after epoch close");
        assert_ne!(s.epoch_beacon, [0u8; 32], "beacon must be non-zero after epoch close");
    }

    #[test]
    fn test_tick_epoch_close_beacon_is_deterministic() {
        let mut s1 = WorldState::new();
        let mut s2 = WorldState::new();
        s1.tick_epoch_close();
        s2.tick_epoch_close();
        assert_eq!(s1.epoch_beacon, s2.epoch_beacon, "beacon must be deterministic");
    }

    #[test]
    fn test_tick_epoch_close_beacon_chain_differs_across_epochs() {
        let mut s = WorldState::new();
        s.tick_epoch_close();
        let beacon_after_1 = s.epoch_beacon;
        s.tick_epoch_close();
        let beacon_after_2 = s.epoch_beacon;
        assert_ne!(beacon_after_1, beacon_after_2, "successive epoch beacons must differ");
    }

    // ─── ADR 0029 Phase 2: committee_for_height ───────────────────────────────

    fn pool_state_with_active_validators(n: usize) -> WorldState {
        use vinx_core::validator_pool::{PoolStatus, ValidatorPoolEntry};
        let mut s = WorldState::new();
        for _ in 0..n {
            let (_, addr) = kp_addr();
            let mut entry = ValidatorPoolEntry::new(MIN_VALIDATOR_BOND_ATOMS, 0);
            entry.status = PoolStatus::Active;
            s.validator_pool.insert(addr, entry);
        }
        s
    }

    #[test]
    fn test_committee_for_height_respects_k_cap() {
        let s = pool_state_with_active_validators(10);
        let committee = s.committee_for_height(1, 5);
        assert_eq!(committee.len(), 5);
    }

    #[test]
    fn test_committee_for_height_at_most_pool_size() {
        let s = pool_state_with_active_validators(3);
        let committee = s.committee_for_height(1, 10);
        assert_eq!(committee.len(), 3, "committee cannot exceed pool size");
    }

    #[test]
    fn test_committee_for_height_is_deterministic() {
        let s = pool_state_with_active_validators(10);
        let c1 = s.committee_for_height(42, 5);
        let c2 = s.committee_for_height(42, 5);
        assert_eq!(c1, c2, "committee selection must be deterministic");
    }

    #[test]
    fn test_committee_for_height_differs_across_heights() {
        let s = pool_state_with_active_validators(10);
        let c1 = s.committee_for_height(1, 5);
        let c2 = s.committee_for_height(2, 5);
        // With 10 candidates and 5 slots there's virtually no chance of identical selection.
        assert_ne!(c1, c2, "committee should differ across heights");
    }

    #[test]
    fn test_committee_excludes_unbonding_and_warmup() {
        use vinx_core::validator_pool::{PoolStatus, ValidatorPoolEntry};
        let mut s = WorldState::new();

        let (_, active_addr) = kp_addr();
        let mut active_entry = ValidatorPoolEntry::new(MIN_VALIDATOR_BOND_ATOMS, 0);
        active_entry.status = PoolStatus::Active;
        s.validator_pool.insert(active_addr, active_entry);

        let (_, warmup_addr) = kp_addr();
        let warmup_entry = ValidatorPoolEntry::new(MIN_VALIDATOR_BOND_ATOMS, 0);
        // Warmup is the default status — leave it as-is.
        s.validator_pool.insert(warmup_addr, warmup_entry);

        let (_, unbonding_addr) = kp_addr();
        let mut unbonding_entry = ValidatorPoolEntry::new(MIN_VALIDATOR_BOND_ATOMS, 0);
        unbonding_entry.status = PoolStatus::Unbonding { unlock_ts: 99999 };
        s.validator_pool.insert(unbonding_addr, unbonding_entry);

        let committee = s.committee_for_height(1, 10);
        assert_eq!(committee, vec![active_addr], "only Active/Benched validators are eligible");
    }
}
