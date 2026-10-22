//! Boucle principale : exécute les actions du moteur de consensus.
//!
//! Le nœud réagit aux actions du moteur, aux messages du réseau, aux délais et
//! aux blocs reçus par rattrapage. Sans clé de validateur, ou avec une clé
//! absente de l'ensemble actif, il suit la chaîne en observateur.

use crate::chain::Chain;
use crate::mempool::Mempool;
use crate::network::{NetEvent, NetMessage};
use crate::store::Store;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use vinx_consensus::guard::Step;
use vinx_consensus::{Action, BlockValidator, Engine, SignGuard, SignedVote, Timeout};
use vinx_core::{Block, CommitCertificate, SignedAdminTransaction, SignedTransaction};
use vinx_crypto::KeyPair;
use vinx_state::Genesis;

pub struct Shared {
    pub chain: Chain,
    pub mempool: Mempool,
    pub store: Store,
    pub guard: SignGuard,
    /// Diffusion sur le réseau, si le nœud est connecté.
    pub outbound: Option<mpsc::UnboundedSender<NetMessage>>,
}

impl Shared {
    pub fn broadcast(&self, msg: NetMessage) {
        if let Some(out) = &self.outbound {
            let _ = out.send(msg);
        }
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
    Ok(Arc::new(Mutex::new(Shared {
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
    })))
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

/// Enregistre un bloc finalisé : disque, chaîne, mempool, garde.
fn commit(
    s: &mut Shared,
    block: &Block,
    cert: &CommitCertificate,
    mut next: vinx_state::State,
) -> Result<()> {
    s.store.commit_block(block, cert, &mut next)?;
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
        self.queue.extend(self.engine.start(&mut Checker(&s.chain)));
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
                    let signed = SignedVote::new(vote, me, key);
                    let msg = NetMessage::Vote(signed.clone());
                    s.broadcast(msg.clone());
                    remember(&mut self.own, msg);
                    self.queue
                        .extend(self.engine.on_vote(signed, &mut Checker(&s.chain)));
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
                NetMessage::AdminTx(tx) => {
                    let _ = lock(&self.shared).mempool.add_admin(tx);
                }
                consensus => {
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
            NetEvent::Blocks(mut blocks) => {
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
        }
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
    let mut r = Runner {
        shared,
        key,
        engine: placeholder,
        me: None,
        queue: VecDeque::new(),
        future: Vec::new(),
        own: VecDeque::new(),
        build: None,
        timeouts: timeouts_tx,
    };
    r.new_height();
    if r.me.is_none() {
        info!("mode observateur : ce nœud ne vote pas");
    }
    let mut rebroadcast = tokio::time::interval(REBROADCAST);
    loop {
        r.drain()?;
        let build_in = r
            .build
            .map(|(_, at)| Duration::from_millis(at.saturating_sub(now_ms())));
        tokio::select! {
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
