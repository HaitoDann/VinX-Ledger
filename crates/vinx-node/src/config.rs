use vinx_crypto::{Address, KeyPair};

#[derive(Clone)]
pub struct NodeConfig {
    /// Ed25519 keypair used to sign produced blocks.
    pub validator_keypair: KeyPair,
    /// Address of the validator derived from the keypair.
    pub validator_address: Address,
    /// Target block time in seconds (10 for mainnet, lower for testing).
    pub block_time_secs: u64,
    /// Maximum transactions per block.
    pub max_block_txs: usize,
    /// HTTP listen address for the RPC server.
    pub rpc_listen: String,
}

impl NodeConfig {
    pub fn new(validator_keypair: KeyPair) -> Self {
        let validator_address = Address::from_public_key(&validator_keypair.public_key());
        Self {
            validator_keypair,
            validator_address,
            block_time_secs: 10,
            max_block_txs: 1_000,
            rpc_listen: "127.0.0.1:8545".to_string(),
        }
    }

    pub fn with_block_time(mut self, secs: u64) -> Self {
        self.block_time_secs = secs;
        self
    }

    pub fn with_rpc_listen(mut self, addr: impl Into<String>) -> Self {
        self.rpc_listen = addr.into();
        self
    }
}
