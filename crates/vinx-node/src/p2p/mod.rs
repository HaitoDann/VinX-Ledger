pub mod discovery;
pub mod guard;
pub mod messages;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use libp2p::{
    gossipsub::{self, IdentTopic, TopicHash},
    identify, kad, mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, SwarmBuilder,
};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

use crate::{chain::Chain, config::NodeConfig, mempool::Mempool, node::NodeMetrics, NodeError};
use messages::P2pMessage;
use std::sync::atomic::Ordering;
use vinx_core::Transaction;
use vinx_state::WorldState;

/// What the P2P layer hands to the consensus task (ADR 0082). The consensus task is the
/// only writer of the chain: the network layer never applies anything itself.
#[derive(Debug)]
pub enum NetEvent {
    Proposal(crate::bft::Proposal),
    Vote(vinx_core::SignedVote),
    Committed {
        block: vinx_core::Block,
        cert: vinx_core::CommitCert,
    },
    SyncRows(Vec<(vinx_core::Block, vinx_core::CommitCert)>),
}

#[allow(clippy::large_enum_variant)]
pub enum P2pCommand {
    Broadcast(P2pMessage),
    /// A transaction to relay; batched by the event loop.
    RelayTx(Transaction),
    Shutdown,
}

#[derive(Clone)]
pub struct P2pHandle {
    cmd_tx: mpsc::UnboundedSender<P2pCommand>,
    pub local_peer_id: PeerId,
}

impl P2pHandle {
    pub fn broadcast(&self, msg: P2pMessage) {
        let _ = self.cmd_tx.send(P2pCommand::Broadcast(msg));
    }
    pub fn broadcast_tx(&self, tx: &Transaction) {
        let _ = self.cmd_tx.send(P2pCommand::RelayTx(tx.clone()));
    }
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(P2pCommand::Shutdown);
    }
}

#[derive(NetworkBehaviour)]
struct VinxBehaviour {
    gossipsub: gossipsub::Behaviour,
    identify: identify::Behaviour,
    mdns: libp2p::swarm::behaviour::toggle::Toggle<mdns::tokio::Behaviour>,
    kad: kad::Behaviour<kad::store::MemoryStore>,
}

const IDENTIFY_PROTOCOL: &str = "/vinx/1.0.0";

const TOPICS: &[&str] = &[
    "vinx/txs/1",
    "vinx/consensus/1",
    "vinx/blocks/1",
    "vinx/sync/1",
];

/// Starts the P2P layer. Consensus messages and sync responses are forwarded to
/// `consensus_tx`; transactions go to the mempool after admission checks.
pub async fn start(
    config: &NodeConfig,
    chain: Arc<RwLock<Chain>>,
    mempool: Arc<RwLock<Mempool>>,
    state: Arc<RwLock<WorldState>>,
    metrics: NodeMetrics,
    consensus_tx: mpsc::UnboundedSender<NetEvent>,
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
        .with_dns()
        .map_err(|e| NodeError::Config(format!("P2P DNS setup failed: {e}")))?
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
                IDENTIFY_PROTOCOL.to_string(),
                key.public(),
            ));
            // LAN discovery; VINX_NO_MDNS=1 turns it off (tests of wide-area discovery).
            let mdns = std::env::var_os("VINX_NO_MDNS")
                .is_none()
                .then(|| {
                    mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id())
                        .expect("valid mDNS behaviour")
                })
                .into();
            // Peer discovery (Kademlia), scoped to this chain so nodes of another
            // VinX network never end up in our routing table.
            let peer_id = key.public().to_peer_id();
            let kad_proto =
                libp2p::StreamProtocol::try_from_owned(format!("/vinx/{}/kad/1", config.chain_id))
                    .expect("valid protocol name");
            let mut kad = kad::Behaviour::with_config(
                peer_id,
                kad::store::MemoryStore::new(peer_id),
                kad::Config::new(kad_proto),
            );
            kad.set_mode(Some(kad::Mode::Server));
            Ok(VinxBehaviour {
                gossipsub,
                identify,
                mdns,
                kad,
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

    // Entry points: configured peers, bootstrap peers (CLI + the seeds built into the
    // binary for this chain) and the peers remembered from previous runs. Any one of
    // them being reachable is enough to rejoin the network.
    let mut entry: Vec<Multiaddr> = Vec::new();
    for a in config
        .peer_addrs
        .iter()
        .chain(config.bootstrap_peers.iter())
        .cloned()
        .chain(discovery::builtin_seeds(config.chain_id))
    {
        match a.parse::<Multiaddr>() {
            Ok(addr) => entry.push(addr),
            Err(e) => warn!(addr = %a, error = %e, "Invalid peer address"),
        }
    }
    for addr in &entry {
        info!(addr = %addr, "Dialing entry peer");
        let _ = swarm.dial(addr.clone());
    }
    let store_path = config.data_dir.as_ref().map(|d| d.join("peers.json"));
    let known = store_path
        .as_deref()
        .map(discovery::load_peers)
        .unwrap_or_default();
    if !known.is_empty() {
        info!(count = known.len(), "Reconnecting to remembered peers");
    }
    for (peer, addrs) in known {
        if peer == local_peer_id {
            continue;
        }
        for a in &addrs {
            swarm.behaviour_mut().kad.add_address(&peer, a.clone());
        }
        dial_once(&mut swarm, peer, addrs);
    }

    info!(peer_id = %local_peer_id, listen = %listen_addr, "P2P service started");

    tokio::spawn(async move {
        let disc = Discovery { entry, store_path };
        run_event_loop(
            swarm,
            cmd_rx,
            chain,
            mempool,
            state,
            metrics,
            consensus_tx,
            disc,
        )
        .await;
    });

    Ok(P2pHandle {
        cmd_tx,
        local_peer_id,
    })
}

/// Publishes the buffered transactions as `NewTransactions` batches.
fn flush_txs(swarm: &mut libp2p::Swarm<VinxBehaviour>, buf: &mut Vec<Transaction>) {
    while !buf.is_empty() {
        let n = buf.len().min(messages::MAX_TXS_PER_MESSAGE);
        let msg = P2pMessage::NewTransactions(buf.drain(..n).collect());
        let topic = IdentTopic::new(msg.topic());
        if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, msg.encode()) {
            debug!(error = %e, "Transaction batch publish failed");
        }
    }
}

/// Dials `peer` only if it is neither connected nor already being dialed, trying its
/// known addresses within a single attempt.
fn dial_once(swarm: &mut libp2p::Swarm<VinxBehaviour>, peer: PeerId, addrs: Vec<Multiaddr>) {
    use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
    if addrs.is_empty() || swarm.is_connected(&peer) {
        return;
    }
    let opts = DialOpts::peer_id(peer)
        .condition(PeerCondition::DisconnectedAndNotDialing)
        .addresses(addrs)
        .build();
    let _ = swarm.dial(opts);
}

/// What the event loop needs to keep the node connected without any fixed server.
struct Discovery {
    entry: Vec<Multiaddr>,
    store_path: Option<std::path::PathBuf>,
}

/// Saves the routing table so the next start reconnects without the entry points.
fn save_peers(swarm: &mut libp2p::Swarm<VinxBehaviour>, path: &std::path::Path) {
    let mut peers: Vec<(PeerId, Vec<Multiaddr>)> = Vec::new();
    for bucket in swarm.behaviour_mut().kad.kbuckets() {
        for e in bucket.iter() {
            peers.push((
                *e.node.key.preimage(),
                e.node.value.iter().cloned().collect(),
            ));
        }
    }
    discovery::save_peers(path, &peers);
}

#[allow(clippy::too_many_arguments)]
async fn run_event_loop(
    mut swarm: libp2p::Swarm<VinxBehaviour>,
    mut cmd_rx: mpsc::UnboundedReceiver<P2pCommand>,
    chain: Arc<RwLock<Chain>>,
    mempool: Arc<RwLock<Mempool>>,
    state: Arc<RwLock<WorldState>>,
    metrics: NodeMetrics,
    consensus_tx: mpsc::UnboundedSender<NetEvent>,
    disc: Discovery,
) {
    let mut discover = tokio::time::interval(Duration::from_secs(30));
    let mut ticks: u64 = 0;
    let mut guard = guard::PeerGuard::new();
    // Transactions to relay are grouped and flushed every 250 ms (or when a batch is
    // full): one gossip message per batch.
    let mut tx_buf: Vec<Transaction> = Vec::new();
    let mut flush = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! {
            _ = flush.tick() => {
                flush_txs(&mut swarm, &mut tx_buf);
            }
            _ = discover.tick() => {
                ticks += 1;
                for peer in guard.expired_bans(std::time::Instant::now()) {
                    info!(peer = %peer, "Ban lifted");
                    swarm.behaviour_mut().gossipsub.remove_blacklisted_peer(&peer);
                }
                let connected = swarm.connected_peers().count();
                metrics.peer_count.store(connected as u64, Ordering::Relaxed);
                if connected == 0 {
                    // Isolated: retry every entry point (a seed may be back online).
                    for addr in &disc.entry {
                        let _ = swarm.dial(addr.clone());
                    }
                }
                if connected < discovery::TARGET_PEERS {
                    // Random walk: asks the network for peers close to a random id;
                    // the answers land in the routing table and get dialed below.
                    swarm.behaviour_mut().kad.get_closest_peers(PeerId::random());
                }
                if ticks % 10 == 1 {
                    let _ = swarm.behaviour_mut().kad.bootstrap();
                }
                if ticks.is_multiple_of(2) {
                    if let Some(path) = &disc.store_path {
                        save_peers(&mut swarm, path);
                    }
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(P2pCommand::Broadcast(msg)) => {
                        let topic = IdentTopic::new(msg.topic());
                        if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, msg.encode()) {
                            debug!(error = %e, "Gossip publish failed");
                        }
                    }
                    Some(P2pCommand::RelayTx(tx)) => {
                        tx_buf.push(tx);
                        if tx_buf.len() >= messages::MAX_TXS_PER_MESSAGE {
                            flush_txs(&mut swarm, &mut tx_buf);
                        }
                    }
                    Some(P2pCommand::Shutdown) | None => {
                        info!("P2P event loop shutting down");
                        break;
                    }
                }
            }
            event = swarm.next() => {
                let Some(event) = event else { continue };
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!(address = %address, "P2P listening");
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, num_established, .. } => {
                        if num_established.get() == 1
                            && swarm.connected_peers().count() > discovery::MAX_PEERS
                        {
                            debug!(peer = %peer_id, "Peer limit reached, closing");
                            let _ = swarm.disconnect_peer_id(peer_id);
                        } else {
                            info!(peer = %peer_id, "Peer connected");
                        }
                        metrics.peer_count.store(swarm.connected_peers().count() as u64, Ordering::Relaxed);
                    }
                    SwarmEvent::Behaviour(VinxBehaviourEvent::Kad(kad::Event::RoutingUpdated {
                        peer,
                        addresses,
                        ..
                    })) => {
                        if swarm.connected_peers().count() < discovery::TARGET_PEERS {
                            dial_once(&mut swarm, peer, addresses.into_vec());
                        }
                    }
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        debug!(peer = %peer_id, "Peer disconnected");
                        guard.forget(&peer_id);
                        metrics.peer_count.store(swarm.connected_peers().count() as u64, Ordering::Relaxed);
                    }
                    SwarmEvent::Behaviour(VinxBehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
                        for (peer_id, addr) in peers {
                            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                            dial_once(&mut swarm, peer_id, vec![addr]);
                        }
                    }
                    SwarmEvent::Behaviour(VinxBehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
                        for (peer_id, _) in peers {
                            swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                        }
                    }
                    SwarmEvent::Behaviour(VinxBehaviourEvent::Identify(identify::Event::Received {
                        peer_id,
                        info,
                        ..
                    })) => {
                        // Identify runs on every new connection: dialing every advertised
                        // address unconditionally reconnected already-connected peers, each new
                        // connection re-triggered identify, and two nodes listening on several
                        // interfaces (LAN mode: TCP + QUIC × loopback + LAN) entered an
                        // exponential connection storm until they ran out of file descriptors.
                        if info.protocol_version != IDENTIFY_PROTOCOL {
                            continue;
                        }
                        // Feed the routing table with the addresses other nodes can reach
                        // (no loopback unless we are a loopback-only test network).
                        let local_only = swarm.listeners().all(discovery::is_loopback);
                        let addrs: Vec<Multiaddr> = info
                            .listen_addrs
                            .into_iter()
                            .filter(|a| local_only || !discovery::is_loopback(a))
                            .collect();
                        for a in &addrs {
                            swarm.behaviour_mut().kad.add_address(&peer_id, a.clone());
                        }
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                        dial_once(&mut swarm, peer_id, addrs);
                    }
                    SwarmEvent::Behaviour(VinxBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        message,
                        propagation_source,
                        ..
                    })) => {
                        // ADR 0022: rate-limit inbound messages per peer before any work.
                        if let guard::Admit::RateLimited { ban } =
                            guard.admit(propagation_source, std::time::Instant::now())
                        {
                            if ban {
                                warn!(peer = %propagation_source, "Banning peer for flooding (5 min)");
                                guard.ban(propagation_source, std::time::Instant::now());
                                swarm.behaviour_mut().gossipsub.blacklist_peer(&propagation_source);
                            }
                            continue;
                        }
                        let Some(msg) = P2pMessage::decode(&message.data) else {
                            if guard.penalize(propagation_source, guard::BAD_MESSAGE_PENALTY) {
                                warn!(peer = %propagation_source, "Banning peer for invalid messages (5 min)");
                                guard.ban(propagation_source, std::time::Instant::now());
                                swarm.behaviour_mut().gossipsub.blacklist_peer(&propagation_source);
                            }
                            continue;
                        };
                        if let Some(reply) =
                            dispatch(msg, &chain, &mempool, &state, &metrics, &consensus_tx).await
                        {
                            let topic = IdentTopic::new(reply.topic());
                            let _ = swarm.behaviour_mut().gossipsub.publish(topic, reply.encode());
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Handles one inbound message. Returns a reply to publish, if any (sync responses).
async fn dispatch(
    msg: P2pMessage,
    chain: &Arc<RwLock<Chain>>,
    mempool: &Arc<RwLock<Mempool>>,
    state: &Arc<RwLock<WorldState>>,
    metrics: &NodeMetrics,
    consensus_tx: &mpsc::UnboundedSender<NetEvent>,
) -> Option<P2pMessage> {
    match msg {
        P2pMessage::NewTransactions(txs) => {
            metrics
                .p2p_tx_recv
                .fetch_add(txs.len() as u64, Ordering::Relaxed);
            // Stateful admission before staging (anti-spam): same gate as the RPC path.
            let state = state.read().await;
            let admitted: Vec<Transaction> = txs
                .into_iter()
                .filter(|tx| match state.admission_check(tx) {
                    Ok(()) => true,
                    Err(e) => {
                        debug!(error = %e, "P2P transaction rejected at admission");
                        false
                    }
                })
                .collect();
            drop(state);
            let mut mempool = mempool.write().await;
            for tx in admitted {
                mempool.stage(tx);
            }
            None
        }
        P2pMessage::Proposal(p) => {
            let _ = consensus_tx.send(NetEvent::Proposal(p));
            None
        }
        P2pMessage::Vote(v) => {
            let _ = consensus_tx.send(NetEvent::Vote(v));
            None
        }
        P2pMessage::Committed { block, cert } => {
            metrics.p2p_blocks_recv.fetch_add(1, Ordering::Relaxed);
            let _ = consensus_tx.send(NetEvent::Committed { block, cert });
            None
        }
        P2pMessage::SyncRequest { from_height, limit } => {
            let c = chain.read().await;
            let tip = c.tip_height();
            // ADR 0022: clamp the count, saturate the range, trim to the byte budget.
            let limit = limit.min(messages::MAX_SYNC_RESPONSE_BLOCKS);
            let end = from_height.saturating_add(limit as u64).min(tip + 1);
            let candidates: Vec<(vinx_core::Block, vinx_core::CommitCert)> = (from_height..end)
                .map_while(|h| {
                    let row = c.row(h)?;
                    Some((row.block.clone(), row.commit.clone()?))
                })
                .collect();
            drop(c);
            let sizes: Vec<usize> = candidates
                .iter()
                .map(|r| borsh::to_vec(r).map(|v| v.len()).unwrap_or(usize::MAX))
                .collect();
            let take = messages::sync_batch_len(
                &sizes,
                messages::SYNC_RESPONSE_BUDGET_BYTES,
                messages::MAX_SYNC_RESPONSE_BLOCKS as usize,
            );
            let rows: Vec<_> = candidates.into_iter().take(take).collect();
            if rows.is_empty() {
                None
            } else {
                Some(P2pMessage::SyncResponse { rows })
            }
        }
        P2pMessage::SyncResponse { rows } => {
            let _ = consensus_tx.send(NetEvent::SyncRows(rows));
            None
        }
    }
}
