use std::path::{Path, PathBuf};
use clap::Parser;
use tracing_subscriber::EnvFilter;
use vinx_crypto::{Address, KeyPair};
use vinx_node::{chain::Chain, config::NodeConfig, storage::Storage};
use vinx_state::{create_genesis_state, GenesisConfig};

// ─── Config file ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
struct NodeConfigFile {
    block_time_secs: Option<u64>,
    max_block_txs: Option<usize>,
    rpc_listen: Option<String>,
    data_dir: Option<PathBuf>,
    p2p_listen: Option<String>,
    peers: Option<Vec<String>>,
    validator_key_file: Option<PathBuf>,
    admin_key_file: Option<PathBuf>,
    sync_peer_rpc: Option<String>,
}

impl NodeConfigFile {
    fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(cfg) => {
                tracing::info!(path = %path.display(), "Loaded config file");
                cfg
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "Failed to parse config file, using defaults");
                Self::default()
            }
        }
    }
}

// ─── CLI args ─────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "vinx-node", about = "VinX Ledger full node", version)]
struct Args {
    /// Path to TOML config file (default: config.toml if it exists)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Block time in seconds (overrides config)
    #[arg(long)]
    block_time: Option<u64>,
    /// RPC listen address, e.g. 0.0.0.0:8545 (overrides config)
    #[arg(long)]
    rpc_listen: Option<String>,
    /// P2P listen multiaddr, e.g. /ip4/0.0.0.0/tcp/9000 (overrides config)
    #[arg(long)]
    p2p_listen: Option<String>,
    /// Data directory for keys and persisted state (overrides config)
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Bootstrap peer multiaddrs (overrides config, repeatable)
    #[arg(long, num_args = 0..)]
    peers: Vec<String>,
    /// RPC URL of a trusted peer to sync from on startup, e.g. http://1.2.3.4:8545
    #[arg(long)]
    sync_peer: Option<String>,
}

// ─── Key file helpers ─────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct KeyFile {
    address: String,
    secret_key_hex: String,
}

impl KeyFile {
    fn generate() -> (Self, KeyPair) {
        let kp = KeyPair::generate();
        let address = Address::from_public_key(&kp.public_key()).to_string();
        let secret_key_hex = hex::encode(kp.secret_bytes());
        (Self { address, secret_key_hex }, kp)
    }

    fn load_or_generate(path: &Path) -> (Self, KeyPair) {
        if path.exists() {
            let json = std::fs::read_to_string(path).expect("read key file");
            let kf: Self = serde_json::from_str(&json).expect("parse key file");
            let bytes = hex::decode(&kf.secret_key_hex).expect("hex decode");
            let arr: [u8; 32] = bytes.try_into().expect("32-byte key");
            let kp = KeyPair::from_secret_bytes(&arr);
            (kf, kp)
        } else {
            let (kf, kp) = Self::generate();
            let json = serde_json::to_string_pretty(&kf).unwrap();
            std::fs::write(path, json).expect("write key file");
            (kf, kp)
        }
    }
}

// ─── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    // Config file: --config path, or config.toml if it exists, or empty defaults
    let cfg_path = args.config.clone().unwrap_or_else(|| PathBuf::from("config.toml"));
    let file_cfg = if cfg_path.exists() {
        NodeConfigFile::load(&cfg_path)
    } else {
        NodeConfigFile::default()
    };

    // CLI flag > config file > hardcoded default
    let block_time = args.block_time.or(file_cfg.block_time_secs).unwrap_or(3);
    let rpc_listen = args.rpc_listen.or(file_cfg.rpc_listen)
        .unwrap_or_else(|| "0.0.0.0:8545".to_string());
    let p2p_listen = args.p2p_listen.or(file_cfg.p2p_listen);
    let data_dir = args.data_dir.or(file_cfg.data_dir)
        .unwrap_or_else(|| PathBuf::from("devnet"));
    let peers = if !args.peers.is_empty() { args.peers } else { file_cfg.peers.unwrap_or_default() };
    let max_block_txs = file_cfg.max_block_txs.unwrap_or(1_000);
    let sync_peer_rpc = args.sync_peer.or(file_cfg.sync_peer_rpc);

    std::fs::create_dir_all(&data_dir).expect("create data dir");

    let validator_key_path = file_cfg.validator_key_file
        .unwrap_or_else(|| data_dir.join("validator.json"));
    let admin_key_path = file_cfg.admin_key_file
        .unwrap_or_else(|| data_dir.join("admin.json"));

    let (admin_kf, _admin_kp) = KeyFile::load_or_generate(&admin_key_path);
    let (validator_kf, validator_kp) = KeyFile::load_or_generate(&validator_key_path);

    let admin_addr: Address = admin_kf.address.parse().expect("admin address");
    let validator_addr: Address = validator_kf.address.parse().expect("validator address");

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let storage = Storage::new(&data_dir);
    let (mut state, mut chain, resumed) = match storage.load() {
        Some((s, c)) => {
            let height = s.block_height;
            tracing::info!(height, "Resuming from persisted state");
            (s, c, true)
        }
        None => {
            let state = create_genesis_state(&GenesisConfig {
                admin_address: admin_addr.clone(),
                validator_address: validator_addr.clone(),
            });
            let (chain, _genesis) = Chain::new_with_genesis(validator_addr.clone(), timestamp);
            (state, chain, false)
        }
    };

    let mut config = NodeConfig::new(validator_kp)
        .with_block_time(block_time)
        .with_rpc_listen(&rpc_listen)
        .with_data_dir(&data_dir);

    if let Some(ref p2p) = p2p_listen {
        config = config.with_p2p(p2p);
    }
    if !peers.is_empty() {
        config = config.with_peers(peers);
    }
    if let Some(ref url) = sync_peer_rpc {
        config = config.with_sync_peer(url.clone());
    }
    config.max_block_txs = max_block_txs;

    // Startup chain sync from trusted peer (if configured)
    if let Some(ref peer_url) = sync_peer_rpc {
        tracing::info!(peer = %peer_url, "Starting chain sync from peer");
        let applied = vinx_node::sync::sync_from_peer(
            peer_url,
            &mut state,
            &mut chain,
            &config.validator_set,
        ).await;
        if applied > 0 {
            tracing::info!(applied, tip = chain.tip_height(), "Chain sync complete");
        } else {
            tracing::info!("Chain sync: already up to date");
        }
    }

    print_banner(&admin_kf.address, &validator_kf.address, state.block_height, resumed, &rpc_listen);

    let node = vinx_node::Node::new_with_p2p(state, chain, config).await;

    let block_node = std::sync::Arc::clone(&node);
    tokio::spawn(async move { block_node.run_block_producer().await });

    if let Err(e) = node.run_rpc().await {
        tracing::error!(error = %e, "RPC server terminated");
        std::process::exit(1);
    }
}

fn print_banner(admin: &str, validator: &str, height: u64, resumed: bool, rpc: &str) {
    let line = "═".repeat(62);
    let mode = if resumed { format!("Reprise depuis le bloc {height}") } else { "Nouveau genesis".to_string() };
    println!("\n{line}");
    println!("  VinX Ledger — DEVNET  (RPC: {rpc})");
    println!("  {mode}");
    println!("{line}");
    println!("  Admin     : {admin}");
    println!("             21,000,000.00 VINX");
    println!("  Validator : {validator}");
    println!("{line}");
    println!("  Générer un wallet :");
    println!("    cargo run -p vinx-wallet -- keygen --output my-wallet.json");
    println!();
    println!("  Envoyer des VINX :");
    println!("    cargo run -p vinx-wallet -- transfer \\");
    println!("      --wallet devnet/admin.json --to <ADRESSE> --amount 10000");
    println!("{line}\n");
}
