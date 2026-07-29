pub mod messages;

use std::collections::HashMap;
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
use rayon::prelude::*;
use std::sync::atomic::Ordering;
use vinx_core::{Block, BlockSignature, SlashEvidence, Transaction, ValidatorSet};
use vinx_crypto::{Address, Hash32};
use vinx_state::WorldState;

/// Returns the proposer's own co-signature on `block` (the one whose validator is the
/// block's proposer), if present. Used to assemble equivocation evidence.
fn proposer_signature(block: &Block) -> Option<&BlockSignature> {
    block
        .signatures
        .iter()
        .find(|s| s.validator == block.header.validator)
}

/// Auto-reports a proven equivocation (ADR 0003): builds a `SlashValidator` transaction
/// carrying the two-header evidence, submits it to the local mempool, and gossips it so
/// any validator can include it. Only registered validators report (the reporter must
/// have an on-chain account to be `from` and collect the bounty).
#[allow(clippy::too_many_arguments)]
async fn report_equivocation(
    target: Address,
    evidence: SlashEvidence,
    mempool: &Arc<RwLock<Mempool>>,
    state: &Arc<RwLock<WorldState>>,
    vs: &ValidatorSet,
    local_kp: &vinx_crypto::KeyPair,
    local_addr: &Address,
    swarm: &mut libp2p::Swarm<VinxBehaviour>,
) {
    if !vs.contains(local_addr) {
        debug!(%target, "Equivocation observed but this node is not a validator — not reporting");
        return;
    }
    // Bind the slash transaction to the node's real chain id and nonce.
    let (chain_id, nonce) = {
        let s = state.read().await;
        (
            s.chain_id,
            s.get_account(local_addr).map(|a| a.nonce).unwrap_or(0),
        )
    };
    let mut tx = Transaction::new_slash_validator(local_kp, target, &evidence, nonce);
    tx.chain_id = chain_id;
    tx.sign(local_kp); // re-sign so the signature commits to the real chain id

    if mempool.write().await.add(tx.clone()).is_ok() {
        let out = P2pMessage::NewTransaction(tx);
        let topic = IdentTopic::new(out.topic());
        let _ = swarm.behaviour_mut().gossipsub.publish(topic, out.encode());
        warn!(%target, "EQUIVOCATION — slash transaction submitted and gossiped");
    } else {
        debug!(%target, "Equivocation slash tx not admitted (already pending?)");
    }
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
    pub fn broadcast_signature(&self, height: u64, signature: BlockSignature) {
        let _ = self
            .cmd_tx
            .send(P2pCommand::Broadcast(P2pMessage::BlockCoSignature {
                height,
                signature,
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

const TOPICS: &[&str] = &["vinx/blocks/1", "vinx/txs/1", "vinx/sigs/1", "vinx/sync/1"];

pub async fn start(
    config: &NodeConfig,
    chain: Arc<RwLock<Chain>>,
    mempool: Arc<RwLock<Mempool>>,
    state: Arc<RwLock<WorldState>>,
    validator_set: Arc<RwLock<ValidatorSet>>,
    metrics: NodeMetrics,
) -> Result<P2pHandle, NodeError> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<P2pCommand>();

    let validator_secret = config.validator_keypair.secret_bytes();
    let p2p_secret = {
        use vinx_crypto::sha256;
        let mut derived = sha256(&validator_secret);
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
        )
        .await;
    });

    Ok(P2pHandle {
        cmd_tx,
        local_peer_id,
    })
}

const BAN_THRESHOLD: i32 = -5;

#[allow(clippy::too_many_arguments)]
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
) {
    let mut reputation: HashMap<PeerId, i32> = HashMap::new();

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
                    handle_swarm_event(event, &chain, &mempool, &state, &validator_set, &local_kp, &local_addr, &mut swarm, &mut reputation, &metrics).await;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_swarm_event(
    event: SwarmEvent<VinxBehaviourEvent>,
    chain: &Arc<RwLock<Chain>>,
    mempool: &Arc<RwLock<Mempool>>,
    state: &Arc<RwLock<WorldState>>,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    local_kp: &vinx_crypto::KeyPair,
    local_addr: &Address,
    swarm: &mut libp2p::Swarm<VinxBehaviour>,
    reputation: &mut HashMap<PeerId, i32>,
    metrics: &NodeMetrics,
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
            let Some(msg) = P2pMessage::decode(&message.data) else {
                // Undecipherable message — penalize sender
                let score = reputation.entry(propagation_source).or_insert(0);
                *score -= 1;
                if *score <= BAN_THRESHOLD {
                    warn!(peer = %propagation_source, score, "Banning peer for invalid messages");
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

/// Verifies all co-signatures on a block in parallel (rayon).
/// Returns `true` only if every signature has a valid pubkey→address binding
/// and a valid ed25519 signature over `block_hash`.
/// The proposer signature is included in the same pass.
fn verify_block_signatures_parallel(sigs: &[BlockSignature], block_hash: &Hash32) -> bool {
    sigs.par_iter().all(|sig| {
        if Address::from_public_key(&sig.pub_key) != sig.validator {
            return false;
        }
        sig.pub_key.verify(block_hash, &sig.signature).is_ok()
    })
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_message(
    msg: P2pMessage,
    chain: &Arc<RwLock<Chain>>,
    mempool: &Arc<RwLock<Mempool>>,
    state: &Arc<RwLock<WorldState>>,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    local_kp: &vinx_crypto::KeyPair,
    local_addr: &Address,
    swarm: &mut libp2p::Swarm<VinxBehaviour>,
    metrics: &NodeMetrics,
) {
    match msg {
        P2pMessage::NewTransaction(tx) => {
            metrics.p2p_tx_recv.fetch_add(1, Ordering::Relaxed);
            // Stage for deferred parallel verification (flush_staged() called at block production)
            mempool.write().await.stage(tx);
        }

        P2pMessage::NewBlock(block) => {
            let height = block.header.height;
            let vs = validator_set.read().await.clone();

            // 0. Equivocation detection (ADR 0003): a *different* block by the same
            //    proposer at a height we already hold — with a valid proposer signature
            //    on each — is a provable double-proposal. Assemble the evidence while we
            //    still hold both headers, then auto-report a SlashValidator transaction.
            let equivocation: Option<SlashEvidence> = {
                let chain_guard = chain.read().await;
                if height <= chain_guard.tip_height() {
                    chain_guard.get_block(height).and_then(|ours| {
                        let hb = block.hash();
                        let ha = ours.hash();
                        if ours.header.validator != block.header.validator || ha == hb {
                            return None;
                        }
                        let sig_a = proposer_signature(ours)?.clone();
                        let sig_b = proposer_signature(&block)?.clone();
                        // Both proposer signatures must be cryptographically valid over
                        // their respective headers, else it's just a bogus block.
                        let valid = Address::from_public_key(&sig_b.pub_key)
                            == block.header.validator
                            && sig_b.pub_key.verify(&hb, &sig_b.signature).is_ok()
                            && sig_a.pub_key.verify(&ha, &sig_a.signature).is_ok();
                        valid.then(|| SlashEvidence {
                            header_a: ours.header.clone(),
                            header_b: block.header.clone(),
                            sig_a,
                            sig_b,
                        })
                    })
                } else {
                    None
                }
            };
            if let Some(evidence) = equivocation {
                report_equivocation(
                    block.header.validator,
                    evidence,
                    mempool,
                    state,
                    &vs,
                    local_kp,
                    local_addr,
                    swarm,
                )
                .await;
            }

            // 1. Height and prev_hash linkage
            {
                let chain_guard = chain.read().await;
                let expected = chain_guard.tip_height() + 1;
                if height < expected {
                    debug!(height, "P2P block already seen");
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
            }

            // 2. Proposer authority
            if vs.leader_at(height) != &block.header.validator {
                warn!(height, "P2P block wrong proposer");
                return;
            }

            // 3. Signature verification — proposer must be present; all sigs verified in parallel.
            let block_hash = block.hash();
            if !block
                .signatures
                .iter()
                .any(|s| s.validator == block.header.validator)
            {
                warn!(height, "P2P block missing proposer sig");
                return;
            }
            if !verify_block_signatures_parallel(&block.signatures, &block_hash) {
                warn!(height, "P2P block has invalid signature(s)");
                return;
            }

            // 4. State transition with rollback
            let applied = {
                let mut sg = state.write().await;
                let snapshot = sg.clone();
                sg.set_block_context(block.header.timestamp);
                let mut ok = true;
                for tx in &block.transactions {
                    if let Err(e) = sg.apply_transaction(tx) {
                        warn!(height, error = %e, "P2P block tx failed, rolling back");
                        *sg = snapshot.clone();
                        ok = false;
                        break;
                    }
                }
                if ok {
                    sg.block_height = height;
                    sg.check_upgrade_activation();
                    let _ = sg.settle_block(&block.header.validator, block.header.timestamp);
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
                chain.write().await.push(block.clone());
            }
            info!(height, "P2P: block validated and applied");
            metrics.p2p_blocks_recv.fetch_add(1, Ordering::Relaxed);

            // 6. Co-sign if we're a validator
            let vs = validator_set.read().await.clone();
            if vs.contains(local_addr) {
                let sig = BlockSignature {
                    validator: *local_addr,
                    pub_key: local_kp.public_key(),
                    signature: local_kp.sign(&block_hash),
                };
                let co_msg = P2pMessage::BlockCoSignature {
                    height,
                    signature: sig.clone(),
                };
                let topic = IdentTopic::new(co_msg.topic());
                let _ = swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic, co_msg.encode());
                debug!(height, "Co-signed block");

                let mut c = chain.write().await;
                c.record_signature(local_addr, height, block_hash);
                let finalized = c.add_co_signature(height, sig, &vs);
                if finalized {
                    c.advance_finality(&vs); // ADR 0002
                    info!(height, "Block finalized after co-signing");
                }
            }
        }

        P2pMessage::BlockCoSignature { height, signature } => {
            // Pubkey/address consistency
            if Address::from_public_key(&signature.pub_key) != signature.validator {
                warn!(height, "P2P co-sig pubkey mismatch");
                return;
            }
            let vs = validator_set.read().await.clone();
            if !vs.contains(&signature.validator) {
                debug!(height, "P2P co-sig from non-validator");
                return;
            }
            // Get block hash for sig verification
            let block_hash = {
                let c = chain.read().await;
                match c.get_block(height) {
                    Some(b) => b.hash(),
                    None => {
                        debug!(height, "P2P co-sig for unknown height");
                        return;
                    }
                }
            };
            if signature
                .pub_key
                .verify(&block_hash, &signature.signature)
                .is_err()
            {
                warn!(height, validator = %signature.validator, "P2P co-sig invalid crypto");
                return;
            }
            // Double-sign (equivocation) detection
            let mut c = chain.write().await;
            if c.record_signature(&signature.validator, height, block_hash) {
                warn!(height, validator = %signature.validator, "EQUIVOCATION: double-sign detected, dropping");
                return;
            }
            let finalized = c.add_co_signature(height, signature, &vs);
            if finalized {
                c.advance_finality(&vs); // ADR 0002
                info!(height, "Block finalized via co-signatures");
            }
        }

        // Respond to sync requests with our stored blocks
        P2pMessage::SyncRequest { from_height, limit } => {
            let c = chain.read().await;
            let tip = c.tip_height();
            let end = (from_height + limit as u64).min(tip + 1);
            let blocks: Vec<Block> = (from_height..end)
                .filter_map(|h| c.get_block(h).cloned())
                .collect();
            drop(c);
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
                if vs.leader_at(height) != &block.header.validator {
                    warn!(height, "SyncResponse block wrong proposer");
                    break;
                }
                if block.header.prev_hash != chain.read().await.tip_hash() {
                    warn!(height, "SyncResponse block wrong prev_hash");
                    break;
                }
                // Verify all bundled co-signatures in parallel before applying state.
                let block_hash = block.hash();
                if !block.signatures.is_empty()
                    && !verify_block_signatures_parallel(&block.signatures, &block_hash)
                {
                    warn!(height, "SyncResponse block has invalid signature(s)");
                    break;
                }
                let ok = {
                    let mut sg = state.write().await;
                    let snapshot = sg.clone();
                    sg.set_block_context(block.header.timestamp);
                    let mut ok = true;
                    for tx in &block.transactions {
                        if let Err(e) = sg.apply_transaction(tx) {
                            warn!(height, error=%e, "SyncResponse tx failed");
                            *sg = snapshot.clone();
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        sg.block_height = height;
                        sg.check_upgrade_activation();
                        let _ = sg.settle_block(&block.header.validator, block.header.timestamp);
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
                    chain.write().await.push(block);
                    info!(height, "Block applied via P2P sync");
                } else {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::block::GENESIS_PREV_HASH;
    use vinx_core::BlockHeader;
    use vinx_crypto::KeyPair;

    fn signed_block(kp: &KeyPair, validator: Address, height: u64, tag: u8) -> Block {
        let header = BlockHeader {
            height,
            prev_hash: GENESIS_PREV_HASH,
            timestamp: 0,
            validator,
            tx_count: 0,
            state_root: [tag; 32],
            base_fee: 0,
            receipts_root: [0u8; 32],
        };
        let sig = BlockSignature {
            validator,
            pub_key: kp.public_key(),
            signature: kp.sign(&header.hash()),
        };
        Block {
            header,
            transactions: vec![],
            signatures: vec![sig],
        }
    }

    #[test]
    fn test_proposer_signature_found() {
        let kp = KeyPair::generate();
        let v = Address::from_public_key(&kp.public_key());
        let b = signed_block(&kp, v, 5, 0xAA);
        assert_eq!(proposer_signature(&b).unwrap().validator, v);
    }

    #[test]
    fn test_equivocation_evidence_is_well_formed() {
        // Two different blocks at the same height, both signed by the same proposer,
        // assemble into evidence whose signatures verify over their own header hashes.
        let kp = KeyPair::generate();
        let v = Address::from_public_key(&kp.public_key());
        let a = signed_block(&kp, v, 5, 0xAA);
        let b = signed_block(&kp, v, 5, 0xBB);
        assert_eq!(a.header.height, b.header.height);
        assert_ne!(a.hash(), b.hash());

        let sig_a = proposer_signature(&a).unwrap().clone();
        let sig_b = proposer_signature(&b).unwrap().clone();
        assert!(sig_a.pub_key.verify(&a.hash(), &sig_a.signature).is_ok());
        assert!(sig_b.pub_key.verify(&b.hash(), &sig_b.signature).is_ok());

        let evidence = SlashEvidence {
            header_a: a.header.clone(),
            header_b: b.header.clone(),
            sig_a,
            sig_b,
        };
        assert_eq!(evidence.header_a.height, evidence.header_b.height);
        assert_ne!(evidence.header_a.hash(), evidence.header_b.hash());
    }
}
