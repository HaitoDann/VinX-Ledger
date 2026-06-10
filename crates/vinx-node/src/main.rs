use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;
use vinx_crypto::{Address, KeyPair};
use vinx_node::{chain::Chain, config::NodeConfig};
use vinx_state::{create_genesis_state, GenesisConfig};

// Keystore JSON — same format as vinx-wallet so keys are directly usable
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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let devnet_dir = PathBuf::from("devnet");
    std::fs::create_dir_all(&devnet_dir).expect("create devnet/");

    let (admin_kf, _admin_kp) =
        KeyFile::load_or_generate(&devnet_dir.join("admin.json"));
    let (validator_kf, validator_kp) =
        KeyFile::load_or_generate(&devnet_dir.join("validator.json"));

    let admin_addr: Address = admin_kf.address.parse().expect("admin address");
    let validator_addr: Address = validator_kf.address.parse().expect("validator address");

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let state = create_genesis_state(&GenesisConfig {
        admin_address: admin_addr.clone(),
    });

    let (chain, _genesis) = Chain::new_with_genesis(validator_addr.clone(), timestamp);

    let config = NodeConfig::new(validator_kp)
        .with_block_time(3)
        .with_rpc_listen("0.0.0.0:8545");

    print_banner(&admin_kf.address, &validator_kf.address);

    let node = vinx_node::Node::new(state, chain, config);

    let block_node = std::sync::Arc::clone(&node);
    tokio::spawn(async move { block_node.run_block_producer().await });

    if let Err(e) = node.run_rpc().await {
        tracing::error!(error = %e, "RPC server terminated");
        std::process::exit(1);
    }
}

fn print_banner(admin: &str, validator: &str) {
    let line = "═".repeat(60);
    println!("\n{line}");
    println!("  VinX Ledger — DEVNET  (block time: 3s | RPC: :8545)");
    println!("{line}");
    println!("  Admin     : {admin}");
    println!("             21,000,000.00 VINX — clé dans devnet/admin.json");
    println!("  Validator : {validator}");
    println!("{line}");
    println!("  Générer un wallet :");
    println!("    cargo run -p vinx-wallet -- keygen --output my-wallet.json");
    println!();
    println!("  Vérifier l'adresse :");
    println!("    cargo run -p vinx-wallet -- address --wallet my-wallet.json");
    println!();
    println!("  Envoyer des VINX depuis l'admin vers votre wallet :");
    println!("    cargo run -p vinx-wallet -- transfer \\");
    println!("      --wallet devnet/admin.json \\");
    println!("      --to <VOTRE_ADRESSE> \\");
    println!("      --amount 10000");
    println!();
    println!("  Vérifier le solde :");
    println!("    cargo run -p vinx-wallet -- balance <VOTRE_ADRESSE>");
    println!("{line}\n");
}
