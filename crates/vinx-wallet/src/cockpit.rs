//! Cockpit : interface web locale pour piloter le réseau depuis le poste
//! d'administration.
//!
//! `vinx-wallet cockpit` sert une page sur `127.0.0.1` et une petite API
//! locale. Les clés restent sur le poste : la page n'en voit jamais le
//! contenu, seul ce programme les déchiffre, avec le mot de passe saisi pour
//! chaque signature, jamais gardé. La page vient de ce programme, pas des
//! serveurs : un nœud compromis ne peut pas la modifier.
//!
//! Le dossier de travail contient :
//! - `cockpit.json` : les nœuds suivis (nom et URL) ;
//! - les fichiers de clé (`*.json`) : ceux qui correspondent à un signataire
//!   admin de la chaîne sont reconnus comme tels, les autres sont des comptes ;
//! - `propositions/` : les propositions admin, au format de `admin propose`.
//!
//! Protections de l'API locale : écoute sur 127.0.0.1 seulement, en-tête
//! `Host` vérifié (contre le « DNS rebinding »), jeton aléatoire exigé sur
//! chaque appel (une autre page ouverte dans le navigateur ne peut pas le
//! lire, donc pas agir).

use crate::admin;
use crate::amount::{format_vinx, parse_vinx};
use crate::client::Client;
use crate::{ops, params, preflight};
use anyhow::{Context, Result, anyhow, bail};
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use vinx_core::{AdminAction, SignedAdminTransaction};
use vinx_crypto::{Address, KeyPair};
use vinx_keystore::encoding::{key_from_hex, key_to_hex};
use vinx_keystore::keyfile::{DEFAULT_KDF, KeyFile};

const CONFIG: &str = "cockpit.json";
const PROPOSALS: &str = "propositions";
/// Routes du nœud que la page peut lire à travers le cockpit.
const NODE_ROUTES: [&str; 10] = [
    "/status",
    "/governance",
    "/validators",
    "/escrow",
    "/fees",
    "/mempool",
    "/admin-history",
    "/blocks",
    "/history",
    "/evidence",
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub nodes: Vec<NodeEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeEntry {
    pub name: String,
    pub url: String,
}

struct Cockpit {
    dir: PathBuf,
    token: String,
    port: u16,
}

type Shared = Arc<Cockpit>;
type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn fail(e: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": e.to_string() })),
    )
}

/// Exécute un travail bloquant (réseau, Argon2) hors des fils d'axum.
async fn blocking<F>(f: F) -> ApiResult
where
    F: FnOnce() -> Result<Value> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(Json(v)),
        Ok(Err(e)) => Err(fail(format!("{e:#}"))),
        Err(e) => Err(fail(e)),
    }
}

impl Cockpit {
    fn config(&self) -> Result<Config> {
        let path = self.dir.join(CONFIG);
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("lecture de {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("{} invalide", path.display()))
    }

    fn save_config(&self, config: &Config) -> Result<()> {
        let path = self.dir.join(CONFIG);
        std::fs::write(&path, serde_json::to_string_pretty(config)?)
            .with_context(|| format!("écriture de {}", path.display()))
    }

    /// Premier nœud joignable de la liste : c'est lui qui reçoit les envois.
    fn primary(&self) -> Result<Client> {
        let config = self.config()?;
        if config.nodes.is_empty() {
            bail!("aucun nœud configuré : ajoute-en un dans l'onglet Réseau");
        }
        for n in &config.nodes {
            let c = Client::new(&n.url);
            if c.status().is_ok() {
                return Ok(c);
            }
        }
        bail!("aucun nœud joignable : vérifie les tunnels SSH et les URL de l'onglet Réseau")
    }

    fn proposals_dir(&self) -> Result<PathBuf> {
        let d = self.dir.join(PROPOSALS);
        std::fs::create_dir_all(&d).with_context(|| format!("création de {}", d.display()))?;
        Ok(d)
    }

    /// Fichier de proposition désigné par son nom, sans sortir du dossier.
    fn proposal_path(&self, file: &str) -> Result<PathBuf> {
        if file.is_empty() || file.contains(['/', '\\']) || file.starts_with('.') {
            bail!("nom de fichier invalide : {file}");
        }
        Ok(self.proposals_dir()?.join(file))
    }

    /// Fichier de clé du dossier, désigné par son nom.
    fn key_path(&self, file: &str) -> Result<PathBuf> {
        if file.is_empty() || file.contains(['/', '\\']) || file.starts_with('.') {
            bail!("nom de fichier invalide : {file}");
        }
        let p = self.dir.join(file);
        if !p.exists() {
            bail!("fichier de clé introuvable dans le dossier du cockpit : {file}");
        }
        Ok(p)
    }
}

/// Champ JSON par chemin (`/a/b`), `null` s'il manque.
fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    v.pointer(path).unwrap_or(&Value::Null)
}

/// Fichiers de clé du dossier : (nom de fichier, clé publique en hexadécimal).
fn key_files(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            if path.extension()? != "json" {
                return None;
            }
            let name = path.file_name()?.to_str()?.to_owned();
            if name == CONFIG {
                return None;
            }
            let key = KeyFile::load(&path).ok()?;
            Some((name, key.public_key))
        })
        .collect();
    out.sort();
    out
}

fn decrypt(path: &Path, password: &str) -> Result<KeyPair> {
    KeyFile::load(path)?
        .decrypt(password)
        .map_err(|_| anyhow!("mot de passe incorrect pour {}", path.display()))
}

/// Action admin lisible.
fn describe(action: &AdminAction) -> Value {
    match action {
        AdminAction::EscrowRelease { amount } => {
            json!({ "kind": "escrow-release", "label": "Libération d'escrow", "amount": format_vinx(*amount) })
        }
        AdminAction::EscrowReturn { amount } => {
            json!({ "kind": "escrow-return", "label": "Retour en escrow", "amount": format_vinx(*amount) })
        }
        AdminAction::TreasuryTransfer { to, amount, memo } => json!({
            "kind": "treasury-transfer",
            "label": "Transfert de trésorerie",
            "to": to.to_string(),
            "amount": format_vinx(*amount),
            "memo": memo.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()),
        }),
        AdminAction::AddValidator { key } => {
            json!({ "kind": "add-validator", "label": "Ajout de validateur", "key": key_to_hex(key) })
        }
        AdminAction::RemoveValidator { key } => {
            json!({ "kind": "remove-validator", "label": "Retrait de validateur", "key": key_to_hex(key) })
        }
        AdminAction::SetParam { param, value } => json!({
            "kind": "set-param",
            "label": "Paramètre",
            "param": params::param_name(*param),
            "value": value,
        }),
        AdminAction::ReplaceSigner { index, new_key } => json!({
            "kind": "replace-signer",
            "label": "Remplacement de signataire",
            "index": index,
            "new_key": key_to_hex(new_key),
        }),
        AdminAction::ScheduleUpgrade {
            version,
            activation_height,
        } => json!({
            "kind": "schedule-upgrade",
            "label": "Mise à jour du protocole",
            "version": version,
            "activation_height": activation_height,
        }),
    }
}

fn str_field<'a>(v: &'a Value, name: &str) -> Result<&'a str> {
    v.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("champ « {name} » manquant"))
}

fn u64_field(v: &Value, name: &str) -> Result<u64> {
    match v.get(name) {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
    .ok_or_else(|| anyhow!("champ « {name} » : nombre entier attendu"))
}

/// Action admin à partir du formulaire de la page.
fn parse_action(v: &Value) -> Result<AdminAction> {
    let memo = v
        .get("memo")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| m.as_bytes().to_vec());
    Ok(match str_field(v, "kind")? {
        "escrow-release" => AdminAction::EscrowRelease {
            amount: parse_vinx(str_field(v, "amount")?)?,
        },
        "escrow-return" => AdminAction::EscrowReturn {
            amount: parse_vinx(str_field(v, "amount")?)?,
        },
        "treasury-transfer" => AdminAction::TreasuryTransfer {
            to: str_field(v, "to")?
                .parse::<Address>()
                .map_err(|e| anyhow!("destinataire : {e}"))?,
            amount: parse_vinx(str_field(v, "amount")?)?,
            memo,
        },
        "add-validator" => AdminAction::AddValidator {
            key: key_from_hex(str_field(v, "key")?)?,
        },
        "remove-validator" => AdminAction::RemoveValidator {
            key: key_from_hex(str_field(v, "key")?)?,
        },
        "set-param" => AdminAction::SetParam {
            param: params::param_id(str_field(v, "param")?)?,
            value: u64_field(v, "value")?,
        },
        "replace-signer" => AdminAction::ReplaceSigner {
            index: u8::try_from(u64_field(v, "index")?)
                .map_err(|_| anyhow!("indice de signataire invalide"))?,
            new_key: key_from_hex(str_field(v, "new_key")?)?,
        },
        "schedule-upgrade" => AdminAction::ScheduleUpgrade {
            version: u32::try_from(u64_field(v, "version")?)
                .map_err(|_| anyhow!("version invalide"))?,
            activation_height: u64_field(v, "activation_height")?,
        },
        other => bail!("action inconnue : {other}"),
    })
}

/// Propositions du dossier, avec leur état par rapport au nœud.
fn list_proposals(c: &Cockpit) -> Result<Value> {
    let dir = c.proposals_dir()?;
    let node = c.primary().ok();
    let gov = node.as_ref().and_then(|n| n.governance().ok());
    let node_nonce = gov.as_ref().and_then(|g| at(g, "/admin/nonce").as_u64());
    let chain_id = node
        .as_ref()
        .and_then(|n| n.status().ok())
        .map(|s| s.chain_id);
    let history: Vec<String> = node
        .as_ref()
        .and_then(|n| n.get_json("/admin-history").ok().flatten())
        .and_then(|h| {
            at(&h, "/actions").as_array().map(|a| {
                a.iter()
                    .filter_map(|x| at(x, "/id").as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default();
    let pending: Vec<String> = node
        .as_ref()
        .and_then(|n| n.get_json("/mempool").ok().flatten())
        .and_then(|m| {
            at(&m, "/admin_transactions").as_array().map(|a| {
                a.iter()
                    .filter_map(|x| at(x, "/id").as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default();
    let mut items = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(file) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        let Ok(tx) = admin::load(&path) else {
            items.push(json!({ "file": file, "status": "illisible" }));
            continue;
        };
        let id = hex::encode(tx.id());
        let approvals: Vec<u8> = tx.approvals.iter().map(|a| a.signer_index).collect();
        // Proposition d'un autre réseau (par exemple d'avant une remise à zéro) :
        // elle ne passera jamais, et ne doit pas compter dans la numérotation.
        let other_chain = chain_id.as_ref().is_some_and(|c| *c != tx.tx.chain_id);
        let status = if other_chain {
            "autre chaîne"
        } else if history.contains(&id) {
            "finalisée"
        } else if pending.contains(&id) {
            "envoyée"
        } else if node_nonce.is_some_and(|n| tx.tx.nonce <= n) {
            "périmée"
        } else if approvals.len() >= vinx_core::ADMIN_THRESHOLD {
            "prête"
        } else {
            "à signer"
        };
        let mut v = describe(&tx.tx.action);
        if let Some(o) = v.as_object_mut() {
            o.extend([
                ("file".into(), json!(file)),
                ("id".into(), json!(id)),
                ("nonce".into(), json!(tx.tx.nonce)),
                ("chain_id".into(), json!(tx.tx.chain_id)),
                ("approvals".into(), json!(approvals)),
                ("status".into(), json!(status)),
            ]);
        }
        items.push(v);
    }
    items.sort_by_key(|v| std::cmp::Reverse(at(v, "/nonce").as_u64().unwrap_or(0)));
    Ok(json!({ "node_nonce": node_nonce, "proposals": items }))
}

fn create_proposal(c: &Cockpit, body: &Value) -> Result<Value> {
    let action = parse_action(body.get("action").unwrap_or(&Value::Null))?;
    let node = c.primary()?;
    let status = node.status()?;
    // Après le nonce du nœud et celui des propositions locales encore en cours.
    let local_max = at(&list_proposals(c)?, "/proposals")
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|p| {
                    matches!(
                        at(p, "/status").as_str(),
                        Some("à signer" | "prête" | "envoyée")
                    )
                })
                .filter_map(|p| at(p, "/nonce").as_u64())
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let nonce = status
        .admin_nonce
        .max(local_max)
        .checked_add(1)
        .context("nonce admin épuisé")?;
    let tx = admin::propose(status.chain_id, nonce, action);
    let kind = at(&describe(&tx.tx.action), "/kind")
        .as_str()
        .unwrap_or("action")
        .to_owned();
    // Un fichier du même nom peut rester d'un autre réseau (remise à zéro) :
    // on numérote le nouveau au lieu de l'écraser.
    let mut file = format!("{nonce:05}-{kind}.json");
    let mut path = c.proposal_path(&file)?;
    let mut n = 1u32;
    while path.exists() {
        n = n
            .checked_add(1)
            .context("trop de propositions du même nom")?;
        file = format!("{nonce:05}-{kind}-{n}.json");
        path = c.proposal_path(&file)?;
    }
    admin::save(&tx, &path)?;
    Ok(json!({ "file": file, "nonce": nonce }))
}

/// Comptes et signataires : fichiers de clé du dossier, reconnus par leur clé.
fn keys(c: &Cockpit) -> Result<Value> {
    let node = c.primary().ok();
    let signers: Vec<String> = node
        .as_ref()
        .and_then(|n| n.governance().ok())
        .and_then(|g| {
            at(&g, "/admin/signers").as_array().map(|a| {
                a.iter()
                    .filter_map(|k| k.as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default();
    let mut admins = Vec::new();
    let mut accounts = Vec::new();
    for (file, key) in key_files(&c.dir) {
        if let Some(index) = signers.iter().position(|s| *s == key) {
            admins.push(json!({ "index": index, "file": file, "key": key }));
            continue;
        }
        let Ok(pk) = key_from_hex(&key) else { continue };
        let address = Address::from_public_key(&pk);
        let info = node
            .as_ref()
            .and_then(|n| n.account(&address).ok().flatten());
        accounts.push(json!({
            "file": file,
            "address": address.to_string(),
            "key": key,
            "balance": info.as_ref().map(|a| format_vinx(a.balance)),
            "exists": info.is_some(),
        }));
    }
    let missing: Vec<usize> = (0..signers.len())
        .filter(|i| {
            !admins
                .iter()
                .any(|a| at(a, "/index").as_u64() == Some(*i as u64))
        })
        .collect();
    Ok(json!({
        "dir": c.dir.display().to_string(),
        "signers": signers,
        "admins": admins,
        "missing_signers": missing,
        "accounts": accounts,
    }))
}

// ----- Routes -----

async fn page(State(c): State<Shared>) -> Html<String> {
    Html(render_page(include_str!("cockpit.html")).replace("__COCKPIT_TOKEN__", &c.token))
}

/// Police de la charte (DM Sans, licence OFL), servie avec les pages.
pub(crate) async fn font() -> impl axum::response::IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "font/woff2"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        include_bytes!("../../../assets/fonts/dm-sans.woff2").as_slice(),
    )
}

/// Page complète : feuille de style commune, logo et icône de la charte VinX
/// (`assets/`), insérés dans le gabarit.
pub(crate) fn render_page(template: &str) -> String {
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

/// Refuse tout appel sans jeton, ou adressé à un autre hôte que 127.0.0.1.
async fn guard(State(c): State<Shared>, headers: HeaderMap, req: Request, next: Next) -> Response {
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    let allowed = [
        format!("127.0.0.1:{}", c.port),
        format!("localhost:{}", c.port),
    ];
    if !allowed.iter().any(|a| a == host) {
        return (StatusCode::FORBIDDEN, "hôte refusé").into_response();
    }
    if req.uri().path().starts_with("/api/") {
        let token = headers
            .get("x-cockpit-token")
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        if token != c.token {
            return (StatusCode::FORBIDDEN, "jeton absent ou invalide").into_response();
        }
    }
    next.run(req).await
}

async fn get_config(State(c): State<Shared>) -> ApiResult {
    let config = c.config().map_err(fail)?;
    Ok(Json(json!(config)))
}

async fn put_config(State(c): State<Shared>, Json(config): Json<Config>) -> ApiResult {
    for n in &config.nodes {
        if !(n.url.starts_with("http://") || n.url.starts_with("https://")) {
            return Err(fail(format!(
                "URL invalide pour {} : http://… attendu",
                n.name
            )));
        }
    }
    c.save_config(&config).map_err(fail)?;
    Ok(Json(json!(config)))
}

/// État de chaque nœud configuré.
async fn nodes(State(c): State<Shared>) -> ApiResult {
    blocking(move || {
        let config = c.config()?;
        let items: Vec<Value> = config
            .nodes
            .iter()
            .map(|n| {
                let client = Client::new(&n.url);
                match client.status_json() {
                    Ok(status) => {
                        json!({ "name": n.name, "url": n.url, "ok": true, "status": status })
                    }
                    Err(e) => {
                        json!({ "name": n.name, "url": n.url, "ok": false, "error": e.to_string() })
                    }
                }
            })
            .collect();
        Ok(json!({ "nodes": items }))
    })
    .await
}

#[derive(Deserialize)]
struct NodeQuery {
    /// Indice du nœud dans la configuration ; à défaut, le premier joignable.
    node: Option<usize>,
    path: String,
}

/// Lecture d'une route du nœud, limitée aux routes publiques en lecture.
async fn node_get(State(c): State<Shared>, Query(q): Query<NodeQuery>) -> ApiResult {
    blocking(move || {
        let route = q.path.split('?').next().unwrap_or_default();
        let allowed = NODE_ROUTES.contains(&route)
            || route
                .strip_prefix("/accounts/")
                .is_some_and(|a| !a.contains('/'))
            || route
                .strip_prefix("/blocks/")
                .is_some_and(|h| h.parse::<u64>().is_ok())
            || route
                .strip_prefix("/transactions/")
                .is_some_and(|t| !t.contains('/'));
        if !allowed {
            bail!("route non autorisée : {route}");
        }
        let client = match q.node {
            Some(i) => {
                let config = c.config()?;
                let n = config
                    .nodes
                    .get(i)
                    .ok_or_else(|| anyhow!("nœud {i} inconnu"))?;
                Client::new(&n.url)
            }
            None => c.primary()?,
        };
        client
            .get_json(&q.path)?
            .ok_or_else(|| anyhow!("introuvable sur le nœud"))
    })
    .await
}

async fn get_keys(State(c): State<Shared>) -> ApiResult {
    blocking(move || keys(&c)).await
}

async fn get_proposals(State(c): State<Shared>) -> ApiResult {
    blocking(move || list_proposals(&c)).await
}

async fn new_proposal(State(c): State<Shared>, Json(body): Json<Value>) -> ApiResult {
    blocking(move || create_proposal(&c, &body)).await
}

#[derive(Deserialize)]
struct Approve {
    file: String,
    /// Fichier de clé du signataire, dans le dossier du cockpit.
    key_file: String,
    password: String,
}

async fn approve(State(c): State<Shared>, Json(b): Json<Approve>) -> ApiResult {
    blocking(move || {
        let path = c.proposal_path(&b.file)?;
        let mut tx = admin::load(&path)?;
        let keypair = decrypt(&c.key_path(&b.key_file)?, &b.password)?;
        let signers = at(&keys(&c)?, "/signers").clone();
        let key = key_to_hex(&keypair.public_key());
        let index = signers
            .as_array()
            .and_then(|s| s.iter().position(|k| k.as_str() == Some(key.as_str())))
            .ok_or_else(|| {
                anyhow!(
                    "{} n'est pas une clé de signataire admin de la chaîne",
                    b.key_file
                )
            })?;
        admin::approve(&mut tx, u8::try_from(index)?, &keypair)?;
        admin::save(&tx, &path)?;
        Ok(json!({ "approvals": tx.approvals.len(), "index": index }))
    })
    .await
}

#[derive(Deserialize)]
struct FileRef {
    file: String,
}

async fn submit(State(c): State<Shared>, Json(b): Json<FileRef>) -> ApiResult {
    blocking(move || {
        let tx: SignedAdminTransaction = admin::load(&c.proposal_path(&b.file)?)?;
        let id = c.primary()?.submit_admin(&tx)?;
        Ok(json!({ "id": id }))
    })
    .await
}

async fn remove(State(c): State<Shared>, Json(b): Json<FileRef>) -> ApiResult {
    blocking(move || {
        std::fs::remove_file(c.proposal_path(&b.file)?)?;
        Ok(json!({}))
    })
    .await
}

#[derive(Deserialize)]
struct Import {
    file: String,
    content: String,
}

/// Importe une proposition reçue d'un autre signataire.
async fn import(State(c): State<Shared>, Json(b): Json<Import>) -> ApiResult {
    blocking(move || {
        let path = c.proposal_path(&b.file)?;
        let tmp = c.proposal_path(&format!("{}.tmp", b.file))?;
        std::fs::write(&tmp, &b.content)?;
        let parsed = admin::load(&tmp);
        let _ = std::fs::remove_file(&tmp);
        let tx = parsed.context("ce fichier n'est pas une proposition VinX")?;
        admin::save(&tx, &path)?;
        Ok(json!({ "file": b.file }))
    })
    .await
}

#[derive(Deserialize)]
struct Export {
    file: String,
}

async fn export(
    State(c): State<Shared>,
    Query(q): Query<Export>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let path = c.proposal_path(&q.file).map_err(fail)?;
    let text = std::fs::read_to_string(&path).map_err(fail)?;
    Ok((
        [
            ("content-type", "application/json".to_owned()),
            (
                "content-disposition",
                format!("attachment; filename=\"{}\"", q.file),
            ),
        ],
        text,
    )
        .into_response())
}

#[derive(Deserialize)]
struct Preflight {
    urls: Vec<String>,
}

async fn run_preflight(State(c): State<Shared>, Json(b): Json<Preflight>) -> ApiResult {
    blocking(move || {
        let reference = c.primary()?;
        let nodes: Vec<Client> = b.urls.iter().map(Client::new).collect();
        let (_, report) = preflight::run(&reference, &nodes)?;
        Ok(json!({
            "errors": report.errors,
            "warnings": report.warnings,
            "keys_to_add": report.keys_to_add,
            "final_validators": report.final_validators,
            "quorum": preflight::quorum(report.final_validators),
        }))
    })
    .await
}

#[derive(Deserialize)]
struct NewAccount {
    name: String,
    password: String,
}

async fn new_account(State(c): State<Shared>, Json(b): Json<NewAccount>) -> ApiResult {
    blocking(move || {
        let name = b.name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_alphanumeric() || "-_".contains(ch))
        {
            bail!("nom invalide : lettres, chiffres, - et _ seulement");
        }
        if b.password.chars().count() < 8 {
            bail!("mot de passe trop court : 8 caractères au moins");
        }
        let path = c.dir.join(format!("{name}.json"));
        let keypair = KeyPair::generate();
        KeyFile::encrypt(&keypair, &b.password, DEFAULT_KDF)?.save(&path)?;
        Ok(json!({
            "file": format!("{name}.json"),
            "address": Address::from_public_key(&keypair.public_key()).to_string(),
        }))
    })
    .await
}

#[derive(Deserialize)]
struct Payment {
    key_file: String,
    password: String,
    to: String,
    amount: String,
    memo: Option<String>,
}

async fn send(State(c): State<Shared>, Json(b): Json<Payment>) -> ApiResult {
    blocking(move || {
        let keypair = decrypt(&c.key_path(&b.key_file)?, &b.password)?;
        let to: Address =
            b.to.trim()
                .parse()
                .map_err(|e| anyhow!("destinataire : {e}"))?;
        let memo = b.memo.filter(|m| !m.trim().is_empty());
        let id = ops::transfer(
            &c.primary()?,
            &keypair,
            None,
            to,
            parse_vinx(&b.amount)?,
            memo,
            None,
        )?;
        Ok(json!({ "id": id }))
    })
    .await
}

pub fn router(dir: PathBuf, token: String, port: u16) -> Router {
    let shared = Arc::new(Cockpit { dir, token, port });
    Router::new()
        .route("/", get(page))
        .route("/fonts/dm-sans.woff2", get(font))
        .route("/api/config", get(get_config).post(put_config))
        .route("/api/nodes", get(nodes))
        .route("/api/node", get(node_get))
        .route("/api/keys", get(get_keys))
        .route("/api/proposals", get(get_proposals).post(new_proposal))
        .route("/api/proposals/approve", post(approve))
        .route("/api/proposals/submit", post(submit))
        .route("/api/proposals/delete", post(remove))
        .route("/api/proposals/import", post(import))
        .route("/api/proposals/export", get(export))
        .route("/api/preflight", post(run_preflight))
        .route("/api/accounts", post(new_account))
        .route("/api/send", post(send))
        .layer(axum::middleware::from_fn_with_state(shared.clone(), guard))
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024))
        .with_state(shared)
}

/// Jeton de session : 32 octets aléatoires.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
    hex::encode(bytes)
}

/// Démarre le cockpit et attend Ctrl+C. `first_node` est ajouté à la
/// configuration si elle n'a encore aucun nœud.
pub fn serve(dir: PathBuf, port: u16, first_node: &str) -> Result<()> {
    let dir = dir
        .canonicalize()
        .with_context(|| format!("dossier introuvable : {}", dir.display()))?;
    let cockpit = Cockpit {
        dir: dir.clone(),
        token: String::new(),
        port,
    };
    let mut config = cockpit.config()?;
    if config.nodes.is_empty() {
        config.nodes.push(NodeEntry {
            name: "nœud principal".into(),
            url: first_node.to_owned(),
        });
        cockpit.save_config(&config)?;
    }
    let token = new_token();
    let app = router(dir.clone(), token, port);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .with_context(|| format!("port {port} déjà utilisé ? essaie --port"))?;
        println!("Cockpit VinX Ledger : http://127.0.0.1:{port}/");
        println!("dossier : {}", dir.display());
        println!("Ctrl+C pour arrêter.");
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .context("serveur du cockpit")
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // code de test
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app(dir: &Path) -> Router {
        router(dir.to_path_buf(), "jeton".into(), 8600)
    }

    async fn call(
        app: Router,
        host: &str,
        path: &str,
        token: Option<&str>,
    ) -> (StatusCode, String) {
        let mut req = axum::http::Request::builder()
            .uri(path)
            .header("host", host);
        if let Some(t) = token {
            req = req.header("x-cockpit-token", t);
        }
        let res = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = res.status();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn the_api_needs_the_token_and_the_local_host() {
        let dir = tempfile::tempdir().unwrap();
        let (s, page) = call(app(dir.path()), "127.0.0.1:8600", "/", None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(
            page.contains(r#"TOKEN = "jeton""#),
            "le jeton est servi avec la page"
        );
        let (s, _) = call(app(dir.path()), "127.0.0.1:8600", "/api/config", None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _) = call(
            app(dir.path()),
            "127.0.0.1:8600",
            "/api/config",
            Some("faux"),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        // Page atteinte par un nom de domaine qui pointe sur 127.0.0.1 (« DNS rebinding »).
        let (s, _) = call(app(dir.path()), "piege.example:8600", "/", None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, body) = call(
            app(dir.path()),
            "localhost:8600",
            "/api/config",
            Some("jeton"),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
    }

    #[tokio::test]
    async fn only_read_routes_of_the_node_are_relayed() {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            "/admin-transactions",
            "/transactions",
            "/../etc",
            "/blocks/abc",
        ] {
            let uri = format!("/api/node?path={path}");
            let (s, body) = call(app(dir.path()), "127.0.0.1:8600", &uri, Some("jeton")).await;
            assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{path}");
            assert!(body.contains("non autorisée"), "{path} : {body}");
        }
    }

    #[test]
    fn file_names_cannot_leave_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cockpit {
            dir: dir.path().to_path_buf(),
            token: String::new(),
            port: 0,
        };
        for bad in ["../x.json", "a/b.json", "..\\x.json", ".cache", ""] {
            assert!(c.proposal_path(bad).is_err(), "{bad}");
            assert!(c.key_path(bad).is_err(), "{bad}");
        }
        assert!(c.proposal_path("00004-set-param.json").is_ok());
    }

    #[test]
    fn form_values_become_admin_actions() {
        let a = parse_action(
            &json!({ "kind": "set-param", "param": "timeout_propose", "value": "1500" }),
        )
        .unwrap();
        assert_eq!(
            a,
            AdminAction::SetParam {
                param: vinx_core::ParamId::TimeoutPropose,
                value: 1500
            }
        );
        let a = parse_action(&json!({ "kind": "escrow-release", "amount": "7500000" })).unwrap();
        assert_eq!(
            a,
            AdminAction::EscrowRelease {
                amount: 7_500_000 * vinx_core::UNITS_PER_VINX
            }
        );
        assert!(
            parse_action(&json!({ "kind": "set-param", "param": "inconnu", "value": 1 })).is_err()
        );
        assert!(
            parse_action(&json!({ "kind": "treasury-transfer", "to": "01abc", "amount": "1" }))
                .is_err()
        );
        assert!(parse_action(&json!({ "kind": "rien" })).is_err());
        assert_eq!(describe(&a)["label"], json!("Libération d'escrow"));
    }
}
