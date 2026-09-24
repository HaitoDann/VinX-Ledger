use crate::account_map::AccountMap;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use vinx_core::{
    amount::{
        cumulative_emission_atoms, Amount, ADMIN_TENURE_SECS, BOND_COOLDOWN_SECS, BOND_STEP_BPS,
        BPS_DENOM, DEFAULT_FEE_FLOOR_ATOMS, EPOCH_DURATION_SECS, EVIDENCE_MAX_AGE_SECS,
        EXISTENTIAL_DEPOSIT_ATOMS, FEE_PRODUCER_SHARE_BPS, MAX_ACTIVE_SET_SIZE, MAX_BOND_HARD_CAP,
        MAX_NONCE_AHEAD, MAX_TX_PAYLOAD_BYTES, MAX_VALIDATOR_EXITS_PER_EPOCH, MIN_ACTIVE_SET_SIZE,
        MIN_BOND_HARD_FLOOR, MIN_STAKE_ATOMS, MIN_VALIDATOR_BOND_ATOMS, PROPOSER_SHARE_BPS,
        SLASH_BASE_BPS, SLASH_BOUNTY_BPS, SLASH_CORRELATION_FACTOR, UNBONDING_SECS,
        VALIDATOR_SCORE_WINDOW_SECS,
    },
    chain_id::CHAIN_ID_DEVNET,
    consensus::VoteEquivocation,
    governance::GovernanceAction,
    protocol::{ProtocolVersion, ScheduledUpgrade},
    reliability::{self, ReliabilityMap},
    validator_pool::PoolStatus,
    Account, CoreError, RegisterBlsKeyPayload, Transaction, TransactionType, ValidatorExitRequest,
    ValidatorSet,
};
use vinx_crypto::{hash256, Address, BlsPubKey, BlsSignature, Hash32, SmtProof, SparseMerkleTree};

/// In-memory representation of the full chain state.
/// Domain-separation tag for the two-subtree state root (VINX-04).
const STATE_ROOT_DST: &[u8] = b"VINX:state_root:v2";
/// Domain-separation tag for the consensus subtree (VINX-04).
const CONSENSUS_ROOT_DST: &[u8] = b"VINX:consensus_root:v1";

#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct WorldState {
    /// Accounts, sorted by address; persistent map with O(1) clone (ADR 0083).
    pub(crate) accounts: AccountMap,
    /// Tokens in circulation (Σ balances + staked + pending unbonds). Together with
    /// `epoch_dist_emission_pot` and `destroyed_atoms` this always equals `emitted_atoms`.
    pub circulating_supply: Amount,
    pub block_height: u64,
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
    #[borsh(skip)]
    current_block_ts: u64,
    /// Fees collected from the transactions of the block currently being applied.
    /// Credited in full to the block producer by `settle_block`. Not persisted
    /// (transient within a single block).
    #[serde(skip)]
    #[borsh(skip)]
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
    /// Protocol block time in seconds (ADR 0081 C6) — fixed at genesis, committed in the
    /// consensus root, never a per-node setting.
    #[serde(default = "default_block_time_secs")]
    pub block_time_secs: u64,
    /// Sparse Merkle tree of the accounts, keyed by `account_key(address)` (ADR 0083).
    /// Not persisted — built once on the first `compute_state_root` after load, then
    /// updated in O(log n) per changed account. Cloning it is O(1) (shared nodes).
    #[serde(skip)]
    #[borsh(skip)]
    account_tree: SparseMerkleTree,
    /// True once `account_tree` reflects `accounts` (up to `dirty_addrs`).
    #[serde(skip)]
    #[borsh(skip)]
    tree_synced: bool,
    /// Accounts modified (or removed) since the last `compute_state_root` call.
    #[serde(skip)]
    #[borsh(skip)]
    dirty_addrs: HashSet<Address>,
    /// Accounts modified since the last persistence flush. Distinct from
    /// `dirty_addrs` (which is consumed by `compute_state_root`): this set survives
    /// until `take_persist_dirty` drains it, so incremental persistence can write
    /// only the accounts that actually changed instead of the whole map.
    #[serde(skip)]
    #[borsh(skip)]
    persist_dirty: HashSet<Address>,
    /// Optional K-of-M admin committee (ADR 0011). When set, governance actions require
    /// `threshold` approvals among `signers`, superseding the single `admin_address`. When
    /// `None`, `admin_address` is the sole authority (1-of-1). Either way the authority
    /// expires after `ADMIN_TENURE_SECS` (ADR 0081 D7b).
    #[serde(default)]
    pub admin_policy: Option<AdminPolicy>,
    /// Governance proposals awaiting enough committee approvals to execute (ADR 0011).
    /// Empty under a single admin (actions execute immediately). Consensus meta, like
    /// `pending_unbonds`: derived deterministically from the same transaction history.
    #[serde(default)]
    pub pending_governance: Vec<GovernanceProposal>,
    /// Slash proceeds awaiting distribution to honest validators (ADR 0040).
    /// 90 % of every slashed bond flows here; distributed at epoch close (ADR 0028).
    #[serde(default)]
    pub epoch_dist_emission_pot: Amount,
    /// Cumulative atoms permanently destroyed by reaping dust (ADR 0026 + ADR 0040).
    /// The only source of destruction on VinX — amounts are ≤ 0.001 VINX per account.
    #[serde(default)]
    pub destroyed_atoms: u128,
    /// Fiabilité des validateurs (ADR 0027) : manquements de proposition + jailing, dérivés
    /// **déterministiquement** de la séquence de blocs (comme `pending_unbonds`).
    ///
    /// Depuis VINX-04 (ADR 0072) ce champ **est engagé** par `consensus_root` : ses intrants
    /// (`rel`, `validator_set`, hauteur, tour, proposeur — ADR 0082) sont tous
    /// déterministes et il est persisté, donc l'engager rend visible toute divergence de
    /// jailing au lieu de la laisser silencieuse.
    #[serde(default)]
    pub reliability: ReliabilityMap,
    // ── Open PoA (ADR 0038) ─────────────────────────────────────────────────────
    /// Pool of all bonded validators (ADR 0038). Keyed by validator signing address.
    /// Includes active, benched, warming-up, and unbonding entries.
    #[serde(default)]
    pub validator_pool: BTreeMap<Address, vinx_core::ValidatorPoolEntry>,
    /// Validator keys permanently banned after a proven equivocation (ADR 0038).
    /// `AddValidator`/bond transactions referencing a banned key are rejected.
    #[serde(default)]
    pub banned_validator_keys: std::collections::HashSet<Address>,
    /// Timestamp of the last governance modification to `min_validator_bond` (ADR 0038).
    /// Used to enforce BOND_COOLDOWN_SECS between modifications.
    #[serde(default)]
    pub last_bond_change_ts: u64,
    /// Timestamp of the most recent epoch close (ADR 0028 / ADR 0038).
    /// Epoch closes trigger score decay, active-set rotation, warmup ticks, and
    /// epoch-pot distribution. Zero until the first epoch close fires.
    #[serde(default)]
    pub last_epoch_close_ts: u64,
    // ── ADR 0038 complement ─────────────────────────────────────────────────────
    /// Governable minimum bond to enter the validator pool (ADR 0038).
    /// Default: MIN_VALIDATOR_BOND_ATOMS (10 000 VinX). Adjustable within
    /// [MIN_BOND_HARD_FLOOR, MAX_BOND_HARD_CAP] in steps of ±BOND_STEP_BPS with
    /// BOND_COOLDOWN_SECS between modifications.
    #[serde(default = "default_min_validator_bond")]
    pub min_validator_bond_atoms: u128,
    // ── ADR 0036 — validator churn bounds ───────────────────────────────────────
    /// FIFO exit queue for validators whose bond dropped below the minimum floor
    /// (ADR 0036). Entries are promoted to `Unbonding` status at each epoch close,
    /// at most `MAX_VALIDATOR_EXITS_PER_EPOCH` per epoch. This rate-limits churn and
    /// prevents a coordinated mass-exit from draining the active set in one epoch.
    /// Bond remains slashable while the request sits in the queue.
    #[serde(default)]
    pub exit_queue: Vec<ValidatorExitRequest>,
    /// The validator set that voted on the last committed block, with its proposer
    /// priorities as they were when it voted (ADR 0082). `Block::last_commit` of the next
    /// block is a certificate by *this* set — the active set may have rotated since.
    #[serde(default)]
    pub last_voting_set: Option<ValidatorSet>,
    /// BLS keys of `last_voting_set`, frozen when it voted (ADR 0084). The certificate
    /// of a block is checked against the keys its voters signed with — a key rotated, or
    /// a validator slashed out of the pool, in that very block must not make a valid
    /// certificate unverifiable (which would halt the chain).
    #[serde(default)]
    pub last_voting_keys: Vec<Option<Vec<u8>>>,
    /// Equivocations slashed within the evidence window (ADR 0084 S3), for correlated
    /// penalties.
    #[serde(default)]
    pub slash_history: Vec<SlashRecord>,
}

/// Validated `(bls_pub_key, bls_pop, operator)` of a key registration.
type BlsRegistration = (Vec<u8>, Vec<u8>, Option<Address>);

/// One slashed equivocation (ADR 0084 S3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct SlashRecord {
    pub validator: Address,
    /// Height at which the slash was applied.
    pub height: u64,
    /// Share of the total voting power the validator held, in basis points.
    pub power_bps: u128,
}

/// A bond amount in its unbonding delay, waiting to return to `address`'s balance
/// at `unlock_ts` (unix seconds). Slashable until it matures.
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct PendingUnbond {
    pub address: Address,
    pub amount: Amount,
    pub unlock_ts: u64,
}

/// A K-of-M admin committee (ADR 0011). `threshold` signatures among the distinct
/// `signers` are required to enact any governance action.
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize, PartialEq)]
pub struct AdminPolicy {
    pub signers: Vec<Address>,
    pub threshold: u16,
}

/// A governance action accumulating committee approvals until it reaches the threshold and
/// executes (ADR 0011). Identified by `action_hash = hash256(borsh(action))` so identical
/// actions proposed by different signers converge on the same tally.
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize, PartialEq)]
pub struct GovernanceProposal {
    pub action_hash: Hash32,
    pub action: GovernanceAction,
    /// Distinct signer addresses that have approved, in first-seen order.
    pub approvals: Vec<Address>,
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

/// BLAKE3(epoch_number_le || address) — deterministic sort key for tiebreaking
/// validators with identical reliability scores at epoch rotation (ADR 0038; hash → BLAKE3 per ADR 0069).
fn epoch_tiebreaker(epoch: u64, addr: &Address) -> [u8; 32] {
    let mut buf = [0u8; 8 + 20];
    buf[..8].copy_from_slice(&epoch.to_le_bytes());
    buf[8..].copy_from_slice(addr.as_bytes());
    hash256(&buf)
}

fn default_fee_floor() -> Amount {
    Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS)
}

fn default_block_time_secs() -> u64 {
    vinx_core::amount::DEFAULT_BLOCK_TIME_SECS
}

fn default_chain_id() -> u32 {
    CHAIN_ID_DEVNET
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
            accounts: AccountMap::new(),
            circulating_supply: Amount::ZERO,
            block_height: 0,
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
            block_time_secs: vinx_core::amount::DEFAULT_BLOCK_TIME_SECS,
            account_tree: SparseMerkleTree::new(),
            tree_synced: false,
            dirty_addrs: HashSet::new(),
            persist_dirty: HashSet::new(),
            admin_policy: None,
            pending_governance: Vec::new(),
            epoch_dist_emission_pot: Amount::ZERO,
            destroyed_atoms: 0,
            reliability: ReliabilityMap::new(),
            validator_pool: BTreeMap::new(),
            banned_validator_keys: std::collections::HashSet::new(),
            last_bond_change_ts: 0,
            last_epoch_close_ts: 0,
            min_validator_bond_atoms: MIN_VALIDATOR_BOND_ATOMS,
            exit_queue: Vec::new(),
            last_voting_set: None,
            last_voting_keys: Vec::new(),
            slash_history: Vec::new(),
        }
    }

    /// Marks an account address as dirty (changed or removed).
    #[inline]
    fn mark_dirty(&mut self, addr: &Address) {
        self.dirty_addrs.insert(*addr);
        self.persist_dirty.insert(*addr);
    }

    /// Brings the account tree up to date: a full O(n log n) build after load, then only
    /// the changed accounts, O(log n) each.
    fn flush_dirty(&mut self) {
        if !self.tree_synced {
            self.account_tree = SparseMerkleTree::from_entries(
                self.accounts
                    .values()
                    .map(|a| (account_key(&a.address), hash_account(a)))
                    .collect(),
            );
            self.tree_synced = true;
            self.dirty_addrs.clear();
            return;
        }
        for addr in std::mem::take(&mut self.dirty_addrs) {
            match self.accounts.get(&addr) {
                Some(a) => self
                    .account_tree
                    .insert(account_key(&addr), hash_account(a)),
                None => self.account_tree.remove(&account_key(&addr)),
            }
        }
    }

    /// Proof of an account's state (or absence) against the accounts root, together with
    /// the consensus root, so a client can recompute the block's `state_root` (ADR 0083).
    pub fn account_proof(&mut self, addr: &Address) -> AccountProof {
        self.flush_dirty();
        AccountProof {
            key: account_key(addr),
            leaf_value: self.accounts.get(addr).map(hash_account),
            proof: self.account_tree.prove(&account_key(addr)),
            accounts_root: self.account_tree.root(),
            consensus_root: self.compute_consensus_root(),
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

    /// Opens block `header` (ADR 0082) — call it on a state at height `header.height - 1`,
    /// **before** applying the block's transactions, on every path that executes a block.
    ///
    /// 1. The proposer must be the scheduled proposer of `header.round` (weighted proposer
    ///    priorities, jailed validators skipped).
    /// 2. `last_commit` must be a valid certificate for the previous block, signed by more
    ///    than 2/3 of the power of the set that voted on it (`last_voting_set`). Its signers
    ///    are credited as co-signers (ADR 0028) — rewards follow data every node holds.
    /// 3. Proposers of the rounds before `header.round` are charged a missed proposal.
    /// 4. The current set becomes `last_voting_set` and its proposer priority advances.
    ///
    /// Deterministic and self-contained: it reads only the state and the header, so every
    /// node reaches the same result. Fails without mutating anything.
    pub fn begin_block(
        &mut self,
        header: &vinx_core::BlockHeader,
        last_commit: Option<&vinx_core::CommitCert>,
    ) -> Result<(), CoreError> {
        let height = header.height;
        if height != self.block_height + 1 {
            return Err(CoreError::InvalidTransaction(format!(
                "block {height} does not follow state height {}",
                self.block_height
            )));
        }
        let expected =
            reliability::proposer_for_round(&self.validator_set, &self.reliability, header.round);
        if header.validator != expected {
            return Err(CoreError::InvalidTransaction(format!(
                "block {height} round {} proposed by {}, expected {expected}",
                header.round, header.validator
            )));
        }
        let cosigners: Vec<Address> = match (height, last_commit) {
            (1, None) => vec![],
            (1, Some(_)) => {
                return Err(CoreError::InvalidTransaction(
                    "block 1 cannot carry a last commit".to_string(),
                ))
            }
            (_, None) => {
                return Err(CoreError::InvalidTransaction(format!(
                    "block {height} is missing the commit certificate of block {}",
                    height - 1
                )))
            }
            (_, Some(cert)) => {
                let voters = self.last_voting_set.as_ref().ok_or_else(|| {
                    CoreError::InvalidTransaction("no voting set recorded".to_string())
                })?;
                let keys: Vec<Option<[u8; 48]>> = self
                    .last_voting_keys
                    .iter()
                    .map(|k| k.as_deref().and_then(|b| b.try_into().ok()))
                    .collect();
                cert.verify(self.chain_id, height - 1, &header.prev_hash, voters, &keys)
                    .map_err(|e| {
                        CoreError::InvalidTransaction(format!("invalid last commit: {e}"))
                    })?;
                cert.signers()
                    .into_iter()
                    .filter_map(|i| voters.validators().get(i).copied())
                    .collect()
            }
        };
        // ── validated: mutate ──
        if !cosigners.is_empty() {
            self.record_block_cosigns(&cosigners);
        }
        if reliability::on_block_committed(
            &mut self.reliability,
            &self.validator_set,
            height,
            header.round,
            &header.validator,
        ) {
            tracing::warn!(
                height,
                "ADR 0027 : validateur jailé (manquements de proposition consécutifs)"
            );
        }
        self.last_voting_set = Some(self.validator_set.clone());
        self.last_voting_keys = self
            .indexed_bls_keys(&self.validator_set)
            .into_iter()
            .map(|k| k.map(|k| k.to_vec()))
            .collect();
        let rel = &self.reliability;
        self.validator_set
            .advance_proposer_priority(|a| reliability::is_eligible(rel, a));
        Ok(())
    }

    /// Settles block-level rewards to the producer and matures due unbonds. Call once
    /// per block, after applying all transactions and before `compute_state_root`, in
    /// every path that builds/replays a block (producer, P2P apply, sync). Returns
    /// `(fees, emission)` for logging.
    ///
    /// Fees stay in circulation (they move sender → producer). Emission is newly minted
    /// (progressive minting, ADR 0040). Both are computed deterministically from the
    /// block so validators re-applying it reach the identical state.
    pub fn settle_block(&mut self, producer: &Address, block_ts: u64) -> (Amount, Amount) {
        // 1. Collected transaction fees (ADR 0081 D4): FEE_PRODUCER_SHARE_BPS to the
        //    producer now, the rest into the epoch pot for the co-signers. Nothing burned.
        let fees = std::mem::replace(&mut self.block_fees, Amount::ZERO);
        if fees > Amount::ZERO {
            let producer_atoms = fees.atoms() * FEE_PRODUCER_SHARE_BPS / BPS_DENOM;
            let cosigner_atoms = fees.atoms() - producer_atoms;
            if producer_atoms > 0 {
                self.credit(producer, Amount::from_atoms(producer_atoms));
            }
            if cosigner_atoms > 0 {
                // The fee was debited from the sender without leaving circulation;
                // parking it in the pot keeps `circulating + pot == emitted` exact.
                self.move_to_epoch_pot(Amount::from_atoms(cosigner_atoms));
            }
        }
        // 2. Mature any unbonds whose delay has elapsed (real time).
        self.mature_unbonds(block_ts);
        // 3. Work emission — mint new tokens → producer (ADR 0040).
        let emission = self.emit_work_reward(producer, block_ts);
        // 4. Epoch close (ADR 0028/0038) — triggered when EPOCH_DURATION_SECS have elapsed
        //    since the last close. Deterministic on block_ts so all nodes close the same epoch.
        if EPOCH_DURATION_SECS > 0 && self.emission_started {
            let since_last = block_ts.saturating_sub(if self.last_epoch_close_ts == 0 {
                self.emission_epoch_ts
            } else {
                self.last_epoch_close_ts
            });
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
    /// set by score (BLAKE3 tiebreaker), distributes the epoch pot, and updates
    /// `validator_set` to the new active set (ADR 0028 + ADR 0038).
    ///
    /// Called automatically from `settle_block` when `EPOCH_DURATION_SECS` have elapsed.
    pub fn tick_epoch_close(&mut self) {
        let epoch_number = if EPOCH_DURATION_SECS > 0 {
            self.last_epoch_close_ts / EPOCH_DURATION_SECS
        } else {
            0
        };

        // 0. Process exit queue (ADR 0036): promote up to MAX_VALIDATOR_EXITS_PER_EPOCH
        //    validators whose bond dropped below the floor to Unbonding status.
        //    Sorted FIFO by (request_height, address). The floor guard ensures the
        //    eligible pool cannot shrink below MIN_ACTIVE_SET_SIZE.
        {
            self.exit_queue.sort_by(|a, b| {
                a.request_height
                    .cmp(&b.request_height)
                    .then_with(|| a.address.cmp(&b.address))
            });

            let eligible_count = self
                .validator_pool
                .values()
                .filter(|e| matches!(e.status, PoolStatus::Active | PoolStatus::Benched))
                .count();

            let mut processed = 0usize;
            let mut eligible_removed = 0usize;
            let mut remaining_queue: Vec<ValidatorExitRequest> = Vec::new();

            for req in self.exit_queue.drain(..) {
                if processed >= MAX_VALIDATOR_EXITS_PER_EPOCH {
                    remaining_queue.push(req);
                    continue;
                }
                match self.validator_pool.get_mut(&req.address) {
                    None => { /* validator was slashed / removed — discard silently */ }
                    Some(entry) => {
                        if entry.bond_atoms >= self.min_validator_bond_atoms {
                            // Bond was topped back up — discard the stale exit request.
                            continue;
                        }
                        let is_eligible =
                            matches!(entry.status, PoolStatus::Active | PoolStatus::Benched);
                        if is_eligible {
                            let after = eligible_count.saturating_sub(eligible_removed + 1);
                            if after < MIN_ACTIVE_SET_SIZE as usize {
                                // Promoting this validator would violate the floor — defer.
                                remaining_queue.push(req);
                                continue;
                            }
                            eligible_removed += 1;
                        }
                        entry.status = PoolStatus::Unbonding {
                            unlock_ts: req.unlock_ts,
                        };
                        processed += 1;
                        tracing::info!(
                            addr = %req.address,
                            "ADR 0036: validator exit processed, moved to Unbonding"
                        );
                    }
                }
            }
            self.exit_queue = remaining_queue;
        }

        // 1. Decay all pool window counters (sliding-window approximation).
        let window_epochs = VALIDATOR_SCORE_WINDOW_SECS / EPOCH_DURATION_SECS.max(1);
        for entry in self.validator_pool.values_mut() {
            entry.decay_window(window_epochs);
        }

        // 2. Tick warmup counters — validators completing warm-up become Benched.
        for entry in self.validator_pool.values_mut() {
            entry.tick_warmup();
        }

        // 3. Rank eligible validators by score, BLAKE3 tiebreaker.
        let n = MAX_ACTIVE_SET_SIZE as usize;
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
                            self.distribute_from_epoch_pot(addr, Amount::from_atoms(share_atoms));
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
            self.validator_set = self.weighted_validator_set(new_vs_addrs);
        }
        // If the pool is empty (e.g. genesis before any bonds), leave validator_set as-is.

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
        lhs == Some(self.emitted_atoms) && self.emitted_atoms <= vinx_core::amount::MAX_SUPPLY_ATOMS
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
        // Removed from the tree at the next flush, and erased from persistence.
        self.mark_dirty(addr);
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
    pub fn serialize_meta(&mut self) -> Result<Vec<u8>, std::io::Error> {
        let accounts = std::mem::take(&mut self.accounts);
        let result = borsh::to_vec(&*self);
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
    /// - the fee meets the current `base_fee` for fee-bearing types (Transfer —
    ///   stake/unstake/slash are exempt per ADR 0009),
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
        // VINX-13: bound the payload before anything else. The fee is derived from
        // `amount`, not from size, so an oversized payload is otherwise free block bloat.
        if tx.payload.len() > MAX_TX_PAYLOAD_BYTES {
            return Err(CoreError::InvalidTransaction(format!(
                "payload of {} bytes exceeds the {MAX_TX_PAYLOAD_BYTES}-byte limit",
                tx.payload.len()
            )));
        }
        if tx.chain_id != self.chain_id {
            return Err(CoreError::InvalidTransaction(format!(
                "chain_id {} does not match this network ({})",
                tx.chain_id, self.chain_id
            )));
        }

        // Fee floor for the fee-bearing type (mirrors apply_transfer).
        if tx.tx_type == TransactionType::Transfer && tx.fee < self.base_fee {
            return Err(CoreError::InvalidTransaction(format!(
                "fee {} is below the current base fee {}",
                tx.fee, self.base_fee
            )));
        }

        let Some(account) = self.accounts.get(&tx.sender()) else {
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
            // VINX-03 defence in depth: a sponsored transaction must at least *carry* the
            // sponsor's key and signature. The authoritative cryptographic check is
            // `verify_tx_signature_pure` on the admission paths; this structural gate makes
            // a stripped-sponsorship transaction impossible to park in the mempool even if
            // a future caller forgets it.
            if tx.sponsor_pub_key.is_none() || tx.sponsor_signature.is_none() {
                return Err(CoreError::InvalidTransaction(
                    "sponsored transaction is missing the sponsor's key or signature".to_string(),
                ));
            }
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
        // VINX-13: consensus-level payload bound. Enforced here rather than only at
        // admission because both `apply_transaction` and `apply_transaction_trusted` go
        // through this gate — so an oversized payload cannot enter state via a block
        // either, and every node agrees on which blocks are valid.
        if tx.payload.len() > MAX_TX_PAYLOAD_BYTES {
            return Err(CoreError::InvalidTransaction(format!(
                "payload of {} bytes exceeds the {MAX_TX_PAYLOAD_BYTES}-byte limit",
                tx.payload.len()
            )));
        }
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
        // ADR 0081 D6: the sender is derived from `pub_key`, so there is no separate
        // `from` that could disagree with it — only the signature needs checking.
        let pk = &tx.pub_key;
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
        if self.admin_tenure_expired() {
            return Err(CoreError::Unauthorized);
        }
        // VINX-20 (complément) : échec fermé, comme `apply_admin_action`. Le premier
        // correctif n'avait traité que `apply_admin_action` et avait laissé ce chemin —
        // qui garde `AnnounceUpgrade` — sur l'ancien motif permissif : sans admin
        // configuré, le `if let Some(..)` était sauté et **n'importe qui** pouvait
        // planifier une mise à jour de protocole. Une autorité absente n'est pas une
        // autorité permissive.
        let Some(ref admin) = self.admin_address else {
            return Err(CoreError::Unauthorized);
        };
        if &tx.sender() != admin {
            return Err(CoreError::Unauthorized);
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
            (total, tx.sender())
        };

        // ── ADR 0026: existential deposit. Validate every party's resulting balance
        // BEFORE mutating any state: the block producer skips a rejected tx *without*
        // rolling back partial mutations, so a late rejection here would corrupt the block.
        // A party may land at exactly 0 (it gets reaped below) or at >= ED — never in the
        // forbidden ]0, ED[ band, unless it is staked (a bonded account is never dust).
        {
            let ed = EXISTENTIAL_DEPOSIT_ATOMS;
            let mut deltas: HashMap<Address, i128> = HashMap::new();
            *deltas.entry(tx.sender()).or_default() -= sender_debit.atoms() as i128;
            if fee_payer != tx.sender() {
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
                .get_mut(&tx.sender())
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
        if fee_payer != tx.sender() {
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

        self.mark_dirty(&tx.sender());
        self.mark_dirty(&tx.to);
        if fee_payer != tx.sender() {
            self.mark_dirty(&fee_payer);
        }

        // ADR 0026: reap any party the transfer left at exactly zero (no balance, no
        // stake, no bond unbonding), returning its 60 bytes to the free state.
        self.reap_if_empty(&tx.sender());
        if fee_payer != tx.sender() {
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

        // ADR 0075 §3.1 — un bond qui fait ENTRER au pool doit porter la clé BLS du
        // validateur et sa PoP.
        //
        // Sans cette exigence, « tout validateur actif est authentifiable » n'était qu'une
        // propriété de convergence : un validateur pouvait bonder puis enregistrer sa clé
        // plus tard, ou jamais. Or `quorum()` compte **tous** les membres du set, y compris
        // ceux qui ne peuvent pas produire de bloc accepté (ADR 0070) : au-delà d'un tiers
        // de membres sans clé, le quorum devient inatteignable et la finalité se fige. La
        // liaison au bonding en fait un invariant structurel.
        //
        // Validé avant toute mutation : un bond refusé ne doit pas consommer le nonce.
        let creates_pool_entry = {
            let staked_after = self
                .account_staked(&tx.sender())
                .checked_add(tx.amount)
                .ok_or(CoreError::AmountOverflow)?;
            staked_after.atoms() >= self.min_validator_bond_atoms
                && !self.banned_validator_keys.contains(&tx.sender())
                && !self.validator_pool.contains_key(&tx.sender())
        };
        let entry_bls = if creates_pool_entry {
            Some(self.validate_bls_registration(tx.sender(), &tx.payload)?)
        } else {
            None
        };
        // Compute new staked total in a scoped borrow so we can modify self.validator_pool after.
        let new_staked = {
            let account = self
                .accounts
                .get_mut(&tx.sender())
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
        self.mark_dirty(&tx.sender());
        // ADR 0038: auto-enter the validator pool once the bond floor is met.
        let bond = new_staked.atoms();
        if bond >= self.min_validator_bond_atoms
            && !self.banned_validator_keys.contains(&tx.sender())
        {
            if let Some(entry) = self.validator_pool.get_mut(&tx.sender()) {
                entry.bond_atoms = bond;
            } else {
                let mut entry = vinx_core::ValidatorPoolEntry::new(bond, self.current_block_ts);
                // `creates_pool_entry` a été calculé sur le même prédicat, donc la clé est
                // présente ici — mais on ne suppose pas : sans elle, pas d'entrée.
                let (pk, pop, operator) = entry_bls.clone().ok_or_else(|| {
                    CoreError::InvalidTransaction(
                        "bond entering the validator pool must carry a BLS key".to_string(),
                    )
                })?;
                entry.bls_pub_key = Some(pk);
                entry.bls_pop = Some(pop);
                entry.operator = operator;
                self.validator_pool.insert(tx.sender(), entry);
                tracing::info!(addr = %tx.sender(), bond, "ADR 0038: validator auto-entered pool");
            }
        }
        Ok(())
    }

    fn apply_unstake(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let bond_floor = Amount::from_atoms(self.min_validator_bond_atoms);
        let is_active_validator = self.validator_set.contains(&tx.sender());
        let unlock_ts = self.current_block_ts.saturating_add(UNBONDING_SECS);

        // ADR 0009: cap concurrent unbonding entries per account (anti-spam on
        // `pending_unbonds`, a stronger bound than a negligible flat fee would be).
        let pending_for_sender = self
            .pending_unbonds
            .iter()
            .filter(|u| u.address == tx.sender())
            .count();
        if pending_for_sender >= vinx_core::amount::MAX_PENDING_UNBONDS_PER_ACCOUNT {
            return Err(CoreError::InvalidTransaction(
                "too many pending unbonds — wait for one to mature".to_string(),
            ));
        }

        let remaining = {
            let account = self
                .accounts
                .get_mut(&tx.sender())
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
            address: tx.sender(),
            amount: tx.amount,
            unlock_ts,
        });
        self.mark_dirty(&tx.sender());
        // ADR 0038 + ADR 0036: update pool entry when bond drops below the floor.
        // Instead of immediately transitioning to Unbonding, enqueue for rate-limited
        // exit at the next epoch close (at most MAX_VALIDATOR_EXITS_PER_EPOCH per epoch).
        // Bond remains slashable while queued.
        let new_bond = remaining.atoms();
        if let Some(entry) = self.validator_pool.get_mut(&tx.sender()) {
            entry.bond_atoms = new_bond;
            if new_bond < self.min_validator_bond_atoms {
                let already_queued = self.exit_queue.iter().any(|r| r.address == tx.sender());
                if !already_queued {
                    self.exit_queue.push(ValidatorExitRequest {
                        address: tx.sender(),
                        request_height: self.block_height,
                        unlock_ts,
                    });
                    tracing::info!(
                        addr = %tx.sender(),
                        new_bond,
                        "ADR 0036: bond below floor, validator enqueued for rate-limited exit"
                    );
                }
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
            .get_mut(&tx.sender())
            .ok_or(CoreError::InsufficientBalance)?;
        if sender.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: sender.nonce,
                got: tx.nonce,
            });
        }
        sender.nonce += 1;
        self.mark_dirty(&tx.sender());

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
        let accounts_root = self.account_tree.root();
        let consensus_root = self.compute_consensus_root();
        state_root_of(&accounts_root, &consensus_root)
    }

    /// Merkle-equivalent commitment to the **consensus** portion of the state
    /// (VINX-04).
    ///
    /// `compute_state_root` used to return the accounts root alone, so the root committed
    /// to nothing but `(address, balance, nonce, staked)` per account. Everything that
    /// decides who may produce blocks and who governs the chain — the validator set and
    /// pool (bonds, BLS keys, PoP, status), the admin key and policy, pending
    /// governance and upgrades, the epoch beacon, the supply counters — sat outside it.
    /// Two nodes could therefore disagree on the entire validator set and the admin key
    /// while publishing an identical `state_root`, and since `state_root` is the only
    /// state-integrity check on every block-validation path (`p2p`, `sync`, `reorg`), that
    /// divergence was silent and undetectable.
    ///
    /// The encoding is deterministic by construction: every collection is a `BTreeMap` or
    /// an order-carrying `Vec`, and the one `HashSet` is sorted before hashing. Fields
    /// marked `serde(skip)` are transient within a block and deliberately excluded.
    ///
    /// Changing this encoding changes every `state_root` and is a hard fork.
    pub fn compute_consensus_root(&self) -> Hash32 {
        // Sorted, so the iteration order of the HashSet cannot leak into the root.
        let mut banned: Vec<&Address> = self.banned_validator_keys.iter().collect();
        banned.sort_unstable();

        #[derive(BorshSerialize)]
        struct ConsensusCommitment<'a> {
            chain_id: u32,
            block_time_secs: u64,
            block_height: u64,
            validator_set: &'a ValidatorSet,
            validator_pool: &'a BTreeMap<Address, vinx_core::ValidatorPoolEntry>,
            banned_validator_keys: Vec<&'a Address>,
            min_validator_bond_atoms: u128,
            admin_address: &'a Option<Address>,
            admin_policy: &'a Option<AdminPolicy>,
            pending_governance: &'a [GovernanceProposal],
            current_version: &'a ProtocolVersion,
            pending_upgrade: &'a Option<ScheduledUpgrade>,
            pending_unbonds: &'a [PendingUnbond],
            exit_queue: &'a [ValidatorExitRequest],
            reliability: &'a ReliabilityMap,
            fee_floor: u128,
            base_fee: u128,
            circulating_supply: u128,
            emitted_atoms: u128,
            destroyed_atoms: u128,
            epoch_dist_emission_pot: u128,
            emission_epoch_ts: u64,
            last_epoch_close_ts: u64,
            last_bond_change_ts: u64,
            last_voting_set: &'a Option<ValidatorSet>,
            last_voting_keys: &'a [Option<Vec<u8>>],
            slash_history: &'a [SlashRecord],
            emission_started: bool,
        }

        let commitment = ConsensusCommitment {
            chain_id: self.chain_id,
            block_time_secs: self.block_time_secs,
            block_height: self.block_height,
            validator_set: &self.validator_set,
            validator_pool: &self.validator_pool,
            banned_validator_keys: banned,
            min_validator_bond_atoms: self.min_validator_bond_atoms,
            admin_address: &self.admin_address,
            admin_policy: &self.admin_policy,
            pending_governance: &self.pending_governance,
            current_version: &self.current_version,
            pending_upgrade: &self.pending_upgrade,
            pending_unbonds: &self.pending_unbonds,
            exit_queue: &self.exit_queue,
            reliability: &self.reliability,
            fee_floor: self.fee_floor.atoms(),
            base_fee: self.base_fee.atoms(),
            circulating_supply: self.circulating_supply.atoms(),
            emitted_atoms: self.emitted_atoms,
            destroyed_atoms: self.destroyed_atoms,
            epoch_dist_emission_pot: self.epoch_dist_emission_pot.atoms(),
            emission_epoch_ts: self.emission_epoch_ts,
            last_epoch_close_ts: self.last_epoch_close_ts,
            last_bond_change_ts: self.last_bond_change_ts,
            last_voting_set: &self.last_voting_set,
            last_voting_keys: &self.last_voting_keys,
            slash_history: &self.slash_history,
            // `emission_started` gouverne l'émission ET la clôture d'époque : deux nœuds
            // qui en divergent émettent différemment. Il n'était engagé qu'indirectement,
            // via `emission_epoch_ts` — donc invisible quand celui-ci vaut 0.
            emission_started: self.emission_started,
            // Dormant depuis ADR 0040, mais persisté et désérialisé : l'engager coûte
            // 16 octets et supprime la question « est-il vraiment mort ? ».
        };

        let encoded =
            borsh::to_vec(&commitment).expect("consensus commitment serialization is infallible");
        let mut buf = Vec::with_capacity(CONSENSUS_ROOT_DST.len() + encoded.len());
        buf.extend_from_slice(CONSENSUS_ROOT_DST);
        buf.extend_from_slice(&encoded);
        hash256(&buf)
    }

    /// Returns the registered BLS G1 keys for every validator in `validator_set`,
    /// in ValidatorSet order (ADR 0029 Phase 1).
    ///
    /// `result[i]` is the 48-byte compressed G1 key of the validator at index `i`,
    /// or `None` when that validator has not yet registered a BLS key (ADR 0046).
    /// Pass this slice to `Block::bls_signer_count_from_bitmap` or
    /// `consensus::validate_block_with_registry` to enforce that co-signers are
    /// registered on-chain and bind BLS keys to specific validator identities.
    pub fn indexed_bls_keys(
        &self,
        validator_set: &vinx_core::ValidatorSet,
    ) -> Vec<Option<[u8; 48]>> {
        validator_set
            .validators()
            .iter()
            .map(|addr| {
                self.validator_pool
                    .get(addr)
                    .and_then(|e| e.bls_pub_key.as_deref())
                    .and_then(|b| b.try_into().ok())
            })
            .collect()
    }

    fn apply_slash_validator(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let evidence: VoteEquivocation = borsh::from_slice(&tx.payload)
            .map_err(|_| CoreError::InvalidTransaction("malformed slash evidence".to_string()))?;

        let target = &tx.to;

        // 1. Two votes of the target, same kind/height/round, different values — the
        //    definition of equivocation (ADR 0082).
        if evidence.vote_a.validator != *target || !evidence.is_conflicting() {
            return Err(CoreError::InvalidTransaction(
                "evidence is not a conflicting pair of votes by the target".to_string(),
            ));
        }

        // 2. THE crucial check: both signatures must verify against the target's
        //    registered BLS key. Only the target could have produced both — forging
        //    evidence requires forging a BLS signature.
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
        if !evidence.verify(self.chain_id, &bls_pk) {
            return Err(CoreError::InvalidTransaction(
                "BLS equivocation proof does not verify".to_string(),
            ));
        }

        // A validator that already left the active set stays slashable while its bond is
        // still in the pool (warm-up, bench, unbonding).
        if !self.validator_set.contains(target) && !self.validator_pool.contains_key(target) {
            return Err(CoreError::InvalidTransaction(
                "target is not a validator".to_string(),
            ));
        }
        // ADR 0084 S2: evidence expires with the unbonding delay.
        let max_age = EVIDENCE_MAX_AGE_SECS / self.block_time_secs.max(1);
        if self.block_height.saturating_sub(evidence.vote_a.height) > max_age {
            return Err(CoreError::InvalidTransaction(
                "equivocation evidence is older than the unbonding delay".to_string(),
            ));
        }

        let sender = self
            .accounts
            .get_mut(&tx.sender())
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

        // ADR 0084 S3 — correlated penalty: base + factor × share of the voting power
        // slashed within the evidence window, this fault included.
        let window = EVIDENCE_MAX_AGE_SECS / self.block_time_secs.max(1);
        let height = self.block_height;
        self.slash_history
            .retain(|r| height.saturating_sub(r.height) <= window);
        let set = if self.validator_set.contains(target) {
            &self.validator_set
        } else {
            self.last_voting_set.as_ref().unwrap_or(&self.validator_set)
        };
        let power_bps = (set.power_of(target) as u128 * BPS_DENOM)
            .checked_div(set.total_power() as u128)
            .unwrap_or(0);
        self.slash_history.push(SlashRecord {
            validator: *target,
            height,
            power_bps,
        });
        let correlated_bps: u128 = self.slash_history.iter().map(|r| r.power_bps).sum();
        let penalty_bps = SLASH_BASE_BPS
            .saturating_add(SLASH_CORRELATION_FACTOR.saturating_mul(correlated_bps))
            .min(BPS_DENOM);

        if slashable > 0 {
            if let Some(acc) = self.accounts.get_mut(target) {
                acc.staked = Amount::ZERO;
            }
            self.pending_unbonds.retain(|u| &u.address != target);

            let slashed = slashable * penalty_bps / BPS_DENOM;
            let returned = slashable.saturating_sub(slashed);
            let bounty = slashed * SLASH_BOUNTY_BPS / BPS_DENOM;
            let to_pot = slashed.saturating_sub(bounty);

            self.mark_dirty(target);
            // The remainder is not freed at once: it serves a full unbonding delay, like
            // any exit, and stays slashable by further evidence meanwhile.
            if returned > 0 {
                self.pending_unbonds.push(PendingUnbond {
                    address: *target,
                    amount: Amount::from_atoms(returned),
                    unlock_ts: self.current_block_ts.saturating_add(UNBONDING_SECS),
                });
            }
            self.credit(&tx.sender(), Amount::from_atoms(bounty)); // reporter bounty (10%)
            self.move_to_epoch_pot(Amount::from_atoms(to_pot)); // 90% → honest validators
            tracing::warn!(
                validator = %target,
                penalty_bps,
                slashed,
                "ADR 0084: correlated slash applied"
            );
        }

        self.mark_dirty(&tx.sender());

        // ADR 0084: an equivocator is excluded for good — out of the pool and the exit
        // queue, and banned from bonding again. (Its remaining funds unbond normally.)
        self.validator_pool.remove(target);
        self.exit_queue.retain(|r| &r.address != target);
        self.banned_validator_keys.insert(*target);

        // Remove from validator set (can't produce blocks anymore).
        if self.validator_set.len() > 1 {
            let remaining: Vec<Address> = self
                .validator_set
                .validators()
                .iter()
                .copied()
                .filter(|a| a != target)
                .collect();
            self.validator_set = self.weighted_validator_set(remaining);
            tracing::warn!(validator = %target, slashed = slashable, "Validator slashed for equivocation");
        } else {
            tracing::warn!(validator = %target, "Slash accounting applied — last validator kept in set");
        }

        Ok(())
    }

    fn apply_admin_action(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        // ADR 0011: authorize against the effective admin authority — the K-of-M committee
        // if one is set, else the legacy single admin key, else dev mode (open).
        // VINX-20: fail closed. This used to skip the check entirely when no admin
        // authority was configured ("dev mode (open)"), which means a chain that never set
        // one — or that cleared it — let *anyone* add and remove validators, schedule
        // protocol upgrades and rotate the admin key. An absent authority is not a
        // permissive authority.
        let (signers, threshold) = self.effective_admin();
        let Some(ref signers) = signers else {
            return Err(CoreError::Unauthorized);
        };
        if !signers.contains(&tx.sender()) {
            return Err(CoreError::Unauthorized);
        }

        // ADR 0007: check the nonce but do NOT consume it yet — a governance action that
        // fails validation (e.g. duplicate validator, missing bond) must leave the nonce
        // untouched. It is only bumped once the action has been applied (or an approval
        // recorded) successfully.
        let cur_nonce = self
            .accounts
            .get(&tx.sender())
            .ok_or(CoreError::InsufficientBalance)?
            .nonce;
        if cur_nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: cur_nonce,
                got: tx.nonce,
            });
        }

        let action: GovernanceAction = borsh::from_slice(&tx.payload).map_err(|_| {
            CoreError::InvalidTransaction("malformed governance action payload".to_string())
        })?;

        // Single admin (or dev mode) executes immediately; a committee accumulates approvals
        // and executes on the one that reaches the threshold. Either path mutates state only
        // after full validation, so a rejected action consumes no nonce (the producer skips
        // a failed tx without rolling back — ADR 0026).
        if threshold <= 1 {
            self.execute_governance_action(action)?;
        } else {
            self.record_governance_approval(action, tx.sender(), threshold)?;
        }

        // Success: consume the nonce.
        self.accounts
            .get_mut(&tx.sender())
            .expect("sender existence checked above")
            .nonce += 1;
        self.mark_dirty(&tx.sender());
        Ok(())
    }

    /// Builds the active set for `addrs` with stake-weighted, capped voting power
    /// (ADR 0081 C4). A validator's stake is its bond (pool entry or staked balance),
    /// floored at the minimum bond so a grandfathered genesis validator (bond 0) still
    /// votes like a minimally bonded one. Measured in whole VINX.
    pub fn weighted_validator_set(&self, addrs: Vec<Address>) -> ValidatorSet {
        let unit = vinx_core::amount::DECIMAL_FACTOR;
        let stakes = addrs
            .iter()
            .map(|a| {
                let pool = self
                    .validator_pool
                    .get(a)
                    .map(|e| e.bond_atoms)
                    .unwrap_or(0);
                let staked = self.account_staked(a).atoms();
                let bond = pool.max(staked).max(self.min_validator_bond_atoms);
                (bond / unit).min(u64::MAX as u128) as u64
            })
            .collect();
        ValidatorSet::with_stakes(addrs, stakes)
    }

    /// True once the on-chain admin tenure is over (ADR 0081 D7b): `ADMIN_TENURE_SECS`
    /// after the emission epoch (first block). Derived only from committed fields and a
    /// graved constant, so every node agrees and no transaction can extend it.
    pub fn admin_tenure_expired(&self) -> bool {
        self.emission_started
            && self.current_block_ts >= self.emission_epoch_ts.saturating_add(ADMIN_TENURE_SECS)
    }

    /// The effective admin authority (ADR 0011): `(Some(signers), threshold)` under a
    /// committee or a single admin key; `(None, 1)` when no authority is configured — in
    /// which case callers must **refuse** the action (VINX-20), never allow it.
    fn effective_admin(&self) -> (Option<Vec<Address>>, u16) {
        if self.admin_tenure_expired() {
            (None, 1)
        } else if let Some(ref p) = self.admin_policy {
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
        let action_hash = hash256(&borsh::to_vec(&action).map_err(|_| {
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
                let mut addrs = self.validator_set.validators().to_vec();
                addrs.push(addr);
                self.validator_set = self.weighted_validator_set(addrs);
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
                let remaining: Vec<Address> = self
                    .validator_set
                    .validators()
                    .iter()
                    .copied()
                    .filter(|a| *a != addr)
                    .collect();
                self.validator_set = self.weighted_validator_set(remaining);
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
                let delta = atoms.abs_diff(current);
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

    /// Applies a `RegisterBlsKey` transaction (ADR 0046): stores a validator's BLS12-381
    /// public key and Proof-of-Possession in the validator pool after verifying both.
    ///
    /// Self-authorized — any bonded validator may call this for themselves; no admin key
    /// required. Validates the PoP cryptographically before writing, so the pool never holds
    /// an unverified BLS key. Like `apply_admin_action`, validation happens before mutation,
    /// so a rejected payload does not consume the sender's nonce.
    /// Décode et valide une `RegisterBlsKeyPayload` pour `owner` : longueurs, point G1 valide,
    /// PoP liée à `(clé, owner, chain_id)` (VINX-11), et unicité de la clé G1 dans le pool.
    ///
    /// Partagé par `apply_register_bls_key` et par le chemin de bonding (ADR 0075 §3.1) —
    /// une clé qui entre dans l'état doit passer les mêmes contrôles quel que soit le
    /// chemin, sinon l'un des deux devient la faille.
    fn validate_bls_registration(
        &self,
        owner: Address,
        payload_bytes: &[u8],
    ) -> Result<BlsRegistration, CoreError> {
        let payload: RegisterBlsKeyPayload = borsh::from_slice(payload_bytes).map_err(|_| {
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
        bls_pub
            .verify_pop(&BlsSignature(pop_arr), owner.as_bytes(), self.chain_id)
            .map_err(|_| {
                CoreError::InvalidTransaction(
                    "BLS Proof-of-Possession verification failed".to_string(),
                )
            })?;
        if self
            .validator_pool
            .iter()
            .any(|(a, e)| a != &owner && e.bls_pub_key.as_deref() == Some(&pk_arr[..]))
        {
            return Err(CoreError::InvalidTransaction(
                "BLS public key already registered by another validator".to_string(),
            ));
        }
        if payload.operator == Some(owner) {
            return Err(CoreError::InvalidTransaction(
                "the operator must differ from the owner (omit it instead)".to_string(),
            ));
        }
        Ok((payload.bls_pub_key, payload.bls_pop, payload.operator))
    }

    fn apply_register_bls_key(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let account = self
            .accounts
            .get(&tx.sender())
            .ok_or(CoreError::InsufficientBalance)?;
        if account.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: account.nonce,
                got: tx.nonce,
            });
        }

        if !self.validator_pool.contains_key(&tx.sender()) {
            return Err(CoreError::InvalidTransaction(
                "only bonded validators may register a BLS key".to_string(),
            ));
        }

        // Contrôle unique, partagé avec le chemin de bonding (ADR 0075 §3.1) : longueurs,
        // point G1 valide, PoP liée à (clé, validateur, chain_id), unicité de la clé.
        // Validation avant mutation — un payload refusé ne consomme pas le nonce.
        let (pk, pop, operator) = self.validate_bls_registration(tx.sender(), &tx.payload)?;

        let entry = self
            .validator_pool
            .get_mut(&tx.sender())
            .expect("existence checked above");
        entry.bls_pub_key = Some(pk);
        entry.bls_pop = Some(pop);
        entry.operator = operator;

        self.accounts
            .get_mut(&tx.sender())
            .expect("existence checked above")
            .nonce += 1;
        self.mark_dirty(&tx.sender());
        tracing::info!(validator = %tx.sender(), "ADR 0046: BLS key registered");
        Ok(())
    }

    fn apply_unjail(&mut self, tx: &Transaction) -> Result<(), CoreError> {
        let account = self
            .accounts
            .get(&tx.sender())
            .ok_or(CoreError::InsufficientBalance)?;
        if account.nonce != tx.nonce {
            return Err(CoreError::InvalidNonce {
                expected: account.nonce,
                got: tx.nonce,
            });
        }
        // ADR 0084 S5: `to` is the validator; the owner or its registered operator signs.
        let target = tx.to;
        let operator = self.validator_pool.get(&target).and_then(|e| e.operator);
        if tx.sender() != target && Some(tx.sender()) != operator {
            return Err(CoreError::Unauthorized);
        }
        let height = self.block_height;
        if !reliability::try_unjail(&mut self.reliability, &target, height) {
            return Err(CoreError::InvalidTransaction(
                "unjail failed: validator is not jailed or cooldown has not elapsed".to_string(),
            ));
        }
        self.accounts
            .get_mut(&tx.sender())
            .expect("existence checked above")
            .nonce += 1;
        self.mark_dirty(&tx.sender());
        tracing::info!(validator = %target, height, "ADR 0027: validator unjailed");
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

/// Key of an account in the state tree (ADR 0083).
pub fn account_key(addr: &Address) -> Hash32 {
    let mut buf = Vec::with_capacity(ACCOUNT_KEY_DST.len() + 32);
    buf.extend_from_slice(ACCOUNT_KEY_DST);
    buf.extend_from_slice(addr.as_bytes());
    hash256(&buf)
}

const ACCOUNT_KEY_DST: &[u8] = b"VINX_ACCOUNT_KEY";

/// Proof of one account against a `state_root` (ADR 0083). Verify with
/// [`AccountProof::verify`]; the `state_root` itself is trusted through the block
/// header, which a quorum certificate commits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountProof {
    pub key: Hash32,
    /// `hash_account` of the account, or `None` when it does not exist.
    pub leaf_value: Option<Hash32>,
    pub proof: SmtProof,
    pub accounts_root: Hash32,
    pub consensus_root: Hash32,
}

impl AccountProof {
    /// Checks the proof against a trusted `state_root` and, when `account` is given, that
    /// it is exactly the proven account (`None` proves absence).
    pub fn verify(&self, state_root: &Hash32, addr: &Address, account: Option<&Account>) -> bool {
        if self.key != account_key(addr) {
            return false;
        }
        let expected = account.map(hash_account);
        if expected != self.leaf_value {
            return false;
        }
        state_root_of(&self.accounts_root, &self.consensus_root) == *state_root
            && self
                .proof
                .verify(&self.accounts_root, &self.key, self.leaf_value.as_ref())
    }
}

/// `state_root = H(DST ‖ accounts_root ‖ consensus_root)`.
pub fn state_root_of(accounts_root: &Hash32, consensus_root: &Hash32) -> Hash32 {
    let mut buf = Vec::with_capacity(32 + 32 + STATE_ROOT_DST.len());
    buf.extend_from_slice(STATE_ROOT_DST);
    buf.extend_from_slice(accounts_root);
    buf.extend_from_slice(consensus_root);
    hash256(&buf)
}

/// Leaf value of an account in the state tree.
pub fn hash_account(account: &Account) -> Hash32 {
    let addr = account.address.as_bytes();
    let mut buf = Vec::with_capacity(addr.len() + 16 + 8 + 16);
    buf.extend_from_slice(addr);
    buf.extend_from_slice(&account.balance.atoms().to_be_bytes());
    buf.extend_from_slice(&account.nonce.to_be_bytes());
    buf.extend_from_slice(&account.staked.atoms().to_be_bytes());
    hash256(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_tree_incremental_matches_rebuild_and_proves() {
        let addrs: Vec<Address> = (0..50)
            .map(|_| Address::from_public_key(&vinx_crypto::KeyPair::generate().public_key()))
            .collect();
        let mut s = WorldState::new();
        for (i, a) in addrs.iter().enumerate() {
            s.credit_for_test(*a, Amount::from_atoms(1_000 + i as u128));
            if i % 7 == 0 {
                s.compute_state_root(); // interleave incremental flushes
            }
        }
        s.accounts.remove(&addrs[3]);
        s.mark_dirty(&addrs[3]);
        let root = s.compute_state_root();

        // Same accounts, tree built from scratch (as after a reload).
        let mut fresh = s.clone();
        fresh.tree_synced = false;
        assert_eq!(fresh.compute_state_root(), root);

        let p = s.account_proof(&addrs[5]);
        assert!(p.verify(&root, &addrs[5], s.get_account(&addrs[5])));
        let p = s.account_proof(&addrs[3]);
        assert!(
            p.verify(&root, &addrs[3], None),
            "removed account proven absent"
        );
        let wrong = Account::new_with_balance(addrs[5], Amount::from_atoms(1));
        let p = s.account_proof(&addrs[5]);
        assert!(!p.verify(&root, &addrs[5], Some(&wrong)));
    }
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
        state.admin_address = Some(admin_addr);
        state.credit_for_test(admin_addr, Amount::from_vinx(1_000));
        (state, admin_kp, admin_addr)
    }

    // ─── Fair launch: emission, fees, bond, unbonding, slashing ──────────────
    use vinx_core::amount::{cumulative_emission_atoms, EMISSION_T_HALF_SECS};

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
        let (fees, emission) = s.settle_block(&producer, 1_000);
        assert_eq!(fees, Amount::ZERO);
        assert_eq!(emission, Amount::ZERO);
        assert_eq!(s.emission_epoch_ts, 1_000);
        assert_eq!(s.emitted_atoms, 0);
    }

    #[test]
    fn test_emission_rewards_producer_and_conserves_supply() {
        let mut s = WorldState::new();
        let (_, producer) = kp_addr();
        s.settle_block(&producer, 0); // establish epoch at t=0
                                      // One full half-life later: ~50% of the supply has been minted.
        let (_, emission) = s.settle_block(&producer, EMISSION_T_HALF_SECS);
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
        s.settle_block(&producer, 0);
        let (_, emission) = s.settle_block(&producer, EMISSION_T_HALF_SECS / 20); // ~1 year
        assert!(emission > Amount::ZERO);
        // ADR 0028: producer receives PROPOSER_SHARE_BPS (20%) of the emission.
        let expected_balance =
            Amount::from_atoms(emission.atoms() * PROPOSER_SHARE_BPS / BPS_DENOM);
        assert_eq!(s.accounts[&producer].balance, expected_balance);
    }

    #[test]
    fn test_fee_split_between_producer_and_epoch_pot() {
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
        // Fee is collected at apply time; the pot moves only when the block settles.
        assert_eq!(s.epoch_dist_emission_pot, pot_before);
        // Settling (epoch established, no emission) splits the fee (ADR 0081 D4).
        s.settle_block(&producer, 100);
        let producer_part = Amount::from_atoms(fee.atoms() * FEE_PRODUCER_SHARE_BPS / BPS_DENOM);
        let cosigner_part = fee.checked_sub(producer_part).unwrap();
        assert!(producer_part > Amount::ZERO && cosigner_part > Amount::ZERO);
        assert_eq!(s.accounts[&producer].balance, producer_part);
        assert_eq!(
            s.epoch_dist_emission_pot,
            pot_before.saturating_add(cosigner_part)
        );
        // Nothing burned: circulation + pot still equals everything emitted.
        assert_eq!(
            s.circulating_supply
                .saturating_add(s.epoch_dist_emission_pot),
            Amount::from_vinx(1_000)
        );
    }

    #[test]
    fn test_admin_authority_expires_after_tenure() {
        // ADR 0081 D7b: the admin acts during its tenure, then never again.
        use vinx_core::amount::ADMIN_TENURE_SECS;
        use vinx_core::GovernanceAction;
        let (mut state, admin_kp, _) = admin_state();
        state.emission_started = true;
        state.emission_epoch_ts = 1_000;
        let fee_floor = |atoms, nonce| {
            Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::UpdateFeeFloor { atoms },
                nonce,
            )
        };
        state.set_block_context(1_000 + ADMIN_TENURE_SECS - 1);
        assert!(!state.admin_tenure_expired());
        state.apply_transaction(&fee_floor(200_000, 0)).unwrap();

        state.set_block_context(1_000 + ADMIN_TENURE_SECS);
        assert!(state.admin_tenure_expired());
        assert_eq!(
            state.apply_transaction(&fee_floor(300_000, 1)),
            Err(CoreError::Unauthorized)
        );
        // Neither a rotation nor a committee can revive it.
        let (_, other) = kp_addr();
        assert_eq!(
            state.apply_transaction(&Transaction::new_admin_action(
                &admin_kp,
                &GovernanceAction::RotateAdmin(other),
                1,
            )),
            Err(CoreError::Unauthorized)
        );
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
        s.settle_block(&producer, 1_000 + UNBONDING_SECS - 1);
        assert_eq!(s.pending_unbonds.len(), 1);
        // At unlock: the bond returns to addr's balance.
        s.settle_block(&producer, 1_000 + UNBONDING_SECS);
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
        s.apply_transaction(&stake_with_bls(&kp, stake, 0, s.chain_id))
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
        s.settle_block(&addr, 1_000 + UNBONDING_SECS);
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
        s.apply_transaction(&stake_with_bls(
            &kp,
            Amount::from_atoms(bond),
            0,
            s.chain_id,
        ))
        .unwrap();
        s.validator_set = ValidatorSet::single(addr);
        s.set_block_context(1_000);
        // Unstaking any of the bond would drop below the minimum → rejected.
        let bad = Transaction::new_unstake(&kp, Amount::from_atoms(bond / 2), Amount::ZERO, 1);
        assert!(s.apply_transaction(&bad).is_err());
    }

    // A prevote by `validator` at (5, 0) for `value`, signed with `bls_sk` (chain devnet).
    fn signed_prevote(
        bls_sk: &vinx_crypto::BlsSecretKey,
        validator: Address,
        value: Option<[u8; 32]>,
    ) -> vinx_core::SignedVote {
        let mut v = vinx_core::SignedVote {
            kind: vinx_core::VoteKind::Prevote,
            height: 5,
            round: 0,
            value,
            validator,
            signature: vec![],
        };
        v.signature = bls_sk.sign(&v.sign_bytes(CHAIN_ID_DEVNET)).0.to_vec();
        v
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

        // Forged: two conflicting votes signed with the attacker's key, not the victim's.
        let evidence = VoteEquivocation {
            vote_a: signed_prevote(&attacker_bls_sk, victim, Some([0xAA; 32])),
            vote_b: signed_prevote(&attacker_bls_sk, victim, Some([0xBB; 32])),
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

        // Two genuinely signed, conflicting votes at the same (height, round) = equivocation.
        let evidence = VoteEquivocation {
            vote_a: signed_prevote(&victim_bls_sk, victim, Some([0xAA; 32])),
            vote_b: signed_prevote(&victim_bls_sk, victim, None),
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

    // ─── ADR 0084: staking ───────────────────────────────────────────────────

    /// 20 equally bonded validators (5 % of the power each) with BLS keys.
    fn twenty_validators() -> (WorldState, Vec<(Address, vinx_crypto::BlsSecretKey)>) {
        let mut s = WorldState::new();
        let vals: Vec<_> = (0..20)
            .map(|_| (kp_addr().1, vinx_crypto::BlsSecretKey::generate()))
            .collect();
        for (a, sk) in &vals {
            s.set_staked_for_test(a, Amount::from_vinx(10_000));
            register_bls_key_for_test(&mut s, *a, sk);
        }
        s.validator_set = s.weighted_validator_set(vals.iter().map(|v| v.0).collect());
        s.block_height = 10;
        (s, vals)
    }

    fn equivocation(sk: &vinx_crypto::BlsSecretKey, v: Address) -> VoteEquivocation {
        VoteEquivocation {
            vote_a: signed_prevote(sk, v, Some([0xAA; 32])),
            vote_b: signed_prevote(sk, v, None),
        }
    }

    #[test]
    fn test_correlated_slash_grows_with_simultaneous_faults() {
        let (mut s, vals) = twenty_validators();
        let (rep_kp, rep) = kp_addr();
        s.credit_for_test(rep, Amount::from_vinx(10));

        // Alone with 5 % of the power: 5 % + 3 × 5 % = 20 % of 10 000 VINX.
        let (v1, sk1) = &vals[0];
        let tx = Transaction::new_slash_validator(&rep_kp, *v1, &equivocation(sk1, *v1), 0);
        s.apply_transaction(&tx).unwrap();
        assert_eq!(s.accounts[v1].staked, Amount::ZERO);
        let returned: u128 = s
            .pending_unbonds
            .iter()
            .filter(|u| &u.address == v1)
            .map(|u| u.amount.atoms())
            .sum();
        assert_eq!(
            returned,
            Amount::from_vinx(8_000).atoms(),
            "80 % returns, unbonding"
        );
        assert!(s.banned_validator_keys.contains(v1));
        assert!(!s.validator_pool.contains_key(v1), "excluded for good");

        // A second fault in the window: 5 % + 3 × (5 % + ~5.3 %) ≈ 36 %.
        let (v2, sk2) = &vals[1];
        let tx = Transaction::new_slash_validator(&rep_kp, *v2, &equivocation(sk2, *v2), 1);
        s.apply_transaction(&tx).unwrap();
        let returned2: u128 = s
            .pending_unbonds
            .iter()
            .filter(|u| &u.address == v2)
            .map(|u| u.amount.atoms())
            .sum();
        assert!(
            returned2 < Amount::from_vinx(6_500).atoms()
                && returned2 > Amount::from_vinx(6_000).atoms(),
            "correlated penalty ≈ 36 %: returned {returned2}"
        );

        // The same validator cannot be slashed twice.
        let tx = Transaction::new_slash_validator(&rep_kp, *v1, &equivocation(sk1, *v1), 2);
        assert!(s.apply_transaction(&tx).is_err());
    }

    #[test]
    fn test_slash_evidence_expires_with_unbonding() {
        let (mut s, vals) = twenty_validators();
        let (rep_kp, rep) = kp_addr();
        s.credit_for_test(rep, Amount::from_vinx(10));
        let (v, sk) = &vals[0];
        // Evidence is at height 5; move just past the window.
        s.block_height = 5 + EVIDENCE_MAX_AGE_SECS / s.block_time_secs + 1;
        let tx = Transaction::new_slash_validator(&rep_kp, *v, &equivocation(sk, *v), 0);
        assert!(s.apply_transaction(&tx).is_err(), "expired evidence");
        s.block_height -= 1;
        s.apply_transaction(&tx)
            .expect("evidence at the edge of the window");
    }

    #[test]
    fn test_operator_may_unjail_but_not_strangers() {
        let (owner_kp, owner) = kp_addr();
        let (op_kp, op) = kp_addr();
        let (x_kp, x) = kp_addr();
        let mut s = WorldState::new();
        for a in [owner, op, x] {
            s.credit_for_test(a, Amount::from_vinx(1));
        }
        let mut e = vinx_core::ValidatorPoolEntry::new(MIN_VALIDATOR_BOND_ATOMS, 0);
        e.operator = Some(op);
        s.validator_pool.insert(owner, e);
        s.reliability.entry(owner).or_default().jailed_until = Some(0);
        s.block_height = 10;

        let tx = Transaction::new_unjail_for(&x_kp, owner, 0);
        assert!(matches!(
            s.apply_transaction(&tx),
            Err(CoreError::Unauthorized)
        ));
        let _ = x;
        let tx = Transaction::new_unjail_for(&op_kp, owner, 0);
        s.apply_transaction(&tx).expect("operator unjails");
        assert!(!s.reliability[&owner].is_jailed());
        let _ = owner_kp;
    }

    #[test]
    fn test_certificate_survives_key_rotation_in_its_block() {
        // Block 1 is voted with the old key; a key change applied *in* block 1 must not
        // make its certificate unverifiable at block 2.
        use vinx_core::{BlockHeader, CommitCert};
        let (_, v) = kp_addr();
        let old = vinx_crypto::BlsSecretKey::generate();
        let mut s = WorldState::new();
        s.set_staked_for_test(&v, Amount::from_vinx(10_000));
        register_bls_key_for_test(&mut s, v, &old);
        s.validator_set = ValidatorSet::single(v);
        let header = |height: u64, prev: [u8; 32]| BlockHeader {
            height,
            round: 0,
            prev_hash: prev,
            timestamp: height,
            validator: v,
            tx_count: 0,
            state_root: [0; 32],
            base_fee: 0,
            receipts_root: [0; 32],
            last_commit_hash: [0; 32],
        };
        let h1 = header(1, [0; 32]);
        s.begin_block(&h1, None).unwrap();
        s.block_height = 1;
        // Rotation inside block 1.
        register_bls_key_for_test(&mut s, v, &vinx_crypto::BlsSecretKey::generate());
        let hash1 = h1.hash();
        let sig = old.sign(&vinx_core::consensus::vote_sign_bytes(
            CHAIN_ID_DEVNET,
            vinx_core::VoteKind::Precommit,
            1,
            0,
            Some(&hash1),
        ));
        let cert = CommitCert::from_precommits(1, 0, hash1, vec![(0, sig)]).unwrap();
        s.begin_block(&header(2, hash1), Some(&cert))
            .expect("certificate checked against the keys its voters used");
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
        assert!(
            s.apply_transaction(&tx).is_err(),
            "unjail before cooldown must fail"
        );
        // After cooldown: unjail should succeed.
        s.block_height = UNJAIL_COOLDOWN_HEIGHTS + 2;
        let tx = Transaction::new_unjail(&kp, 0);
        s.apply_transaction(&tx).unwrap();
        assert!(!s
            .reliability
            .get(&addr)
            .map(|r| r.is_jailed())
            .unwrap_or(false));
    }

    #[test]
    fn test_unjail_not_jailed_fails() {
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_vinx(1));
        // Validator was never jailed.
        let tx = Transaction::new_unjail(&kp, 0);
        assert!(
            s.apply_transaction(&tx).is_err(),
            "unjail when not jailed must fail"
        );
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

    /// VINX-04: the root is no longer the bare accounts root, so an empty account set no
    /// longer implies an all-zeros root — it still commits to the consensus state. The
    /// empty *accounts* subtree is what is zero.
    #[test]
    fn test_state_root_empty_accounts_still_commits_consensus() {
        let mut s = WorldState::new();
        let root = s.compute_state_root();
        assert_ne!(
            root, [0u8; 32],
            "an empty account set must still commit to consensus state"
        );
        // Deterministic: same state, same root.
        let mut s2 = WorldState::new();
        assert_eq!(root, s2.compute_state_root());
    }

    /// VINX-04 regression — mutating any consensus field must move the root. Before the
    /// fix, two states differing on the validator set, the admin key and the epoch beacon
    /// produced *equal* roots, so the divergence was silent on every validation path.
    #[test]
    #[allow(clippy::type_complexity)]
    fn test_state_root_commits_to_consensus_state() {
        let addr = || Address::from_public_key(&KeyPair::generate().public_key());

        let base = WorldState::new();
        let baseline = base.clone().compute_state_root();

        // Each mutation is applied to an otherwise identical state.
        let mut mutations: Vec<(&str, Box<dyn Fn(&mut WorldState)>)> = Vec::new();
        let v = addr();
        mutations.push((
            "validator_set",
            Box::new(move |s: &mut WorldState| {
                s.validator_set.add(v);
            }),
        ));
        let a = addr();
        mutations.push((
            "admin_address",
            Box::new(move |s: &mut WorldState| {
                s.admin_address = Some(a);
            }),
        ));
        mutations.push((
            "chain_id",
            Box::new(|s: &mut WorldState| {
                s.chain_id = s.chain_id.wrapping_add(1);
            }),
        ));
        mutations.push((
            "min_validator_bond_atoms",
            Box::new(|s: &mut WorldState| {
                s.min_validator_bond_atoms += 1;
            }),
        ));
        mutations.push((
            "emitted_atoms",
            Box::new(|s: &mut WorldState| {
                s.emitted_atoms += 1;
            }),
        ));
        mutations.push((
            "destroyed_atoms",
            Box::new(|s: &mut WorldState| {
                s.destroyed_atoms += 1;
            }),
        ));
        let b = addr();
        mutations.push((
            "banned_validator_keys",
            Box::new(move |s: &mut WorldState| {
                s.banned_validator_keys.insert(b);
            }),
        ));
        let p = addr();
        mutations.push((
            "validator_pool",
            Box::new(move |s: &mut WorldState| {
                s.validator_pool
                    .insert(p, vinx_core::ValidatorPoolEntry::new(1, 0));
            }),
        ));

        for (name, mutate) in mutations {
            let mut s = base.clone();
            mutate(&mut s);
            assert_ne!(
                s.compute_state_root(),
                baseline,
                "state_root must change when `{name}` changes"
            );
        }
    }

    /// The sorted encoding of `banned_validator_keys` must not depend on insertion order,
    /// or two honest nodes would compute different roots for the same state.
    #[test]
    fn test_consensus_root_is_insertion_order_independent() {
        let a = Address::from_public_key(&KeyPair::generate().public_key());
        let b = Address::from_public_key(&KeyPair::generate().public_key());

        let mut s1 = WorldState::new();
        s1.banned_validator_keys.insert(a);
        s1.banned_validator_keys.insert(b);

        let mut s2 = WorldState::new();
        s2.banned_validator_keys.insert(b);
        s2.banned_validator_keys.insert(a);

        assert_eq!(s1.compute_consensus_root(), s2.compute_consensus_root());
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
        s.credit_for_test(addr, Amount::from_vinx(100));
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
        s1.credit_for_test(addr1, Amount::from_vinx(50));
        s1.credit_for_test(addr2, Amount::from_vinx(200));

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
        s.credit_for_test(addr, Amount::from_vinx(10));
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
    /// Bond de test portant une clé BLS — ce qu'un validateur réel doit faire depuis
    /// ADR 0075 §3.1. Les tests qui bondent pour *entrer* au pool passent par ici.
    fn stake_with_bls(kp: &KeyPair, amount: Amount, nonce: u64, chain_id: u32) -> Transaction {
        Transaction::new_stake_with_bls(
            kp,
            amount,
            Amount::ZERO,
            nonce,
            &vinx_crypto::BlsSecretKey::generate(),
            chain_id,
        )
    }

    fn validator_pool_state() -> (WorldState, KeyPair, Address) {
        use vinx_core::validator_pool::ValidatorPoolEntry;
        let mut s = WorldState::new();
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        s.credit_for_test(addr, Amount::from_vinx(1_000));
        // Insert directly into the pool (bypasses admission rules for test simplicity).
        s.validator_pool
            .insert(addr, ValidatorPoolEntry::new(MIN_VALIDATOR_BOND_ATOMS, 0));
        (s, kp, addr)
    }

    #[test]
    fn test_register_bls_key_happy_path() {
        use vinx_core::RegisterBlsKeyPayload;
        use vinx_crypto::BlsSecretKey;

        let (mut s, kp, addr) = validator_pool_state();

        let bls_sk = BlsSecretKey::generate();
        let bls_pk = bls_sk.public_key();
        let pop = bls_sk.proof_of_possession(addr.as_bytes(), s.chain_id);
        let payload = RegisterBlsKeyPayload {
            bls_pub_key: bls_pk.0.to_vec(),
            bls_pop: pop.0.to_vec(),
            operator: None,
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
        let pop = bls_sk.proof_of_possession(addr.as_bytes(), s.chain_id);
        let payload = RegisterBlsKeyPayload {
            bls_pub_key: bls_sk.public_key().0.to_vec(),
            bls_pop: pop.0.to_vec(),
            operator: None,
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
        let bad_pop = wrong_sk.proof_of_possession(addr.as_bytes(), s.chain_id);
        let payload = RegisterBlsKeyPayload {
            bls_pub_key: bls_sk.public_key().0.to_vec(),
            bls_pop: bad_pop.0.to_vec(),
            operator: None,
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
        use vinx_core::CoreError;
        use vinx_core::RegisterBlsKeyPayload;

        let (mut s, kp, _addr) = validator_pool_state();

        // 47-byte pubkey (should be 48).
        let bad_pk = RegisterBlsKeyPayload {
            bls_pub_key: vec![0u8; 47],
            bls_pop: vec![0u8; 96],
            operator: None,
        };
        assert!(matches!(
            s.apply_transaction(&Transaction::new_register_bls_key(&kp, &bad_pk, 0)),
            Err(CoreError::InvalidTransaction(_))
        ));

        // 95-byte PoP (should be 96).
        let bad_pop = RegisterBlsKeyPayload {
            bls_pub_key: vec![0u8; 48],
            bls_pop: vec![0u8; 95],
            operator: None,
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
        let pop_a = sk_a.proof_of_possession(addr.as_bytes(), s.chain_id);
        let payload_a = RegisterBlsKeyPayload {
            bls_pub_key: sk_a.public_key().0.to_vec(),
            bls_pop: pop_a.0.to_vec(),
            operator: None,
        };
        s.apply_transaction(&Transaction::new_register_bls_key(&kp, &payload_a, 0))
            .unwrap();

        // Re-register with key B — should overwrite.
        let sk_b = BlsSecretKey::generate();
        let pop_b = sk_b.proof_of_possession(addr.as_bytes(), s.chain_id);
        let payload_b = RegisterBlsKeyPayload {
            bls_pub_key: sk_b.public_key().0.to_vec(),
            bls_pop: pop_b.0.to_vec(),
            operator: None,
        };
        s.apply_transaction(&Transaction::new_register_bls_key(&kp, &payload_b, 1))
            .unwrap();

        let entry = &s.validator_pool[&addr];
        assert_eq!(
            entry.bls_pub_key.as_deref(),
            Some(sk_b.public_key().0.as_slice())
        );
    }

    // ─── ADR 0038: open PoS admission (bond floor & active-set governance) ────

    #[test]
    fn test_stake_auto_enters_pool_at_bond_floor() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
        assert!(!s.validator_pool.contains_key(&addr));

        s.apply_transaction(&stake_with_bls(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
            0,
            s.chain_id,
        ))
        .unwrap();

        assert!(
            s.validator_pool.contains_key(&addr),
            "pool entry must be created on floor stake"
        );
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

        assert!(
            !s.validator_pool.contains_key(&addr),
            "sub-floor bond must not enter pool"
        );
    }

    #[test]
    fn test_stake_banned_address_not_admitted_to_pool() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS));
        s.banned_validator_keys.insert(addr);

        s.apply_transaction(&stake_with_bls(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
            0,
            s.chain_id,
        ))
        .unwrap();

        assert!(
            !s.validator_pool.contains_key(&addr),
            "banned address must not enter pool"
        );
    }

    // ADR 0036: when bond drops below floor the validator is queued for exit, not
    // immediately moved to Unbonding. After tick_epoch_close the exit is processed.
    #[test]
    fn test_unstake_below_floor_queues_exit_then_epoch_close_unbonds() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        use vinx_core::validator_pool::PoolStatus;
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS * 2));

        // Stake enough to enter the pool.
        s.apply_transaction(&stake_with_bls(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
            0,
            s.chain_id,
        ))
        .unwrap();
        assert!(s.validator_pool.contains_key(&addr));

        // Unstake all — bond drops below floor → queued for exit (ADR 0036).
        s.set_block_context(1_000);
        s.apply_transaction(&Transaction::new_unstake(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
            Amount::ZERO,
            1,
        ))
        .unwrap();

        // Immediately after unstake: still in Warmup (not yet Unbonding).
        assert_eq!(s.exit_queue.len(), 1, "exit request must be queued");
        assert_eq!(s.exit_queue[0].address, addr);
        assert!(
            !matches!(s.validator_pool[&addr].status, PoolStatus::Unbonding { .. }),
            "pool entry must NOT yet be Unbonding before epoch close"
        );

        // Tick epoch close — exit is processed and validator moves to Unbonding.
        s.tick_epoch_close();
        assert!(
            matches!(s.validator_pool[&addr].status, PoolStatus::Unbonding { .. }),
            "pool entry must be Unbonding after epoch close processes the exit queue"
        );
        assert!(s.exit_queue.is_empty(), "exit queue must be drained");
    }

    // ─── ADR 0038: UpdateMinValidatorBond governance ──────────────────────────

    #[test]
    fn test_update_min_validator_bond_happy_path() {
        use vinx_core::amount::{BOND_STEP_BPS, BPS_DENOM, MIN_VALIDATOR_BOND_ATOMS};
        use vinx_core::GovernanceAction;
        let (mut s, admin_kp, _) = admin_state();
        // Increase by exactly BOND_STEP_BPS (25 %).
        let new_atoms =
            MIN_VALIDATOR_BOND_ATOMS + MIN_VALIDATOR_BOND_ATOMS * BOND_STEP_BPS / BPS_DENOM;

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
        let too_large =
            MIN_VALIDATOR_BOND_ATOMS + MIN_VALIDATOR_BOND_ATOMS * BOND_STEP_BPS / BPS_DENOM + 1;

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
        use vinx_core::amount::{
            BOND_COOLDOWN_SECS, BOND_STEP_BPS, BPS_DENOM, MIN_VALIDATOR_BOND_ATOMS,
        };
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

    // ─── ADR 0036: validator churn bounds ─────────────────────────────────────

    fn bonded_active_validator() -> (WorldState, vinx_crypto::KeyPair, Address) {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        use vinx_core::validator_pool::PoolStatus;
        let (kp, addr) = kp_addr();
        let mut s = WorldState::new();
        s.credit_for_test(addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS * 3));
        // Stake bond.
        s.apply_transaction(&stake_with_bls(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
            0,
            s.chain_id,
        ))
        .unwrap();
        // Force pool entry to Active so floor checks are meaningful.
        s.validator_pool.get_mut(&addr).unwrap().status = PoolStatus::Active;
        (s, kp, addr)
    }

    #[test]
    fn test_exit_queue_dedup_same_validator() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        let (mut s, kp, _addr) = bonded_active_validator();
        s.set_block_context(1_000);

        // First unstake — drops below floor → queued.
        s.apply_transaction(&Transaction::new_unstake(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS / 2),
            Amount::ZERO,
            1,
        ))
        .unwrap();
        assert_eq!(
            s.exit_queue.len(),
            1,
            "first unstake below floor queues once"
        );

        // Second unstake — already queued → no duplicate.
        s.apply_transaction(&Transaction::new_unstake(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS / 4),
            Amount::ZERO,
            2,
        ))
        .unwrap();
        assert_eq!(
            s.exit_queue.len(),
            1,
            "second unstake must not add a duplicate exit request"
        );
    }

    #[test]
    fn test_exit_queue_rate_limits_to_max_per_epoch() {
        use vinx_core::amount::{MAX_VALIDATOR_EXITS_PER_EPOCH, MIN_VALIDATOR_BOND_ATOMS};
        use vinx_core::validator_pool::{PoolStatus, ValidatorPoolEntry};

        // Spin up more validators than MAX_VALIDATOR_EXITS_PER_EPOCH (=2).
        let n_exit = MAX_VALIDATOR_EXITS_PER_EPOCH + 2;
        let mut s = WorldState::new();
        let mut kps_addrs: Vec<(vinx_crypto::KeyPair, Address)> = Vec::new();

        for i in 0..n_exit {
            let (kp, addr) = kp_addr();
            s.credit_for_test(addr, Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS * 2));
            s.apply_transaction(&stake_with_bls(
                &kp,
                Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
                0,
                s.chain_id,
            ))
            .unwrap();
            // Set Active so floor checks apply.
            s.validator_pool.get_mut(&addr).unwrap().status = PoolStatus::Active;
            kps_addrs.push((kp, addr));
            let _ = i;
        }

        // Add extra Benched validators so MIN_ACTIVE_SET_SIZE is not threatened.
        for _ in 0..10 {
            let (_, extra) = kp_addr();
            let entry = ValidatorPoolEntry {
                bond_atoms: MIN_VALIDATOR_BOND_ATOMS,
                bonded_since_ts: 0,
                status: PoolStatus::Benched,
                cosign_count_in_window: 0,
                eligible_blocks_in_window: 0,
                bls_pub_key: None,
                bls_pop: None,
                operator: None,
            };
            s.validator_pool.insert(extra, entry);
        }

        s.set_block_context(1_000);
        // Unstake all n_exit validators below the floor.
        for (kp, _) in &kps_addrs {
            let addr = Address::from_public_key(&kp.public_key());
            let _ = s.apply_transaction(&Transaction::new_unstake(
                kp,
                Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
                Amount::ZERO,
                1,
            ));
            let _ = addr;
        }
        assert_eq!(s.exit_queue.len(), n_exit, "all {n_exit} exits queued");

        // First epoch close: exactly MAX exits processed, rest stay in queue.
        s.tick_epoch_close();
        let unbonding_count = s
            .validator_pool
            .values()
            .filter(|e| matches!(e.status, PoolStatus::Unbonding { .. }))
            .count();
        assert_eq!(
            unbonding_count, MAX_VALIDATOR_EXITS_PER_EPOCH,
            "first epoch close must process exactly MAX_VALIDATOR_EXITS_PER_EPOCH"
        );
        assert_eq!(
            s.exit_queue.len(),
            n_exit - MAX_VALIDATOR_EXITS_PER_EPOCH,
            "remaining exits stay in queue for next epoch"
        );

        // Second epoch close: clears the remaining exits (≤ MAX).
        s.tick_epoch_close();
        let still_queued = s.exit_queue.len();
        assert_eq!(
            still_queued, 0,
            "all exits processed after two epoch closes"
        );
    }

    #[test]
    fn test_exit_queue_fifo_by_height_then_address() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        use vinx_core::validator_pool::{PoolStatus, ValidatorPoolEntry};

        let mut s = WorldState::new();

        // Create 3 validators and manually insert their exit requests in non-FIFO order.
        let mut addrs: Vec<Address> = Vec::new();
        for _ in 0..3 {
            let (_, addr) = kp_addr();
            let entry = ValidatorPoolEntry {
                bond_atoms: 0, // below floor
                bonded_since_ts: 0,
                status: PoolStatus::Active,
                cosign_count_in_window: 0,
                eligible_blocks_in_window: 0,
                bls_pub_key: None,
                bls_pop: None,
                operator: None,
            };
            s.validator_pool.insert(addr, entry);
            addrs.push(addr);
        }
        addrs.sort(); // sort so we can predict tiebreaker order

        // Insert requests at different heights (out of order) to test FIFO.
        use vinx_core::ValidatorExitRequest;
        s.exit_queue = vec![
            ValidatorExitRequest {
                address: addrs[2],
                request_height: 10,
                unlock_ts: 9999,
            },
            ValidatorExitRequest {
                address: addrs[0],
                request_height: 5,
                unlock_ts: 9999,
            },
            ValidatorExitRequest {
                address: addrs[1],
                request_height: 7,
                unlock_ts: 9999,
            },
        ];

        // Add extra Benched validators so floor check doesn't block any exit.
        for _ in 0..20 {
            let (_, extra) = kp_addr();
            let entry = ValidatorPoolEntry {
                bond_atoms: MIN_VALIDATOR_BOND_ATOMS,
                bonded_since_ts: 0,
                status: PoolStatus::Benched,
                cosign_count_in_window: 0,
                eligible_blocks_in_window: 0,
                bls_pub_key: None,
                bls_pop: None,
                operator: None,
            };
            s.validator_pool.insert(extra, entry);
        }

        s.tick_epoch_close(); // processes first 2 (MAX_VALIDATOR_EXITS_PER_EPOCH)

        // Height 5 (addrs[0]) and height 7 (addrs[1]) must be Unbonding.
        assert!(
            matches!(
                s.validator_pool[&addrs[0]].status,
                PoolStatus::Unbonding { .. }
            ),
            "earliest request (height 5) must be processed first"
        );
        assert!(
            matches!(
                s.validator_pool[&addrs[1]].status,
                PoolStatus::Unbonding { .. }
            ),
            "second earliest request (height 7) must be processed second"
        );
        // Height 10 (addrs[2]) still queued.
        assert!(
            !matches!(
                s.validator_pool[&addrs[2]].status,
                PoolStatus::Unbonding { .. }
            ),
            "latest request (height 10) must stay queued after first epoch close"
        );
    }

    #[test]
    fn test_exit_queue_floor_guard_defers_exit() {
        use vinx_core::amount::MIN_ACTIVE_SET_SIZE;
        use vinx_core::validator_pool::{PoolStatus, ValidatorPoolEntry};
        use vinx_core::ValidatorExitRequest;

        // Build a pool with exactly MIN_ACTIVE_SET_SIZE eligible validators.
        let mut s = WorldState::new();
        let mut targets: Vec<Address> = Vec::new();
        for _ in 0..(MIN_ACTIVE_SET_SIZE as usize) {
            let (_, addr) = kp_addr();
            let entry = ValidatorPoolEntry {
                bond_atoms: 0, // below floor
                bonded_since_ts: 0,
                status: PoolStatus::Active,
                cosign_count_in_window: 0,
                eligible_blocks_in_window: 0,
                bls_pub_key: None,
                bls_pop: None,
                operator: None,
            };
            s.validator_pool.insert(addr, entry);
            targets.push(addr);
        }
        targets.sort();

        // Enqueue exits for all of them.
        s.exit_queue = targets
            .iter()
            .map(|&addr| ValidatorExitRequest {
                address: addr,
                request_height: 1,
                unlock_ts: 9999,
            })
            .collect();

        // Epoch close: floor guard must prevent any exit (active set already at floor).
        s.tick_epoch_close();
        assert!(
            !s.validator_pool
                .values()
                .any(|e| matches!(e.status, PoolStatus::Unbonding { .. })),
            "floor guard must prevent all exits when active set is at MIN_ACTIVE_SET_SIZE"
        );
        assert_eq!(
            s.exit_queue.len(),
            MIN_ACTIVE_SET_SIZE as usize,
            "all exit requests must stay in queue"
        );
    }

    #[test]
    fn test_exit_queue_bond_topup_discards_request() {
        use vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;
        use vinx_core::validator_pool::{PoolStatus, ValidatorPoolEntry};
        use vinx_core::ValidatorExitRequest;

        let (_, addr) = kp_addr();
        let mut s = WorldState::new();
        let entry = ValidatorPoolEntry {
            bond_atoms: MIN_VALIDATOR_BOND_ATOMS, // back above floor
            bonded_since_ts: 0,
            status: PoolStatus::Benched,
            cosign_count_in_window: 0,
            eligible_blocks_in_window: 0,
            bls_pub_key: None,
            bls_pop: None,
            operator: None,
        };
        s.validator_pool.insert(addr, entry);

        // Exit was queued when bond was low, but bond has since been topped up.
        s.exit_queue = vec![ValidatorExitRequest {
            address: addr,
            request_height: 1,
            unlock_ts: 9999,
        }];

        s.tick_epoch_close();

        assert!(
            !matches!(s.validator_pool[&addr].status, PoolStatus::Unbonding { .. }),
            "topped-up bond must cause exit request to be discarded silently"
        );
        assert!(s.exit_queue.is_empty(), "discarded request must be removed");
    }

    #[test]
    fn test_exit_queue_unlock_ts_is_set_from_request_time() {
        use vinx_core::amount::{MIN_VALIDATOR_BOND_ATOMS, UNBONDING_SECS};
        let (mut s, kp, addr) = bonded_active_validator();
        let request_ts = 5_000u64;
        s.set_block_context(request_ts);

        s.apply_transaction(&Transaction::new_unstake(
            &kp,
            Amount::from_atoms(MIN_VALIDATOR_BOND_ATOMS),
            Amount::ZERO,
            1,
        ))
        .unwrap();

        let expected_unlock = request_ts.saturating_add(UNBONDING_SECS);
        assert_eq!(s.exit_queue[0].unlock_ts, expected_unlock);

        // Add extra validators so floor check passes.
        use vinx_core::validator_pool::{PoolStatus, ValidatorPoolEntry};
        for _ in 0..20 {
            let (_, extra) = kp_addr();
            let entry = ValidatorPoolEntry {
                bond_atoms: MIN_VALIDATOR_BOND_ATOMS,
                bonded_since_ts: 0,
                status: PoolStatus::Benched,
                cosign_count_in_window: 0,
                eligible_blocks_in_window: 0,
                bls_pub_key: None,
                bls_pop: None,
                operator: None,
            };
            s.validator_pool.insert(extra, entry);
        }

        s.tick_epoch_close();

        if let vinx_core::validator_pool::PoolStatus::Unbonding { unlock_ts } =
            s.validator_pool[&addr].status
        {
            assert_eq!(
                unlock_ts, expected_unlock,
                "unlock_ts must equal the value computed at request time"
            );
        } else {
            panic!("expected Unbonding status");
        }
    }

    #[test]
    fn test_exit_queue_removed_validator_discarded() {
        use vinx_core::ValidatorExitRequest;

        // A validator that was already removed from the pool (e.g. slashed) while in the queue.
        let (_, addr) = kp_addr();
        let mut s = WorldState::new();
        // Validator NOT in the pool.
        s.exit_queue = vec![ValidatorExitRequest {
            address: addr,
            request_height: 1,
            unlock_ts: 9999,
        }];

        s.tick_epoch_close(); // must not panic

        assert!(
            s.exit_queue.is_empty(),
            "ghost exit must be silently discarded"
        );
    }
}
