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
        .route("/", get(dashboard))
        .route("/metrics", get(metrics))
        .route("/governance", get(governance))
        .route("/blocks", get(recent_blocks))
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

/// Tableau de bord web : une page autonome qui interroge l'API.
async fn dashboard() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("dashboard.html"))
}

/// Métriques au format texte de Prometheus.
async fn metrics(
    State(shared): State<SharedRef>,
) -> (StatusCode, [(&'static str, &'static str); 1], String) {
    let s = lock(&shared);
    let st = &s.chain.state;
    let m = &s.metrics;
    let last_block_s = s.chain.last.as_ref().map_or(0, |h| h.timestamp / 1000);
    let evidence = s.store.evidence().map_or(0, |e| e.len());
    let lowest = s.store.lowest_height().ok().flatten().unwrap_or(0);
    let validators = st.validators_at(st.height.saturating_add(1)).len();
    let next_tranche = st.escrow.tranches.front().map_or(0, |t| t.matures_at);
    let gauges: [(&str, &str, String); 16] = [
        (
            "vinx_height",
            "Hauteur du dernier bloc finalisé.",
            st.height.to_string(),
        ),
        (
            "vinx_last_block_timestamp_seconds",
            "Horodatage du dernier bloc finalisé.",
            last_block_s.to_string(),
        ),
        (
            "vinx_blocks_total",
            "Blocs finalisés depuis le démarrage du nœud.",
            m.blocks.to_string(),
        ),
        (
            "vinx_blocks_first_round_total",
            "Dont finalisés au premier round.",
            m.blocks_first_round.to_string(),
        ),
        (
            "vinx_last_block_round",
            "Round de finalisation du dernier bloc.",
            m.last_round.to_string(),
        ),
        ("vinx_peers", "Pairs connectés.", m.peers.to_string()),
        (
            "vinx_validators",
            "Validateurs de la prochaine hauteur.",
            validators.to_string(),
        ),
        (
            "vinx_is_validator",
            "1 si ce nœud vote.",
            u8::from(m.validator_index.is_some()).to_string(),
        ),
        (
            "vinx_mempool_transactions",
            "Transactions en attente.",
            s.mempool.len().to_string(),
        ),
        (
            "vinx_evidence_total",
            "Preuves d'équivoque connues.",
            evidence.to_string(),
        ),
        (
            "vinx_treasury_balance_units",
            "Solde de la trésorerie, en unités.",
            st.balance(&vinx_crypto::Address::treasury()).to_string(),
        ),
        (
            "vinx_escrow_units",
            "VINX encore en escrow, en unités.",
            st.escrow.total().to_string(),
        ),
        (
            "vinx_escrow_next_maturity_seconds",
            "Échéance de la prochaine tranche d'escrow (0 : aucune).",
            next_tranche.to_string(),
        ),
        (
            "vinx_accounts",
            "Nombre de comptes.",
            st.accounts.len().to_string(),
        ),
        (
            "vinx_lowest_block",
            "Plus petite hauteur conservée.",
            lowest.to_string(),
        ),
        (
            "vinx_snapshots_installed_total",
            "Instantanés d'état installés.",
            m.snapshots_installed.to_string(),
        ),
    ];
    let mut out = String::new();
    for (name, help, value) in gauges {
        out.push_str(&format!(
            "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}\n"
        ));
    }
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        out,
    )
}

/// Tout ce qui relève de la gouvernance : multisig, validateurs, paramètres,
/// mise à jour programmée, escrow et trésorerie.
async fn governance(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let st = &s.chain.state;
    let p = &st.params;
    let pending: Vec<Value> = st
        .validators
        .pending
        .iter()
        .map(|c| {
            let (change, key) = match &c.change {
                vinx_state::ValidatorChange::Add(k) => ("ajout", k),
                vinx_state::ValidatorChange::Remove(k) => ("retrait", k),
            };
            json!({ "change": change, "key": key_to_hex(key), "effective_height": c.effective_height })
        })
        .collect();
    Ok(Json(json!({
        "admin": {
            "signers": st.admin.signers.iter().map(key_to_hex).collect::<Vec<_>>(),
            "threshold": vinx_core::ADMIN_THRESHOLD,
            "nonce": st.admin.nonce,
        },
        "validators": {
            "active": st.validators.active.iter().map(key_to_hex).collect::<Vec<_>>(),
            "pending": pending,
        },
        "params": {
            "min_fee": amount_to_string(p.min_fee),
            "min_reserve": amount_to_string(p.min_reserve),
            "max_memo_bytes": p.max_memo_bytes,
            "max_block_txs": p.max_block_txs,
            "max_block_bytes": p.max_block_bytes,
            "timeout_propose_ms": p.timeout_propose_ms,
            "timeout_propose_delta_ms": p.timeout_propose_delta_ms,
            "timeout_vote_ms": p.timeout_vote_ms,
            "timeout_vote_delta_ms": p.timeout_vote_delta_ms,
            "min_block_interval_ms": p.min_block_interval_ms,
        },
        "protocol_version": st.protocol_version,
        "upgrade": st.upgrade.as_ref().map(|u| json!({
            "version": u.version,
            "activation_height": u.activation_height,
        })),
        "treasury": {
            "address": vinx_crypto::Address::treasury().to_string(),
            "balance": amount_to_string(st.balance(&vinx_crypto::Address::treasury())),
        },
        "escrow": {
            "total": st.escrow.total().to_string(),
            "tranches": st.escrow.tranches.len(),
            "next": st.escrow.tranches.front().map(|t| json!({
                "amount": amount_to_string(t.amount),
                "matures_at": t.matures_at,
            })),
        },
    })))
}

/// Les 20 derniers blocs conservés, du plus récent au plus ancien.
async fn recent_blocks(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let mut out = Vec::new();
    let mut h = s.chain.state.height;
    while h > 0 && out.len() < 20 {
        let Ok(Some(b)) = s.store.block(h) else { break };
        let round = s.store.certificate(h).ok().flatten().map(|c| c.round);
        out.push(json!({
            "height": h,
            "hash": hash_to_hex(&b.hash()),
            "timestamp": b.header.timestamp,
            "proposer": b.header.proposer,
            "round": round,
            "transactions": b.txs.len(),
            "admin_transactions": b.admin_txs.len(),
        }));
        h = h.saturating_sub(1);
    }
    Ok(Json(json!({ "blocks": out })))
}

async fn status(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let st = &s.chain.state;
    Ok(Json(json!({
        "chain_id": st.chain_id,
        "node_version": env!("CARGO_PKG_VERSION"),
        "protocol_version": st.protocol_version,
        "height": st.height,
        "last_block_hash": s.chain.last.as_ref().map(|h| hash_to_hex(&h.hash())),
        "state_root": hash_to_hex(&st.state_root()),
        "validators": st.validators_at(st.height.saturating_add(1)).iter().map(key_to_hex).collect::<Vec<_>>(),
        "pending_transactions": s.mempool.len(),
        "admin_nonce": st.admin.nonce,
        "peers": s.metrics.peers,
        "validator_index": s.metrics.validator_index,
        "validator_key": s.metrics.validator_key.as_ref().map(key_to_hex),
        "first_round_ratio": (s.metrics.blocks > 0).then(|| {
            s.metrics.blocks_first_round as f64 / s.metrics.blocks as f64
        }),
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
