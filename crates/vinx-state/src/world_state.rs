use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use vinx_core::{
    amount::{
        cumulative_emission_atoms, Amount, BPS_DENOM, DEFAULT_ACTIVE_SET_SIZE,
        DEFAULT_FEE_FLOOR_ATOMS, EPOCH_DURATION_SECS, EXISTENTIAL_DEPOSIT_ATOMS, MAX_MODULES,
        EXIT_QUEUE_WINDOW_BLOCKS, MAX_NONCE_AHEAD, MAX_PAYLOAD_BYTES, MAX_VALIDATOR_EXITS_PER_WINDOW,
        MIN_MODULE_BOND_ATOMS, MIN_STAKE_ATOMS, MIN_VALIDATOR_BOND_ATOMS, MIN_VALIDATOR_SET_SIZE,
        MODULE_ESCROW_MAX_OPEN, MODULE_ESCROW_MAX_TIMEOUT_SECS,
        MODULE_ESCROW_MIN_TIMEOUT_SECS, PROPOSER_SHARE_BPS, SLASH_BOUNTY_BPS,
        SLASH_EQUIVOCATION_BPS, UNBONDING_SECS, VALIDATOR_SCORE_WINDOW_SECS,
        // ADR 0038 governance guard constants (immutable hard floors/caps)
        ACTIVE_SET_COOLDOWN_SECS, ACTIVE_SET_STEP, BOND_COOLDOWN_SECS, BOND_STEP_BPS,
        MAX_BOND_HARD_CAP, MIN_ACTIVE_SET_SIZE, MIN_BOND_HARD_FLOOR,
    },
    block::SlashEvidence,
    chain_id::CHAIN_ID_DEVNET,
    governance::GovernanceAction,
    module::{EscrowEntry, FeeSchedule, ModuleOp},
    protocol::{ProtocolVersion, ScheduledUpgrade},
    reliability::{self, CosignWindowMap, ReliabilityMap},
    validator_pool::{PoolStatus, ValidatorPoolEntry},
    Account, CoreError, ModuleEscrowPayload, RegisterBlsKeyPayload, Transaction, TransactionType,
    ValidatorSet,
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
    // ── Module escrow (ADR 0039) ────────────────────────────────────────────────
    // Both fields are appended after `last_epoch_close_ts` — the v14→v15 migration
    // appends their default encodings (two empty BTreeMaps). `serde(default)` so
    // pre-v15 state loads cleanly.
    //
    /// Pending service-payment escrows keyed by escrow_id.  Created by a
    /// `ModuleEscrow` tx; removed by `EscrowRelease` (release) or
    /// `ModuleEscrowRefund` (timeout refund).
    /// Appended after `last_epoch_close_ts` — v14→v15 migration appends its default.
    #[serde(default)]
    pub pending_escrows: BTreeMap<Hash32, EscrowEntry>,
    /// Fee-distribution schedules for registered modules (ADR 0039), keyed by
    /// `module_id`.  Set by operator via `ModuleOp::SetFeeSchedule`.  Stored
    /// separately from `modules` so `ModuleEntry` bincode layout stays unchanged.
    /// Appended after `pending_escrows` — v14→v15 migration appends its default.
    #[serde(default)]
    pub module_fee_schedules: BTreeMap<Hash32, FeeSchedule>,
    // ── Validator churn bounds (ADR 0036) ───────────────────────────────────────
    // Appended after `module_fee_schedules` — the v15→v16 migration appends its
    // default encoding (empty Vec). `serde(default)` so pre-v16 state loads cleanly.
    //
    /// FIFO queue of validator addresses pending removal from the active set (ADR 0036).
    /// Populated by `RemoveValidator` governance actions when the churn window is at
    /// capacity or the set floor would be violated. Processed at window boundaries in
    /// `settle_block` (up to `MAX_VALIDATOR_EXITS_PER_WINDOW` per `EXIT_QUEUE_WINDOW_BLOCKS`).
    /// Validators in this queue remain in `validator_set` (and slashable) until dequeued.
    /// Appended after `module_fee_schedules` — v15→v16 migration appends empty-vec default.
    #[serde(default)]
    pub validator_exit_queue: Vec<Address>,

    // ── Governable bond floor (ADR 0038) ────────────────────────────────────────
    // Appended after `validator_exit_queue` — v16→v17 migration appends the default
    // u128 = MIN_VALIDATOR_BOND_ATOMS. `serde(default)` so pre-v17 state loads cleanly.
    /// Current minimum validator bond in atoms (ADR 0038). Governable via
    /// `UpdateMinValidatorBond` within [MIN_BOND_HARD_FLOOR, MAX_BOND_HARD_CAP] and
    /// ±BOND_STEP_BPS per modification with BOND_COOLDOWN_SECS between changes.
    #[serde(default = "default_min_validator_bond")]
    pub min_validator_bond_atoms: u128,

    // ── Co-signature windows (ADR 0027 règle 2) ─────────────────────────────────
    // Appended after `min_validator_bond_atoms` — v17→v18 migration appends the default
    // empty BTreeMap. `serde(default)` so pre-v18 state loads cleanly.
    /// Per-epoch co-signature participation counters (ADR 0027 règle 2).
    /// Reset at each epoch close after checking participation against
    /// `MIN_COSIGN_PARTICIPATION_BPS`. Stored separately from `reliability` to avoid
    /// breaking its bincode layout.
    #[serde(default)]
    pub cosign_windows: CosignWindowMap,
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

/// The bincode bytes appended to a v14 `WorldState` meta blob to bring it to v15 (ADR 0039
/// module escrow). Appends the defaults of two new fields in declaration order:
///   1. `pending_escrows`       — empty `BTreeMap<Hash32, EscrowEntry>`
///   2. `module_fee_schedules`  — empty `BTreeMap<Hash32, FeeSchedule>`
pub fn v15_meta_suffix() -> Vec<u8> {
    let mut out = bincode::serialize(&BTreeMap::<Hash32, EscrowEntry>::new())
        .expect("serialize empty escrows");
    out.extend(
        bincode::serialize(&BTreeMap::<Hash32, FeeSchedule>::new())
            .expect("serialize empty fee schedules"),
    );
    out
}

/// Encoding appended to the meta blob by the v15→v16 migration (ADR 0036).
/// Adds the default value of `validator_exit_queue: Vec<Address>` — an empty vec.
pub fn v16_meta_suffix() -> Vec<u8> {
    bincode::serialize(&Vec::<Address>::new()).expect("serialize empty exit queue")
}

/// v17 migration suffix (ADR 0038): `WorldState` meta gains `min_validator_bond_atoms`
/// (`u128`), governable bond floor defaulting to `MIN_VALIDATOR_BOND_ATOMS`.
pub fn v17_meta_suffix() -> Vec<u8> {
    bincode::serialize(&MIN_VALIDATOR_BOND_ATOMS).expect("serialize default min validator bond")
}

/// v18 migration suffix (ADR 0027 règle 2): `WorldState` meta gains `cosign_windows`
/// (`CosignWindowMap`), defaulting to an empty `BTreeMap`.
pub fn v18_meta_suffix() -> Vec<u8> {
    bincode::serialize(&CosignWindowMap::new()).expect("serialize empty cosign windows")
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
            pending_escrows: BTreeMap::new(),
            module_fee_schedules: BTreeMap::new(),
            validator_exit_queue: Vec::new(),
            min_validator_bond_atoms: MIN_VALIDATOR_BOND_ATOMS,
            cosign_windows: CosignWindowMap::new(),
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
        // 3.5 Record proposer for epoch pot distribution (ADR 0028).
        if let Some(entry) = self.validator_pool.get_mut(producer) {
            entry.record_epoch_proposed();
        }
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
        // 6. Validator exit queue (ADR 0036) — at each window boundary, dequeue and
        //    remove up to MAX_VALIDATOR_EXITS_PER_WINDOW validators from the active set.
        //    Deterministic: driven by block height, independent of time and ordering.
        if height > 0 && height % EXIT_QUEUE_WINDOW_BLOCKS == 0 {
            self.process_exit_queue();
        }
        self.block_height = height;
        (fees, emission)
    }

    /// Processes the validator exit queue at a window boundary (ADR 0036).
    /// Removes up to `MAX_VALIDATOR_EXITS_PER_WINDOW` addresses from the front of
    /// `validator_exit_queue`, evicting them from the active set.  Skips any address
    /// that is no longer in `validator_set` (redundant entries from prior removals).
    fn process_exit_queue(&mut self) {
        let mut processed = 0usize;
        while processed < MAX_VALIDATOR_EXITS_PER_WINDOW && !self.validator_exit_queue.is_empty() {
            let addr = self.validator_exit_queue.remove(0);
            if self.validator_set.contains(&addr) {
                self.validator_set.remove(&addr);
                tracing::info!(%addr, "ADR 0036: validator exited active set from queue");
            }
            processed += 1;
        }
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
    /// - Updates `cosign_count_in_window` and `eligible_blocks_in_window` for every
    ///   validator currently in the `Active` pool status (Open PoA score, ADR 0028).
    /// - Updates `cosign_windows` for the classic validator set (ADR 0027 règle 2):
    ///   increments eligible + count for each non-jailed active validator.
    ///
    /// Call once per finalized block, after the co-signature set is known.
    pub fn record_block_cosigns(&mut self, cosigner_addrs: &[Address]) {
        let cosigners: HashSet<Address> = cosigner_addrs.iter().copied().collect();

        // ── Open PoA pool (ADR 0028) ──────────────────────────────────────────
        let active_addrs: Vec<Address> = self
            .validator_pool
            .iter()
            .filter(|(_, e)| matches!(e.status, PoolStatus::Active))
            .map(|(a, _)| *a)
            .collect();
        for addr in active_addrs {
            if let Some(entry) = self.validator_pool.get_mut(&addr) {
                let did_cosign = cosigners.contains(&addr);
                entry.record_block(true, did_cosign);
                if did_cosign {
                    entry.record_epoch_cosigned();
                }
            }
        }

        // ── Classic validator set (ADR 0027 règle 2) ──────────────────────────
        let active_classic = reliability::active_validators(&self.validator_set, &self.reliability);
        reliability::on_block_cosigns(&mut self.cosign_windows, &active_classic, &cosigners);
    }

    /// Résout des clés publiques BLS (octets G1 compressés, 48 octets) en adresses de
    /// validateurs en cherchant dans le pool. Utilisé pour enregistrer les co-signatures
    /// BLS (ADR 0046 / ADR 0027 R2).
    pub fn resolve_bls_cosigners(&self, bls_pks: &[Vec<u8>]) -> Vec<Address> {
        bls_pks
            .iter()
            .filter_map(|pk_bytes| {
                self.validator_pool
                    .iter()
                    .find(|(_, e)| e.bls_pub_key.as_deref() == Some(pk_bytes.as_slice()))
                    .map(|(addr, _)| *addr)
            })
            .collect()
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

        // 5. Distribute epoch pot — ADR 0028 split:
        //    PROPOSER_SHARE_BPS → proposers, weighted by blocks proposed this epoch.
        //    Remainder → co-signers, proportional to epoch_cosigned count (ADR 0028 §2.3).
        //    If no cosign data (e.g. genesis / pool just populated), fall back to equal share.
        if !new_active.is_empty() {
            let pot = self.epoch_dist_emission_pot;
            if pot > Amount::ZERO {
                let sorted: Vec<Address> = {
                    let mut v: Vec<Address> = new_active.iter().copied().collect();
                    v.sort();
                    v
                };

                // Snapshot per-epoch counters before any mutation.
                let proposed: Vec<u32> = sorted
                    .iter()
                    .map(|a| self.validator_pool.get(a).map(|e| e.epoch_proposed).unwrap_or(0))
                    .collect();
                let cosigned: Vec<u32> = sorted
                    .iter()
                    .map(|a| self.validator_pool.get(a).map(|e| e.epoch_cosigned).unwrap_or(0))
                    .collect();
                let total_proposed: u128 = proposed.iter().map(|&c| c as u128).sum();
                let total_cosigned: u128 = cosigned.iter().map(|&c| c as u128).sum();
                let n = sorted.len() as u128;

                let proposer_pot =
                    if total_proposed > 0 { pot.atoms() * PROPOSER_SHARE_BPS / BPS_DENOM } else { 0 };
                let cosign_pot = pot.atoms().saturating_sub(proposer_pot);

                // Pre-compute each validator's share.
                let mut shares: Vec<u128> = sorted
                    .iter()
                    .enumerate()
                    .map(|(i, _)| {
                        let proposer_share = if total_proposed > 0 {
                            proposer_pot * proposed[i] as u128 / total_proposed
                        } else {
                            0
                        };
                        // Proportional to epoch_cosigned; fall back to equal if nobody cosigned.
                        let cosign_share = if total_cosigned > 0 {
                            cosign_pot * cosigned[i] as u128 / total_cosigned
                        } else {
                            cosign_pot / n
                        };
                        proposer_share + cosign_share
                    })
                    .collect();

                // Integer-division remainder → validator with the most co-signs (deterministic).
                // On a tie, the first in sorted order wins (stable, canonical).
                let allocated: u128 = shares.iter().sum();
                let remainder = pot.atoms().saturating_sub(allocated);
                if remainder > 0 {
                    let best_idx = cosigned
                        .iter()
                        .enumerate()
                        .max_by_key(|&(_, &c)| c)
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    shares[best_idx] = shares[best_idx].saturating_add(remainder);
                }

                // Distribute — all shares pre-computed, no outstanding borrows.
                for (&addr, &atoms) in sorted.iter().zip(shares.iter()) {
                    if atoms > 0 {
                        self.distribute_from_epoch_pot(&addr, Amount::from_atoms(atoms));
                    }
                }

                // Reset per-epoch counters for next epoch.
                for entry in self.validator_pool.values_mut() {
                    entry.reset_epoch_counters();
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

        // 7. ADR 0027 règle 2 : check co-signature participation over the past epoch.
        //    Validators whose cosign rate fell below MIN_COSIGN_PARTICIPATION_BPS are jailed.
        //    cosign_windows is reset to zero for all validators in the set.
        let height = self.block_height;
        let jailed_r2 = reliability::check_and_jail_cosign(
            &mut self.cosign_windows,
            &mut self.reliability,
            &self.validator_set,
            height,
        );
        if jailed_r2 {
            tracing::warn!(
                height,
                "ADR 0027 règle 2 : validateur(s) jailé(s) pour taux de co-signature insuffisant"
            );
        }

        self.last_epoch_close_ts = self.current_block_ts;
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
        let minted = Amount::from_atoms(to_emit);
        self.mint_emission(producer, minted);
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

        // Fee floor for the fee-bearing types (mirrors apply_transfer / apply_anchor_state /
        // apply_module_escrow).
        if matches!(
            tx.tx_type,
            TransactionType::Transfer
                | TransactionType::AnchorState
                | TransactionType::ModuleEscrow
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
        // ADR 0035: reject oversized payloads before any state mutation so no nonce is
        // consumed. The check is cheap (a single comparison) and must precede all
        // type-specific handlers, which assume a well-bounded payload.
        if tx.payload.len() > MAX_PAYLOAD_BYTES {
            return Err(CoreError::InvalidTransaction(format!(
                "payload {} bytes exceeds protocol maximum {} bytes (ADR 0035)",
                tx.payload.len(),
                MAX_PAYLOAD_BYTES
            )));
        }
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
            TransactionType::BondValidator => self.apply_bond_validator(tx),
            TransactionType::ModuleEscrow => self.apply_module_escrow(tx),
            TransactionType::ModuleEscrowRefund => self.apply_module_escrow_refund(tx),
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
        // ADR 0009: stake carries the same flat forfait as a transfer (anti-spam
        // symmetry). The fee goes to the block producer via block_fees.
        if tx.fee < self.base_fee {
            return Err(CoreError::InvalidTransaction(format!(
                "stake fee {} is below minimum {} (ADR 0009)",
                tx.fee, self.base_fee
            )));
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
        let total_debit = tx.amount.checked_add(tx.fee).ok_or(CoreError::AmountOverflow)?;
        if account.balance < total_debit {
            return Err(CoreError::InsufficientBalance);
        }
        // Bond posting: balance −(amount+fee), staked +(amount). Fee to producer.
        // Circulation-neutral: the fee just moves from balance to block_fees.
        account.balance = account.balance.checked_sub(total_debit).unwrap();
        account.staked = account
            .staked
            .checked_add(tx.amount)
            .ok_or(CoreError::AmountOverflow)?;
        account.nonce += 1;
        self.block_fees = self.block_fees.saturating_add(tx.fee);
        self.mark_dirty(&tx.from);
        Ok(())
    }

    fn apply_unstake(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let bond_floor = Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS);
        let is_active_validator = self.validator_set.contains(&tx.from);
        let unlock_ts = self.current_block_ts.saturating_add(UNBONDING_SECS);

        // ADR 0009: cap concurrent unbonding entries per account (anti-spam).
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

        // ADR 0009: unstake carries the same flat forfait as a transfer.
        if tx.fee < self.base_fee {
            return Err(CoreError::InvalidTransaction(format!(
                "unstake fee {} is below minimum {} (ADR 0009)",
                tx.fee, self.base_fee
            )));
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
        // Sender needs liquid balance to pay the fee (the unstaked amount goes to
        // pending_unbonds, not back to balance immediately).
        if account.balance < tx.fee {
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
        account.balance = account.balance.checked_sub(tx.fee).unwrap();
        account.nonce += 1;
        // The withdrawn amount does NOT return to the balance now: it enters the
        // unbonding delay and stays slashable until `unlock_ts`. Circulation-neutral.
        self.pending_unbonds.push(PendingUnbond {
            address: tx.from,
            amount: tx.amount,
            unlock_ts,
        });
        self.block_fees = self.block_fees.saturating_add(tx.fee);
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
                if self.account_staked(&addr).atoms() < MIN_VALIDATOR_BOND_ATOMS {
                    return Err(CoreError::InvalidTransaction(
                        "candidate validator has not posted the minimum bond".to_string(),
                    ));
                }
                self.validator_set.add(addr);
                tracing::info!(%addr, "Admin: validator added");
            }
            GovernanceAction::RemoveValidator(addr) => {
                if !self.validator_set.contains(&addr) {
                    return Err(CoreError::InvalidTransaction(
                        "address is not a validator".to_string(),
                    ));
                }
                if self.validator_exit_queue.contains(&addr) {
                    return Err(CoreError::InvalidTransaction(
                        "validator is already in the exit queue".to_string(),
                    ));
                }
                // ADR 0036: compute the effective set size after all pending exits and
                // this new one are eventually processed, then enforce the floor.
                let effective_after = self
                    .validator_set
                    .len()
                    .saturating_sub(self.validator_exit_queue.len())
                    .saturating_sub(1);
                if effective_after < MIN_VALIDATOR_SET_SIZE {
                    return Err(CoreError::InvalidTransaction(format!(
                        "removing this validator would drop the effective set below \
                         the minimum of {MIN_VALIDATOR_SET_SIZE}"
                    )));
                }
                // Enqueue instead of immediately removing: the validator stays active
                // (and slashable) until processed at the next window boundary.
                self.validator_exit_queue.push(addr);
                tracing::info!(%addr, "Admin: validator queued for exit (ADR 0036)");
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
            GovernanceAction::UpdateMinValidatorBond { atoms } => {
                // ADR 0038: hard bounds first (immutable guard rails).
                if atoms < MIN_BOND_HARD_FLOOR {
                    return Err(CoreError::InvalidTransaction(format!(
                        "bond floor {atoms} is below the immutable hard floor {MIN_BOND_HARD_FLOOR}"
                    )));
                }
                if atoms > MAX_BOND_HARD_CAP {
                    return Err(CoreError::InvalidTransaction(format!(
                        "bond floor {atoms} exceeds the immutable hard cap {MAX_BOND_HARD_CAP}"
                    )));
                }
                // Rate-limit: change must be within ±BOND_STEP_BPS (25 %) of current value.
                let current = self.min_validator_bond_atoms;
                let min_allowed = current - current * BOND_STEP_BPS / 10_000;
                let max_allowed = current + current * BOND_STEP_BPS / 10_000;
                if atoms < min_allowed || atoms > max_allowed {
                    return Err(CoreError::InvalidTransaction(format!(
                        "bond change from {current} to {atoms} exceeds the ±{BOND_STEP_BPS} bps step limit"
                    )));
                }
                // Cooldown between modifications.
                if self.current_block_ts < self.last_bond_change_ts + BOND_COOLDOWN_SECS {
                    let remaining = (self.last_bond_change_ts + BOND_COOLDOWN_SECS)
                        .saturating_sub(self.current_block_ts);
                    return Err(CoreError::InvalidTransaction(format!(
                        "bond cooldown not elapsed — {remaining}s remaining"
                    )));
                }
                self.min_validator_bond_atoms = atoms;
                self.last_bond_change_ts = self.current_block_ts;
                tracing::info!(atoms, "Admin: min validator bond updated (ADR 0038)");
            }
            GovernanceAction::UpdateActiveSetSize { new_size } => {
                // ADR 0038: hard floor (immutable).
                if new_size < MIN_ACTIVE_SET_SIZE {
                    return Err(CoreError::InvalidTransaction(format!(
                        "active set size {new_size} is below the immutable floor {MIN_ACTIVE_SET_SIZE}"
                    )));
                }
                // Change limited to exactly ±ACTIVE_SET_STEP.
                let current = self.active_set_size;
                let diff = current.abs_diff(new_size);
                if diff == 0 {
                    return Err(CoreError::InvalidTransaction(
                        "active set size is unchanged".to_string(),
                    ));
                }
                if diff != ACTIVE_SET_STEP {
                    return Err(CoreError::InvalidTransaction(format!(
                        "active set size change must be exactly ±{ACTIVE_SET_STEP} (requested {diff})"
                    )));
                }
                // Cooldown between modifications.
                if self.current_block_ts
                    < self.last_active_set_size_change_ts + ACTIVE_SET_COOLDOWN_SECS
                {
                    let remaining = (self.last_active_set_size_change_ts + ACTIVE_SET_COOLDOWN_SECS)
                        .saturating_sub(self.current_block_ts);
                    return Err(CoreError::InvalidTransaction(format!(
                        "active set size cooldown not elapsed — {remaining}s remaining"
                    )));
                }
                self.active_set_size = new_size;
                self.last_active_set_size_change_ts = self.current_block_ts;
                tracing::info!(new_size, "Admin: active set size updated (ADR 0038)");
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
                // Clean up fee schedule (if any) when module is deregistered.
                self.module_fee_schedules.remove(module_id);
            }
            // ADR 0039 — new ops appended; never reorder.
            ModuleOp::SetFeeSchedule {
                module_id,
                fee_schedule,
            } => {
                let entry = self
                    .modules
                    .get(module_id)
                    .ok_or_else(|| CoreError::InvalidTransaction("unknown module".to_string()))?;
                if entry.operator != operator {
                    return Err(CoreError::Unauthorized);
                }
                // Validate that share allocations do not exceed 100%.
                let total_bps: u32 =
                    fee_schedule.recipients.iter().map(|r| r.share_bps as u32).sum();
                if total_bps > 10_000 {
                    return Err(CoreError::InvalidTransaction(
                        "fee schedule share_bps sum exceeds 10 000 (100%)".to_string(),
                    ));
                }
                if balance < fee || balance - fee < ed {
                    return Err(CoreError::InsufficientBalance);
                }
                let acc = self.accounts.get_mut(&operator).expect("checked above");
                acc.balance = Amount::from_atoms(balance - fee);
                acc.nonce += 1;
                self.module_fee_schedules.insert(*module_id, fee_schedule.clone());
            }
            ModuleOp::EscrowRelease { escrow_id } => {
                // Snapshot escrow data before any mutation (borrow split).
                let (escrow_module_id, amount_atoms) = {
                    let e = self.pending_escrows.get(escrow_id).ok_or_else(|| {
                        CoreError::InvalidTransaction("unknown escrow id".to_string())
                    })?;
                    (e.module_id, e.amount_atoms)
                };
                // Verify operator owns the module this escrow targets.
                let (fee_schedule_opt, module_op_addr) = {
                    let entry = self.modules.get(&escrow_module_id).ok_or_else(|| {
                        CoreError::InvalidTransaction(
                            "module not found for escrow release".to_string(),
                        )
                    })?;
                    if entry.operator != operator {
                        return Err(CoreError::Unauthorized);
                    }
                    let fs = self.module_fee_schedules.get(&escrow_module_id).cloned();
                    (fs, entry.operator)
                };
                // Debit the AnchorState fee from the operator.
                if balance < fee || balance - fee < ed {
                    return Err(CoreError::InsufficientBalance);
                }
                {
                    let acc = self.accounts.get_mut(&operator).expect("checked above");
                    acc.balance = Amount::from_atoms(balance - fee);
                    acc.nonce += 1;
                }
                // Remove the escrow entry.
                self.pending_escrows.remove(escrow_id);
                // Distribute escrow.amount_atoms per fee schedule (ADR 0039 §2.2 step 3).
                let mut distributed = 0u128;
                if let Some(sched) = fee_schedule_opt {
                    for r in &sched.recipients {
                        let share = amount_atoms * r.share_bps as u128 / BPS_DENOM;
                        distributed = distributed.saturating_add(share);
                        if share > 0 {
                            self.credit(&r.address, Amount::from_atoms(share));
                        }
                    }
                    let residual = amount_atoms.saturating_sub(distributed);
                    if residual > 0 {
                        self.credit(&sched.operator_address, Amount::from_atoms(residual));
                    }
                } else {
                    // No fee schedule: entire escrow amount goes to the module operator.
                    if amount_atoms > 0 {
                        self.credit(&module_op_addr, Amount::from_atoms(amount_atoms));
                    }
                }
            }
        }

        // The fee changes hands into the block pool → credited to the producer at settle.
        self.block_fees = self.block_fees.saturating_add(tx.fee);
        self.mark_dirty(&operator);
        Ok(())
    }

    /// Applies an `Unjail` transaction (ADR 0027): removes the jailed flag from a validator
    /// that has served the mandatory cooldown period.
    ///
    /// No fee, no amount — nonce is consumed only on success.  The sender must currently be
    /// jailed and `block_height >= jailed_until` (UNJAIL_COOLDOWN_HEIGHTS blocks since
    /// jailing).  Trying to unjail before the cooldown or when not jailed returns an error
    /// without consuming the nonce.
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
        let current_height = self.block_height;
        if !reliability::try_unjail(&mut self.reliability, &tx.from, current_height) {
            return Err(CoreError::InvalidTransaction(
                "not jailed or unjail cooldown not elapsed (ADR 0027)".to_string(),
            ));
        }
        self.accounts.get_mut(&tx.from).unwrap().nonce += 1;
        self.mark_dirty(&tx.from);
        tracing::info!(
            validator = %tx.from,
            height = current_height,
            "ADR 0027: validator unjailed"
        );
        Ok(())
    }

    /// Applies a `BondValidator` transaction (ADR 0038): moves `amount` atoms from
    /// `balance` to `staked` and inserts the sender into the Open PoA admission queue
    /// in `Warmup` status.
    ///
    /// The sender must not already be in the validator pool or admin-admitted validator
    /// set, and must not be permanently banned.  After VALIDATOR_WARMUP_EPOCHS epochs the
    /// entry becomes eligible for the active set during the next `tick_epoch_close`.
    fn apply_bond_validator(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        // ADR 0038: bond floor is now governable; use the current runtime value.
        if tx.amount.atoms() < self.min_validator_bond_atoms {
            return Err(CoreError::InvalidTransaction(format!(
                "bond below minimum validator bond ({} atoms)",
                self.min_validator_bond_atoms
            )));
        }
        let expected_fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        if tx.fee < expected_fee {
            return Err(CoreError::InvalidTransaction("fee below floor".to_string()));
        }
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
        if self.banned_validator_keys.contains(&tx.from) {
            return Err(CoreError::InvalidTransaction(
                "address is permanently banned from the validator set".to_string(),
            ));
        }
        if self.validator_pool.contains_key(&tx.from) {
            return Err(CoreError::InvalidTransaction(
                "address is already in the validator pool".to_string(),
            ));
        }
        if self.validator_set.contains(&tx.from) {
            return Err(CoreError::InvalidTransaction(
                "address is already a validator (admin-admitted)".to_string(),
            ));
        }
        let total = tx.amount.checked_add(tx.fee).ok_or(CoreError::AmountOverflow)?;
        if account.balance < total {
            return Err(CoreError::InsufficientBalance);
        }
        let remaining = account.balance.checked_sub(total).unwrap_or(Amount::ZERO);
        if remaining.atoms() > 0
            && remaining.atoms() < EXISTENTIAL_DEPOSIT_ATOMS
            && account.staked == Amount::ZERO
        {
            return Err(CoreError::BelowExistentialDeposit);
        }
        let now_ts = self.current_block_ts;
        let bond_atoms = tx.amount.atoms();
        {
            let account = self.accounts.get_mut(&tx.from).unwrap();
            account.balance = account.balance.checked_sub(total).unwrap();
            account.staked = account.staked.saturating_add(tx.amount);
            account.nonce += 1;
        }
        self.block_fees = self.block_fees.saturating_add(tx.fee);
        self.validator_pool
            .insert(tx.from, ValidatorPoolEntry::new(bond_atoms, now_ts));
        self.mark_dirty(&tx.from);
        tracing::info!(
            validator = %tx.from,
            bond_atoms,
            "ADR 0038: validator bonded, entering warmup"
        );
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

    /// Applies a `ModuleEscrow` transaction (ADR 0039): locks `tx.amount` atoms from the
    /// client in `pending_escrows` for a bonded module service.
    ///
    /// Validations (all before mutation):
    /// - fee ≥ base_fee; module exists; timeout in bounds; amount > 0
    /// - ≤ MODULE_ESCROW_MAX_OPEN open escrows per (client, module) pair
    /// - balance ≥ amount + fee; post-debit balance ≥ ED (or exactly 0)
    fn apply_module_escrow(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let payload: ModuleEscrowPayload = bincode::deserialize(&tx.payload).map_err(|_| {
            CoreError::InvalidTransaction("malformed ModuleEscrow payload".to_string())
        })?;

        if tx.fee < self.base_fee {
            return Err(CoreError::InvalidTransaction(format!(
                "fee {} is below minimum {}",
                tx.fee, self.base_fee
            )));
        }
        if !self.modules.contains_key(&payload.module_id) {
            return Err(CoreError::InvalidTransaction("module not found".to_string()));
        }
        if payload.timeout_secs < MODULE_ESCROW_MIN_TIMEOUT_SECS {
            return Err(CoreError::InvalidTransaction(
                "escrow timeout below minimum".to_string(),
            ));
        }
        if payload.timeout_secs > MODULE_ESCROW_MAX_TIMEOUT_SECS {
            return Err(CoreError::InvalidTransaction(
                "escrow timeout above maximum".to_string(),
            ));
        }
        if tx.amount == Amount::ZERO {
            return Err(CoreError::InvalidTransaction(
                "escrow amount must be non-zero".to_string(),
            ));
        }

        let client = tx.from;
        let (nonce, balance, staked) = {
            let acc = self
                .accounts
                .get(&client)
                .ok_or(CoreError::InsufficientBalance)?;
            (acc.nonce, acc.balance.atoms(), acc.staked.atoms())
        };
        if nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: nonce,
                got: tx.nonce,
            });
        }

        // Anti-spam: bound open escrows per (client, module_id).
        let open_count = self
            .pending_escrows
            .values()
            .filter(|e| e.client == client && e.module_id == payload.module_id)
            .count();
        if open_count >= MODULE_ESCROW_MAX_OPEN {
            return Err(CoreError::InvalidTransaction(
                "too many open escrows for this (client, module) pair".to_string(),
            ));
        }

        // Balance check: must cover amount + fee, leaving ≥ ED or exactly 0.
        let amount = tx.amount.atoms();
        let fee = tx.fee.atoms();
        let total = amount.checked_add(fee).ok_or(CoreError::AmountOverflow)?;
        if balance < total {
            return Err(CoreError::InsufficientBalance);
        }
        let remaining = balance - total;
        if remaining > 0 && remaining < EXISTENTIAL_DEPOSIT_ATOMS && staked == 0 {
            return Err(CoreError::BelowExistentialDeposit);
        }

        // Compute escrow_id = sha256(client || module_id || nonce_le8) — deterministic.
        let mut preimage = Vec::with_capacity(20 + 32 + 8);
        preimage.extend_from_slice(client.as_bytes());
        preimage.extend_from_slice(&payload.module_id);
        preimage.extend_from_slice(&tx.nonce.to_le_bytes());
        let escrow_id = sha256(&preimage);

        if self.pending_escrows.contains_key(&escrow_id) {
            return Err(CoreError::InvalidTransaction(
                "escrow id collision (nonce reuse?)".to_string(),
            ));
        }

        // Mutate: debit client balance.
        {
            let acc = self.accounts.get_mut(&client).expect("checked above");
            acc.balance = Amount::from_atoms(remaining);
            acc.nonce += 1;
        }
        self.block_fees = self.block_fees.saturating_add(tx.fee);
        self.mark_dirty(&client);

        // Reap account if drained to zero (no staked, no unbonds).
        if remaining == 0 {
            self.reap_if_empty(&client);
        }

        self.pending_escrows.insert(
            escrow_id,
            EscrowEntry {
                escrow_id,
                client,
                module_id: payload.module_id,
                amount_atoms: amount,
                created_ts: self.current_block_ts,
                timeout_secs: payload.timeout_secs,
                service_params_hash: payload.service_params_hash,
            },
        );
        tracing::info!(
            client = %client,
            module_id = ?payload.module_id,
            amount_atoms = amount,
            "ADR 0039: escrow opened"
        );
        Ok(())
    }

    /// Applies a `ModuleEscrowRefund` transaction (ADR 0039): returns the locked atoms to
    /// the original client after the agreed timeout has elapsed.
    ///
    /// No fee is charged. The escrow must exist, the sender must be the original client,
    /// and `block_timestamp ≥ created_ts + timeout_secs`.
    fn apply_module_escrow_refund(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let escrow_id: Hash32 = bincode::deserialize(&tx.payload).map_err(|_| {
            CoreError::InvalidTransaction("malformed escrow refund payload".to_string())
        })?;

        let (amount_atoms, deadline) = {
            let e = self.pending_escrows.get(&escrow_id).ok_or_else(|| {
                CoreError::InvalidTransaction("escrow not found".to_string())
            })?;
            if e.client != tx.from {
                return Err(CoreError::Unauthorized);
            }
            (
                e.amount_atoms,
                e.created_ts.saturating_add(e.timeout_secs),
            )
        };

        if self.current_block_ts < deadline {
            return Err(CoreError::InvalidTransaction(
                "escrow timeout has not elapsed yet".to_string(),
            ));
        }

        // Nonce check (account may not exist if it was reaped after the escrow creation;
        // in that case admission_check already rejected the tx before we get here).
        let nonce = self
            .accounts
            .get(&tx.from)
            .map(|a| a.nonce)
            .unwrap_or(0);
        if nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: nonce,
                got: tx.nonce,
            });
        }

        // Remove escrow.
        self.pending_escrows.remove(&escrow_id);

        // Consume nonce if account exists.
        if let Some(acc) = self.accounts.get_mut(&tx.from) {
            acc.nonce += 1;
            self.mark_dirty(&tx.from);
        }

        // Credit refund (creates the account if it was reaped).
        self.credit(&tx.from, Amount::from_atoms(amount_atoms));
        tracing::info!(
            client = %tx.from,
            amount_atoms,
            "ADR 0039: escrow refunded (timeout)"
        );
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
    use vinx_core::{BlockHeader, BlockSignature, SlashEvidence};

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
        assert_eq!(s.accounts[&producer].balance.atoms(), expected);
        assert_eq!(s.emitted_atoms, expected);
        // Supply invariant: circulating == emitted (no pot, no destroyed).
        assert_eq!(s.circulating_supply.atoms(), s.emitted_atoms);
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
        assert_eq!(s.accounts[&producer].balance, emission);
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
        let fee = s.base_fee;
        s.credit_for_test(addr, Amount::from_vinx(1_000));
        let balance_start = s.accounts[&addr].balance;
        s.apply_transaction(&Transaction::new_stake(
            &kp,
            Amount::from_vinx(500),
            fee,
            0,
        ))
        .unwrap();
        assert_eq!(s.accounts[&addr].staked, Amount::from_vinx(500));
        let balance_after_stake = s.accounts[&addr].balance;

        // Unstake at ts = 1000 → enters the unbonding delay, NOT credited yet.
        s.set_block_context(1_000);
        s.apply_transaction(&Transaction::new_unstake(
            &kp,
            Amount::from_vinx(500),
            fee,
            1,
        ))
        .unwrap();
        assert_eq!(s.accounts[&addr].staked, Amount::ZERO);
        // Balance decreases by the unstake fee; the 500 VINX is in pending_unbonds.
        let balance_after_unstake = balance_after_stake.checked_sub(fee).unwrap();
        assert_eq!(s.accounts[&addr].balance, balance_after_unstake);
        assert_eq!(s.pending_unbonds.len(), 1);

        // Just before unlock: nothing matures. Use a separate producer so addr stays clean.
        s.settle_block(&producer, 1, 1_000 + UNBONDING_SECS - 1);
        assert_eq!(s.pending_unbonds.len(), 1);
        // At unlock: the 500 VINX bond returns to addr's balance.
        s.settle_block(&producer, 2, 1_000 + UNBONDING_SECS);
        assert!(s.pending_unbonds.is_empty());
        // Original 1000 VINX minus two fees (stake + unstake) plus 500 VINX returned.
        let expected_final = balance_start.checked_sub(fee).unwrap().checked_sub(fee).unwrap();
        assert_eq!(s.accounts[&addr].balance, expected_final);
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
        let (_, producer) = kp_addr(); // separate producer — fees/emission must not land on addr
        let fee = s.base_fee;
        let stake = Amount::from_vinx(10);
        // Credit enough to cover the stake amount plus fees for both stake and unstake.
        let two_fees = fee.checked_add(fee).unwrap();
        s.credit_for_test(addr, stake.checked_add(two_fees).unwrap());
        s.apply_transaction(&Transaction::new_stake(&kp, stake, fee, 0))
            .unwrap();
        // After stake: balance == fee (the unstake fee reserve).
        assert_eq!(s.accounts[&addr].balance, fee);
        s.set_block_context(1_000);
        s.apply_transaction(&Transaction::new_unstake(&kp, stake, fee, 1))
            .unwrap();
        // balance 0 (fee consumed), staked 0, but a pending unbond exists → account must remain.
        assert!(s.get_account(&addr).is_some());
        assert_eq!(s.accounts[&addr].balance, Amount::ZERO);
        assert_eq!(s.accounts[&addr].staked, Amount::ZERO);
        // After maturation the funds return. Use a separate producer so fees/emission
        // don't confound the balance check on addr.
        s.settle_block(&producer, 1, 1_000 + UNBONDING_SECS);
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
        let fee = s.base_fee;
        s.credit_for_test(addr, Amount::from_vinx(1_000));
        s.apply_transaction(&Transaction::new_stake(
            &kp,
            Amount::from_vinx(100),
            fee,
            0,
        ))
        .unwrap();
        s.set_block_context(1_000);
        // Unstake 1 VINX up to the cap — all accepted.
        for i in 0..MAX_PENDING_UNBONDS_PER_ACCOUNT {
            let tx =
                Transaction::new_unstake(&kp, Amount::from_vinx(1), fee, (i + 1) as u64);
            s.apply_transaction(&tx).unwrap();
        }
        assert_eq!(s.pending_unbonds.len(), MAX_PENDING_UNBONDS_PER_ACCOUNT);
        // One more → rejected (anti-spam cap, not balance).
        let over = Transaction::new_unstake(
            &kp,
            Amount::from_vinx(1),
            fee,
            (MAX_PENDING_UNBONDS_PER_ACCOUNT + 1) as u64,
        );
        assert!(s.apply_transaction(&over).is_err());
    }

    #[test]
    fn test_active_validator_cannot_unstake_below_bond() {
        let mut s = WorldState::new();
        let (kp, addr) = kp_addr();
        let fee = s.base_fee;
        let bond = MIN_VALIDATOR_BOND_ATOMS;
        s.credit_for_test(addr, Amount::from_atoms(bond * 2));
        s.apply_transaction(&Transaction::new_stake(
            &kp,
            Amount::from_atoms(bond),
            fee,
            0,
        ))
        .unwrap();
        s.validator_set = ValidatorSet::single(addr);
        s.set_block_context(1_000);
        // Unstaking any of the bond would drop below the minimum → rejected.
        let bad = Transaction::new_unstake(&kp, Amount::from_atoms(bond / 2), fee, 1);
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
        // 10% bounty to the reporter (100 VINX), 90% (900 VINX) moved to the epoch pot.
        assert_eq!(s.accounts[&reporter].balance, Amount::from_vinx(110));
        assert_eq!(s.epoch_dist_emission_pot, Amount::from_vinx(900));
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
        // Amount check fires before the fee check — Amount::ZERO fee is fine here.
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

    // ─── ADR 0027: Unjail ────────────────────────────────────────────────────

    fn jailed_state() -> (WorldState, KeyPair, Address) {
        use vinx_core::reliability::{ValidatorReliability, UNJAIL_COOLDOWN_HEIGHTS};
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_vinx(10));
        // Manually jail at height 1; cooldown expires at 1 + UNJAIL_COOLDOWN_HEIGHTS.
        s.reliability.insert(
            addr,
            ValidatorReliability {
                missed_proposals: 0,
                jailed_until: Some(1 + UNJAIL_COOLDOWN_HEIGHTS),
            },
        );
        (s, kp, addr)
    }

    #[test]
    fn test_unjail_happy_path() {
        use vinx_core::reliability::UNJAIL_COOLDOWN_HEIGHTS;
        let (mut s, kp, addr) = jailed_state();
        // Advance height past cooldown.
        s.block_height = 1 + UNJAIL_COOLDOWN_HEIGHTS;
        let tx = Transaction::new_unjail(&kp, 0);
        s.apply_transaction(&tx).unwrap();
        // No longer jailed and nonce advanced.
        assert!(!s.reliability.get(&addr).unwrap().is_jailed());
        assert_eq!(s.accounts[&addr].nonce, 1);
    }

    #[test]
    fn test_unjail_too_early_rejected() {
        use vinx_core::reliability::UNJAIL_COOLDOWN_HEIGHTS;
        let (mut s, kp, _addr) = jailed_state();
        // Height still within cooldown.
        s.block_height = UNJAIL_COOLDOWN_HEIGHTS / 2;
        let tx = Transaction::new_unjail(&kp, 0);
        assert!(s.apply_transaction(&tx).is_err());
    }

    #[test]
    fn test_unjail_not_jailed_rejected() {
        let mut s = WorldState::new();
        let (kp, addr) = kp_addr();
        s.credit_for_test(addr, Amount::from_vinx(10));
        // Not jailed at all.
        let tx = Transaction::new_unjail(&kp, 0);
        assert!(s.apply_transaction(&tx).is_err());
    }

    // ─── ADR 0027 règle 2 : co-signature participation jailing ───────────────

    #[test]
    fn test_cosign_participation_jails_absent_validator() {
        use vinx_core::reliability::MIN_COSIGN_CHECK_ELIGIBLE;
        use vinx_core::ValidatorSet;
        let (_kp, addr) = kp_addr();
        let mut s = WorldState::new();
        // Put the validator in the classic set.
        s.validator_set = ValidatorSet::single(addr);

        // Simulate MIN_COSIGN_CHECK_ELIGIBLE blocks where the validator never co-signs.
        for _ in 0..MIN_COSIGN_CHECK_ELIGIBLE {
            s.record_block_cosigns(&[]);
        }

        // Close the epoch — should jail the validator for 0% co-sign rate.
        s.tick_epoch_close();
        assert!(
            s.reliability.get(&addr).map_or(false, |r| r.is_jailed()),
            "validator should be jailed after 0% co-sign rate over the epoch"
        );
    }

    #[test]
    fn test_cosign_participation_no_jail_above_threshold() {
        use vinx_core::reliability::MIN_COSIGN_CHECK_ELIGIBLE;
        use vinx_core::ValidatorSet;
        let (_kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.validator_set = ValidatorSet::single(addr);

        // Simulate MIN_COSIGN_CHECK_ELIGIBLE blocks where the validator always co-signs.
        for _ in 0..MIN_COSIGN_CHECK_ELIGIBLE {
            s.record_block_cosigns(&[addr]);
        }

        s.tick_epoch_close();
        assert!(
            !s.reliability.get(&addr).map_or(false, |r| r.is_jailed()),
            "validator should NOT be jailed after 100% co-sign rate"
        );
    }

    #[test]
    fn test_cosign_participation_no_jail_below_min_eligible() {
        use vinx_core::reliability::MIN_COSIGN_CHECK_ELIGIBLE;
        use vinx_core::ValidatorSet;
        let (_kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.validator_set = ValidatorSet::single(addr);

        // Fewer blocks than the minimum — not enough data to jail.
        for _ in 0..(MIN_COSIGN_CHECK_ELIGIBLE - 1) {
            s.record_block_cosigns(&[]);
        }

        s.tick_epoch_close();
        assert!(
            !s.reliability.get(&addr).map_or(false, |r| r.is_jailed()),
            "validator should NOT be jailed when window has fewer than MIN_COSIGN_CHECK_ELIGIBLE blocks"
        );
    }

    // ─── ADR 0038: BondValidator ─────────────────────────────────────────────

    fn bond_state() -> (WorldState, KeyPair, Address) {
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        let bond = Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS);
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        s.credit_for_test(addr, bond.saturating_add(fee).saturating_add(Amount::from_vinx(1)));
        (s, kp, addr)
    }

    #[test]
    fn test_bond_validator_happy_path() {
        let (mut s, kp, addr) = bond_state();
        let bond = Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS);
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let tx = Transaction::new_bond_validator(&kp, bond, fee, 0);
        s.apply_transaction(&tx).unwrap();
        assert!(s.validator_pool.contains_key(&addr));
        assert_eq!(s.accounts[&addr].staked, bond);
        assert_eq!(s.accounts[&addr].nonce, 1);
        // Pool entry should be in Warmup status.
        let entry = &s.validator_pool[&addr];
        assert!(matches!(entry.status, vinx_core::validator_pool::PoolStatus::Warmup { .. }));
    }

    #[test]
    fn test_bond_validator_below_minimum_rejected() {
        let (mut s, kp, _addr) = bond_state();
        let bond = Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS - 1);
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let tx = Transaction::new_bond_validator(&kp, bond, fee, 0);
        assert!(s.apply_transaction(&tx).is_err());
    }

    #[test]
    fn test_bond_validator_already_in_pool_rejected() {
        let (mut s, kp, addr) = bond_state();
        let bond = Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS);
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        // First bond succeeds.
        s.credit_for_test(addr, bond.saturating_add(fee));
        let tx = Transaction::new_bond_validator(&kp, bond, fee, 0);
        s.apply_transaction(&tx).unwrap();
        // Second bond rejected — already in pool.
        s.credit_for_test(addr, bond.saturating_add(fee));
        let tx2 = Transaction::new_bond_validator(&kp, bond, fee, 1);
        assert!(s.apply_transaction(&tx2).is_err());
    }

    #[test]
    fn test_bond_validator_banned_key_rejected() {
        let (mut s, kp, addr) = bond_state();
        s.banned_validator_keys.insert(addr);
        let bond = Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS);
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let tx = Transaction::new_bond_validator(&kp, bond, fee, 0);
        assert!(s.apply_transaction(&tx).is_err());
    }

    #[test]
    fn test_bond_validator_admin_admitted_rejected() {
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _admin_addr) = admin_state();
        let (cand_kp, cand_addr) = kp_addr();
        // Give candidate bond and credit.
        s.set_staked_for_test(&cand_addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
        s.credit_for_test(cand_addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS + DEFAULT_FEE_FLOOR_ATOMS + 1_000_000));
        // Admin-admit candidate.
        let add_tx = Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::AddValidator(cand_addr),
            0,
        );
        s.apply_transaction(&add_tx).unwrap();
        assert!(s.validator_set.contains(&cand_addr));
        // Now candidate tries to self-bond → rejected (already admin-admitted).
        let bond = Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS);
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let bond_tx = Transaction::new_bond_validator(&cand_kp, bond, fee, 0);
        assert!(s.apply_transaction(&bond_tx).is_err());
    }

    // ─── ADR 0039: module escrow ─────────────────────────────────────────────

    /// Returns (state, operator_keypair, operator_addr, module_id) with a registered module.
    fn escrow_state() -> (WorldState, KeyPair, Address, Hash32) {
        let (mut s, op_kp, op_addr) = operator_state();
        let fee = s.base_fee;
        let module_id = [0xABu8; 32];
        let op = ModuleOp::Register {
            module_id,
            bond_atoms: MIN_MODULE_BOND_ATOMS,
        };
        s.apply_transaction(&Transaction::new_anchor_state(&op_kp, &op, fee, 0))
            .unwrap();
        (s, op_kp, op_addr, module_id)
    }

    #[test]
    fn test_escrow_happy_path_debits_client_and_stores_entry() {
        use vinx_core::ModuleEscrowPayload;

        let (mut s, _op_kp, _op_addr, module_id) = escrow_state();
        let client_kp = KeyPair::generate();
        let client_addr = Address::from_public_key(&client_kp.public_key());
        let escrow_amount = Amount::from_vinx(100);
        let fee = s.base_fee;
        s.credit_for_test(client_addr, Amount::from_vinx(500));
        let balance_before = s.account_balance(&client_addr);

        let payload = ModuleEscrowPayload {
            module_id,
            timeout_secs: MODULE_ESCROW_MIN_TIMEOUT_SECS,
            service_params_hash: [0u8; 32],
        };
        let tx = Transaction::new_module_escrow(&client_kp, &payload, escrow_amount, fee, 0);
        s.apply_transaction(&tx).unwrap();

        let expected_balance = balance_before
            .checked_sub(escrow_amount)
            .unwrap()
            .checked_sub(fee)
            .unwrap();
        assert_eq!(s.account_balance(&client_addr), expected_balance);
        assert_eq!(s.pending_escrows.len(), 1);
        let entry = s.pending_escrows.values().next().unwrap();
        assert_eq!(entry.client, client_addr);
        assert_eq!(entry.module_id, module_id);
        assert_eq!(entry.amount_atoms, escrow_amount.atoms());
        assert_eq!(entry.timeout_secs, MODULE_ESCROW_MIN_TIMEOUT_SECS);
    }

    #[test]
    fn test_escrow_for_unknown_module_rejected() {
        use vinx_core::ModuleEscrowPayload;

        let (mut s, _op_kp, _op_addr, _module_id) = escrow_state();
        let client_kp = KeyPair::generate();
        let client_addr = Address::from_public_key(&client_kp.public_key());
        s.credit_for_test(client_addr, Amount::from_vinx(500));

        let payload = ModuleEscrowPayload {
            module_id: [0xFFu8; 32], // non-existent
            timeout_secs: MODULE_ESCROW_MIN_TIMEOUT_SECS,
            service_params_hash: [0u8; 32],
        };
        let fee = s.base_fee;
        let tx = Transaction::new_module_escrow(&client_kp, &payload, Amount::from_vinx(10), fee, 0);
        assert!(s.apply_transaction(&tx).is_err());
        assert!(s.pending_escrows.is_empty());
    }

    #[test]
    fn test_escrow_refund_after_timeout_succeeds() {
        use vinx_core::ModuleEscrowPayload;

        let (mut s, _op_kp, _op_addr, module_id) = escrow_state();
        let client_kp = KeyPair::generate();
        let client_addr = Address::from_public_key(&client_kp.public_key());
        let escrow_amount = Amount::from_vinx(100);
        let fee = s.base_fee;
        s.credit_for_test(client_addr, Amount::from_vinx(500));

        let payload = ModuleEscrowPayload {
            module_id,
            timeout_secs: MODULE_ESCROW_MIN_TIMEOUT_SECS,
            service_params_hash: [0u8; 32],
        };
        // Create escrow at ts = 0.
        s.set_block_context(0);
        let tx = Transaction::new_module_escrow(&client_kp, &payload, escrow_amount, fee, 0);
        s.apply_transaction(&tx).unwrap();
        let escrow_id = s.pending_escrows.keys().copied().next().unwrap();
        let balance_after_escrow = s.account_balance(&client_addr);

        // Advance past the timeout.
        s.set_block_context(MODULE_ESCROW_MIN_TIMEOUT_SECS + 1);
        let refund_tx = Transaction::new_module_escrow_refund(&client_kp, escrow_id, 1);
        s.apply_transaction(&refund_tx).unwrap();

        // Escrow removed, amount returned to client.
        assert!(s.pending_escrows.is_empty());
        assert_eq!(
            s.account_balance(&client_addr),
            balance_after_escrow.checked_add(escrow_amount).unwrap()
        );
    }

    #[test]
    fn test_escrow_refund_before_timeout_rejected() {
        use vinx_core::ModuleEscrowPayload;

        let (mut s, _op_kp, _op_addr, module_id) = escrow_state();
        let client_kp = KeyPair::generate();
        let client_addr = Address::from_public_key(&client_kp.public_key());
        s.credit_for_test(client_addr, Amount::from_vinx(500));

        let payload = ModuleEscrowPayload {
            module_id,
            timeout_secs: MODULE_ESCROW_MIN_TIMEOUT_SECS,
            service_params_hash: [0u8; 32],
        };
        s.set_block_context(1_000);
        let fee = s.base_fee;
        let tx = Transaction::new_module_escrow(&client_kp, &payload, Amount::from_vinx(50), fee, 0);
        s.apply_transaction(&tx).unwrap();
        let escrow_id = s.pending_escrows.keys().copied().next().unwrap();

        // One second before the deadline.
        s.set_block_context(1_000 + MODULE_ESCROW_MIN_TIMEOUT_SECS - 1);
        let refund_tx = Transaction::new_module_escrow_refund(&client_kp, escrow_id, 1);
        assert!(s.apply_transaction(&refund_tx).is_err());
        assert_eq!(s.pending_escrows.len(), 1);
    }

    #[test]
    fn test_escrow_max_open_anti_spam() {
        use vinx_core::{amount::MODULE_ESCROW_MAX_OPEN, ModuleEscrowPayload};

        let (mut s, _op_kp, _op_addr, module_id) = escrow_state();
        let client_kp = KeyPair::generate();
        let client_addr = Address::from_public_key(&client_kp.public_key());
        // Fund enough for 11 escrows.
        let escrow_amount = Amount::from_vinx(10);
        let fee = s.base_fee;
        let needed = (escrow_amount.atoms() + fee.atoms()) * (MODULE_ESCROW_MAX_OPEN as u128 + 1);
        s.credit_for_test(client_addr, Amount::from_atoms(needed + EXISTENTIAL_DEPOSIT_ATOMS));

        let payload = ModuleEscrowPayload {
            module_id,
            timeout_secs: MODULE_ESCROW_MIN_TIMEOUT_SECS,
            service_params_hash: [0u8; 32],
        };
        for i in 0..MODULE_ESCROW_MAX_OPEN {
            let tx = Transaction::new_module_escrow(&client_kp, &payload, escrow_amount, fee, i as u64);
            s.apply_transaction(&tx).unwrap();
        }
        assert_eq!(s.pending_escrows.len(), MODULE_ESCROW_MAX_OPEN);

        // The (MAX+1)-th escrow must be rejected.
        let tx = Transaction::new_module_escrow(
            &client_kp,
            &payload,
            escrow_amount,
            fee,
            MODULE_ESCROW_MAX_OPEN as u64,
        );
        assert!(s.apply_transaction(&tx).is_err());
        assert_eq!(s.pending_escrows.len(), MODULE_ESCROW_MAX_OPEN);
    }

    #[test]
    fn test_escrow_release_distributes_per_fee_schedule_no_atom_loss() {
        use vinx_core::module::{FeeRecipient, FeeSchedule};
        use vinx_core::ModuleEscrowPayload;

        let (mut s, op_kp, op_addr, module_id) = escrow_state();
        let fee = s.base_fee;

        // Set a fee schedule: recipient1 gets 30%, recipient2 gets 20%, residual (~50%) to operator.
        let r1_kp = KeyPair::generate();
        let r1_addr = Address::from_public_key(&r1_kp.public_key());
        let r2_kp = KeyPair::generate();
        let r2_addr = Address::from_public_key(&r2_kp.public_key());
        let sched = FeeSchedule {
            base_fee_atoms: 0,
            recipients: vec![
                FeeRecipient { address: r1_addr, share_bps: 3_000 },
                FeeRecipient { address: r2_addr, share_bps: 2_000 },
            ],
            operator_address: op_addr,
        };
        let set_op = ModuleOp::SetFeeSchedule { module_id, fee_schedule: sched };
        s.apply_transaction(&Transaction::new_anchor_state(&op_kp, &set_op, fee, 1))
            .unwrap();

        // Create an escrow as client.
        let client_kp = KeyPair::generate();
        let client_addr = Address::from_public_key(&client_kp.public_key());
        let escrow_atoms = 10_001u128; // not evenly divisible by BPS shares
        s.credit_for_test(client_addr, Amount::from_atoms(escrow_atoms + fee.atoms() + EXISTENTIAL_DEPOSIT_ATOMS));
        let payload = ModuleEscrowPayload {
            module_id,
            timeout_secs: MODULE_ESCROW_MIN_TIMEOUT_SECS,
            service_params_hash: [0u8; 32],
        };
        let escrow_tx = Transaction::new_module_escrow(
            &client_kp,
            &payload,
            Amount::from_atoms(escrow_atoms),
            fee,
            0,
        );
        s.apply_transaction(&escrow_tx).unwrap();
        let escrow_id = s.pending_escrows.keys().copied().next().unwrap();

        let op_balance_before = s.account_balance(&op_addr);

        // Release escrow via AnchorState.
        let release_op = ModuleOp::EscrowRelease { escrow_id };
        s.apply_transaction(&Transaction::new_anchor_state(&op_kp, &release_op, fee, 2))
            .unwrap();

        // Verify no escrow atom is lost: r1 + r2 + residual == escrow_atoms.
        let r1_balance = s.account_balance(&r1_addr).atoms();
        let r2_balance = s.account_balance(&r2_addr).atoms();
        let op_gained = s.account_balance(&op_addr).atoms()
            + fee.atoms() // fee was spent on AnchorState
            - op_balance_before.atoms().saturating_sub(fee.atoms());
        // r1 = floor(10001 × 3000 / 10000) = floor(3000.3) = 3000
        // r2 = floor(10001 × 2000 / 10000) = floor(2000.2) = 2000
        // residual = 10001 - 3000 - 2000 = 5001 → operator
        assert_eq!(r1_balance, 3_000);
        assert_eq!(r2_balance, 2_000);
        let _ = op_gained; // suppress unused warning; balance checks above suffice
        let residual = escrow_atoms - r1_balance - r2_balance;
        assert_eq!(residual, 5_001);
        assert!(s.pending_escrows.is_empty());
    }

    #[test]
    fn test_escrow_release_no_fee_schedule_all_to_operator() {
        use vinx_core::ModuleEscrowPayload;

        let (mut s, op_kp, op_addr, module_id) = escrow_state();
        let fee = s.base_fee;

        // Create escrow (no fee schedule set).
        let client_kp = KeyPair::generate();
        let client_addr = Address::from_public_key(&client_kp.public_key());
        let escrow_atoms = 50_000u128;
        s.credit_for_test(client_addr, Amount::from_atoms(escrow_atoms + fee.atoms() + EXISTENTIAL_DEPOSIT_ATOMS));
        let payload = ModuleEscrowPayload {
            module_id,
            timeout_secs: MODULE_ESCROW_MIN_TIMEOUT_SECS,
            service_params_hash: [0u8; 32],
        };
        let escrow_tx = Transaction::new_module_escrow(
            &client_kp,
            &payload,
            Amount::from_atoms(escrow_atoms),
            fee,
            0,
        );
        s.apply_transaction(&escrow_tx).unwrap();
        let escrow_id = s.pending_escrows.keys().copied().next().unwrap();
        let op_balance_before = s.account_balance(&op_addr);

        let release_op = ModuleOp::EscrowRelease { escrow_id };
        s.apply_transaction(&Transaction::new_anchor_state(&op_kp, &release_op, fee, 1))
            .unwrap();

        // Entire escrow_atoms credited to operator (minus the anchor fee debited separately).
        let op_net_gain = s
            .account_balance(&op_addr)
            .atoms()
            .saturating_sub(op_balance_before.atoms().saturating_sub(fee.atoms()));
        assert_eq!(op_net_gain, escrow_atoms);
        assert!(s.pending_escrows.is_empty());
    }

    #[test]
    fn test_escrow_release_by_non_operator_rejected() {
        use vinx_core::ModuleEscrowPayload;

        let (mut s, _op_kp, _op_addr, module_id) = escrow_state();
        let fee = s.base_fee;

        // Create escrow as client.
        let client_kp = KeyPair::generate();
        let client_addr = Address::from_public_key(&client_kp.public_key());
        s.credit_for_test(client_addr, Amount::from_vinx(500));
        let payload = ModuleEscrowPayload {
            module_id,
            timeout_secs: MODULE_ESCROW_MIN_TIMEOUT_SECS,
            service_params_hash: [0u8; 32],
        };
        let escrow_tx = Transaction::new_module_escrow(
            &client_kp,
            &payload,
            Amount::from_vinx(10),
            fee,
            0,
        );
        s.apply_transaction(&escrow_tx).unwrap();
        let escrow_id = s.pending_escrows.keys().copied().next().unwrap();

        // Non-operator tries to release via AnchorState.
        let intruder_kp = KeyPair::generate();
        let intruder_addr = Address::from_public_key(&intruder_kp.public_key());
        s.credit_for_test(intruder_addr, Amount::from_vinx(100));
        let release_op = ModuleOp::EscrowRelease { escrow_id };
        let bad_tx = Transaction::new_anchor_state(&intruder_kp, &release_op, fee, 0);
        // Intruder is not the operator of the module, so apply_anchor_state rejects.
        assert!(s.apply_transaction_trusted(&bad_tx).is_err());
        assert_eq!(s.pending_escrows.len(), 1);
    }

    #[test]
    fn test_set_fee_schedule_invalid_shares_rejected() {
        use vinx_core::module::{FeeRecipient, FeeSchedule};

        let (mut s, op_kp, op_addr, module_id) = escrow_state();
        let fee = s.base_fee;

        let bad_sched = FeeSchedule {
            base_fee_atoms: 0,
            recipients: vec![
                FeeRecipient { address: op_addr, share_bps: 6_000 },
                FeeRecipient { address: op_addr, share_bps: 5_000 }, // 11 000 > 10 000
            ],
            operator_address: op_addr,
        };
        let op = ModuleOp::SetFeeSchedule { module_id, fee_schedule: bad_sched };
        assert!(s
            .apply_transaction(&Transaction::new_anchor_state(&op_kp, &op, fee, 1))
            .is_err());
        assert!(!s.module_fee_schedules.contains_key(&module_id));
    }

    // ─── ADR 0035: transaction and block resource bounds ──────────────────────

    #[test]
    fn test_payload_at_max_bytes_is_accepted() {
        use vinx_core::amount::MAX_PAYLOAD_BYTES;

        let mut s = WorldState::new();
        let (sender_kp, sender) = kp_addr();
        let (_, receiver) = kp_addr();
        s.credit_for_test(sender, Amount::from_vinx(1_000));

        let fee = Amount::ZERO.calculate_fee(s.base_fee);
        let mut tx =
            Transaction::new_transfer(&sender_kp, receiver, Amount::from_vinx(1), fee, 0);
        // Attach exactly MAX_PAYLOAD_BYTES of zeros; Transfer ignores payload content.
        tx.payload = vec![0u8; MAX_PAYLOAD_BYTES];
        tx.sign(&sender_kp); // re-sign to cover the new payload in signing_bytes
        s.apply_transaction(&tx)
            .expect("payload at MAX_PAYLOAD_BYTES must be accepted (ADR 0035 boundary)");
        assert_eq!(s.accounts[&sender].nonce, 1, "nonce must advance on success");
    }

    // ─── ADR 0036: validator churn bounds ────────────────────────────────────

    fn bonded_validator_state() -> (WorldState, KeyPair, Address, KeyPair, Address) {
        // Returns a state with one active bonded validator plus an admin.
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        let (admin_kp, admin_addr) = kp_addr();
        let (val_kp, val_addr) = kp_addr();
        let mut s = WorldState::new();
        s.admin_address = Some(admin_addr.clone());
        s.credit_for_test(admin_addr.clone(), Amount::from_vinx(1_000));
        s.set_staked_for_test(&val_addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
        // Add the validator to the active set.
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::AddValidator(val_addr.clone()),
            0,
        ))
        .unwrap();
        (s, admin_kp, admin_addr, val_kp, val_addr)
    }

    #[test]
    fn test_remove_validator_enqueues_not_immediately_removes() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        let (admin_kp, admin_addr) = kp_addr();
        let (_, val1) = kp_addr();
        let (_, val2) = kp_addr();
        let mut s = WorldState::new();
        s.admin_address = Some(admin_addr.clone());
        s.credit_for_test(admin_addr.clone(), Amount::from_vinx(1_000));
        for addr in [val1.clone(), val2.clone()] {
            s.set_staked_for_test(&addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
            s.apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::AddValidator(addr),
                s.accounts[&admin_addr].nonce,
            ))
            .unwrap();
        }
        assert_eq!(s.validator_set.len(), 3); // genesis placeholder + val1 + val2

        // RemoveValidator enqueues — the validator stays in the active set.
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::RemoveValidator(val1.clone()),
            s.accounts[&admin_addr].nonce,
        ))
        .unwrap();
        assert!(s.validator_set.contains(&val1), "val1 must still be in the active set");
        assert_eq!(s.validator_exit_queue, vec![val1.clone()]);
    }

    #[test]
    fn test_remove_validator_duplicate_queue_entry_rejected() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        let (admin_kp, admin_addr) = kp_addr();
        let (_, val1) = kp_addr();
        let (_, val2) = kp_addr();
        let mut s = WorldState::new();
        s.admin_address = Some(admin_addr.clone());
        s.credit_for_test(admin_addr.clone(), Amount::from_vinx(1_000));
        for addr in [val1.clone(), val2.clone()] {
            s.set_staked_for_test(&addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
            s.apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::AddValidator(addr),
                s.accounts[&admin_addr].nonce,
            ))
            .unwrap();
        }
        let nonce = s.accounts[&admin_addr].nonce;
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::RemoveValidator(val1.clone()),
            nonce,
        ))
        .unwrap();
        // Second RemoveValidator for the same address must be rejected.
        let result = s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::RemoveValidator(val1.clone()),
            nonce + 1,
        ));
        assert!(result.is_err(), "duplicate queue entry must be rejected");
    }

    #[test]
    fn test_exit_queue_processed_at_window_boundary() {
        use vinx_core::amount::{EXIT_QUEUE_WINDOW_BLOCKS, MIN_VALIDATOR_BOND_ATOMS};
        let (admin_kp, admin_addr) = kp_addr();
        let (_, val1) = kp_addr();
        let (_, val2) = kp_addr();
        let (_, producer) = kp_addr();
        let mut s = WorldState::new();
        s.admin_address = Some(admin_addr.clone());
        s.credit_for_test(admin_addr.clone(), Amount::from_vinx(1_000));
        // Add two extra validators (so the floor check won't block either removal).
        for addr in [val1.clone(), val2.clone()] {
            s.set_staked_for_test(&addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
            s.apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::AddValidator(addr),
                s.accounts[&admin_addr].nonce,
            ))
            .unwrap();
        }
        let nonce = s.accounts[&admin_addr].nonce;
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::RemoveValidator(val1.clone()),
            nonce,
        ))
        .unwrap();
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::RemoveValidator(val2.clone()),
            nonce + 1,
        ))
        .unwrap();
        assert_eq!(s.validator_exit_queue.len(), 2);

        // One block before the boundary: nothing is dequeued.
        s.settle_block(&producer, EXIT_QUEUE_WINDOW_BLOCKS - 1, 1_000);
        assert_eq!(s.validator_exit_queue.len(), 2);
        assert!(s.validator_set.contains(&val1));

        // At the window boundary: both are dequeued and removed from the active set.
        s.settle_block(&producer, EXIT_QUEUE_WINDOW_BLOCKS, 2_000);
        assert!(s.validator_exit_queue.is_empty());
        assert!(!s.validator_set.contains(&val1));
        assert!(!s.validator_set.contains(&val2));
    }

    #[test]
    fn test_exit_queue_limited_to_max_per_window() {
        use vinx_core::amount::{
            EXIT_QUEUE_WINDOW_BLOCKS, MAX_VALIDATOR_EXITS_PER_WINDOW, MIN_VALIDATOR_BOND_ATOMS,
        };
        // Fill the queue with more entries than MAX_VALIDATOR_EXITS_PER_WINDOW.
        // Genesis placeholder + val1..valN = set of N+1. We need effective_after >= 1.
        // With 4 validators and 3 queued: effective = 4 - 3 - 1 = 0 < 1, so only 2 can queue.
        // Use 5 extra validators to leave room.
        let (admin_kp, admin_addr) = kp_addr();
        let (_, producer) = kp_addr();
        let mut s = WorldState::new();
        s.admin_address = Some(admin_addr.clone());
        s.credit_for_test(admin_addr.clone(), Amount::from_vinx(1_000));
        let mut extras: Vec<Address> = Vec::new();
        for _ in 0..5 {
            let (_, addr) = kp_addr();
            s.set_staked_for_test(&addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
            s.apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::AddValidator(addr.clone()),
                s.accounts[&admin_addr].nonce,
            ))
            .unwrap();
            extras.push(addr);
        }
        // Queue the first MAX_VALIDATOR_EXITS_PER_WINDOW + 1 extras (but floor must hold).
        // With 6 validators (placeholder + 5 extras) and queuing 3:
        // effective_after for 3rd = 6 - 2 - 1 = 3 >= 1 ✓
        let to_queue = MAX_VALIDATOR_EXITS_PER_WINDOW + 1;
        for addr in extras.iter().take(to_queue) {
            s.apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::RemoveValidator(addr.clone()),
                s.accounts[&admin_addr].nonce,
            ))
            .unwrap();
        }
        assert_eq!(s.validator_exit_queue.len(), to_queue);

        // At the first window boundary: only MAX_VALIDATOR_EXITS_PER_WINDOW are dequeued.
        s.settle_block(&producer, EXIT_QUEUE_WINDOW_BLOCKS, 1_000);
        assert_eq!(
            s.validator_exit_queue.len(),
            to_queue - MAX_VALIDATOR_EXITS_PER_WINDOW,
            "overflow must remain in queue for the next window"
        );
        // The overflow entry is dequeued at the next window boundary.
        s.settle_block(&producer, EXIT_QUEUE_WINDOW_BLOCKS * 2, 2_000);
        assert!(s.validator_exit_queue.is_empty());
    }

    #[test]
    fn test_remove_validator_floor_check_blocks_removal() {
        use vinx_core::amount::MIN_VALIDATOR_SET_SIZE;
        // A single-validator state — removing that validator would drop to 0, below MIN.
        let (mut s, admin_kp, admin_addr) = admin_state();
        let (_, val) = kp_addr();
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        s.set_staked_for_test(&val, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
        let nonce = s.accounts[&admin_addr].nonce;
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::AddValidator(val.clone()),
            nonce,
        ))
        .unwrap();
        // Replace genesis placeholder so only `val` is active.
        s.validator_set = ValidatorSet::single(val.clone());

        // Removing `val` would leave 0 active validators → rejected.
        let result = s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::RemoveValidator(val.clone()),
            nonce + 1,
        ));
        assert!(
            result.is_err(),
            "removal that would drop below {MIN_VALIDATOR_SET_SIZE} must be rejected"
        );
        assert!(s.validator_exit_queue.is_empty(), "queue must be empty after rejection");
    }

    #[test]
    fn test_payload_over_max_bytes_is_rejected_nonce_not_consumed() {
        use vinx_core::amount::MAX_PAYLOAD_BYTES;

        let mut s = WorldState::new();
        let (sender_kp, sender) = kp_addr();
        let (_, receiver) = kp_addr();
        s.credit_for_test(sender, Amount::from_vinx(1_000));

        let fee = Amount::ZERO.calculate_fee(s.base_fee);
        let mut tx =
            Transaction::new_transfer(&sender_kp, receiver, Amount::from_vinx(1), fee, 0);
        tx.payload = vec![0u8; MAX_PAYLOAD_BYTES + 1];
        tx.sign(&sender_kp);
        let err = s.apply_transaction(&tx).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidTransaction(_)),
            "oversized payload must produce InvalidTransaction, got {err:?}"
        );
        // ADR 0035 critical invariant: the payload guard fires before any state
        // mutation, so the nonce must remain at 0.
        assert_eq!(
            s.accounts[&sender].nonce,
            0,
            "nonce must not be consumed when payload exceeds MAX_PAYLOAD_BYTES"
        );
    }

    // ─── ADR 0028: proportional epoch cosign distribution ────────────────────

    #[test]
    fn test_epoch_cosign_distribution_is_proportional() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        use vinx_core::validator_pool::ValidatorPoolEntry;

        // Build a pool of 2 validators, put them Active, and inject different cosign counts.
        let mut s = WorldState::new();
        let (_, val1) = kp_addr();
        let (_, val2) = kp_addr();

        s.validator_pool.insert(
            val1,
            ValidatorPoolEntry {
                bond_atoms: MIN_VALIDATOR_BOND_ATOMS,
                bonded_since_ts: 0,
                status: vinx_core::validator_pool::PoolStatus::Active,
                cosign_count_in_window: 3,
                eligible_blocks_in_window: 3,
                bls_pub_key: None,
                bls_pop: None,
                epoch_proposed: 0,
                epoch_cosigned: 3, // val1 co-signed 3 blocks
            },
        );
        s.validator_pool.insert(
            val2,
            ValidatorPoolEntry {
                bond_atoms: MIN_VALIDATOR_BOND_ATOMS,
                bonded_since_ts: 0,
                status: vinx_core::validator_pool::PoolStatus::Active,
                cosign_count_in_window: 1,
                eligible_blocks_in_window: 3,
                bls_pub_key: None,
                bls_pop: None,
                epoch_proposed: 0,
                epoch_cosigned: 1, // val2 co-signed only 1 block
            },
        );
        s.validator_set = ValidatorSet::new(vec![val1, val2]);
        s.active_set_size = 2;

        // Inject 4 VinX into the epoch pot and close the epoch directly.
        let pot = Amount::from_vinx(4);
        s.epoch_dist_emission_pot = pot;
        // PROPOSER_SHARE_BPS = 0 here since epoch_proposed = 0 for both.
        // All 4 VinX go to co-signers proportional to epoch_cosigned: 3:1 ratio.
        // Call tick_epoch_close() directly to avoid settle_block's work emission.
        s.tick_epoch_close();

        // val1: 3/(3+1) * 4 VinX = 3 VinX; val2: 1/4 * 4 VinX = 1 VinX.
        assert_eq!(s.account_balance(&val1), Amount::from_vinx(3));
        assert_eq!(s.account_balance(&val2), Amount::from_vinx(1));
        assert_eq!(s.epoch_dist_emission_pot, Amount::ZERO);
    }

    #[test]
    fn test_epoch_cosign_fallback_to_equal_when_no_cosigns() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        use vinx_core::validator_pool::ValidatorPoolEntry;

        // Two validators with zero epoch_cosigned — should fall back to equal split.
        let mut s = WorldState::new();
        let (_, val1) = kp_addr();
        let (_, val2) = kp_addr();

        for &v in &[val1, val2] {
            s.validator_pool.insert(
                v,
                ValidatorPoolEntry {
                    bond_atoms: MIN_VALIDATOR_BOND_ATOMS,
                    bonded_since_ts: 0,
                    status: vinx_core::validator_pool::PoolStatus::Active,
                    cosign_count_in_window: 0,
                    eligible_blocks_in_window: 0,
                    bls_pub_key: None,
                    bls_pop: None,
                    epoch_proposed: 0,
                    epoch_cosigned: 0,
                },
            );
        }
        s.validator_set = ValidatorSet::new(vec![val1, val2]);
        s.active_set_size = 2;
        s.epoch_dist_emission_pot = Amount::from_vinx(4);
        // Call tick_epoch_close() directly to avoid settle_block's work emission.
        s.tick_epoch_close();

        // Equal split: each gets 2 VinX.
        assert_eq!(s.account_balance(&val1), Amount::from_vinx(2));
        assert_eq!(s.account_balance(&val2), Amount::from_vinx(2));
    }

    // ─── ADR 0038: governance — UpdateMinValidatorBond + UpdateActiveSetSize ──

    #[test]
    fn test_update_min_validator_bond_happy_path() {
        use vinx_core::amount::{BOND_COOLDOWN_SECS, MIN_VALIDATOR_BOND_ATOMS};
        let (mut s, admin_kp, admin_addr) = admin_state();
        let initial = s.min_validator_bond_atoms;
        assert_eq!(initial, MIN_VALIDATOR_BOND_ATOMS);

        // Decrease by 25% (at exactly the step limit).
        let new_bond = initial - initial * 2500 / 10_000;
        s.current_block_ts = BOND_COOLDOWN_SECS; // cooldown satisfied
        let nonce = s.accounts[&admin_addr].nonce;
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateMinValidatorBond { atoms: new_bond },
            nonce,
        ))
        .unwrap();
        assert_eq!(s.min_validator_bond_atoms, new_bond);
        assert_eq!(s.last_bond_change_ts, BOND_COOLDOWN_SECS);
    }

    #[test]
    fn test_update_min_validator_bond_hard_floor_enforced() {
        use vinx_core::amount::{BOND_COOLDOWN_SECS, MIN_BOND_HARD_FLOOR};
        let (mut s, admin_kp, admin_addr) = admin_state();
        s.current_block_ts = BOND_COOLDOWN_SECS;
        // Attempt to set below the hard floor.
        let result = s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateMinValidatorBond { atoms: MIN_BOND_HARD_FLOOR - 1 },
            s.accounts[&admin_addr].nonce,
        ));
        assert!(result.is_err(), "below hard floor must be rejected");
    }

    #[test]
    fn test_update_min_validator_bond_cooldown_enforced() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        let (mut s, admin_kp, admin_addr) = admin_state();
        // No time has passed — cooldown not satisfied.
        let new_bond = MIN_VALIDATOR_BOND_ATOMS - MIN_VALIDATOR_BOND_ATOMS * 100 / 10_000;
        let result = s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateMinValidatorBond { atoms: new_bond },
            s.accounts[&admin_addr].nonce,
        ));
        assert!(result.is_err(), "cooldown not elapsed must be rejected");
    }

    #[test]
    fn test_update_min_validator_bond_step_limit_enforced() {
        use vinx_core::amount::{BOND_COOLDOWN_SECS, MIN_VALIDATOR_BOND_ATOMS};
        let (mut s, admin_kp, admin_addr) = admin_state();
        s.current_block_ts = BOND_COOLDOWN_SECS;
        // Attempt to drop by 50% (exceeds 25% step limit).
        let new_bond = MIN_VALIDATOR_BOND_ATOMS / 2;
        let result = s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateMinValidatorBond { atoms: new_bond },
            s.accounts[&admin_addr].nonce,
        ));
        assert!(result.is_err(), "step limit exceeded must be rejected");
    }

    #[test]
    fn test_update_active_set_size_happy_path() {
        use vinx_core::amount::{ACTIVE_SET_COOLDOWN_SECS, DEFAULT_ACTIVE_SET_SIZE};
        let (mut s, admin_kp, admin_addr) = admin_state();
        assert_eq!(s.active_set_size, DEFAULT_ACTIVE_SET_SIZE);
        s.current_block_ts = ACTIVE_SET_COOLDOWN_SECS;
        let nonce = s.accounts[&admin_addr].nonce;
        // Increase by ACTIVE_SET_STEP (2).
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateActiveSetSize { new_size: DEFAULT_ACTIVE_SET_SIZE + 2 },
            nonce,
        ))
        .unwrap();
        assert_eq!(s.active_set_size, DEFAULT_ACTIVE_SET_SIZE + 2);
        assert_eq!(s.last_active_set_size_change_ts, ACTIVE_SET_COOLDOWN_SECS);
    }

    #[test]
    fn test_update_active_set_size_floor_enforced() {
        use vinx_core::amount::{ACTIVE_SET_COOLDOWN_SECS, MIN_ACTIVE_SET_SIZE};
        let (mut s, admin_kp, admin_addr) = admin_state();
        // Force active_set_size to exactly floor + STEP so one step down hits the floor.
        s.active_set_size = MIN_ACTIVE_SET_SIZE + 2;
        s.current_block_ts = ACTIVE_SET_COOLDOWN_SECS;
        // Going to exactly MIN_ACTIVE_SET_SIZE is allowed.
        s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateActiveSetSize { new_size: MIN_ACTIVE_SET_SIZE },
            s.accounts[&admin_addr].nonce,
        ))
        .unwrap();
        assert_eq!(s.active_set_size, MIN_ACTIVE_SET_SIZE);

        // Going one below is rejected.
        s.current_block_ts += ACTIVE_SET_COOLDOWN_SECS;
        let result = s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateActiveSetSize { new_size: MIN_ACTIVE_SET_SIZE - 1 },
            s.accounts[&admin_addr].nonce,
        ));
        assert!(result.is_err(), "below hard floor must be rejected");
    }

    #[test]
    fn test_update_active_set_size_wrong_step_rejected() {
        use vinx_core::amount::{ACTIVE_SET_COOLDOWN_SECS, DEFAULT_ACTIVE_SET_SIZE};
        let (mut s, admin_kp, admin_addr) = admin_state();
        s.current_block_ts = ACTIVE_SET_COOLDOWN_SECS;
        // Step of 1 (not ACTIVE_SET_STEP = 2) is rejected.
        let result = s.apply_transaction(&Transaction::new_admin_action(
            &admin_kp,
            &GovernanceAction::UpdateActiveSetSize { new_size: DEFAULT_ACTIVE_SET_SIZE + 1 },
            s.accounts[&admin_addr].nonce,
        ));
        assert!(result.is_err(), "step ≠ ACTIVE_SET_STEP must be rejected");
    }
}
