use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use crate::{
    chain::Chain,
    config::NodeConfig,
    mempool::Mempool,
    producer::produce_block,
    NodeError,
};
use vinx_core::Block;
use vinx_state::WorldState;

pub struct Node {
    pub state: Arc<RwLock<WorldState>>,
    pub mempool: Arc<RwLock<Mempool>>,
    pub chain: Arc<RwLock<Chain>>,
    pub config: NodeConfig,
}

impl Node {
    pub fn new(state: WorldState, chain: Chain, config: NodeConfig) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(RwLock::new(state)),
            mempool: Arc::new(RwLock::new(Mempool::default())),
            chain: Arc::new(RwLock::new(chain)),
            config,
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

        produce_block(&mut state, &mut chain, &mut mempool, &self.config, timestamp)
    }

    /// Background task: produce a block every `block_time_secs`.
    pub async fn run_block_producer(self: Arc<Self>) {
        let interval = std::time::Duration::from_secs(self.config.block_time_secs);
        loop {
            tokio::time::sleep(interval).await;
            match self.tick().await {
                Ok(block) => tracing::info!(
                    height = block.header.height,
                    txs = block.header.tx_count,
                    "Block sealed"
                ),
                Err(e) => tracing::error!(error = %e, "Block production failed"),
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
    /// Used by integration tests to bind on port 0 without a race condition.
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
