use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// Real-time counters updated by the node as events occur.
/// All fields are atomics — reads in `GET /metrics` never acquire any lock.
#[derive(Clone)]
pub struct NodeMetrics {
    inner: Arc<NodeMetricsInner>,
}

pub struct NodeMetricsInner {
    pub blocks_produced: AtomicU64,
    /// Transactions successfully admitted to the mempool via RPC.
    pub tx_submitted_ok: AtomicU64,
    /// Transactions rejected at the RPC layer (bad sig, mempool full, etc.).
    pub tx_submitted_err: AtomicU64,
    /// Total transactions included in produced blocks.
    pub tx_in_block: AtomicU64,
    /// Blocks received via P2P gossip.
    pub p2p_blocks_recv: AtomicU64,
    /// Transactions received via P2P gossip.
    pub p2p_tx_recv: AtomicU64,
    /// Requests rejected by the rate limiter.
    pub ratelimit_hit: AtomicU64,
    /// Unix timestamp (seconds) of the last block produced or received.
    pub last_block_secs: AtomicU64,
}

impl NodeMetrics {
    fn new() -> Self {
        Self {
            inner: Arc::new(NodeMetricsInner {
                blocks_produced: AtomicU64::new(0),
                tx_submitted_ok: AtomicU64::new(0),
                tx_submitted_err: AtomicU64::new(0),
                tx_in_block: AtomicU64::new(0),
                p2p_blocks_recv: AtomicU64::new(0),
                p2p_tx_recv: AtomicU64::new(0),
                ratelimit_hit: AtomicU64::new(0),
                last_block_secs: AtomicU64::new(0),
            }),
        }
    }
}

// Delegate atomic accessors through the Arc so NodeMetrics can be freely cloned
// and shared between Node and RateLimiter without extra indirection.
impl std::ops::Deref for NodeMetrics {
    type Target = NodeMetricsInner;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Events broadcast to SSE/WebSocket subscribers on each produced block.
#[derive(Clone, Debug)]
pub struct BlockEvent {
    pub height: u64,
    pub tx_count: u32,
    pub hash_hex: String,
    /// Dynamic base fee at the time of this block, in atoms.
    pub base_fee_atoms: u64,
    /// Bech32 address of the validator that produced this block.
    pub proposer: String,
    /// Hex-encoded account state root after this block.
    pub state_root_hex: String,
}

pub struct Node {
    pub state: Arc<RwLock<WorldState>>,
    pub mempool: Arc<RwLock<Mempool>>,
    pub chain: Arc<RwLock<Chain>>,
    pub config: NodeConfig,
    storage: Option<Arc<Storage>>,
    /// P2P handle — present when p2p_listen is configured.
    pub p2p: Option<P2pHandle>,
    /// Live validator set — updated after each block that modifies it.
    pub validator_set: Arc<RwLock<ValidatorSet>>,
    /// Broadcast channel for new-block SSE/WebSocket events.
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
    pub receipts: Arc<RwLock<LruCache<String, TxReceipt>>>,
    /// LRU cache for bech32 address decoding — avoids re-parsing on every RPC request.
    pub address_cache: tokio::sync::Mutex<LruCache<String, vinx_crypto::Address>>,
    /// Lock-free real-time counters exposed on GET /metrics.
    pub metrics: NodeMetrics,
    /// Validators temporarily suspended from the round-robin due to liveness eviction.
    pub suspended_validators: Arc<RwLock<HashSet<String>>>,
}

impl Node {
    pub fn new(state: WorldState, chain: Chain, config: NodeConfig) -> Arc<Self> {
        let storage = config
            .data_dir
            .as_ref()
            .map(|p| Arc::new(Storage::new(p.clone())));
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
            receipts: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(100_000).unwrap(),
            ))),
            address_cache: tokio::sync::Mutex::new(LruCache::new(
                NonZeroUsize::new(1_024).unwrap(),
            )),
            metrics: NodeMetrics::new(),
            suspended_validators: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    /// Creates a node and immediately starts the P2P layer (if configured).
    pub async fn new_with_p2p(state: WorldState, chain: Chain, config: NodeConfig) -> Arc<Self> {
        let storage = config
            .data_dir
            .as_ref()
            .map(|p| Arc::new(Storage::new(p.clone())));
        let initial_vs = state.validator_set.clone();
        let state_arc = Arc::new(RwLock::new(state));
        let chain_arc = Arc::new(RwLock::new(chain));
        let mempool_arc = Arc::new(RwLock::new(Mempool::default()));
        let vs_arc = Arc::new(RwLock::new(initial_vs));
        let (block_events, _) = broadcast::channel(64);

        let metrics = NodeMetrics::new();

        let p2p = if config.p2p_listen.is_some() {
            match crate::p2p::start(
                &config,
                Arc::clone(&chain_arc),
                Arc::clone(&mempool_arc),
                Arc::clone(&state_arc),
                Arc::clone(&vs_arc),
                metrics.clone(),
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
            receipts: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(100_000).unwrap(),
            ))),
            address_cache: tokio::sync::Mutex::new(LruCache::new(
                NonZeroUsize::new(1_024).unwrap(),
            )),
            metrics,
            suspended_validators: Arc::new(RwLock::new(HashSet::new())),
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

        // Flush mempool entries whose nonce is now consumed by this block.
        {
            let mut confirmed_nonces = HashMap::new();
            for tx in &block.transactions {
                confirmed_nonces.insert(tx.from.to_string(), tx.nonce + 1);
            }
            if !confirmed_nonces.is_empty() {
                mempool.update_confirmed_nonces(&confirmed_nonces);
            }
        }

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
        {
            let mut rx = self.receipts.write().await;
            for (k, v) in receipts {
                rx.put(k, v);
            }
        }

        // Update real-time metrics
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.metrics.blocks_produced.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .tx_in_block
            .fetch_add(block.header.tx_count as u64, Ordering::Relaxed);
        self.metrics
            .last_block_secs
            .store(now_secs, Ordering::Relaxed);

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

        // Broadcast enriched event to SSE/WebSocket subscribers
        let event = BlockEvent {
            height: block.header.height,
            tx_count: block.header.tx_count,
            hash_hex: hex::encode(block.hash()),
            base_fee_atoms: block.header.base_fee,
            proposer: block.header.validator.to_string(),
            state_root_hex: hex::encode(block.header.state_root),
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

        // If the scheduled leader is already suspended (liveness-evicted), halve the
        // activation time so backup validators step in sooner.
        let leader_addr = vs.leader_at(height).to_string();
        let leader_suspended = self
            .suspended_validators
            .read()
            .await
            .contains(&leader_addr);
        let activation_secs = if leader_suspended {
            ((distance as u64 + 1) * block_time).max(block_time / 2)
        } else {
            (distance as u64 + 1) * block_time
        };

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
            // Only arm the heartbeat timer when something time-sensitive is pending
            // (upgrade scheduled or account frozen). Otherwise sleep forever — a
            // transaction signal is the only thing that can wake us up.
            let needs_heartbeat = self.state.read().await.has_pending_time_sensitive_ops();

            let triggered_by_tx = if needs_heartbeat {
                tokio::select! {
                    _ = tx_ready.notified() => true,
                    _ = tokio::time::sleep(heartbeat) => false,
                }
            } else {
                tx_ready.notified().await;
                true
            };

            if triggered_by_tx {
                // Brief batch window: let concurrent txs accumulate before sealing.
                tokio::time::sleep(batch_window).await;
                tracing::debug!("Block triggered by transaction");
            } else {
                tracing::debug!("Heartbeat block — upgrade or freeze pending");
            }

            match self.tick().await {
                Ok(block) => {
                    tracing::info!(
                        height = block.header.height,
                        txs = block.header.tx_count,
                        base_fee = block.header.base_fee,
                        "Block sealed"
                    );
                    {
                        use vinx_core::amount::{BLOCK_RETENTION_COUNT, PRUNE_INTERVAL};
                        const AUTO_COMPACT_INTERVAL: u64 = 500;
                        const LIVENESS_EVICTION_BLOCKS: u64 = 50;
                        let h = block.header.height;
                        // Periodic pruning: drop old tx/sig data every PRUNE_INTERVAL blocks
                        if h > 0 && h % PRUNE_INTERVAL == 0 {
                            self.chain.write().await.prune(BLOCK_RETENTION_COUNT);
                        }
                        // Auto-compact old tx index every 500 blocks (E)
                        if h > 0 && h % AUTO_COMPACT_INTERVAL == 0 {
                            self.chain
                                .write()
                                .await
                                .compact_old_txs(BLOCK_RETENTION_COUNT);
                            tracing::debug!(height = h, "Auto-compacted chain tx data");
                        }
                        // Update suspended validators based on liveness (F)
                        if h >= LIVENESS_EVICTION_BLOCKS {
                            let liveness = self.validator_liveness.read().await;
                            let vs = self.validator_set.read().await;
                            let my_addr = self.config.validator_address.to_string();
                            let mut suspended = self.suspended_validators.write().await;
                            for addr in vs.validators() {
                                let addr_str = addr.to_string();
                                if addr_str == my_addr {
                                    continue; // never suspend ourselves
                                }
                                let offline_for = liveness
                                    .get(&addr_str)
                                    .map_or(h, |&last| h.saturating_sub(last));
                                if offline_for >= LIVENESS_EVICTION_BLOCKS {
                                    if suspended.insert(addr_str.clone()) {
                                        tracing::warn!(
                                            address = %addr_str,
                                            offline_for,
                                            "Validator suspended (liveness eviction)"
                                        );
                                    }
                                } else if suspended.remove(&addr_str) {
                                    tracing::info!(
                                        address = %addr_str,
                                        "Validator restored (back online)"
                                    );
                                }
                            }
                        }
                    }
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

    /// Writes chain, state, and mempool to disk (no-op if no data_dir configured).
    ///
    /// Full persist: writes every account and wipes any stale rows. Used after a
    /// snapshot import replaces the live world state (the incremental `persist` path
    /// only writes changed rows and cannot detect accounts that disappeared).
    pub async fn persist_full(&self) {
        let Some(storage) = self.storage.as_ref().map(Arc::clone) else {
            return;
        };
        let state_write = {
            let mut state = self.state.write().await;
            let chain = self.chain.read().await;
            Storage::serialize_full(&mut state, &chain)
        };
        match state_write {
            Err(e) => tracing::warn!(error = %e, "Failed to serialize state for full persist"),
            Ok(sw) => {
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = storage.write_state(sw) {
                        tracing::warn!(error = %e, "Failed to persist full state to disk");
                    }
                })
                .await;
            }
        }
    }

    /// Serializes to bytes while holding read locks (fast, pure in-memory), releases
    /// the locks, then offloads zstd compression + redb write to a blocking thread.
    /// The JoinHandle is awaited so the persist completes before the caller proceeds,
    /// but locks are never held during I/O.
    pub async fn persist(&self) {
        let Some(storage) = self.storage.as_ref().map(Arc::clone) else {
            return;
        };

        // Serialize under a short write lock — moving the accounts map out for the
        // meta blob and draining the persist-dirty set both require &mut. Only the
        // accounts changed since the last flush are serialized here (O(dirty)).
        let (state_write, mempool_blob) = {
            let mut state = self.state.write().await;
            let chain = self.chain.read().await;
            let mempool = self.mempool.read().await;
            let txs = mempool.pending_txs();
            let sw = Storage::serialize_incremental(&mut state, &chain);
            let mp_blob = Storage::serialize_mempool(&txs);
            (sw, mp_blob)
        }; // all locks dropped here

        match state_write {
            Err(e) => tracing::warn!(error = %e, "Failed to serialize state for persist"),
            Ok(sw) => {
                // Compress + write in a blocking thread. Locks already released.
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = storage.write_state(sw) {
                        tracing::warn!(error = %e, "Failed to persist state to disk");
                    }
                    match mempool_blob {
                        Ok(blob) => {
                            if let Err(e) = storage.save_mempool_blob(blob) {
                                tracing::warn!(error = %e, "Failed to persist mempool to disk");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to serialize mempool for persist");
                        }
                    }
                })
                .await;
            }
        }
    }

    /// Parses and caches a bech32 address string, returning an ApiError on failure.
    pub async fn parse_address(
        &self,
        raw: &str,
    ) -> Result<vinx_crypto::Address, crate::rpc::handlers::ApiError> {
        {
            let mut cache = self.address_cache.lock().await;
            if let Some(addr) = cache.get(raw) {
                return Ok(addr.clone());
            }
        }
        let addr = vinx_crypto::Address::from_bech32(raw)
            .map_err(|e| crate::rpc::handlers::ApiError::BadRequest(e.to_string()))?;
        self.address_cache
            .lock()
            .await
            .put(raw.to_string(), addr.clone());
        Ok(addr)
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
