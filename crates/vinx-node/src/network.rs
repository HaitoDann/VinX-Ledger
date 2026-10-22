//! Réseau P2P (SPEC §8.1) : libp2p, liste de pairs autorisés, relais et rattrapage.
//!
//! - Transport TCP chiffré et authentifié (Noise), multiplexé (Yamux).
//! - Seuls les pairs de la liste autorisée peuvent se connecter.
//! - Gossipsub relaie propositions, votes et transactions ; chaque message est
//!   identifié par le hachage de son contenu, donc relayé une seule fois.
//! - Un protocole requête-réponse sert les blocs finalisés et leurs certificats
//!   aux nœuds en retard.

use crate::node::{SharedRef, lock};
use crate::snapshot::SnapshotInfo;
use anyhow::{Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{
    Multiaddr, PeerId, StreamProtocol, Swarm, SwarmBuilder, allow_block_list, gossipsub, identity,
    noise, tcp, yamux,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use vinx_consensus::{Evidence, SignedProposal, SignedVote};
use vinx_core::{
    Account, Block, CommitCertificate, SignedAdminTransaction, SignedTransaction, encode,
};
use vinx_crypto::Address;

/// Messages relayés par gossip.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum NetMessage {
    Proposal(SignedProposal) = 0x01,
    Vote(SignedVote) = 0x02,
    Tx(SignedTransaction) = 0x03,
    AdminTx(SignedAdminTransaction) = 0x04,
    /// Preuve d'équivoque d'un validateur.
    Evidence(Box<Evidence>) = 0x05,
}

/// Enveloppe gossip. Le numéro de séquence rend chaque envoi unique : gossipsub
/// déduplique par contenu, et un message retransmis doit pouvoir repartir.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
struct Envelope {
    seq: u64,
    msg: NetMessage,
}

/// Requêtes du protocole de rattrapage `/vinx/sync/2`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum SyncRequest {
    /// Blocs finalisés à partir de cette hauteur.
    Blocks { from_height: u64 } = 0x01,
    /// Description de l'instantané d'état le plus récent.
    SnapshotInfo = 0x02,
    /// Comptes de l'instantané de `height`, après `after`.
    SnapshotChunk { height: u64, after: Option<Address> } = 0x03,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum SyncResponse {
    /// `lowest` : plus petite hauteur encore conservée par le pair.
    Blocks {
        lowest: u64,
        blocks: Vec<(Block, CommitCertificate)>,
    } = 0x01,
    SnapshotInfo(Option<Box<SnapshotInfo>>) = 0x02,
    /// `None` si le pair ne sert plus l'instantané de cette hauteur.
    SnapshotChunk {
        height: u64,
        chunk: Option<Vec<(Address, Account)>>,
        done: bool,
    } = 0x03,
}

/// Blocs servis par réponse de rattrapage.
pub const SYNC_BATCH: u64 = 50;
/// Taille maximale d'un message réseau.
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Taille visée d'une réponse de rattrapage : nettement sous `MAX_MESSAGE_BYTES`,
/// car 50 blocs pleins (4 Mio chacun) la dépasseraient.
const SYNC_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Messages gossip relayés par seconde au plus, par pair.
const MAX_MESSAGES_PER_SEC: u32 = 1_000;
/// Poids d'un message invalide dans le quota : un pair qui en envoie est vite coupé.
const INVALID_WEIGHT: u32 = 100;
/// Requêtes de rattrapage par seconde au plus, par pair.
const MAX_SYNC_PER_SEC: u32 = 4;
/// Durée d'exclusion d'un pair qui dépasse ses quotas.
const BAN: Duration = Duration::from_secs(60);

/// Compteurs d'un pair sur la fenêtre d'une seconde en cours.
#[derive(Default)]
struct Usage {
    window: Option<Instant>,
    messages: u32,
    sync: u32,
    banned_until: Option<Instant>,
}

#[derive(Default)]
struct Quotas {
    peers: BTreeMap<PeerId, Usage>,
}

impl Quotas {
    fn usage(&mut self, peer: PeerId, now: Instant) -> &mut Usage {
        let u = self.peers.entry(peer).or_default();
        if u.window
            .is_none_or(|w| now.duration_since(w) >= Duration::from_secs(1))
        {
            u.window = Some(now);
            u.messages = 0;
            u.sync = 0;
        }
        u
    }

    fn is_banned(&self, peer: &PeerId, now: Instant) -> bool {
        self.peers
            .get(peer)
            .and_then(|u| u.banned_until)
            .is_some_and(|t| now < t)
    }

    /// Compte un message gossip ; renvoie `false` si le pair dépasse son quota.
    fn charge_message(&mut self, peer: PeerId, valid: bool, now: Instant) -> bool {
        let u = self.usage(peer, now);
        u.messages = u
            .messages
            .saturating_add(if valid { 1 } else { INVALID_WEIGHT });
        if u.messages > MAX_MESSAGES_PER_SEC {
            u.banned_until = now.checked_add(BAN);
            return false;
        }
        true
    }

    /// Compte une requête de rattrapage ; renvoie `false` au-delà du quota.
    fn charge_sync(&mut self, peer: PeerId, now: Instant) -> bool {
        let u = self.usage(peer, now);
        u.sync = u.sync.saturating_add(1);
        u.sync <= MAX_SYNC_PER_SEC
    }
}

/// Ce que le réseau transmet au nœud.
#[derive(Debug)]
pub enum NetEvent {
    Message(Box<NetMessage>),
    /// Réponse de rattrapage, avec le pair qui l'a envoyée.
    Sync(PeerId, Box<SyncResponse>),
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
    mut requests: mpsc::UnboundedReceiver<(PeerId, SyncRequest)>,
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
                // Un message n'est relayé qu'après validation par le nœud.
                .validate_messages()
                .build()
                .map_err(|e| io::Error::other(e.to_string()))?;
            let gossip = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossip_config,
            )
            .map_err(|e| io::Error::other(e.to_string()))?;
            let sync = request_response::Behaviour::with_codec(
                SyncCodec,
                [(StreamProtocol::new("/vinx/sync/2"), ProtocolSupport::Full)],
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
    let mut quotas = Quotas::default();
    let mut seq: u64 = 0;
    let mut redial = tokio::time::interval(Duration::from_secs(3));
    let mut sync_tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            Some(msg) = outbound.recv() => {
                seq = seq.wrapping_add(1);
                let data = encode(&Envelope { seq, msg });
                if let Err(e) = swarm.behaviour_mut().gossip.publish(topic.clone(), data) {
                    // Pas encore de pair, ou message déjà diffusé : sans conséquence.
                    debug!("diffusion impossible : {e}");
                }
            }
            Some((peer, request)) = requests.recv() => {
                swarm.behaviour_mut().sync.send_request(&peer, request);
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
                        swarm.behaviour_mut().sync.send_request(peer, SyncRequest::Blocks { from_height: next });
                    }
                }
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    if quotas.is_banned(&peer_id, Instant::now()) {
                        let _ = swarm.disconnect_peer_id(peer_id);
                    } else {
                        info!(%peer_id, "pair connecté");
                        connected.insert(peer_id);
                    }
                }
                SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
                    info!(%peer_id, "pair déconnecté");
                    connected.remove(&peer_id);
                }
                SwarmEvent::Behaviour(BehaviourEvent::Gossip(gossipsub::Event::Message {
                    propagation_source,
                    message_id,
                    message,
                })) => {
                    let decoded = borsh::from_slice::<Envelope>(&message.data).ok().map(|e| e.msg);
                    let within_quota = quotas.charge_message(propagation_source, decoded.is_some(), Instant::now());
                    let verdict = match (decoded, within_quota) {
                        (Some(msg), true) => {
                            let _ = inbound.send(NetEvent::Message(Box::new(msg)));
                            gossipsub::MessageAcceptance::Accept
                        }
                        (None, true) => gossipsub::MessageAcceptance::Reject,
                        (_, false) => {
                            warn!(peer_id = %propagation_source, "quota dépassé : pair exclu {} s", BAN.as_secs());
                            let _ = swarm.disconnect_peer_id(propagation_source);
                            gossipsub::MessageAcceptance::Ignore
                        }
                    };
                    let _ = swarm
                        .behaviour_mut()
                        .gossip
                        .report_message_validation_result(&message_id, &propagation_source, verdict);
                }
                SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::Message { peer, message, .. })) => {
                    match message {
                        request_response::Message::Request { request, channel, .. } => {
                            if quotas.charge_sync(peer, Instant::now()) {
                                let response = serve(&shared, request);
                                let _ = swarm.behaviour_mut().sync.send_response(channel, response);
                            }
                        }
                        request_response::Message::Response { response, .. } => {
                            let _ = inbound.send(NetEvent::Sync(peer, Box::new(response)));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn serve(shared: &SharedRef, request: SyncRequest) -> SyncResponse {
    match request {
        SyncRequest::Blocks { from_height } => {
            let lowest = lock(shared)
                .store
                .lowest_height()
                .ok()
                .flatten()
                .unwrap_or(0);
            SyncResponse::Blocks {
                lowest,
                blocks: serve_blocks(shared, from_height),
            }
        }
        SyncRequest::SnapshotInfo => {
            let s = lock(shared);
            SyncResponse::SnapshotInfo(s.snapshot.as_ref().map(|snap| Box::new(snap.info())))
        }
        SyncRequest::SnapshotChunk { height, after } => {
            let s = lock(shared);
            match s
                .snapshot
                .as_ref()
                .filter(|snap| snap.block.header.height == height)
            {
                Some(snap) => {
                    let (chunk, done) = snap.chunk(after.as_ref());
                    SyncResponse::SnapshotChunk {
                        height,
                        chunk: Some(chunk),
                        done,
                    }
                }
                None => SyncResponse::SnapshotChunk {
                    height,
                    chunk: None,
                    done: false,
                },
            }
        }
    }
}

fn serve_blocks(shared: &SharedRef, from: u64) -> Vec<(Block, CommitCertificate)> {
    let s = lock(shared);
    let mut out = Vec::new();
    let mut h = from;
    let mut bytes = 0usize;
    while u64::try_from(out.len()).unwrap_or(u64::MAX) < SYNC_BATCH && h <= s.chain.state.height {
        match (s.store.block(h), s.store.certificate(h)) {
            (Ok(Some(b)), Ok(Some(c))) => {
                bytes = bytes.saturating_add(b.encoded_len());
                // Toujours au moins un bloc, sinon un nœud en retard ne progresserait plus.
                if !out.is_empty() && bytes > SYNC_RESPONSE_BYTES {
                    break;
                }
                out.push((b, c));
            }
            _ => break,
        }
        h = h.saturating_add(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flooding_peer_is_banned_then_released() {
        let mut q = Quotas::default();
        let peer = PeerId::random();
        let t0 = Instant::now();
        for _ in 0..MAX_MESSAGES_PER_SEC {
            assert!(q.charge_message(peer, true, t0));
        }
        assert!(!q.charge_message(peer, true, t0));
        assert!(q.is_banned(&peer, t0));
        assert!(!q.is_banned(&peer, t0 + BAN + Duration::from_millis(1)));
        // Un autre pair n'est pas affecté.
        assert!(q.charge_message(PeerId::random(), true, t0));
    }

    #[test]
    fn invalid_messages_exhaust_the_quota_quickly() {
        let mut q = Quotas::default();
        let peer = PeerId::random();
        let t0 = Instant::now();
        let allowed = (0..20)
            .take_while(|_| q.charge_message(peer, false, t0))
            .count();
        assert_eq!(allowed, (MAX_MESSAGES_PER_SEC / INVALID_WEIGHT) as usize);
    }

    #[test]
    fn quota_window_resets_every_second() {
        let mut q = Quotas::default();
        let peer = PeerId::random();
        let t0 = Instant::now();
        for _ in 0..MAX_SYNC_PER_SEC {
            assert!(q.charge_sync(peer, t0));
        }
        assert!(!q.charge_sync(peer, t0));
        assert!(q.charge_sync(peer, t0 + Duration::from_secs(1)));
    }
}
