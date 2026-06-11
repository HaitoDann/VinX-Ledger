use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use crate::{
    chain::Chain,
    config::NodeConfig,
    mempool::Mempool,
    p2p::P2pHandle,
    producer::produce_block,
    storage::Storage,
    NodeError,
};
use vinx_core::Block;
use vinx_state::WorldState;

pub struct Node {
    pub state: Arc<RwLock<WorldState>>,
    pub mempool: Arc<RwLock<Mempool>>,
    pub chain: Arc<RwLock<Chain>>,
    pub config: NodeConfig,
    storage: Option<Storage>,
    /// P2P handle — present when p2p_listen is configured.
    pub p2p: Option<P2pHandle>,
}

impl Node {
    pub fn new(state: WorldState, chain: Chain, config: NodeConfig) -> Arc<Self> {
        let storage = config.data_dir.as_ref().map(|p| Storage::new(p.clone()));
        Arc::new(Self {
            state: Arc::new(RwLock::new(state)),
            mempool: Arc::new(RwLock::new(Mempool::default())),
            chain: Arc::new(RwLock::new(chain)),
            config,
            storage,
            p2p: None,
        })
    }

    /// Creates a node and immediately starts the P2P layer (if configured).
    pub async fn new_with_p2p(
        state: WorldState,
        chain: Chain,
        config: NodeConfig,
    ) -> Arc<Self> {
        let storage = config.data_dir.as_ref().map(|p| Storage::new(p.clone()));
        let state_arc = Arc::new(RwLock::new(state));
        let chain_arc = Arc::new(RwLock::new(chain));
        let mempool_arc = Arc::new(RwLock::new(Mempool::default()));

        let p2p = if config.p2p_listen.is_some() {
            match crate::p2p::start(&config, Arc::clone(&chain_arc), Arc::clone(&mempool_arc)).await {
                Ok(handle) => {
                    tracing::info!(peer_id = %handle.local_peer_id, "P2P layer active");
                    Some(handle)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "P2P layer failed to start, continuing without it");
                    None
                }
            }
        } else {
            None
        };

        Arc::new(Self {
            state: state_arc,
            mempool: mempool_arc,
            chain: chain_arc,
            config,
            storage,
            p2p,
        })
    }

    /// Manually trigger block production — used in tests and by the block loop.
    pub async fn tick(&self) -> Result<Block, NodeError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut state = self.state.write().await;
        let mut chain = self.chain.write().await;
        let mut mempool = self.mempool.write().await;

        let block = produce_block(&mut state, &mut chain, &mut mempool, &self.config, timestamp)?;

        // Broadcast the new block via P2P so co-validators can sign it
        if let Some(ref p2p) = self.p2p {
            p2p.broadcast_block(&block);
        }

        Ok(block)
    }

    /// Background task: produce a block every `block_time_secs`, then persist.
    pub async fn run_block_producer(self: Arc<Self>) {
        let interval = std::time::Duration::from_secs(self.config.block_time_secs);
        loop {
            tokio::time::sleep(interval).await;
            match self.tick().await {
                Ok(block) => {
                    tracing::info!(
                        height = block.header.height,
                        txs = block.header.tx_count,
                        "Block sealed"
                    );
                    self.persist().await;
                }
                Err(e) => tracing::error!(error = %e, "Block production failed"),
            }
        }
    }

    /// Writes chain and state to disk (no-op if no data_dir configured).
    async fn persist(&self) {
        if let Some(ref storage) = self.storage {
            let state = self.state.read().await;
            let chain = self.chain.read().await;
            if let Err(e) = storage.save(&state, &chain) {
                tracing::warn!(error = %e, "Failed to persist state to disk");
            }
        }
    }

    /// Starts the HTTP RPC server (blocks until shutdown).
    pub async fn run_rpc(self: Arc<Self>) -> Result<(), NodeError> {
        let addr: std::net::SocketAddr = self
            .config
            .rpc_listen
            .parse()
            .map_err(|e: std::net::AddrParseError| NodeError::Config(e.to_string()))?;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| NodeError::Rpc(e.to_string()))?;
        self.run_rpc_on(listener).await
    }

    /// Serves the RPC on an already-bound listener.
    pub async fn run_rpc_on(
        self: Arc<Self>,
        listener: tokio::net::TcpListener,
    ) -> Result<(), NodeError> {
        let app = crate::rpc::router(Arc::clone(&self));
        tracing::info!(listen = %listener.local_addr().unwrap(), "RPC server starting");
        axum::serve(listener, app)
            .await
            .map_err(|e| NodeError::Rpc(e.to_string()))
    }
}
