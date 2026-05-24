use axum::{
    extract::{Path, State},
    http::StatusCode,
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
        Some(block) => Ok(Json(BlockResponse::from_block(block))),
        None => Err(ApiError::NotFound(format!("Block {} not found", height))),
    }
}

pub async fn get_mempool(State(node): State<Arc<Node>>) -> ApiResult<MempoolResponse> {
    let pending = node.mempool.read().await.size();
    Ok(Json(MempoolResponse { pending }))
}
