//! Réseau P2P (SPEC §8.1) : libp2p, liste de pairs autorisés, relais et rattrapage.
//!
//! - Transport TCP chiffré et authentifié (Noise), multiplexé (Yamux).
//! - Seuls les pairs de la liste autorisée peuvent se connecter.
//! - Gossipsub relaie propositions, votes et transactions ; chaque message est
//!   identifié par le hachage de son contenu, donc relayé une seule fois.
//! - Un protocole requête-réponse sert les blocs finalisés et leurs certificats
//!   aux nœuds en retard.

use crate::node::{SharedRef, lock};
use anyhow::{Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{
    Multiaddr, PeerId, StreamProtocol, Swarm, SwarmBuilder, allow_block_list, gossipsub, identity,
    noise, tcp, yamux,
};
use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use vinx_consensus::{SignedProposal, SignedVote};
use vinx_core::{Block, CommitCertificate, SignedAdminTransaction, SignedTransaction, encode};

/// Messages relayés par gossip.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum NetMessage {
    Proposal(SignedProposal) = 0x01,
    Vote(SignedVote) = 0x02,
    Tx(SignedTransaction) = 0x03,
    AdminTx(SignedAdminTransaction) = 0x04,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SyncRequest {
    pub from_height: u64,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SyncResponse {
    pub blocks: Vec<(Block, CommitCertificate)>,
}

/// Blocs servis par réponse de rattrapage.
pub const SYNC_BATCH: u64 = 50;
/// Taille maximale d'un message réseau.
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Ce que le réseau transmet au nœud.
#[derive(Debug)]
pub enum NetEvent {
    Message(Box<NetMessage>),
    Blocks(Vec<(Block, CommitCertificate)>),
}

#[derive(Clone, Copy, Default)]
pub struct SyncCodec;

async fn read_borsh<T: BorshDeserialize, R: futures::AsyncRead + Unpin + Send>(
    io: &mut R,
) -> io::Result<T> {
    let mut bytes = Vec::new();
    io.take(u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .await?;
    borsh::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

async fn write_borsh<T: BorshSerialize, W: futures::AsyncWrite + Unpin + Send>(
    io: &mut W,
    value: &T,
) -> io::Result<()> {
    io.write_all(&encode(value)).await?;
    io.close().await
}

#[async_trait::async_trait]
impl request_response::Codec for SyncCodec {
    type Protocol = StreamProtocol;
    type Request = SyncRequest;
    type Response = SyncResponse;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<SyncRequest>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        read_borsh(io).await
    }

    async fn read_response<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<SyncResponse>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        read_borsh(io).await
    }

    async fn write_request<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        req: SyncRequest,
    ) -> io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        write_borsh(io, &req).await
    }

    async fn write_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        res: SyncResponse,
    ) -> io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        write_borsh(io, &res).await
    }
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    allow: allow_block_list::Behaviour<allow_block_list::AllowedPeers>,
    gossip: gossipsub::Behaviour,
    sync: request_response::Behaviour<SyncCodec>,
}

pub struct NetConfig {
    pub listen: Multiaddr,
    /// Pairs à joindre ; ils sont autorisés d'office.
    pub peers: Vec<Multiaddr>,
    /// Pairs supplémentaires autorisés à se connecter.
    pub allowed: Vec<PeerId>,
}

/// Clé d'identité réseau, distincte des clés de consensus. Créée au premier démarrage.
pub fn load_or_create_identity(data_dir: &Path) -> Result<identity::Keypair> {
    let path = data_dir.join("network.key");
    if path.exists() {
        let mut bytes =
            std::fs::read(&path).with_context(|| format!("lecture de {}", path.display()))?;
        return identity::Keypair::ed25519_from_bytes(&mut bytes).context("clé réseau invalide");
    }
    let keypair = identity::Keypair::generate_ed25519();
    let secret = keypair
        .clone()
        .try_into_ed25519()
        .context("clé réseau")?
        .secret();
    std::fs::create_dir_all(data_dir)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options.open(&path)?.write_all(secret.as_ref())?;
    Ok(keypair)
}

/// Extrait l'identifiant de pair d'une adresse `/…/p2p/<id>`.
pub fn peer_of(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|p| match p {
        libp2p::multiaddr::Protocol::P2p(id) => Some(id),
        _ => None,
    })
}

fn topic(chain_id: &str) -> gossipsub::IdentTopic {
    gossipsub::IdentTopic::new(format!("vinx/{chain_id}/1"))
}

/// Réseau en cours d'exécution : envoie au nœud les messages reçus, diffuse
/// ceux qu'il reçoit sur `outbound`, sert et demande les blocs manquants.
pub async fn run(
    identity: identity::Keypair,
    config: NetConfig,
    shared: SharedRef,
    mut outbound: mpsc::UnboundedReceiver<NetMessage>,
    inbound: mpsc::UnboundedSender<NetEvent>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let chain_id = lock(&shared).chain.state.chain_id.clone();
    let topic = topic(&chain_id);
    let mut swarm: Swarm<Behaviour> = SwarmBuilder::with_existing_identity(identity)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            let gossip_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_millis(500))
                .validation_mode(gossipsub::ValidationMode::Strict)
                .message_id_fn(|m: &gossipsub::Message| {
                    gossipsub::MessageId::from(blake3::hash(&m.data).as_bytes().to_vec())
                })
                .max_transmit_size(MAX_MESSAGE_BYTES)
                .build()
                .map_err(|e| io::Error::other(e.to_string()))?;
            let gossip = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossip_config,
            )
            .map_err(|e| io::Error::other(e.to_string()))?;
            let sync = request_response::Behaviour::with_codec(
                SyncCodec,
                [(StreamProtocol::new("/vinx/sync/1"), ProtocolSupport::Full)],
                request_response::Config::default().with_request_timeout(Duration::from_secs(10)),
            );
            Ok(Behaviour {
                allow: allow_block_list::Behaviour::default(),
                gossip,
                sync,
            })
        })
        .map_err(|e| anyhow::anyhow!("réseau : {e}"))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(3600)))
        .build();

    swarm.behaviour_mut().gossip.subscribe(&topic)?;
    let dial_targets: Vec<(PeerId, Multiaddr)> = config
        .peers
        .iter()
        .filter_map(|a| Some((peer_of(a)?, a.clone())))
        .collect();
    for peer in config
        .allowed
        .iter()
        .chain(dial_targets.iter().map(|(p, _)| p))
    {
        swarm.behaviour_mut().allow.allow_peer(*peer);
    }
    swarm.listen_on(config.listen.clone())?;
    info!(peer_id = %swarm.local_peer_id(), listen = %config.listen, "réseau démarré");

    let mut connected: BTreeSet<PeerId> = BTreeSet::new();
    let mut redial = tokio::time::interval(Duration::from_secs(3));
    let mut sync_tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            Some(msg) = outbound.recv() => {
                if let Err(e) = swarm.behaviour_mut().gossip.publish(topic.clone(), encode(&msg)) {
                    // Pas encore de pair, ou message déjà diffusé : sans conséquence.
                    debug!("diffusion impossible : {e}");
                }
            }
            _ = redial.tick() => {
                for (peer, addr) in &dial_targets {
                    if !connected.contains(peer) {
                        let _ = swarm.dial(addr.clone());
                    }
                }
            }
            _ = sync_tick.tick() => {
                // Demande périodique des blocs suivants : sans réponse utile, rien ne change.
                let next = lock(&shared).chain.next_height();
                let peers: Vec<PeerId> = connected.iter().copied().collect();
                if !peers.is_empty() {
                    let pick = usize::try_from(next).unwrap_or(0).checked_rem(peers.len()).unwrap_or(0);
                    if let Some(peer) = peers.get(pick) {
                        swarm.behaviour_mut().sync.send_request(peer, SyncRequest { from_height: next });
                    }
                }
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!(%peer_id, "pair connecté");
                    connected.insert(peer_id);
                }
                SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
                    info!(%peer_id, "pair déconnecté");
                    connected.remove(&peer_id);
                }
                SwarmEvent::Behaviour(BehaviourEvent::Gossip(gossipsub::Event::Message { message, .. })) => {
                    match borsh::from_slice::<NetMessage>(&message.data) {
                        Ok(msg) => { let _ = inbound.send(NetEvent::Message(Box::new(msg))); }
                        Err(_) => warn!("message réseau mal encodé ignoré"),
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::Message { message, .. })) => {
                    match message {
                        request_response::Message::Request { request, channel, .. } => {
                            let blocks = serve_blocks(&shared, request.from_height);
                            let _ = swarm.behaviour_mut().sync.send_response(channel, SyncResponse { blocks });
                        }
                        request_response::Message::Response { response, .. } => {
                            if !response.blocks.is_empty() {
                                let _ = inbound.send(NetEvent::Blocks(response.blocks));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn serve_blocks(shared: &SharedRef, from: u64) -> Vec<(Block, CommitCertificate)> {
    let s = lock(shared);
    let mut out = Vec::new();
    let mut h = from;
    while u64::try_from(out.len()).unwrap_or(u64::MAX) < SYNC_BATCH && h <= s.chain.state.height {
        match (s.store.block(h), s.store.certificate(h)) {
            (Ok(Some(b)), Ok(Some(c))) => out.push((b, c)),
            _ => break,
        }
        h = h.saturating_add(1);
    }
    out
}
