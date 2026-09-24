pub mod guard;
pub mod messages;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use libp2p::{
    gossipsub::{self, IdentTopic, TopicHash},
    identify, mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, SwarmBuilder,
};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

use crate::{
    chain::Chain, config::NodeConfig, mempool::Mempool, node::ForkChoiceCtx, node::NodeMetrics,
    storage::Storage, NodeError,
};
use messages::P2pMessage;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use vinx_core::{Block, BlockHeader, SlashEvidence, Transaction, ValidatorSet};
use vinx_crypto::{Address, Hash32, VrfProof, VrfSecretKey, VRF_PROOF_LEN};

/// In-flight state for a compact block being reassembled (ADR 0037).
struct CompactBlockState {
    header: BlockHeader,
    tx_hashes: Vec<Hash32>,
    /// Transactions resolved so far, keyed by hash for O(1) lookup.
    resolved: HashMap<Hash32, Transaction>,
    /// Proposer proof carried by the announcement (VINX-01): the reconstructed block
    /// must be re-authenticated, so the aggregate/bitmap cannot be dropped here.
    bls_aggregate: Option<Vec<u8>>,
    bls_bitmap: Vec<u8>,
    /// ADR 0029 Phase 2a — the proposer's VRF proof, preserved so the reassembled block
    /// keeps the fork-choice priority every node computes identically.
    vrf_proof: Option<Vec<u8>>,
}
use vinx_state::WorldState;

#[allow(clippy::large_enum_variant)]
pub enum P2pCommand {
    Broadcast(P2pMessage),
    Shutdown,
}

#[derive(Clone)]
pub struct P2pHandle {
    cmd_tx: mpsc::UnboundedSender<P2pCommand>,
    pub local_peer_id: PeerId,
}

impl P2pHandle {
    pub fn broadcast_block(&self, block: &Block) {
        let _ = self
            .cmd_tx
            .send(P2pCommand::Broadcast(P2pMessage::NewBlock(block.clone())));
    }
    pub fn broadcast_tx(&self, tx: &Transaction) {
        let _ = self
            .cmd_tx
            .send(P2pCommand::Broadcast(P2pMessage::NewTransaction(
                tx.clone(),
            )));
    }
    pub fn broadcast_bls_cosig(
        &self,
        height: u64,
        block_hash: Vec<u8>,
        bls_sig: Vec<u8>,
        validator_addr: Vec<u8>,
    ) {
        let _ = self
            .cmd_tx
            .send(P2pCommand::Broadcast(P2pMessage::BlockBlsCoSignature {
                height,
                block_hash,
                bls_sig,
                validator_addr,
            }));
    }
    /// Gossips a compact block (header + tx hashes) for efficient block propagation
    /// (ADR 0037). Peers reconstruct the block from their mempool and request any
    /// missing transactions via `TxRequest`.
    pub fn broadcast_compact_block(&self, block: &Block) {
        let msg = P2pMessage::compact_from_block(block);
        let _ = self.cmd_tx.send(P2pCommand::Broadcast(msg));
    }

    pub fn broadcast_vrf_proof(&self, height: u64, vrf_proof: Vec<u8>, validator_addr: Vec<u8>) {
        let _ = self
            .cmd_tx
            .send(P2pCommand::Broadcast(P2pMessage::BlockVrfProof {
                height,
                vrf_proof,
                validator_addr,
            }));
    }

    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(P2pCommand::Shutdown);
    }
}

#[derive(NetworkBehaviour)]
struct VinxBehaviour {
    gossipsub: gossipsub::Behaviour,
    identify: identify::Behaviour,
    mdns: mdns::tokio::Behaviour,
}

const TOPICS: &[&str] = &[
    "vinx/blocks/1",
    "vinx/txs/1",
    "vinx/bls/1",
    "vinx/sync/1",
    "vinx/vrf/1",
    "vinx/compact/1",
];

#[allow(clippy::too_many_arguments)]
pub async fn start(
    config: &NodeConfig,
    chain: Arc<RwLock<Chain>>,
    mempool: Arc<RwLock<Mempool>>,
    state: Arc<RwLock<WorldState>>,
    validator_set: Arc<RwLock<ValidatorSet>>,
    metrics: NodeMetrics,
    fork_choice: ForkChoiceCtx,
    recent_block_txs: Arc<RwLock<std::collections::HashMap<u64, Vec<Transaction>>>>,
) -> Result<P2pHandle, NodeError> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<P2pCommand>();

    let validator_secret = config.validator_keypair.secret_bytes();
    let p2p_secret = {
        use vinx_crypto::hash256;
        let mut derived = hash256(&validator_secret);
        derived[0] &= 0xf8;
        derived[31] = (derived[31] & 0x1f) | 0x40;
        derived
    };

    let libp2p_kp = libp2p::identity::Keypair::ed25519_from_bytes(p2p_secret)
        .map_err(|e| NodeError::Config(format!("P2P key derivation failed: {e}")))?;
    let local_peer_id = PeerId::from(libp2p_kp.public());

    let listen_addr: Multiaddr = config
        .p2p_listen
        .as_deref()
        .unwrap_or("/ip4/0.0.0.0/tcp/9000")
        .parse()
        .map_err(|e: libp2p::multiaddr::Error| NodeError::Config(e.to_string()))?;

    let mut swarm = SwarmBuilder::with_existing_identity(libp2p_kp)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .map_err(|e| NodeError::Config(format!("P2P TCP setup failed: {e}")))?
        .with_quic()
        .with_behaviour(|key| {
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(1))
                .validation_mode(gossipsub::ValidationMode::Strict)
                // ADR 0022: cap the on-wire message size. Also lifts the libp2p 64 KiB
                // default so full blocks and sync batches can be gossiped, while the
                // decoder separately bounds the *decompressed* size (anti zip-bomb).
                .max_transmit_size(messages::MAX_DECODED_BYTES)
                .build()
                .expect("valid gossipsub config");
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )
            .expect("valid gossipsub behaviour");
            let identify = identify::Behaviour::new(identify::Config::new(
                "/vinx/1.0.0".to_string(),
                key.public(),
            ));
            let mdns =
                mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id())
                    .expect("valid mDNS behaviour");
            Ok(VinxBehaviour {
                gossipsub,
                identify,
                mdns,
            })
        })
        .map_err(|e| NodeError::Config(format!("P2P behaviour setup failed: {e}")))?
        .build();

    let _topic_map: Vec<(TopicHash, IdentTopic)> = TOPICS
        .iter()
        .map(|t| {
            let topic = IdentTopic::new(*t);
            swarm
                .behaviour_mut()
                .gossipsub
                .subscribe(&topic)
                .expect("subscribe ok");
            (topic.hash(), topic)
        })
        .collect();

    swarm
        .listen_on(listen_addr.clone())
        .map_err(|e| NodeError::Config(format!("P2P listen failed: {e}")))?;

    // Also listen on QUIC (same port as TCP, different protocol)
    let quic_listen = config
        .p2p_listen
        .as_deref()
        .unwrap_or("/ip4/0.0.0.0/tcp/9000")
        .replace("/tcp/", "/udp/");
    let quic_addr_str = format!("{}/quic-v1", quic_listen);
    if let Ok(addr) = quic_addr_str.parse::<Multiaddr>() {
        let _ = swarm.listen_on(addr);
    }

    // Dial explicitly configured peers (e.g. from config file)
    for peer_addr in &config.peer_addrs {
        if let Ok(addr) = peer_addr.parse::<Multiaddr>() {
            let _ = swarm.dial(addr);
        }
    }
    // Dial hardcoded bootstrap peers for initial network discovery
    for peer_addr in &config.bootstrap_peers {
        match peer_addr.parse::<Multiaddr>() {
            Ok(addr) => {
                info!(addr = %addr, "Dialing bootstrap peer");
                let _ = swarm.dial(addr);
            }
            Err(e) => warn!(addr = %peer_addr, error = %e, "Invalid bootstrap peer address"),
        }
    }

    info!(peer_id = %local_peer_id, listen = %listen_addr, "P2P service started");

    let local_kp = config.validator_keypair.clone();
    let local_addr = config.validator_address;
    let bls_sk = config.bls_secret_key.clone();
    let vrf_sk = config.vrf_secret_key.clone();

    tokio::spawn(async move {
        run_event_loop(
            swarm,
            cmd_rx,
            chain,
            mempool,
            state,
            validator_set,
            local_kp,
            local_addr,
            metrics,
            fork_choice,
            bls_sk,
            vrf_sk,
            recent_block_txs,
        )
        .await;
    });

    Ok(P2pHandle {
        cmd_tx,
        local_peer_id,
    })
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
async fn run_event_loop(
    mut swarm: libp2p::Swarm<VinxBehaviour>,
    mut cmd_rx: mpsc::UnboundedReceiver<P2pCommand>,
    chain: Arc<RwLock<Chain>>,
    mempool: Arc<RwLock<Mempool>>,
    state: Arc<RwLock<WorldState>>,
    validator_set: Arc<RwLock<ValidatorSet>>,
    local_kp: vinx_crypto::KeyPair,
    local_addr: Address,
    metrics: NodeMetrics,
    fork_choice: ForkChoiceCtx,
    bls_sk: vinx_crypto::BlsSecretKey,
    vrf_sk: Option<VrfSecretKey>,
    recent_block_txs: Arc<RwLock<std::collections::HashMap<u64, Vec<Transaction>>>>,
) {
    let mut guard = guard::PeerGuard::new();
    // ADR 0029 Phase 1: pending BLS co-signatures keyed by (height, block_hash).
    // Each entry maps to Vec<(validator_idx, bls_sig)> sorted by validator_idx for canonical
    // aggregation. Pruned below finality; drained once quorum is reached.
    let mut pending_bls: HashMap<(u64, Hash32), Vec<(usize, [u8; 96])>> = HashMap::new();
    // ADR 0030: co-signature conflict accountability.
    // Maps (height, validator_addr) → (signed_hash, bls_sig) to detect when the same
    // validator signs two different block hashes at the same height.
    let mut cosig_index: HashMap<(u64, Address), (Hash32, [u8; 96])> = HashMap::new();
    // Stores headers of competing (non-canonical) blocks received via NewBlock, keyed by
    // (height, block_hash). Used to build SlashEvidence when a conflict is detected.
    let mut competing_headers: HashMap<(u64, Hash32), BlockHeader> = HashMap::new();
    // ADR 0029 Phase 2b: pending VRF proofs for committee selection.
    // Maps height → Vec<(validator_addr, proof_bytes)>. Pruned below finality.
    let mut pending_vrf: HashMap<u64, Vec<(Address, [u8; VRF_PROOF_LEN])>> = HashMap::new();
    // ADR 0037: in-flight compact block reassembly. Pruned once the block is
    // applied or once a full `NewBlock` for the same height arrives.
    let mut pending_compact: HashMap<u64, CompactBlockState> = HashMap::new();

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(P2pCommand::Broadcast(msg)) => {
                        let topic = IdentTopic::new(msg.topic());
                        if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, msg.encode()) {
                            debug!(error = %e, "Gossip publish failed");
                        }
                    }
                    Some(P2pCommand::Shutdown) | None => {
                        info!("P2P event loop shutting down");
                        break;
                    }
                }
            }
            event = swarm.next() => {
                if let Some(event) = event {
                    handle_swarm_event(event, &chain, &mempool, &state, &validator_set, &local_kp, &local_addr, &mut swarm, &mut guard, &metrics, &fork_choice, &bls_sk, &vrf_sk, &mut pending_bls, &mut cosig_index, &mut competing_headers, &mut pending_vrf, &mut pending_compact, &recent_block_txs).await;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
async fn handle_swarm_event(
    event: SwarmEvent<VinxBehaviourEvent>,
    chain: &Arc<RwLock<Chain>>,
    mempool: &Arc<RwLock<Mempool>>,
    state: &Arc<RwLock<WorldState>>,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    local_kp: &vinx_crypto::KeyPair,
    local_addr: &Address,
    swarm: &mut libp2p::Swarm<VinxBehaviour>,
    guard: &mut guard::PeerGuard,
    metrics: &NodeMetrics,
    fork_choice: &ForkChoiceCtx,
    bls_sk: &vinx_crypto::BlsSecretKey,
    vrf_sk: &Option<VrfSecretKey>,
    pending_bls: &mut HashMap<(u64, Hash32), Vec<(usize, [u8; 96])>>,
    cosig_index: &mut HashMap<(u64, Address), (Hash32, [u8; 96])>,
    competing_headers: &mut HashMap<(u64, Hash32), BlockHeader>,
    pending_vrf: &mut HashMap<u64, Vec<(Address, [u8; VRF_PROOF_LEN])>>,
    pending_compact: &mut HashMap<u64, CompactBlockState>,
    recent_block_txs: &Arc<RwLock<std::collections::HashMap<u64, Vec<Transaction>>>>,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            info!(address = %address, "P2P listening");
        }
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            info!(peer = %peer_id, "Peer connected");
        }
        SwarmEvent::ConnectionClosed { peer_id, .. } => {
            debug!(peer = %peer_id, "Peer disconnected");
            // Bound per-peer state against connection churn.
            guard.forget(&peer_id);
        }

        // mDNS: automatic LAN peer discovery
        SwarmEvent::Behaviour(VinxBehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
            for (peer_id, addr) in peers {
                debug!(peer = %peer_id, %addr, "mDNS peer discovered");
                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                let _ = swarm.dial(addr);
            }
        }
        SwarmEvent::Behaviour(VinxBehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
            for (peer_id, _) in peers {
                debug!(peer = %peer_id, "mDNS peer expired");
                swarm
                    .behaviour_mut()
                    .gossipsub
                    .remove_explicit_peer(&peer_id);
            }
        }

        SwarmEvent::Behaviour(VinxBehaviourEvent::Gossipsub(gossipsub::Event::Message {
            message,
            propagation_source,
            ..
        })) => {
            // ADR 0022: rate-limit inbound messages per peer before doing any work.
            if let guard::Admit::RateLimited { ban } =
                guard.admit(propagation_source, std::time::Instant::now())
            {
                if ban {
                    warn!(peer = %propagation_source, "Banning peer for flooding");
                    swarm
                        .behaviour_mut()
                        .gossipsub
                        .blacklist_peer(&propagation_source);
                }
                return;
            }

            let Some(msg) = P2pMessage::decode(&message.data) else {
                // Undecipherable or oversized (incl. decompression-bomb) message — penalize.
                if guard.penalize(propagation_source, guard::BAD_MESSAGE_PENALTY) {
                    warn!(peer = %propagation_source, "Banning peer for invalid messages");
                    swarm
                        .behaviour_mut()
                        .gossipsub
                        .blacklist_peer(&propagation_source);
                }
                return;
            };
            dispatch_message(
                msg,
                chain,
                mempool,
                state,
                validator_set,
                local_kp,
                local_addr,
                swarm,
                metrics,
                fork_choice,
                bls_sk,
                vrf_sk,
                pending_bls,
                cosig_index,
                competing_headers,
                pending_vrf,
                pending_compact,
                recent_block_txs,
            )
            .await;
        }

        SwarmEvent::Behaviour(VinxBehaviourEvent::Identify(identify::Event::Received {
            peer_id,
            info,
            ..
        })) => {
            debug!(peer = %peer_id, protocols = ?info.protocols, "Peer identified");
            for addr in info.listen_addrs {
                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                let _ = swarm.dial(addr);
            }
        }
        _ => {}
    }
}

/// Verifies every **transaction** signature in a block in parallel (rayon, ADR 0015).
///
/// Ed25519 verification is the dominant CPU cost of validating a received block, it reads
/// no chain state, and it is a pure pass/fail conjunction — so running it across all cores
/// is both a large throughput win and fully deterministic (order-independent). State is
/// still applied **sequentially** afterwards via `apply_transaction_trusted`, which keeps
/// the deterministic execution order untouched. This parallelises validation, not
/// execution: the risky part (parallel state transitions) is deferred — see ADR 0015.
fn verify_block_tx_signatures_parallel(txs: &[Transaction]) -> bool {
    // Primitive partagée (consensus.rs) : le chemin P2P, la sync et le banc adversarial
    // doivent avoir *une* définition de la validité cryptographique d'un bloc.
    crate::consensus::verify_block_tx_signatures(txs)
}

/// ADR 0031 — traite un bloc **concurrent** (collision leader/backup) à une hauteur non
/// finalisée : le valide (proposeur ∈ set + signatures, comme le chemin normal), puis lance
/// l'orchestration de fork-choice **partagée** avec le banc n=3 (`reorg::consider_candidate`).
/// En cas de réorg : maintient le snapshot finalisé et **persiste en entier** (la chaîne a pu
/// être tronquée — le persist incrémental ne le refléterait pas). Verrous acquis dans l'ordre
/// state → chain → validator_set → finalized_state (cohérent avec `tick`, évite tout AB-BA) ;
/// l'écriture disque se fait hors verrous.
#[allow(clippy::too_many_arguments)]
async fn consider_competing_block(
    block: Block,
    chain: &Arc<RwLock<Chain>>,
    state: &Arc<RwLock<WorldState>>,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    vs: &ValidatorSet,
    fork_choice: &ForkChoiceCtx,
    metrics: &NodeMetrics,
) {
    let height = block.header.height;
    // Validité de base — identique au chemin normal.
    if !vs.contains(&block.header.validator) {
        debug!(
            height,
            "Fork-choice: bloc concurrent d'un non-validateur, ignoré"
        );
        return;
    }
    if !verify_block_tx_signatures_parallel(&block.transactions) {
        warn!(
            height,
            "Fork-choice: bloc concurrent à transaction(s) invalide(s)"
        );
        return;
    }

    // Orchestration sous verrous ordonnés. serialize_full est calculé DANS le scope (sous
    // verrous) mais l'écriture disque est faite APRÈS (jamais d'I/O en tenant un verrou).
    let (outcome, state_write) = {
        let mut sg = state.write().await;
        let mut cg = chain.write().await;
        let mut vg = validator_set.write().await;
        let mut fg = fork_choice.finalized_state.write().await;
        let snap_h = fg.0;
        let outcome =
            crate::reorg::consider_candidate(&mut cg, &mut sg, &mut vg, &fg.1, snap_h, block);
        let sw = if outcome == crate::reorg::ReorgOutcome::Reorged {
            crate::reorg::advance_snapshot(&mut fg, &cg);
            fork_choice
                .storage
                .as_ref()
                .and_then(|_| Storage::serialize_full(&mut sg, &mut cg).ok())
        } else {
            None
        };
        (outcome, sw)
    };

    if outcome == crate::reorg::ReorgOutcome::Reorged {
        warn!(
            height,
            "ADR 0031 — RÉORG : bascule sur le bloc canonique concurrent"
        );
        metrics.p2p_blocks_recv.fetch_add(1, Ordering::Relaxed);
        // Persistance complète (chaîne potentiellement tronquée), hors verrous.
        if let (Some(storage), Some(sw)) =
            (fork_choice.storage.as_ref().map(Arc::clone), state_write)
        {
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(e) = storage.write_state(sw) {
                    warn!(error = %e, "Échec de la persistance complète après réorg");
                }
            })
            .await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
async fn dispatch_message(
    msg: P2pMessage,
    chain: &Arc<RwLock<Chain>>,
    mempool: &Arc<RwLock<Mempool>>,
    state: &Arc<RwLock<WorldState>>,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    _local_kp: &vinx_crypto::KeyPair,
    local_addr: &Address,
    swarm: &mut libp2p::Swarm<VinxBehaviour>,
    metrics: &NodeMetrics,
    fork_choice: &ForkChoiceCtx,
    bls_sk: &vinx_crypto::BlsSecretKey,
    vrf_sk: &Option<VrfSecretKey>,
    pending_bls: &mut HashMap<(u64, Hash32), Vec<(usize, [u8; 96])>>,
    cosig_index: &mut HashMap<(u64, Address), (Hash32, [u8; 96])>,
    competing_headers: &mut HashMap<(u64, Hash32), BlockHeader>,
    pending_vrf: &mut HashMap<u64, Vec<(Address, [u8; VRF_PROOF_LEN])>>,
    pending_compact: &mut HashMap<u64, CompactBlockState>,
    recent_block_txs: &Arc<RwLock<std::collections::HashMap<u64, Vec<Transaction>>>>,
) {
    match msg {
        P2pMessage::NewTransaction(tx) => {
            metrics.p2p_tx_recv.fetch_add(1, Ordering::Relaxed);
            // Stateful admission before staging (anti-spam): a gossiping peer must
            // not get unfunded/wrong-chain transactions parked in our mempool any
            // more than an RPC client would. Signatures are still verified later,
            // in parallel, by flush_staged() at block production.
            if let Err(e) = state.read().await.admission_check(&tx) {
                debug!(error = %e, "P2P transaction rejected at admission");
                return;
            }
            mempool.write().await.stage(tx);
        }

        P2pMessage::NewBlock(block) => {
            let height = block.header.height;
            let vs = validator_set.read().await.clone();
            // ADR 0002/0027 — SÛRETÉ : quorum de finalité sur le set COMPLET bondé (jamais le
            // set actif — le jailing dérivé est subjectif sous partition et casserait la
            // sûreté). Capturé avant les changements de set par gouvernance de ce bloc.
            let pre_quorum = vs.quorum();

            // 1. Height and prev_hash linkage
            {
                let chain_guard = chain.read().await;
                let expected = chain_guard.tip_height() + 1;
                if height < expected {
                    // ADR 0031 — un bloc à une hauteur déjà tenue n'est pas forcément un doublon :
                    // c'est peut-être un CONCURRENT valide (collision leader/backup) à une
                    // hauteur non finalisée, bâti sur le même parent. Le fork-choice tranche.
                    let is_competitor = height > chain_guard.finalized_height()
                        && chain_guard.get_block(height).map(|b| b.hash()) != Some(block.hash())
                        && chain_guard
                            .get_block(height.saturating_sub(1))
                            .map(|b| b.hash())
                            == Some(block.header.prev_hash);
                    drop(chain_guard); // libérer le verrou lecture avant l'orchestration (écriture)
                    if is_competitor {
                        // ADR 0030: store the competing block's header so co-signature
                        // conflict detection can build SlashEvidence if needed.
                        if vs.contains(&block.header.validator) {
                            let h = block.hash();
                            competing_headers.insert((height, h), block.header.clone());
                            let fin = chain.read().await.finalized_height();
                            competing_headers.retain(|(bh, _), _| *bh > fin);
                        }
                        consider_competing_block(
                            block,
                            chain,
                            state,
                            validator_set,
                            &vs,
                            fork_choice,
                            metrics,
                        )
                        .await;
                    } else {
                        debug!(height, "P2P block already seen");
                    }
                    return;
                }
                if height > expected {
                    // Behind — request missing blocks
                    let limit = (height - expected).min(200) as u32 + 1;
                    let req = P2pMessage::SyncRequest {
                        from_height: expected,
                        limit,
                    };
                    let topic = IdentTopic::new(req.topic());
                    let _ = swarm.behaviour_mut().gossipsub.publish(topic, req.encode());
                    debug!(
                        our = expected,
                        peer = height,
                        "Sent SyncRequest to catch up"
                    );
                    return;
                }
                if block.header.prev_hash != chain_guard.tip_hash() {
                    warn!(height, "P2P block wrong prev_hash");
                    return;
                }
                // ADR 0005: timestamp bounds. Reject non-monotonic or far-future
                // timestamps so a producer can't inflate emission / shorten unbonding
                // via a bogus clock. The state_root does not catch this (both sides use
                // the same block timestamp), so it must be checked explicitly.
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if block.header.timestamp <= chain_guard.tip_timestamp() {
                    warn!(height, "P2P block timestamp not monotonic — rejecting");
                    return;
                }
                if block.header.timestamp
                    > now.saturating_add(vinx_core::amount::MAX_CLOCK_DRIFT_SECS)
                {
                    warn!(
                        height,
                        "P2P block timestamp too far in the future — rejecting"
                    );
                    return;
                }
            }

            // 2. Proposer authority — tout validateur enregistré peut proposer. Le leader
            //    round-robin est indicatif : sur slot-skip, un backup produit légitimement
            //    (ADR 0027/0031). On exige donc l'appartenance au set, pas le leader strict —
            //    cohérent avec consensus::validate_block (sinon les blocs backup sont rejetés
            //    et la finalité se fige à n≥2, cf. banc n=3).
            // VINX-01 / VX-RED-002 — set membership is not authorship. `header.validator`
            // is a public address, so on its own it proves nothing: any peer could name an
            // honest validator and have the block applied. Require a co-signature from the
            // proposer's *registered* BLS key over this exact header.
            {
                let st = state.read().await;
                let indexed_pks = st.indexed_bls_keys(&vs);
                if let Err(e) =
                    crate::consensus::verify_proposer_authenticated(&block, &vs, &indexed_pks)
                {
                    warn!(height, error = %e, "P2P block failed proposer authentication");
                    return;
                }
                // ADR 0029 Phase 2a — si le bloc revendique une élection VRF, la preuve doit
                // vérifier contre la clé VRF enregistrée du proposeur et son tirage passer sous
                // le seuil. Les blocs sans VRF (round-robin/backup) passent inchangés.
                if let Err(e) = crate::consensus::verify_vrf_leadership(&block, &st, vs.len()) {
                    warn!(height, error = %e, "P2P block failed VRF leadership check");
                    return;
                }
            }

            // ADR 0003: equivocation detection — check if this proposer already signed a
            // different block at this height (using the previously stored hash).
            {
                let proposer = block.header.validator;
                let new_hash = block.hash();
                let equivocation_block = {
                    let c = chain.read().await;
                    c.get_block(height).and_then(|existing| {
                        if existing.header.validator == proposer && existing.hash() != new_hash {
                            Some(existing.clone())
                        } else {
                            None
                        }
                    })
                };
                if let Some(existing_block) = equivocation_block {
                    warn!(height, %proposer, "EQUIVOCATION: proposer sent two different blocks");
                    if let (Some(sig_a), Some(sig_b)) =
                        (&existing_block.bls_aggregate, &block.bls_aggregate)
                    {
                        if sig_a.len() == 96 && sig_b.len() == 96 {
                            let evidence = SlashEvidence {
                                header_a: existing_block.header.clone(),
                                header_b: block.header.clone(),
                                bls_sig_a: sig_a.clone(),
                                bls_sig_b: sig_b.clone(),
                            };
                            let (chain_id, nonce) = {
                                let s = state.read().await;
                                (
                                    s.chain_id,
                                    s.get_account(local_addr).map(|a| a.nonce).unwrap_or(0),
                                )
                            };
                            let mut slash_tx = Transaction::new_slash_validator(
                                _local_kp, proposer, &evidence, nonce,
                            );
                            slash_tx.chain_id = chain_id;
                            slash_tx.sign(_local_kp);
                            if mempool.write().await.add(slash_tx.clone()).is_ok() {
                                let out = P2pMessage::NewTransaction(slash_tx);
                                let topic = IdentTopic::new(out.topic());
                                let _ =
                                    swarm.behaviour_mut().gossipsub.publish(topic, out.encode());
                            }
                        }
                    }
                    return; // reject the equivocating block
                }
            }

            // 3. Transaction signatures verified in parallel; state applied sequentially.
            if !verify_block_tx_signatures_parallel(&block.transactions) {
                warn!(height, "P2P block has invalid transaction signature(s)");
                return;
            }

            // 4. State transition with rollback. Protocol time = MTP including this
            // block (ADR 0005) — identical to what the producer computed.
            let protocol_ts = chain
                .read()
                .await
                .median_time_past_with(block.header.timestamp);
            let applied = {
                let mut sg = state.write().await;
                let snapshot = sg.clone();
                sg.set_block_context(protocol_ts);
                let mut ok = true;
                for tx in &block.transactions {
                    if let Err(e) = sg.apply_transaction_trusted(tx) {
                        warn!(height, error = %e, "P2P block tx failed, rolling back");
                        *sg = snapshot.clone();
                        ok = false;
                        break;
                    }
                }
                if ok {
                    sg.block_height = height;
                    sg.check_upgrade_activation();
                    let _ =
                        sg.settle_block(&block.header.validator, block.header.height, protocol_ts);
                    let root = sg.compute_state_root();
                    // ADR 0004: the state_root only covers accounts, not the Foundry —
                    // check the supply invariant explicitly on received blocks too.
                    if !sg.supply_invariant_holds() {
                        warn!(height, "P2P block breaks supply invariant, rolling back");
                        *sg = snapshot;
                        ok = false;
                    } else if root != block.header.state_root {
                        warn!(height, "P2P block state_root mismatch, rolling back");
                        *sg = snapshot;
                        ok = false;
                    } else {
                        let new_vs = sg.validator_set.clone();
                        drop(sg);
                        *validator_set.write().await = new_vs;
                    }
                }
                ok
            };
            if !applied {
                return;
            }

            // 5. Commit
            {
                let mut c = chain.write().await;
                c.note_quorum(height, pre_quorum); // ADR 0002/0027 — quorum historique
                c.push(block.clone());
            }
            // ADR 0037: drop any pending compact block reassembly for this height since
            // the block has now been applied — also prune heights below finality.
            pending_compact.remove(&height);
            let fin = chain.read().await.finalized_height();
            pending_compact.retain(|h, _| *h > fin);
            // Prune compact-block tx cache for heights at or below finality.
            recent_block_txs.write().await.retain(|h, _| *h > fin);

            info!(height, "P2P: block validated and applied");
            metrics.p2p_blocks_recv.fetch_add(1, Ordering::Relaxed);

            // 6. BLS co-sign if we're a validator (ADR 0029 Phase 1).
            let vs = validator_set.read().await.clone();
            if vs.contains(local_addr) {
                let block_hash = block.hash();

                // VX-RED-003 / VX-RED-007 — claim the vote lock BEFORE signing.
                //
                // The node used to sign and gossip first, then call `record_signature`
                // and discard its result. Detection is not prevention: an attacker who
                // sends two individually valid blocks at the same height, both under the
                // finality depth, made an *honest* validator co-sign both, so honest
                // validators contributed to two competing branches and the
                // "one validator, one vote per height" invariant did not hold.
                //
                // The lock is committed durably before any signature is released, so it
                // also survives a restart between the two blocks. Re-signing the same
                // hash stays allowed: a re-gossiped block is not equivocation.
                if let Some(storage) = fork_choice.storage.as_ref() {
                    match storage.claim_vote(height, block_hash) {
                        Ok(true) => {}
                        Ok(false) => {
                            warn!(
                                height,
                                "refusing to co-sign a second block at this height                                  (vote already locked) — equivocation prevented"
                            );
                            return;
                        }
                        Err(e) => {
                            // Fail closed: without a durable lock we cannot prove we are
                            // not equivocating, and equivocation is slashable.
                            warn!(height, error = %e, "vote lock unavailable — not co-signing");
                            return;
                        }
                    }
                }

                let bls_sig = bls_sk.sign(&block_hash);
                let bls_msg = P2pMessage::BlockBlsCoSignature {
                    height,
                    block_hash: block_hash.to_vec(),
                    bls_sig: bls_sig.0.to_vec(),
                    validator_addr: local_addr.as_bytes().to_vec(),
                };
                let bls_topic = IdentTopic::new(bls_msg.topic());
                let _ = swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(bls_topic, bls_msg.encode());
                debug!(height, "BLS co-signed block");
                // Still recorded: `record_signature` remains the slashing-evidence index
                // for *other* validators' equivocation. Our own vote is now gated by the
                // durable lock above rather than merely observed here.
                let mut c = chain.write().await;
                c.record_signature(local_addr, height, block_hash);
            }

            // 7. VRF proof for committee selection (ADR 0029 Phase 2b).
            // Only validators that have registered a VRF key participate.
            if let Some(sk) = vrf_sk {
                let has_vrf_key = state
                    .read()
                    .await
                    .validator_pool
                    .get(local_addr)
                    .and_then(|e| e.vrf_pub_key)
                    .is_some();
                if has_vrf_key {
                    let alpha = state.read().await.committee_alpha(height);
                    let proof = sk.prove(&alpha);
                    let vrf_msg = P2pMessage::BlockVrfProof {
                        height,
                        vrf_proof: proof.0.to_vec(),
                        validator_addr: local_addr.as_bytes().to_vec(),
                    };
                    let vrf_topic = IdentTopic::new(vrf_msg.topic());
                    let _ = swarm
                        .behaviour_mut()
                        .gossipsub
                        .publish(vrf_topic, vrf_msg.encode());
                    debug!(height, "VRF proof gossiped for committee selection");
                }
            }
        }

        // ADR 0046 / ADR 0030: accumulate BLS co-signatures; detect co-signer equivocation.
        P2pMessage::BlockBlsCoSignature {
            height,
            block_hash,
            bls_sig,
            validator_addr,
        } => {
            // Size guards — reject malformed frames early.
            if validator_addr.len() != 20 || bls_sig.len() != 96 || block_hash.len() != 32 {
                warn!(height, "P2P BLS cosig: wrong byte lengths");
                return;
            }
            let claimed_hash: Hash32 = block_hash.as_slice().try_into().unwrap();
            let sig_arr: [u8; 96] = bls_sig.as_slice().try_into().unwrap();

            // ADR 0029 Phase 1: resolve the sender's validator index and registered BLS PK
            // from the on-chain registry. Reject if the sender is not a registered validator
            // or has not registered a BLS key — never trust the claimed PK from the wire.
            let addr_arr: [u8; 20] = validator_addr.as_slice().try_into().unwrap();
            let sender_addr = Address::from_bytes(addr_arr);
            let vs = validator_set.read().await.clone();
            let validator_idx = match vs.index_of(&sender_addr) {
                Some(idx) => idx,
                None => {
                    debug!(height, "P2P BLS cosig: sender not in validator set");
                    return;
                }
            };
            let registered_pk = {
                let st = state.read().await;
                st.validator_pool
                    .get(&sender_addr)
                    .and_then(|e| e.bls_pub_key.as_deref())
                    .and_then(|b| <[u8; 48]>::try_from(b).ok())
            };
            let pk_arr = match registered_pk {
                Some(arr) => arr,
                None => {
                    debug!(height, "P2P BLS cosig: sender has no registered BLS key");
                    return;
                }
            };
            // Verify BLS sig against the CLAIMED hash (not necessarily our canonical one).
            // This is critical for ADR 0030: we must verify what the peer claims to sign,
            // even if it differs from our view, to build valid SlashEvidence later.
            let bls_pub = match vinx_crypto::BlsPubKey::from_bytes(&pk_arr) {
                Ok(p) => p,
                Err(_) => {
                    warn!(height, "P2P BLS cosig: registry contains invalid BLS PK");
                    return;
                }
            };
            let bls_signature = vinx_crypto::BlsSignature(sig_arr);
            if vinx_crypto::bls_verify(&bls_pub, &bls_signature, &claimed_hash).is_err() {
                warn!(height, "P2P BLS cosig: signature verification failed");
                return;
            }

            // ADR 0030: co-signer conflict detection.
            // Check if this validator has already co-signed a DIFFERENT hash at this height.
            let fin = chain.read().await.finalized_height();
            cosig_index.retain(|(h, _), _| *h > fin);
            let cosig_key = (height, sender_addr);
            if let Some((prev_hash, prev_sig)) = cosig_index.get(&cosig_key).copied() {
                if prev_hash == claimed_hash {
                    debug!(height, "P2P BLS cosig: duplicate, already recorded");
                    return;
                }
                // Same validator, same height, different hash — equivocation detected.
                warn!(
                    height, validator = %sender_addr,
                    "ADR 0030: co-signer equivocation detected — validator signed two hashes"
                );
                // Try to build SlashEvidence if we have both block headers.
                let header_a = {
                    let c = chain.read().await;
                    c.get_block(height)
                        .map(|b| b.header.clone())
                        .or_else(|| competing_headers.get(&(height, prev_hash)).cloned())
                };
                let header_b = competing_headers.get(&(height, claimed_hash)).cloned();
                if let (Some(ha), Some(hb)) = (header_a, header_b) {
                    // Only submit on-chain slash when both headers name the equivocating
                    // validator as proposer (current apply_slash_validator constraint).
                    if ha.validator == sender_addr && hb.validator == sender_addr {
                        let (chain_id, nonce) = {
                            let s = state.read().await;
                            (
                                s.chain_id,
                                s.get_account(local_addr).map(|a| a.nonce).unwrap_or(0),
                            )
                        };
                        let evidence = SlashEvidence {
                            header_a: ha,
                            header_b: hb,
                            bls_sig_a: prev_sig.to_vec(),
                            bls_sig_b: sig_arr.to_vec(),
                        };
                        let mut slash_tx = Transaction::new_slash_validator(
                            _local_kp,
                            sender_addr,
                            &evidence,
                            nonce,
                        );
                        slash_tx.chain_id = chain_id;
                        slash_tx.sign(_local_kp);
                        if mempool.write().await.add(slash_tx.clone()).is_ok() {
                            let out = P2pMessage::NewTransaction(slash_tx);
                            let topic = IdentTopic::new(out.topic());
                            let _ = swarm.behaviour_mut().gossipsub.publish(topic, out.encode());
                            warn!(height, validator = %sender_addr, "ADR 0030: slash evidence gossipped");
                        }
                    }
                }
                // Do not accumulate an equivocating co-signature.
                return;
            }
            cosig_index.insert(cosig_key, (claimed_hash, sig_arr));

            // Only accumulate towards quorum if the peer co-signed OUR canonical block.
            let our_block_hash: Hash32 = {
                let c = chain.read().await;
                match c.get_block(height) {
                    Some(b) => b.hash(),
                    None => {
                        debug!(height, "P2P BLS cosig for unknown height");
                        return;
                    }
                }
            };
            if claimed_hash != our_block_hash {
                debug!(
                    height,
                    "P2P BLS cosig: not for our canonical block — skipping accumulation"
                );
                return;
            }
            // Prune entries that can no longer affect finality.
            pending_bls.retain(|(h, _), _| *h > fin);
            // Accumulate, deduplicating by validator index.
            let count = {
                let entry = pending_bls.entry((height, our_block_hash)).or_default();
                if entry.iter().any(|(idx, _)| *idx == validator_idx) {
                    debug!(
                        height,
                        validator_idx, "P2P BLS cosig: duplicate validator, ignoring"
                    );
                    return;
                }
                entry.push((validator_idx, sig_arr));
                entry.len()
            };
            debug!(height, count, "P2P BLS cosig accumulated");
            // Check quorum — aggregate and update block when met.
            if count < vs.quorum() {
                return;
            }
            // Build aggregate in canonical (validator-index) order.
            let pending = match pending_bls.get_mut(&(height, our_block_hash)) {
                Some(v) => {
                    v.sort_unstable_by_key(|(idx, _)| *idx);
                    v.clone()
                }
                None => return,
            };
            let bls_sigs: Vec<vinx_crypto::BlsSignature> = pending
                .iter()
                .map(|(_, s)| vinx_crypto::BlsSignature(*s))
                .collect();
            let agg = match vinx_crypto::bls_aggregate(&bls_sigs) {
                Ok(a) => a,
                Err(e) => {
                    warn!(height, error = %e, "P2P BLS aggregate failed");
                    return;
                }
            };
            // Collect canonical PKs and build bitmap.
            let st = state.read().await;
            let canonical_pks: Vec<Vec<u8>> = pending
                .iter()
                .filter_map(|(idx, _)| {
                    vs.validators()
                        .get(*idx)
                        .and_then(|addr| st.validator_pool.get(addr))
                        .and_then(|e| e.bls_pub_key.clone())
                })
                .collect();
            drop(st);
            let mut bitmap = vec![];
            for (idx, _) in &pending {
                // Reuse Block's logic via a temporary helper.
                let byte_idx = idx / 8;
                let bit_pos = idx % 8;
                if bitmap.len() <= byte_idx {
                    bitmap.resize(byte_idx + 1, 0u8);
                }
                bitmap[byte_idx] |= 1 << bit_pos;
            }
            let agg_count = pending.len();
            // Resolve cosigner addresses from validator indices (ADR 0028).
            let cosigner_addrs: Vec<Address> = pending
                .iter()
                .filter_map(|(idx, _)| vs.validators().get(*idx).copied())
                .collect();
            {
                let indexed_pks = state.read().await.indexed_bls_keys(&vs);
                let mut c = chain.write().await;
                c.set_block_bls(height, agg.0.to_vec(), canonical_pks, bitmap);
                let fin = c.advance_finality(&vs, &indexed_pks);
                if fin >= height {
                    let mut snap = fork_choice.finalized_state.write().await;
                    crate::reorg::advance_snapshot(&mut snap, &c);
                    info!(
                        height,
                        cosigners = agg_count,
                        "Block finalized via BLS aggregate"
                    );
                }
            }
            // ADR 0028: record co-signers for proportional epoch distribution.
            state.write().await.record_block_cosigns(&cosigner_addrs);
            pending_bls.remove(&(height, our_block_hash));
        }

        // ADR 0029 Phase 2b: accumulate VRF proofs for committee selection.
        P2pMessage::BlockVrfProof {
            height,
            vrf_proof,
            validator_addr,
        } => {
            // Size guards — reject malformed frames early.
            if validator_addr.len() != 20
                || vrf_proof.len() != messages::P2pMessage::VRF_PROOF_WIRE_LEN
            {
                warn!(height, "P2P VRF proof: wrong byte lengths");
                return;
            }
            let addr_arr: [u8; 20] = validator_addr.as_slice().try_into().unwrap();
            let sender_addr = Address::from_bytes(addr_arr);
            let proof_arr: [u8; VRF_PROOF_LEN] = vrf_proof.as_slice().try_into().unwrap();
            let vrf_proof_typed = VrfProof(proof_arr);

            // Verify the proof against the validator's registered VRF key.
            let verified = {
                let st = state.read().await;
                st.verify_committee_vrf_proof(&sender_addr, height, &vrf_proof_typed)
                    .is_ok()
            };
            if !verified {
                warn!(height, validator = %sender_addr, "P2P VRF proof: verification failed or no registered key");
                return;
            };

            // Prune heights at or below finality.
            let fin = chain.read().await.finalized_height();
            pending_vrf.retain(|h, _| *h > fin);

            // Deduplicate by validator address.
            let entry = pending_vrf.entry(height).or_default();
            if entry.iter().any(|(a, _)| a == &sender_addr) {
                debug!(height, validator = %sender_addr, "P2P VRF proof: duplicate, ignoring");
                return;
            }
            entry.push((sender_addr, proof_arr));
            let count = entry.len();
            debug!(height, count, "P2P VRF proof accumulated");

            // Derive committee once we have enough proofs (≥ quorum).
            let vs = validator_set.read().await.clone();
            let quorum = vs.quorum();
            if count >= quorum {
                let proofs_typed: Vec<(Address, VrfProof)> =
                    entry.iter().map(|(a, b)| (*a, VrfProof(*b))).collect();
                let active_set_size = vinx_core::amount::MAX_ACTIVE_SET_SIZE as usize;
                let committee = state.read().await.committee_from_vrf_proofs(
                    height,
                    &proofs_typed,
                    active_set_size,
                );
                info!(
                    height,
                    count,
                    committee_size = committee.len(),
                    "VRF committee derived for height"
                );
            }
        }

        // ADR 0037 — Compact block propagation.
        P2pMessage::CompactBlock {
            header,
            tx_hashes,
            bls_aggregate,
            bls_bitmap,
            vrf_proof,
        } => {
            let height = header.height;

            // Discard if we're already at or past this height.
            if chain.read().await.tip_height() >= height {
                debug!(height, "CompactBlock: already at height, ignoring");
                return;
            }
            // Discard if too far ahead (would be served via sync anyway).
            if height > chain.read().await.tip_height() + 1 {
                debug!(height, "CompactBlock: too far ahead, waiting for sync");
                return;
            }

            // VINX-09 — bound the announced work before doing any. `tx_hashes` was limited
            // only by MAX_DECODED_BYTES (16 MiB ≈ 524 288 hashes), and each hash triggered a
            // full mempool allocation plus a SHA-256 per entry, all under the mempool lock:
            // one message froze the node. The count must also match the header the proposer
            // committed to.
            if tx_hashes.len() != header.tx_count as usize {
                warn!(
                    height,
                    announced = tx_hashes.len(),
                    header_tx_count = header.tx_count,
                    "CompactBlock: tx_hashes disagree with header.tx_count"
                );
                return;
            }
            if tx_hashes.len() > P2pMessage::MAX_TX_REQUEST_HASHES {
                warn!(
                    height,
                    count = tx_hashes.len(),
                    "CompactBlock: too many tx hashes"
                );
                return;
            }

            // Look up which transactions we already have in the mempool — single scan.
            let (resolved_txs, missing_hashes): (Vec<Transaction>, Vec<[u8; 32]>) =
                mempool.read().await.resolve_hashes(&tx_hashes);

            // Build an in-flight state for this height.
            let mut state_entry = CompactBlockState {
                header: header.clone(),
                tx_hashes: tx_hashes.clone(),
                resolved: HashMap::new(),
                bls_aggregate: bls_aggregate.clone(),
                bls_bitmap: bls_bitmap.clone(),
                vrf_proof: vrf_proof.clone(),
            };
            for tx in resolved_txs {
                state_entry.resolved.insert(tx.hash(), tx);
            }

            if missing_hashes.is_empty() {
                // All transactions are known — reconstruct and apply the full block.
                let mut ordered_txs = Vec::with_capacity(tx_hashes.len());
                for h in &tx_hashes {
                    if let Some(tx) = state_entry.resolved.get(h) {
                        ordered_txs.push(tx.clone());
                    }
                }
                let block = Block {
                    header,
                    transactions: ordered_txs,
                    bls_aggregate,
                    bls_cosigner_pks: vec![],
                    bls_bitmap,
                    vrf_proof,
                };
                info!(height, "CompactBlock: all txs known, applying immediately");
                // Re-dispatch as a full NewBlock through the existing path.
                Box::pin(dispatch_message(
                    P2pMessage::NewBlock(block),
                    chain,
                    mempool,
                    state,
                    validator_set,
                    _local_kp,
                    local_addr,
                    swarm,
                    metrics,
                    fork_choice,
                    bls_sk,
                    vrf_sk,
                    pending_bls,
                    cosig_index,
                    competing_headers,
                    pending_vrf,
                    pending_compact,
                    recent_block_txs,
                ))
                .await;
            } else {
                // Request missing transactions.
                let n_missing = missing_hashes.len();
                let capped: Vec<[u8; 32]> = missing_hashes
                    .into_iter()
                    .take(messages::P2pMessage::MAX_TX_REQUEST_HASHES)
                    .collect();
                let req = P2pMessage::TxRequest {
                    height,
                    hashes: capped,
                };
                let topic = IdentTopic::new(req.topic());
                let _ = swarm.behaviour_mut().gossipsub.publish(topic, req.encode());
                debug!(
                    height,
                    missing = n_missing,
                    "CompactBlock: requesting missing txs"
                );
                pending_compact.insert(height, state_entry);
            }
        }

        P2pMessage::TxRequest { height, hashes } => {
            // Serve up to MAX_TX_REQUEST_HASHES transactions.
            // Check the mempool first (live txs), then the recent-block tx cache
            // (ADR 0037 — producer flushes committed txs from mempool before broadcast).
            let capped: Vec<[u8; 32]> = hashes
                .into_iter()
                .take(messages::P2pMessage::MAX_TX_REQUEST_HASHES)
                .collect();
            let mut txs = mempool.read().await.get_by_hashes(&capped);
            // Check the compact-block tx cache for any hashes not found in the mempool.
            if txs.len() < capped.len() {
                let served: std::collections::HashSet<[u8; 32]> =
                    txs.iter().map(|tx| tx.hash()).collect();
                let cache = recent_block_txs.read().await;
                for h in &capped {
                    if !served.contains(h) {
                        // Search all cached block heights for this hash.
                        if let Some(tx) = cache
                            .values()
                            .flat_map(|v| v.iter())
                            .find(|tx| &tx.hash() == h)
                        {
                            txs.push(tx.clone());
                        }
                    }
                }
            }
            if !txs.is_empty() {
                let resp = P2pMessage::TxResponse { height, txs };
                let topic = IdentTopic::new(resp.topic());
                let _ = swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic, resp.encode());
                debug!(height, "TxRequest: served response");
            }
        }

        P2pMessage::TxResponse { height, txs } => {
            let Some(entry) = pending_compact.get_mut(&height) else {
                debug!(height, "TxResponse: no pending compact block, ignoring");
                return;
            };
            // Integrate received transactions.
            for tx in txs
                .into_iter()
                .take(messages::P2pMessage::MAX_TX_RESPONSE_TXS)
            {
                let h = tx.hash();
                if entry.tx_hashes.contains(&h) {
                    entry.resolved.insert(h, tx);
                }
            }
            // Check if reassembly is complete.
            if entry
                .tx_hashes
                .iter()
                .all(|h| entry.resolved.contains_key(h))
            {
                let header = entry.header.clone();
                let tx_hashes = entry.tx_hashes.clone();
                let mut ordered_txs = Vec::with_capacity(tx_hashes.len());
                for h in &tx_hashes {
                    if let Some(tx) = entry.resolved.get(h) {
                        ordered_txs.push(tx.clone());
                    }
                }
                let bls_aggregate = entry.bls_aggregate.clone();
                let bls_bitmap = entry.bls_bitmap.clone();
                let vrf_proof = entry.vrf_proof.clone();
                pending_compact.remove(&height);
                let block = Block {
                    header,
                    transactions: ordered_txs,
                    bls_aggregate,
                    bls_cosigner_pks: vec![],
                    bls_bitmap,
                    vrf_proof,
                };
                info!(height, "CompactBlock: reassembly complete, applying");
                Box::pin(dispatch_message(
                    P2pMessage::NewBlock(block),
                    chain,
                    mempool,
                    state,
                    validator_set,
                    _local_kp,
                    local_addr,
                    swarm,
                    metrics,
                    fork_choice,
                    bls_sk,
                    vrf_sk,
                    pending_bls,
                    cosig_index,
                    competing_headers,
                    pending_vrf,
                    pending_compact,
                    recent_block_txs,
                ))
                .await;
            } else {
                let remaining = entry
                    .tx_hashes
                    .iter()
                    .filter(|h| !entry.resolved.contains_key(*h))
                    .count();
                debug!(height, remaining, "TxResponse: partially resolved");
            }
        }

        // Respond to sync requests with our stored blocks
        P2pMessage::SyncRequest { from_height, limit } => {
            let c = chain.read().await;
            let tip = c.tip_height();
            // ADR 0022: clamp the requested count, use saturating arithmetic (a crafted
            // `from_height` near u64::MAX would otherwise overflow), then trim the batch to
            // the byte budget so a single response can never exceed MAX_DECODED_BYTES.
            let limit = limit.min(messages::MAX_SYNC_RESPONSE_BLOCKS);
            let end = from_height.saturating_add(limit as u64).min(tip + 1);
            let candidates: Vec<Block> = (from_height..end)
                .filter_map(|h| c.get_block(h).cloned())
                .collect();
            drop(c);
            let sizes: Vec<usize> = candidates
                .iter()
                .map(|b| borsh::to_vec(b).map(|v| v.len()).unwrap_or(usize::MAX))
                .collect();
            let take = messages::sync_batch_len(
                &sizes,
                messages::SYNC_RESPONSE_BUDGET_BYTES,
                messages::MAX_SYNC_RESPONSE_BLOCKS as usize,
            );
            let blocks: Vec<Block> = candidates.into_iter().take(take).collect();
            if !blocks.is_empty() {
                let n = blocks.len();
                let resp = P2pMessage::SyncResponse { blocks };
                let topic = IdentTopic::new(resp.topic());
                let _ = swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic, resp.encode());
                debug!(from = from_height, count = n, "Served SyncResponse");
            }
        }

        // Apply received sync blocks
        P2pMessage::SyncResponse { blocks } => {
            for block in blocks {
                let height = block.header.height;
                let expected = chain.read().await.tip_height() + 1;
                if height != expected {
                    debug!(height, expected, "SyncResponse block out of order");
                    break;
                }
                let vs = validator_set.read().await.clone();
                // ADR 0002/0027 — quorum de finalité sur le set COMPLET (voir note de sûreté).
                let pre_quorum = vs.quorum();
                // Tout validateur enregistré peut proposer (backup sur slot-skip, ADR 0027/0031) —
                // même règle que consensus::validate_block. Le leader strict rejetait à tort les
                // blocs backup et figeait la sync/finalité à n≥2 (révélé par le banc n=3).
                // VINX-01 — `SyncResponse` is a freely gossipable message carrying up to
                // MAX_SYNC_RESPONSE_BLOCKS blocks. Authenticate each proposer against the
                // on-chain BLS registry, exactly like the NewBlock path.
                {
                    let indexed_pks = state.read().await.indexed_bls_keys(&vs);
                    if let Err(e) =
                        crate::consensus::verify_proposer_authenticated(&block, &vs, &indexed_pks)
                    {
                        warn!(height, error = %e, "SyncResponse block failed proposer authentication");
                        break;
                    }
                }
                if block.header.prev_hash != chain.read().await.tip_hash() {
                    warn!(height, "SyncResponse block wrong prev_hash");
                    break;
                }
                // ADR 0005: timestamps must be monotonic even for historical blocks.
                if block.header.timestamp <= chain.read().await.tip_timestamp() {
                    warn!(height, "SyncResponse block timestamp not monotonic");
                    break;
                }
                // VINX-07 — the three other block-ingestion paths (NewBlock, sync_from_peer,
                // parallel_sync_from_peer) bound the timestamp by now + MAX_CLOCK_DRIFT_SECS;
                // this one only checked monotonicity. Six consecutive blocks dated far in the
                // future move the Median Time Past, and protocol time drives both emission
                // (`emit_work_reward`) and unbond maturation — so an unbounded timestamp mints
                // the remaining supply and matures every slashable bond at once.
                {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    if block.header.timestamp
                        > now.saturating_add(vinx_core::amount::MAX_CLOCK_DRIFT_SECS)
                    {
                        warn!(height, "SyncResponse block timestamp too far in the future");
                        break;
                    }
                }
                // ADR 0015: parallel transaction-signature verification, then sequential
                // trusted apply.
                if !verify_block_tx_signatures_parallel(&block.transactions) {
                    warn!(
                        height,
                        "SyncResponse block has invalid transaction signature(s)"
                    );
                    break;
                }
                // ADR 0005: protocol time = MTP including this block, computed from
                // the same chain prefix the producer used — deterministic on both sides.
                let protocol_ts = chain
                    .read()
                    .await
                    .median_time_past_with(block.header.timestamp);
                let ok = {
                    let mut sg = state.write().await;
                    let snapshot = sg.clone();
                    sg.set_block_context(protocol_ts);
                    let mut ok = true;
                    for tx in &block.transactions {
                        if let Err(e) = sg.apply_transaction_trusted(tx) {
                            warn!(height, error=%e, "SyncResponse tx failed");
                            *sg = snapshot.clone();
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        sg.block_height = height;
                        sg.check_upgrade_activation();
                        let _ = sg.settle_block(
                            &block.header.validator,
                            block.header.height,
                            protocol_ts,
                        );
                        let root = sg.compute_state_root();
                        if !sg.supply_invariant_holds() {
                            warn!(height, "SyncResponse block breaks supply invariant");
                            *sg = snapshot.clone();
                            ok = false;
                        } else if root != block.header.state_root {
                            warn!(height, "SyncResponse state_root mismatch");
                            *sg = snapshot.clone();
                            ok = false;
                        } else {
                            let nv = sg.validator_set.clone();
                            drop(sg);
                            *validator_set.write().await = nv;
                        }
                    }
                    ok
                };
                if ok {
                    // ADR 0002 — observabilité de la finalité : les blocs servis par sync
                    // portent déjà les co-signatures accumulées par le producteur, mais le
                    // pointeur de finalité LOCAL ne bouge que si on le fait avancer. Sans ça,
                    // un nœud qui rattrape par sync voit `finalized_height` figé (révélé par le
                    // banc n=3 : hauteur qui monte, finalité à 0). Prefix-closed → ne finalise
                    // que les blocs ayant réellement le quorum.
                    let vs = validator_set.read().await.clone();
                    let indexed_pks = state.read().await.indexed_bls_keys(&vs);
                    let mut c = chain.write().await;
                    c.note_quorum(height, pre_quorum); // ADR 0002/0027 — quorum historique
                    c.push(block);
                    c.advance_finality(&vs, &indexed_pks);
                    drop(c);
                    info!(height, "Block applied via P2P sync");
                } else {
                    break;
                }
            }
        }
    }
}
