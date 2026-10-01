pub mod guard;
pub mod messages;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use libp2p::{
    gossipsub::{self, IdentTopic, TopicHash},
    identify, mdns, noise,
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
        self.broadcast(P2pMessage::NewTransaction(tx.clone()));
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

    tokio::spawn(async move {
        run_event_loop(swarm, cmd_rx, chain, mempool, state, metrics, consensus_tx).await;
    });

    Ok(P2pHandle {
        cmd_tx,
        local_peer_id,
    })
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

async fn run_event_loop(
    mut swarm: libp2p::Swarm<VinxBehaviour>,
    mut cmd_rx: mpsc::UnboundedReceiver<P2pCommand>,
    chain: Arc<RwLock<Chain>>,
    mempool: Arc<RwLock<Mempool>>,
    state: Arc<RwLock<WorldState>>,
    metrics: NodeMetrics,
    consensus_tx: mpsc::UnboundedSender<NetEvent>,
) {
    let mut guard = guard::PeerGuard::new();
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
                let Some(event) = event else { continue };
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!(address = %address, "P2P listening");
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        info!(peer = %peer_id, "Peer connected");
                    }
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        debug!(peer = %peer_id, "Peer disconnected");
                        guard.forget(&peer_id);
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
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                        dial_once(&mut swarm, peer_id, info.listen_addrs);
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
                                warn!(peer = %propagation_source, "Banning peer for flooding");
                                swarm.behaviour_mut().gossipsub.blacklist_peer(&propagation_source);
                            }
                            continue;
                        }
                        let Some(msg) = P2pMessage::decode(&message.data) else {
                            if guard.penalize(propagation_source, guard::BAD_MESSAGE_PENALTY) {
                                warn!(peer = %propagation_source, "Banning peer for invalid messages");
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
        P2pMessage::NewTransaction(tx) => {
            metrics.p2p_tx_recv.fetch_add(1, Ordering::Relaxed);
            // Stateful admission before staging (anti-spam): same gate as the RPC path.
            if let Err(e) = state.read().await.admission_check(&tx) {
                debug!(error = %e, "P2P transaction rejected at admission");
                return None;
            }
            mempool.write().await.stage(tx);
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
