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
use vinx_core::{AdminAction, ParamId, SignedAdminTransaction, SignedTransaction, TxBody, decode};
use vinx_crypto::{Address, PublicKey};

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
        .route("/validators", get(validators))
        .route("/history", get(history))
        .route("/mempool", get(mempool))
        .route("/admin-history", get(admin_history))
        .route("/transactions/{id}", get(transaction))
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
    static PAGE: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| render_page(include_str!("dashboard.html")));
    axum::response::Html(PAGE.as_str())
}

/// Page complète : feuille de style commune, logo et icône de la charte VinX
/// (`assets/`), insérés dans le gabarit.
fn render_page(template: &str) -> String {
    let logo = include_str!("../../../assets/logo.svg").trim();
    let favicon = format!(
        "data:image/svg+xml,{}",
        logo.replace("currentColor", "#0DDC8E")
            .replace('"', "'")
            .replace('#', "%23")
            .replace('<', "%3C")
            .replace('>', "%3E")
    );
    template
        .replace("/*__THEME__*/", include_str!("../../../assets/theme.css"))
        .replace("<!--__LOGO__-->", logo)
        .replace("__FAVICON__", &favicon)
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
    // Par validateur, d'après les certificats de la dernière heure.
    let set = st.validators_at(st.height.saturating_add(1));
    let reports = m.activity.validators(&set, crate::node::now_ms());
    out.push_str(
        "# HELP vinx_validator_up 1 si le validateur a signé l'un des derniers blocs.\n\
         # TYPE vinx_validator_up gauge\n",
    );
    for (i, r) in reports.iter().enumerate() {
        let up = u8::from(r.presence == crate::activity::Presence::Online);
        out.push_str(&format!(
            "vinx_validator_up{{{}}} {up}\n",
            validator_labels(&s, i, &r.key)
        ));
    }
    out.push_str(
        "# HELP vinx_validator_signed_ratio Part des blocs de la dernière heure signés.\n\
         # TYPE vinx_validator_signed_ratio gauge\n",
    );
    for (i, r) in reports.iter().enumerate() {
        if r.expected > 0 {
            out.push_str(&format!(
                "vinx_validator_signed_ratio{{{}}} {}\n",
                validator_labels(&s, i, &r.key),
                r.signed as f64 / r.expected as f64
            ));
        }
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
        // Sur la dernière heure ; depuis le démarrage dans `first_round_ratio_total`.
        "first_round_ratio": s.metrics.recent_first_round_ratio(),
        "first_round_ratio_total": (s.metrics.blocks > 0).then(|| {
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
    let cert = s
        .store
        .certificate(height)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    // Ensemble de validateurs actuel : exact tant qu'il n'a pas changé depuis.
    let set = s.chain.state.validators_at(height);
    let signers: Vec<Value> = cert
        .iter()
        .flat_map(|c| c.signatures.iter())
        .filter_map(|(i, _)| {
            let i = usize::from(*i);
            Some(validator_json(&s, i, set.get(i)?))
        })
        .collect();
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
        "round": cert.as_ref().map(|c| c.round),
        "proposer_validator": set.get(usize::from(h.proposer)).map(|k| validator_json(&s, usize::from(h.proposer), k)),
        "signers": signers,
        "admin_details": block.admin_txs.iter().map(admin_json).collect::<Vec<_>>(),
        // Le détail des 200 premières : au-delà, la réponse deviendrait lourde.
        "details": block.txs.iter().take(200).map(tx_json).collect::<Vec<_>>(),
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
        "released": vinx_state::escrow::GENESIS_TRANCHES
            .saturating_sub(u64::try_from(e.tranches.len()).unwrap_or(u64::MAX)),
        "schedule": e.tranches.iter().take(24).map(|t| json!({
            "amount": amount_to_string(t.amount),
            "matures_at": t.matures_at,
        })).collect::<Vec<_>>(),
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
    let id = match s.mempool.add(tx.clone()) {
        Ok(id) => id,
        Err(e) => {
            s.record_reject(Some(tx.tx.sender.to_string()), e.to_string());
            return Err(error(StatusCode::UNPROCESSABLE_ENTITY, e));
        }
    };
    s.broadcast(NetMessage::Tx(tx));
    Ok(Json(json!({ "id": hash_to_hex(&id) })))
}

async fn submit_admin(State(shared): State<SharedRef>, Json(body): Json<Submission>) -> ApiResult {
    let tx: SignedAdminTransaction = decode_hex(&body.tx)?;
    let mut s = lock(&shared);
    let id = match s.mempool.add_admin(tx.clone()) {
        Ok(id) => id,
        Err(e) => {
            s.record_reject(Some("admin".into()), e.to_string());
            return Err(error(StatusCode::UNPROCESSABLE_ENTITY, e));
        }
    };
    s.broadcast(NetMessage::AdminTx(tx));
    Ok(Json(json!({ "id": hash_to_hex(&id) })))
}

/// Transaction utilisateur lisible.
fn tx_json(stx: &SignedTransaction) -> Value {
    let t = &stx.tx;
    let mut v = json!({
        "id": hash_to_hex(&stx.id()),
        "sender": t.sender.to_string(),
        "nonce": t.nonce,
        "fee": amount_to_string(t.fee),
    });
    let body = match &t.body {
        TxBody::Transfer { to, amount, memo } => json!({
            "kind": "paiement",
            "to": to.to_string(),
            "amount": amount_to_string(*amount),
            "memo": memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
        }),
        TxBody::SetMemoRequired { required } => json!({
            "kind": "mémo obligatoire",
            "required": required,
        }),
        TxBody::RotateKey { new_key } => json!({
            "kind": "rotation de clé",
            "new_key": key_to_hex(new_key),
        }),
    };
    if let (Some(o), Some(b)) = (v.as_object_mut(), body.as_object()) {
        o.extend(b.clone());
    }
    v
}

/// Action admin lisible.
fn admin_json(satx: &SignedAdminTransaction) -> Value {
    let (kind, detail) = match &satx.tx.action {
        AdminAction::EscrowRelease { amount } => (
            "libération d'escrow",
            json!({ "amount": amount_to_string(*amount) }),
        ),
        AdminAction::EscrowReturn { amount } => (
            "retour en escrow",
            json!({ "amount": amount_to_string(*amount) }),
        ),
        AdminAction::TreasuryTransfer { to, amount, memo } => (
            "transfert de trésorerie",
            json!({
                "to": to.to_string(),
                "amount": amount_to_string(*amount),
                "memo": memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
            }),
        ),
        AdminAction::AddValidator { key } => {
            ("ajout de validateur", json!({ "key": key_to_hex(key) }))
        }
        AdminAction::RemoveValidator { key } => {
            ("retrait de validateur", json!({ "key": key_to_hex(key) }))
        }
        AdminAction::SetParam { param, value } => (
            "paramètre",
            json!({ "param": param_name(*param), "value": value }),
        ),
        AdminAction::ReplaceSigner { index, new_key } => (
            "remplacement de signataire",
            json!({ "index": index, "new_key": key_to_hex(new_key) }),
        ),
        AdminAction::ScheduleUpgrade {
            version,
            activation_height,
        } => (
            "mise à jour du protocole",
            json!({ "version": version, "activation_height": activation_height }),
        ),
    };
    json!({
        "id": hash_to_hex(&satx.id()),
        "nonce": satx.tx.nonce,
        "kind": kind,
        "detail": detail,
        "approvals": satx.approvals.iter().map(|a| a.signer_index).collect::<Vec<_>>(),
    })
}

/// Nom d'un paramètre, tel que l'attend `vinx-wallet admin propose set-param`.
pub fn param_name(p: ParamId) -> &'static str {
    match p {
        ParamId::MinFee => "min_fee",
        ParamId::MinReserve => "min_reserve",
        ParamId::MaxMemoBytes => "max_memo_bytes",
        ParamId::MaxBlockTxs => "max_block_txs",
        ParamId::MaxBlockBytes => "max_block_bytes",
        ParamId::TimeoutPropose => "timeout_propose",
        ParamId::TimeoutProposeDelta => "timeout_propose_delta",
        ParamId::TimeoutVote => "timeout_vote",
        ParamId::TimeoutVoteDelta => "timeout_vote_delta",
        ParamId::MinBlockInterval => "min_block_interval",
    }
}

/// Étiquettes Prometheus d'un validateur : numéro (à partir de 1) et nom.
fn validator_labels(s: &crate::node::Shared, index: usize, key: &PublicKey) -> String {
    let name: String = s
        .name_of(key)
        .unwrap_or_default()
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\n'))
        .collect();
    format!("validator=\"{}\",name=\"{name}\"", index.saturating_add(1))
}

/// Validateur : clé et nom lisible éventuel.
fn validator_json(s: &crate::node::Shared, index: usize, key: &PublicKey) -> Value {
    json!({
        "index": index,
        "key": key_to_hex(key),
        "name": s.name_of(key),
    })
}

/// État de chaque validateur, tiré des certificats de la dernière heure.
async fn validators(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let st = &s.chain.state;
    let set = st.validators_at(st.height.saturating_add(1));
    let me = s.metrics.validator_key.clone();
    let reports = s.metrics.activity.validators(&set, crate::node::now_ms());
    let items: Vec<Value> = reports
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut v = validator_json(&s, i, &r.key);
            if let Some(o) = v.as_object_mut() {
                o.extend([
                    ("presence".into(), json!(r.presence.as_str())),
                    ("last_signed_height".into(), json!(r.last_signed_height)),
                    ("last_signed_ms".into(), json!(r.last_signed_ms)),
                    ("signed".into(), json!(r.signed)),
                    ("expected".into(), json!(r.expected)),
                    ("missed_proposals".into(), json!(r.missed_proposals)),
                    ("this_node".into(), json!(me.as_ref() == Some(&r.key))),
                ]);
            }
            v
        })
        .collect();
    Ok(Json(json!({
        "window_blocks": s.metrics.activity.blocks.len(),
        "validators": items,
    })))
}

/// Blocs de la dernière heure, pour les graphiques.
async fn history(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let blocks: Vec<Value> = s
        .metrics
        .activity
        .blocks
        .iter()
        .map(|b| {
            json!({
                "height": b.height,
                "timestamp": b.timestamp,
                "round": b.round,
                "transactions": b.txs,
                // Inconnu pour un bloc relu sur le disque au démarrage.
                "peers": b.live.then_some(b.peers),
                "signers": b.participants(),
            })
        })
        .collect();
    Ok(Json(json!({ "blocks": blocks })))
}

/// Une transaction par son identifiant : finalisée (avec son bloc) ou en attente.
async fn transaction(State(shared): State<SharedRef>, Path(id): Path<String>) -> ApiResult {
    let bytes = hex::decode(id.trim())
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "identifiant invalide : 64 caractères hexadécimaux",
            )
        })?;
    let s = lock(&shared);
    if let Some(t) = s.mempool.txs().iter().find(|t| t.id() == bytes) {
        let mut v = tx_json(t);
        if let Some(o) = v.as_object_mut() {
            o.insert("status".into(), json!("en attente"));
        }
        return Ok(Json(v));
    }
    let height = s
        .store
        .tx_height(&bytes)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                "transaction inconnue de ce nœud (jamais vue, ou dans un bloc élagué)",
            )
        })?;
    let block = s
        .store
        .block(height)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "bloc élagué"))?;
    let found = block
        .txs
        .iter()
        .position(|t| t.id() == bytes)
        .and_then(|i| Some((i, tx_json(block.txs.get(i)?))))
        .or_else(|| {
            let i = block.admin_txs.iter().position(|t| t.id() == bytes)?;
            Some((i, admin_json(block.admin_txs.get(i)?)))
        });
    let (index, mut v) =
        found.ok_or_else(|| error(StatusCode::NOT_FOUND, "transaction absente de son bloc"))?;
    if let Some(o) = v.as_object_mut() {
        o.extend([
            ("status".into(), json!("finalisée")),
            ("height".into(), json!(height)),
            ("index".into(), json!(index)),
            ("timestamp".into(), json!(block.header.timestamp)),
        ]);
    }
    Ok(Json(v))
}

/// Historique des actions admin finalisées.
async fn admin_history(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let log = s
        .store
        .admin_log(100)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let items: Vec<Value> = log
        .iter()
        .map(|e| {
            let mut v = admin_json(&e.tx);
            if let Some(o) = v.as_object_mut() {
                o.extend([
                    ("height".into(), json!(e.height)),
                    ("timestamp".into(), json!(e.timestamp)),
                ]);
            }
            v
        })
        .collect();
    Ok(Json(json!({ "actions": items })))
}

/// Transactions en attente (les 100 premières par priorité) et derniers refus.
async fn mempool(State(shared): State<SharedRef>) -> ApiResult {
    let s = lock(&shared);
    let pending: Vec<Value> = s
        .mempool
        .by_priority()
        .iter()
        .take(100)
        .map(tx_json)
        .collect();
    let admin: Vec<Value> = s.mempool.admin_txs().iter().map(admin_json).collect();
    let rejects: Vec<Value> = s
        .rejects
        .iter()
        .map(|r| json!({ "at": r.at_ms, "sender": r.sender, "reason": r.reason }))
        .collect();
    Ok(Json(json!({
        "count": s.mempool.len(),
        "capacity": s.mempool.capacity(),
        "transactions": pending,
        "admin_transactions": admin,
        "rejects": rejects,
    })))
}
