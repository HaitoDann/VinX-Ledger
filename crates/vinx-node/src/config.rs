use std::path::PathBuf;
use vinx_core::{chain_id::CHAIN_ID_DEVNET, ValidatorSet};
use vinx_crypto::{Address, KeyPair};

#[derive(Clone)]
pub struct NodeConfig {
    /// Ed25519 keypair used to sign produced blocks.
    pub validator_keypair: KeyPair,
    /// Address of the validator derived from the keypair.
    pub validator_address: Address,
    /// Authorized validator set used for leader selection and quorum checks.
    pub validator_set: ValidatorSet,
    /// Target block time in seconds (10 for mainnet, lower for testing).
    pub block_time_secs: u64,
    /// Maximum transactions per block.
    pub max_block_txs: usize,
    /// HTTP listen address for the RPC server.
    pub rpc_listen: String,
    /// Directory for persistent chain and state data. `None` = in-memory only.
    pub data_dir: Option<PathBuf>,
    /// P2P listen address (e.g. "/ip4/0.0.0.0/tcp/9000"). `None` = P2P disabled.
    pub p2p_listen: Option<String>,
    /// Bootstrap peer multiaddrs to dial on startup.
    pub peer_addrs: Vec<String>,
    /// RPC URL of a trusted peer to sync blocks from on startup. `None` = no sync.
    pub sync_peer_rpc: Option<String>,
    /// Bearer token required for admin routes. `None` = no auth required.
    pub admin_token: Option<String>,
    /// Keypair used to sign faucet transfer transactions. `None` = faucet disabled.
    pub faucet_keypair: Option<KeyPair>,
    /// Atoms to drip per faucet request (default: 100 VinX).
    pub faucet_amount_atoms: u128,
    /// Seconds a given address must wait between faucet requests (default: 86 400 = 24 h).
    pub faucet_cooldown_secs: u64,
    /// Chain ID for transaction replay protection (1 = mainnet, 7 = testnet, 42 = devnet).
    pub chain_id: u32,
    /// Bootstrap peer multiaddrs dialed on P2P startup for initial peer discovery.
    pub bootstrap_peers: Vec<String>,
}

impl NodeConfig {
    /// Creates a single-validator (dev) config.
    pub fn new(validator_keypair: KeyPair) -> Self {
        let validator_address = Address::from_public_key(&validator_keypair.public_key());
        let validator_set = ValidatorSet::single(validator_address);
        Self {
            validator_keypair,
            validator_address,
            validator_set,
            block_time_secs: 10,
            max_block_txs: 1_000,
            rpc_listen: "127.0.0.1:8545".to_string(),
            data_dir: None,
            p2p_listen: None,
            peer_addrs: vec![],
            sync_peer_rpc: None,
            admin_token: None,
            faucet_keypair: None,
            faucet_amount_atoms: 100 * 1_000_000_000_000_000_000, // 100 VinX
            faucet_cooldown_secs: 86_400,
            chain_id: CHAIN_ID_DEVNET,
            bootstrap_peers: vec![],
        }
    }

    pub fn with_validator_set(mut self, validator_set: ValidatorSet) -> Self {
        self.validator_set = validator_set;
        self
    }

    pub fn with_p2p(mut self, listen: impl Into<String>) -> Self {
        self.p2p_listen = Some(listen.into());
        self
    }

    pub fn with_peers(mut self, peers: Vec<String>) -> Self {
        self.peer_addrs = peers;
        self
    }

    pub fn with_block_time(mut self, secs: u64) -> Self {
        self.block_time_secs = secs;
        self
    }

    pub fn with_rpc_listen(mut self, addr: impl Into<String>) -> Self {
        self.rpc_listen = addr.into();
        self
    }

    pub fn with_data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(dir.into());
        self
    }

    pub fn with_sync_peer(mut self, url: impl Into<String>) -> Self {
        self.sync_peer_rpc = Some(url.into());
        self
    }

    pub fn with_admin_token(mut self, token: impl Into<String>) -> Self {
        self.admin_token = Some(token.into());
        self
    }

    pub fn with_faucet(mut self, keypair: KeyPair, amount_atoms: u128, cooldown_secs: u64) -> Self {
        self.faucet_keypair = Some(keypair);
        self.faucet_amount_atoms = amount_atoms;
        self.faucet_cooldown_secs = cooldown_secs;
        self
    }

    pub fn with_chain_id(mut self, chain_id: u32) -> Self {
        self.chain_id = chain_id;
        self
    }

    pub fn with_bootstrap_peers(mut self, peers: Vec<String>) -> Self {
        self.bootstrap_peers = peers;
        self
    }
}
