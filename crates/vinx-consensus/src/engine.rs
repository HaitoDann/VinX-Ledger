//! Machine à états Tendermint pour une hauteur (SPEC §7.3).
//!
//! Transcription de l'algorithme de Buchman, Kwon et Milosevic, « The latest
//! gossip on BFT consensus » (2018). Les commentaires `L22`, `L28`… renvoient aux
//! lignes de l'algorithme 1 de l'article.
//!
//! Un moteur couvre une seule hauteur : une fois le bloc décidé, le nœud crée un
//! nouveau moteur pour la hauteur suivante.

use crate::header::proposer_index;
use crate::messages::{Proposal, SignedProposal, SignedVote};
use std::collections::{BTreeMap, BTreeSet};
use vinx_core::{Block, CommitCertificate, Params, Vote, VoteKind, quorum};
use vinx_crypto::{Hash, PublicKey};

/// Vérification du contenu d'un bloc, fournie par le nœud : chaînage de
/// l'en-tête, horodatage, exécution de l'état.
pub trait BlockValidator {
    fn is_valid(&mut self, block: &Block) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimeoutKind {
    Propose,
    Prevote,
    Precommit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timeout {
    pub kind: TimeoutKind,
    pub height: u64,
    pub round: u32,
}

/// Ce que le nœud doit faire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Construire un bloc pour ce round, puis appeler [`Engine::on_block_built`].
    BuildBlock { height: u64, round: u32 },
    /// Signer (via le garde), diffuser, puis renvoyer à [`Engine::on_proposal`].
    Propose(Proposal),
    /// Signer (via le garde), diffuser, puis renvoyer à [`Engine::on_vote`].
    Vote(Vote),
    /// Appeler [`Engine::on_timeout`] après ce délai.
    ScheduleTimeout { timeout: Timeout, after_ms: u64 },
    /// Le bloc est finalisé.
    Decide {
        block: Block,
        certificate: CommitCertificate,
    },
}

/// Votes d'une étape : validateur → cible (`None` = nil) → vote signé.
type VotesByValidator = BTreeMap<u16, BTreeMap<Option<Hash>, SignedVote>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Propose,
    Prevote,
    Precommit,
}

pub struct Engine {
    chain_id: String,
    height: u64,
    validators: Vec<PublicKey>,
    /// Notre indice, si nous sommes validateur.
    me: Option<u16>,
    params: Params,

    round: u32,
    step: Step,
    locked: Option<(u32, Hash)>,
    valid: Option<(u32, Hash)>,
    decided: bool,

    blocks: BTreeMap<Hash, Block>,
    validity: BTreeMap<Hash, bool>,
    /// Première proposition reçue du proposeur légitime de chaque round.
    proposals: BTreeMap<u32, (Hash, Option<u32>)>,
    /// Votes par `(round, type)`, puis par validateur et par cible.
    ///
    /// Un validateur honnête ne vote qu'une fois par étape. Un validateur
    /// malveillant peut voter pour plusieurs cibles (équivoque) : on garde ces
    /// votes et on le compte pour chacune. C'est sans danger pour la sûreté
    /// (deux quorums se recoupent toujours sur au moins un validateur honnête),
    /// et indispensable à la vivacité : sinon deux nœuds honnêtes peuvent voir
    /// des quorums différents et rester verrouillés indéfiniment.
    votes: BTreeMap<(u32, u8), VotesByValidator>,
    prevote_timeout_set: BTreeSet<u32>,
    precommit_timeout_set: BTreeSet<u32>,
    /// Rounds où la règle L36 a déjà été appliquée.
    l36_done: BTreeSet<u32>,
    /// Rounds où nous avons déjà proposé.
    proposed: BTreeSet<u32>,
}

impl Engine {
    pub fn new(
        chain_id: impl Into<String>,
        height: u64,
        validators: Vec<PublicKey>,
        me: Option<u16>,
        params: Params,
    ) -> Self {
        Self {
            chain_id: chain_id.into(),
            height,
            validators,
            me,
            params,
            round: 0,
            step: Step::Propose,
            locked: None,
            valid: None,
            decided: false,
            blocks: BTreeMap::new(),
            validity: BTreeMap::new(),
            proposals: BTreeMap::new(),
            votes: BTreeMap::new(),
            prevote_timeout_set: BTreeSet::new(),
            precommit_timeout_set: BTreeSet::new(),
            l36_done: BTreeSet::new(),
            proposed: BTreeSet::new(),
        }
    }

    pub fn height(&self) -> u64 {
        self.height
    }

    pub fn round(&self) -> u32 {
        self.round
    }

    pub fn is_decided(&self) -> bool {
        self.decided
    }

    /// Démarre le round 0.
    pub fn start(&mut self, v: &mut impl BlockValidator) -> Vec<Action> {
        let mut out = Vec::new();
        self.start_round(0, &mut out);
        self.process(v, &mut out);
        out
    }

    /// Bloc construit par le nœud après [`Action::BuildBlock`].
    pub fn on_block_built(&mut self, round: u32, block: Block) -> Vec<Action> {
        let mut out = Vec::new();
        if !self.decided
            && round == self.round
            && self.step == Step::Propose
            && self.is_proposer(round)
            && self.proposed.insert(round)
        {
            out.push(Action::Propose(Proposal {
                chain_id: self.chain_id.clone(),
                height: self.height,
                round,
                pol_round: None,
                block,
            }));
        }
        out
    }

    pub fn on_proposal(&mut self, sp: SignedProposal, v: &mut impl BlockValidator) -> Vec<Action> {
        let mut out = Vec::new();
        if self.accept_proposal(sp) {
            self.process(v, &mut out);
        }
        out
    }

    pub fn on_vote(&mut self, sv: SignedVote, v: &mut impl BlockValidator) -> Vec<Action> {
        let mut out = Vec::new();
        if self.accept_vote(sv) {
            self.process(v, &mut out);
        }
        out
    }

    pub fn on_timeout(&mut self, t: Timeout, v: &mut impl BlockValidator) -> Vec<Action> {
        let mut out = Vec::new();
        if self.decided || t.height != self.height || t.round != self.round {
            return out;
        }
        match t.kind {
            // OnTimeoutPropose
            TimeoutKind::Propose if self.step == Step::Propose => {
                self.cast(VoteKind::Prevote, None, &mut out);
                self.step = Step::Prevote;
            }
            // OnTimeoutPrevote
            TimeoutKind::Prevote if self.step == Step::Prevote => {
                self.cast(VoteKind::Precommit, None, &mut out);
                self.step = Step::Precommit;
            }
            // OnTimeoutPrecommit
            TimeoutKind::Precommit => {
                self.start_round(self.round.saturating_add(1), &mut out);
            }
            _ => {}
        }
        self.process(v, &mut out);
        out
    }

    // ── Réception ───────────────────────────────────────────────────────────

    fn is_proposer(&self, round: u32) -> bool {
        self.me.is_some() && proposer_index(self.height, round, self.validators.len()) == self.me
    }

    fn accept_proposal(&mut self, sp: SignedProposal) -> bool {
        let p = &sp.proposal;
        let Some(proposer) = proposer_index(self.height, p.round, self.validators.len()) else {
            return false;
        };
        let Some(key) = self.validators.get(usize::from(proposer)) else {
            return false;
        };
        let h = &p.block.header;
        let well_formed = p.chain_id == self.chain_id
            && p.height == self.height
            && h.height == self.height
            && h.chain_id == self.chain_id
            && h.proposer == proposer
            && p.pol_round.is_none_or(|vr| vr < p.round);
        if !well_formed || self.proposals.contains_key(&p.round) || !sp.verify(key) {
            // Une seconde proposition pour le même round est une équivoque : ignorée.
            return false;
        }
        let hash = p.block.hash();
        self.proposals.insert(p.round, (hash, p.pol_round));
        self.blocks.entry(hash).or_insert(sp.proposal.block);
        true
    }

    fn accept_vote(&mut self, sv: SignedVote) -> bool {
        let vote = &sv.vote;
        if vote.chain_id != self.chain_id || vote.height != self.height {
            return false;
        }
        let Some(key) = self.validators.get(usize::from(sv.validator)) else {
            return false;
        };
        let target = vote.block_hash;
        let known_proposal = self
            .proposals
            .get(&vote.round)
            .is_some_and(|(h, _)| Some(*h) == target);
        let by_validator = self
            .votes
            .entry((vote.round, vote.kind as u8))
            .or_default()
            .entry(sv.validator)
            .or_default();
        // Mémoire bornée face à un validateur malveillant : au plus deux cibles
        // par étape, plus la cible de la proposition du round.
        let room = by_validator.len() < 2 || known_proposal;
        if by_validator.contains_key(&target) || !room || !sv.verify(key) {
            return false;
        }
        by_validator.insert(target, sv);
        true
    }

    // ── Outils ──────────────────────────────────────────────────────────────

    fn quorum(&self) -> usize {
        quorum(self.validators.len())
    }

    /// Plus d'un tiers des validateurs : `n − quorum + 1`.
    fn one_third(&self) -> usize {
        self.validators
            .len()
            .saturating_sub(self.quorum())
            .saturating_add(1)
    }

    fn votes_of(&self, round: u32, kind: VoteKind) -> Option<&VotesByValidator> {
        self.votes.get(&(round, kind as u8))
    }

    /// Nombre de validateurs ayant voté pour `target`.
    fn count(&self, round: u32, kind: VoteKind, target: Option<Hash>) -> usize {
        self.votes_of(round, kind).map_or(0, |m| {
            m.values().filter(|t| t.contains_key(&target)).count()
        })
    }

    fn count_any(&self, round: u32, kind: VoteKind) -> usize {
        self.votes_of(round, kind).map_or(0, BTreeMap::len)
    }

    fn is_valid(&mut self, hash: Hash, v: &mut impl BlockValidator) -> bool {
        if let Some(known) = self.validity.get(&hash) {
            return *known;
        }
        let ok = self.blocks.get(&hash).is_some_and(|b| v.is_valid(b));
        self.validity.insert(hash, ok);
        ok
    }

    fn cast(&self, kind: VoteKind, block_hash: Option<Hash>, out: &mut Vec<Action>) {
        if self.me.is_some() {
            out.push(Action::Vote(Vote {
                chain_id: self.chain_id.clone(),
                kind,
                height: self.height,
                round: self.round,
                block_hash,
            }));
        }
    }

    fn schedule(&self, kind: TimeoutKind, round: u32, out: &mut Vec<Action>) {
        let (base, delta) = match kind {
            TimeoutKind::Propose => (
                self.params.timeout_propose_ms,
                self.params.timeout_propose_delta_ms,
            ),
            TimeoutKind::Prevote | TimeoutKind::Precommit => (
                self.params.timeout_vote_ms,
                self.params.timeout_vote_delta_ms,
            ),
        };
        out.push(Action::ScheduleTimeout {
            timeout: Timeout {
                kind,
                height: self.height,
                round,
            },
            after_ms: base.saturating_add(delta.saturating_mul(u64::from(round))),
        });
    }

    // ── Algorithme ──────────────────────────────────────────────────────────

    /// StartRound(r).
    fn start_round(&mut self, round: u32, out: &mut Vec<Action>) {
        self.round = round;
        self.step = Step::Propose;
        if self.is_proposer(round) {
            match self
                .valid
                .and_then(|(vr, h)| Some((vr, self.blocks.get(&h)?.clone())))
            {
                Some((vr, block)) => {
                    if self.proposed.insert(round) {
                        out.push(Action::Propose(Proposal {
                            chain_id: self.chain_id.clone(),
                            height: self.height,
                            round,
                            pol_round: Some(vr),
                            block,
                        }));
                    }
                }
                None => out.push(Action::BuildBlock {
                    height: self.height,
                    round,
                }),
            }
        }
        self.schedule(TimeoutKind::Propose, round, out);
    }

    /// Applique les règles tant que l'une d'elles se déclenche.
    fn process(&mut self, v: &mut impl BlockValidator, out: &mut Vec<Action>) {
        while !self.decided {
            let fired = self.try_decide(v, out)
                || self.try_skip_round(out)
                || self.try_prevote_on_proposal(v, out)
                || self.try_lock(v, out)
                || self.try_precommit_nil(out)
                || self.try_schedule_timeouts(out);
            if !fired {
                break;
            }
        }
    }

    /// L49 : proposition + quorum de precommits pour elle, dans n'importe quel round.
    fn try_decide(&mut self, v: &mut impl BlockValidator, out: &mut Vec<Action>) -> bool {
        let q = self.quorum();
        let candidates: Vec<(u32, Hash)> = self
            .proposals
            .iter()
            .map(|(r, (h, _))| (*r, *h))
            .filter(|(r, h)| self.count(*r, VoteKind::Precommit, Some(*h)) >= q)
            .collect();
        for (round, hash) in candidates {
            if !self.is_valid(hash, v) {
                continue;
            }
            let (Some(block), Some(precommits)) = (
                self.blocks.get(&hash).cloned(),
                self.votes_of(round, VoteKind::Precommit),
            ) else {
                continue;
            };
            let signatures = precommits
                .iter()
                .filter_map(|(i, t)| Some((*i, t.get(&Some(hash))?.signature.clone())))
                .collect();
            out.push(Action::Decide {
                block,
                certificate: CommitCertificate {
                    height: self.height,
                    round,
                    block_hash: hash,
                    signatures,
                },
            });
            self.decided = true;
            return true;
        }
        false
    }

    /// L55 : plus d'un tiers des validateurs sont déjà dans un round supérieur.
    fn try_skip_round(&mut self, out: &mut Vec<Action>) -> bool {
        let threshold = self.one_third();
        let mut senders: BTreeMap<u32, BTreeSet<u16>> = BTreeMap::new();
        for ((round, _), m) in self.votes.range((self.round.saturating_add(1), 0)..) {
            senders.entry(*round).or_default().extend(m.keys());
        }
        for round in self.proposals.keys().filter(|r| **r > self.round) {
            if let Some(p) = proposer_index(self.height, *round, self.validators.len()) {
                senders.entry(*round).or_default().insert(p);
            }
        }
        let target = senders
            .iter()
            .rev()
            .find(|(_, s)| s.len() >= threshold)
            .map(|(r, _)| *r);
        match target {
            Some(round) => {
                self.start_round(round, out);
                true
            }
            None => false,
        }
    }

    /// L22 et L28 : prevote sur la proposition du round courant.
    fn try_prevote_on_proposal(
        &mut self,
        v: &mut impl BlockValidator,
        out: &mut Vec<Action>,
    ) -> bool {
        if self.step != Step::Propose {
            return false;
        }
        let Some((hash, pol)) = self.proposals.get(&self.round).copied() else {
            return false;
        };
        let accept = match pol {
            // L22 : nouvelle proposition.
            None => self.locked.is_none_or(|(_, lh)| lh == hash),
            // L28 : re-proposition justifiée par un quorum de prevotes au round `vr`.
            Some(vr) => {
                if self.count(vr, VoteKind::Prevote, Some(hash)) < self.quorum() {
                    return false;
                }
                self.locked.is_none_or(|(lr, lh)| lr <= vr || lh == hash)
            }
        };
        let target = (accept && self.is_valid(hash, v)).then_some(hash);
        self.cast(VoteKind::Prevote, target, out);
        self.step = Step::Prevote;
        true
    }

    /// L36 : quorum de prevotes pour la proposition du round courant.
    fn try_lock(&mut self, v: &mut impl BlockValidator, out: &mut Vec<Action>) -> bool {
        if self.step < Step::Prevote || self.l36_done.contains(&self.round) {
            return false;
        }
        let Some((hash, _)) = self.proposals.get(&self.round).copied() else {
            return false;
        };
        if self.count(self.round, VoteKind::Prevote, Some(hash)) < self.quorum()
            || !self.is_valid(hash, v)
        {
            return false;
        }
        self.l36_done.insert(self.round);
        if self.step == Step::Prevote {
            self.locked = Some((self.round, hash));
            self.cast(VoteKind::Precommit, Some(hash), out);
            self.step = Step::Precommit;
        }
        self.valid = Some((self.round, hash));
        true
    }

    /// L44 : quorum de prevotes nil.
    fn try_precommit_nil(&mut self, out: &mut Vec<Action>) -> bool {
        if self.step != Step::Prevote
            || self.count(self.round, VoteKind::Prevote, None) < self.quorum()
        {
            return false;
        }
        self.cast(VoteKind::Precommit, None, out);
        self.step = Step::Precommit;
        true
    }

    /// L34 et L47 : délais après un quorum de votes quelconques.
    fn try_schedule_timeouts(&mut self, out: &mut Vec<Action>) -> bool {
        let q = self.quorum();
        let r = self.round;
        if self.step == Step::Prevote
            && self.count_any(r, VoteKind::Prevote) >= q
            && self.prevote_timeout_set.insert(r)
        {
            self.schedule(TimeoutKind::Prevote, r, out);
            return true;
        }
        if self.count_any(r, VoteKind::Precommit) >= q && self.precommit_timeout_set.insert(r) {
            self.schedule(TimeoutKind::Precommit, r, out);
            return true;
        }
        false
    }
}
