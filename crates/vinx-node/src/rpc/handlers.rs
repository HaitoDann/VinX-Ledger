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
use std::{convert::Infallible, sync::atomic::Ordering, sync::Arc, time::SystemTime};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;
use vinx_core::{amount::Amount, Transaction};
use vinx_crypto::Address;

use crate::{rpc::types::*, Node};
use vinx_state::WorldState;

pub type ApiResult<T> = Result<Json<T>, ApiError>;

/// Admin routes are **fail-closed**: with no `admin_token` configured they refuse
/// every request instead of accepting all of them (the RPC listens on 0.0.0.0 by
/// default — an open `POST /snapshot` would let anyone replace the node's state).
/// Tokens are compared via their SHA-256 digests so the comparison cost is
/// independent of how many leading bytes match (no timing side channel).
fn check_admin_auth(headers: &axum::http::HeaderMap, expected: Option<&str>) -> bool {
    let Some(token) = expected else { return false };
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    match provided {
        Some(p) => vinx_crypto::hash256(p.as_bytes()) == vinx_crypto::hash256(token.as_bytes()),
        None => false,
    }
}

/// `check_admin_auth` as a `Result`, with an error message that tells the operator
/// *why* access is denied (missing configuration vs. bad token).
fn require_admin(headers: &axum::http::HeaderMap, expected: Option<&str>) -> Result<(), ApiError> {
    if expected.is_none() {
        return Err(ApiError::Unauthorized(
            "admin routes are disabled: no admin_token configured (set admin_token in config.toml)"
                .to_string(),
        ));
    }
    if !check_admin_auth(headers, expected) {
        return Err(ApiError::Unauthorized("invalid admin token".to_string()));
    }
    Ok(())
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
    let (height, finalized_height, tip_timestamp) = {
        let chain = node.chain.read().await;
        (
            chain.tip_height(),
            chain.finalized_height(),
            chain.tip_timestamp(),
        )
    };
    let pending = node.mempool.read().await.size();
    let (chain_id, block_time_secs, protocol) = {
        let s = node.state.read().await;
        (s.chain_id, s.block_time_secs, s.current_version.clone())
    };
    let software = vinx_core::protocol::NODE_PROTOCOL_VERSION;
    Ok(Json(HealthResponse {
        status: "ok",
        height,
        finalized_height,
        mempool_pending: pending,
        chain_id,
        block_time_secs,
        tip_timestamp,
        peers: node
            .metrics
            .peer_count
            .load(std::sync::atomic::Ordering::Relaxed),
        reachable: match node
            .metrics
            .reachability
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            1 => "public",
            2 => "private",
            _ => "unknown",
        },
        relays: node
            .metrics
            .relays
            .load(std::sync::atomic::Ordering::Relaxed),
        upgrade_required: protocol.as_u32() > software.as_u32(),
        protocol: protocol.to_string(),
        software_protocol: software.to_string(),
    }))
}

/// `GET /chain/genesis` — the shared genesis spec of this chain (public: addresses, BLS
/// keys and their proofs of possession, parameters), so another machine can join it.
pub async fn get_genesis_spec(State(node): State<Arc<Node>>) -> impl IntoResponse {
    match &node.config.genesis_spec {
        Some(spec) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            spec.clone(),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "this node was not started from a shared genesis spec".into(),
            }),
        )
            .into_response(),
    }
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
        Some(account) => {
            let mut r = AccountResponse::from_account(account);
            r.memo_required = state.memo_required.contains(&address);
            Ok(Json(r))
        }
        None => Err(ApiError::NotFound(format!(
            "Account {} not found",
            raw_address
        ))),
    }
}

/// Rejects a transaction whose `payload` exceeds the consensus bound, before any
/// expensive work touches it (ADR 0035 / finding VINX-13).
///
/// `admission_check` enforces the same bound as a consensus rule; this is purely an
/// ordering guard so the RPC surface does not verify a signature and hash a payload it is
/// going to refuse anyway.
fn payload_bound_msg(tx: &Transaction) -> Option<String> {
    (tx.payload.len() > vinx_core::amount::MAX_TX_PAYLOAD_BYTES).then(|| {
        format!(
            "payload of {} bytes exceeds the {}-byte limit",
            tx.payload.len(),
            vinx_core::amount::MAX_TX_PAYLOAD_BYTES
        )
    })
}

pub async fn submit_tx(
    State(node): State<Arc<Node>>,
    Json(tx): Json<Transaction>,
) -> ApiResult<TxSubmitResponse> {
    // Cheapest structural check first: the payload bound gates how much data the
    // signature verification and the two SHA-256 passes below will chew through. It used
    // to be enforced only in `admission_check`, i.e. *after* all of that, so an
    // unauthenticated caller could force the full crypto cost on an oversized payload.
    // The P2P path already had this ordering right (admission before crypto).
    if let Some(msg) = payload_bound_msg(&tx) {
        return Err(ApiError::BadRequest(msg));
    }

    // Pre-validate signature before accepting into mempool.
    // VINX-03: must be the canonical predicate — an ad-hoc sender-only check let a
    // sponsored transaction with no sponsor signature drain the named sponsor's account.
    WorldState::verify_tx_signature_pure(&tx).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let tx_hash = hex::encode(tx.hash());
    match admit_to_mempool(&node, tx).await {
        Ok(()) => {
            node.metrics.tx_submitted_ok.fetch_add(1, Ordering::Relaxed);
            Ok(Json(TxSubmitResponse {
                accepted: true,
                tx_hash,
            }))
        }
        Err(e) => {
            node.metrics
                .tx_submitted_err
                .fetch_add(1, Ordering::Relaxed);
            Err(ApiError::BadRequest(e))
        }
    }
}

/// Stateful admission (anti-spam) + mempool insertion, shared by `/tx/submit`
/// and `/tx/batch`. The signature must already be verified by the caller.
///
/// On top of `WorldState::admission_check` (chain_id, fee floor, funded sender,
/// nonce window), this enforces the **cumulative** funding rule: the sender's
/// balance must cover every transaction already queued for it plus this one —
/// otherwise one funded fee could back a whole queue of unpayable high-priority
/// entries. State is read-locked before the mempool write lock (same order as
/// the persist path) so the check and the insert are atomic with respect to
/// competing submissions.
async fn admit_to_mempool(node: &Arc<Node>, tx: Transaction) -> Result<(), String> {
    let state = node.state.read().await;
    state.admission_check(&tx).map_err(|e| e.to_string())?;

    let mut mempool = node.mempool.write().await;
    let queued = mempool.queued_cost_atoms(&tx.sender());
    let needed = queued.saturating_add(tx.admission_cost_atoms());
    if state.account_balance(&tx.sender()).atoms() < needed {
        return Err(format!(
            "sender balance does not cover already-queued transactions plus this one \
             (queued cost {queued} atoms)"
        ));
    }
    drop(state);
    mempool.add(tx.clone()).map_err(|e| e.to_string())?;
    drop(mempool);
    relay(node, &tx);
    Ok(())
}

/// Gossips a transaction accepted over RPC to the other nodes, so it is included whoever
/// proposes the next block — not only when this node does (it may not be a validator).
fn relay(node: &Node, tx: &Transaction) {
    if let Some(p2p) = &node.p2p {
        p2p.broadcast_tx(tx);
    }
}

pub async fn get_block(
    State(node): State<Arc<Node>>,
    Path(height): Path<u64>,
) -> ApiResult<BlockResponse> {
    let chain = node.chain.read().await;
    match chain.row(height) {
        Some(row) => Ok(Json(BlockResponse::from_block(
            &row.block,
            row.commit.as_ref(),
        ))),
        None => Err(ApiError::NotFound(format!("Block {} not found", height))),
    }
}

pub async fn get_mempool(State(node): State<Arc<Node>>) -> ApiResult<MempoolResponse> {
    let pending = node.mempool.read().await.size();
    Ok(Json(MempoolResponse { pending }))
}

pub async fn get_validators(State(node): State<Arc<Node>>) -> ApiResult<ValidatorSetResponse> {
    let state = node.state.read().await;
    Ok(Json(ValidatorSetResponse::from_validator_set(
        &state.validator_set,
        &state.reliability,
    )))
}

/// `GET /validator/:address` — where an address stands on the way to validating:
/// `none` (no bond), `warmup` (with epochs left), `benched`, `active` or `unbonding`,
/// plus its bond, rewards-relevant counters and when the next epoch closes.
pub async fn get_validator_status(
    State(node): State<Arc<Node>>,
    Path(address): Path<String>,
) -> ApiResult<serde_json::Value> {
    use vinx_core::validator_pool::PoolStatus;
    let addr = node.parse_address(&address).await?;
    let state = node.state.read().await;
    let epoch = vinx_core::amount::EPOCH_DURATION_SECS;
    let last = if state.last_epoch_close_ts == 0 {
        state.emission_epoch_ts
    } else {
        state.last_epoch_close_ts
    };
    let mut v = serde_json::json!({
        "address": addr.to_string(),
        "status": "none",
        "min_bond_atoms": state.min_validator_bond_atoms.to_string(),
        "next_epoch_ts": last.saturating_add(epoch),
        "epoch_secs": epoch,
        "in_set": state.validator_set.contains(&addr),
        "jailed": state.reliability.get(&addr).is_some_and(|r| r.is_jailed()),
    });
    if let Some(e) = state.validator_pool.get(&addr) {
        let (status, left) = match e.status {
            PoolStatus::Warmup { epochs_remaining } => ("warmup", epochs_remaining),
            PoolStatus::Active => ("active", 0),
            PoolStatus::Benched => ("benched", 0),
            PoolStatus::Unbonding { .. } => ("unbonding", 0),
        };
        v["status"] = status.into();
        v["warmup_epochs_left"] = left.into();
        v["bond_atoms"] = e.bond_atoms.to_string().into();
        v["cosigned_in_window"] = e.cosign_count_in_window.into();
        v["eligible_in_window"] = e.eligible_blocks_in_window.into();
        v["has_bls_key"] = e.bls_pub_key.is_some().into();
    }
    Ok(Json(v))
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
    require_admin(&headers, node.config.admin_token.as_deref())?;
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
    let hash_bytes: [u8; 32] = hex::decode(&hash)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| ApiError::NotFound(format!("Transaction {} not found", hash)))?;
    let chain = node.chain.read().await;
    match chain.get_tx_by_hash(&hash_bytes) {
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
    let tx_hashes = chain.get_account_txs(&address, limit, params.offset);
    let total = chain.account_tx_count(&address);

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
        if let Some(row) = chain.row(h) {
            blocks.push(BlockResponse::from_block(&row.block, row.commit.as_ref()));
        }
    }
    let count = blocks.len();
    Ok(Json(ChainSyncResponse {
        from: start,
        count,
        blocks,
    }))
}

/// Committed blocks with their certificates — `GET /chain/commits?from=&limit=` — the
/// source for node-to-node HTTP sync (ADR 0082). The requester re-verifies everything.
pub async fn get_chain_commits(
    State(node): State<Arc<Node>>,
    Query(params): Query<SyncParams>,
) -> Json<serde_json::Value> {
    let limit = params.limit.min(500) as u64;
    let chain = node.chain.read().await;
    let end = params
        .from
        .saturating_add(limit)
        .min(chain.tip_height().saturating_add(1));
    let rows: Vec<crate::sync::CommittedRow> = (params.from..end)
        .map_while(|h| {
            let row = chain.row(h)?;
            Some(crate::sync::CommittedRow {
                block: row.block.clone(),
                commit: row.commit.clone()?,
            })
        })
        .collect();
    Json(serde_json::json!({ "rows": rows }))
}

/// Server-Sent Events stream: sends a JSON event on every new block.
/// Each event carries: height, tx_count, hash, base_fee_atoms, proposer, state_root.
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
                "base_fee_atoms": evt.base_fee_atoms.to_string(),
                "proposer": evt.proposer,
                "state_root": evt.state_root_hex,
            });
            Ok(Event::default().data(data.to_string()))
        })
    });
    let stream = stream::StreamExt::map(stream, |x: Result<Event, Infallible>| x);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// WebSocket endpoint — streams new-block events as JSON messages.
///
/// Each message is a JSON object:
/// `{"type":"new_block","height":N,"tx_count":N,"hash":"hex","base_fee_atoms":"N","proposer":"vinx1...","state_root":"hex"}`
///
/// Ping/pong keepalive is sent every 30 s to detect silently-closed connections.
pub async fn ws_events(ws: WebSocketUpgrade, State(node): State<Arc<Node>>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws_client(socket, node))
}

async fn handle_ws_client(mut socket: WebSocket, node: Arc<Node>) {
    let mut rx = node.block_events.subscribe();
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
    ping_interval.tick().await; // skip the immediate first tick

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(evt) => {
                        let msg = serde_json::json!({
                            "type": "new_block",
                            "height": evt.height,
                            "tx_count": evt.tx_count,
                            "hash": evt.hash_hex,
                            "base_fee_atoms": evt.base_fee_atoms.to_string(),
                            "proposer": evt.proposer,
                            "state_root": evt.state_root_hex,
                        });
                        if socket
                            .send(Message::Text(msg.to_string()))
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
            _ = ping_interval.tick() => {
                // Keepalive ping — break if the client is gone
                if socket.send(Message::Ping(vec![])).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {} // Pong and other frames — ignore
                }
            }
        }
    }
}

pub async fn get_snapshot(
    headers: axum::http::HeaderMap,
    State(node): State<Arc<Node>>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&headers, node.config.admin_token.as_deref()) {
        return e.into_response();
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

/// Public bootstrap snapshot — GET /chain/snapshot (ADR snapshot-sync).
/// No auth required: any new node can call this to bootstrap from a trusted peer.
/// Returns the full WorldState (borsh+zstd+hex) and the tip block so the caller
/// can initialize `Chain::new_from_snapshot` without replaying history from genesis.
pub async fn get_chain_snapshot(State(node): State<Arc<Node>>) -> impl IntoResponse {
    let mut state_guard = node.state.write().await;
    let chain_guard = node.chain.read().await;
    let height = chain_guard.tip_height();
    let Some((block, commit)) = chain_guard
        .row(height)
        .and_then(|r| Some((r.block.clone(), r.commit.clone()?)))
    else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "tip block or its commit certificate not found".into(),
            }),
        )
            .into_response();
    };
    let ancestors = chain_guard.tip_ancestors();
    let state_root = hex::encode(state_guard.compute_state_root());
    // Encode: borsh → zstd → hex
    let Ok(raw) = borsh::to_vec(&*state_guard) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "state serialization failed".into(),
            }),
        )
            .into_response();
    };
    let compressed = match zstd::encode_all(raw.as_slice(), 3) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("zstd: {e}"),
                }),
            )
                .into_response();
        }
    };
    let snap = ChainSnapshotResponse {
        height,
        state_root,
        block,
        commit,
        ancestors,
        state_hex: hex::encode(&compressed),
    };
    (StatusCode::OK, Json(snap)).into_response()
}

/// Returns node metrics in Prometheus text format.
/// Activity counters (blocks, tx, P2P, rate-limit) are read from lock-free
/// atomics on NodeMetrics — no lock acquired for those fields.
pub async fn get_metrics(State(node): State<Arc<Node>>) -> impl IntoResponse {
    // Lock-free reads — no contention on the hot path
    let m = &node.metrics;
    let blocks_produced = m.blocks_produced.load(Ordering::Relaxed);
    let tx_ok = m.tx_submitted_ok.load(Ordering::Relaxed);
    let tx_err = m.tx_submitted_err.load(Ordering::Relaxed);
    let tx_in_block = m.tx_in_block.load(Ordering::Relaxed);
    let p2p_blocks = m.p2p_blocks_recv.load(Ordering::Relaxed);
    let p2p_tx = m.p2p_tx_recv.load(Ordering::Relaxed);
    let rl_hit = m.ratelimit_hit.load(Ordering::Relaxed);
    let last_block_secs = m.last_block_secs.load(Ordering::Relaxed);

    // Economic state — lock required (changes only on block production)
    let height = node.chain.read().await.tip_height();
    let mempool_size = node.mempool.read().await.size();
    let state = node.state.read().await;
    let base_fee = state.base_fee.atoms();
    let remaining = state.remaining_supply();
    let destroyed = state.destroyed_atoms;
    let circulating = state.circulating_supply.atoms();
    let validator_count = state.validator_set.len();
    drop(state);

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
         # HELP vinx_remaining_supply Supply not yet emitted in atoms (MAX_SUPPLY − emitted)\n\
         # TYPE vinx_remaining_supply gauge\n\
         vinx_remaining_supply {remaining}\n\
         # HELP vinx_destroyed_atoms Atoms permanently destroyed by reaping dust\n\
         # TYPE vinx_destroyed_atoms counter\n\
         vinx_destroyed_atoms {destroyed}\n\
         # HELP vinx_circulating_supply Total circulating supply in atoms\n\
         # TYPE vinx_circulating_supply gauge\n\
         vinx_circulating_supply {circulating}\n\
         # HELP vinx_validator_count Number of active validators in the PoA set\n\
         # TYPE vinx_validator_count gauge\n\
         vinx_validator_count {validator_count}\n\
         # HELP vinx_blocks_produced_total Total blocks produced by this node\n\
         # TYPE vinx_blocks_produced_total counter\n\
         vinx_blocks_produced_total {blocks_produced}\n\
         # HELP vinx_tx_submitted_total Transactions submitted via RPC, by outcome\n\
         # TYPE vinx_tx_submitted_total counter\n\
         vinx_tx_submitted_total{{status=\"ok\"}} {tx_ok}\n\
         vinx_tx_submitted_total{{status=\"err\"}} {tx_err}\n\
         # HELP vinx_tx_in_block_total Total transactions included in produced blocks\n\
         # TYPE vinx_tx_in_block_total counter\n\
         vinx_tx_in_block_total {tx_in_block}\n\
         # HELP vinx_p2p_blocks_received_total Blocks received via P2P gossip\n\
         # TYPE vinx_p2p_blocks_received_total counter\n\
         vinx_p2p_blocks_received_total {p2p_blocks}\n\
         # HELP vinx_p2p_tx_received_total Transactions received via P2P gossip\n\
         # TYPE vinx_p2p_tx_received_total counter\n\
         vinx_p2p_tx_received_total {p2p_tx}\n\
         # HELP vinx_ratelimit_hit_total Requests rejected by the rate limiter\n\
         # TYPE vinx_ratelimit_hit_total counter\n\
         vinx_ratelimit_hit_total {rl_hit}\n\
         # HELP vinx_last_block_timestamp_seconds Unix timestamp of the last produced block\n\
         # TYPE vinx_last_block_timestamp_seconds gauge\n\
         vinx_last_block_timestamp_seconds {last_block_secs}\n"
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

/// Returns economic network statistics (base_fee, remaining supply, circulating supply).
pub async fn get_network_stats(State(node): State<Arc<Node>>) -> ApiResult<NetworkStatsResponse> {
    let state = node.state.read().await;
    Ok(Json(NetworkStatsResponse {
        base_fee_atoms: state.base_fee.atoms().to_string(),
        remaining_supply: vinx_core::Amount::from_atoms(state.remaining_supply()).to_string(),
        destroyed_atoms: state.destroyed_atoms.to_string(),
        circulating_supply: state.circulating_supply.to_string(),
        admin_address: state.admin_address.as_ref().map(|a| a.to_string()),
        min_validator_bond_atoms: state.min_validator_bond_atoms.to_string(),
    }))
}

/// Compacts transaction data from blocks older than `keep_last` blocks.
pub async fn post_compact(
    headers: axum::http::HeaderMap,
    State(node): State<Arc<Node>>,
    axum::extract::Query(params): axum::extract::Query<CompactParams>,
) -> ApiResult<CompactResponse> {
    require_admin(&headers, node.config.admin_token.as_deref())?;
    let keep_last = params.keep_last.unwrap_or(1000);
    let mut chain = node.chain.write().await;
    let tip = chain.tip_height();
    chain.prune(keep_last);
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

    let to_addr = node
        .parse_address(&req.address)
        .await
        .map_err(|e| match e {
            ApiError::BadRequest(m) => ApiError::BadRequest(format!("Invalid address: {}", m)),
            e => e,
        })?;

    let faucet_addr = Address::from_public_key(&faucet_kp.public_key());

    // Lock serializes concurrent requests and enforces per-address cooldown.
    let mut cooldowns = node.faucet_cooldowns.lock().await;

    let cooldown = std::time::Duration::from_secs(node.config.faucet_cooldown_secs);
    if let Some(&last_at) = cooldowns.get(&to_addr) {
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
        .add(tx.clone())
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    relay(&node, &tx);

    cooldowns.insert(to_addr, std::time::Instant::now());

    Ok(Json(crate::rpc::types::FaucetResponse {
        accepted: true,
        tx_hash,
        amount_atoms: amount.atoms().to_string(),
        to: req.address,
    }))
}

/// Imports a previously-exported state snapshot, replacing the live world state.
/// Admin-only.  The snapshot body must match the format produced by `GET /snapshot`.
pub async fn post_snapshot(
    headers: axum::http::HeaderMap,
    State(node): State<Arc<Node>>,
    Json(body): Json<SnapshotResponse>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&headers, node.config.admin_token.as_deref()) {
        return e.into_response();
    }
    match serde_json::from_value::<WorldState>(body.state) {
        Ok(new_state) => {
            let new_vs = new_state.validator_set.clone();
            let validator_count = new_vs.len();
            let height = body.height;
            *node.state.write().await = new_state;
            *node.validator_set.write().await = new_vs;
            // Full persist: the imported state may hold fewer accounts than the one
            // it replaces, so wipe stale rows and write the whole set to disk now.
            node.persist_full().await;
            tracing::info!(
                height,
                validator_count,
                "Snapshot imported via POST /snapshot"
            );
            (
                StatusCode::OK,
                Json(SnapshotImportResponse {
                    imported: true,
                    height,
                    validator_count,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// Accepts a batch of up to 100 transactions, verifies signatures in parallel
/// using rayon, and submits valid transactions to the mempool.
pub async fn submit_tx_batch(
    State(node): State<Arc<Node>>,
    Json(txs): Json<Vec<Transaction>>,
) -> ApiResult<TxBatchResponse> {
    use rayon::prelude::*;

    if txs.is_empty() {
        return Ok(Json(TxBatchResponse {
            total: 0,
            accepted: 0,
            results: vec![],
        }));
    }
    if txs.len() > 100 {
        return Err(ApiError::BadRequest(
            "Batch size exceeds maximum of 100 transactions".to_string(),
        ));
    }

    // Parallel signature verification — no locks held during CPU-bound work.
    let verifications: Vec<(String, Result<(), String>)> = txs
        .par_iter()
        .map(|tx| {
            let hash = hex::encode(tx.hash());
            // Payload bound before crypto, as in the single-tx path — and it matters
            // more here: the batch fans out over rayon, so the amplification is
            // multiplied by the number of entries.
            let result = payload_bound_msg(tx).map_or_else(
                || {
                    // VINX-03: same canonical predicate as the single-tx path — it
                    // also verifies the sponsor's key and signature.
                    WorldState::verify_tx_signature_pure(tx).map_err(|e| e.to_string())
                },
                Err,
            );
            (hash, result)
        })
        .collect();

    let total = txs.len();
    let mut results = Vec::with_capacity(total);
    let mut accepted = 0usize;
    // Same lock order as the persist path and admit_to_mempool: state before
    // mempool. Held across the batch so admission + insertion stay atomic; the
    // cumulative funding rule then naturally accounts for earlier transactions
    // of the same batch (they are already in the mempool when the next one is
    // checked).
    let state = node.state.read().await;
    let mut mempool = node.mempool.write().await;

    for (tx, (hash, sig_result)) in txs.into_iter().zip(verifications) {
        let admit = sig_result.and_then(|()| {
            state.admission_check(&tx).map_err(|e| e.to_string())?;
            let queued = mempool.queued_cost_atoms(&tx.sender());
            let needed = queued.saturating_add(tx.admission_cost_atoms());
            if state.account_balance(&tx.sender()).atoms() < needed {
                return Err(format!(
                    "sender balance does not cover already-queued transactions plus this one \
                     (queued cost {queued} atoms)"
                ));
            }
            mempool.add(tx.clone()).map_err(|e| e.to_string())?;
            relay(&node, &tx);
            Ok(())
        });
        match admit {
            Ok(()) => {
                node.metrics.tx_submitted_ok.fetch_add(1, Ordering::Relaxed);
                accepted += 1;
                results.push(BatchTxResult {
                    tx_hash: hash,
                    accepted: true,
                    error: None,
                });
            }
            Err(e) => {
                node.metrics
                    .tx_submitted_err
                    .fetch_add(1, Ordering::Relaxed);
                results.push(BatchTxResult {
                    tx_hash: hash,
                    accepted: false,
                    error: Some(e),
                });
            }
        }
    }

    Ok(Json(TxBatchResponse {
        total,
        accepted,
        results,
    }))
}

// ─── Merkle proof handler ────────────────────────────────────────────────────

pub async fn get_account_proof(
    State(node): State<Arc<Node>>,
    Path(raw_address): Path<String>,
) -> ApiResult<MerkleProofResponse> {
    let address = node.parse_address(&raw_address).await?;
    // Write lock: the proof flushes the account tree (a no-op right after a commit).
    let mut state = node.state.write().await;
    let (height, state_root) = {
        let chain = node.chain.read().await;
        let tip = chain
            .get_block(chain.tip_height())
            .ok_or_else(|| ApiError::Internal("tip block unavailable".to_string()))?;
        (tip.header.height, tip.header.state_root)
    };
    let p = state.account_proof(&address);
    let valid = p.verify(&state_root, &address, state.get_account(&address));
    Ok(Json(MerkleProofResponse {
        address: raw_address,
        height,
        state_root: hex::encode(state_root),
        accounts_root: hex::encode(p.accounts_root),
        consensus_root: hex::encode(p.consensus_root),
        key: hex::encode(p.key),
        leaf_value: p.leaf_value.map(hex::encode),
        proof_leaf: p.proof.leaf.map(|(k, v)| [hex::encode(k), hex::encode(v)]),
        siblings: p.proof.siblings.iter().map(hex::encode).collect(),
        valid,
    }))
}

pub async fn get_tx_receipt(
    Path(hash_hex): Path<String>,
    State(node): State<Arc<Node>>,
) -> ApiResult<TxReceiptResponse> {
    let hash_bytes: [u8; 32] = hex::decode(&hash_hex)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| ApiError::NotFound(format!("receipt not found for tx {hash_hex}")))?;
    let mut receipts = node.receipts.write().await;
    let receipt = receipts
        .get(&hash_bytes)
        .ok_or_else(|| ApiError::NotFound(format!("receipt not found for tx {hash_hex}")))?;
    Ok(Json(TxReceiptResponse {
        tx_hash: receipt.tx_hash.clone(),
        block_height: receipt.block_height,
        success: receipt.success,
        error: receipt.error.clone(),
    }))
}

/// `GET /tx/:hash/proof` — payment receipt with its inclusion proof (ADR 0083, L6).
pub async fn get_tx_proof(
    Path(hash_hex): Path<String>,
    State(node): State<Arc<Node>>,
) -> ApiResult<PaymentReceiptResponse> {
    let not_found = || ApiError::NotFound(format!("transaction {hash_hex} not found"));
    let hash: [u8; 32] = hex::decode(&hash_hex)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(not_found)?;
    let chain = node.chain.read().await;
    let (height, block, tx) = chain.get_tx_by_hash(&hash).ok_or_else(not_found)?;
    let hashes: Vec<vinx_crypto::Hash32> = block.transactions.iter().map(|t| t.hash()).collect();
    let index = hashes
        .iter()
        .position(|h| *h == hash)
        .ok_or_else(not_found)?;
    let siblings = vinx_crypto::tx_proof(&hashes, index)
        .ok_or_else(|| ApiError::Internal("proof generation failed".into()))?;
    Ok(Json(PaymentReceiptResponse {
        tx_hash: hash_hex.to_lowercase(),
        index: index as u32,
        siblings: siblings.iter().map(hex::encode).collect(),
        header: HeaderJson::from(&block.header),
        block_hash: hex::encode(block.hash()),
        commit: chain.get_commit(height).cloned(),
        tx: tx.clone(),
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
        mempool_max: node.config.max_mempool_size,
    }))
}
