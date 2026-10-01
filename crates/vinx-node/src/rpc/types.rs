use serde::{Deserialize, Serialize};
use vinx_core::{
    reliability::ReliabilityMap, Account, Block, CommitCert, ProtocolVersion, ScheduledUpgrade,
    Transaction, ValidatorSet,
};
use vinx_crypto::Hash32;

fn hash_to_hex(h: &Hash32) -> String {
    hex::encode(h)
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub height: u64,
    /// Highest final (quorum-signed) height — irreversible (ADR 0002).
    pub finalized_height: u64,
    pub mempool_pending: usize,
    /// Chain ID this node validates — clients must sign transactions with it.
    pub chain_id: u32,
    /// Protocol block time (genesis parameter, ADR 0082 C6).
    pub block_time_secs: u64,
    /// Timestamp of the tip block (unix seconds).
    pub tip_timestamp: u64,
}

#[derive(Serialize)]
pub struct HeightResponse {
    pub height: u64,
}

#[derive(Serialize)]
pub struct AccountResponse {
    pub address: String,
    pub balance: String,
    /// u128 serialized as string to avoid JSON precision loss.
    pub balance_atoms: String,
    pub nonce: u64,
    pub staked: String,
    pub staked_atoms: String,
    /// The account refuses transfers without a memo (ADR 0085).
    pub memo_required: bool,
}

impl AccountResponse {
    pub fn from_account(account: &Account) -> Self {
        Self {
            address: account.address.to_string(),
            balance: account.balance.to_string(),
            balance_atoms: account.balance.atoms().to_string(),
            nonce: account.nonce,
            staked: account.staked.to_string(),
            staked_atoms: account.staked.atoms().to_string(),
            memo_required: false,
        }
    }
}

#[derive(Serialize)]
pub struct TxSubmitResponse {
    pub accepted: bool,
    pub tx_hash: String,
}

#[derive(Serialize)]
pub struct TxResponse {
    pub tx_type: String,
    pub from: String,
    pub to: String,
    pub amount: String,
    pub amount_atoms: String,
    pub fee: String,
    pub fee_atoms: String,
    pub nonce: u64,
    pub hash: String,
    /// Transfer memo (ADR 0085): UTF-8 text when valid, else null (see `memo_hex`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo_hex: Option<String>,
}

impl TxResponse {
    pub fn from_tx(tx: &Transaction) -> Self {
        Self {
            tx_type: format!("{:?}", tx.tx_type),
            from: tx.sender().to_string(),
            to: tx.to.to_string(),
            amount: tx.amount.to_string(),
            amount_atoms: tx.amount.atoms().to_string(),
            fee: tx.fee.to_string(),
            fee_atoms: tx.fee.atoms().to_string(),
            nonce: tx.nonce,
            hash: hash_to_hex(&tx.hash()),
            memo: tx
                .memo()
                .and_then(|m| std::str::from_utf8(m).ok())
                .map(str::to_string),
            memo_hex: tx.memo().map(hex::encode),
        }
    }
}

#[derive(Serialize)]
pub struct BlockResponse {
    pub height: u64,
    /// Consensus round in which the block was built (ADR 0082).
    pub round: u32,
    pub hash: String,
    pub prev_hash: String,
    pub timestamp: u64,
    pub validator: String,
    pub tx_count: u32,
    pub state_root: String,
    /// Base fee (in atoms) of this block.
    pub base_fee: u64,
    /// Validators whose precommits are in this block's commit certificate.
    pub signatures_count: usize,
    /// Every stored block is committed by a certificate (ADR 0082): always true, kept for
    /// API compatibility.
    pub finalized: bool,
    pub transactions: Vec<TxResponse>,
}

impl BlockResponse {
    pub fn from_block(block: &Block, commit: Option<&CommitCert>) -> Self {
        Self {
            height: block.header.height,
            round: block.header.round,
            hash: hash_to_hex(&block.hash()),
            prev_hash: hash_to_hex(&block.header.prev_hash),
            timestamp: block.header.timestamp,
            validator: block.header.validator.to_string(),
            tx_count: block.header.tx_count,
            state_root: hash_to_hex(&block.header.state_root),
            base_fee: block.header.base_fee,
            signatures_count: commit.map(|c| c.signers().len()).unwrap_or(0),
            finalized: true,
            transactions: block.transactions.iter().map(TxResponse::from_tx).collect(),
        }
    }
}

#[derive(Serialize)]
pub struct MempoolResponse {
    pub pending: usize,
}

#[derive(Serialize)]
pub struct TxWithBlockResponse {
    pub block_height: u64,
    pub block_hash: String,
    #[serde(flatten)]
    pub tx: TxResponse,
}

impl TxWithBlockResponse {
    pub fn new(height: u64, block_hash: &Hash32, tx: &Transaction) -> Self {
        Self {
            block_height: height,
            block_hash: hash_to_hex(block_hash),
            tx: TxResponse::from_tx(tx),
        }
    }
}

#[derive(Serialize)]
pub struct ValidatorInfo {
    pub address: String,
    /// True if this validator proposes round 0 of the next height.
    pub is_next_leader: bool,
    /// Voting power (bond in VINX, capped — ADR 0081 C4).
    pub power: u64,
    /// Consecutive missed proposals (ADR 0027).
    pub missed_proposals: u32,
    /// Not jailed and no missed proposal in a row.
    pub online: bool,
    /// Jailed (skipped by the proposer rotation until `Unjail`).
    pub suspended: bool,
}

#[derive(Serialize)]
pub struct ValidatorSetResponse {
    pub count: usize,
    /// Voting power needed to commit: strictly more than 2/3 of `total_power`.
    pub quorum: u64,
    pub total_power: u64,
    pub next_leader: String,
    pub validators: Vec<ValidatorInfo>,
}

impl ValidatorSetResponse {
    pub fn from_validator_set(vs: &ValidatorSet, rel: &ReliabilityMap) -> Self {
        let next_leader_addr = vinx_core::reliability::proposer_for_round(vs, rel, 0);
        let validators = vs
            .validators()
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let r = rel.get(a).cloned().unwrap_or_default();
                ValidatorInfo {
                    is_next_leader: *a == next_leader_addr,
                    address: a.to_string(),
                    power: vs.power_at(i),
                    missed_proposals: r.missed_proposals,
                    online: !r.is_jailed() && r.missed_proposals == 0,
                    suspended: r.is_jailed(),
                }
            })
            .collect();
        Self {
            count: vs.len(),
            quorum: vs.quorum_power(),
            total_power: vs.total_power(),
            next_leader: next_leader_addr.to_string(),
            validators,
        }
    }
}

// ─── Validator join request ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ValidatorJoinRequestBody {
    pub address: String,
    pub p2p_multiaddr: Option<String>,
    pub message: Option<String>,
}

#[derive(Serialize)]
pub struct ValidatorJoinResponse {
    pub accepted: bool,
    pub address: String,
    pub note: &'static str,
}

#[derive(Serialize)]
pub struct ValidatorJoinListResponse {
    pub count: usize,
    pub requests: Vec<ValidatorJoinRequestItem>,
}

#[derive(Serialize)]
pub struct ValidatorJoinRequestItem {
    pub address: String,
    pub p2p_multiaddr: Option<String>,
    pub message: Option<String>,
    pub submitted_at: u64,
}

#[derive(Serialize)]
pub struct ProtocolStatusResponse {
    pub current_version: String,
    pub pending_upgrade: Option<PendingUpgradeResponse>,
}

#[derive(Serialize)]
pub struct PendingUpgradeResponse {
    pub version: String,
    /// Unix timestamp (seconds) at which the upgrade activates (ADR 0006).
    pub activation_ts: u64,
    pub announced_at: u64,
}

impl ProtocolStatusResponse {
    pub fn new(version: &ProtocolVersion, upgrade: Option<&ScheduledUpgrade>) -> Self {
        Self {
            current_version: version.to_string(),
            pending_upgrade: upgrade.map(|u| PendingUpgradeResponse {
                version: u.version.to_string(),
                activation_ts: u.activation_ts,
                announced_at: u.announced_at,
            }),
        }
    }
}

#[derive(Serialize)]
pub struct AccountTxsResponse {
    pub address: String,
    pub total: usize,
    pub offset: usize,
    pub txs: Vec<TxWithBlockResponse>,
}

#[derive(Serialize)]
pub struct ChainSyncResponse {
    pub from: u64,
    pub count: usize,
    pub blocks: Vec<BlockResponse>,
}

#[derive(Serialize, Deserialize)]
pub struct SnapshotResponse {
    /// Chain tip height at snapshot time.
    pub height: u64,
    /// Hex-encoded tip block hash.
    pub tip_hash: String,
    /// Full serialized world state (serde_json).
    pub state: serde_json::Value,
}

/// Public chain-snapshot response — used for bootstrap sync (ADR snapshot-sync).
/// Returned by GET /chain/snapshot; no auth required.
#[derive(Serialize, Deserialize)]
pub struct ChainSnapshotResponse {
    /// Chain tip height at snapshot time.
    pub height: u64,
    /// Hex-encoded state root at this height.
    pub state_root: String,
    /// The block at `height` (needed to call `Chain::new_from_snapshot`).
    pub block: vinx_core::Block,
    /// Its commit certificate (ADR 0082), verified against the snapshot's voting set.
    pub commit: vinx_core::CommitCert,
    /// The blocks just before `block`, oldest first (protocol-clock window, ADR 0005).
    #[serde(default)]
    pub ancestors: Vec<vinx_core::Block>,
    /// WorldState serialized as borsh, compressed with zstd, hex-encoded.
    pub state_hex: String,
}

/// Economic network stats — exposed via GET /network/stats
#[derive(Serialize)]
pub struct NetworkStatsResponse {
    pub base_fee_atoms: String,
    /// Supply not yet emitted: `MAX_SUPPLY − emitted_atoms` (ADR 0040).
    pub remaining_supply: String,
    /// Atoms permanently destroyed by reaping dust (ADR 0040, normally tiny).
    pub destroyed_atoms: String,
    pub circulating_supply: String,
    /// On-chain admin address (bech32), if one is configured. Public info — it
    /// signs governance transactions visible on-chain. Used by the admin console
    /// to confirm a loaded key is the current admin before enabling actions.
    pub admin_address: Option<String>,
}

#[derive(Serialize)]
pub struct CompactResponse {
    pub compacted: bool,
    pub kept_last: u64,
    pub tip_height: u64,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Proof of an account against the `state_root` of block `height` (ADR 0083).
///
/// Light-client check: `state_root` is in the header of block `height`, committed by the
/// quorum certificate embedded in block `height + 1`. Recompute
/// `H("VINX_STATE_ROOT"… ‖ accounts_root ‖ consensus_root)` and fold the sparse Merkle
/// proof from the leaf up to `accounts_root`.
#[derive(Serialize)]
pub struct MerkleProofResponse {
    pub address: String,
    pub height: u64,
    pub state_root: String,
    pub accounts_root: String,
    pub consensus_root: String,
    /// Tree key of the account (`account_key(address)`).
    pub key: String,
    /// Leaf value (`hash_account`), or null when the account does not exist.
    pub leaf_value: Option<String>,
    /// Leaf reached by the key's path: `[key, value]`, or null for an empty subtree.
    pub proof_leaf: Option<[String; 2]>,
    /// Sibling hashes, root first.
    pub siblings: Vec<String>,
    pub valid: bool,
}

/// Header fields in hex/JSON form, enough to recompute the block hash client-side
/// (`BlockHeader::hash`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HeaderJson {
    pub height: u64,
    pub round: u32,
    pub prev_hash: String,
    pub timestamp: u64,
    pub validator: String,
    pub tx_count: u32,
    pub state_root: String,
    pub base_fee: u64,
    pub receipts_root: String,
    pub last_commit_hash: String,
}

impl From<&vinx_core::BlockHeader> for HeaderJson {
    fn from(h: &vinx_core::BlockHeader) -> Self {
        Self {
            height: h.height,
            round: h.round,
            prev_hash: hex::encode(h.prev_hash),
            timestamp: h.timestamp,
            validator: h.validator.to_string(),
            tx_count: h.tx_count,
            state_root: hex::encode(h.state_root),
            base_fee: h.base_fee,
            receipts_root: hex::encode(h.receipts_root),
            last_commit_hash: hex::encode(h.last_commit_hash),
        }
    }
}

/// Payment receipt (ADR 0083, L6): proof that a transaction is in a committed block,
/// meant to be **kept by the wallet** — it stays valid after nodes prune the block.
///
/// Check: `verify_tx_proof(tx_hash, index, header.tx_count, siblings,
/// header.receipts_root)`, then `hash(header) == block_hash`, then that `commit`
/// (more than 2/3 of the voting power) signs `block_hash` at `header.height`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PaymentReceiptResponse {
    pub tx_hash: String,
    pub index: u32,
    /// Sibling hashes, bottom-up.
    pub siblings: Vec<String>,
    pub header: HeaderJson,
    pub block_hash: String,
    /// Commit certificate of the block (quorum BLS aggregate + signer bitmap).
    pub commit: Option<vinx_core::CommitCert>,
    pub tx: vinx_core::Transaction,
}

// ─── Batch transaction submission ────────────────────────────────────────────

#[derive(Serialize)]
pub struct BatchTxResult {
    pub tx_hash: String,
    pub accepted: bool,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct TxBatchResponse {
    pub total: usize,
    pub accepted: usize,
    pub results: Vec<BatchTxResult>,
}

// ─── Snapshot import ─────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SnapshotImportResponse {
    pub imported: bool,
    pub height: u64,
    pub validator_count: usize,
}

// ─── Receipts ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct TxReceiptResponse {
    pub tx_hash: String,
    pub block_height: u64,
    pub success: bool,
    pub error: Option<String>,
}

// ─── Fee estimation ───────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct FeeEstimateResponse {
    /// Current dynamic base fee in atoms (1× at low load, up to 3× at full mempool).
    pub base_fee_atoms: String,
    /// Recommended minimum fee for a standard 100 VinX transfer at the current base fee.
    pub recommended_fee_atoms: String,
    /// Mempool occupancy: pending / max.
    pub mempool_pending: usize,
    pub mempool_max: usize,
}

// ─── Faucet ──────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct FaucetRequest {
    pub address: String,
}

#[derive(Serialize)]
pub struct FaucetResponse {
    pub accepted: bool,
    pub tx_hash: String,
    /// Atoms sent as a string to avoid JSON precision loss.
    pub amount_atoms: String,
    pub to: String,
}
