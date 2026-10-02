//! The bundled validator node: located next to the app, run detached with its own data
//! directory, found again by its local API after the window was closed.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::{get, App};

pub const LOCAL_RPC_PORT: u16 = 18545;
pub const LOCAL_API: &str = "http://127.0.0.1:18545";
const P2P_PORT: u16 = 9001;

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "vinx-node.exe"
    } else {
        "vinx-node"
    }
}

/// The node binary: `VINX_NODE_BIN`, next to the app (installed bundles), or the
/// repository's release build (development).
pub fn binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("VINX_NODE_BIN") {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![dir.join(exe_name())];
    // macOS bundles keep sidecars in Contents/MacOS, resources one level up.
    candidates.push(dir.join("../Resources").join(exe_name()));
    candidates.push(dir.join("../../../../target/release").join(exe_name()));
    candidates.into_iter().find(|p| p.is_file())
}

fn data_dir(app: &App) -> PathBuf {
    app.dir.join("node")
}

/// Height of the local node, if it runs.
pub async fn local_height(app: &App) -> Option<u64> {
    get(app, &format!("{LOCAL_API}/health"))
        .await
        .ok()
        .and_then(|h| h["height"].as_u64())
}

fn host_of(entry_api: &str) -> String {
    entry_api
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split(['/', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// First start: fetches the chain's genesis from the entry point and remembers it.
pub async fn prepare(app: &App, _owner: &str, entry_api: &str) -> Result<(), String> {
    binary().ok_or("le programme du nœud (vinx-node) est introuvable à côté de l'application")?;
    let genesis = app.dir.join("genesis.json");
    if !genesis.exists() {
        let spec = get(app, &format!("{entry_api}/chain/genesis"))
            .await
            .map_err(|_| {
                "impossible de récupérer la genèse auprès du point d'entrée".to_string()
            })?;
        std::fs::write(&genesis, serde_json::to_string_pretty(&spec).unwrap())
            .map_err(|e| e.to_string())?;
    }
    // The entry's P2P port follows its API port (8545 → 9001), like `./vinx`.
    let rpc_port: u16 = entry_api
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse().ok())
        .unwrap_or(8545);
    let p2p = (rpc_port as i32 - 8545 + P2P_PORT as i32).clamp(1, 65535);
    std::fs::write(
        app.dir.join("entry"),
        format!("{} {rpc_port} {p2p}", host_of(entry_api)),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn chain_id(app: &App) -> Result<u32, String> {
    let g: Value = serde_json::from_str(
        &std::fs::read_to_string(app.dir.join("genesis.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(g["chain_id"].as_u64().unwrap_or(0) as u32)
}

fn base_args(app: &App, owner: &str, chain: u32) -> Vec<String> {
    vec![
        "--data-dir".into(),
        data_dir(app).display().to_string(),
        "--chain-id".into(),
        chain.to_string(),
        "--validator-owner".into(),
        owner.into(),
    ]
}

/// The node's BLS key and its proof of possession bound to `owner`, plus the
/// operator address: what the wallet's bond registers on-chain.
pub fn validator_entry(app: &App, owner: &str, chain: u32) -> Result<Value, String> {
    let bin = binary().ok_or("vinx-node introuvable")?;
    let out = Command::new(bin)
        .args(base_args(app, owner, chain))
        .arg("--genesis-entry")
        .stderr(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.starts_with('{'))
        .and_then(|l| serde_json::from_str(l).ok())
        .ok_or_else(|| "le nœud n'a pas fourni ses clés de validateur".to_string())
}

/// Starts the node in the background unless it already runs.
pub async fn start(app: &App, owner: &str) -> Result<(), String> {
    if local_height(app).await.is_some() {
        return Ok(());
    }
    let bin = binary().ok_or("vinx-node introuvable")?;
    let chain = chain_id(app)?;
    let entry_file = std::fs::read_to_string(app.dir.join("entry")).unwrap_or_default();
    let mut parts = entry_file.split_whitespace();
    let entry = parts.next().unwrap_or("");
    let entry_rpc = parts.next().unwrap_or("8545");
    let entry_p2p = parts.next().unwrap_or("9001");
    std::fs::create_dir_all(data_dir(app)).map_err(|e| e.to_string())?;
    let mut args = base_args(app, owner, chain);
    args.extend([
        "--rpc-listen".into(),
        format!("127.0.0.1:{LOCAL_RPC_PORT}"),
        "--p2p-listen".into(),
        format!("/ip4/0.0.0.0/tcp/{P2P_PORT}"),
    ]);
    if !entry.is_empty() {
        let proto = if entry.parse::<std::net::Ipv4Addr>().is_ok() {
            "ip4"
        } else {
            "dns4"
        };
        args.extend([
            "--peers".into(),
            format!("/{proto}/{entry}/tcp/{entry_p2p}"),
            "--sync-peer".into(),
            format!("http://{entry}:{entry_rpc}"),
        ]);
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(app.dir.join("node.log"))
        .map_err(|e| e.to_string())?;
    let mut cmd = Command::new(bin);
    cmd.args(&args)
        .env("VINX_GENESIS_SPEC", app.dir.join("genesis.json"))
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    detach(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("démarrage du nœud impossible : {e}"))?;
    let _ = std::fs::write(app.dir.join("node.pid"), child.id().to_string());
    // Startup sync can take a while; wait for the API up to ~2 minutes.
    for _ in 0..240 {
        if local_height(app).await.is_some() {
            return Ok(());
        }
        sleep_ms(500).await;
    }
    Err("le nœud ne répond pas — voir node.log dans le dossier de l'application".into())
}

/// Stops the node started by the app.
pub fn stop(app: &App) {
    let pid_file = app.dir.join("node.pid");
    if let Ok(pid) = std::fs::read_to_string(&pid_file) {
        let pid = pid.trim();
        #[cfg(unix)]
        let _ = Command::new("kill").arg(pid).status();
        #[cfg(windows)]
        let _ = Command::new("taskkill")
            .args(["/PID", pid, "/T", "/F"])
            .status();
    }
    let _ = std::fs::remove_file(pid_file);
}

#[cfg(unix)]
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0); // survives the window closing
}

#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
}

pub async fn sleep_ms(ms: u64) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        let _ = tx.send(());
    });
    let _ = tauri::async_runtime::spawn_blocking(move || rx.recv()).await;
}
