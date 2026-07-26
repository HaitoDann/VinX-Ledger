// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use tauri::{Manager, State};
use vinx_core::amount::Amount;
use vinx_core::protocol::ProtocolVersion;
use vinx_crypto::{Address, KeyPair};
use vinx_desktop_core as core;
use vinx_desktop_core::dto;

/// Session state. The private key lives only here, in-process; it never crosses the
/// IPC boundary to the webview and is never sent to the node.
struct AppState {
    wallet: Mutex<Option<KeyPair>>,
    http: reqwest::Client,
}

impl AppState {
    /// Clones the loaded keypair out under a short lock (so no guard is held across
    /// an await point). Errors if no wallet is open.
    fn keypair(&self) -> Result<KeyPair, String> {
        self.wallet
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "no wallet is open".to_string())
    }
}

fn base(node: &str) -> String {
    node.trim().trim_end_matches('/').to_string()
}

// ─── low-level RPC helpers ────────────────────────────────────────────────────

async fn get_json<T: DeserializeOwned>(client: &reqwest::Client, url: &str) -> Result<T, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("node unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(node_error(resp).await);
    }
    resp.json::<T>().await.map_err(|e| format!("bad response: {e}"))
}

/// Like `get_json` but maps a 404 to `Ok(None)` (used for accounts that don't exist yet).
async fn get_json_opt<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<Option<T>, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("node unreachable: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(node_error(resp).await);
    }
    resp.json::<T>().await.map(Some).map_err(|e| format!("bad response: {e}"))
}

async fn node_error(resp: reqwest::Response) -> String {
    let status = resp.status();
    #[derive(serde::Deserialize)]
    struct E {
        error: String,
    }
    match resp.json::<E>().await {
        Ok(e) => e.error,
        Err(_) => format!("HTTP {status}"),
    }
}

async fn submit_tx(
    client: &reqwest::Client,
    node: &str,
    tx: &vinx_core::Transaction,
) -> Result<String, String> {
    let url = format!("{}/tx/submit", base(node));
    let resp = client
        .post(&url)
        .json(tx)
        .send()
        .await
        .map_err(|e| format!("node unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(node_error(resp).await);
    }
    let r: dto::SubmitResult = resp.json().await.map_err(|e| format!("bad response: {e}"))?;
    Ok(r.tx_hash)
}

// ─── context needed to build a transaction ────────────────────────────────────

async fn chain_id(client: &reqwest::Client, node: &str) -> Result<u32, String> {
    let h: dto::Health = get_json(client, &format!("{}/health", base(node))).await?;
    Ok(h.chain_id)
}

async fn next_nonce(client: &reqwest::Client, node: &str, address: &str) -> Result<u64, String> {
    let acct: Option<dto::Account> =
        get_json_opt(client, &format!("{}/account/{}", base(node), address)).await?;
    Ok(acct.map(|a| a.nonce).unwrap_or(0))
}

async fn base_fee(client: &reqwest::Client, node: &str) -> Result<Amount, String> {
    let s: dto::NetworkStats = get_json(client, &format!("{}/network/stats", base(node))).await?;
    let atoms: u128 = s
        .base_fee_atoms
        .parse()
        .map_err(|_| "invalid base_fee from node".to_string())?;
    Ok(Amount::from_atoms(atoms))
}

fn own_address(kp: &KeyPair) -> String {
    Address::from_public_key(&kp.public_key()).to_string()
}

// ─── wallet commands ──────────────────────────────────────────────────────────

#[tauri::command]
fn wallet_address(state: State<'_, AppState>) -> Option<String> {
    state.wallet.lock().unwrap().as_ref().map(own_address)
}

#[tauri::command]
fn wallet_create(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let (ks, kp) = core::Keystore::generate();
    ks.save(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
    let address = own_address(&kp);
    *state.wallet.lock().unwrap() = Some(kp);
    Ok(address)
}

#[tauri::command]
fn wallet_open(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let ks = core::Keystore::load(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
    let kp = ks.to_keypair().map_err(|e| e.to_string())?;
    let address = own_address(&kp);
    *state.wallet.lock().unwrap() = Some(kp);
    Ok(address)
}

#[tauri::command]
fn wallet_close(state: State<'_, AppState>) {
    *state.wallet.lock().unwrap() = None;
}

#[tauri::command]
fn suggest_wallet_path(app: tauri::AppHandle) -> String {
    let dir: PathBuf = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("vinx-wallet.json").to_string_lossy().to_string()
}

// ─── read-only queries ────────────────────────────────────────────────────────

#[tauri::command]
async fn node_health(state: State<'_, AppState>, node: String) -> Result<dto::Health, String> {
    get_json(&state.http, &format!("{}/health", base(&node))).await
}

#[tauri::command]
async fn account(state: State<'_, AppState>, node: String, address: String) -> Result<dto::Account, String> {
    let acct: Option<dto::Account> =
        get_json_opt(&state.http, &format!("{}/account/{}", base(&node), address)).await?;
    Ok(acct.unwrap_or(dto::Account {
        address,
        balance: "0.00 VINX".into(),
        balance_atoms: "0".into(),
        nonce: 0,
        staked: "0.00 VINX".into(),
        staked_atoms: "0".into(),
    }))
}

#[tauri::command]
async fn network_stats(state: State<'_, AppState>, node: String) -> Result<dto::NetworkStats, String> {
    get_json(&state.http, &format!("{}/network/stats", base(&node))).await
}

#[tauri::command]
async fn validators(state: State<'_, AppState>, node: String) -> Result<dto::Validators, String> {
    get_json(&state.http, &format!("{}/validators", base(&node))).await
}

#[tauri::command]
async fn protocol(state: State<'_, AppState>, node: String) -> Result<dto::Protocol, String> {
    get_json(&state.http, &format!("{}/protocol/version", base(&node))).await
}

#[tauri::command]
async fn history(
    state: State<'_, AppState>,
    node: String,
    address: String,
    limit: usize,
    offset: usize,
) -> Result<dto::History, String> {
    let url = format!("{}/account/{}/txs?limit={}&offset={}", base(&node), address, limit, offset);
    get_json(&state.http, &url).await
}

// ─── signing commands (require an open wallet) ────────────────────────────────

#[tauri::command]
async fn send_transfer(
    state: State<'_, AppState>,
    node: String,
    to: String,
    amount: String,
) -> Result<String, String> {
    let kp = state.keypair()?;
    let amount = core::parse_amount(&amount).map_err(|e| e.to_string())?;
    let cid = chain_id(&state.http, &node).await?;
    let nonce = next_nonce(&state.http, &node, &own_address(&kp)).await?;
    let fee = base_fee(&state.http, &node).await?;
    let tx = core::build_transfer(&kp, &to, amount, fee, nonce, cid).map_err(|e| e.to_string())?;
    submit_tx(&state.http, &node, &tx).await
}

#[tauri::command]
async fn stake(state: State<'_, AppState>, node: String, amount: String) -> Result<String, String> {
    let kp = state.keypair()?;
    let amount = core::parse_amount(&amount).map_err(|e| e.to_string())?;
    let cid = chain_id(&state.http, &node).await?;
    let nonce = next_nonce(&state.http, &node, &own_address(&kp)).await?;
    let tx = core::build_stake(&kp, amount, Amount::ZERO, nonce, cid);
    submit_tx(&state.http, &node, &tx).await
}

#[tauri::command]
async fn unstake(state: State<'_, AppState>, node: String, amount: String) -> Result<String, String> {
    let kp = state.keypair()?;
    let amount = core::parse_amount(&amount).map_err(|e| e.to_string())?;
    let cid = chain_id(&state.http, &node).await?;
    let nonce = next_nonce(&state.http, &node, &own_address(&kp)).await?;
    let tx = core::build_unstake(&kp, amount, Amount::ZERO, nonce, cid);
    submit_tx(&state.http, &node, &tx).await
}

// ─── admin commands ───────────────────────────────────────────────────────────

#[tauri::command]
async fn admin_add_validator(state: State<'_, AppState>, node: String, validator: String) -> Result<String, String> {
    let kp = state.keypair()?;
    let cid = chain_id(&state.http, &node).await?;
    let nonce = next_nonce(&state.http, &node, &own_address(&kp)).await?;
    let tx = core::build_add_validator(&kp, &validator, nonce, cid).map_err(|e| e.to_string())?;
    submit_tx(&state.http, &node, &tx).await
}

#[tauri::command]
async fn admin_remove_validator(state: State<'_, AppState>, node: String, validator: String) -> Result<String, String> {
    let kp = state.keypair()?;
    let cid = chain_id(&state.http, &node).await?;
    let nonce = next_nonce(&state.http, &node, &own_address(&kp)).await?;
    let tx = core::build_remove_validator(&kp, &validator, nonce, cid).map_err(|e| e.to_string())?;
    submit_tx(&state.http, &node, &tx).await
}

#[tauri::command]
async fn admin_announce_upgrade(
    state: State<'_, AppState>,
    node: String,
    version: String,
    activation_height: u64,
) -> Result<String, String> {
    let kp = state.keypair()?;
    let ver: ProtocolVersion = version.parse().map_err(|e: String| e)?;
    let cid = chain_id(&state.http, &node).await?;
    let nonce = next_nonce(&state.http, &node, &own_address(&kp)).await?;
    let tx = core::build_announce_upgrade(&kp, ver, activation_height, nonce, cid);
    submit_tx(&state.http, &node, &tx).await
}

fn main() {
    let state = AppState {
        wallet: Mutex::new(None),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client"),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            wallet_address,
            wallet_create,
            wallet_open,
            wallet_close,
            suggest_wallet_path,
            node_health,
            account,
            network_stats,
            validators,
            protocol,
            history,
            send_transfer,
            stake,
            unstake,
            admin_add_validator,
            admin_remove_validator,
            admin_announce_upgrade,
        ])
        .run(tauri::generate_context!())
        .expect("error while running VinX Wallet");
}
