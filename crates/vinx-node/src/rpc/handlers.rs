use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use futures::stream::{self, Stream};
use std::{convert::Infallible, sync::Arc, time::SystemTime};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;
use vinx_core::{amount::Amount, Transaction};
use vinx_crypto::Address;

use crate::{rpc::types::*, Node};

pub type ApiResult<T> = Result<Json<T>, ApiError>;

fn check_admin_auth(headers: &axum::http::HeaderMap, expected: Option<&str>) -> bool {
    let Some(token) = expected else { return true }; // auth disabled
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        == Some(token)
}

// ─── Error type ─────────────────────────────────────────────────────────────

pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
    Unauthorized(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
            ApiError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
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
    let address = node.parse_address(&raw_address).await?;
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
            return Err(ApiError::BadRequest(
                "Public key does not match sender address".to_string(),
            ));
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
        Some(block) => Ok(Json(BlockResponse::from_block(
            block,
            &*node.validator_set.read().await,
        ))),
        None => Err(ApiError::NotFound(format!("Block {} not found", height))),
    }
}

pub async fn get_mempool(State(node): State<Arc<Node>>) -> ApiResult<MempoolResponse> {
    let pending = node.mempool.read().await.size();
    Ok(Json(MempoolResponse { pending }))
}

pub async fn get_validators(State(node): State<Arc<Node>>) -> ApiResult<ValidatorSetResponse> {
    let vs = node.validator_set.read().await;
    let next_height = node.chain.read().await.tip_height() + 1;
    let liveness = node.validator_liveness.read().await;
    // "Online" = seen within 10 slots
    Ok(Json(ValidatorSetResponse::from_validator_set(
        &vs,
        next_height,
        &liveness,
        10,
    )))
}

pub async fn post_validator_request(
    State(node): State<Arc<Node>>,
    Json(body): Json<ValidatorJoinRequestBody>,
) -> ApiResult<ValidatorJoinResponse> {
    use crate::node::ValidatorJoinRequest;

    // Basic address format check
    node.parse_address(&body.address).await?;

    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let req = ValidatorJoinRequest {
        address: body.address.clone(),
        p2p_multiaddr: body.p2p_multiaddr,
        message: body.message,
        submitted_at: now,
    };

    node.validator_requests.lock().await.push(req);
    tracing::info!(address = %body.address, "Validator join request received");

    Ok(Json(ValidatorJoinResponse {
        accepted: true,
        address: body.address,
        note: "Request queued. An admin must approve it via AddValidator transaction.",
    }))
}

pub async fn get_validator_requests(
    State(node): State<Arc<Node>>,
    headers: axum::http::HeaderMap,
) -> ApiResult<ValidatorJoinListResponse> {
    if !check_admin_auth(&headers, node.config.admin_token.as_deref()) {
        return Err(ApiError::Unauthorized("Admin token required".to_string()));
    }
    let requests = node.validator_requests.lock().await;
    Ok(Json(ValidatorJoinListResponse {
        count: requests.len(),
        requests: requests
            .iter()
            .map(|r| ValidatorJoinRequestItem {
                address: r.address.clone(),
                p2p_multiaddr: r.p2p_multiaddr.clone(),
                message: r.message.clone(),
                submitted_at: r.submitted_at,
            })
            .collect(),
    }))
}

pub async fn get_protocol_status(
    State(node): State<Arc<Node>>,
) -> ApiResult<ProtocolStatusResponse> {
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
        None => Err(ApiError::NotFound(format!(
            "Transaction {} not found",
            hash
        ))),
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
    let address = node.parse_address(&raw_address).await?;
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
        return Ok(Json(ChainSyncResponse {
            from: start,
            count: 0,
            blocks: vec![],
        }));
    }

    let end = (start + limit as u64).min(tip + 1);
    let mut blocks = Vec::new();
    for h in start..end {
        if let Some(block) = chain.get_block(h) {
            blocks.push(BlockResponse::from_block(
                block,
                &*node.validator_set.read().await,
            ));
        }
    }
    let count = blocks.len();
    Ok(Json(ChainSyncResponse {
        from: start,
        count,
        blocks,
    }))
}

/// Server-Sent Events stream: sends a JSON event on every new block.
pub async fn sse_events(
    State(node): State<Arc<Node>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = node.block_events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        result.ok().map(|evt| {
            let data = serde_json::json!({
                "type": "new_block",
                "height": evt.height,
                "tx_count": evt.tx_count,
                "hash": evt.hash_hex,
            });
            Ok(Event::default().data(data.to_string()))
        })
    });
    let stream = stream::StreamExt::map(stream, |x: Result<Event, Infallible>| x);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// WebSocket endpoint — streams new-block events as JSON messages.
/// Connect with `ws://host:port/ws`. Each message is a JSON object:
/// `{"type":"new_block","height":N,"tx_count":N,"hash":"hex"}`
pub async fn ws_events(ws: WebSocketUpgrade, State(node): State<Arc<Node>>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws_client(socket, node))
}

async fn handle_ws_client(mut socket: WebSocket, node: Arc<Node>) {
    let mut rx = node.block_events.subscribe();
    loop {
        match rx.recv().await {
            Ok(evt) => {
                let msg = serde_json::json!({
                    "type": "new_block",
                    "height": evt.height,
                    "tx_count": evt.tx_count,
                    "hash": evt.hash_hex,
                });
                if socket
                    .send(Message::Text(msg.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
        }
    }
}

pub async fn get_snapshot(
    headers: axum::http::HeaderMap,
    State(node): State<Arc<Node>>,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, node.config.admin_token.as_deref()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "unauthorized".to_string(),
            }),
        )
            .into_response();
    }
    let state_guard = node.state.read().await;
    let chain_guard = node.chain.read().await;
    let height = chain_guard.tip_height();
    let tip_hash = hex::encode(chain_guard.tip_hash());

    match serde_json::to_value(&*state_guard) {
        Ok(state_json) => {
            let snap = SnapshotResponse {
                height,
                tip_hash,
                state: state_json,
            };
            (StatusCode::OK, Json(snap)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// Returns node metrics in Prometheus text format.
pub async fn get_metrics(State(node): State<Arc<Node>>) -> impl IntoResponse {
    let height = node.chain.read().await.tip_height();
    let mempool_size = node.mempool.read().await.size();
    let state = node.state.read().await;
    let base_fee = state.base_fee.atoms();
    let melt_pool = state.melt_pool.atoms();
    let staking_pool = state.staking_pool.atoms();
    let distribution_pool = state.distribution_pool.atoms();
    let circulating = state.circulating_supply.atoms();
    let validator_count = state.validator_set.len();

    let body = format!(
        "# HELP vinx_chain_height Current chain tip height\n\
         # TYPE vinx_chain_height gauge\n\
         vinx_chain_height {height}\n\
         # HELP vinx_mempool_size Number of transactions pending in mempool\n\
         # TYPE vinx_mempool_size gauge\n\
         vinx_mempool_size {mempool_size}\n\
         # HELP vinx_base_fee Current dynamic fee floor in atoms\n\
         # TYPE vinx_base_fee gauge\n\
         vinx_base_fee {base_fee}\n\
         # HELP vinx_melt_pool Total melted fees in atoms (redistribution reserve)\n\
         # TYPE vinx_melt_pool gauge\n\
         vinx_melt_pool {melt_pool}\n\
         # HELP vinx_staking_pool Current staking reward pool in atoms\n\
         # TYPE vinx_staking_pool gauge\n\
         vinx_staking_pool {staking_pool}\n\
         # HELP vinx_distribution_pool Distribution pool in atoms\n\
         # TYPE vinx_distribution_pool gauge\n\
         vinx_distribution_pool {distribution_pool}\n\
         # HELP vinx_circulating_supply Total circulating supply in atoms\n\
         # TYPE vinx_circulating_supply gauge\n\
         vinx_circulating_supply {circulating}\n\
         # HELP vinx_validator_count Number of active validators in the PoA set\n\
         # TYPE vinx_validator_count gauge\n\
         vinx_validator_count {validator_count}\n"
    );

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

/// Returns economic network statistics (base_fee, staking pool, melt pool).
pub async fn get_network_stats(State(node): State<Arc<Node>>) -> ApiResult<NetworkStatsResponse> {
    let state = node.state.read().await;
    Ok(Json(NetworkStatsResponse {
        base_fee_atoms: state.base_fee.atoms().to_string(),
        staking_pool: state.staking_pool.to_string(),
        melt_pool: state.melt_pool.to_string(),
        distribution_pool: state.distribution_pool.to_string(),
        circulating_supply: state.circulating_supply.to_string(),
    }))
}

/// Compacts transaction data from blocks older than `keep_last` blocks.
pub async fn post_compact(
    headers: axum::http::HeaderMap,
    State(node): State<Arc<Node>>,
    axum::extract::Query(params): axum::extract::Query<CompactParams>,
) -> ApiResult<CompactResponse> {
    if !check_admin_auth(&headers, node.config.admin_token.as_deref()) {
        return Err(ApiError::Unauthorized("unauthorized".to_string()));
    }
    let keep_last = params.keep_last.unwrap_or(1000);
    let mut chain = node.chain.write().await;
    let tip = chain.tip_height();
    chain.compact_old_txs(keep_last);
    Ok(Json(CompactResponse {
        compacted: true,
        kept_last: keep_last,
        tip_height: tip,
    }))
}

#[derive(serde::Deserialize)]
pub struct CompactParams {
    pub keep_last: Option<u64>,
}

// ─── Faucet handler ──────────────────────────────────────────────────────────

pub async fn faucet_request(
    State(node): State<Arc<Node>>,
    Json(req): Json<crate::rpc::types::FaucetRequest>,
) -> ApiResult<crate::rpc::types::FaucetResponse> {
    let faucet_kp =
        node.config.faucet_keypair.as_ref().ok_or_else(|| {
            ApiError::BadRequest("Faucet is not enabled on this node".to_string())
        })?;

    let to_addr = node.parse_address(&req.address).await
        .map_err(|e| match e { ApiError::BadRequest(m) => ApiError::BadRequest(format!("Invalid address: {}", m)), e => e })?;

    let faucet_addr = Address::from_public_key(&faucet_kp.public_key());

    // Lock serializes concurrent requests and enforces per-address cooldown.
    let mut cooldowns = node.faucet_cooldowns.lock().await;

    let cooldown = std::time::Duration::from_secs(node.config.faucet_cooldown_secs);
    if let Some(&last_at) = cooldowns.get(req.address.as_str()) {
        let elapsed = last_at.elapsed();
        if elapsed < cooldown {
            let remaining = (cooldown - elapsed).as_secs();
            return Err(ApiError::BadRequest(format!(
                "Rate limited: wait {} seconds before requesting again",
                remaining
            )));
        }
    }

    // Nonce = highest confirmed nonce or next pending nonce (whichever is larger).
    let confirmed_nonce = node
        .state
        .read()
        .await
        .get_account(&faucet_addr)
        .map(|a| a.nonce)
        .unwrap_or(0);
    let nonce = {
        let mempool = node.mempool.read().await;
        mempool
            .next_nonce_for(&faucet_addr)
            .unwrap_or(confirmed_nonce)
    };

    let amount = Amount::from_atoms(node.config.faucet_amount_atoms);
    let fee = amount.calculate_fee(node.state.read().await.base_fee);

    let tx = Transaction::new_transfer(faucet_kp, to_addr, amount, fee, nonce);
    let tx_hash = hex::encode(tx.hash());

    node.mempool
        .write()
        .await
        .add(tx)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    cooldowns.insert(req.address.clone(), std::time::Instant::now());

    Ok(Json(crate::rpc::types::FaucetResponse {
        accepted: true,
        tx_hash,
        amount_atoms: amount.atoms().to_string(),
        to: req.address,
    }))
}

// ─── Merkle proof handler ────────────────────────────────────────────────────

pub async fn get_account_proof(
    State(node): State<Arc<Node>>,
    Path(raw_address): Path<String>,
) -> ApiResult<MerkleProofResponse> {
    use vinx_crypto::{merkle_proof_for, merkle_root, sha256, verify_merkle_proof};

    let address = node.parse_address(&raw_address).await?;

    let state = node.state.read().await;

    // Build sorted leaf list (same order as compute_state_root)
    let entries = state.accounts_sorted();

    let index = entries
        .iter()
        .position(|a| a.address == address)
        .ok_or_else(|| ApiError::NotFound(format!("Account {} not found", raw_address)))?;

    let leaves: Vec<vinx_crypto::Hash32> = entries
        .iter()
        .map(|a| {
            let addr = a.address.as_str().as_bytes();
            let mut buf = Vec::with_capacity(addr.len() + 48);
            buf.extend_from_slice(addr);
            buf.extend_from_slice(&a.balance.atoms().to_be_bytes());
            buf.extend_from_slice(&a.nonce.to_be_bytes());
            buf.extend_from_slice(&a.staked.atoms().to_be_bytes());
            buf.push(a.frozen as u8);
            buf.extend_from_slice(&a.stake_since.to_be_bytes());
            buf.extend_from_slice(&a.frozen_since.to_be_bytes());
            sha256(&buf)
        })
        .collect();

    let state_root = merkle_root(&leaves);
    let leaf_hash = leaves[index];
    let proof = merkle_proof_for(&leaves, index)
        .ok_or_else(|| ApiError::Internal("proof generation failed".to_string()))?;

    let valid = verify_merkle_proof(&leaf_hash, &proof, &state_root);

    Ok(Json(MerkleProofResponse {
        address: raw_address,
        leaf_hash: hex::encode(leaf_hash),
        state_root: hex::encode(state_root),
        proof: proof
            .iter()
            .map(|s| MerkleProofStepResponse {
                sibling: hex::encode(s.sibling),
                sibling_is_right: s.sibling_is_right,
            })
            .collect(),
        valid,
    }))
}

pub async fn get_tx_receipt(
    Path(hash_hex): Path<String>,
    State(node): State<Arc<Node>>,
) -> ApiResult<TxReceiptResponse> {
    let mut receipts = node.receipts.write().await;
    let receipt = receipts
        .get(&hash_hex)
        .ok_or_else(|| ApiError::NotFound(format!("receipt not found for tx {hash_hex}")))?;
    Ok(Json(TxReceiptResponse {
        tx_hash: receipt.tx_hash.clone(),
        block_height: receipt.block_height,
        success: receipt.success,
        error: receipt.error.clone(),
    }))
}

pub async fn get_fee_estimate(State(node): State<Arc<Node>>) -> ApiResult<FeeEstimateResponse> {
    let state = node.state.read().await;
    let mempool = node.mempool.read().await;
    let base_fee = state.base_fee;
    let sample_amount = vinx_core::Amount::from_vinx(100);
    let recommended_fee = sample_amount.calculate_fee(base_fee);
    Ok(Json(FeeEstimateResponse {
        base_fee_atoms: base_fee.atoms().to_string(),
        recommended_fee_atoms: recommended_fee.atoms().to_string(),
        mempool_pending: mempool.size(),
        mempool_max: node.config.max_block_txs,
    }))
}
