use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use vinx_core::{Account, Block, ProtocolVersion, ScheduledUpgrade, Transaction, ValidatorSet};
use vinx_crypto::{Address, Hash32};

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
}

impl TxResponse {
    pub fn from_tx(tx: &Transaction) -> Self {
        Self {
            tx_type: format!("{:?}", tx.tx_type),
            from: tx.from.to_string(),
            to: tx.to.to_string(),
            amount: tx.amount.to_string(),
            amount_atoms: tx.amount.atoms().to_string(),
            fee: tx.fee.to_string(),
            fee_atoms: tx.fee.atoms().to_string(),
            nonce: tx.nonce,
            hash: hash_to_hex(&tx.hash()),
        }
    }
}

#[derive(Serialize)]
pub struct BlockResponse {
    pub height: u64,
    pub hash: String,
    pub prev_hash: String,
    pub timestamp: u64,
    pub validator: String,
    pub tx_count: u32,
    pub state_root: String,
    /// Dynamic fee floor (in atoms) at block production time.
    pub base_fee: u64,
    /// Number of valid co-signatures from registered validators.
    pub signatures_count: usize,
    /// True when signatures_count >= quorum.
    pub finalized: bool,
    pub transactions: Vec<TxResponse>,
}

impl BlockResponse {
    pub fn from_block(block: &Block, validator_set: &ValidatorSet) -> Self {
        Self {
            height: block.header.height,
            hash: hash_to_hex(&block.hash()),
            prev_hash: hash_to_hex(&block.header.prev_hash),
            timestamp: block.header.timestamp,
            validator: block.header.validator.to_string(),
            tx_count: block.header.tx_count,
            state_root: hash_to_hex(&block.header.state_root),
            base_fee: block.header.base_fee,
            signatures_count: block.valid_signer_count(validator_set),
            finalized: block.is_finalized(validator_set),
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
    /// True if this validator is the round-robin leader for the next block.
    pub is_next_leader: bool,
    /// Last block height produced by this validator (None = never seen on this node).
    pub last_seen_height: Option<u64>,
    /// Considered online if it produced a block within the last 10 slots.
    pub online: bool,
    /// True when the validator has been offline for too many consecutive blocks and
    /// has been temporarily suspended from the round-robin by liveness eviction.
    pub suspended: bool,
}

#[derive(Serialize)]
pub struct ValidatorSetResponse {
    pub count: usize,
    pub quorum: usize,
    pub next_leader: String,
    pub validators: Vec<ValidatorInfo>,
}

impl ValidatorSetResponse {
    pub fn from_validator_set(
        vs: &ValidatorSet,
        next_height: u64,
        liveness: &HashMap<Address, u64>,
        slot_window: u64,
        suspended: &HashSet<Address>,
    ) -> Self {
        let next_leader_addr = vs.leader_at(next_height);
        let next_leader = next_leader_addr.to_string();
        let validators = vs
            .validators()
            .iter()
            .map(|a| {
                let last_seen = liveness.get(a).copied();
                let online =
                    last_seen.is_some_and(|h| next_height.saturating_sub(h) <= slot_window);
                ValidatorInfo {
                    is_next_leader: a == next_leader_addr,
                    suspended: suspended.contains(a),
                    address: a.to_string(),
                    last_seen_height: last_seen,
                    online,
                }
            })
            .collect();
        Self {
            count: vs.len(),
            quorum: vs.quorum(),
            next_leader,
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
    pub activation_height: u64,
    pub announced_at: u64,
}

impl ProtocolStatusResponse {
    pub fn new(version: &ProtocolVersion, upgrade: Option<&ScheduledUpgrade>) -> Self {
        Self {
            current_version: version.to_string(),
            pending_upgrade: upgrade.map(|u| PendingUpgradeResponse {
                version: u.version.to_string(),
                activation_height: u.activation_height,
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

/// Economic network stats — exposed via GET /network/stats
#[derive(Serialize)]
pub struct NetworkStatsResponse {
    pub base_fee_atoms: String,
    /// La Fonderie reserve (melt/forge). `circulating_supply + foundry == MAX_SUPPLY`.
    pub foundry: String,
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

#[derive(Serialize)]
pub struct MerkleProofResponse {
    pub address: String,
    pub leaf_hash: String,
    pub state_root: String,
    pub proof: Vec<MerkleProofStepResponse>,
    pub valid: bool,
}

#[derive(Serialize)]
pub struct MerkleProofStepResponse {
    pub sibling: String,
    pub sibling_is_right: bool,
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
