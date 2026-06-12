use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use std::sync::Arc;
use vinx_core::Transaction;
use vinx_crypto::Address;

use crate::{rpc::types::*, Node};

pub type ApiResult<T> = Result<Json<T>, ApiError>;

// ─── Error type ─────────────────────────────────────────────────────────────

pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(ErrorResponse { error: msg })).into_response()
    }
}

// ─── Handlers ───────────────────────────────────────────────────────────────

pub async fn health(State(node): State<Arc<Node>>) -> ApiResult<HealthResponse> {
    let height = node.chain.read().await.tip_height();
    let pending = node.mempool.read().await.size();
    Ok(Json(HealthResponse {
        status: "ok",
        height,
        mempool_pending: pending,
    }))
}

pub async fn get_height(State(node): State<Arc<Node>>) -> ApiResult<HeightResponse> {
    let height = node.chain.read().await.tip_height();
    Ok(Json(HeightResponse { height }))
}

pub async fn get_account(
    State(node): State<Arc<Node>>,
    Path(raw_address): Path<String>,
) -> ApiResult<AccountResponse> {
    let address =
        Address::from_bech32(&raw_address).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let state = node.state.read().await;
    match state.get_account(&address) {
        Some(account) => Ok(Json(AccountResponse::from_account(account))),
        None => Err(ApiError::NotFound(format!(
            "Account {} not found",
            raw_address
        ))),
    }
}

pub async fn submit_tx(
    State(node): State<Arc<Node>>,
    Json(tx): Json<Transaction>,
) -> ApiResult<TxSubmitResponse> {
    // Pre-validate signature before accepting into mempool
    if let Some(pk) = &tx.pub_key {
        let derived = Address::from_public_key(pk);
        if derived != tx.from {
            return Err(ApiError::BadRequest("Public key does not match sender address".to_string()));
        }
        if let Some(sig) = &tx.signature {
            pk.verify(&tx.signing_bytes(), sig)
                .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        } else {
            return Err(ApiError::BadRequest("Missing signature".to_string()));
        }
    } else {
        return Err(ApiError::BadRequest("Missing public key".to_string()));
    }

    let tx_hash = hex::encode(tx.hash());
    node.mempool
        .write()
        .await
        .add(tx)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    Ok(Json(TxSubmitResponse {
        accepted: true,
        tx_hash,
    }))
}

pub async fn get_block(
    State(node): State<Arc<Node>>,
    Path(height): Path<u64>,
) -> ApiResult<BlockResponse> {
    let chain = node.chain.read().await;
    match chain.get_block(height) {
        Some(block) => Ok(Json(BlockResponse::from_block(block, &node.config.validator_set))),
        None => Err(ApiError::NotFound(format!("Block {} not found", height))),
    }
}

pub async fn get_mempool(State(node): State<Arc<Node>>) -> ApiResult<MempoolResponse> {
    let pending = node.mempool.read().await.size();
    Ok(Json(MempoolResponse { pending }))
}

pub async fn get_validators(State(node): State<Arc<Node>>) -> ApiResult<ValidatorSetResponse> {
    Ok(Json(ValidatorSetResponse::from_validator_set(
        &node.config.validator_set,
    )))
}

pub async fn get_protocol_status(State(node): State<Arc<Node>>) -> ApiResult<ProtocolStatusResponse> {
    let state = node.state.read().await;
    Ok(Json(ProtocolStatusResponse::new(
        &state.current_version,
        state.pending_upgrade.as_ref(),
    )))
}

pub async fn get_tx_by_hash(
    State(node): State<Arc<Node>>,
    Path(hash): Path<String>,
) -> ApiResult<TxWithBlockResponse> {
    let chain = node.chain.read().await;
    match chain.get_tx_by_hash(&hash) {
        Some((height, block, tx)) => Ok(Json(TxWithBlockResponse::new(height, &block.hash(), tx))),
        None => Err(ApiError::NotFound(format!("Transaction {} not found", hash))),
    }
}

#[derive(serde::Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    50
}

pub async fn get_account_txs(
    State(node): State<Arc<Node>>,
    Path(raw_address): Path<String>,
    Query(params): Query<PaginationParams>,
) -> ApiResult<AccountTxsResponse> {
    let address =
        Address::from_bech32(&raw_address).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let addr_str = address.to_string();
    let limit = params.limit.min(200);

    let chain = node.chain.read().await;
    let tx_hashes = chain.get_account_txs(&addr_str, limit, params.offset);
    let total = chain.account_tx_count(&addr_str);

    let mut txs = Vec::with_capacity(tx_hashes.len());
    for hash in &tx_hashes {
        if let Some((height, block, tx)) = chain.get_tx_by_hash(hash) {
            txs.push(TxWithBlockResponse::new(height, &block.hash(), tx));
        }
    }

    Ok(Json(AccountTxsResponse {
        address: addr_str,
        total,
        offset: params.offset,
        txs,
    }))
}

#[derive(serde::Deserialize)]
pub struct SyncParams {
    #[serde(default)]
    pub from: u64,
    #[serde(default = "default_sync_limit")]
    pub limit: usize,
}

fn default_sync_limit() -> usize {
    100
}

pub async fn get_chain_sync(
    State(node): State<Arc<Node>>,
    Query(params): Query<SyncParams>,
) -> ApiResult<ChainSyncResponse> {
    let limit = params.limit.min(500);
    let chain = node.chain.read().await;
    let tip = chain.tip_height();

    let start = params.from;
    if start > tip {
        return Ok(Json(ChainSyncResponse { from: start, count: 0, blocks: vec![] }));
    }

    let end = (start + limit as u64).min(tip + 1);
    let mut blocks = Vec::new();
    for h in start..end {
        if let Some(block) = chain.get_block(h) {
            blocks.push(BlockResponse::from_block(block, &node.config.validator_set));
        }
    }
    let count = blocks.len();
    Ok(Json(ChainSyncResponse { from: start, count, blocks }))
}

/// Returns node metrics in Prometheus text format.
pub async fn get_metrics(State(node): State<Arc<Node>>) -> impl IntoResponse {
    let height = node.chain.read().await.tip_height();
    let mempool_size = node.mempool.read().await.size();

    let body = format!(
        "# HELP vinx_chain_height Current chain tip height\n\
         # TYPE vinx_chain_height gauge\n\
         vinx_chain_height {height}\n\
         # HELP vinx_mempool_size Number of transactions pending in mempool\n\
         # TYPE vinx_mempool_size gauge\n\
         vinx_mempool_size {mempool_size}\n"
    );

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}
