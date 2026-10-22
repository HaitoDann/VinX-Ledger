//! API REST JSON du nœud.
//!
//! Les montants sont des chaînes en unités (1 VINX = 10⁹ unités) ; les
//! transactions sont soumises encodées en borsh, en hexadécimal.

use crate::encoding::{amount_to_string, hash_to_hex, key_to_hex};
use crate::network::NetMessage;
use crate::node::{SharedRef, lock};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use vinx_core::{SignedAdminTransaction, SignedTransaction, decode};
use vinx_crypto::Address;

/// Taille maximale d'une requête.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Requêtes traitées simultanément au plus.
pub const MAX_CONCURRENT_REQUESTS: usize = 64;

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn error(status: StatusCode, message: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.to_string() })))
}

pub fn router(shared: SharedRef) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/accounts/{address}", get(account))
        .route("/blocks/{height}", get(block))
        .route("/escrow", get(escrow))
        .route("/fees", get(fees))
        .route("/evidence", get(evidence))
        .route("/transactions", post(submit))
        .route("/admin-transactions", post(submit_admin))
        // Une transaction encodée tient largement en 64 Kio.
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        // Au-delà, les requêtes attendent au lieu d'épuiser le nœud.
        .layer(tower::limit::ConcurrencyLimitLayer::new(
            MAX_CONCURRENT_REQUESTS,
        ))
        .with_state(shared)
}

async fn status(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let st = &s.chain.state;
    Ok(Json(json!({
        "chain_id": st.chain_id,
        "protocol_version": st.protocol_version,
        "height": st.height,
        "last_block_hash": s.chain.last.as_ref().map(|h| hash_to_hex(&h.hash())),
        "state_root": hash_to_hex(&st.state_root()),
        "validators": st.validators_at(st.height.saturating_add(1)).iter().map(key_to_hex).collect::<Vec<_>>(),
        "pending_transactions": s.mempool.len(),
        "admin_nonce": st.admin.nonce,
        "params": {
            "min_fee": amount_to_string(st.params.min_fee),
            "min_reserve": amount_to_string(st.params.min_reserve),
            "max_memo_bytes": st.params.max_memo_bytes,
            "min_block_interval_ms": st.params.min_block_interval_ms,
        },
    })))
}

async fn account(State(shared): State<SharedRef>, Path(address): Path<String>) -> ApiResult {
    let address: Address = address
        .parse()
        .map_err(|e| error(StatusCode::BAD_REQUEST, e))?;
    let s = lock(&shared);
    let account = s
        .chain
        .state
        .accounts
        .get(&address)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "compte inconnu"))?;
    Ok(Json(json!({
        "address": address.to_string(),
        "balance": amount_to_string(account.balance),
        "nonce": account.nonce,
        "memo_required": account.memo_required,
        "public_key": account.key.as_ref().map(key_to_hex),
    })))
}

async fn block(State(shared): State<SharedRef>, Path(height): Path<u64>) -> ApiResult {
    let s = lock(&shared);
    let block = s
        .store
        .block(height)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "bloc introuvable"))?;
    let h = &block.header;
    Ok(Json(json!({
        "height": h.height,
        "hash": hash_to_hex(&h.hash()),
        "protocol_version": h.protocol_version,
        "timestamp": h.timestamp,
        "prev_hash": hash_to_hex(&h.prev_hash),
        "tx_root": hash_to_hex(&h.tx_root),
        "state_root": hash_to_hex(&h.state_root),
        "proposer": h.proposer,
        "admin_transactions": block.admin_txs.iter().map(|t| hash_to_hex(&t.id())).collect::<Vec<_>>(),
        "transactions": block.txs.iter().map(|t| hash_to_hex(&t.id())).collect::<Vec<_>>(),
    })))
}

async fn escrow(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let e = &s.chain.state.escrow;
    Ok(Json(json!({
        "total": e.total().to_string(),
        "tranches": e.tranches.len(),
        "period": e.period,
        "next": e.tranches.front().map(|t| json!({
            "amount": amount_to_string(t.amount),
            "matures_at": t.matures_at,
        })),
    })))
}

async fn fees(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let p = &s.chain.state.params;
    Ok(Json(json!({
        "min_fee": amount_to_string(p.min_fee),
        "recommended_fee": amount_to_string(s.mempool.recommended_fee(p.min_fee, p.max_block_txs)),
        "pending_transactions": s.mempool.len(),
        "max_block_txs": p.max_block_txs,
    })))
}

/// Preuves d'équivoque connues du nœud.
async fn evidence(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let list = s
        .store
        .evidence()
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let items: Vec<Value> = list
        .iter()
        .map(|ev| {
            let validators = s.chain.state.validators_at(ev.height());
            let index = ev.validator(validators.len());
            json!({
                "id": hash_to_hex(&ev.id()),
                "height": ev.height(),
                "round": ev.round(),
                "step": ev.step(),
                "validator_index": index,
                "validator_key": index
                    .and_then(|i| validators.get(usize::from(i)))
                    .map(key_to_hex),
                "proof": hex::encode(vinx_core::encode(ev)),
            })
        })
        .collect();
    Ok(Json(json!({ "evidence": items })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Submission {
    /// Transaction signée, encodée en borsh puis en hexadécimal.
    tx: String,
}

fn decode_hex<T: borsh::BorshDeserialize>(s: &str) -> Result<T, (StatusCode, Json<Value>)> {
    let bytes =
        hex::decode(s).map_err(|_| error(StatusCode::BAD_REQUEST, "hexadécimal invalide"))?;
    decode(&bytes).map_err(|_| error(StatusCode::BAD_REQUEST, "transaction mal encodée"))
}

async fn submit(State(shared): State<SharedRef>, Json(body): Json<Submission>) -> ApiResult {
    let tx: SignedTransaction = decode_hex(&body.tx)?;
    let mut s = lock(&shared);
    let id = s
        .mempool
        .add(tx.clone())
        .map_err(|e| error(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    s.broadcast(NetMessage::Tx(tx));
    Ok(Json(json!({ "id": hash_to_hex(&id) })))
}

async fn submit_admin(State(shared): State<SharedRef>, Json(body): Json<Submission>) -> ApiResult {
    let tx: SignedAdminTransaction = decode_hex(&body.tx)?;
    let mut s = lock(&shared);
    let id = s
        .mempool
        .add_admin(tx.clone())
        .map_err(|e| error(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    s.broadcast(NetMessage::AdminTx(tx));
    Ok(Json(json!({ "id": hash_to_hex(&id) })))
}
