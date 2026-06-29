use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::{
    chain::Chain,
    config::NodeConfig,
    mempool::Mempool,
    p2p::P2pHandle,
    producer::{produce_block, produce_block_backup},
    storage::Storage,
    NodeError,
};
use vinx_core::{Block, ValidatorSet};
use vinx_state::WorldState;

// ─── Transaction receipt ─────────────────────────────────────────────────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TxReceipt {
    pub tx_hash: String,
    pub block_height: u64,
    pub success: bool,
    pub error: Option<String>,
}

// ─── Validator join request ───────────────────────────────────────────────────

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidatorJoinRequest {
    pub address: String,
    pub p2p_multiaddr: Option<String>,
    pub message: Option<String>,
    pub submitted_at: u64,
}

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
    /// Timestamp of the last block produced or received — used for slot-skip logic.
    pub last_block_instant: Arc<RwLock<Instant>>,
    /// Last block height seen from each validator address (liveness tracking).
    pub validator_liveness: Arc<RwLock<HashMap<String, u64>>>,
    /// Pending validator join requests (in-memory, not persisted).
    pub validator_requests: Arc<Mutex<Vec<ValidatorJoinRequest>>>,
    /// Transaction receipts indexed by hex-encoded tx hash.
    pub receipts: Arc<RwLock<HashMap<String, TxReceipt>>>,
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
            last_block_instant: Arc::new(RwLock::new(Instant::now())),
            validator_liveness: Arc::new(RwLock::new(HashMap::new())),
            validator_requests: Arc::new(Mutex::new(Vec::new())),
            receipts: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Creates a node and immediately starts the P2P layer (if configured).
    pub async fn new_with_p2p(state: WorldState, chain: Chain, config: NodeConfig) -> Arc<Self> {
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
            )
            .await
            {
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
            last_block_instant: Arc::new(RwLock::new(Instant::now())),
            validator_liveness: Arc::new(RwLock::new(HashMap::new())),
            validator_requests: Arc::new(Mutex::new(Vec::new())),
            receipts: Arc::new(RwLock::new(HashMap::new())),
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

        mempool.prune_expired(state.block_height);

        let block = produce_block(
            &mut state,
            &mut chain,
            &mut mempool,
            &self.config,
            &vs,
            timestamp,
        )?;

        let receipts = block
            .transactions
            .iter()
            .map(|tx| {
                let hash = hex::encode(tx.hash());
                (
                    hash.clone(),
                    TxReceipt {
                        tx_hash: hash,
                        block_height: block.header.height,
                        success: true,
                        error: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        self.receipts.write().await.extend(receipts);

        self.after_block_produced(&block, &state.validator_set)
            .await;
        Ok(block)
    }

    /// Produces a block as a backup validator (slot skip — the scheduled leader is offline).
    async fn tick_as_backup(&self) -> Result<Block, NodeError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut state = self.state.write().await;
        let mut chain = self.chain.write().await;
        let mut mempool = self.mempool.write().await;
        let vs = self.validator_set.read().await.clone();

        let block = produce_block_backup(
            &mut state,
            &mut chain,
            &mut mempool,
            &self.config,
            &vs,
            timestamp,
        )?;

        self.after_block_produced(&block, &state.validator_set)
            .await;
        Ok(block)
    }

    /// Common bookkeeping after any block is produced by this node.
    async fn after_block_produced(&self, block: &Block, new_vs: &ValidatorSet) {
        // Update validator set from state (AddValidator/RemoveValidator tx effects)
        *self.validator_set.write().await = new_vs.clone();

        // Record liveness for this node
        self.validator_liveness.write().await.insert(
            self.config.validator_address.to_string(),
            block.header.height,
        );

        // Reset the slot-timeout clock
        *self.last_block_instant.write().await = Instant::now();

        // Broadcast SSE event
        let event = BlockEvent {
            height: block.header.height,
            tx_count: block.header.tx_count,
            hash_hex: hex::encode(block.hash()),
        };
        let _ = self.block_events.send(event);

        // Broadcast to P2P peers
        if let Some(ref p2p) = self.p2p {
            p2p.broadcast_block(block);
        }
    }

    /// Checks whether this node should step in as a backup producer because
    /// the scheduled leader has not produced within the slot timeout window.
    ///
    /// Backup validators activate in round-robin order after the stuck leader:
    /// - validator at distance 1 activates after `2 × block_time`
    /// - validator at distance 2 activates after `3 × block_time`
    /// - etc.
    async fn try_backup_production(&self) {
        let block_time = self.config.block_time_secs;
        let elapsed = self.last_block_instant.read().await.elapsed().as_secs();

        // Grace period: at least 2 full slot-times must have elapsed
        if elapsed < 2 * block_time {
            return;
        }

        let vs = self.validator_set.read().await.clone();
        let n = vs.len();
        if n <= 1 {
            return; // Single-validator chain — can't skip yourself
        }

        let height = self.chain.read().await.tip_height() + 1;
        let leader_idx = vs.leader_idx_at(height);

        let my_idx = match vs.index_of(&self.config.validator_address) {
            Some(i) => i,
            None => return, // Not a validator
        };

        if my_idx == leader_idx {
            return; // We ARE the scheduled leader — tick() will handle this
        }

        // My position in the backup queue (1 = first backup, 2 = second, …)
        let distance = (my_idx + n - leader_idx) % n;

        // Each backup activates one slot-time later than the previous
        let activation_secs = (distance as u64 + 1) * block_time;
        if elapsed >= activation_secs {
            tracing::warn!(
                height,
                scheduled_leader = %vs.leader_at(height),
                my_addr = %self.config.validator_address,
                elapsed_secs = elapsed,
                "Slot timeout — stepping in as backup producer (distance {})",
                distance
            );
            match self.tick_as_backup().await {
                Ok(b) => {
                    tracing::info!(height = b.header.height, "Backup block produced");
                    self.persist().await;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "Backup production attempt failed");
                }
            }
        }
    }

    /// Background task: produce blocks on demand (event-driven) with a heartbeat fallback.
    ///
    /// Workflow:
    ///  - Sleeps until the mempool signals a new transaction (`tx_ready` Notify).
    ///  - Waits a short batch window so concurrent submissions land in the same block.
    ///  - Falls back to a heartbeat block every HEARTBEAT_INTERVAL_SECS when idle,
    ///    keeping height-based timers (freeze expiry, upgrade activation) advancing.
    ///
    /// Implements **slot skip**: if the scheduled leader is offline, backup validators
    /// step in after 2, 3, … block-times so the chain keeps advancing.
    pub async fn run_block_producer(self: Arc<Self>) {
        use vinx_core::amount::{BATCH_WINDOW_MS, HEARTBEAT_INTERVAL_SECS};

        let block_time = std::time::Duration::from_secs(self.config.block_time_secs);
        let batch_window = std::time::Duration::from_millis(BATCH_WINDOW_MS);
        let heartbeat = std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS);

        // Extract the Notify handle once — no lock held while awaiting.
        let tx_ready = self.mempool.read().await.tx_ready.clone();

        loop {
            // Wait for either a new transaction or the heartbeat deadline.
            let triggered_by_tx = tokio::select! {
                _ = tx_ready.notified() => true,
                _ = tokio::time::sleep(heartbeat) => false,
            };

            if triggered_by_tx {
                // Brief batch window: let concurrent txs accumulate before sealing.
                tokio::time::sleep(batch_window).await;
                tracing::debug!("Block triggered by transaction");
            } else {
                tracing::debug!("Heartbeat block — mempool idle for {HEARTBEAT_INTERVAL_SECS}s");
            }

            match self.tick().await {
                Ok(block) => {
                    tracing::info!(
                        height = block.header.height,
                        txs = block.header.tx_count,
                        base_fee = block.header.base_fee,
                        "Block sealed"
                    );
                    self.persist().await;
                    // Respect block time before accepting the next production round.
                    if triggered_by_tx {
                        tokio::time::sleep(block_time).await;
                    }
                }
                Err(NodeError::Consensus(_)) => {
                    self.try_backup_production().await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "Block production failed");
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
