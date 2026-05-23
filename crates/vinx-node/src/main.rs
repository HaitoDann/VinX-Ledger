use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use vinx_crypto::{Address, KeyPair};
use vinx_node::{chain::Chain, config::NodeConfig, Node};
use vinx_state::{create_genesis_state, GenesisConfig};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    // In production these would be loaded from config / secure key storage.
    let validator_kp = KeyPair::generate();
    let admin_kp = KeyPair::generate();
    let pool_kp = KeyPair::generate();

    let validator_addr = Address::from_public_key(&validator_kp.public_key());
    let admin_addr = Address::from_public_key(&admin_kp.public_key());
    let pool_addr = Address::from_public_key(&pool_kp.public_key());

    tracing::info!(address = %validator_addr, "Validator identity");
    tracing::info!(address = %admin_addr, "Admin account (500M VINX)");
    tracing::info!(address = %pool_addr, "Public sale pool");

    let genesis_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let state = create_genesis_state(&GenesisConfig {
        admin_address: admin_addr,
        reserve_address: validator_addr.clone(),
    });

    let (chain, genesis) = Chain::new_with_genesis(validator_addr.clone(), genesis_timestamp);
    tracing::info!(hash = %hex::encode(genesis.hash()), "Genesis block created");

    let config = NodeConfig::new(validator_kp, pool_addr)
        .with_block_time(10)
        .with_rpc_listen("0.0.0.0:8545");

    let node = Node::new(state, chain, config);

    tracing::info!("Starting VinX node…");

    let block_node = Arc::clone(&node);
    tokio::spawn(async move { block_node.run_block_producer().await });

    if let Err(e) = node.run_rpc().await {
        tracing::error!(error = %e, "RPC server terminated");
        std::process::exit(1);
    }
}
