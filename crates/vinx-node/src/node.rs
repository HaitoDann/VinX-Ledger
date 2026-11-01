//! Boucle principale : exécute les actions du moteur de consensus.
//!
//! Le nœud réagit aux actions du moteur, aux messages du réseau, aux délais et
//! aux blocs reçus par rattrapage. Sans clé de validateur, ou avec une clé
//! absente de l'ensemble actif, il suit la chaîne en observateur.

use crate::chain::Chain;
use crate::mempool::Mempool;
use crate::network::{NetEvent, NetMessage, SyncRequest, SyncResponse};
use crate::snapshot::{self, SNAPSHOT_INTERVAL, Snapshot, SnapshotInfo};
use crate::store::Store;
use anyhow::{Context, Result, bail};
use libp2p::PeerId;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};
use vinx_consensus::guard::Step;
use vinx_consensus::{Action, BlockValidator, Engine, Evidence, SignGuard, SignedVote, Timeout};
use vinx_core::Account;
use vinx_core::{Block, CommitCertificate, SignedAdminTransaction, SignedTransaction};
use vinx_crypto::KeyPair;
use vinx_crypto::{Address, PublicKey};
use vinx_state::Genesis;

pub struct Shared {
    pub chain: Chain,
    pub mempool: Mempool,
    pub store: Store,
    pub guard: SignGuard,
    /// Diffusion sur le réseau, si le nœud est connecté.
    pub outbound: Option<mpsc::UnboundedSender<NetMessage>>,
    /// Requêtes de rattrapage adressées à un pair précis.
    pub requests: Option<mpsc::UnboundedSender<(PeerId, SyncRequest)>>,
    /// Instantané d'état servi aux pairs.
    pub snapshot: Option<Snapshot>,
    /// Conservation des blocs : `None` en mode archive (tout garder).
    pub retention_ms: Option<u64>,
    /// Validateurs de la genèse, qui authentifient les instantanés (SPEC §8.3).
    pub genesis_validators: Vec<PublicKey>,
    pub metrics: Metrics,
    /// Noms lisibles des validateurs (`--validator-names`), pour l'affichage.
    pub names: BTreeMap<PublicKey, String>,
    /// Nom public de ce nœud (`--node-name`), par exemple « Copper ».
    pub node_name: Option<String>,
    /// Dernières transactions refusées par l'API de ce nœud, la plus récente en tête.
    pub rejects: VecDeque<Reject>,
    /// Dernières transactions (`/transactions/recent`), calculées pour une hauteur.
    pub recent_txs: Option<(u64, Vec<serde_json::Value>)>,
}

/// Transaction refusée à la soumission.
#[derive(Clone, Debug)]
pub struct Reject {
    pub at_ms: u64,
    pub sender: Option<String>,
    pub reason: String,
}

/// Refus gardés pour le tableau de bord.
pub const MAX_REJECTS: usize = 50;

/// Compteurs d'exploitation, exposés par `/metrics` et le tableau de bord.
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    /// Blocs finalisés depuis le démarrage du nœud.
    pub blocks: u64,
    /// Dont finalisés au premier round : un taux proche de 100 % indique un
    /// réseau sain (SPEC §11, critère pour réduire l'intervalle de bloc).
    pub blocks_first_round: u64,
    /// Blocs de la dernière heure : signataires, rounds, rythme. Le tableau de
    /// bord en tire l'état de chaque validateur et des taux qui reflètent
    /// l'état actuel du réseau, et pas une panne ancienne.
    pub activity: crate::activity::Activity,
    pub last_round: u32,
    pub peers: usize,
    /// Indice de ce nœud dans l'ensemble de validateurs, s'il vote.
    pub validator_index: Option<u16>,
    /// Clé de validateur chargée par ce nœud, qu'elle soit déjà dans
    /// l'ensemble ou pas encore.
    pub validator_key: Option<PublicKey>,
    pub snapshots_installed: u64,
    pub started_ms: u64,
}

/// Conservation des blocs par défaut : 24 heures.
pub const DEFAULT_RETENTION_MS: u64 = 24 * 60 * 60 * 1000;

impl Metrics {
    /// Part des blocs de la dernière heure finalisés au premier round.
    pub fn recent_first_round_ratio(&self) -> Option<f64> {
        self.activity.first_round_ratio()
    }
}

impl Shared {
    /// Reconstitue l'activité de la dernière heure à partir des blocs conservés,
    /// pour que le tableau de bord soit complet dès le démarrage. L'ensemble de
    /// validateurs pris est l'actuel : approximatif autour d'un changement.
    fn backfill_activity(&mut self) -> Result<()> {
        let Some(last) = &self.chain.last else {
            return Ok(());
        };
        let newest = last.timestamp;
        let set = self.chain.state.validators_at(last.height);
        let mut records = Vec::new();
        let mut h = last.height;
        while h > 0 {
            let (Some(block), Some(cert)) = (self.store.block(h)?, self.store.certificate(h)?)
            else {
                break;
            };
            if newest.saturating_sub(block.header.timestamp) > crate::activity::WINDOW_MS {
                break;
            }
            let mut record = crate::activity::BlockRecord::new(&block, &cert, set.clone(), 0);
            record.live = false;
            records.push(record);
            h = h.saturating_sub(1);
        }
        for r in records.into_iter().rev() {
            self.metrics.activity.record(r);
        }
        Ok(())
    }

    /// Nom lisible d'un validateur, s'il en a un.
    pub fn name_of(&self, key: &PublicKey) -> Option<&str> {
        self.names.get(key).map(String::as_str)
    }

    pub fn record_reject(&mut self, sender: Option<String>, reason: String) {
        self.rejects.push_front(Reject {
            at_ms: now_ms(),
            sender,
            reason,
        });
        self.rejects.truncate(MAX_REJECTS);
    }

    pub fn broadcast(&self, msg: NetMessage) {
        if let Some(out) = &self.outbound {
            let _ = out.send(msg);
        }
    }

    pub fn request(&self, peer: PeerId, request: SyncRequest) {
        if let Some(out) = &self.requests {
            let _ = out.send((peer, request));
        }
    }

    /// Prépare l'instantané servi aux pairs à partir du dernier bloc finalisé.
    fn refresh_snapshot(&mut self) -> Result<()> {
        let height = self.chain.state.height;
        if height == 0 {
            return Ok(());
        }
        let (Some(block), Some(cert)) =
            (self.store.block(height)?, self.store.certificate(height)?)
        else {
            return Ok(());
        };
        self.snapshot = Some(Snapshot {
            block,
            cert,
            state: self.chain.state.clone(),
        });
        Ok(())
    }
}

pub type SharedRef = Arc<Mutex<Shared>>;

/// Verrouille l'état partagé. Un verrou empoisonné (panique d'un autre fil)
/// est récupéré : les données restent cohérentes car chaque écriture disque
/// est atomique.
pub fn lock(shared: &SharedRef) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Ouvre la base de données et reprend la chaîne là où elle s'était arrêtée.
pub fn open(genesis: Genesis, data_dir: &Path) -> Result<SharedRef> {
    let genesis_time_ms = genesis.genesis_time.saturating_mul(1000);
    let genesis_validators = genesis.validators.clone();
    let initial = genesis
        .into_state()
        .map_err(|e| anyhow::anyhow!("genèse invalide : {e}"))?;
    let genesis_root = initial.state_root();
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("création de {}", data_dir.display()))?;
    let store = Store::open(&data_dir.join("vinx.redb"), genesis_root)?;
    let state = store.load_state()?.unwrap_or(initial);
    let last = match state.height {
        0 => None,
        h => Some(
            store
                .block(h)?
                .context("dernier bloc introuvable dans la base")?
                .header,
        ),
    };
    let guard = store.load_guard()?;
    let mempool = Mempool::new(&state);
    let shared = Arc::new(Mutex::new(Shared {
        chain: Chain {
            state,
            last,
            genesis_root,
            genesis_time_ms,
        },
        mempool,
        store,
        guard,
        outbound: None,
        requests: None,
        snapshot: None,
        retention_ms: Some(DEFAULT_RETENTION_MS),
        genesis_validators,
        metrics: Metrics {
            started_ms: now_ms(),
            ..Metrics::default()
        },
        names: BTreeMap::new(),
        node_name: None,
        rejects: VecDeque::new(),
        recent_txs: None,
    }));
    {
        let mut s = lock(&shared);
        let indexed = s.store.backfill_indexes()?;
        if indexed > 0 {
            info!(blocs = indexed, "index des transactions construit");
        }
        s.backfill_activity()?;
    }
    lock(&shared).refresh_snapshot()?;
    Ok(shared)
}

struct Checker<'a>(&'a Chain);

impl BlockValidator for Checker<'_> {
    fn is_valid(&mut self, block: &Block) -> bool {
        self.0.check(block, now_ms()).is_ok()
    }
}

/// Messages de consensus reçus pour une hauteur future, au plus.
const MAX_BUFFERED: usize = 10_000;

/// Intervalle de retransmission de nos propres messages de la hauteur en cours.
///
/// Tendermint suppose que tout message finit par arriver. Un message diffusé
/// avant que les pairs soient connectés est perdu par gossipsub : sans
/// retransmission, des validateurs démarrés à des moments différents
/// peuvent attendre indéfiniment des votes qui ne viendront plus.
const REBROADCAST: Duration = Duration::from_secs(1);
/// Nos derniers messages retransmis, au plus.
const MAX_OWN: usize = 8;

/// Note pour le tableau de bord le vote d'un autre validateur, après avoir
/// vérifié sa signature. Les votes arrivés après le quorum, que le consensus
/// ignore, comptent aussi : ils montrent que leur auteur est en ligne.
fn note_vote(shared: &SharedRef, v: &SignedVote) {
    let mut s = lock(shared);
    let height = v.vote.height;
    let next = s.chain.next_height();
    // Hauteurs utiles : le dernier bloc finalisé et les suivantes proches.
    if height.saturating_add(1) < next || height > next.saturating_add(2) {
        return;
    }
    let validators = s.chain.state.validators_at(height);
    let Some(key) = validators.get(usize::from(v.validator)) else {
        return;
    };
    if s.metrics.activity.knows_vote(height, key) || !v.verify(key) {
        return;
    }
    s.metrics.activity.note_vote(height, key, now_ms());
}

/// Enregistre un bloc finalisé : disque, chaîne, mempool, garde.
fn commit(
    s: &mut Shared,
    block: &Block,
    cert: &CommitCertificate,
    mut next: vinx_state::State,
) -> Result<()> {
    s.store.commit_block(block, cert, &mut next)?;
    s.metrics.blocks = s.metrics.blocks.saturating_add(1);
    if cert.round == 0 {
        s.metrics.blocks_first_round = s.metrics.blocks_first_round.saturating_add(1);
    }
    s.metrics.last_round = cert.round;
    // Ensemble de validateurs de ce bloc : l'état n'a pas encore avancé.
    let set = s.chain.state.validators_at(block.header.height);
    let peers = s.metrics.peers;
    s.metrics
        .activity
        .record(crate::activity::BlockRecord::new(block, cert, set, peers));
    let included: BTreeSet<_> = block
        .admin_txs
        .iter()
        .map(SignedAdminTransaction::id)
        .chain(block.txs.iter().map(SignedTransaction::id))
        .collect();
    s.chain.advance(block, next);
    s.mempool.reset(&s.chain.state, &included);
    s.guard.prune_below(block.header.height.saturating_add(1));
    s.store.save_guard(&s.guard)?;
    if let Some(retention) = s.retention_ms {
        s.store
            .prune(block.header.timestamp.saturating_sub(retention))?;
    }
    // Un instantané doit rester suivi de blocs conservés : sinon, un nœud qui
    // l'installe ne pourrait pas rattraper les blocs suivants.
    let lowest = s.store.lowest_height()?;
    let unusable = s
        .snapshot
        .as_ref()
        .is_none_or(|snap| lowest.is_some_and(|l| snap.block.header.height.saturating_add(1) < l));
    if unusable || block.header.height.checked_rem(SNAPSHOT_INTERVAL) == Some(0) {
        s.refresh_snapshot()?;
    }
    info!(
        height = block.header.height,
        txs = included.len(),
        "bloc finalisé"
    );
    Ok(())
}

/// Vérifie un bloc reçu par rattrapage avec son certificat, puis l'applique.
fn apply_synced(s: &mut Shared, block: &Block, cert: &CommitCertificate) -> Result<()> {
    let height = s.chain.next_height();
    let validators = s.chain.state.validators_at(height);
    if block.header.height != height || cert.height != height || cert.block_hash != block.hash() {
        bail!("bloc de rattrapage inattendu");
    }
    cert.verify(&s.chain.state.chain_id, &validators)
        .map_err(|e| anyhow::anyhow!("certificat refusé : {e}"))?;
    let next = s
        .chain
        .check(block, now_ms())
        .map_err(|e| anyhow::anyhow!("bloc refusé : {e:?}"))?;
    commit(s, block, cert, next)
}

struct Runner {
    shared: SharedRef,
    key: Option<KeyPair>,
    engine: Engine,
    me: Option<u16>,
    queue: VecDeque<Action>,
    future: Vec<NetMessage>,
    /// Nos messages signés pour la hauteur en cours, à retransmettre.
    own: VecDeque<NetMessage>,
    /// Round et instant (ms) auxquels construire notre proposition.
    build: Option<(u32, u64)>,
    timeouts: mpsc::UnboundedSender<Timeout>,
    download: Option<Download>,
    /// Instant (ms) auquel démarrer le moteur de la hauteur courante.
    start_at: Option<u64>,
}

/// Téléchargement d'un instantané en cours.
struct Download {
    peer: PeerId,
    info: SnapshotInfo,
    accounts: Vec<(Address, Account)>,
    last_progress: Instant,
}

/// Délai sans réponse après lequel un téléchargement d'instantané est abandonné.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Enregistre une preuve d'équivoque, alerte et, si elle vient de nous, la diffuse.
fn record_evidence(s: &Shared, evidence: &Evidence, broadcast: bool) {
    match s.store.record_evidence(evidence) {
        Ok(true) => {
            let validators = s.chain.state.validators_at(evidence.height());
            let index = evidence.validator(validators.len());
            let key = index
                .and_then(|i| validators.get(usize::from(i)))
                .map(vinx_keystore::encoding::key_to_hex)
                .unwrap_or_default();
            error!(
                height = evidence.height(),
                round = evidence.round(),
                step = evidence.step(),
                validator = ?index,
                key = %key,
                "ÉQUIVOQUE : un validateur a signé deux messages contradictoires \
                 (clé compromise ou nœud malveillant) ; envisager son retrait"
            );
            if broadcast {
                s.broadcast(NetMessage::Evidence(Box::new(evidence.clone())));
            }
        }
        Ok(false) => {}
        Err(e) => warn!("preuve d'équivoque non enregistrée : {e}"),
    }
}

fn remember(own: &mut VecDeque<NetMessage>, msg: NetMessage) {
    if own.len() >= MAX_OWN {
        own.pop_front();
    }
    own.push_back(msg);
}

fn message_height(msg: &NetMessage) -> Option<u64> {
    match msg {
        NetMessage::Proposal(p) => Some(p.proposal.height),
        NetMessage::Vote(v) => Some(v.vote.height),
        _ => None,
    }
}

impl Runner {
    /// Démarre le moteur pour la hauteur suivante et rejoue les messages en attente.
    fn new_height(&mut self) {
        let mut s = lock(&self.shared);
        let height = s.chain.next_height();
        let validators = s.chain.state.validators_at(height);
        self.me = self
            .key
            .as_ref()
            .and_then(|k| validators.iter().position(|v| *v == k.public_key()))
            .and_then(|i| u16::try_from(i).ok());
        s.metrics.validator_index = self.me;
        self.engine = Engine::new(
            s.chain.state.chain_id.clone(),
            height,
            validators,
            self.me,
            s.chain.state.params.clone(),
        );
        self.build = None;
        self.own.clear();
        self.queue.clear();
        // Le consensus de cette hauteur ne démarre qu'une fois l'intervalle
        // minimal écoulé depuis le bloc précédent : sinon le délai de
        // proposition expirerait chez les autres validateurs pendant que le
        // proposeur attend encore de pouvoir construire son bloc, et chaque
        // bloc coûterait un round.
        self.start_at = Some(
            s.chain
                .parent_time_ms()
                .saturating_add(s.chain.state.params.min_block_interval_ms),
        );
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.future)
            .into_iter()
            .filter(|m| message_height(m).is_some_and(|h| h >= height))
            .partition(|m| message_height(m) == Some(height));
        self.future = later;
        let s = &mut *s;
        for msg in now {
            let actions = match msg {
                NetMessage::Proposal(p) => self.engine.on_proposal(p, &mut Checker(&s.chain)),
                NetMessage::Vote(v) => self.engine.on_vote(v, &mut Checker(&s.chain)),
                _ => Vec::new(),
            };
            self.queue.extend(actions);
        }
    }

    /// Exécute les actions en attente.
    fn drain(&mut self) -> Result<()> {
        while let Some(action) = self.queue.pop_front() {
            match action {
                Action::BuildBlock { round, .. } => {
                    let s = lock(&self.shared);
                    let at = s
                        .chain
                        .parent_time_ms()
                        .saturating_add(s.chain.state.params.min_block_interval_ms);
                    self.build = Some((round, at));
                }
                Action::Propose(proposal) => {
                    let (Some(key), Some(_)) = (&self.key, self.me) else {
                        continue;
                    };
                    let mut guard = lock(&self.shared);
                    let s = &mut *guard;
                    let content = Some(proposal.block.hash());
                    if !s
                        .guard
                        .allow(proposal.height, proposal.round, Step::Proposal, content)
                    {
                        continue;
                    }
                    s.store.save_guard(&s.guard)?;
                    let signed = proposal.sign(key);
                    let msg = NetMessage::Proposal(signed.clone());
                    s.broadcast(msg.clone());
                    remember(&mut self.own, msg);
                    self.queue
                        .extend(self.engine.on_proposal(signed, &mut Checker(&s.chain)));
                }
                Action::Vote(vote) => {
                    let (Some(key), Some(me)) = (&self.key, self.me) else {
                        continue;
                    };
                    let mut guard = lock(&self.shared);
                    let s = &mut *guard;
                    if !s
                        .guard
                        .allow(vote.height, vote.round, vote.kind.into(), vote.block_hash)
                    {
                        continue;
                    }
                    s.store.save_guard(&s.guard)?;
                    let height = vote.height;
                    let signed = SignedVote::new(vote, me, key);
                    s.metrics
                        .activity
                        .note_vote(height, &key.public_key(), now_ms());
                    let msg = NetMessage::Vote(signed.clone());
                    s.broadcast(msg.clone());
                    remember(&mut self.own, msg);
                    self.queue
                        .extend(self.engine.on_vote(signed, &mut Checker(&s.chain)));
                }
                Action::Equivocation(evidence) => {
                    let s = lock(&self.shared);
                    record_evidence(&s, &evidence, true);
                }
                Action::ScheduleTimeout { timeout, after_ms } => {
                    let tx = self.timeouts.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(after_ms)).await;
                        let _ = tx.send(timeout);
                    });
                }
                Action::Decide { block, certificate } => {
                    {
                        let mut guard = lock(&self.shared);
                        let s = &mut *guard;
                        let next = s
                            .chain
                            .check(&block, u64::MAX)
                            .map_err(|e| anyhow::anyhow!("bloc décidé invalide : {e:?}"))?;
                        commit(s, &block, &certificate, next)?;
                    }
                    self.new_height();
                }
            }
        }
        Ok(())
    }

    fn build_now(&mut self, round: u32) {
        let block = {
            let s = lock(&self.shared);
            let timestamp = now_ms().max(s.chain.parent_time_ms().saturating_add(1));
            let max_bytes =
                usize::try_from(s.chain.state.params.max_block_bytes).unwrap_or(usize::MAX);
            s.chain.build(
                timestamp,
                self.me.unwrap_or(0),
                s.mempool.admin_txs(),
                &s.mempool.by_priority(),
                max_bytes,
            )
        };
        self.queue.extend(self.engine.on_block_built(round, block));
    }

    fn on_net(&mut self, event: NetEvent) {
        match event {
            NetEvent::Message(msg) => match *msg {
                NetMessage::Tx(tx) => {
                    let _ = lock(&self.shared).mempool.add(tx);
                }
                NetMessage::Txs(txs) => {
                    // Signatures vérifiées sur tous les cœurs, hors du verrou.
                    let state = lock(&self.shared).chain.state.clone();
                    vinx_state::sigcache::preverify(&state, &txs);
                    let mut s = lock(&self.shared);
                    for tx in txs {
                        let _ = s.mempool.add(tx);
                    }
                }
                NetMessage::AdminTx(tx) => {
                    let _ = lock(&self.shared).mempool.add_admin(tx);
                }
                NetMessage::Evidence(evidence) => {
                    let s = lock(&self.shared);
                    let validators = s.chain.state.validators_at(evidence.height());
                    if evidence
                        .verify(&s.chain.state.chain_id, &validators)
                        .is_ok()
                    {
                        // Déjà relayée par gossip : pas de nouvelle diffusion.
                        record_evidence(&s, &evidence, false);
                    }
                }
                consensus => {
                    if let NetMessage::Vote(v) = &consensus {
                        note_vote(&self.shared, v);
                    }
                    let height = message_height(&consensus).unwrap_or(0);
                    if height > self.engine.height() {
                        if self.future.len() < MAX_BUFFERED {
                            self.future.push(consensus);
                        }
                        return;
                    }
                    if height < self.engine.height() {
                        return;
                    }
                    let s = lock(&self.shared);
                    let actions = match consensus {
                        NetMessage::Proposal(p) => {
                            self.engine.on_proposal(p, &mut Checker(&s.chain))
                        }
                        NetMessage::Vote(v) => self.engine.on_vote(v, &mut Checker(&s.chain)),
                        _ => Vec::new(),
                    };
                    self.queue.extend(actions);
                }
            },
            NetEvent::Sync(peer, response) => self.on_sync(peer, *response),
        }
    }

    fn on_sync(&mut self, peer: PeerId, response: SyncResponse) {
        match response {
            SyncResponse::Blocks { lowest, blocks } => self.on_blocks(peer, lowest, blocks),
            SyncResponse::SnapshotInfo(Some(info)) => self.on_snapshot_info(peer, *info),
            SyncResponse::SnapshotInfo(None) => {}
            SyncResponse::SnapshotChunk {
                height,
                chunk,
                done,
            } => {
                self.on_snapshot_chunk(peer, height, chunk, done);
            }
        }
    }

    fn on_blocks(
        &mut self,
        peer: PeerId,
        lowest: u64,
        mut blocks: Vec<(Block, CommitCertificate)>,
    ) {
        let next = lock(&self.shared).chain.next_height();
        // Les blocs qui nous manquent ont été élagués chez ce pair : on passe
        // par un instantané d'état.
        if blocks.is_empty() && lowest > next {
            // Un nœud archive doit garder tout l'historique : il attend un pair
            // qui possède encore les blocs, au lieu d'installer un instantané.
            if lock(&self.shared).retention_ms.is_none() {
                debug!(%peer, next, lowest, "blocs élagués chez ce pair : un nœud archive attend un autre pair");
                return;
            }
            let stalled = self
                .download
                .as_ref()
                .is_none_or(|d| d.last_progress.elapsed() > DOWNLOAD_TIMEOUT);
            if stalled {
                info!(%peer, next, lowest, "blocs élagués chez le pair : synchronisation par l'état");
                self.download = None;
                lock(&self.shared).request(peer, SyncRequest::SnapshotInfo);
            }
            return;
        }
        blocks.sort_by_key(|(b, _)| b.header.height);
        let mut advanced = false;
        {
            let mut guard = lock(&self.shared);
            for (block, cert) in &blocks {
                if block.header.height < guard.chain.next_height() {
                    continue;
                }
                match apply_synced(&mut guard, block, cert) {
                    Ok(()) => advanced = true,
                    Err(e) => {
                        warn!("rattrapage interrompu : {e}");
                        break;
                    }
                }
            }
        }
        if advanced {
            self.new_height();
        }
    }

    fn on_snapshot_info(&mut self, peer: PeerId, info: SnapshotInfo) {
        if self.download.is_some() {
            return;
        }
        let s = lock(&self.shared);
        if info.block.header.height < s.chain.next_height() {
            return;
        }
        if let Err(e) = snapshot::verify_info(&info, &s.chain.state.chain_id, &s.genesis_validators)
        {
            warn!(%peer, "instantané refusé : {e}");
            return;
        }
        let height = info.block.header.height;
        s.request(
            peer,
            SyncRequest::SnapshotChunk {
                height,
                after: None,
            },
        );
        drop(s);
        info!(%peer, height, accounts = info.accounts, "téléchargement de l'instantané");
        self.download = Some(Download {
            peer,
            info,
            accounts: Vec::new(),
            last_progress: Instant::now(),
        });
    }

    fn on_snapshot_chunk(
        &mut self,
        peer: PeerId,
        height: u64,
        chunk: Option<Vec<(Address, Account)>>,
        done: bool,
    ) {
        let Some(d) = self.download.as_mut() else {
            return;
        };
        if d.peer != peer || d.info.block.header.height != height {
            return;
        }
        let Some(chunk) = chunk else {
            warn!(%peer, "le pair ne sert plus cet instantané");
            self.download = None;
            return;
        };
        d.accounts.extend(chunk);
        d.last_progress = Instant::now();
        let over_limit = u64::try_from(d.accounts.len()).unwrap_or(u64::MAX) > d.info.accounts;
        if over_limit {
            warn!(%peer, "instantané plus grand qu'annoncé : abandon");
            self.download = None;
            return;
        }
        if !done {
            let after = d.accounts.last().map(|(a, _)| *a);
            lock(&self.shared).request(peer, SyncRequest::SnapshotChunk { height, after });
            return;
        }
        let Some(d) = self.download.take() else {
            return;
        };
        match self.install(d) {
            Ok(()) => self.new_height(),
            Err(e) => warn!("instantané refusé : {e}"),
        }
    }

    /// Vérifie l'état téléchargé et l'adopte.
    fn install(&mut self, d: Download) -> Result<()> {
        let mut state = snapshot::assemble(&d.info, d.accounts)?;
        let mut guard = lock(&self.shared);
        let s = &mut *guard;
        if d.info.block.header.height < s.chain.next_height() {
            return Ok(());
        }
        s.store
            .install_snapshot(&d.info.block, &d.info.cert, &mut state)?;
        s.chain.advance(&d.info.block, state);
        s.mempool.reset(&s.chain.state, &BTreeSet::new());
        s.metrics.snapshots_installed = s.metrics.snapshots_installed.saturating_add(1);
        s.refresh_snapshot()?;
        info!(
            height = d.info.block.header.height,
            "état installé depuis un instantané"
        );
        Ok(())
    }
}

/// Fait tourner le consensus jusqu'à ce que `shutdown` passe à `true`.
///
/// `key` est la clé de validateur, absente pour un observateur. `inbound`
/// apporte les messages du réseau.
pub async fn run(
    shared: SharedRef,
    key: Option<KeyPair>,
    mut inbound: mpsc::UnboundedReceiver<NetEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let (timeouts_tx, mut timeouts_rx) = mpsc::unbounded_channel::<Timeout>();
    let placeholder = {
        let s = lock(&shared);
        Engine::new(
            s.chain.state.chain_id.clone(),
            0,
            Vec::new(),
            None,
            s.chain.state.params.clone(),
        )
    };
    lock(&shared).metrics.validator_key = key.as_ref().map(KeyPair::public_key);
    let mut r = Runner {
        shared,
        key,
        engine: placeholder,
        me: None,
        queue: VecDeque::new(),
        future: Vec::new(),
        own: VecDeque::new(),
        build: None,
        download: None,
        start_at: None,
        timeouts: timeouts_tx,
    };
    r.new_height();
    if r.me.is_none() {
        info!("mode observateur : ce nœud ne vote pas");
    }
    let mut rebroadcast = tokio::time::interval(REBROADCAST);
    loop {
        if r.start_at.is_some_and(|at| at <= now_ms()) {
            r.start_at = None;
            let s = lock(&r.shared);
            let actions = r.engine.start(&mut Checker(&s.chain));
            drop(s);
            r.queue.extend(actions);
        }
        r.drain()?;
        let start_in = r
            .start_at
            .map(|at| Duration::from_millis(at.saturating_sub(now_ms())));
        let build_in = r
            .build
            .map(|(_, at)| Duration::from_millis(at.saturating_sub(now_ms())));
        tokio::select! {
            _ = tokio::time::sleep(start_in.unwrap_or(Duration::MAX)), if start_in.is_some() => {}
            _ = tokio::time::sleep(build_in.unwrap_or(Duration::MAX)), if build_in.is_some() => {
                if let Some((round, _)) = r.build.take() {
                    r.build_now(round);
                }
            }
            Some(timeout) = timeouts_rx.recv() => {
                let s = lock(&r.shared);
                let actions = r.engine.on_timeout(timeout, &mut Checker(&s.chain));
                drop(s);
                r.queue.extend(actions);
            }
            Some(event) = inbound.recv() => r.on_net(event),
            _ = rebroadcast.tick() => {
                let s = lock(&r.shared);
                for msg in &r.own {
                    s.broadcast(msg.clone());
                }
            }
            _ = shutdown.changed() => return Ok(()),
        }
    }
}
