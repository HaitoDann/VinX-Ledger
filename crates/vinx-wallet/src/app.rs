//! Portefeuille : application locale pour payer en VINX, pensée pour tout le
//! monde.
//!
//! `vinx-wallet` lancé sans argument (double-clic) ou `vinx-wallet app` sert
//! une page sur `127.0.0.1` et l'ouvre dans le navigateur. Comme le cockpit,
//! les clés ne quittent jamais ce programme : la page ne voit que des
//! adresses, des montants et des aperçus.
//!
//! - Une phrase de récupération de 12 mots (BIP-39) est créée à la première
//!   ouverture. Elle seule permet de tout restaurer sur un autre poste.
//! - Les comptes en sont dérivés : compte `i` = clé Ed25519 de
//!   `BLAKE3-derive_key(vinx_mobile::DERIVATION, graine ‖ i)`, `graine` étant la graine
//!   BIP-39 de la phrase, sans phrase secrète.
//! - `portefeuille.json` garde la phrase chiffrée (Argon2id +
//!   XChaCha20-Poly1305), les libellés des comptes, les contacts et l'URL du
//!   nœud. Après déverrouillage, la graine reste en mémoire jusqu'au
//!   verrouillage, automatique après 15 minutes sans activité.
//!
//! Avant chaque envoi, la page demande un aperçu : frais, total, solde après
//! l'opération et mises en garde (adresse inconnue, réserve minimale…). Les
//! mêmes vérifications sont refaites au moment de signer.

use crate::amount::{format_units, format_vinx, parse_units, parse_vinx};
use crate::client::Client;
use anyhow::{Context, Result, anyhow, bail};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use vinx_core::{Amount, Transaction, TxBody};
use vinx_crypto::{Address, KeyPair};
use vinx_keystore::keyfile::{DEFAULT_KDF, Kdf, Sealed};

const VAULT: &str = "portefeuille.json";
/// Verrouillage automatique après cette durée sans activité.
const LOCK_AFTER: Duration = Duration::from_secs(15 * 60);
/// Horodatage : la plus petite somme possible (une unité) versée à la
/// trésorerie, l'empreinte du fichier en mémo. Le protocole n'a pas encore de
/// transaction dédiée (prévue en v3).
const TIMESTAMP_UNITS: Amount = 1;
/// Échéance maximale d'un séquestre, en jours (la chaîne refuse au-delà d'un an).
const MAX_DEAL_DAYS: u64 = 365;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Vault {
    version: u8,
    node: String,
    phrase: Sealed,
    accounts: Vec<AccountEntry>,
    #[serde(default)]
    contacts: Vec<Contact>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AccountEntry {
    index: u32,
    label: String,
    address: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Contact {
    name: String,
    address: String,
}

struct Session {
    seed: [u8; 64],
    last_used: Instant,
}

struct App {
    dir: PathBuf,
    token: String,
    port: u16,
    kdf: Kdf,
    session: Mutex<Option<Session>>,
}

type Shared = Arc<App>;
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

/// Clé du compte `index` dérivée de la graine : celle du portefeuille mobile.
fn derive_account(seed: &[u8; 64], index: u32) -> KeyPair {
    vinx_mobile::derive_account(seed, index)
}

fn address_of(seed: &[u8; 64], index: u32) -> Address {
    Address::from_public_key(&derive_account(seed, index).public_key())
}

/// Graine d'une phrase de 12 mots ; refuse une phrase mal recopiée.
fn seed_of(words: &str) -> Result<[u8; 64]> {
    vinx_mobile::seed_of(words).map_err(|e| anyhow!(e))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    v.pointer(path).unwrap_or(&Value::Null)
}

fn units(v: &Value) -> Result<Amount> {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("montant invalide dans la réponse du nœud"))
}

impl App {
    fn vault_path(&self) -> PathBuf {
        self.dir.join(VAULT)
    }

    fn vault(&self) -> Result<Option<Vault>> {
        let path = self.vault_path();
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("lecture de {}", path.display()))?;
        Ok(Some(
            serde_json::from_str(&text).with_context(|| format!("{} invalide", path.display()))?,
        ))
    }

    fn existing_vault(&self) -> Result<Vault> {
        self.vault()?
            .ok_or_else(|| anyhow!("aucun portefeuille : crée-le ou restaure-le d'abord"))
    }

    /// Écrit le portefeuille via un fichier temporaire : jamais à moitié écrit.
    fn save(&self, vault: &Vault) -> Result<()> {
        let path = self.vault_path();
        let tmp = self.dir.join(format!("{VAULT}.tmp"));
        let _ = std::fs::remove_file(&tmp);
        vinx_keystore::keyfile::write_private(
            &tmp,
            serde_json::to_string_pretty(vault)?.as_bytes(),
        )
        .with_context(|| format!("écriture de {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("écriture de {}", path.display()))
    }

    fn lock(&self) {
        if let Ok(mut s) = self.session.lock() {
            *s = None;
        }
    }

    fn unlocked(&self) -> bool {
        self.session
            .lock()
            .ok()
            .and_then(|s| s.as_ref().map(|s| s.last_used.elapsed() < LOCK_AFTER))
            .unwrap_or(false)
    }

    /// Graine de la session ouverte, et prolonge celle-ci.
    fn seed(&self) -> Result<[u8; 64]> {
        let mut guard = self
            .session
            .lock()
            .map_err(|_| anyhow!("session indisponible"))?;
        match guard.as_mut() {
            Some(s) if s.last_used.elapsed() < LOCK_AFTER => {
                s.last_used = Instant::now();
                Ok(s.seed)
            }
            _ => {
                *guard = None;
                bail!("portefeuille verrouillé : saisis ton mot de passe")
            }
        }
    }

    fn open_session(&self, seed: [u8; 64]) {
        if let Ok(mut s) = self.session.lock() {
            *s = Some(Session {
                seed,
                last_used: Instant::now(),
            });
        }
    }

    fn client(&self) -> Result<Client> {
        Ok(Client::new(self.existing_vault()?.node))
    }

    fn account(&self, index: u32) -> Result<AccountEntry> {
        self.existing_vault()?
            .accounts
            .into_iter()
            .find(|a| a.index == index)
            .ok_or_else(|| anyhow!("compte inconnu"))
    }
}

/// Crée (ou restaure) le portefeuille avec la phrase `words`.
fn install(app: &App, words: &str, password: &str, node: &str) -> Result<()> {
    if app.vault()?.is_some() {
        bail!("un portefeuille existe déjà dans ce dossier");
    }
    if password.chars().count() < 8 {
        bail!("mot de passe trop court : au moins 8 caractères");
    }
    let seed = seed_of(words)?;
    let vault = Vault {
        version: 1,
        node: node.trim().to_owned(),
        phrase: Sealed::seal(words.as_bytes(), password, app.kdf)?,
        accounts: vec![AccountEntry {
            index: 0,
            label: "Compte principal".into(),
            address: address_of(&seed, 0).to_string(),
        }],
        contacts: Vec::new(),
    };
    app.save(&vault)?;
    app.open_session(seed);
    Ok(())
}

/// Destinataire saisi : adresse, contact ou nom `.vinx`. Renvoie l'adresse et
/// la façon dont elle a été trouvée.
fn resolve(app: &App, client: &Client, input: &str) -> Result<(Address, String)> {
    let input = input.trim();
    if input.is_empty() {
        bail!("destinataire manquant");
    }
    if let Ok(a) = input.parse::<Address>() {
        let vault = app.existing_vault()?;
        let label = vault
            .contacts
            .iter()
            .find(|c| c.address == a.to_string())
            .map_or_else(
                || "adresse".to_owned(),
                |c| format!("contact « {} »", c.name),
            );
        return Ok((a, label));
    }
    let vault = app.existing_vault()?;
    if let Some(c) = vault
        .contacts
        .iter()
        .find(|c| c.name.to_lowercase() == input.to_lowercase())
    {
        let a = c
            .address
            .parse()
            .map_err(|e| anyhow!("contact {} : {e}", c.name))?;
        return Ok((a, format!("contact « {} »", c.name)));
    }
    let lower = input.to_lowercase();
    let name = lower.strip_suffix(".vinx").unwrap_or(&lower);
    if !vinx_core_name_ok(name) {
        bail!("« {input} » n'est ni une adresse vinx1…, ni un contact, ni un nom .vinx");
    }
    let v = client
        .get_json(&format!("/names/{name}"))?
        .ok_or_else(|| anyhow!("ce nœud ne connaît pas encore les noms .vinx"))?;
    let owner = at(&v, "/owner")
        .as_str()
        .ok_or_else(|| anyhow!("le nom {name}.vinx n'est attribué à personne"))?;
    Ok((owner.parse()?, format!("nom {name}.vinx")))
}

/// Mêmes règles que la chaîne pour un nom.
fn vinx_core_name_ok(name: &str) -> bool {
    let ok_len = (3..=32).contains(&name.len());
    ok_len
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

#[derive(Deserialize)]
struct Operation {
    account: u32,
    action: String,
    #[serde(default)]
    to: String,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    memo: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    arbiter: String,
    #[serde(default)]
    days: u64,
    #[serde(default)]
    deal: String,
    /// Horodatage : empreinte SHA-256 du fichier, en hexadécimal.
    #[serde(default)]
    hash: String,
    /// Paiement groupé : une ligne « destinataire ; montant ; mémo » par paiement.
    #[serde(default)]
    lines: String,
    /// Actifs natifs (protocole 5) : symbole, et pour la création ses réglages.
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    decimals: u8,
    #[serde(default)]
    max_supply: String,
    #[serde(default)]
    mint: bool,
    #[serde(default)]
    freeze: bool,
    #[serde(default)]
    clawback: bool,
}

/// Pouvoirs d'un émetteur, en clair.
fn powers_text(mint: bool, freeze: bool, clawback: bool) -> String {
    let list: Vec<&str> = [
        (mint, "émettre de nouvelles unités"),
        (freeze, "geler des comptes ou tout le jeton"),
        (clawback, "reprendre des unités sur un compte"),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, t)| *t)
    .collect();
    if list.is_empty() {
        "aucun : supply fixe, l'émetteur ne peut plus rien changer".into()
    } else {
        list.join(", ")
    }
}

/// Fiche d'un actif sur le nœud, et ses décimales.
fn asset_info(client: &Client, symbol: &str) -> Result<(Value, usize)> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol.is_empty() {
        bail!("indique le symbole du jeton");
    }
    let v = client
        .get_json(&format!("/assets/{symbol}"))?
        .ok_or_else(|| anyhow!("aucun jeton {symbol} sur ce réseau"))?;
    let decimals = at(&v, "/decimals")
        .as_u64()
        .and_then(|d| usize::try_from(d).ok())
        .unwrap_or(0);
    Ok((v, decimals))
}

/// Ligne de `holder` pour l'actif `symbol` dans `/accounts/{a}` : `None` si
/// l'actif n'est pas accepté.
fn holding(account: &Value, symbol: &str) -> Option<Value> {
    at(account, "/assets")
        .as_array()?
        .iter()
        .find(|h| at(h, "/symbol").as_str() == Some(symbol))
        .cloned()
}

/// Opération prête à signer, avec ce qu'il faut montrer avant.
struct Plan {
    body: TxBody,
    sender: Address,
    fee: Amount,
    lines: Vec<(String, String)>,
    warnings: Vec<String>,
    /// Total débité du compte, frais compris.
    total: Amount,
    balance: Amount,
}

fn memo_bytes(memo: &str) -> Option<Vec<u8>> {
    let m = memo.trim();
    (!m.is_empty()).then(|| m.as_bytes().to_vec())
}

fn plan(app: &App, op: &Operation) -> Result<Plan> {
    let seed = app.seed()?;
    let me = address_of(&seed, op.account);
    app.account(op.account)?;
    let client = app.client()?;
    let status = client.status_json()?;
    let min_reserve = units(at(&status, "/params/min_reserve"))?;
    let max_memo = at(&status, "/params/max_memo_bytes").as_u64().unwrap_or(32);
    let protocol = at(&status, "/protocol_version").as_u64().unwrap_or(1);
    let mut fee = client.recommended_fee()?;
    let Some(mine) = client.account(&me)? else {
        bail!("ce compte est vide : il doit d'abord recevoir des VINX (onglet Recevoir)");
    };
    if let Some(m) = memo_bytes(&op.memo) {
        if u64::try_from(m.len()).map_or(true, |l| l > max_memo) {
            bail!("mémo trop long : {max_memo} octets au plus");
        }
    }
    let need_v2 = || -> Result<()> {
        if protocol < 2 {
            bail!(
                "fonction pas encore active sur ce réseau : ses administrateurs doivent \
                 d'abord programmer la mise à jour du protocole 2"
            );
        }
        Ok(())
    };
    let mut lines = Vec::new();
    let mut warnings = Vec::new();
    let mut spent: Amount = 0;
    let body = match op.action.as_str() {
        "send" | "open-deal" => {
            let amount = parse_vinx(&op.amount)?;
            if amount == 0 {
                bail!("montant nul");
            }
            let (to, how) = resolve(app, &client, &op.to)?;
            if to == me {
                bail!("c'est l'adresse de ce compte");
            }
            lines.push(("Destinataire".into(), to.to_string()));
            lines.push(("Trouvé par".into(), how.clone()));
            lines.push(("Montant".into(), format!("{} VINX", format_vinx(amount))));
            if how == "adresse" {
                warnings.push(
                    "Cette adresse n'est pas dans tes contacts : vérifie-la caractère par caractère."
                        .into(),
                );
            }
            match client.account(&to)? {
                None if amount < min_reserve => bail!(
                    "ce destinataire n'a encore jamais reçu de VINX : le premier versement doit \
                     être d'au moins {} VINX",
                    format_vinx(min_reserve)
                ),
                None => warnings
                    .push("Ce destinataire n'a encore jamais reçu de VINX sur ce réseau.".into()),
                Some(r) if r.memo_required && memo_bytes(&op.memo).is_none() => {
                    bail!("ce destinataire exige un mémo (référence de facture, de client…)")
                }
                Some(_) => {}
            }
            spent = amount;
            if op.action == "send" {
                TxBody::Transfer {
                    to,
                    amount,
                    memo: memo_bytes(&op.memo),
                }
            } else {
                need_v2()?;
                if op.days == 0 || op.days > MAX_DEAL_DAYS {
                    bail!("échéance : entre 1 et {MAX_DEAL_DAYS} jours");
                }
                let arbiter = if op.arbiter.trim().is_empty() {
                    None
                } else {
                    let (a, how) = resolve(app, &client, &op.arbiter)?;
                    if a == me || a == to {
                        bail!("l'arbitre doit être une troisième personne");
                    }
                    lines.push(("Arbitre".into(), format!("{a} ({how})")));
                    Some(a)
                };
                let deadline = now_secs()
                    .checked_add(op.days.saturating_mul(86_400))
                    .ok_or_else(|| anyhow!("échéance invalide"))?;
                lines.push(("Échéance".into(), format!("dans {} jour(s)", op.days)));
                warnings.push(
                    "Les fonds restent bloqués jusqu'à ce que tu les verses au vendeur, \
                     qu'il te rembourse, que l'arbitre tranche, ou jusqu'à l'échéance."
                        .into(),
                );
                TxBody::OpenDeal {
                    seller: to,
                    arbiter,
                    amount,
                    deadline,
                    memo: memo_bytes(&op.memo),
                }
            }
        }
        "register-name" => {
            need_v2()?;
            let name = op.name.trim().to_lowercase();
            let name = name.strip_suffix(".vinx").unwrap_or(&name).to_owned();
            if !vinx_core_name_ok(&name) {
                bail!(
                    "nom invalide : 3 à 32 caractères a-z, 0-9 et tiret, sans tiret au début ni à la fin"
                );
            }
            let v = client
                .get_json(&format!("/names/{name}"))?
                .ok_or_else(|| anyhow!("ce nœud ne connaît pas les noms .vinx"))?;
            let owner = at(&v, "/owner").as_str();
            let mine_already = owner == Some(me.to_string().as_str());
            if !mine_already && at(&v, "/available") != &Value::Bool(true) {
                bail!("{name}.vinx est déjà pris");
            }
            let price = units(at(&v, "/fee"))?;
            spent = price;
            lines.push(("Nom".into(), format!("{name}.vinx")));
            lines.push((
                "Opération".into(),
                if mine_already {
                    "renouvellement d'un an"
                } else {
                    "enregistrement pour un an"
                }
                .into(),
            ));
            lines.push(("Prix".into(), format!("{} VINX", format_vinx(price))));
            TxBody::RegisterName { name }
        }
        "transfer-name" => {
            need_v2()?;
            let name = op.name.trim().to_lowercase();
            let name = name.strip_suffix(".vinx").unwrap_or(&name).to_owned();
            let (to, how) = resolve(app, &client, &op.to)?;
            lines.push(("Nom".into(), format!("{name}.vinx")));
            lines.push(("Nouveau titulaire".into(), format!("{to} ({how})")));
            warnings.push("Une fois cédé, le nom ne t'appartient plus.".into());
            TxBody::TransferName { name, to }
        }
        "release-deal" | "refund-deal" => {
            need_v2()?;
            let id_hex = op.deal.trim();
            let d = client
                .get_json(&format!("/deals/{id_hex}"))?
                .ok_or_else(|| anyhow!("séquestre inconnu ou déjà clos"))?;
            let id: [u8; 32] = hex::decode(id_hex)
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| anyhow!("identifiant de séquestre invalide"))?;
            // Séquestre en actif : montant en unités de l'actif.
            let shown = match at(&d, "/asset").as_str() {
                Some(symbol) => {
                    let (_, dec) = asset_info(&client, symbol)?;
                    format!("{} {symbol}", format_units(units(at(&d, "/units"))?, dec))
                }
                None => format!("{} VINX", format_vinx(units(at(&d, "/amount"))?)),
            };
            let me_s = me.to_string();
            let is = |k: &str| at(&d, k).as_str() == Some(me_s.as_str());
            let release = op.action == "release-deal";
            let allowed = if release {
                is("/buyer") || is("/arbiter")
            } else {
                is("/seller")
                    || is("/arbiter")
                    || (is("/buyer") && at(&d, "/expired") == &Value::Bool(true))
            };
            if !allowed {
                bail!(if release {
                    "seuls l'acheteur ou l'arbitre peuvent verser ce séquestre"
                } else {
                    "seuls le vendeur ou l'arbitre peuvent rembourser, ou l'acheteur après l'échéance"
                });
            }
            let who = if release { "/seller" } else { "/buyer" };
            lines.push((
                if release {
                    "Versé à"
                } else {
                    "Remboursé à"
                }
                .into(),
                at(&d, who).as_str().unwrap_or_default().to_owned(),
            ));
            lines.push(("Montant".into(), shown));
            if release {
                TxBody::ReleaseDeal { id }
            } else {
                TxBody::RefundDeal { id }
            }
        }
        "timestamp" => {
            let digest: [u8; 32] = hex::decode(op.hash.trim())
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| {
                    anyhow!("empreinte invalide : 64 caractères hexadécimaux attendus")
                })?;
            if u64::try_from(digest.len()).map_or(true, |l| l > max_memo) {
                bail!("le réseau limite le mémo à {max_memo} octets : horodatage impossible");
            }
            if !op.name.trim().is_empty() {
                lines.push(("Fichier".into(), op.name.trim().to_owned()));
            }
            lines.push(("Empreinte SHA-256".into(), hex::encode(digest)));
            warnings.push(
                "Seule l'empreinte part sur la chaîne : le fichier reste sur ton ordinateur. \
                 Garde-le intact, la moindre modification change l'empreinte."
                    .into(),
            );
            if protocol >= 4 {
                TxBody::Attest { digest }
            } else {
                spent = TIMESTAMP_UNITS;
                TxBody::Transfer {
                    to: Address::treasury(),
                    amount: TIMESTAMP_UNITS,
                    memo: Some(digest.to_vec()),
                }
            }
        }
        "batch" => {
            if protocol < 4 {
                bail!(
                    "paiement groupé pas encore actif sur ce réseau : il arrive avec le protocole 4"
                );
            }
            let mut payments = Vec::new();
            let mut new_accounts = std::collections::BTreeSet::new();
            let mut unknown = std::collections::BTreeSet::new();
            for (n, raw) in op.lines.lines().enumerate() {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let row = n.saturating_add(1);
                let mut cols = line.split([';', '\t']).map(str::trim);
                let who = cols.next().unwrap_or_default();
                let amount = parse_vinx(&cols.next().unwrap_or_default().replace(',', "."))
                    .map_err(|e| anyhow!("ligne {row} : {e}"))?;
                if amount == 0 {
                    bail!("ligne {row} : montant nul");
                }
                let memo = cols.next().and_then(memo_bytes);
                if let Some(m) = &memo {
                    if u64::try_from(m.len()).map_or(true, |l| l > max_memo) {
                        bail!("ligne {row} : mémo trop long ({max_memo} octets au plus)");
                    }
                }
                let (to, how) =
                    resolve(app, &client, who).map_err(|e| anyhow!("ligne {row} : {e}"))?;
                if to == me {
                    bail!("ligne {row} : c'est l'adresse de ce compte");
                }
                if how == "adresse" {
                    unknown.insert(to);
                }
                match client.account(&to)? {
                    None if amount < min_reserve => bail!(
                        "ligne {row} : {who} n'a encore jamais reçu de VINX, le premier \
                         versement doit être d'au moins {} VINX",
                        format_vinx(min_reserve)
                    ),
                    None => {
                        new_accounts.insert(to);
                    }
                    Some(r) if r.memo_required && memo.is_none() => {
                        bail!("ligne {row} : {who} exige un mémo")
                    }
                    Some(_) => {}
                }
                spent = spent
                    .checked_add(amount)
                    .ok_or_else(|| anyhow!("total trop grand"))?;
                payments.push(vinx_core::Payment { to, amount, memo });
            }
            if payments.is_empty() {
                bail!("aucun paiement : une ligne « destinataire ; montant ; mémo » par paiement");
            }
            if payments.len() > vinx_core::MAX_BATCH_PAYMENTS {
                bail!(
                    "{} paiements : {} au plus par envoi groupé",
                    payments.len(),
                    vinx_core::MAX_BATCH_PAYMENTS
                );
            }
            let body = TxBody::BatchTransfer { payments };
            fee = body.min_fee(fee);
            if let TxBody::BatchTransfer { payments } = &body {
                lines.push(("Paiements".into(), payments.len().to_string()));
                for p in payments.iter().take(5) {
                    lines.push((p.to.to_string(), format!("{} VINX", format_vinx(p.amount))));
                }
                if payments.len() > 5 {
                    lines.push((
                        "…".into(),
                        format!("et {} autres", payments.len().saturating_sub(5)),
                    ));
                }
            }
            lines.push(("Total versé".into(), format!("{} VINX", format_vinx(spent))));
            if !unknown.is_empty() {
                warnings.push(format!(
                    "{} destinataire(s) donné(s) par une adresse hors de tes contacts : vérifie-les.",
                    unknown.len()
                ));
            }
            if !new_accounts.is_empty() {
                warnings.push(format!(
                    "{} destinataire(s) n'ont encore jamais reçu de VINX sur ce réseau.",
                    new_accounts.len()
                ));
            }
            body
        }
        "grant-mandate" | "revoke-mandate" | "collect" => {
            if protocol < 4 {
                bail!("mandats pas encore actifs sur ce réseau : ils arrivent avec le protocole 4");
            }
            let (other, how) = resolve(app, &client, &op.to)?;
            if other == me {
                bail!("c'est l'adresse de ce compte");
            }
            match op.action.as_str() {
                "grant-mandate" => {
                    let max = parse_vinx(&op.amount)?;
                    if max == 0 {
                        bail!("montant maximal nul");
                    }
                    if op.days == 0 || op.days > 366 {
                        bail!("période : entre 1 et 366 jours");
                    }
                    lines.push(("Bénéficiaire".into(), format!("{other} ({how})")));
                    lines.push((
                        "Autorisation".into(),
                        format!(
                            "jusqu'à {} VINX tous les {} jour(s)",
                            format_vinx(max),
                            op.days
                        ),
                    ));
                    warnings.push(
                        "Ce bénéficiaire pourra prélever sans te redemander ton accord, dans cette \
                         limite, jusqu'à ce que tu révoques le mandat (onglet Mandats). Un nouveau \
                         mandat pour le même bénéficiaire remplace l'ancien."
                            .into(),
                    );
                    TxBody::GrantMandate {
                        payee: other,
                        max_per_period: max,
                        period: op.days.saturating_mul(86_400),
                        expires_at: None,
                    }
                }
                "revoke-mandate" => {
                    lines.push(("Mandat révoqué".into(), format!("{other} ({how})")));
                    TxBody::RevokeMandate { payee: other }
                }
                _ => {
                    let amount = parse_vinx(&op.amount)?;
                    if amount == 0 {
                        bail!("montant nul");
                    }
                    let payer = client
                        .get_json(&format!("/accounts/{other}"))?
                        .ok_or_else(|| anyhow!("ce payeur n'existe pas"))?;
                    let me_s = me.to_string();
                    let mandate = at(&payer, "/mandates/granted")
                        .as_array()
                        .and_then(|l| {
                            l.iter()
                                .find(|m| at(m, "/payee").as_str() == Some(me_s.as_str()))
                        })
                        .ok_or_else(|| anyhow!("ce payeur ne t'a accordé aucun mandat"))?;
                    let available = units(at(mandate, "/available"))?;
                    if at(mandate, "/expired") == &Value::Bool(true) {
                        bail!("ce mandat a expiré");
                    }
                    if amount > available {
                        bail!(
                            "le mandat permet encore {} VINX sur cette période",
                            format_vinx(available)
                        );
                    }
                    lines.push(("Payeur".into(), format!("{other} ({how})")));
                    lines.push((
                        "Prélèvement".into(),
                        format!("{} VINX", format_vinx(amount)),
                    ));
                    lines.push((
                        "Reste sur la période".into(),
                        format!("{} VINX", format_vinx(available.saturating_sub(amount))),
                    ));
                    TxBody::Collect {
                        payer: other,
                        amount,
                        memo: memo_bytes(&op.memo),
                    }
                }
            }
        }
        action if action.ends_with("-asset") => {
            if protocol < 5 {
                bail!("jetons pas encore actifs sur ce réseau : ils arrivent avec le protocole 5");
            }
            let me_json = client
                .get_json(&format!("/accounts/{me}"))?
                .unwrap_or(Value::Null);
            let symbol = op.symbol.trim().to_ascii_uppercase();
            if action == "create-asset" {
                let name = symbol.to_lowercase();
                let owner = client
                    .get_json(&format!("/names/{name}"))?
                    .and_then(|n| at(&n, "/owner").as_str().map(str::to_owned));
                if owner.as_deref() != Some(me.to_string().as_str()) {
                    bail!(
                        "le symbole {symbol} exige de posséder le nom {name}.vinx sur ce compte                          (onglet Noms .vinx)"
                    );
                }
                if client.get_json(&format!("/assets/{symbol}"))?.is_some() {
                    bail!("le symbole {symbol} est déjà utilisé par un jeton");
                }
                let title = op.name.trim();
                if title.is_empty() || title.len() > 64 {
                    bail!("nom affiché : 1 à 64 caractères");
                }
                if op.decimals > 9 {
                    bail!("décimales : de 0 à 9");
                }
                let d = usize::from(op.decimals);
                let supply = if op.amount.trim().is_empty() {
                    0
                } else {
                    parse_units(&op.amount, d)?
                };
                let max = if op.max_supply.trim().is_empty() {
                    None
                } else {
                    Some(parse_units(&op.max_supply, d)?)
                };
                if max.is_some_and(|m| m == 0 || supply > m) {
                    bail!("la supply initiale dépasse le plafond");
                }
                if supply == 0 && !op.mint {
                    bail!("sans supply initiale, il faut garder le pouvoir d'émettre");
                }
                let fee = at(&status, "/params/asset_fee")
                    .as_str()
                    .and_then(|f| f.parse().ok())
                    .unwrap_or(0);
                spent = fee;
                lines.push(("Jeton".into(), format!("{symbol} — {title}")));
                lines.push(("Décimales".into(), op.decimals.to_string()));
                lines.push((
                    "Supply initiale".into(),
                    format!("{} {symbol}", format_units(supply, d)),
                ));
                lines.push((
                    "Plafond".into(),
                    max.map_or("aucun".into(), |m| {
                        format!("{} {symbol}", format_units(m, d))
                    }),
                ));
                lines.push((
                    "Pouvoirs de l'émetteur".into(),
                    powers_text(op.mint, op.freeze, op.clawback),
                ));
                lines.push((
                    "Prix de création".into(),
                    format!("{} VINX", format_vinx(fee)),
                ));
                warnings.push(
                    "Ces pouvoirs sont affichés à tous les détenteurs. Tu pourras les abandonner \
                     plus tard, jamais en ajouter."
                        .into(),
                );
                TxBody::CreateAsset {
                    symbol,
                    name: title.to_owned(),
                    decimals: op.decimals,
                    initial_supply: supply,
                    max_supply: max,
                    powers: vinx_core::AssetPowers {
                        mint: op.mint,
                        freeze: op.freeze,
                        clawback: op.clawback,
                    },
                }
            } else {
                let (asset, d) = asset_info(&client, &symbol)?;
                let issuer = at(&asset, "/issuer").as_str() == Some(me.to_string().as_str());
                let power = |p: &str| at(&asset, &format!("/powers/{p}")) == &Value::Bool(true);
                let title = at(&asset, "/name").as_str().unwrap_or("").to_owned();
                lines.push(("Jeton".into(), format!("{symbol} — {title}")));
                let mine = holding(&me_json, &symbol);
                let my_units: Amount = mine
                    .as_ref()
                    .and_then(|h| at(h, "/units").as_str()?.parse().ok())
                    .unwrap_or(0);
                let amount = || -> Result<Amount> {
                    let a = parse_units(&op.amount, d)?;
                    if a == 0 {
                        bail!("montant nul");
                    }
                    Ok(a)
                };
                let only_issuer = |what: &str| -> Result<()> {
                    if !issuer {
                        bail!("seul l'émetteur du jeton peut {what}");
                    }
                    Ok(())
                };
                match action {
                    "accept-asset" => {
                        if mine.is_some() {
                            bail!("ce compte a déjà accepté {symbol}");
                        }
                        lines.push((
                            "Émetteur".into(),
                            at(&asset, "/issuer").as_str().unwrap_or("").to_owned(),
                        ));
                        lines.push((
                            "Pouvoirs de l'émetteur".into(),
                            powers_text(power("mint"), power("freeze"), power("clawback")),
                        ));
                        if power("clawback") || power("freeze") {
                            warnings.push(
                                "L'émetteur de ce jeton peut geler ton solde ou le reprendre : \
                                 n'accepte que des jetons d'émetteurs en qui tu as confiance."
                                    .into(),
                            );
                        }
                        let per_object = at(&status, "/params/object_reserve")
                            .as_str()
                            .and_then(|v| v.parse::<Amount>().ok())
                            .filter(|p| protocol >= 6 && *p > 0);
                        if let Some(per) = per_object {
                            lines.push((
                                "Réserve pour ce jeton".into(),
                                format!("{} VINX, rendus si tu retires le jeton", format_vinx(per)),
                            ));
                        }
                        TxBody::AcceptAsset {
                            symbol,
                            accept: true,
                        }
                    }
                    "leave-asset" => {
                        if mine.is_none() {
                            bail!("ce compte n'a pas accepté {symbol}");
                        }
                        if my_units > 0 {
                            bail!(
                                "il reste des {symbol} sur ce compte : envoie-les ou détruis-les d'abord"
                            );
                        }
                        lines.push((
                            "Effet".into(),
                            "le compte ne pourra plus recevoir ce jeton".into(),
                        ));
                        TxBody::AcceptAsset {
                            symbol,
                            accept: false,
                        }
                    }
                    "send-asset" | "burn-asset" => {
                        let amount = amount()?;
                        if mine
                            .as_ref()
                            .is_some_and(|h| at(h, "/frozen") == &Value::Bool(true))
                        {
                            bail!("{symbol} est gelé sur ce compte");
                        }
                        if amount > my_units {
                            bail!(
                                "solde insuffisant : {} {symbol} disponibles",
                                format_units(my_units, d)
                            );
                        }
                        let shown = format!("{} {symbol}", format_units(amount, d));
                        if action == "burn-asset" {
                            lines.push(("Détruit".into(), shown));
                            warnings
                                .push("Les unités détruites disparaissent définitivement.".into());
                            TxBody::BurnAsset { symbol, amount }
                        } else {
                            let (to, how) = resolve(app, &client, &op.to)?;
                            if to == me {
                                bail!("c'est l'adresse de ce compte");
                            }
                            let theirs = client
                                .get_json(&format!("/accounts/{to}"))?
                                .unwrap_or(Value::Null);
                            if holding(&theirs, &symbol).is_none() {
                                bail!(
                                    "ce destinataire n'a pas encore accepté {symbol} : il doit \
                                     l'accepter dans son onglet Jetons"
                                );
                            }
                            if at(&theirs, "/memo_required") == &Value::Bool(true)
                                && memo_bytes(&op.memo).is_none()
                            {
                                bail!(
                                    "ce destinataire exige un mémo (référence de facture, de client…)"
                                );
                            }
                            lines.push(("Destinataire".into(), to.to_string()));
                            lines.push(("Trouvé par".into(), how.clone()));
                            lines.push(("Montant".into(), shown));
                            if how == "adresse" {
                                warnings.push(
                                    "Cette adresse n'est pas dans tes contacts : vérifie-la \
                                     caractère par caractère."
                                        .into(),
                                );
                            }
                            TxBody::TransferAsset {
                                symbol,
                                to,
                                amount,
                                memo: memo_bytes(&op.memo),
                            }
                        }
                    }
                    "mint-asset" => {
                        only_issuer("émettre de nouvelles unités")?;
                        if !power("mint") {
                            bail!("l'émetteur a renoncé au pouvoir d'émettre ce jeton");
                        }
                        let amount = amount()?;
                        let to = if op.to.trim().is_empty() {
                            me
                        } else {
                            resolve(app, &client, &op.to)?.0
                        };
                        let theirs = if to == me {
                            me_json.clone()
                        } else {
                            client
                                .get_json(&format!("/accounts/{to}"))?
                                .unwrap_or(Value::Null)
                        };
                        if holding(&theirs, &symbol).is_none() {
                            bail!(
                                "ce bénéficiaire n'a pas encore accepté {symbol} : il doit \
                                 l'accepter avant de recevoir des unités"
                            );
                        }
                        let supply: Amount = at(&asset, "/supply_units")
                            .as_str()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        let max: Option<Amount> = at(&asset, "/max_supply_units")
                            .as_str()
                            .and_then(|v| v.parse().ok());
                        if let Some(m) = max {
                            if supply.saturating_add(amount) > m {
                                bail!(
                                    "au-delà du plafond : {} {symbol} peuvent encore être émis",
                                    format_units(m.saturating_sub(supply), d)
                                );
                            }
                        }
                        lines.push(("Bénéficiaire".into(), to.to_string()));
                        lines.push((
                            "Émis".into(),
                            format!("{} {symbol}", format_units(amount, d)),
                        ));
                        TxBody::MintAsset { symbol, to, amount }
                    }
                    "freeze-asset" | "unfreeze-asset" => {
                        only_issuer("geler ou dégeler")?;
                        let frozen = action == "freeze-asset";
                        if frozen && !power("freeze") {
                            bail!("l'émetteur a renoncé au pouvoir de geler ce jeton");
                        }
                        let account = if op.to.trim().is_empty() {
                            lines.push((
                                "Portée".into(),
                                format!("tout le jeton : plus aucun {symbol} ne circule"),
                            ));
                            None
                        } else {
                            let (a, how) = resolve(app, &client, &op.to)?;
                            lines.push(("Compte".into(), format!("{a} ({how})")));
                            Some(a)
                        };
                        lines.push(("Effet".into(), if frozen { "gel" } else { "dégel" }.into()));
                        TxBody::FreezeAsset {
                            symbol,
                            account,
                            frozen,
                        }
                    }
                    "clawback-asset" => {
                        only_issuer("reprendre des unités")?;
                        if !power("clawback") {
                            bail!("l'émetteur a renoncé à la reprise forcée sur ce jeton");
                        }
                        let amount = amount()?;
                        let (from, how) = resolve(app, &client, &op.to)?;
                        lines.push(("Repris sur".into(), format!("{from} ({how})")));
                        lines.push((
                            "Montant".into(),
                            format!("{} {symbol}", format_units(amount, d)),
                        ));
                        TxBody::ClawbackAsset {
                            symbol,
                            from,
                            amount,
                        }
                    }
                    "deal-asset" => {
                        let amount = amount()?;
                        if amount > my_units {
                            bail!(
                                "solde insuffisant : {} {symbol} disponibles",
                                format_units(my_units, d)
                            );
                        }
                        let (seller, how) = resolve(app, &client, &op.to)?;
                        if seller == me {
                            bail!("c'est l'adresse de ce compte");
                        }
                        let theirs = client
                            .get_json(&format!("/accounts/{seller}"))?
                            .unwrap_or(Value::Null);
                        if holding(&theirs, &symbol).is_none() {
                            bail!(
                                "ce vendeur n'a pas accepté {symbol} : il ne peut pas être payé dans ce jeton"
                            );
                        }
                        if op.days == 0 || op.days > MAX_DEAL_DAYS {
                            bail!("échéance : entre 1 et {MAX_DEAL_DAYS} jours");
                        }
                        let arbiter = if op.arbiter.trim().is_empty() {
                            None
                        } else {
                            let (a, how) = resolve(app, &client, &op.arbiter)?;
                            if a == me || a == seller {
                                bail!("l'arbitre doit être une troisième personne");
                            }
                            lines.push(("Arbitre".into(), format!("{a} ({how})")));
                            Some(a)
                        };
                        lines.push(("Vendeur".into(), format!("{seller} ({how})")));
                        lines.push((
                            "Montant bloqué".into(),
                            format!("{} {symbol}", format_units(amount, d)),
                        ));
                        lines.push(("Échéance".into(), format!("dans {} jour(s)", op.days)));
                        warnings.push(
                            "Les jetons restent bloqués jusqu'à ce que tu les verses au vendeur, \
                             qu'il te rembourse, que l'arbitre tranche, ou jusqu'à l'échéance."
                                .into(),
                        );
                        TxBody::OpenAssetDeal {
                            symbol,
                            seller,
                            arbiter,
                            amount,
                            deadline: now_secs().saturating_add(op.days.saturating_mul(86_400)),
                            memo: memo_bytes(&op.memo),
                        }
                    }
                    "mandate-asset" => {
                        let max = amount()?;
                        if op.days == 0 || op.days > 366 {
                            bail!("période : entre 1 et 366 jours");
                        }
                        let (payee, how) = resolve(app, &client, &op.to)?;
                        if payee == me {
                            bail!("c'est l'adresse de ce compte");
                        }
                        lines.push(("Bénéficiaire".into(), format!("{payee} ({how})")));
                        lines.push((
                            "Autorisation".into(),
                            format!(
                                "jusqu'à {} {symbol} tous les {} jour(s)",
                                format_units(max, d),
                                op.days
                            ),
                        ));
                        warnings.push(
                            "Ce bénéficiaire pourra prélever sans te redemander ton accord, dans \
                             cette limite, jusqu'à ce que tu révoques le mandat."
                                .into(),
                        );
                        TxBody::GrantAssetMandate {
                            symbol,
                            payee,
                            max_per_period: max,
                            period: op.days.saturating_mul(86_400),
                            expires_at: None,
                        }
                    }
                    "unmandate-asset" => {
                        let (payee, how) = resolve(app, &client, &op.to)?;
                        lines.push(("Mandat révoqué".into(), format!("{payee} ({how})")));
                        TxBody::RevokeAssetMandate { symbol, payee }
                    }
                    "collect-asset" => {
                        let amount = amount()?;
                        let (payer, how) = resolve(app, &client, &op.to)?;
                        if mine.is_none() {
                            bail!(
                                "accepte d'abord {symbol} sur ce compte pour pouvoir le prélever"
                            );
                        }
                        lines.push(("Payeur".into(), format!("{payer} ({how})")));
                        lines.push((
                            "Prélèvement".into(),
                            format!("{} {symbol}", format_units(amount, d)),
                        ));
                        TxBody::CollectAsset {
                            symbol,
                            payer,
                            amount,
                            memo: memo_bytes(&op.memo),
                        }
                    }
                    "renounce-asset" => {
                        only_issuer("abandonner ses pouvoirs")?;
                        if !(op.mint || op.freeze || op.clawback) {
                            bail!("choisis au moins un pouvoir à abandonner");
                        }
                        lines.push((
                            "Pouvoirs abandonnés".into(),
                            powers_text(op.mint, op.freeze, op.clawback),
                        ));
                        warnings
                            .push("C'est définitif : aucun pouvoir ne peut être rajouté.".into());
                        TxBody::RenounceAssetPowers {
                            symbol,
                            powers: vinx_core::AssetPowers {
                                mint: op.mint,
                                freeze: op.freeze,
                                clawback: op.clawback,
                            },
                        }
                    }
                    other => bail!("opération inconnue : {other}"),
                }
            }
        }
        other => bail!("opération inconnue : {other}"),
    };
    // Règles du compte (protocole 4) : refuser ici ce que le réseau refuserait.
    let account_json = client
        .get_json(&format!("/accounts/{me}"))?
        .unwrap_or(Value::Null);
    let policy = at(&account_json, "/policy");
    let needs_approval = matches!(
        body,
        TxBody::MintAsset { .. }
            | TxBody::FreezeAsset { .. }
            | TxBody::ClawbackAsset { .. }
            | TxBody::RenounceAssetPowers { .. }
    );
    if !policy.is_null() && needs_approval {
        bail!(
            "ce compte a des règles : émettre, geler, reprendre ou abandonner un pouvoir exige \
             l'approbation de ses signataires (« vinx-wallet account propose »)"
        );
    }
    if !policy.is_null() {
        let threshold = at(policy, "/threshold").as_u64().unwrap_or(0);
        match at(policy, "/daily_limit").as_str() {
            None => bail!(
                "ce compte exige {threshold} approbation(s) de ses signataires : la transaction \
                 se prépare avec « vinx-wallet account propose », puis chaque signataire \
                 l'approuve (guide du portefeuille, comptes à plusieurs signatures)"
            ),
            Some(limit) => {
                let limit: Amount = limit.parse().unwrap_or(0);
                let used = units(at(policy, "/spent_today")).unwrap_or(0);
                let left = limit.saturating_sub(used);
                if spent > left {
                    bail!(
                        "plafond journalier dépassé : il reste {} VINX aujourd'hui sur {} ; au-delà, \
                         il faut l'approbation de la clé coffre (« vinx-wallet account propose »)",
                        format_vinx(left),
                        format_vinx(limit)
                    );
                }
                lines.push((
                    "Plafond du jour".into(),
                    format!(
                        "{} VINX restants après l'opération",
                        format_vinx(left.saturating_sub(spent))
                    ),
                ));
            }
        }
    }
    let total = spent
        .checked_add(fee)
        .ok_or_else(|| anyhow!("montant trop grand"))?;
    let after = mine.balance.checked_sub(total).ok_or_else(|| {
        anyhow!(
            "solde insuffisant : il faut {} VINX frais compris, le compte en a {}",
            format_vinx(total),
            format_vinx(mine.balance)
        )
    })?;
    // Protocole 6 : chaque objet possédé (jeton accepté, mandat, règles)
    // immobilise une réserve en plus de celle du compte.
    let objects = at(&account_json, "/objects").as_u64().unwrap_or(0);
    let per_object: Amount = at(&status, "/params/object_reserve")
        .as_str()
        .and_then(|v| v.parse().ok())
        .filter(|_| protocol >= 6)
        .unwrap_or(0);
    let creates = matches!(
        body,
        TxBody::AcceptAsset { accept: true, .. }
            | TxBody::GrantMandate { .. }
            | TxBody::GrantAssetMandate { .. }
    );
    let count = objects.saturating_add(u64::from(creates && per_object > 0));
    let required = if count == 0 {
        0
    } else {
        per_object.saturating_mul(count).saturating_add(min_reserve)
    };
    if required > 0 && after < required {
        bail!(
            "il doit rester au moins {} VINX sur le compte : réserve du compte et de ses {} \
             objet(s) (jetons acceptés, mandats, règles), à {} VINX chacun",
            format_vinx(required),
            count,
            format_vinx(per_object)
        );
    }
    if after != 0 && after < min_reserve {
        bail!(
            "il doit rester au moins {} VINX sur le compte (réserve du réseau), ou rien du tout",
            format_vinx(min_reserve)
        );
    }
    if required > 0 {
        lines.push((
            "Réserve immobilisée".into(),
            format!("{} VINX ({count} objet(s))", format_vinx(required)),
        ));
    }
    Ok(Plan {
        body,
        sender: me,
        fee,
        lines,
        warnings,
        total,
        balance: mine.balance,
    })
}

fn plan_json(p: &Plan) -> Value {
    json!({
        "lines": p.lines.iter().map(|(k, v)| json!({ "label": k, "value": v })).collect::<Vec<_>>(),
        "warnings": p.warnings,
        "fee": format_vinx(p.fee),
        "total": format_vinx(p.total),
        "balance": format_vinx(p.balance),
        "after": format_vinx(p.balance.saturating_sub(p.total)),
    })
}

/// Prochain nonce et présence de la clé publique, en tenant compte des
/// transactions du compte encore en attente.
fn next_nonce(client: &Client, me: &Address) -> Result<(u64, bool)> {
    client.next_nonce(me)?.ok_or_else(|| anyhow!("compte vide"))
}

fn execute(app: &App, op: &Operation) -> Result<Value> {
    let p = plan(app, op)?;
    let seed = app.seed()?;
    let keypair = derive_account(&seed, op.account);
    let client = app.client()?;
    let chain_id = client.status()?.chain_id;
    let (nonce, has_key) = next_nonce(&client, &p.sender)?;
    let tx = Transaction {
        chain_id,
        sender: p.sender,
        nonce,
        fee: p.fee,
        body: p.body,
    };
    let deal = matches!(tx.body, TxBody::OpenDeal { .. })
        .then(|| hex::encode(vinx_core::deal_id(&tx.sender, tx.nonce)));
    let id = client.submit(&tx.sign(&keypair, !has_key))?;
    Ok(json!({ "id": id, "deal": deal }))
}

fn overview(app: &App, index: u32) -> Result<Value> {
    let entry = app.account(index)?;
    let client = app.client()?;
    let status = client.status_json()?;
    let account = client.get_json(&format!("/accounts/{}", entry.address))?;
    let history = client
        .get_json(&format!(
            "/accounts/{}/transactions?limit=100",
            entry.address
        ))?
        .map(|h| at(&h, "/transactions").clone())
        .unwrap_or(Value::Array(Vec::new()));
    let me = entry.address.to_string();
    // Tous les jetons du réseau : ceux de ce compte (console émetteur) et
    // le catalogue « Découvrir des jetons ».
    let catalog: Vec<Value> = client
        .get_json("/assets")?
        .and_then(|v| at(&v, "/assets").as_array().cloned())
        .unwrap_or_default();
    let issued: Vec<Value> = catalog
        .iter()
        .filter(|a| at(a, "/issuer").as_str() == Some(me.as_str()))
        .cloned()
        .collect();
    Ok(json!({
        "address": entry.address,
        "label": entry.label,
        "exists": account.is_some(),
        "balance": account.as_ref().map_or_else(|| "0".to_owned(), |a| at(a, "/balance").as_str().unwrap_or("0").to_owned()),
        "names": account.as_ref().map_or(Value::Array(Vec::new()), |a| at(a, "/names").clone()),
        "deals": account.as_ref().map_or(Value::Array(Vec::new()), |a| at(a, "/deals").clone()),
        "policy": account.as_ref().map_or(Value::Null, |a| at(a, "/policy").clone()),
        "mandates": account.as_ref().map_or(Value::Null, |a| at(a, "/mandates").clone()),
        "assets": account.as_ref().map_or(Value::Array(Vec::new()), |a| at(a, "/assets").clone()),
        "issued": issued,
        "catalog": catalog,
        "reserve": account.as_ref().map_or(Value::Null, |a| at(a, "/reserve").clone()),
        "spendable": account.as_ref().map_or(Value::Null, |a| at(a, "/spendable").clone()),
        "objects": account.as_ref().map_or(Value::Null, |a| at(a, "/objects").clone()),
        "history": history,
        "network": {
            "chain_id": at(&status, "/chain_id"),
            "height": at(&status, "/height"),
            "protocol_version": at(&status, "/protocol_version"),
            "min_reserve": at(&status, "/params/min_reserve"),
            "name_fee": at(&status, "/params/name_fee"),
            "asset_fee": at(&status, "/params/asset_fee"),
            "object_reserve": at(&status, "/params/object_reserve"),
        },
    }))
}

// ----- Routes -----

async fn page(State(app): State<Shared>) -> Html<String> {
    Html(crate::cockpit::render_page(include_str!("app.html")).replace("__APP_TOKEN__", &app.token))
}

/// Refuse tout appel sans jeton, ou adressé à un autre hôte que 127.0.0.1.
async fn guard(
    State(app): State<Shared>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    let allowed = [
        format!("127.0.0.1:{}", app.port),
        format!("localhost:{}", app.port),
    ];
    if !allowed.iter().any(|a| a == host) {
        return (StatusCode::FORBIDDEN, "hôte refusé").into_response();
    }
    if req.uri().path().starts_with("/api/") {
        let token = headers
            .get("x-app-token")
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        if token != app.token {
            return (StatusCode::FORBIDDEN, "jeton absent ou invalide").into_response();
        }
    }
    next.run(req).await
}

async fn get_state(State(app): State<Shared>) -> ApiResult {
    let vault = app.vault().map_err(fail)?;
    Ok(Json(match vault {
        None => json!({ "exists": false }),
        Some(v) => json!({
            "exists": true,
            "unlocked": app.unlocked(),
            "node": v.node,
            "accounts": v.accounts,
            "contacts": v.contacts,
        }),
    }))
}

async fn new_phrase() -> ApiResult {
    let mut entropy = [0u8; 16];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut entropy);
    let m = bip39::Mnemonic::from_entropy(&entropy).map_err(fail)?;
    Ok(Json(json!({ "words": m.to_string() })))
}

#[derive(Deserialize)]
struct Install {
    words: String,
    password: String,
    node: String,
}

async fn create(State(app): State<Shared>, Json(b): Json<Install>) -> ApiResult {
    blocking(move || {
        install(&app, &b.words, &b.password, &b.node)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[derive(Deserialize)]
struct Password {
    password: String,
}

async fn unlock(State(app): State<Shared>, Json(b): Json<Password>) -> ApiResult {
    blocking(move || {
        let vault = app.existing_vault()?;
        let words = String::from_utf8(vault.phrase.open(&b.password)?)?;
        app.open_session(seed_of(&words)?);
        Ok(json!({ "ok": true }))
    })
    .await
}

async fn lock(State(app): State<Shared>) -> ApiResult {
    app.lock();
    Ok(Json(json!({ "ok": true })))
}

/// Affiche la phrase de récupération, après une nouvelle saisie du mot de passe.
async fn reveal(State(app): State<Shared>, Json(b): Json<Password>) -> ApiResult {
    blocking(move || {
        let vault = app.existing_vault()?;
        let words = String::from_utf8(vault.phrase.open(&b.password)?)?;
        Ok(json!({ "words": words }))
    })
    .await
}

#[derive(Deserialize)]
struct Label {
    label: String,
}

async fn add_account(State(app): State<Shared>, Json(b): Json<Label>) -> ApiResult {
    blocking(move || {
        let seed = app.seed()?;
        let mut vault = app.existing_vault()?;
        let index = vault
            .accounts
            .iter()
            .map(|a| a.index)
            .max()
            .and_then(|i| i.checked_add(1))
            .unwrap_or(0);
        let label = b.label.trim();
        vault.accounts.push(AccountEntry {
            index,
            label: if label.is_empty() {
                format!("Compte {}", index.saturating_add(1))
            } else {
                label.to_owned()
            },
            address: address_of(&seed, index).to_string(),
        });
        app.save(&vault)?;
        Ok(json!({ "index": index }))
    })
    .await
}

#[derive(Deserialize)]
struct Rename {
    index: u32,
    label: String,
}

async fn rename_account(State(app): State<Shared>, Json(b): Json<Rename>) -> ApiResult {
    blocking(move || {
        let mut vault = app.existing_vault()?;
        let entry = vault
            .accounts
            .iter_mut()
            .find(|a| a.index == b.index)
            .ok_or_else(|| anyhow!("compte inconnu"))?;
        let label = b.label.trim();
        if label.is_empty() {
            bail!("libellé vide");
        }
        entry.label = label.to_owned();
        app.save(&vault)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[derive(Deserialize)]
struct ContactsBody {
    contacts: Vec<Contact>,
}

async fn put_contacts(State(app): State<Shared>, Json(b): Json<ContactsBody>) -> ApiResult {
    blocking(move || {
        let mut seen = std::collections::BTreeSet::new();
        for c in &b.contacts {
            if c.name.trim().is_empty() {
                bail!("contact sans nom");
            }
            c.address
                .trim()
                .parse::<Address>()
                .map_err(|e| anyhow!("adresse de {} : {e}", c.name))?;
            if !seen.insert(c.name.trim().to_lowercase()) {
                bail!("deux contacts portent le nom {}", c.name);
            }
        }
        let mut vault = app.existing_vault()?;
        vault.contacts = b
            .contacts
            .into_iter()
            .map(|c| Contact {
                name: c.name.trim().to_owned(),
                address: c.address.trim().to_owned(),
            })
            .collect();
        app.save(&vault)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[derive(Deserialize)]
struct NodeBody {
    node: String,
}

async fn put_node(State(app): State<Shared>, Json(b): Json<NodeBody>) -> ApiResult {
    blocking(move || {
        let url = b.node.trim().to_owned();
        Client::new(url.clone())
            .status()
            .context("nœud injoignable à cette adresse")?;
        let mut vault = app.existing_vault()?;
        vault.node = url;
        app.save(&vault)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[derive(Deserialize)]
struct Index {
    account: u32,
}

async fn get_overview(
    State(app): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<Index>,
) -> ApiResult {
    blocking(move || overview(&app, q.account)).await
}

async fn preview(State(app): State<Shared>, Json(op): Json<Operation>) -> ApiResult {
    blocking(move || Ok(plan_json(&plan(&app, &op)?))).await
}

async fn run(State(app): State<Shared>, Json(op): Json<Operation>) -> ApiResult {
    blocking(move || execute(&app, &op)).await
}

#[derive(Deserialize)]
struct AssetQuery {
    symbol: String,
    #[serde(default)]
    offset: usize,
    before: Option<u64>,
}

/// Console émetteur : fiche d'un jeton, ses détenteurs et son historique.
/// `holders` et `history` sont `null` sur un nœud antérieur à la 0.22.
async fn get_asset(
    State(app): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<AssetQuery>,
) -> ApiResult {
    blocking(move || {
        let client = app.client()?;
        let (asset, _) = asset_info(&client, &q.symbol)?;
        let symbol = q.symbol.trim().to_ascii_uppercase();
        let holders = client.get_json(&format!(
            "/assets/{symbol}/holders?limit=100&offset={}",
            q.offset
        ))?;
        let before = q.before.map(|b| format!("&before={b}")).unwrap_or_default();
        let history =
            client.get_json(&format!("/assets/{symbol}/transactions?limit=30{before}"))?;
        Ok(json!({ "asset": asset, "holders": holders, "history": history }))
    })
    .await
}

/// Lignes au plus d'une distribution : la file d'attente d'un compte en
/// accepte 100.
const MAX_DISTRIBUTION: usize = 100;

#[derive(Deserialize)]
struct Distribution {
    account: u32,
    symbol: String,
    /// `send` : depuis le solde du compte ; `mint` : émission (émetteur).
    mode: String,
    /// Une ligne « destinataire ; montant ; mémo » par versement.
    lines: String,
    /// Signer et envoyer ; sinon, seulement vérifier.
    #[serde(default)]
    execute: bool,
}

async fn distribute_route(State(app): State<Shared>, Json(d): Json<Distribution>) -> ApiResult {
    blocking(move || distribute(&app, &d)).await
}

/// Vérifie chaque ligne d'une distribution de jetons ; avec `execute`, et si
/// aucune ligne n'est en erreur, signe et envoie une transaction par ligne,
/// avec des nonces consécutifs.
fn distribute(app: &App, d: &Distribution) -> Result<Value> {
    let seed = app.seed()?;
    let me = address_of(&seed, d.account);
    app.account(d.account)?;
    let mint = match d.mode.as_str() {
        "mint" => true,
        "send" => false,
        other => bail!("mode inconnu : {other}"),
    };
    let client = app.client()?;
    let status = client.status_json()?;
    let max_memo = at(&status, "/params/max_memo_bytes").as_u64().unwrap_or(32);
    let fee = client.recommended_fee()?;
    let (asset, dec) = asset_info(&client, &d.symbol)?;
    let symbol = d.symbol.trim().to_ascii_uppercase();
    let me_json = client
        .get_json(&format!("/accounts/{me}"))?
        .ok_or_else(|| anyhow!("ce compte est vide : il doit d'abord recevoir des VINX"))?;
    if at(&asset, "/frozen") == &Value::Bool(true) {
        bail!("{symbol} est entièrement gelé : plus aucune unité ne circule");
    }
    let mine = holding(&me_json, &symbol);
    let my_units: Amount = mine
        .as_ref()
        .and_then(|h| at(h, "/units").as_str()?.parse().ok())
        .unwrap_or(0);
    let supply: Amount = at(&asset, "/supply_units")
        .as_str()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let max: Option<Amount> = at(&asset, "/max_supply_units")
        .as_str()
        .and_then(|v| v.parse().ok());
    if mint {
        if at(&asset, "/issuer").as_str() != Some(me.to_string().as_str()) {
            bail!("seul l'émetteur de {symbol} peut en émettre");
        }
        if at(&asset, "/powers/mint") != &Value::Bool(true) {
            bail!("l'émetteur a renoncé au pouvoir d'émettre {symbol}");
        }
        if !at(&me_json, "/policy").is_null() {
            bail!(
                "ce compte a des règles : émettre exige l'approbation de ses signataires \
                 (« vinx-wallet account propose »)"
            );
        }
    } else if mine
        .as_ref()
        .is_some_and(|h| at(h, "/frozen") == &Value::Bool(true))
    {
        bail!("{symbol} est gelé sur ce compte");
    }

    let mut rows = Vec::new();
    let mut bodies = Vec::new();
    let mut total: Amount = 0;
    let mut errors = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    let mut warnings = Vec::new();
    for (n, raw) in d.lines.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let row = n.saturating_add(1);
        let mut cols = line.split([';', '\t']).map(str::trim);
        let who = cols.next().unwrap_or_default().to_owned();
        let amount_text = cols.next().unwrap_or_default().replace(',', ".");
        let memo = cols.next().unwrap_or_default().to_owned();
        let checked = (|| -> Result<(Address, String, Amount, TxBody)> {
            let amount = parse_units(&amount_text, dec)?;
            if amount == 0 {
                bail!("montant nul");
            }
            let memo_b = memo_bytes(&memo);
            if memo_b
                .as_ref()
                .is_some_and(|m| u64::try_from(m.len()).map_or(true, |l| l > max_memo))
            {
                bail!("mémo trop long ({max_memo} octets au plus)");
            }
            let (to, how) = resolve(app, &client, &who)?;
            if to == me && !mint {
                bail!("c'est l'adresse de ce compte");
            }
            let theirs = if to == me {
                me_json.clone()
            } else {
                client
                    .get_json(&format!("/accounts/{to}"))?
                    .unwrap_or(Value::Null)
            };
            let Some(h) = holding(&theirs, &symbol) else {
                bail!("n'a pas encore accepté {symbol}");
            };
            if at(&h, "/frozen") == &Value::Bool(true) {
                bail!("{symbol} est gelé sur ce compte");
            }
            let body = if mint {
                TxBody::MintAsset {
                    symbol: symbol.clone(),
                    to,
                    amount,
                }
            } else {
                if at(&theirs, "/memo_required") == &Value::Bool(true) && memo_b.is_none() {
                    bail!("exige un mémo");
                }
                TxBody::TransferAsset {
                    symbol: symbol.clone(),
                    to,
                    amount,
                    memo: memo_b,
                }
            };
            Ok((to, how, amount, body))
        })();
        match checked {
            Ok((to, how, amount, body)) => {
                if !seen.insert(to) {
                    warnings.push(format!("ligne {row} : destinataire déjà présent plus haut"));
                }
                total = total.saturating_add(amount);
                rows.push(json!({
                    "row": row, "input": who, "to": to.to_string(), "how": how,
                    "amount": format_units(amount, dec), "memo": memo, "error": null,
                }));
                bodies.push(body);
            }
            Err(e) => {
                errors = errors.saturating_add(1);
                rows.push(json!({
                    "row": row, "input": who, "to": null, "how": null,
                    "amount": amount_text, "memo": memo, "error": format!("{e:#}"),
                }));
            }
        }
    }
    let mut problems = Vec::new();
    if rows.is_empty() {
        problems.push(
            "aucune ligne : une ligne « destinataire ; montant ; mémo » par versement".to_owned(),
        );
    }
    if rows.len() > MAX_DISTRIBUTION {
        problems.push(format!(
            "{} lignes : {MAX_DISTRIBUTION} au plus par distribution, découpe la liste",
            rows.len()
        ));
    }
    if mint {
        if let Some(m) = max {
            if supply.saturating_add(total) > m {
                problems.push(format!(
                    "au-delà du plafond : {} {symbol} peuvent encore être émis",
                    format_units(m.saturating_sub(supply), dec)
                ));
            }
        }
    } else if total > my_units {
        problems.push(format!(
            "solde insuffisant : {} {symbol} à verser, {} disponibles",
            format_units(total, dec),
            format_units(my_units, dec)
        ));
    }
    let count = u64::try_from(rows.len()).unwrap_or(u64::MAX);
    let fees = fee.saturating_mul(count);
    let spendable: Amount = at(&me_json, "/spendable")
        .as_str()
        .or_else(|| at(&me_json, "/balance").as_str())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if fees > spendable {
        problems.push(format!(
            "VINX insuffisants pour les frais : {} VINX nécessaires, {} disponibles",
            format_vinx(fees),
            format_vinx(spendable)
        ));
    }
    let mut sent = Vec::new();
    let mut failed = Value::Null;
    if d.execute {
        if errors > 0 || !problems.is_empty() {
            bail!("corrige les lignes en erreur avant d'envoyer");
        }
        let keypair = derive_account(&seed, d.account);
        let chain_id = client.status()?.chain_id;
        let (first, has_key) = next_nonce(&client, &me)?;
        for (i, body) in bodies.into_iter().enumerate() {
            let nonce = first.saturating_add(u64::try_from(i)?);
            let tx = Transaction {
                chain_id: chain_id.clone(),
                sender: me,
                nonce,
                fee,
                body,
            };
            match client.submit(&tx.sign(&keypair, !has_key && i == 0)) {
                Ok(id) => sent.push(id),
                Err(e) => {
                    let row = rows.get(i).map_or(&Value::Null, |r| at(r, "/row"));
                    failed = json!(format!("ligne {row} : {e:#}"));
                    break;
                }
            }
        }
    }
    Ok(json!({
        "symbol": symbol,
        "mode": d.mode,
        "rows": rows,
        "count": rows.len(),
        "errors": errors,
        "problems": problems,
        "warnings": warnings,
        "total": format_units(total, dec),
        "fees": format_vinx(fees),
        "sent": sent,
        "failed": failed,
    }))
}

#[derive(Deserialize)]
struct QrBody {
    text: String,
}

/// QR code d'un texte (demande de paiement), en SVG.
async fn qr(Json(b): Json<QrBody>) -> ApiResult {
    if b.text.len() > 1024 {
        return Err(fail("texte trop long pour un QR code"));
    }
    let code = qrcode::QrCode::new(b.text.as_bytes()).map_err(fail)?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build();
    Ok(Json(json!({ "svg": svg })))
}

#[derive(Deserialize)]
struct Verify {
    id: String,
    hash: String,
}

/// Vérifie qu'une transaction horodate bien l'empreinte donnée.
fn verify_timestamp(client: &Client, id: &str, hash: &str) -> Result<Value> {
    let id = id.trim().to_lowercase();
    let hash = hash.trim().to_lowercase();
    if id.is_empty() {
        // Horodatage natif (protocole 4) : retrouvé par sa seule empreinte.
        let Some(r) = client.get_json(&format!("/attestations/{hash}"))? else {
            return Ok(json!({ "matches": false, "not_found": true }));
        };
        let first = at(&r, "/attestations/0");
        return Ok(json!({
            "matches": true,
            "height": at(first, "/height"),
            "timestamp": at(first, "/timestamp"),
            "sender": at(first, "/sender"),
            "id": at(first, "/id"),
        }));
    }
    let t = client
        .get_json(&format!("/transactions/{id}"))?
        .ok_or_else(|| anyhow!("transaction introuvable sur ce nœud (identifiant faux, ou bloc trop ancien pour un nœud non archive)"))?;
    let on_chain = at(&t, "/digest")
        .as_str()
        .or_else(|| at(&t, "/memo_hex").as_str())
        .unwrap_or_default();
    let matches = !on_chain.is_empty() && on_chain == hash;
    Ok(json!({
        "matches": matches,
        "status": at(&t, "/status"),
        "height": at(&t, "/height"),
        "timestamp": at(&t, "/timestamp"),
        "sender": at(&t, "/sender"),
        "memo_hex": on_chain,
    }))
}

async fn verify(State(app): State<Shared>, Json(b): Json<Verify>) -> ApiResult {
    blocking(move || verify_timestamp(&app.client()?, &b.id, &b.hash)).await
}

#[derive(Deserialize)]
struct Lookup {
    to: String,
}

async fn lookup(State(app): State<Shared>, Json(b): Json<Lookup>) -> ApiResult {
    blocking(move || {
        let client = app.client()?;
        let (a, how) = resolve(&app, &client, &b.to)?;
        Ok(json!({ "address": a.to_string(), "how": how }))
    })
    .await
}

pub fn router(dir: PathBuf, token: String, port: u16) -> Router {
    router_with_kdf(dir, token, port, DEFAULT_KDF)
}

fn router_with_kdf(dir: PathBuf, token: String, port: u16, kdf: Kdf) -> Router {
    let shared = Arc::new(App {
        dir,
        token,
        port,
        kdf,
        session: Mutex::new(None),
    });
    Router::new()
        .route("/", get(page))
        .route("/fonts/{name}", get(crate::cockpit::font))
        .route("/api/state", get(get_state))
        .route("/api/phrase", get(new_phrase))
        .route("/api/create", post(create))
        .route("/api/unlock", post(unlock))
        .route("/api/lock", post(lock))
        .route("/api/reveal", post(reveal))
        .route("/api/accounts", post(add_account))
        .route("/api/accounts/rename", post(rename_account))
        .route("/api/contacts", post(put_contacts))
        .route("/api/node", post(put_node))
        .route("/api/overview", get(get_overview))
        .route("/api/lookup", post(lookup))
        .route("/api/qr", post(qr))
        .route("/api/verify", post(verify))
        .route("/api/preview", post(preview))
        .route("/api/execute", post(run))
        .route("/api/asset", get(get_asset))
        .route("/api/distribute", post(distribute_route))
        .layer(axum::middleware::from_fn_with_state(shared.clone(), guard))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .with_state(shared)
}

/// Dossier par défaut du portefeuille : `%APPDATA%\VinX` sous Windows,
/// `~/Library/Application Support/VinX` sous macOS, `~/.local/share/vinx` ailleurs.
pub fn default_dir() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(windows) {
        env("APPDATA")
            .unwrap_or_else(|| PathBuf::from("."))
            .join("VinX")
    } else if cfg!(target_os = "macos") {
        env("HOME")
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/VinX")
    } else {
        env("XDG_DATA_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("vinx")
    }
}

/// Ouvre l'URL dans le navigateur par défaut ; sans effet si c'est impossible.
fn open_browser(url: &str) {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    let _ = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Démarre le portefeuille et attend Ctrl+C (ou la fermeture de la fenêtre).
pub fn serve(dir: &Path, port: u16, open: bool) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    let dir = dir
        .canonicalize()
        .with_context(|| format!("dossier introuvable : {}", dir.display()))?;
    let token = crate::cockpit::new_token();
    let app = router(dir.clone(), token, port);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .with_context(|| {
                format!("port {port} déjà utilisé : le portefeuille est peut-être déjà ouvert")
            })?;
        let url = format!("http://127.0.0.1:{port}/");
        println!("Portefeuille VinX : {url}");
        println!("dossier : {}", dir.display());
        println!("Laisse cette fenêtre ouverte ; ferme-la (ou Ctrl+C) pour quitter.");
        if open {
            open_browser(&url);
        }
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .context("serveur du portefeuille")
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)] // code de test
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    /// Argon2 au minimum : les tests n'ont pas à être lents.
    const FAST: Kdf = Kdf {
        m_cost_kib: 8,
        t_cost: 1,
        p_cost: 1,
    };

    fn app(dir: &Path) -> Router {
        router_with_kdf(dir.to_path_buf(), "jeton".into(), 8700, FAST)
    }

    async fn call(app: &Router, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = axum::http::Request::builder()
            .uri(path)
            .header("host", "127.0.0.1:8700")
            .header("x-app-token", "jeton")
            .header("content-type", "application/json")
            .method(if body.is_some() { "POST" } else { "GET" })
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[test]
    fn derivation_is_stable_and_distinct() {
        let seed = seed_of(WORDS).unwrap();
        assert_eq!(
            address_of(&seed, 0),
            address_of(&seed_of(&WORDS.to_uppercase()).unwrap(), 0)
        );
        assert_ne!(address_of(&seed, 0), address_of(&seed, 1));
        assert!(seed_of("abandon abandon").is_err());
        assert!(seed_of(&WORDS.replace("about", "abandon")).is_err());
    }

    #[tokio::test]
    async fn create_lock_unlock_and_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let (_, s) = call(&app, "/api/state", None).await;
        assert_eq!(s["exists"], false);
        let (_, p) = call(&app, "/api/phrase", None).await;
        assert_eq!(p["words"].as_str().unwrap().split(' ').count(), 12);

        let install = json!({ "words": WORDS, "password": "court", "node": "http://127.0.0.1:1" });
        let (code, _) = call(&app, "/api/create", Some(install)).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        let install =
            json!({ "words": WORDS, "password": "mot de passe", "node": "http://127.0.0.1:1" });
        let (code, _) = call(&app, "/api/create", Some(install.clone())).await;
        assert_eq!(code, StatusCode::OK);
        let (code, _) = call(&app, "/api/create", Some(install)).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "pas d'écrasement");

        let text = std::fs::read_to_string(dir.path().join(VAULT)).unwrap();
        assert!(!text.contains("abandon"), "la phrase est chiffrée");

        let (_, s) = call(&app, "/api/state", None).await;
        assert_eq!(s["unlocked"], true);
        let seed = seed_of(WORDS).unwrap();
        assert_eq!(
            s["accounts"][0]["address"],
            address_of(&seed, 0).to_string()
        );

        call(&app, "/api/accounts", Some(json!({ "label": "Épargne" }))).await;
        let (_, s) = call(&app, "/api/state", None).await;
        assert_eq!(
            s["accounts"][1]["address"],
            address_of(&seed, 1).to_string()
        );
        assert_eq!(s["accounts"][1]["label"], "Épargne");

        call(&app, "/api/lock", Some(json!({}))).await;
        let (code, _) = call(&app, "/api/accounts", Some(json!({ "label": "x" }))).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "verrouillé");
        let (code, _) = call(&app, "/api/unlock", Some(json!({ "password": "faux" }))).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        let (code, _) = call(
            &app,
            "/api/unlock",
            Some(json!({ "password": "mot de passe" })),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (_, r) = call(
            &app,
            "/api/reveal",
            Some(json!({ "password": "mot de passe" })),
        )
        .await;
        assert_eq!(r["words"], WORDS);
    }

    #[tokio::test]
    async fn contacts_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let install =
            json!({ "words": WORDS, "password": "mot de passe", "node": "http://127.0.0.1:1" });
        call(&app, "/api/create", Some(install)).await;
        let addr = address_of(&seed_of(WORDS).unwrap(), 3).to_string();
        let ok = json!({ "contacts": [{ "name": "Bob", "address": addr }] });
        assert_eq!(
            call(&app, "/api/contacts", Some(ok)).await.0,
            StatusCode::OK
        );
        let bad = json!({ "contacts": [{ "name": "Bob", "address": "vinx1abc" }] });
        assert_eq!(
            call(&app, "/api/contacts", Some(bad)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let twice = json!({ "contacts": [{ "name": "Bob", "address": addr }, { "name": "bob", "address": addr }] });
        assert_eq!(
            call(&app, "/api/contacts", Some(twice)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn payment_request_qr() {
        let dir = tempfile::tempdir().unwrap();
        let link = json!({ "text": "vinx:alice.vinx?amount=12.5&memo=FAC-42" });
        let (code, r) = call(&app(dir.path()), "/api/qr", Some(link)).await;
        assert_eq!(code, StatusCode::OK);
        assert!(r["svg"].as_str().unwrap().contains("<svg"));
        let long = json!({ "text": "x".repeat(2000) });
        assert_eq!(
            call(&app(dir.path()), "/api/qr", Some(long)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn api_needs_token() {
        let dir = tempfile::tempdir().unwrap();
        let req = axum::http::Request::builder()
            .uri("/api/state")
            .header("host", "127.0.0.1:8700")
            .body(Body::empty())
            .unwrap();
        let res = app(dir.path()).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }
}
