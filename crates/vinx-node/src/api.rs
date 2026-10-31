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

/// Autorise les appels d'autres origines (portefeuille mobile, sites). L'API
/// est publique et sans cookie : n'importe quelle origine peut lire et
/// soumettre des transactions déjà signées, comme avec `curl`.
async fn cors(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{HeaderValue, Method, header};
    use axum::response::IntoResponse;
    let mut res = if req.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(req).await
    };
    let h = res.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("86400"),
    );
    res
}

/// Copie de l'état courant, prise sous le verrou puis relâchée : les tables
/// sont persistantes, la copie ne coûte presque rien. Une route qui parcourt
/// l'état travaille ainsi sans bloquer le consensus.
fn view(shared: &SharedRef) -> vinx_state::State {
    lock(shared).chain.state.clone()
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn error(status: StatusCode, message: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.to_string() })))
}

pub fn router(shared: SharedRef) -> Router {
    router_with_limit(shared, 0)
}

/// Comme [`router`], avec au plus `per_minute` requêtes par minute et par
/// adresse IP (0 : sans limite). L'adresse vient de la connexion ; derrière un
/// proxy local (Caddy), de son en-tête `X-Forwarded-For`.
pub fn router_with_limit(shared: SharedRef, per_minute: u32) -> Router {
    let limiter = std::sync::Arc::new(RateLimiter::new(per_minute));
    Router::new()
        .route("/", get(dashboard))
        .route("/explorer", get(explorer))
        .route("/fonts/{name}", get(font))
        .route("/metrics", get(metrics))
        .route("/governance", get(governance))
        .route("/blocks", get(recent_blocks))
        .route("/transactions/recent", get(recent_transactions))
        .route("/status", get(status))
        .route("/accounts/{address}", get(account))
        .route("/accounts/{address}/transactions", get(account_history))
        .route("/names/{name}", get(name))
        .route("/deals/{id}", get(deal))
        .route("/assets", get(assets))
        .route("/assets/{symbol}", get(asset))
        .route("/assets/{symbol}/holders", get(asset_holders))
        .route("/assets/{symbol}/transactions", get(asset_history))
        .route("/openapi.json", get(openapi))
        .route("/docs", get(docs))
        .route("/attestations/{digest}", get(attestation))
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
        .layer(axum::middleware::from_fn_with_state(limiter, rate_limit))
        // Le portefeuille mobile appelle l'API depuis sa propre origine.
        .layer(axum::middleware::from_fn(cors))
        .with_state(shared)
}

/// Compteur de requêtes par adresse IP, par fenêtre d'une minute.
pub struct RateLimiter {
    per_minute: u32,
    seen: std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, (u64, u32)>>,
}

/// Adresses suivies au plus ; au-delà, les fenêtres passées sont oubliées.
const MAX_TRACKED_IPS: usize = 100_000;

impl RateLimiter {
    pub fn new(per_minute: u32) -> Self {
        Self {
            per_minute,
            seen: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Compte une requête de `ip` à l'instant `now_ms` ; faux si la limite
    /// de la minute en cours est dépassée.
    pub fn allow(&self, ip: std::net::IpAddr, now_ms: u64) -> bool {
        if self.per_minute == 0 {
            return true;
        }
        let window = now_ms / 60_000;
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() >= MAX_TRACKED_IPS {
            seen.retain(|_, (w, _)| *w == window);
        }
        let entry = seen.entry(ip).or_insert((window, 0));
        if entry.0 != window {
            *entry = (window, 0);
        }
        entry.1 = entry.1.saturating_add(1);
        entry.1 <= self.per_minute
    }
}

/// Adresse du client : celle de la connexion, ou, si la connexion vient de la
/// machine elle-même (proxy local), la dernière de `X-Forwarded-For`, celle
/// que le proxy a ajoutée.
fn client_ip(req: &axum::extract::Request) -> Option<std::net::IpAddr> {
    let peer = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()?
        .0
        .ip();
    if !peer.is_loopback() {
        return Some(peer);
    }
    let forwarded = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .and_then(|v| v.trim().parse().ok());
    Some(forwarded.unwrap_or(peer))
}

async fn rate_limit(
    State(limiter): State<std::sync::Arc<RateLimiter>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Some(ip) = client_ip(&req) {
        if !limiter.allow(ip, crate::node::now_ms()) {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "trop de requêtes depuis cette adresse : réessaie dans une minute",
            )
            .into_response();
        }
    }
    next.run(req).await
}

/// Tableau de bord web : une page autonome qui interroge l'API.
async fn dashboard() -> axum::response::Html<&'static str> {
    static PAGE: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| render_page(include_str!("dashboard.html")));
    axum::response::Html(PAGE.as_str())
}

async fn explorer() -> axum::response::Html<&'static str> {
    static PAGE: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| render_page(include_str!("explorer.html")));
    axum::response::Html(PAGE.as_str())
}

/// Polices de la charte (licence OFL), servies avec les pages : DM Sans pour
/// le texte, et les seuls chiffres d'Inter (zéro barré, largeur fixe).
pub(crate) async fn font(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let bytes: &'static [u8] = match name.as_str() {
        "dm-sans.woff2" => include_bytes!("../../../assets/fonts/dm-sans.woff2"),
        "digits.woff2" => include_bytes!("../../../assets/fonts/digits.woff2"),
        _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (axum::http::header::CONTENT_TYPE, "font/woff2"),
            (
                axum::http::header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ),
        ],
        bytes,
    )
        .into_response()
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
        .replace("/*__CHARTS__*/", include_str!("../../../assets/charts.js"))
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
    let state = view(&shared);
    let st = &state;
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
            "name_fee": amount_to_string(st.registry.name_fee),
            "asset_fee": amount_to_string(st.assets.fee),
            "object_reserve": amount_to_string(st.reserves.per_object),
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
        "emission": {
            "budget": amount_to_string(st.emission.budget),
            "emitted": amount_to_string(st.emission.emitted),
            "remaining": amount_to_string(st.emission.remaining()),
            "pot": amount_to_string(st.emission.pot),
            "daily_rate": amount_to_string(st.emission.daily_rate(st.registry.now)),
            "epoch_blocks": st.emission.epoch_blocks,
            "next_epoch_height": st.height
                .checked_div(st.emission.epoch_blocks.max(1))
                .and_then(|e| e.checked_add(1))
                .and_then(|e| e.checked_mul(st.emission.epoch_blocks.max(1))),
            "proposed": st.emission.proposed.iter().map(|(k, n)| json!({
                "key": key_to_hex(k),
                "address": Address::from_public_key(k).to_string(),
                "blocks": n,
            })).collect::<Vec<_>>(),
            "fees_to_validators": st.protocol_version >= 3,
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

/// Dernières transactions des blocs conservés (50 au plus, sur les 500
/// derniers blocs), recalculées une fois par bloc : l'explorateur n'a plus
/// à parcourir les blocs un par un.
async fn recent_transactions(
    State(shared): State<SharedRef>,
    axum::extract::Query(q): axum::extract::Query<Limit>,
) -> ApiResult {
    let limit = q.limit.unwrap_or(10).clamp(1, RECENT_TXS);
    let mut s = lock(&shared);
    let height = s.chain.state.height;
    let fresh = s.recent_txs.as_ref().filter(|(h, _)| *h == height);
    let list = match fresh {
        Some((_, list)) => list.clone(),
        None => {
            let mut list = Vec::new();
            let mut h = height;
            let oldest = height.saturating_sub(500);
            while h > oldest && list.len() < RECENT_TXS {
                let Ok(Some(b)) = s.store.block(h) else { break };
                for t in b.txs.iter().rev() {
                    let mut v = tx_json(t);
                    if let Some(o) = v.as_object_mut() {
                        o.insert("height".into(), json!(h));
                        o.insert("timestamp".into(), json!(b.header.timestamp));
                    }
                    list.push(v);
                }
                h = h.saturating_sub(1);
            }
            list.truncate(RECENT_TXS);
            s.recent_txs = Some((height, list.clone()));
            list
        }
    };
    drop(s);
    let list: Vec<Value> = list.into_iter().take(limit).collect();
    Ok(Json(json!({ "transactions": list })))
}

/// Transactions récentes gardées en cache.
const RECENT_TXS: usize = 50;

/// Les 20 derniers blocs conservés, du plus récent au plus ancien.
#[derive(Deserialize)]
struct Before {
    /// Blocs strictement sous cette hauteur (pagination) ; les derniers sinon.
    before: Option<u64>,
    /// Nombre de blocs, 20 par défaut, 100 au plus.
    limit: Option<usize>,
}

async fn recent_blocks(
    State(shared): State<SharedRef>,
    axum::extract::Query(q): axum::extract::Query<Before>,
) -> ApiResult {
    let s = lock(&shared);
    let mut out = Vec::new();
    let mut h = q.before.map_or(s.chain.state.height, |b| {
        b.saturating_sub(1).min(s.chain.state.height)
    });
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    while h > 0 && out.len() < limit {
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
            "name_fee": amount_to_string(st.registry.name_fee),
            "asset_fee": amount_to_string(st.assets.fee),
            "object_reserve": amount_to_string(st.reserves.per_object),
        },
    })))
}

async fn account(State(shared): State<SharedRef>, Path(address): Path<String>) -> ApiResult {
    let address: Address = address
        .parse()
        .map_err(|e| error(StatusCode::BAD_REQUEST, e))?;
    let state = view(&shared);
    let st = &state;
    let account = st
        .accounts
        .get(&address)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "compte inconnu"))?;
    let objects = st.reserves.count(&address);
    let reserve = if st.protocol_version >= 6 {
        st.reserves.required(objects, st.params.min_reserve)
    } else {
        0
    };
    let reg = &st.registry;
    let names: Vec<Value> = reg
        .names_of(&address, reg.now)
        .into_iter()
        .map(|(n, expires_at)| json!({ "name": format!("{n}.vinx"), "expires_at": expires_at }))
        .collect();
    let mut deals: Vec<Value> = reg
        .deals_of(&address)
        .iter()
        .map(|(id, d)| deal_json(id, d, reg.now))
        .collect();
    deals.extend(
        st.assets
            .deals
            .iter()
            .filter(|(_, d)| {
                let d = &d.deal;
                d.buyer == address || d.seller == address || d.arbiter == Some(address)
            })
            .map(|(id, d)| asset_deal_json(id, d, reg.now)),
    );
    // Mandats en actifs : mêmes champs, montants en unités de `asset`.
    let in_asset = |mut v: Value, symbol: &str| {
        if let Some(o) = v.as_object_mut() {
            o.insert("asset".into(), json!(symbol));
        }
        v
    };
    let granted_assets = st
        .assets
        .mandates
        .iter()
        .filter(|((_, payer, _), _)| *payer == address)
        .map(|((sym, _, payee), m)| {
            in_asset(mandate_json(payee, &m.mandate, reg.now, "payee"), sym)
        });
    let received_assets = st
        .assets
        .mandates
        .iter()
        .filter(|((_, _, payee), _)| *payee == address)
        .map(|((sym, payer, _), m)| {
            in_asset(mandate_json(payer, &m.mandate, reg.now, "payer"), sym)
        });
    Ok(Json(json!({
        "address": address.to_string(),
        "balance": amount_to_string(account.balance),
        "nonce": account.nonce,
        "memo_required": account.memo_required,
        // Protocole 6 : objets possédés et solde que le compte doit garder.
        "objects": objects,
        "reserve": amount_to_string(reserve),
        "spendable": amount_to_string(account.balance.saturating_sub(reserve)),
        "public_key": account.key.as_ref().map(key_to_hex),
        "names": names,
        "deals": deals,
        "assets": st.assets.holdings_of(&address).iter().map(|(symbol, h)| {
            let asset = st.assets.assets.get(symbol.as_str());
            json!({
                "symbol": symbol,
                "name": asset.map(|a| a.name.clone()),
                "decimals": asset.map(|a| a.decimals),
                "units": amount_to_string(h.amount),
                "frozen": h.frozen || asset.is_some_and(|a| a.frozen),
            })
        }).collect::<Vec<_>>(),
        "mandates": {
            "granted": st.mandates.iter()
                .filter(|((payer, _), _)| *payer == address)
                .map(|((_, payee), m)| mandate_json(payee, m, st.registry.now, "payee"))
                .chain(granted_assets)
                .collect::<Vec<_>>(),
            "received": st.mandates.iter()
                .filter(|((_, payee), _)| *payee == address)
                .map(|((payer, _), m)| mandate_json(payer, m, st.registry.now, "payer"))
                .chain(received_assets)
                .collect::<Vec<_>>(),
        },
        "policy": st.policies.get(&address).map(|p| {
            let mut v = policy_json(&p.config);
            if let Some(o) = v.as_object_mut() {
                let today = st.registry.now / 86_400;
                let spent = if p.day == today { p.spent } else { 0 };
                o.insert("spent_today".into(), json!(amount_to_string(spent)));
            }
            v
        }),
    })))
}

#[derive(Deserialize)]
struct Limit {
    limit: Option<usize>,
}

/// Dernières transactions d'une adresse (blocs conservés), plus les siennes en attente.
async fn account_history(
    State(shared): State<SharedRef>,
    Path(address): Path<String>,
    axum::extract::Query(q): axum::extract::Query<Limit>,
) -> ApiResult {
    let address: Address = address
        .parse()
        .map_err(|e| error(StatusCode::BAD_REQUEST, e))?;
    let limit = q.limit.unwrap_or(50).min(200);
    let s = lock(&shared);
    let mut out: Vec<Value> = s
        .mempool
        .txs()
        .iter()
        .filter(|t| crate::store::parties(t).contains(&address))
        .map(|t| {
            let mut v = tx_json(t);
            if let Some(o) = v.as_object_mut() {
                o.insert("status".into(), json!("en attente"));
            }
            v
        })
        .collect();
    let done = s
        .store
        .address_history(&address, limit)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    for (height, timestamp, entry) in done {
        let mut v = match &entry {
            crate::store::HistoryEntry::User(t) => tx_json(t),
            crate::store::HistoryEntry::Admin(t) => treasury_payment_json(t),
        };
        if let Some(o) = v.as_object_mut() {
            o.insert("status".into(), json!("finalisée"));
            o.insert("height".into(), json!(height));
            o.insert("timestamp".into(), json!(timestamp));
        }
        out.push(v);
    }
    Ok(Json(json!({ "transactions": out })))
}

/// Paiement de la trésorerie, présenté comme un paiement ordinaire.
fn treasury_payment_json(t: &SignedAdminTransaction) -> Value {
    let AdminAction::TreasuryTransfer { to, amount, memo } = &t.tx.action else {
        return admin_json(t);
    };
    json!({
        "id": hash_to_hex(&t.id()),
        "kind": "paiement",
        "sender": Address::treasury().to_string(),
        "to": to.to_string(),
        "amount": amount_to_string(*amount),
        "memo": memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
            "memo_hex": memo.as_ref().map(hex::encode),
        "treasury": true,
    })
}

/// Horodatages d'une empreinte (`Attest`, protocole 4) encore dans les blocs conservés.
async fn attestation(State(shared): State<SharedRef>, Path(digest): Path<String>) -> ApiResult {
    let digest = hex::decode(digest.trim())
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "empreinte invalide : 64 caractères hexadécimaux",
            )
        })?;
    let s = lock(&shared);
    let found = s
        .store
        .attestations(&digest)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if found.is_empty() {
        return Err(error(
            StatusCode::NOT_FOUND,
            "aucun horodatage de cette empreinte dans les blocs conservés par ce nœud",
        ));
    }
    let list: Vec<Value> = found
        .into_iter()
        .map(|(height, timestamp, t)| {
            json!({
                "height": height,
                "timestamp": timestamp,
                "id": hash_to_hex(&t.id()),
                "sender": t.tx.sender.to_string(),
            })
        })
        .collect();
    Ok(Json(
        json!({ "digest": hex::encode(digest), "attestations": list }),
    ))
}

/// Titulaire d'un nom (`alice` ou `alice.vinx`) ; `available` s'il peut être enregistré.
async fn name(State(shared): State<SharedRef>, Path(name): Path<String>) -> ApiResult {
    let name = name.trim().to_lowercase();
    let name = name.strip_suffix(".vinx").unwrap_or(&name).to_string();
    if !vinx_state::registry::valid_name(&name) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "nom invalide : 3 à 32 caractères a-z, 0-9 et tiret",
        ));
    }
    let state = view(&shared);
    let reg = &state.registry;
    let record = reg.names.get(&name);
    let grace_end = record.map(|r| {
        r.expires_at
            .saturating_add(vinx_state::registry::NAME_GRACE)
    });
    Ok(Json(json!({
        "name": format!("{name}.vinx"),
        "owner": reg.resolve(&name, reg.now).map(|a| a.to_string()),
        "expires_at": record.map(|r| r.expires_at),
        "available": grace_end.is_none_or(|g| reg.now >= g),
        "fee": amount_to_string(reg.name_fee),
        "active": state.protocol_version >= 2,
    })))
}

/// Actif lisible ; montants en unités de l'actif (`decimals` décimales).
fn asset_json(symbol: &str, a: &vinx_state::Asset) -> Value {
    json!({
        "symbol": symbol,
        "name": a.name,
        "issuer": a.issuer.to_string(),
        "decimals": a.decimals,
        "supply_units": amount_to_string(a.supply),
        "max_supply_units": a.max_supply.map(amount_to_string),
        "powers": {
            "mint": a.powers.mint,
            "freeze": a.powers.freeze,
            "clawback": a.powers.clawback,
        },
        "frozen": a.frozen,
    })
}

async fn assets(State(shared): State<SharedRef>) -> ApiResult {
    let state = view(&shared);
    let st = &state;
    // Détenteurs (solde non nul) de chaque actif, en un seul passage.
    let mut holders: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for ((symbol, _), h) in st.assets.holdings.iter() {
        if h.amount > 0 {
            let n = holders.entry(symbol.as_str()).or_insert(0);
            *n = n.saturating_add(1);
        }
    }
    Ok(Json(json!({
        "active": st.protocol_version >= 5,
        "fee": amount_to_string(st.assets.fee),
        "assets": st.assets.assets.iter()
            .map(|(symbol, a)| {
                let mut v = asset_json(symbol, a);
                if let Some(o) = v.as_object_mut() {
                    o.insert("holders".into(), json!(holders.get(symbol.as_str()).copied().unwrap_or(0)));
                }
                v
            })
            .collect::<Vec<_>>(),
    })))
}

async fn asset(State(shared): State<SharedRef>, Path(symbol): Path<String>) -> ApiResult {
    let symbol = symbol.trim().to_ascii_uppercase();
    let state = view(&shared);
    let st = &state;
    let a = st
        .assets
        .assets
        .get(symbol.as_str())
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "actif inconnu"))?;
    let holders = st
        .assets
        .holdings
        .iter()
        .filter(|((s, _), h)| *s == symbol && h.amount > 0)
        .count();
    let mut v = asset_json(&symbol, a);
    if let Some(o) = v.as_object_mut() {
        o.insert("holders".into(), json!(holders));
    }
    Ok(Json(v))
}

/// Pagination par décalage.
#[derive(Deserialize)]
struct Page {
    /// Nombre d'entrées, 50 par défaut, 200 au plus.
    limit: Option<usize>,
    /// Entrées à sauter, 0 par défaut.
    offset: Option<usize>,
}

/// Comptes qui ont accepté l'actif, du plus gros solde au plus petit.
async fn asset_holders(
    State(shared): State<SharedRef>,
    Path(symbol): Path<String>,
    axum::extract::Query(q): axum::extract::Query<Page>,
) -> ApiResult {
    let symbol = symbol.trim().to_ascii_uppercase();
    let state = view(&shared);
    if !state.assets.assets.contains_key(symbol.as_str()) {
        return Err(error(StatusCode::NOT_FOUND, "actif inconnu"));
    }
    let mut rows: Vec<_> = state
        .assets
        .holdings
        .iter()
        .filter(|((s, _), _)| *s == symbol)
        .map(|((_, a), h)| (*a, h.clone()))
        .collect();
    rows.sort_by(|x, y| y.1.amount.cmp(&x.1.amount).then(x.0.cmp(&y.0)));
    let total = rows.len();
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0);
    let holders: Vec<Value> = rows
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(a, h)| {
            json!({
                "address": a.to_string(),
                "units": amount_to_string(h.amount),
                "frozen": h.frozen,
            })
        })
        .collect();
    Ok(Json(json!({
        "symbol": symbol,
        "total": total,
        "offset": offset,
        "holders": holders,
    })))
}

/// Historique d'un actif dans les blocs conservés.
async fn asset_history(
    State(shared): State<SharedRef>,
    Path(symbol): Path<String>,
    axum::extract::Query(q): axum::extract::Query<Before>,
) -> ApiResult {
    let symbol = symbol.trim().to_ascii_uppercase();
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let s = lock(&shared);
    if !s.chain.state.assets.assets.contains_key(symbol.as_str()) {
        return Err(error(StatusCode::NOT_FOUND, "actif inconnu"));
    }
    let done = s
        .store
        .asset_history(&symbol, q.before, limit)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    drop(s);
    let out: Vec<Value> = done
        .into_iter()
        .map(|(height, timestamp, t)| {
            let mut v = tx_json(&t);
            if let Some(o) = v.as_object_mut() {
                o.insert("height".into(), json!(height));
                o.insert("timestamp".into(), json!(timestamp));
            }
            v
        })
        .collect();
    Ok(Json(json!({ "symbol": symbol, "transactions": out })))
}

/// Description OpenAPI 3 de l'API, avec la version du nœud.
async fn openapi() -> axum::response::Response {
    use axum::response::IntoResponse;
    static SPEC: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        include_str!("openapi.json").replace("__VERSION__", env!("CARGO_PKG_VERSION"))
    });
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        SPEC.as_str(),
    )
        .into_response()
}

/// Documentation lisible de l'API, construite depuis `/openapi.json`.
async fn docs() -> axum::response::Html<&'static str> {
    static PAGE: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| render_page(include_str!("docs.html")));
    axum::response::Html(PAGE.as_str())
}

async fn deal(State(shared): State<SharedRef>, Path(id): Path<String>) -> ApiResult {
    let id = hex::decode(id.trim())
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "identifiant invalide"))?;
    let state = view(&shared);
    let st = &state;
    let now = st.registry.now;
    if let Some(d) = st.registry.deals.get(&id) {
        return Ok(Json(deal_json(&id, d, now)));
    }
    let d = st
        .assets
        .deals
        .get(&id)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "séquestre inconnu ou clos"))?;
    Ok(Json(asset_deal_json(&id, d, now)))
}

/// Séquestre en actif : `units` en unités de l'actif, au lieu de `amount`.
fn asset_deal_json(id: &vinx_crypto::Hash, d: &vinx_state::assets::AssetDeal, now: u64) -> Value {
    let mut v = deal_json(id, &d.deal, now);
    if let Some(o) = v.as_object_mut() {
        o.remove("amount");
        o.insert("asset".into(), json!(d.symbol));
        o.insert("units".into(), json!(amount_to_string(d.deal.amount)));
    }
    v
}

fn deal_json(id: &vinx_crypto::Hash, d: &vinx_state::Deal, now: u64) -> Value {
    json!({
        "id": hash_to_hex(id),
        "buyer": d.buyer.to_string(),
        "seller": d.seller.to_string(),
        "arbiter": d.arbiter.map(|a| a.to_string()),
        "amount": amount_to_string(d.amount),
        "deadline": d.deadline,
        "expired": now >= d.deadline,
        "memo": d.memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
    })
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
    let body = match t.body.inner() {
        TxBody::Transfer { to, amount, memo } => json!({
            "kind": "paiement",
            "to": to.to_string(),
            "amount": amount_to_string(*amount),
            "memo": memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
            "memo_hex": memo.as_ref().map(hex::encode),
        }),
        TxBody::SetMemoRequired { required } => json!({
            "kind": "mémo obligatoire",
            "required": required,
        }),
        TxBody::RotateKey { new_key } => json!({
            "kind": "rotation de clé",
            "new_key": key_to_hex(new_key),
        }),
        TxBody::RegisterName { name } => json!({
            "kind": "nom",
            "name": format!("{name}.vinx"),
        }),
        TxBody::TransferName { name, to } => json!({
            "kind": "cession de nom",
            "name": format!("{name}.vinx"),
            "to": to.to_string(),
        }),
        TxBody::OpenDeal {
            seller,
            arbiter,
            amount,
            deadline,
            memo,
        } => json!({
            "kind": "séquestre",
            "deal": hash_to_hex(&vinx_core::deal_id(&t.sender, t.nonce)),
            "to": seller.to_string(),
            "arbiter": arbiter.map(|a| a.to_string()),
            "amount": amount_to_string(*amount),
            "deadline": deadline,
            "memo": memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
            "memo_hex": memo.as_ref().map(hex::encode),
        }),
        TxBody::ReleaseDeal { id } => json!({
            "kind": "séquestre versé",
            "deal": hash_to_hex(id),
        }),
        TxBody::RefundDeal { id } => json!({
            "kind": "séquestre remboursé",
            "deal": hash_to_hex(id),
        }),
        TxBody::BatchTransfer { payments } => json!({
            "kind": "paiement groupé",
            "amount": amount_to_string(
                payments.iter().fold(0u64, |s, p| s.saturating_add(p.amount))
            ),
            "payments": payments.iter().map(|p| json!({
                "to": p.to.to_string(),
                "amount": amount_to_string(p.amount),
                "memo": p.memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
            })).collect::<Vec<_>>(),
        }),
        TxBody::Attest { digest } => json!({
            "kind": "horodatage",
            "digest": hex::encode(digest),
        }),
        TxBody::SetPolicy { policy } => json!({
            "kind": "règles du compte",
            "policy": policy_json(policy),
        }),
        TxBody::GrantMandate {
            payee,
            max_per_period,
            period,
            expires_at,
        } => json!({
            "kind": "mandat accordé",
            "to": payee.to_string(),
            "max_per_period": amount_to_string(*max_per_period),
            "period": period,
            "expires_at": expires_at,
        }),
        TxBody::RevokeMandate { payee } => json!({
            "kind": "mandat révoqué",
            "to": payee.to_string(),
        }),
        TxBody::Collect {
            payer,
            amount,
            memo,
        } => json!({
            "kind": "prélèvement",
            "payer": payer.to_string(),
            "amount": amount_to_string(*amount),
            "memo": memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
        }),
        // Montants d'actifs en unités de l'actif : leurs décimales sont propres à chacun.
        TxBody::CreateAsset {
            symbol,
            name,
            decimals,
            initial_supply,
            max_supply,
            powers,
        } => json!({
            "kind": "création d'actif",
            "asset": symbol,
            "name": name,
            "decimals": decimals,
            "units": initial_supply.to_string(),
            "max_supply_units": max_supply.map(|m| m.to_string()),
            "powers": { "mint": powers.mint, "freeze": powers.freeze, "clawback": powers.clawback },
        }),
        TxBody::MintAsset { symbol, to, amount } => json!({
            "kind": "émission d'actif",
            "asset": symbol,
            "to": to.to_string(),
            "units": amount.to_string(),
        }),
        TxBody::BurnAsset { symbol, amount } => json!({
            "kind": "destruction d'actif",
            "asset": symbol,
            "units": amount.to_string(),
        }),
        TxBody::TransferAsset {
            symbol,
            to,
            amount,
            memo,
        } => json!({
            "kind": "paiement en actif",
            "asset": symbol,
            "to": to.to_string(),
            "units": amount.to_string(),
            "memo": memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
            "memo_hex": memo.as_ref().map(hex::encode),
        }),
        TxBody::FreezeAsset {
            symbol,
            account,
            frozen,
        } => json!({
            "kind": if *frozen { "gel d'actif" } else { "dégel d'actif" },
            "asset": symbol,
            "account": account.map(|a| a.to_string()),
        }),
        TxBody::ClawbackAsset {
            symbol,
            from,
            amount,
        } => json!({
            "kind": "reprise forcée",
            "asset": symbol,
            "from": from.to_string(),
            "units": amount.to_string(),
        }),
        TxBody::RenounceAssetPowers { symbol, powers } => json!({
            "kind": "abandon de pouvoirs",
            "asset": symbol,
            "powers": { "mint": powers.mint, "freeze": powers.freeze, "clawback": powers.clawback },
        }),
        TxBody::OpenAssetDeal {
            symbol,
            seller,
            arbiter,
            amount,
            deadline,
            memo,
        } => json!({
            "kind": "séquestre",
            "asset": symbol,
            "deal": hash_to_hex(&vinx_core::deal_id(&t.sender, t.nonce)),
            "to": seller.to_string(),
            "arbiter": arbiter.map(|a| a.to_string()),
            "units": amount.to_string(),
            "deadline": deadline,
            "memo": memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
            "memo_hex": memo.as_ref().map(hex::encode),
        }),
        TxBody::GrantAssetMandate {
            symbol,
            payee,
            max_per_period,
            period,
            expires_at,
        } => json!({
            "kind": "mandat accordé",
            "asset": symbol,
            "to": payee.to_string(),
            "max_per_period_units": max_per_period.to_string(),
            "period": period,
            "expires_at": expires_at,
        }),
        TxBody::RevokeAssetMandate { symbol, payee } => json!({
            "kind": "mandat révoqué",
            "asset": symbol,
            "to": payee.to_string(),
        }),
        TxBody::CollectAsset {
            symbol,
            payer,
            amount,
            memo,
        } => json!({
            "kind": "prélèvement",
            "asset": symbol,
            "payer": payer.to_string(),
            "units": amount.to_string(),
            "memo": memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
        }),
        TxBody::AcceptAsset { symbol, accept } => json!({
            "kind": if *accept { "actif accepté" } else { "actif retiré" },
            "asset": symbol,
        }),
        // `inner()` ne renvoie jamais une enveloppe.
        TxBody::Approved { .. } | TxBody::Sponsored { .. } => json!({ "kind": "enveloppe" }),
    };
    if let TxBody::Sponsored { sponsor, .. } = &t.body {
        if let Some(o) = v.as_object_mut() {
            o.insert("sponsor".into(), json!(sponsor.to_string()));
        }
    }
    if let Some(approvals) = t.body.approvals() {
        if let Some(o) = v.as_object_mut() {
            o.insert("approvals".into(), json!(approvals.len()));
        }
    }
    if let (Some(o), Some(b)) = (v.as_object_mut(), body.as_object()) {
        o.extend(b.clone());
    }
    v
}

/// Mandat lisible ; `other` est l'autre partie, nommée `role`.
fn mandate_json(other: &Address, m: &vinx_state::Mandate, now: u64, role: &str) -> Value {
    json!({
        role: other.to_string(),
        "max_per_period": amount_to_string(m.max_per_period),
        "period": m.period,
        "expires_at": m.expires_at,
        "available": amount_to_string(m.available(now)),
        "expired": m.expires_at.is_some_and(|e| now >= e),
    })
}

/// Règles d'un compte, lisibles.
fn policy_json(p: &vinx_core::PolicyConfig) -> Value {
    json!({
        "signers": p.signers.iter().map(|k| json!({
            "key": key_to_hex(k),
            "address": Address::from_public_key(k).to_string(),
        })).collect::<Vec<_>>(),
        "threshold": p.threshold,
        "daily_limit": p.daily_limit.map(amount_to_string),
    })
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
            "memo_hex": memo.as_ref().map(hex::encode),
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
        ParamId::NameFee => "name_fee",
        ParamId::AssetFee => "asset_fee",
        ParamId::ObjectReserve => "object_reserve",
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

#[cfg(test)]
#[allow(clippy::unwrap_used)] // code de test
mod rate_tests {
    use super::RateLimiter;

    #[test]
    fn limit_is_per_ip_and_per_minute() {
        let l = RateLimiter::new(3);
        let (a, b) = ("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap());
        assert!((0..3).all(|_| l.allow(a, 1_000)));
        assert!(!l.allow(a, 2_000), "4e requête de la minute");
        assert!(l.allow(b, 2_000), "autre adresse");
        assert!(l.allow(a, 61_000), "minute suivante");
        let off = RateLimiter::new(0);
        assert!((0..100).all(|_| off.allow(a, 0)), "0 : sans limite");
    }
}
