use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
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

/// ADR 0031 — contexte de fork-choice partagé avec la tâche P2P : le **snapshot d'état
/// finalisé** (base de rejeu bornée pour les réorgs, cf. `reorg::advance_snapshot`) et le
/// `storage` (pour une persistance **complète** après une réorg — la chaîne a pu être tronquée,
/// ce que le persist incrémental ne saurait refléter).
#[derive(Clone)]
pub struct ForkChoiceCtx {
    pub finalized_state: Arc<RwLock<(u64, WorldState)>>,
    pub storage: Option<Arc<Storage>>,
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
    /// ADR 0031 — snapshot d'état finalisé `(hauteur, état)` : base de rejeu bornée pour les
    /// réorgs de fork-choice. Maintenu par `reorg::advance_snapshot` après chaque avancée de
    /// finalité (tick + chemins P2P). Partagé avec la tâche P2P via `ForkChoiceCtx`.
    pub finalized_state: Arc<RwLock<(u64, WorldState)>>,
}

impl Node {
    pub fn new(state: WorldState, chain: Chain, config: NodeConfig) -> Arc<Self> {
        let storage = config
            .data_dir
            .as_ref()
            .map(|p| Arc::new(Storage::new(p.clone())));
        let initial_vs = state.validator_set.clone();
        let (block_events, _) = broadcast::channel(64);
        // ADR 0031 — snapshot finalisé initialisé à (tip, état courant) : ≥ finalité, donc les
        // réorgs sous ce tip sont sûrement ignorées jusqu'à ce que la finalité le dépasse.
        let finalized_state = Arc::new(RwLock::new((chain.tip_height(), state.clone())));
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
            finalized_state,
        })
    }

    /// Creates a node and immediately starts the P2P layer (if configured).
    pub async fn new_with_p2p(state: WorldState, chain: Chain, config: NodeConfig) -> Arc<Self> {
        let storage = config
            .data_dir
            .as_ref()
            .map(|p| Arc::new(Storage::new(p.clone())));
        let initial_vs = state.validator_set.clone();
        // ADR 0031 — snapshot finalisé initialisé à (tip, état courant) avant de déplacer l'état.
        let finalized_state = Arc::new(RwLock::new((chain.tip_height(), state.clone())));
        let state_arc = Arc::new(RwLock::new(state));
        let chain_arc = Arc::new(RwLock::new(chain));
        let mempool_arc = Arc::new(RwLock::new(Mempool::new(config.max_mempool_size)));
        let vs_arc = Arc::new(RwLock::new(initial_vs));
        let (block_events, _) = broadcast::channel(64);

        let metrics = NodeMetrics::new();

        let fork_choice = ForkChoiceCtx {
            finalized_state: Arc::clone(&finalized_state),
            storage: storage.clone(),
        };

        let p2p = if config.p2p_listen.is_some() {
            match crate::p2p::start(
                &config,
                Arc::clone(&chain_arc),
                Arc::clone(&mempool_arc),
                Arc::clone(&state_arc),
                Arc::clone(&vs_arc),
                metrics.clone(),
                fork_choice,
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
            finalized_state,
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

        // ADR 0002: advance the finalized pointer. On a single-validator chain the
        // proposer's own signature already meets quorum, so the block is final at once;
        // with more validators it becomes final once quorum co-signs (via P2P).
        chain.advance_finality(&state.validator_set);

        // ADR 0038 — update pool co-signature window for the produced block.
        // BLS cosigners are recorded when the block is finalized via advance_finality.
        {
            state.record_block_cosigns(&[]);
        }

        // ADR 0031 — maintenir le snapshot d'état finalisé (base de rejeu des réorgs). Verrous
        // déjà tenus : state, chain ; finalized_state acquis en DERNIER (ordre global cohérent).
        {
            let mut snap = self.finalized_state.write().await;
            crate::reorg::advance_snapshot(&mut snap, &chain);
        }

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
        if vs.len() <= 1 {
            return; // Single-validator chain — can't skip yourself
        }

        let height = self.chain.read().await.tip_height() + 1;

        // ADR 0027 — la rotation (leader et file de backup) porte sur le set ACTIF :
        // les validateurs emprisonnés (jailed) sont sautés, exactement comme dans
        // `produce_block`. Le calcul est déterministe (dérivé de `state.reliability`).
        let active = {
            let state = self.state.read().await;
            vinx_core::reliability::active_validators(&vs, &state.reliability)
        };
        let n = active.len();
        if n <= 1 {
            return; // Un seul validateur actif — pas de backup possible.
        }
        let leader_idx = (height as usize) % n;

        let my_idx = match active
            .iter()
            .position(|a| a == &self.config.validator_address)
        {
            Some(i) => i,
            None => return, // Pas dans le set actif (non-validateur ou emprisonné).
        };

        if my_idx == leader_idx {
            return; // We ARE the scheduled leader — tick() will handle this
        }

        // My position in the backup queue (1 = first backup, 2 = second, …)
        let distance = (my_idx + n - leader_idx) % n;

        // If the scheduled leader is already suspended (liveness-evicted), halve the
        // activation time so backup validators step in sooner.
        let leader_addr = &active[leader_idx];
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

    /// Background task: produce one block every `block_time_secs`, unconditionally.
    ///
    /// ADR 0045 — cadence fixe 12 s, blocs à la demande et heartbeat supprimés.
    /// Empty blocks are normal during low-activity periods; the emission curve is
    /// time-integrated (ADR 0040) so empty blocks emit the same as a silent window
    /// of equal duration. The fixed tick also makes epoch boundaries (ADR 0028) and
    /// warm-up counting (ADR 0038) fully predictable: every epoch = exactly
    /// EPOCH_DURATION_SECS / block_time_secs blocks.
    ///
    /// Slot skip (ADR 0027): if the scheduled leader is offline, backup validators
    /// step in after 2 × block-times via `try_backup_production`.
    pub async fn run_block_producer(self: Arc<Self>) {
        let block_time = std::time::Duration::from_secs(self.config.block_time_secs);

        loop {
            tokio::time::sleep(block_time).await;

            match self.tick().await {
                Ok(block) => {
                    tracing::info!(
                        height = block.header.height,
                        txs = block.header.tx_count,
                        base_fee = block.header.base_fee,
                        "Block sealed"
                    );
                    {
                        use vinx_core::amount::{PRUNE_INTERVAL, TX_RETENTION_SECS};
                        const LIVENESS_EVICTION_BLOCKS: u64 = 50;
                        let h = block.header.height;
                        let block_ts = block.header.timestamp;
                        if h > 0 && h % PRUNE_INTERVAL == 0 {
                            self.chain.write().await.prune_by_age(block_ts, TX_RETENTION_SECS);
                        }
                        if h >= LIVENESS_EVICTION_BLOCKS {
                            let liveness = self.validator_liveness.read().await;
                            let vs = self.validator_set.read().await;
                            let my_addr = self.config.validator_address;
                            let mut suspended = self.suspended_validators.write().await;
                            for addr in vs.validators() {
                                if *addr == my_addr {
                                    continue;
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
            let mut chain = self.chain.write().await;
            Storage::serialize_full(&mut state, &mut chain)
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

    /// Serializes to bytes while holding short write locks (fast, pure in-memory —
    /// draining the state/chain dirty sets requires `&mut`), releases the locks,
    /// then offloads zstd compression + redb write to a blocking thread.
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
            let mut chain = self.chain.write().await;
            let mempool = self.mempool.read().await;
            let txs = mempool.pending_txs();
            let sw = Storage::serialize_incremental(&mut state, &mut chain);
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

// ADR 0043 — la cadence est désormais un plancher fixe `block_time` (plus d'accélération à la
// demande). L'ancienne `dynamic_gap` (écart décroissant → blocs dos-à-dos en saturation) a été
// retirée : elle maximisait la fenêtre de fork sous charge. La boucle de production espace
// simplement les blocs de `block_time` (voir `run_block_producer`).
