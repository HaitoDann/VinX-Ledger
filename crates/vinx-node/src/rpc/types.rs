use serde::Serialize;
use vinx_core::{Account, Block, ProtocolVersion, ScheduledUpgrade, Transaction, ValidatorSet};
use vinx_crypto::Hash32;

fn hash_to_hex(h: &Hash32) -> String {
    hex::encode(h)
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub height: u64,
    pub mempool_pending: usize,
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
    pub frozen: bool,
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
            frozen: account.frozen,
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
pub struct ValidatorSetResponse {
    pub count: usize,
    pub quorum: usize,
    pub validators: Vec<String>,
}

impl ValidatorSetResponse {
    pub fn from_validator_set(vs: &ValidatorSet) -> Self {
        Self {
            count: vs.len(),
            quorum: vs.quorum(),
            validators: vs.validators().iter().map(|a| a.to_string()).collect(),
        }
    }
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

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}
