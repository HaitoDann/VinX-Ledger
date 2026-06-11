pub mod messages;

use std::sync::Arc;
use std::time::Duration;

use libp2p::{
    gossipsub::{self, IdentTopic, TopicHash},
    identify,
    noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, SwarmBuilder,
};
use futures::StreamExt;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info};

use crate::{chain::Chain, config::NodeConfig, mempool::Mempool, NodeError};
use messages::P2pMessage;
use vinx_core::{Block, BlockSignature, Transaction};

/// Commands sent from the rest of the node to the P2P event loop.
pub enum P2pCommand {
    Broadcast(P2pMessage),
    /// Gracefully stop the P2P service.
    Shutdown,
}

/// Thin handle used by the rest of the node to interact with the P2P layer.
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
            .send(P2pCommand::Broadcast(P2pMessage::NewTransaction(tx.clone())));
    }

    pub fn broadcast_signature(&self, height: u64, signature: BlockSignature) {
        let _ = self.cmd_tx.send(P2pCommand::Broadcast(
            P2pMessage::BlockCoSignature { height, signature },
        ));
    }

    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(P2pCommand::Shutdown);
    }
}

#[derive(NetworkBehaviour)]
struct VinxBehaviour {
    gossipsub: gossipsub::Behaviour,
    identify: identify::Behaviour,
}

const TOPICS: &[&str] = &["vinx/blocks/1", "vinx/txs/1", "vinx/sigs/1"];

/// Starts the P2P service and returns a handle.
///
/// The event loop runs in a background tokio task and dispatches received
/// messages directly to the shared node state (mempool / chain).
pub async fn start(
    config: &NodeConfig,
    chain: Arc<RwLock<Chain>>,
    mempool: Arc<RwLock<Mempool>>,
) -> Result<P2pHandle, NodeError> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<P2pCommand>();

    let validator_secret = config.validator_keypair.secret_bytes();
    let p2p_secret = {
        use vinx_crypto::sha256;
        // Deterministically derive a libp2p identity from the validator key
        let mut derived = sha256(&validator_secret);
        derived[0] &= 0xf8; // Clamp for Ed25519 scalar
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
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)
        .map_err(|e| NodeError::Config(format!("P2P TCP setup failed: {e}")))?
        .with_behaviour(|key| {
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(1))
                .validation_mode(gossipsub::ValidationMode::Strict)
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

            Ok(VinxBehaviour { gossipsub, identify })
        })
        .map_err(|e| NodeError::Config(format!("P2P behaviour setup failed: {e}")))?
        .build();

    // Subscribe to all topics
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

    // Dial bootstrap peers
    for peer_addr in &config.peer_addrs {
        if let Ok(addr) = peer_addr.parse::<Multiaddr>() {
            let _ = swarm.dial(addr);
        }
    }

    info!(peer_id = %local_peer_id, listen = %listen_addr, "P2P service started");

    let validator_set = config.validator_set.clone();
    let local_kp = config.validator_keypair.clone();
    let local_addr = config.validator_address.clone();

    // Spawn the event loop
    tokio::spawn(async move {
        run_event_loop(
            swarm,
            cmd_rx,
            chain,
            mempool,
            validator_set,
            local_kp,
            local_addr,
        )
        .await;
    });

    Ok(P2pHandle { cmd_tx, local_peer_id })
}

async fn run_event_loop(
    mut swarm: libp2p::Swarm<VinxBehaviour>,
    mut cmd_rx: mpsc::UnboundedReceiver<P2pCommand>,
    chain: Arc<RwLock<Chain>>,
    mempool: Arc<RwLock<Mempool>>,
    validator_set: vinx_core::ValidatorSet,
    local_kp: vinx_crypto::KeyPair,
    local_addr: vinx_crypto::Address,
) {
    loop {
        tokio::select! {
            // Outbound commands from the node
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(P2pCommand::Broadcast(msg)) => {
                        let topic = IdentTopic::new(msg.topic());
                        let data = msg.encode();
                        if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, data) {
                            debug!(error = %e, "Gossip publish failed");
                        }
                    }
                    Some(P2pCommand::Shutdown) | None => {
                        info!("P2P event loop shutting down");
                        break;
                    }
                }
            }

            // Inbound swarm events
            event = swarm.next() => {
                if let Some(event) = event {
                    handle_swarm_event(
                        event,
                        &chain,
                        &mempool,
                        &validator_set,
                        &local_kp,
                        &local_addr,
                        &mut swarm,
                    )
                    .await;
                }
            }
        }
    }
}

async fn handle_swarm_event(
    event: SwarmEvent<VinxBehaviourEvent>,
    chain: &Arc<RwLock<Chain>>,
    mempool: &Arc<RwLock<Mempool>>,
    validator_set: &vinx_core::ValidatorSet,
    local_kp: &vinx_crypto::KeyPair,
    local_addr: &vinx_crypto::Address,
    swarm: &mut libp2p::Swarm<VinxBehaviour>,
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
        }
        SwarmEvent::Behaviour(VinxBehaviourEvent::Gossipsub(
            gossipsub::Event::Message { message, .. },
        )) => {
            let Some(msg) = P2pMessage::decode(&message.data) else { return };
            dispatch_message(msg, chain, mempool, validator_set, local_kp, local_addr, swarm).await;
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

async fn dispatch_message(
    msg: P2pMessage,
    chain: &Arc<RwLock<Chain>>,
    mempool: &Arc<RwLock<Mempool>>,
    validator_set: &vinx_core::ValidatorSet,
    local_kp: &vinx_crypto::KeyPair,
    local_addr: &vinx_crypto::Address,
    swarm: &mut libp2p::Swarm<VinxBehaviour>,
) {
    match msg {
        P2pMessage::NewTransaction(tx) => {
            let mut mp = mempool.write().await;
            match mp.add(tx) {
                Ok(()) => debug!("P2P: transaction added to mempool"),
                Err(e) => debug!(error = %e, "P2P: transaction rejected"),
            }
        }

        P2pMessage::NewBlock(block) => {
            let height = block.header.height;
            // If we are a co-validator for this block, sign it and broadcast our signature
            if validator_set.contains(local_addr) {
                let expected = validator_set.leader_at(height);
                if expected == &block.header.validator {
                    let header_hash = block.hash();
                    let sig = BlockSignature {
                        validator: local_addr.clone(),
                        pub_key: local_kp.public_key(),
                        signature: local_kp.sign(&header_hash),
                    };
                    let co_sig_msg = P2pMessage::BlockCoSignature {
                        height,
                        signature: sig.clone(),
                    };
                    let topic = IdentTopic::new(co_sig_msg.topic());
                    let _ = swarm.behaviour_mut().gossipsub.publish(topic, co_sig_msg.encode());
                    debug!(height, "Co-signed block and broadcast signature");

                    // Add signature to local chain
                    let mut c = chain.write().await;
                    let finalized = c.add_co_signature(height, sig, validator_set);
                    if finalized {
                        info!(height, "Block finalized after co-signing");
                    }
                }
            }
        }

        P2pMessage::BlockCoSignature { height, signature } => {
            let mut c = chain.write().await;
            let finalized = c.add_co_signature(height, signature, validator_set);
            if finalized {
                info!(height, "Block finalized via P2P signature collection");
            }
        }
    }
}
