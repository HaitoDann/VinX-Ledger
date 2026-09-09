use clap::Parser;
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;
use vinx_core::CHAIN_ID_DEVNET;
use vinx_crypto::{Address, KeyPair};
use vinx_node::{chain::Chain, config::NodeConfig, storage::Storage};
use vinx_state::{create_genesis_state_with_dev_prefund, GenesisConfig};

// ─── Config file ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
struct NodeConfigFile {
    block_time_secs: Option<u64>,
    max_block_txs: Option<usize>,
    max_mempool_size: Option<usize>,
    rpc_listen: Option<String>,
    data_dir: Option<PathBuf>,
    p2p_listen: Option<String>,
    peers: Option<Vec<String>>,
    validator_key_file: Option<PathBuf>,
    admin_key_file: Option<PathBuf>,
    sync_peer_rpc: Option<String>,
    admin_token: Option<String>,
    /// Path to a JSON key file for the faucet account. If the file doesn't
    /// exist it is generated automatically (the account still needs funding).
    faucet_key_file: Option<PathBuf>,
    /// Atoms to drip per faucet request (default: 100 VinX = 100 × 10¹⁸ atoms).
    faucet_amount_atoms: Option<u128>,
    /// Cooldown between faucet requests per address in seconds (default: 86 400 = 24 h).
    faucet_cooldown_secs: Option<u64>,
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
    /// Chain ID: 1=mainnet, 7=testnet, 42=devnet (default: 42)
    #[arg(long, default_value_t = CHAIN_ID_DEVNET)]
    chain_id: u32,
    /// Hardcoded bootstrap peers (repeatable, in addition to --peers)
    #[arg(long, num_args = 0..)]
    bootstrap_peers: Vec<String>,
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
        (
            Self {
                address,
                secret_key_hex,
            },
            kp,
        )
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

// ─── Genesis spec (dev/ops multi-validateurs) ──────────────────────────────────

/// Spécification de genèse **partagée** entre tous les nœuds d'un testnet — garantit une
/// genèse **déterministe et identique** (même hash) sur chaque nœud, sans quoi la sync casse
/// (elle exige `prev_hash == tip` dès la hauteur 1). Chargée via `VINX_GENESIS_SPEC=<fichier>`.
/// DEV/OPS uniquement ; le pré-financement est ignoré sur mainnet (fair launch).
#[derive(serde::Serialize, serde::Deserialize)]
struct GenesisSpec {
    chain_id: u32,
    genesis_timestamp: u64,
    admin_address: String,
    initial_validator: String,
    #[serde(default)]
    prefund_initial_validator_vinx: u128,
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
    let cfg_path = args
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from("config.toml"));
    let file_cfg = if cfg_path.exists() {
        NodeConfigFile::load(&cfg_path)
    } else {
        NodeConfigFile::default()
    };

    // CLI flag > config file > hardcoded default
    let block_time = args.block_time.or(file_cfg.block_time_secs).unwrap_or(12); // ADR 0043
    let rpc_listen = args
        .rpc_listen
        .or(file_cfg.rpc_listen)
        .unwrap_or_else(|| "0.0.0.0:8545".to_string());
    let p2p_listen = args.p2p_listen.or(file_cfg.p2p_listen);
    let data_dir = args
        .data_dir
        .or(file_cfg.data_dir)
        .unwrap_or_else(|| PathBuf::from("devnet"));
    let peers = if !args.peers.is_empty() {
        args.peers
    } else {
        file_cfg.peers.unwrap_or_default()
    };
    let bootstrap_peers = args.bootstrap_peers;
    let chain_id = args.chain_id;
    let max_block_txs = file_cfg.max_block_txs.unwrap_or(3_000);
    let max_mempool_size = file_cfg.max_mempool_size.unwrap_or(100_000);
    let sync_peer_rpc = args.sync_peer.or(file_cfg.sync_peer_rpc);

    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        eprintln!(
            "\n❌ Impossible de créer le dossier de données ({}): {e}\n",
            data_dir.display()
        );
        std::process::exit(1);
    }

    let validator_key_path = file_cfg
        .validator_key_file
        .unwrap_or_else(|| data_dir.join("validator.json"));
    let admin_key_path = file_cfg
        .admin_key_file
        .unwrap_or_else(|| data_dir.join("admin.json"));

    let (admin_kf, _admin_kp) = KeyFile::load_or_generate(&admin_key_path);
    let (validator_kf, validator_kp) = KeyFile::load_or_generate(&validator_key_path);

    let admin_addr: Address = admin_kf.address.parse().expect("admin address");
    let validator_addr: Address = validator_kf.address.parse().expect("validator address");

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let storage = match Storage::open(&data_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "\n❌ Impossible d'ouvrir le stockage ({}): {e}\n",
                data_dir.display()
            );
            std::process::exit(1);
        }
    };
    let (mut state, mut chain, resumed) = match storage.load() {
        Some((s, c)) => {
            let height = s.block_height;
            tracing::info!(height, "Resuming from persisted state");
            (s, c, true)
        }
        None => {
            // DEV/OPS : genèse partagée via VINX_GENESIS_SPEC → tous les nœuds calculent une
            // genèse identique (même hash), prérequis pour que la sync multi-nœuds fonctionne.
            if let Ok(spec_path) = std::env::var("VINX_GENESIS_SPEC") {
                let raw = std::fs::read_to_string(&spec_path).expect("read genesis spec");
                let spec: GenesisSpec = serde_json::from_str(&raw).expect("parse genesis spec");
                let admin: Address = spec.admin_address.parse().expect("spec admin address");
                let init_val: Address = spec
                    .initial_validator
                    .parse()
                    .expect("spec initial_validator");
                let cfg = GenesisConfig {
                    admin_address: admin,
                    validator_address: init_val,
                    chain_id: spec.chain_id,
                };
                let prefund_atoms = spec
                    .prefund_initial_validator_vinx
                    .saturating_mul(vinx_core::amount::DECIMAL_FACTOR);
                tracing::warn!(
                    spec = %spec_path,
                    initial_validator = %spec.initial_validator,
                    "⚠ Genèse construite depuis une spec partagée (testnet dev)"
                );
                let state = create_genesis_state_with_dev_prefund(&cfg, prefund_atoms);
                let (chain, _genesis) = Chain::new_with_genesis(init_val, spec.genesis_timestamp);
                (state, chain, false)
            } else {
                let genesis_cfg = GenesisConfig {
                    admin_address: admin_addr,
                    validator_address: validator_addr,
                    chain_id,
                };
                // VINX_DEV_PREFUND_VINX : pré-finance le validateur de genèse (mono-nœud dev).
                let dev_prefund_atoms = std::env::var("VINX_DEV_PREFUND_VINX")
                    .ok()
                    .and_then(|v| v.parse::<u128>().ok())
                    .map(|vinx| vinx.saturating_mul(vinx_core::amount::DECIMAL_FACTOR))
                    .unwrap_or(0);
                let state = create_genesis_state_with_dev_prefund(&genesis_cfg, dev_prefund_atoms);
                let (chain, _genesis) = Chain::new_with_genesis(validator_addr, timestamp);
                (state, chain, false)
            }
        }
    };

    // Read the persisted mempool now, then release this Storage handle: the Node
    // opens the same redb file itself, and redb forbids two open handles to one
    // database within a process (DatabaseAlreadyOpen).
    let restored_mempool = storage.load_mempool();
    drop(storage);

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
    if !bootstrap_peers.is_empty() {
        config = config.with_bootstrap_peers(bootstrap_peers);
    }
    config = config.with_chain_id(chain_id);
    if let Some(ref url) = sync_peer_rpc {
        config = config.with_sync_peer(url.clone());
    }
    if let Some(token) = file_cfg.admin_token {
        config = config.with_admin_token(token);
    } else {
        tracing::warn!(
            "No admin_token configured — the admin routes (/snapshot, /admin/compact, \
             /validators/pending) are disabled. Set admin_token in config.toml to enable them."
        );
    }
    config.max_block_txs = max_block_txs;
    config.max_mempool_size = max_mempool_size;

    // Optional faucet — only enabled when faucet_key_file is set in config.toml.
    if let Some(ref faucet_path) = file_cfg.faucet_key_file {
        let (faucet_kf, faucet_kp) = KeyFile::load_or_generate(faucet_path);
        let amount = file_cfg
            .faucet_amount_atoms
            .unwrap_or(100 * 1_000_000_000_000_000_000);
        let cooldown = file_cfg.faucet_cooldown_secs.unwrap_or(86_400);
        tracing::info!(
            address = %faucet_kf.address,
            amount_atoms = amount,
            cooldown_secs = cooldown,
            "Faucet enabled"
        );
        config = config.with_faucet(faucet_kp, amount, cooldown);
    }

    // Startup chain sync from trusted peer (if configured)
    if let Some(ref peer_url) = sync_peer_rpc {
        tracing::info!(peer = %peer_url, "Starting chain sync from peer");
        // 1. Snapshot bootstrap — skip replaying history from genesis if gap > 500.
        vinx_node::sync::snapshot_sync_from_peer(peer_url, &mut state, &mut chain).await;
        // 2. Parallel catch-up — concurrent batch downloads, sequential apply.
        let applied =
            vinx_node::sync::parallel_sync_from_peer(peer_url, &mut state, &mut chain).await;
        // 3. Sequential tail — handles the last few unfinalized blocks the parallel
        //    pass may have stopped at (validate_block rejects non-finalized blocks).
        let tail = vinx_node::sync::sync_from_peer(peer_url, &mut state, &mut chain).await;
        let applied = applied + tail;
        if applied > 0 {
            tracing::info!(applied, tip = chain.tip_height(), "Chain sync complete");
        } else {
            tracing::info!("Chain sync: already up to date");
        }
    }

    let faucet_addr = file_cfg.faucet_key_file.as_ref().and_then(|p| {
        let json = std::fs::read_to_string(p).ok()?;
        let kf: KeyFile = serde_json::from_str(&json).ok()?;
        Some(kf.address)
    });
    print_banner(
        &admin_kf.address,
        &validator_kf.address,
        faucet_addr.as_deref(),
        state.block_height,
        resumed,
        &rpc_listen,
    );

    let node = vinx_node::Node::new_with_p2p(state, chain, config).await;

    // Restore mempool from last persist — re-validate each tx against current state.
    if let Some(txs) = restored_mempool {
        let total = txs.len();
        if total > 0 {
            let mut mp = node.mempool.write().await;
            let mut restored = 0usize;
            for tx in txs {
                if mp.add(tx).is_ok() {
                    restored += 1;
                }
            }
            tracing::info!(restored, total, "Mempool restored from disk");
        }
    }

    let block_node = std::sync::Arc::clone(&node);
    tokio::spawn(async move { block_node.run_block_producer().await });

    if let Err(e) = node.run_rpc().await {
        tracing::error!(error = %e, "RPC server terminated");
        std::process::exit(1);
    }
}

fn print_banner(
    admin: &str,
    validator: &str,
    faucet: Option<&str>,
    height: u64,
    resumed: bool,
    rpc: &str,
) {
    let line = "═".repeat(62);
    let mode = if resumed {
        format!("Reprise depuis le bloc {height}")
    } else {
        "Nouveau genesis".to_string()
    };
    println!("\n{line}");
    println!("  VinX Ledger — DEVNET  (RPC: {rpc})");
    println!("  {mode}");
    println!("{line}");
    println!("  Admin     : {admin}");
    println!(
        "               (fair launch — aucun pre-mine ; minting progressif, demi-vie ~20 ans)"
    );
    println!("  Validator : {validator}");
    if let Some(fa) = faucet {
        println!("  Faucet    : {fa}");
        println!("    POST {rpc}/faucet/request  {{\"address\":\"vinx1...\"}}");
    } else {
        println!("  Faucet    : désactivé  (ajouter faucet_key_file dans config.toml)");
    }
    println!("{line}");
    println!("  Générer un wallet :");
    println!("    cargo run -p vinx-wallet -- keygen --output my-wallet.json");
    println!();
    println!("  Envoyer des VINX :");
    println!("    cargo run -p vinx-wallet -- transfer \\");
    println!("      --wallet devnet/admin.json --to <ADRESSE> --amount 10000");
    println!("{line}\n");
}
