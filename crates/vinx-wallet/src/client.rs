use crate::error::WalletError;
use serde::Deserialize;
use vinx_core::Transaction;

// ─── Response types matching the node's RPC ──────────────────────────────────

#[derive(Deserialize)]
pub struct AccountInfo {
    pub address: String,
    pub balance: String,
    #[allow(dead_code)]
    pub balance_atoms: String,
    pub nonce: u64,
    pub staked: String,
    #[allow(dead_code)]
    pub staked_atoms: String,
    pub frozen: bool,
}

#[derive(Deserialize)]
pub struct TxSubmitResponse {
    pub accepted: bool,
    pub tx_hash: String,
}

#[derive(Deserialize)]
pub struct TxInfo {
    pub tx_type: String,
    pub from: String,
    pub to: String,
    #[allow(dead_code)]
    pub amount: String,
    pub fee: String,
    #[allow(dead_code)]
    pub nonce: u64,
    pub hash: String,
}

#[derive(Deserialize)]
pub struct BlockInfo {
    pub height: u64,
    pub hash: String,
    pub prev_hash: String,
    pub timestamp: u64,
    pub validator: String,
    pub tx_count: u32,
    pub transactions: Vec<TxInfo>,
}

#[derive(Deserialize)]
pub struct HealthInfo {
    pub status: String,
    pub height: u64,
    pub mempool_pending: usize,
}

#[derive(Deserialize)]
pub struct TxWithBlockInfo {
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

#[derive(Deserialize)]
pub struct AccountTxsInfo {
    pub address: String,
    pub total: usize,
    #[allow(dead_code)]
    pub offset: usize,
    pub txs: Vec<TxWithBlockInfo>,
}

#[derive(Deserialize)]
pub struct ValidatorsInfo {
    pub count: usize,
    pub quorum: usize,
    pub validators: Vec<String>,
}

#[derive(Deserialize)]
pub struct ProtocolStatusInfo {
    pub current_version: String,
    pub pending_upgrade: Option<PendingUpgradeInfo>,
}

#[derive(Deserialize)]
pub struct PendingUpgradeInfo {
    pub version: String,
    pub activation_height: u64,
    pub announced_at: u64,
}

#[derive(Deserialize)]
pub struct NetworkStatsInfo {
    pub base_fee_atoms: String,
    pub staking_pool: String,
    pub melt_pool: String,
    pub circulating_supply: String,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

// ─── Client ──────────────────────────────────────────────────────────────────

pub struct RpcClient {
    base_url: String,
    client: reqwest::Client,
}

impl RpcClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client build failed"),
        }
    }

    pub async fn health(&self) -> Result<HealthInfo, WalletError> {
        self.get("/health").await
    }

    pub async fn get_account(&self, address: &str) -> Result<AccountInfo, WalletError> {
        self.get(&format!("/account/{}", address)).await
    }

    pub async fn get_block(&self, height: u64) -> Result<BlockInfo, WalletError> {
        self.get(&format!("/block/{}", height)).await
    }

    pub async fn get_tx(&self, hash: &str) -> Result<TxWithBlockInfo, WalletError> {
        self.get(&format!("/tx/{}", hash)).await
    }

    pub async fn get_validators(&self) -> Result<ValidatorsInfo, WalletError> {
        self.get("/validators").await
    }

    pub async fn get_protocol_status(&self) -> Result<ProtocolStatusInfo, WalletError> {
        self.get("/protocol/version").await
    }

    pub async fn get_account_txs(
        &self,
        address: &str,
        limit: usize,
        offset: usize,
    ) -> Result<AccountTxsInfo, WalletError> {
        self.get(&format!("/account/{}/txs?limit={}&offset={}", address, limit, offset)).await
    }

    pub async fn get_network_stats(&self) -> Result<NetworkStatsInfo, WalletError> {
        self.get("/network/stats").await
    }

    pub async fn submit_tx(&self, tx: &Transaction) -> Result<TxSubmitResponse, WalletError> {
        let url = format!("{}/tx/submit", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(tx)
            .send()
            .await
            .map_err(|e| WalletError::NodeUnreachable(self.base_url.clone(), e.to_string()))?;

        if !resp.status().is_success() {
            let body: ErrorBody = resp.json().await.unwrap_or(ErrorBody {
                error: "unknown error".to_string(),
            });
            return Err(WalletError::NodeError(body.error));
        }

        resp.json::<TxSubmitResponse>()
            .await
            .map_err(|e| WalletError::NodeError(e.to_string()))
    }

    async fn get<T: for<'de> serde::Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<T, WalletError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| WalletError::NodeUnreachable(self.base_url.clone(), e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body: ErrorBody = resp.json().await.unwrap_or(ErrorBody {
                error: format!("HTTP {}", status),
            });
            return Err(WalletError::NodeError(body.error));
        }

        resp.json::<T>()
            .await
            .map_err(|e| WalletError::NodeError(e.to_string()))
    }
}
