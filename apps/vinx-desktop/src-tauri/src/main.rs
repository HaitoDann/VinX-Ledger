// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! VinX desktop: a wallet anyone can use, and a validator switch.
//!
//! - The wallet key lives encrypted on disk (`wallet.json`, same format as the CLI) and,
//!   once unlocked, only in this process: it never reaches the webview or a node.
//! - The validator is the bundled `vinx-node`, run with **its own** operator and BLS
//!   keys (ADR 0084 S5). The wallet only signs the bond that names that node, so the
//!   funds key is never written in clear for the node. The node keeps running when the
//!   window closes; the app finds it again on the next start.

mod node;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{Manager, State};
use vinx_core::amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS};
use vinx_core::Transaction;
use vinx_crypto::{Address, KeyPair};
use vinx_desktop_core as core;

const TESTNET_SEEDS: &str = include_str!("../../../../seeds/testnet.txt");

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Settings {
    /// Entry point of the network (DNS name or IP of any node with its API open).
    pub entry: String,
    /// The user turned the validator on: restart it with the app.
    pub validator: bool,
}

pub struct App {
    pub dir: PathBuf,
    wallet: Mutex<Option<KeyPair>>,
    /// Created but not yet saved (the user is writing the phrase down).
    pending: Mutex<Option<KeyPair>>,
    settings: Mutex<Settings>,
    pub http: reqwest::Client,
}

type Res<T> = Result<T, String>;

impl App {
    fn wallet_path(&self) -> PathBuf {
        self.dir.join("wallet.json")
    }
    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }
    fn save_settings(&self, s: Settings) {
        let _ = std::fs::write(
            self.dir.join("settings.json"),
            serde_json::to_string_pretty(&s).unwrap_or_default(),
        );
        *self.settings.lock().unwrap() = s;
    }
    fn keypair(&self) -> Res<KeyPair> {
        self.wallet
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "portefeuille verrouillé".to_string())
    }
    fn address(&self) -> Option<String> {
        core::secure::wallet_address(&self.wallet_path()).ok()
    }
    /// The network's public API (the entry point).
    fn entry_api(&self) -> Res<String> {
        let e = self.settings().entry;
        if e.trim().is_empty() {
            return Err("aucun point d'entrée réseau : renseignez-le dans Réglages".into());
        }
        Ok(api_url(&e))
    }
    /// Where to read and send: the local node once it has caught up, else the entry.
    async fn api(&self) -> Res<String> {
        if let Some(local) = node::local_height(self).await {
            if let Ok(entry) = self.entry_api() {
                match get(self, &format!("{entry}/health")).await {
                    Ok(h) if h["height"].as_u64().unwrap_or(0) > local + 2 => return Ok(entry),
                    _ => {}
                }
            }
            return Ok(node::LOCAL_API.to_string());
        }
        self.entry_api()
    }
}

/// `host`, `host:port` or a full URL → API base URL (default port 8545).
fn api_url(entry: &str) -> String {
    let e = entry.trim().trim_end_matches('/');
    if e.starts_with("http://") || e.starts_with("https://") {
        return e.to_string();
    }
    if e.contains(':') {
        format!("http://{e}")
    } else {
        format!("http://{e}:8545")
    }
}

pub async fn get(app: &App, url: &str) -> Res<Value> {
    let r = app
        .http
        .get(url)
        .send()
        .await
        .map_err(|_| "réseau injoignable".to_string())?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(v["error"].as_str().unwrap_or("erreur du nœud").to_string());
    }
    Ok(v)
}

async fn post(app: &App, url: &str, body: &Value) -> Res<Value> {
    let r = app
        .http
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|_| "réseau injoignable".to_string())?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(v["error"]
            .as_str()
            .unwrap_or("refusé par le nœud")
            .to_string());
    }
    Ok(v)
}

fn fmt_vinx(atoms: u128) -> String {
    let f = vinx_core::amount::DECIMAL_FACTOR;
    let cents = (atoms % f) * 100 / f;
    format!(
        "{}{}",
        atoms / f,
        if cents > 0 {
            format!(",{cents:02}")
        } else {
            String::new()
        }
    )
}

fn atoms(v: &Value) -> u128 {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_u64().map(u128::from))
        .unwrap_or(0)
}

async fn submit(app: &App, api: &str, tx: &Transaction) -> Res<String> {
    let v = post(
        app,
        &format!("{api}/tx/submit"),
        &serde_json::to_value(tx).unwrap(),
    )
    .await?;
    if v["accepted"].as_bool() == Some(false) {
        return Err(v["error"]
            .as_str()
            .unwrap_or("transaction refusée")
            .to_string());
    }
    Ok(v["tx_hash"].as_str().unwrap_or_default().to_string())
}

/// Chain id, nonce and current fee for the next transaction of `addr`.
async fn tx_context(app: &App, api: &str, addr: &str) -> Res<(u32, u64, Amount)> {
    let h = get(app, &format!("{api}/health")).await?;
    let acc = get(app, &format!("{api}/account/{addr}"))
        .await
        .unwrap_or(Value::Null);
    let stats = get(app, &format!("{api}/network/stats"))
        .await
        .unwrap_or(Value::Null);
    let fee = atoms(&stats["base_fee_atoms"]).max(DEFAULT_FEE_FLOOR_ATOMS);
    Ok((
        h["chain_id"].as_u64().unwrap_or(0) as u32,
        acc["nonce"].as_u64().unwrap_or(0),
        Amount::from_atoms(fee),
    ))
}

// ─── Wallet ───────────────────────────────────────────────────────────────────

#[tauri::command]
fn info(app: State<'_, App>) -> Value {
    json!({
        "has_wallet": app.wallet_path().exists(),
        "address": app.address(),
        "unlocked": app.wallet.lock().unwrap().is_some(),
        "settings": app.settings(),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

#[tauri::command]
fn create_wallet(app: State<'_, App>) -> Value {
    let (phrase, kp) = core::secure::new_phrase();
    let address = Address::from_public_key(&kp.public_key()).to_string();
    *app.pending.lock().unwrap() = Some(kp);
    json!({ "phrase": phrase, "address": address })
}

#[tauri::command]
fn confirm_wallet(app: State<'_, App>, password: String) -> Res<String> {
    let kp = app
        .pending
        .lock()
        .unwrap()
        .clone()
        .ok_or("aucun portefeuille en cours de création")?;
    core::secure::save_wallet(&app.wallet_path(), &kp, &password).map_err(|e| e.to_string())?;
    *app.pending.lock().unwrap() = None;
    let addr = Address::from_public_key(&kp.public_key()).to_string();
    *app.wallet.lock().unwrap() = Some(kp);
    Ok(addr)
}

#[tauri::command]
fn restore_wallet(app: State<'_, App>, phrase: String, password: String) -> Res<String> {
    let kp = core::secure::key_from_phrase(&phrase).map_err(|e| e.to_string())?;
    core::secure::save_wallet(&app.wallet_path(), &kp, &password).map_err(|e| e.to_string())?;
    let addr = Address::from_public_key(&kp.public_key()).to_string();
    *app.wallet.lock().unwrap() = Some(kp);
    Ok(addr)
}

#[tauri::command]
fn unlock(app: State<'_, App>, password: String) -> Res<String> {
    let kp = core::secure::open_wallet(&app.wallet_path(), &password).map_err(|e| e.to_string())?;
    let addr = Address::from_public_key(&kp.public_key()).to_string();
    *app.wallet.lock().unwrap() = Some(kp);
    Ok(addr)
}

#[tauri::command]
fn lock(app: State<'_, App>) {
    *app.wallet.lock().unwrap() = None;
}

#[tauri::command]
fn set_entry(app: State<'_, App>, entry: String) {
    let mut s = app.settings();
    s.entry = entry.trim().to_string();
    app.save_settings(s);
}

/// Network and local node state, for the status bar.
#[tauri::command]
async fn status(app: State<'_, App>) -> Res<Value> {
    let entry = match app.entry_api() {
        Ok(u) => get(&app, &format!("{u}/health")).await.ok(),
        Err(_) => None,
    };
    let local = get(&app, &format!("{}/health", node::LOCAL_API)).await.ok();
    let api = app.api().await.ok();
    let reference = local.clone().or(entry.clone());
    Ok(json!({
        "online": reference.is_some(),
        "height": reference.as_ref().map(|h| h["height"].clone()),
        "tip_timestamp": reference.as_ref().map(|h| h["tip_timestamp"].clone()),
        "block_time_secs": reference.as_ref().map(|h| h["block_time_secs"].clone()),
        "chain_id": reference.as_ref().map(|h| h["chain_id"].clone()),
        "entry_height": entry.as_ref().map(|h| h["height"].clone()),
        "local": local,
        "api": api,
    }))
}

#[tauri::command]
async fn account(app: State<'_, App>) -> Res<Value> {
    let addr = app.address().ok_or("aucun portefeuille")?;
    let api = app.api().await?;
    let acc = get(&app, &format!("{api}/account/{addr}"))
        .await
        .unwrap_or(json!({
            "balance_atoms": "0", "staked_atoms": "0", "nonce": 0
        }));
    let txs = get(&app, &format!("{api}/account/{addr}/txs?limit=30"))
        .await
        .map(|v| v["txs"].clone())
        .unwrap_or(json!([]));
    Ok(json!({ "address": addr, "account": acc, "txs": txs }))
}

#[tauri::command]
async fn send(app: State<'_, App>, to: String, amount: String, memo: String) -> Res<String> {
    let kp = app.keypair()?;
    let from = Address::from_public_key(&kp.public_key()).to_string();
    let amount: String = amount.chars().filter(|c| !c.is_whitespace()).collect();
    let amount = core::parse_amount(&amount.replace(',', "."))
        .map_err(|_| "montant invalide".to_string())?;
    if amount.atoms() == 0 {
        return Err("montant invalide".into());
    }
    let api = app.api().await?;
    let (chain_id, nonce, fee) = tx_context(&app, &api, &from).await?;
    let tx = core::build_transfer_memo(&kp, to.trim(), amount, fee, nonce, chain_id, memo.trim())
        .map_err(|e| match e {
        core::CoreError::Address(m) if m.contains("memo") => {
            "mémo trop long (32 octets maximum)".to_string()
        }
        _ => "adresse invalide".to_string(),
    })?;
    submit(&app, &api, &tx).await
}

#[tauri::command]
async fn faucet(app: State<'_, App>) -> Res<String> {
    let addr = app.address().ok_or("aucun portefeuille")?;
    let v = post(
        &app,
        &format!("{}/faucet/request", app.entry_api()?),
        &json!({ "address": addr }),
    )
    .await?;
    Ok(v["amount"].as_str().unwrap_or("").to_string())
}

#[tauri::command]
fn receive_qr(app: State<'_, App>) -> Res<String> {
    let addr = app.address().ok_or("aucun portefeuille")?;
    let code = qrcode::QrCode::new(format!("vinx:{addr}")).map_err(|e| e.to_string())?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .quiet_zone(true)
        .build())
}

// ─── Validator ────────────────────────────────────────────────────────────────

#[tauri::command]
async fn validator(app: State<'_, App>) -> Res<Value> {
    let addr = app.address().ok_or("aucun portefeuille")?;
    let running = node::local_height(&app).await.is_some();
    let api = app.api().await.ok();
    let pool = match &api {
        Some(api) => get(&app, &format!("{api}/validator/{addr}")).await.ok(),
        None => None,
    };
    let set = match &api {
        Some(api) => get(&app, &format!("{api}/validators")).await.ok(),
        None => None,
    };
    Ok(json!({
        "enabled": app.settings().validator,
        "running": running,
        "node_found": node::binary().is_some(),
        "pool": pool,
        "set": set,
        "local": get(&app, &format!("{}/health", node::LOCAL_API)).await.ok(),
    }))
}

/// Turns the validator on: starts the node with its own keys, then bonds the
/// minimum from the wallet so the network admits it (after its warm-up).
#[tauri::command]
async fn validator_start(app: State<'_, App>) -> Res<String> {
    let kp = app.keypair()?;
    let owner = Address::from_public_key(&kp.public_key()).to_string();
    let entry_api = app.entry_api()?;
    node::prepare(&app, &owner, &entry_api).await?;
    let mut s = app.settings();
    s.validator = true;
    app.save_settings(s);
    node::start(&app, &owner).await?;

    let api = app.api().await?;
    let pool = get(&app, &format!("{api}/validator/{owner}")).await?;
    let min = atoms(&pool["min_bond_atoms"]);
    let acc = get(&app, &format!("{api}/account/{owner}"))
        .await
        .unwrap_or(Value::Null);
    if atoms(&acc["staked_atoms"]) >= min && pool["status"] != "none" {
        return Ok("déjà inscrit".into());
    }
    let (chain_id, nonce, fee) = tx_context(&app, &api, &owner).await?;
    if atoms(&acc["balance_atoms"]) < min + fee.atoms() {
        return Err(format!(
            "il faut au moins {} VINX sur le portefeuille pour la garantie",
            fmt_vinx(min + fee.atoms())
        ));
    }
    let entry = node::validator_entry(&app, &owner, chain_id)?;
    let tx =
        core::build_validator_stake(&kp, Amount::from_atoms(min), fee, nonce, chain_id, &entry)
            .map_err(|e| e.to_string())?;
    submit(&app, &api, &tx).await
}

/// Turns the validator off. With `withdraw`, also asks for the bond back (it is
/// released after the unbonding delay).
#[tauri::command]
async fn validator_stop(app: State<'_, App>, withdraw: bool) -> Res<()> {
    if withdraw {
        let kp = app.keypair()?;
        let owner = Address::from_public_key(&kp.public_key()).to_string();
        let api = app.api().await?;
        let acc = get(&app, &format!("{api}/account/{owner}")).await?;
        let staked = atoms(&acc["staked_atoms"]);
        if staked > 0 {
            let (chain_id, nonce, fee) = tx_context(&app, &api, &owner).await?;
            let tx = core::build_unstake(&kp, Amount::from_atoms(staked), fee, nonce, chain_id);
            submit(&app, &api, &tx).await?;
        }
    }
    let mut s = app.settings();
    s.validator = false;
    app.save_settings(s);
    node::stop(&app);
    Ok(())
}

/// `VINX_DESKTOP_SELFTEST=<entry>`: runs the real user journey against a network
/// (restore, faucet, validator on, payment) through the same commands as the UI,
/// prints each step and exits — the app's end-to-end check.
async fn selftest(handle: tauri::AppHandle, entry: String) {
    let st = handle.state::<App>();
    let step = |name: &str, r: Result<String, String>| {
        println!(
            "SELFTEST {name}: {}",
            r.as_ref()
                .map_or_else(|e| format!("ERR {e}"), |v| format!("ok {v}"))
        );
        r.is_ok()
    };
    let (phrase, _) = core::secure::new_phrase();
    let mut ok = step(
        "restore",
        restore_wallet(st.clone(), phrase, "selftest-pass".into()),
    );
    set_entry(st.clone(), entry);
    ok &= step("faucet", faucet(st.clone()).await);
    let mut funded = false;
    for _ in 0..60 {
        if let Ok(a) = account(st.clone()).await {
            if atoms(&a["account"]["balance_atoms"]) > 0 {
                funded = true;
                break;
            }
        }
        node::sleep_ms(1000).await;
    }
    ok &= step(
        "funded",
        if funded {
            Ok("yes".into())
        } else {
            Err("no funds".into())
        },
    );
    ok &= step("validator_start", validator_start(st.clone()).await);
    let mut status = "none".to_string();
    for _ in 0..30 {
        if let Ok(v) = validator(st.clone()).await {
            status = v["pool"]["status"].as_str().unwrap_or("none").to_string();
            if status != "none" {
                break;
            }
        }
        node::sleep_ms(1000).await;
    }
    ok &= step(
        "pool_status",
        if status == "none" || status.is_empty() {
            Err("no bond".into())
        } else {
            Ok(status)
        },
    );
    ok &= step(
        "send",
        send(
            st.clone(),
            "vinx1hmchmuudk6a3tugdfhjdqyd6j4pnpq94lc7e32".into(),
            "1,5".into(),
            "selftest".into(),
        )
        .await,
    );
    ok &= step(
        "validator_stop",
        validator_stop(st.clone(), false)
            .await
            .map(|_| "stopped".into()),
    );
    std::process::exit(if ok { 0 } else { 1 });
}

fn main() {
    tauri::Builder::default()
        .setup(|tauri_app| {
            let dir = tauri_app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from(".vinx-desktop"));
            let _ = std::fs::create_dir_all(&dir);
            let mut settings: Settings = std::fs::read_to_string(dir.join("settings.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            if settings.entry.is_empty() {
                settings.entry = TESTNET_SEEDS
                    .lines()
                    .map(|l| l.split('#').next().unwrap_or("").trim())
                    .find(|l| !l.is_empty())
                    .unwrap_or("")
                    .to_string();
            }
            let app = App {
                dir,
                wallet: Mutex::new(None),
                pending: Mutex::new(None),
                settings: Mutex::new(settings.clone()),
                http: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(8))
                    .build()
                    .expect("http client"),
            };
            tauri_app.manage(app);
            if let Ok(entry) = std::env::var("VINX_DESKTOP_SELFTEST") {
                tauri::async_runtime::spawn(selftest(tauri_app.handle().clone(), entry));
                return Ok(());
            }
            if settings.validator {
                // The validator does not need the wallet: restart it right away.
                let handle = tauri_app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let app = handle.state::<App>();
                    if let Some(owner) = app.address() {
                        let _ = node::start(&app, &owner).await;
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            info,
            create_wallet,
            confirm_wallet,
            restore_wallet,
            unlock,
            lock,
            set_entry,
            status,
            account,
            send,
            faucet,
            receive_qr,
            validator,
            validator_start,
            validator_stop,
        ])
        .run(tauri::generate_context!())
        .expect("error while running VinX");
}
