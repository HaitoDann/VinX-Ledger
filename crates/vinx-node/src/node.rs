use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use vinx_crypto::{Address, BlsPubKey, Hash32};

use crate::{
    bft::{Engine, HeightParams, Host, Input, Output, TimeoutKind},
    chain::{Chain, Tip},
    config::NodeConfig,
    execution,
    mempool::Mempool,
    p2p::{messages::P2pMessage, NetEvent, P2pHandle},
    storage::Storage,
    NodeError,
};
use vinx_core::{Block, CommitCert, SignedVote, Transaction, ValidatorSet};
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
    /// Total transactions included in committed blocks.
    pub tx_in_block: AtomicU64,
    /// Committed blocks received via P2P gossip.
    pub p2p_blocks_recv: AtomicU64,
    /// Transactions received via P2P gossip.
    pub p2p_tx_recv: AtomicU64,
    /// Requests rejected by the rate limiter.
    pub ratelimit_hit: AtomicU64,
    /// Unix timestamp (seconds) of the last block committed.
    pub last_block_secs: AtomicU64,
    /// Consensus round of the height in progress (ADR 0082).
    pub consensus_round: AtomicU64,
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
                consensus_round: AtomicU64::new(0),
            }),
        }
    }
}

impl std::ops::Deref for NodeMetrics {
    type Target = NodeMetricsInner;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Events broadcast to SSE/WebSocket subscribers on each committed block.
#[derive(Clone, Debug)]
pub struct BlockEvent {
    pub height: u64,
    pub tx_count: u32,
    pub hash_hex: String,
    /// Base fee of this block, in atoms.
    pub base_fee_atoms: u64,
    /// Bech32 address of the validator that proposed this block.
    pub proposer: String,
    /// Hex-encoded account state root after this block.
    pub state_root_hex: String,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub struct Node {
    pub state: Arc<RwLock<WorldState>>,
    pub mempool: Arc<RwLock<Mempool>>,
    pub chain: Arc<RwLock<Chain>>,
    pub config: NodeConfig,
    storage: Option<Arc<Storage>>,
    /// P2P handle — present when p2p_listen is configured.
    pub p2p: Option<P2pHandle>,
    /// Live validator set — refreshed after each committed block.
    pub validator_set: Arc<RwLock<ValidatorSet>>,
    /// Broadcast channel for new-block SSE/WebSocket events.
    pub block_events: broadcast::Sender<BlockEvent>,
    /// Per-recipient faucet cooldown tracker + serialization lock for faucet requests.
    pub faucet_cooldowns: Arc<Mutex<HashMap<Address, Instant>>>,
    /// Pending validator join requests (in-memory, not persisted).
    pub validator_requests: Arc<Mutex<Vec<ValidatorJoinRequest>>>,
    /// Transaction receipts indexed by raw `Hash32` tx hash.
    pub receipts: Arc<RwLock<LruCache<Hash32, TxReceipt>>>,
    /// LRU cache for bech32 address decoding.
    pub address_cache: tokio::sync::Mutex<LruCache<String, vinx_crypto::Address>>,
    /// Lock-free real-time counters exposed on GET /metrics.
    pub metrics: NodeMetrics,
    /// Consensus inbox: network events (P2P layer) for the consensus task.
    net_tx: mpsc::UnboundedSender<NetEvent>,
    net_rx: Mutex<Option<mpsc::UnboundedReceiver<NetEvent>>>,
}

/// Shared handles a node is assembled from.
struct Parts {
    state: Arc<RwLock<WorldState>>,
    chain: Arc<RwLock<Chain>>,
    mempool: Arc<RwLock<Mempool>>,
    validator_set: ValidatorSet,
    metrics: NodeMetrics,
    p2p: Option<P2pHandle>,
    net_tx: mpsc::UnboundedSender<NetEvent>,
    net_rx: mpsc::UnboundedReceiver<NetEvent>,
}

impl Node {
    fn assemble(config: NodeConfig, parts: Parts) -> Arc<Self> {
        let storage = config
            .data_dir
            .as_ref()
            .map(|p| Arc::new(Storage::new(p.clone())));
        let (block_events, _) = broadcast::channel(64);
        Arc::new(Self {
            state: parts.state,
            mempool: parts.mempool,
            chain: parts.chain,
            validator_set: Arc::new(RwLock::new(parts.validator_set)),
            config,
            storage,
            p2p: parts.p2p,
            block_events,
            faucet_cooldowns: Arc::new(Mutex::new(HashMap::new())),
            validator_requests: Arc::new(Mutex::new(Vec::new())),
            receipts: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(100_000).unwrap(),
            ))),
            address_cache: tokio::sync::Mutex::new(LruCache::new(
                NonZeroUsize::new(1_024).unwrap(),
            )),
            metrics: parts.metrics,
            net_tx: parts.net_tx,
            net_rx: Mutex::new(Some(parts.net_rx)),
        })
    }

    pub fn new(state: WorldState, chain: Chain, config: NodeConfig) -> Arc<Self> {
        let (net_tx, net_rx) = mpsc::unbounded_channel();
        let parts = Parts {
            validator_set: state.validator_set.clone(),
            mempool: Arc::new(RwLock::new(Mempool::new(config.max_mempool_size))),
            state: Arc::new(RwLock::new(state)),
            chain: Arc::new(RwLock::new(chain)),
            metrics: NodeMetrics::new(),
            p2p: None,
            net_tx,
            net_rx,
        };
        Self::assemble(config, parts)
    }

    /// Creates a node and immediately starts the P2P layer (if configured).
    pub async fn new_with_p2p(state: WorldState, chain: Chain, config: NodeConfig) -> Arc<Self> {
        let validator_set = state.validator_set.clone();
        let state = Arc::new(RwLock::new(state));
        let chain = Arc::new(RwLock::new(chain));
        let mempool = Arc::new(RwLock::new(Mempool::new(config.max_mempool_size)));
        let metrics = NodeMetrics::new();
        let (net_tx, net_rx) = mpsc::unbounded_channel();
        let p2p = if config.p2p_listen.is_some() {
            match crate::p2p::start(
                &config,
                Arc::clone(&chain),
                Arc::clone(&mempool),
                Arc::clone(&state),
                metrics.clone(),
                net_tx.clone(),
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
        let parts = Parts {
            state,
            chain,
            mempool,
            validator_set,
            metrics,
            p2p,
            net_tx,
            net_rx,
        };
        Self::assemble(config, parts)
    }

    /// Sender into the consensus inbox — used by tests and in-process harnesses.
    pub fn net_sender(&self) -> mpsc::UnboundedSender<NetEvent> {
        self.net_tx.clone()
    }

    // ── height context ────────────────────────────────────────────────────────

    /// Snapshot of everything consensus needs for the next height.
    async fn height_context(&self) -> HeightCtx {
        let base = self.state.read().await.clone();
        let tip = self.chain.read().await.tip();
        let candidates = {
            let mut mp = self.mempool.write().await;
            mp.prune_expired(tip.height + 1);
            mp.flush_staged();
            mp.select(self.config.max_block_txs)
        };
        HeightCtx {
            base,
            tip,
            candidates,
        }
    }

    fn engine_for(&self, ctx: &HeightCtx) -> Engine {
        let vs = ctx.base.validator_set.clone();
        let keys = ctx
            .base
            .indexed_bls_keys(&vs)
            .into_iter()
            .map(|k| k.and_then(|b| BlsPubKey::from_bytes(&b).ok()))
            .collect();
        let jailed: HashSet<Address> = ctx
            .base
            .reliability
            .iter()
            .filter(|(_, r)| r.is_jailed())
            .map(|(a, _)| *a)
            .collect();
        let me = vs.contains(&self.config.validator_address).then(|| {
            (
                self.config.validator_address,
                self.config.bls_secret_key.clone(),
            )
        });
        Engine::new(HeightParams {
            chain_id: ctx.base.chain_id,
            height: ctx.tip.height + 1,
            validators: vs,
            keys,
            jailed,
            me,
            timeouts: self.config.timeouts.clone(),
        })
    }

    fn host_for(&self, ctx: HeightCtx) -> NodeHost {
        NodeHost {
            base: ctx.base,
            tip: ctx.tip,
            candidates: ctx.candidates,
            me: self.config.validator_address,
            post_states: HashMap::new(),
            invalid: Vec::new(),
            storage: self.storage.clone(),
        }
    }

    // ── commit ────────────────────────────────────────────────────────────────

    /// Installs a committed block: post-state, chain row, mempool, receipts, events,
    /// persistence, and a `Committed` announcement to peers.
    async fn commit(
        &self,
        block: Block,
        cert: CommitCert,
        post_state: WorldState,
    ) -> Result<(), NodeError> {
        let height = block.header.height;
        if let Err(e) = self.config.checkpoints.accepts_block(height, block.hash()) {
            return Err(NodeError::Consensus(format!(
                "block {height} refused by checkpoints: {e}"
            )));
        }
        {
            let mut state = self.state.write().await;
            let mut chain = self.chain.write().await;
            let mut mempool = self.mempool.write().await;
            if chain.tip_height() + 1 != height || chain.tip_hash() != block.header.prev_hash {
                return Err(NodeError::Consensus(format!(
                    "committed block {height} no longer extends the tip"
                )));
            }
            *state = post_state;
            chain.push(block.clone(), Some(cert.clone()));
            let mut confirmed = HashMap::new();
            for tx in &block.transactions {
                confirmed.insert(tx.sender(), tx.nonce + 1);
            }
            if !confirmed.is_empty() {
                mempool.update_confirmed_nonces(&confirmed);
            }
            use vinx_core::amount::{BLOCK_RETENTION_SECS, PRUNE_INTERVAL};
            if !self.config.archive && height.is_multiple_of(PRUNE_INTERVAL) {
                chain.prune_before(block.header.timestamp, BLOCK_RETENTION_SECS);
            }
            *self.validator_set.write().await = state.validator_set.clone();
        }
        {
            let mut rx = self.receipts.write().await;
            for tx in &block.transactions {
                let hash = tx.hash();
                rx.put(
                    hash,
                    TxReceipt {
                        tx_hash: hex::encode(hash),
                        block_height: height,
                        success: true,
                        error: None,
                    },
                );
            }
        }
        self.metrics.blocks_produced.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .tx_in_block
            .fetch_add(block.header.tx_count as u64, Ordering::Relaxed);
        self.metrics
            .last_block_secs
            .store(now_secs(), Ordering::Relaxed);
        let _ = self.block_events.send(BlockEvent {
            height,
            tx_count: block.header.tx_count,
            hash_hex: hex::encode(block.hash()),
            base_fee_atoms: block.header.base_fee,
            proposer: block.header.validator.to_string(),
            state_root_hex: hex::encode(block.header.state_root),
        });
        self.persist().await;
        if let Some(p2p) = &self.p2p {
            p2p.broadcast(P2pMessage::Committed { block, cert });
        }
        tracing::info!(height, "Block committed");
        Ok(())
    }

    /// Validates and installs a committed block received from a peer.
    pub async fn apply_committed(&self, block: Block, cert: CommitCert) -> Result<(), NodeError> {
        let (state, tip) = (
            self.state.read().await.clone(),
            self.chain.read().await.tip(),
        );
        let post = execution::execute_committed(&state, &tip, &block, &cert, now_secs())
            .map_err(NodeError::Consensus)?;
        self.commit(block, cert, post).await
    }

    /// Runs one full height locally and commits it. Only succeeds when this node alone
    /// holds more than 2/3 of the voting power (a single-validator chain) — used by
    /// tests and single-node development.
    pub async fn tick(&self) -> Result<Block, NodeError> {
        let ctx = self.height_context().await;
        let mut engine = self.engine_for(&ctx);
        let mut host = self.host_for(ctx);
        let outs = engine.start(&mut host);
        for o in outs {
            if let Output::Commit { block, cert } = o {
                let post = host.take_post_state(&block.hash()).ok_or_else(|| {
                    NodeError::Consensus("committed block without post-state".into())
                })?;
                host.drop_invalid(&self.mempool).await;
                self.commit(block.clone(), cert, post).await?;
                return Ok(block);
            }
        }
        Err(NodeError::Consensus(
            "this node alone does not hold a quorum of the voting power".into(),
        ))
    }

    // ── consensus task ────────────────────────────────────────────────────────

    /// The consensus loop (ADR 0082): one [`Engine`] per height, fed by network events
    /// and timers; every commit is installed here, the only writer of the chain.
    ///
    /// Heights start at the protocol cadence: no earlier than `block_time_secs` after the
    /// previous block's timestamp. While waiting, late precommits for the block just
    /// committed keep being collected, so the next block credits more co-signers.
    pub async fn run_consensus(self: Arc<Self>) {
        let Some(mut rx) = self.net_rx.lock().await.take() else {
            tracing::error!("consensus task already running");
            return;
        };
        let (timer_tx, mut timer_rx) = mpsc::unbounded_channel::<(u64, TimeoutKind, u32)>();
        let mut prev: Option<Engine> = None;
        let mut last_sync_request = Instant::now()
            .checked_sub(Duration::from_secs(60))
            .unwrap_or_else(Instant::now);
        let mut rebroadcast = tokio::time::interval(Duration::from_secs(5));

        loop {
            // ── wait for the cadence, collecting late precommits ──
            let start_at = {
                let tip_ts = self.chain.read().await.tip_timestamp();
                let bt = self.state.read().await.block_time_secs;
                tip_ts.saturating_add(bt)
            };
            let mut advanced = false;
            // Messages of the next height that arrive before our cadence starts it (a peer
            // with a slightly faster clock) are kept and replayed, not dropped.
            let mut early: Vec<NetEvent> = Vec::new();
            while now_secs() < start_at && !advanced {
                tokio::select! {
                    ev = rx.recv() => {
                        let Some(ev) = ev else { return };
                        let next = self.chain.read().await.tip_height() + 1;
                        let is_next = matches!(&ev, NetEvent::Proposal(p) if p.height == next)
                            || matches!(&ev, NetEvent::Vote(v) if v.height == next);
                        if is_next {
                            if early.len() < 10_000 {
                                early.push(ev);
                            }
                        } else {
                            advanced = self.handle_between_heights(ev, &mut prev).await;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                }
            }
            if advanced {
                continue;
            }
            // Upgrade the tip's certificate with the late precommits we collected.
            if let Some(cert) = prev.as_ref().and_then(|e| e.best_commit()) {
                self.chain.write().await.upgrade_tip_commit(cert);
            }

            // ── run one height ──
            let ctx = self.height_context().await;
            let height = ctx.tip.height + 1;
            let mut engine = self.engine_for(&ctx);
            if let Some((round, hash)) = self
                .storage
                .as_ref()
                .and_then(|st| st.last_precommit(height))
            {
                engine.restore_lock(round, hash);
            }
            let mut host = self.host_for(ctx);
            self.metrics.consensus_round.store(0, Ordering::Relaxed);
            let mut outs = engine.start(&mut host);
            for ev in early {
                match ev {
                    NetEvent::Proposal(p) if p.height == height => {
                        outs.extend(engine.handle(Input::Proposal(p), &mut host));
                    }
                    NetEvent::Vote(v) if v.height == height => {
                        outs.extend(engine.handle(Input::Vote(v), &mut host));
                    }
                    _ => {}
                }
            }
            let mut decided = self.route(height, outs, &timer_tx).await;
            let mut synced = false;

            while decided.is_none() && !synced {
                tokio::select! {
                    ev = rx.recv() => {
                        let Some(ev) = ev else { return };
                        match ev {
                            NetEvent::Proposal(p) if p.height == height => {
                                let outs = engine.handle(Input::Proposal(p), &mut host);
                                decided = self.route(height, outs, &timer_tx).await;
                            }
                            NetEvent::Vote(v) if v.height == height => {
                                let outs = engine.handle(Input::Vote(v), &mut host);
                                decided = self.route(height, outs, &timer_tx).await;
                            }
                            NetEvent::Vote(v) if v.height + 1 == height => {
                                if let Some(p) = prev.as_mut() {
                                    p.handle(Input::Vote(v), &mut NullHost);
                                }
                            }
                            NetEvent::Committed { block, cert } if block.header.height == height => {
                                let input = Input::Committed { block, cert };
                                let outs = engine.handle(input, &mut host);
                                decided = self.route(height, outs, &timer_tx).await;
                            }
                            NetEvent::SyncRows(rows) => {
                                synced = self.apply_rows(rows).await > 0;
                            }
                            ev => {
                                // A message from a height ahead of ours: we are behind.
                                if event_height(&ev).is_some_and(|h| h > height)
                                    && last_sync_request.elapsed() > Duration::from_secs(2)
                                {
                                    last_sync_request = Instant::now();
                                    if let Some(p2p) = &self.p2p {
                                        p2p.broadcast(P2pMessage::SyncRequest {
                                            from_height: height,
                                            limit: 64,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    t = timer_rx.recv() => {
                        let Some((h, kind, round)) = t else { return };
                        if h == height {
                            let outs = engine.handle(Input::Timeout { kind, round }, &mut host);
                            self.metrics
                                .consensus_round
                                .store(engine.round() as u64, Ordering::Relaxed);
                            decided = self.route(height, outs, &timer_tx).await;
                        }
                    }
                    _ = rebroadcast.tick() => {
                        let outs = engine.rebroadcast();
                        self.route(height, outs, &timer_tx).await;
                    }
                }
            }

            if let Some((block, cert)) = decided {
                let post = host.take_post_state(&block.hash()).or_else(|| {
                    execution::execute_block(&host.base, &host.tip, &block, now_secs()).ok()
                });
                host.drop_invalid(&self.mempool).await;
                match post {
                    Some(post) => {
                        if let Err(e) = self.commit(block, cert, post).await {
                            tracing::error!(height, error = %e, "commit failed");
                        }
                    }
                    None => tracing::error!(height, "decided block does not execute"),
                }
            }
            prev = Some(engine);
        }
    }

    /// Between heights: late precommits go to the previous engine; committed blocks and
    /// sync rows from peers may advance the chain. Returns true when the chain advanced.
    async fn handle_between_heights(&self, ev: NetEvent, prev: &mut Option<Engine>) -> bool {
        match ev {
            NetEvent::Vote(v) => {
                if let Some(p) = prev.as_mut() {
                    if p.height() == v.height {
                        p.handle(Input::Vote(v), &mut NullHost);
                    }
                }
                false
            }
            NetEvent::Committed { block, cert } => {
                let next = self.chain.read().await.tip_height() + 1;
                block.header.height == next && self.apply_committed(block, cert).await.is_ok()
            }
            NetEvent::SyncRows(rows) => self.apply_rows(rows).await > 0,
            NetEvent::Proposal(_) => false,
        }
    }

    /// Applies consecutive committed rows from a sync response. Returns how many applied.
    pub async fn apply_rows(&self, rows: Vec<(Block, CommitCert)>) -> usize {
        let mut applied = 0;
        for (block, cert) in rows {
            let next = self.chain.read().await.tip_height() + 1;
            if block.header.height < next {
                continue;
            }
            if block.header.height > next {
                break;
            }
            match self.apply_committed(block, cert).await {
                Ok(()) => applied += 1,
                Err(e) => {
                    tracing::warn!(error = %e, "sync row rejected");
                    break;
                }
            }
        }
        if applied > 0 {
            tracing::info!(applied, "Blocks applied from sync");
        }
        applied
    }

    /// Executes engine outputs: broadcasts, timers, evidence. Returns the decision.
    async fn route(
        &self,
        height: u64,
        outs: Vec<Output>,
        timer_tx: &mpsc::UnboundedSender<(u64, TimeoutKind, u32)>,
    ) -> Option<(Block, CommitCert)> {
        let mut decided = None;
        for o in outs {
            match o {
                Output::BroadcastProposal(p) => {
                    if let Some(p2p) = &self.p2p {
                        p2p.broadcast(P2pMessage::Proposal(p));
                    }
                }
                Output::BroadcastVote(v) => {
                    if let Some(p2p) = &self.p2p {
                        p2p.broadcast(P2pMessage::Vote(v));
                    }
                }
                Output::ScheduleTimeout { kind, round, after } => {
                    let tx = timer_tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(after).await;
                        let _ = tx.send((height, kind, round));
                    });
                }
                Output::Commit { block, cert } => decided = Some((block, cert)),
                Output::Evidence(ev) => self.submit_evidence(ev).await,
            }
        }
        decided
    }

    /// Turns vote-equivocation evidence into a `SlashValidator` transaction (ADR 0082).
    async fn submit_evidence(&self, ev: vinx_core::VoteEquivocation) {
        let target = ev.vote_a.validator;
        tracing::warn!(validator = %target, height = ev.vote_a.height, "vote equivocation detected");
        let (chain_id, nonce) = {
            let s = self.state.read().await;
            // Signed by this node's own key (the operator key when owner and operator are
            // separate, ADR 0084), so its own nonce.
            let me = Address::from_public_key(&self.config.validator_keypair.public_key());
            (s.chain_id, s.get_account(&me).map(|a| a.nonce).unwrap_or(0))
        };
        let mut tx =
            Transaction::new_slash_validator(&self.config.validator_keypair, target, &ev, nonce);
        tx.chain_id = chain_id;
        tx.sign(&self.config.validator_keypair);
        if self.mempool.write().await.add(tx.clone()).is_ok() {
            if let Some(p2p) = &self.p2p {
                p2p.broadcast_tx(&tx);
            }
        }
    }

    // ── persistence ───────────────────────────────────────────────────────────

    /// Full persist: writes every account and wipes any stale rows. Used after a
    /// snapshot import replaces the live world state.
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

    /// Serializes under short locks, then compresses and writes on a blocking thread.
    pub async fn persist(&self) {
        let Some(storage) = self.storage.as_ref().map(Arc::clone) else {
            return;
        };
        let (state_write, mempool_blob) = {
            let mut state = self.state.write().await;
            let mut chain = self.chain.write().await;
            let mempool = self.mempool.read().await;
            let txs = mempool.pending_txs();
            let sw = Storage::serialize_incremental(&mut state, &mut chain);
            let mp_blob = Storage::serialize_mempool(&txs);
            (sw, mp_blob)
        };
        match state_write {
            Err(e) => tracing::warn!(error = %e, "Failed to serialize state for persist"),
            Ok(sw) => {
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

/// Height of a network event, when it has one.
fn event_height(ev: &NetEvent) -> Option<u64> {
    match ev {
        NetEvent::Proposal(p) => Some(p.height),
        NetEvent::Vote(v) => Some(v.height),
        NetEvent::Committed { block, .. } => Some(block.header.height),
        NetEvent::SyncRows(_) => None,
    }
}

/// Everything a height needs, captured once at its start (the state does not change
/// during a height).
struct HeightCtx {
    base: WorldState,
    tip: Tip,
    candidates: Vec<Transaction>,
}

/// The node's [`Host`]: builds and validates blocks with the shared execution code, and
/// guards every signature with the durable double-sign lock.
struct NodeHost {
    base: WorldState,
    tip: Tip,
    candidates: Vec<Transaction>,
    me: Address,
    /// Post-states of blocks built or validated this height, by hash — reused at commit.
    post_states: HashMap<Hash32, WorldState>,
    /// Candidates found invalid while building (dropped from the mempool afterwards).
    invalid: Vec<Transaction>,
    storage: Option<Arc<Storage>>,
}

impl NodeHost {
    fn take_post_state(&mut self, hash: &Hash32) -> Option<WorldState> {
        self.post_states.remove(hash)
    }

    async fn drop_invalid(&mut self, mempool: &RwLock<Mempool>) {
        if !self.invalid.is_empty() {
            let invalid = std::mem::take(&mut self.invalid);
            mempool.write().await.remove_txs(&invalid);
        }
    }
}

impl Host for NodeHost {
    fn build_block(&mut self, round: u32) -> Option<Block> {
        match execution::build_block(
            &self.base,
            &self.tip,
            &self.candidates,
            self.me,
            round,
            now_secs(),
        ) {
            Ok((block, post, invalid)) => {
                self.invalid.extend(invalid);
                self.post_states.insert(block.hash(), post);
                Some(block)
            }
            Err(e) => {
                tracing::error!(error = %e, "could not build a proposal");
                None
            }
        }
    }

    fn validate_block(&mut self, block: &Block) -> bool {
        let hash = block.hash();
        if self.post_states.contains_key(&hash) {
            return true; // built (or already validated) by us this height
        }
        match execution::execute_block(&self.base, &self.tip, block, now_secs()) {
            Ok(post) => {
                self.post_states.insert(hash, post);
                true
            }
            Err(e) => {
                tracing::warn!(height = block.header.height, error = %e, "proposal rejected");
                false
            }
        }
    }

    fn may_sign(&mut self, vote: &SignedVote) -> bool {
        match &self.storage {
            // Fail closed: without a durable lock we cannot prove we are not equivocating.
            Some(storage) => storage.claim_sign(vote).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "vote lock unavailable — not signing");
                false
            }),
            None => true,
        }
    }
}

/// Host for an engine that is only collecting late precommits (never builds or signs).
struct NullHost;

impl Host for NullHost {
    fn build_block(&mut self, _round: u32) -> Option<Block> {
        None
    }
    fn validate_block(&mut self, _block: &Block) -> bool {
        false
    }
    fn may_sign(&mut self, _vote: &SignedVote) -> bool {
        false
    }
}
