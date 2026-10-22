//! Boucle principale : exécute les actions du moteur de consensus.
//!
//! Dans cette version, le nœud fonctionne seul : il faut être l'unique
//! validateur. Le réseau P2P (relais et rattrapage) arrive dans une étape suivante.

use crate::chain::Chain;
use crate::mempool::Mempool;
use crate::store::Store;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};
use tracing::info;
use vinx_consensus::guard::Step;
use vinx_consensus::{Action, BlockValidator, Engine, SignGuard, SignedVote, Timeout};
use vinx_core::{Block, SignedAdminTransaction, SignedTransaction};
use vinx_crypto::KeyPair;
use vinx_state::Genesis;

pub struct Shared {
    pub chain: Chain,
    pub mempool: Mempool,
    pub store: Store,
    pub guard: SignGuard,
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
    })))
}

struct Checker<'a>(&'a Chain);

impl BlockValidator for Checker<'_> {
    fn is_valid(&mut self, block: &Block) -> bool {
        self.0.check(block, now_ms()).is_ok()
    }
}

/// Fait tourner le consensus jusqu'à ce que `shutdown` passe à `true`.
pub async fn run(
    shared: SharedRef,
    keypair: KeyPair,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let (timeouts_tx, mut timeouts_rx) = mpsc::unbounded_channel::<Timeout>();
    let me_key = keypair.public_key();
    loop {
        // ── Nouvelle hauteur ────────────────────────────────────────────────
        let (mut engine, me, first) = {
            let s = lock(&shared);
            let height = s.chain.next_height();
            let validators = s.chain.state.validators_at(height);
            let Some(me) = validators.iter().position(|k| *k == me_key) else {
                bail!("cette clé n'est pas validatrice à la hauteur {height}");
            };
            if validators.len() > 1 {
                bail!(
                    "{} validateurs dans la genèse : le fonctionnement à plusieurs \
                     nœuds nécessite le réseau P2P, pas encore disponible",
                    validators.len()
                );
            }
            let me = u16::try_from(me).context("indice de validateur trop grand")?;
            let mut engine = Engine::new(
                s.chain.state.chain_id.clone(),
                height,
                validators,
                Some(me),
                s.chain.state.params.clone(),
            );
            let first = engine.start(&mut Checker(&s.chain));
            (engine, me, first)
        };
        let mut queue: VecDeque<Action> = first.into();

        // ── Rounds jusqu'à la décision ──────────────────────────────────────
        'height: loop {
            while let Some(action) = queue.pop_front() {
                match action {
                    Action::BuildBlock { round, .. } => {
                        let wait = {
                            let s = lock(&shared);
                            s.chain
                                .parent_time_ms()
                                .saturating_add(s.chain.state.params.min_block_interval_ms)
                                .saturating_sub(now_ms())
                        };
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
                            _ = shutdown.changed() => return Ok(()),
                        }
                        let block = {
                            let s = lock(&shared);
                            let timestamp =
                                now_ms().max(s.chain.parent_time_ms().saturating_add(1));
                            let max_bytes = usize::try_from(s.chain.state.params.max_block_bytes)
                                .unwrap_or(usize::MAX);
                            s.chain.build(
                                timestamp,
                                me,
                                s.mempool.admin_txs(),
                                s.mempool.txs(),
                                max_bytes,
                            )
                        };
                        queue.extend(engine.on_block_built(round, block));
                    }
                    Action::Propose(proposal) => {
                        let mut guard = lock(&shared);
                        let s = &mut *guard;
                        let content = Some(proposal.block.hash());
                        if !s
                            .guard
                            .allow(proposal.height, proposal.round, Step::Proposal, content)
                        {
                            continue;
                        }
                        s.store.save_guard(&s.guard)?;
                        let signed = proposal.sign(&keypair);
                        queue.extend(engine.on_proposal(signed, &mut Checker(&s.chain)));
                    }
                    Action::Vote(vote) => {
                        let mut guard = lock(&shared);
                        let s = &mut *guard;
                        if !s.guard.allow(
                            vote.height,
                            vote.round,
                            vote.kind.into(),
                            vote.block_hash,
                        ) {
                            continue;
                        }
                        s.store.save_guard(&s.guard)?;
                        let signed = SignedVote::new(vote, me, &keypair);
                        queue.extend(engine.on_vote(signed, &mut Checker(&s.chain)));
                    }
                    Action::ScheduleTimeout { timeout, after_ms } => {
                        let tx = timeouts_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(after_ms)).await;
                            let _ = tx.send(timeout);
                        });
                    }
                    Action::Decide { block, certificate } => {
                        let mut guard = lock(&shared);
                        let s = &mut *guard;
                        let next = s
                            .chain
                            .check(&block, u64::MAX)
                            .map_err(|e| anyhow::anyhow!("bloc décidé invalide : {e:?}"))?;
                        s.store.commit_block(&block, &certificate, &next)?;
                        let included: BTreeSet<_> = block
                            .admin_txs
                            .iter()
                            .map(SignedAdminTransaction::id)
                            .chain(block.txs.iter().map(SignedTransaction::id))
                            .collect();
                        s.chain.advance(&block, next);
                        s.mempool.reset(&s.chain.state, &included);
                        s.guard.prune_below(block.header.height.saturating_add(1));
                        s.store.save_guard(&s.guard)?;
                        info!(
                            height = block.header.height,
                            txs = included.len(),
                            "bloc finalisé"
                        );
                        break 'height;
                    }
                }
            }
            tokio::select! {
                Some(timeout) = timeouts_rx.recv() => {
                    let s = lock(&shared);
                    queue.extend(engine.on_timeout(timeout, &mut Checker(&s.chain)));
                }
                _ = shutdown.changed() => return Ok(()),
            }
        }
    }
}
