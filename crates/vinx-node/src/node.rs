use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex, RwLock};
use vinx_crypto::{Address, Hash32};

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
    /// Per-recipient faucet cooldown tracker + serialization lock for faucet requests.
    /// Keyed by the raw `Address`, not a bech32 String.
    pub faucet_cooldowns: Arc<Mutex<HashMap<Address, Instant>>>,
    /// Timestamp of the last block produced or received — used for slot-skip logic.
    pub last_block_instant: Arc<RwLock<Instant>>,
    /// Last block height seen from each validator `Address` (liveness tracking).
    pub validator_liveness: Arc<RwLock<HashMap<Address, u64>>>,
    /// Pending validator join requests (in-memory, not persisted).
    pub validator_requests: Arc<Mutex<Vec<ValidatorJoinRequest>>>,
    /// Transaction receipts indexed by raw `Hash32` tx hash.
    pub receipts: Arc<RwLock<LruCache<Hash32, TxReceipt>>>,
    /// LRU cache for bech32 address decoding — keyed by the raw input String on
    /// purpose (it caches String → parsed Address, so the String *is* the key).
    pub address_cache: tokio::sync::Mutex<LruCache<String, vinx_crypto::Address>>,
    /// Lock-free real-time counters exposed on GET /metrics.
    pub metrics: NodeMetrics,
    /// Validators temporarily suspended from the round-robin due to liveness eviction.
    pub suspended_validators: Arc<RwLock<HashSet<Address>>>,
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
            mempool: Arc::new(RwLock::new(Mempool::new(config.max_mempool_size))),
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
        let mempool_arc = Arc::new(RwLock::new(Mempool::new(config.max_mempool_size)));
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
                confirmed_nonces.insert(tx.from, tx.nonce + 1);
            }
            if !confirmed_nonces.is_empty() {
                mempool.update_confirmed_nonces(&confirmed_nonces);
            }
        }

        let receipts = block
            .transactions
            .iter()
            .map(|tx| {
                let hash = tx.hash();
                (
                    hash,
                    TxReceipt {
                        tx_hash: hex::encode(hash),
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
        self.validator_liveness
            .write()
            .await
            .insert(self.config.validator_address, block.header.height);

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
        let leader_addr = vs.leader_at(height);
        let leader_suspended = self.suspended_validators.read().await.contains(leader_addr);
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
        use vinx_core::amount::HEARTBEAT_INTERVAL_SECS;

        let block_time = std::time::Duration::from_secs(self.config.block_time_secs);
        let heartbeat = std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS);
        // Courtesy window for the *first* block after an idle period: seal quickly
        // for a snappy confirmation instead of waiting the full demand-scaled gap.
        // A second tx arriving within the window rides along in the same block.
        let first_block_window = std::time::Duration::from_millis(500);

        // Extract the Notify handle once — no lock held while awaiting.
        let tx_ready = self.mempool.read().await.tx_ready.clone();

        // True when the last block came out empty despite a backlog — the pending
        // txs can't be applied yet (fee too low, nonce gap). We then fall back to
        // waiting for a fresh signal instead of spinning on empty blocks.
        let mut stalled = false;

        loop {
            // Heartbeat only when something time-sensitive is pending (a scheduled
            // upgrade). Otherwise we wait for a transaction — no empty blocks at rest.
            let needs_heartbeat = self.state.read().await.has_pending_time_sensitive_ops();

            // Proceed straight to production when there's pending work and we aren't
            // stalled — the demand-scaled gap below paces us, and any leftover txs
            // from a previous block keep draining. Otherwise wait for a signal
            // (or the heartbeat timer).
            let have_work = !stalled && self.mempool.read().await.size() > 0;

            let is_heartbeat_block = if have_work {
                false
            } else if needs_heartbeat {
                tokio::select! {
                    _ = tx_ready.notified() => false,
                    _ = tokio::time::sleep(heartbeat) => true,
                }
            } else {
                tx_ready.notified().await;
                false
            };
            // Any fresh wake clears a prior stall — we retry the backlog.
            stalled = false;

            if is_heartbeat_block {
                tracing::debug!("Heartbeat block — time-sensitive op pending");
            } else {
                // Pacing before sealing:
                // - `have_work` (leftover from a previous block, or continuous
                //   traffic) → demand-scaled gap that shrinks as the mempool fills.
                // - otherwise this is the *first* block after idle → a short courtesy
                //   window for snappy confirmation (isolated users don't wait ~10s).
                let gap = if have_work {
                    let pending = self.mempool.read().await.size();
                    dynamic_gap(pending, self.config.max_block_txs, block_time)
                } else {
                    first_block_window
                };
                if !gap.is_zero() {
                    tokio::time::sleep(gap).await;
                }
                // Nothing to seal (spurious wake or already drained) → don't produce
                // an empty block; loop back to waiting.
                if self.mempool.read().await.size() == 0 {
                    continue;
                }
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
                            let my_addr = self.config.validator_address;
                            let mut suspended = self.suspended_validators.write().await;
                            for addr in vs.validators() {
                                if *addr == my_addr {
                                    continue; // never suspend ourselves
                                }
                                let offline_for =
                                    liveness.get(addr).map_or(h, |&last| h.saturating_sub(last));
                                if offline_for >= LIVENESS_EVICTION_BLOCKS {
                                    if suspended.insert(*addr) {
                                        tracing::warn!(
                                            address = %addr,
                                            offline_for,
                                            "Validator suspended (liveness eviction)"
                                        );
                                    }
                                } else if suspended.remove(addr) {
                                    tracing::info!(
                                        address = %addr,
                                        "Validator restored (back online)"
                                    );
                                }
                            }
                        }
                    }
                    self.persist().await;
                    // A block that came out empty despite a full backlog means the
                    // pending txs can't be applied yet (fee too low, nonce gap). Mark
                    // stalled so the next iteration waits for a fresh signal instead of
                    // spinning on empty blocks; any wake clears it and retries.
                    let pending = self.mempool.read().await.size();
                    stalled = pending >= self.config.max_block_txs && block.header.tx_count == 0;
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
                return Ok(*addr);
            }
        }
        let addr = vinx_crypto::Address::from_bech32(raw)
            .map_err(|e| crate::rpc::handlers::ApiError::BadRequest(e.to_string()))?;
        self.address_cache.lock().await.put(raw.to_string(), addr);
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

/// Demand-scaled gap before sealing the next block, factored out for testing.
///
/// Full `block_time` when the mempool is nearly empty (light-activity cadence),
/// shrinking linearly toward zero as `pending` approaches one block's worth
/// (`max_block_txs`), and exactly zero — back-to-back — once a full block is
/// already queued (saturation). Combined with block-on-demand (no work → no
/// block), this yields the three regimes in a single curve:
/// idle → no block; light traffic → ~`block_time`; rising load → tighter gaps;
/// saturation → back-to-back.
fn dynamic_gap(pending: usize, max_block_txs: usize, block_time: Duration) -> Duration {
    let max = max_block_txs.max(1);
    if pending >= max {
        return Duration::ZERO;
    }
    let fill = pending as f64 / max as f64; // 0.0 ..= 1.0
    block_time.mul_f64(1.0 - fill)
}

#[cfg(test)]
mod tests {
    use super::dynamic_gap;
    use std::time::Duration;

    const BT: Duration = Duration::from_secs(10);
    const MAX: usize = 10_000;

    #[test]
    fn test_gap_full_when_nearly_empty() {
        // Empty / a few txs → ~full block_time (light-activity regime).
        assert_eq!(dynamic_gap(0, MAX, BT), BT);
        let g = dynamic_gap(50, MAX, BT);
        assert!(g > Duration::from_millis(9_900) && g <= BT);
    }

    #[test]
    fn test_gap_shrinks_as_mempool_fills() {
        // Half a block queued → half the gap; nearly a full block → nearly zero.
        assert_eq!(dynamic_gap(MAX / 2, MAX, BT), BT.mul_f64(0.5));
        assert_eq!(dynamic_gap(MAX * 9 / 10, MAX, BT), BT.mul_f64(0.1));
    }

    #[test]
    fn test_gap_zero_under_full_backlog() {
        // A full block (or more) queued → back-to-back, no wait.
        assert_eq!(dynamic_gap(MAX, MAX, BT), Duration::ZERO);
        assert_eq!(dynamic_gap(MAX * 5, MAX, BT), Duration::ZERO);
    }

    #[test]
    fn test_gap_never_divides_by_zero() {
        // max_block_txs = 0 is clamped to 1 — no panic; empty mempool → full gap.
        assert_eq!(dynamic_gap(0, 0, BT), BT);
    }
}
