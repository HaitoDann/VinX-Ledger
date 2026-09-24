//! Deserialization targets for the node's RPC JSON, mirroring `vinx-node`'s
//! `rpc/types.rs`. Re-serialized to the frontend as-is.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    pub height: u64,
    pub mempool_pending: usize,
    pub chain_id: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Account {
    pub address: String,
    pub balance: String,
    pub balance_atoms: String,
    pub nonce: u64,
    pub staked: String,
    pub staked_atoms: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkStats {
    pub base_fee_atoms: String,
    pub remaining_supply: String,
    pub destroyed_atoms: String,
    pub circulating_supply: String,
    pub admin_address: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Validator {
    pub address: String,
    pub is_next_leader: bool,
    /// Voting power (stake-weighted, capped at 10 %, ADR 0082).
    #[serde(default)]
    pub power: u64,
    #[serde(default)]
    pub missed_proposals: u64,
    pub online: bool,
    pub suspended: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Validators {
    pub count: usize,
    pub quorum: u64,
    pub next_leader: String,
    pub validators: Vec<Validator>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PendingUpgrade {
    pub version: String,
    /// Unix timestamp (seconds) at which the upgrade activates (ADR 0006).
    pub activation_ts: u64,
    pub announced_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Protocol {
    pub current_version: String,
    pub pending_upgrade: Option<PendingUpgrade>,
}

/// One row of an account's transaction history. `TxWithBlockResponse` flattens the
/// tx fields alongside the block fields, so this is a flat struct.
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryTx {
    pub block_height: u64,
    pub block_hash: String,
    pub tx_type: String,
    pub from: String,
    pub to: String,
    pub amount: String,
    pub fee: String,
    pub nonce: u64,
    pub hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct History {
    pub address: String,
    pub total: usize,
    pub offset: usize,
    pub txs: Vec<HistoryTx>,
}

/// Node reply to `POST /tx/submit`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SubmitResult {
    pub accepted: bool,
    pub tx_hash: String,
}
