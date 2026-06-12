use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::{
    chain::Chain,
    config::NodeConfig,
    mempool::Mempool,
    p2p::P2pHandle,
    producer::produce_block,
    storage::Storage,
    NodeError,
};
use vinx_core::{Block, ValidatorSet};
use vinx_state::WorldState;

/// Events broadcast to SSE subscribers on each produced block.
#[derive(Clone, Debug)]
pub struct BlockEvent {
    pub height: u64,
    pub tx_count: u32,
    pub hash_hex: String,
}

pub struct Node {
    pub state: Arc<RwLock<WorldState>>,
    pub mempool: Arc<RwLock<Mempool>>,
    pub chain: Arc<RwLock<Chain>>,
    pub config: NodeConfig,
    storage: Option<Storage>,
    /// P2P handle — present when p2p_listen is configured.
    pub p2p: Option<P2pHandle>,
    /// Live validator set — updated after each block that modifies it.
    pub validator_set: Arc<RwLock<ValidatorSet>>,
    /// Broadcast channel for new-block SSE events.
    pub block_events: broadcast::Sender<BlockEvent>,
    /// Per-address faucet cooldown tracker + serialization lock for faucet requests.
    pub faucet_cooldowns: Arc<Mutex<HashMap<String, Instant>>>,
}

impl Node {
    pub fn new(state: WorldState, chain: Chain, config: NodeConfig) -> Arc<Self> {
        let storage = config.data_dir.as_ref().map(|p| Storage::new(p.clone()));
        let initial_vs = state.validator_set.clone();
        let (block_events, _) = broadcast::channel(64);
        Arc::new(Self {
            state: Arc::new(RwLock::new(state)),
            mempool: Arc::new(RwLock::new(Mempool::default())),
            chain: Arc::new(RwLock::new(chain)),
            validator_set: Arc::new(RwLock::new(initial_vs)),
            config,
            storage,
            p2p: None,
            block_events,
            faucet_cooldowns: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Creates a node and immediately starts the P2P layer (if configured).
    pub async fn new_with_p2p(
        state: WorldState,
        chain: Chain,
        config: NodeConfig,
    ) -> Arc<Self> {
        let storage = config.data_dir.as_ref().map(|p| Storage::new(p.clone()));
        let initial_vs = state.validator_set.clone();
        let state_arc = Arc::new(RwLock::new(state));
        let chain_arc = Arc::new(RwLock::new(chain));
        let mempool_arc = Arc::new(RwLock::new(Mempool::default()));
        let vs_arc = Arc::new(RwLock::new(initial_vs));
        let (block_events, _) = broadcast::channel(64);

        let p2p = if config.p2p_listen.is_some() {
            match crate::p2p::start(
                &config,
                Arc::clone(&chain_arc),
                Arc::clone(&mempool_arc),
                Arc::clone(&state_arc),
                Arc::clone(&vs_arc),
            ).await {
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
            validator_set: vs_arc,
            config,
            storage,
            p2p,
            block_events,
            faucet_cooldowns: Arc::new(Mutex::new(HashMap::new())),
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
        let vs = self.validator_set.read().await.clone();

        let block = produce_block(&mut state, &mut chain, &mut mempool, &self.config, &vs, timestamp)?;

        // Sync validator set from state (may have changed if AddValidator/RemoveValidator was applied)
        let new_vs = state.validator_set.clone();
        drop(state);
        drop(chain);
        drop(mempool);
        *self.validator_set.write().await = new_vs;

        // Broadcast new-block event to SSE subscribers
        let event = BlockEvent {
            height: block.header.height,
            tx_count: block.header.tx_count,
            hash_hex: hex::encode(block.hash()),
        };
        let _ = self.block_events.send(event);

        // Broadcast the new block via P2P so co-validators can sign it
        if let Some(ref p2p) = self.p2p {
            p2p.broadcast_block(&block);
        }

        Ok(block)
    }

    /// Background task: produce a block every `block_time_secs`, then persist.
    /// Detects leader timeouts when the expected leader hasn't produced for 3+ consecutive slots.
    pub async fn run_block_producer(self: Arc<Self>) {
        let interval = std::time::Duration::from_secs(self.config.block_time_secs);
        let mut missed_slots: u32 = 0;

        loop {
            tokio::time::sleep(interval).await;
            match self.tick().await {
                Ok(block) => {
                    missed_slots = 0;
                    tracing::info!(
                        height = block.header.height,
                        txs = block.header.tx_count,
                        base_fee = block.header.base_fee,
                        "Block sealed"
                    );
                    self.persist().await;
                }
                Err(NodeError::Consensus(_)) => {
                    // Not our slot — track how long the expected leader has been missing
                    missed_slots += 1;
                    if missed_slots >= 3 {
                        let vs = self.validator_set.read().await.clone();
                        let height = self.chain.read().await.tip_height() + 1;
                        let leader = vs.leader_at(height);
                        tracing::warn!(
                            height,
                            leader = %leader,
                            missed_slots,
                            "Leader timeout — expected leader has not produced for {} consecutive slots",
                            missed_slots
                        );
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Block production failed");
                    missed_slots = 0;
                }
            }
        }
    }

    /// Writes chain and state to disk (no-op if no data_dir configured).
    pub async fn persist(&self) {
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
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
            .await
            .map_err(|e| NodeError::Rpc(e.to_string()))
    }
}
