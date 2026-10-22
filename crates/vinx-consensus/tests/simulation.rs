//! Simulation déterministe d'un réseau de validateurs.
//!
//! Les messages sont livrés dans un ordre pseudo-aléatoire fixé par une graine ;
//! les délais n'expirent que lorsqu'aucun message n'est en transit (réseau
//! synchrone après stabilisation). On vérifie la sûreté (jamais deux blocs
//! différents finalisés à une même hauteur) et la vivacité (la chaîne avance
//! tant que plus de 2/3 des validateurs sont honnêtes et joignables).

// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use vinx_consensus::guard::Step;
use vinx_consensus::{
    Action, BlockValidator, Engine, Proposal, SignGuard, SignedProposal, SignedVote, Timeout,
};
use vinx_core::{Block, BlockHeader, CommitCertificate, Params, Vote};
use vinx_crypto::{Hash, KeyPair, PublicKey};

const CHAIN: &str = "vinx-sim";

/// Générateur pseudo-aléatoire xorshift64, suffisant et reproductible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[derive(Clone)]
enum Msg {
    Proposal(SignedProposal),
    Vote(SignedVote),
}

impl Msg {
    fn height(&self) -> u64 {
        match self {
            Msg::Proposal(p) => p.proposal.height,
            Msg::Vote(v) => v.vote.height,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behavior {
    Honest,
    Crashed,
    /// Envoie des propositions et des votes contradictoires aux deux moitiés du réseau.
    Equivocator,
}

/// Bloc valide s'il prolonge le dernier bloc décidé par le nœud.
struct ParentCheck(Hash);

impl BlockValidator for ParentCheck {
    fn is_valid(&mut self, block: &Block) -> bool {
        block.header.prev_hash == self.0
    }
}

struct Node {
    index: u16,
    keypair: KeyPair,
    behavior: Behavior,
    engine: Engine,
    guard: SignGuard,
    parent: Hash,
    decisions: BTreeMap<u64, Hash>,
    future: Vec<Msg>,
    /// Messages déjà vus, pour ne relayer chacun qu'une fois.
    seen: std::collections::BTreeSet<Vec<u8>>,
}

struct Sim {
    nodes: Vec<Node>,
    validators: Vec<PublicKey>,
    in_flight: Vec<(usize, Msg)>,
    timers: Vec<(u64, usize, Timeout)>,
    now: u64,
    rng: Rng,
    max_round: u32,
    /// Blocs finalisés et leurs certificats, comme les servirait la synchronisation.
    committed: BTreeMap<u64, (Block, CommitCertificate)>,
    synced: usize,
}

impl Sim {
    fn new(behaviors: &[Behavior], seed: u64) -> Self {
        let keys: Vec<KeyPair> = behaviors.iter().map(|_| KeyPair::generate()).collect();
        let validators: Vec<PublicKey> = keys.iter().map(KeyPair::public_key).collect();
        let nodes = keys
            .into_iter()
            .zip(behaviors)
            .enumerate()
            .map(|(i, (keypair, behavior))| Node {
                index: i as u16,
                keypair,
                behavior: *behavior,
                engine: Engine::new(
                    CHAIN,
                    1,
                    validators.clone(),
                    Some(i as u16),
                    Params::default(),
                ),
                guard: SignGuard::default(),
                parent: [0; 32],
                decisions: BTreeMap::new(),
                future: Vec::new(),
                seen: std::collections::BTreeSet::new(),
            })
            .collect();
        let mut sim = Sim {
            nodes,
            validators,
            in_flight: Vec::new(),
            timers: Vec::new(),
            now: 0,
            rng: Rng(seed.max(1)),
            max_round: 0,
            committed: BTreeMap::new(),
            synced: 0,
        };
        for i in 0..sim.nodes.len() {
            if sim.nodes[i].behavior != Behavior::Crashed {
                let mut v = ParentCheck(sim.nodes[i].parent);
                let actions = sim.nodes[i].engine.start(&mut v);
                sim.handle(i, actions);
            }
        }
        sim
    }

    fn broadcast_to(&mut self, recipients: impl Iterator<Item = usize>, msg: Msg) {
        for to in recipients {
            self.in_flight.push((to, msg.clone()));
        }
    }

    fn all(&self) -> std::ops::Range<usize> {
        0..self.nodes.len()
    }

    fn handle(&mut self, i: usize, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::BuildBlock { height, round } => {
                    let block = self.build_block(i, height, round, 0);
                    let more = self.nodes[i].engine.on_block_built(round, block);
                    self.handle(i, more);
                }
                Action::Propose(p) => self.send_proposal(i, p),
                Action::Vote(v) => self.send_vote(i, v),
                Action::ScheduleTimeout { timeout, after_ms } => {
                    self.timers.push((self.now + after_ms, i, timeout));
                }
                Action::Decide { block, certificate } => self.adopt(i, block, certificate),
            }
        }
    }

    /// Finalise `block` chez le nœud `i` et démarre la hauteur suivante.
    fn adopt(&mut self, i: usize, block: Block, certificate: CommitCertificate) {
        certificate
            .verify(CHAIN, &self.validators)
            .expect("certificat de commit invalide");
        assert_eq!(certificate.block_hash, block.hash());
        self.max_round = self.max_round.max(certificate.round);
        let node = &mut self.nodes[i];
        let h = node.engine.height();
        assert_eq!(certificate.height, h);
        node.decisions.insert(h, block.hash());
        node.parent = block.hash();
        node.guard.prune_below(h + 1);
        node.engine = Engine::new(
            CHAIN,
            h + 1,
            self.validators.clone(),
            Some(node.index),
            Params::default(),
        );
        self.committed.entry(h).or_insert((block, certificate));
        let mut v = ParentCheck(self.nodes[i].parent);
        let start = self.nodes[i].engine.start(&mut v);
        self.handle(i, start);
        let buffered = std::mem::take(&mut self.nodes[i].future);
        for msg in buffered {
            self.deliver(i, msg);
        }
    }

    fn build_block(&self, i: usize, height: u64, round: u32, salt: u64) -> Block {
        Block {
            header: BlockHeader {
                protocol_version: 1,
                chain_id: CHAIN.into(),
                height,
                timestamp: height * 5_000 + u64::from(round) + salt,
                prev_hash: self.nodes[i].parent,
                tx_root: [0; 32],
                state_root: [0; 32],
                validators_hash: [0; 32],
                proposer: self.nodes[i].index,
            },
            admin_txs: vec![],
            txs: vec![],
        }
    }

    fn send_proposal(&mut self, i: usize, p: Proposal) {
        let node = &mut self.nodes[i];
        if node.behavior == Behavior::Equivocator {
            let mut other = p.clone();
            other.block = self.build_block(i, p.height, p.round, 1);
            let a = Msg::Proposal(p.sign(&self.nodes[i].keypair));
            let b = Msg::Proposal(other.sign(&self.nodes[i].keypair));
            self.broadcast_to(self.all().filter(|j| j % 2 == 0), a);
            self.broadcast_to(self.all().filter(|j| j % 2 == 1), b);
            return;
        }
        assert!(
            node.guard
                .allow(p.height, p.round, Step::Proposal, Some(p.block.hash())),
            "double proposition"
        );
        let msg = Msg::Proposal(p.sign(&node.keypair));
        self.broadcast_to(self.all(), msg);
    }

    fn send_vote(&mut self, i: usize, v: Vote) {
        let node = &mut self.nodes[i];
        if node.behavior == Behavior::Equivocator {
            let mut other = v.clone();
            other.block_hash = match v.block_hash {
                Some(_) => None,
                None => Some([0xee; 32]),
            };
            let a = Msg::Vote(SignedVote::new(v, node.index, &node.keypair));
            let b = Msg::Vote(SignedVote::new(other, node.index, &node.keypair));
            self.broadcast_to(self.all().filter(|j| j % 2 == 0), a);
            self.broadcast_to(self.all().filter(|j| j % 2 == 1), b);
            return;
        }
        assert!(
            node.guard
                .allow(v.height, v.round, v.kind.into(), v.block_hash),
            "double vote"
        );
        let msg = Msg::Vote(SignedVote::new(v, node.index, &node.keypair));
        self.broadcast_to(self.all(), msg);
    }

    fn deliver(&mut self, i: usize, msg: Msg) {
        let node = &mut self.nodes[i];
        if node.behavior == Behavior::Crashed {
            return;
        }
        let h = node.engine.height();
        if msg.height() > h {
            // Le réseau est déjà plus loin : on rattrape avec un bloc certifié.
            if let Some((block, cert)) = self.committed.get(&h).cloned() {
                self.synced += 1;
                self.nodes[i].future.push(msg);
                self.adopt(i, block, cert);
                return;
            }
            node.future.push(msg);
            return;
        }
        // Gossip : un nœud honnête relaie chaque message la première fois qu'il le voit.
        let id = match &msg {
            Msg::Proposal(p) => borsh::to_vec(p).unwrap(),
            Msg::Vote(v) => borsh::to_vec(v).unwrap(),
        };
        if !node.seen.insert(id) {
            return;
        }
        if node.behavior == Behavior::Honest {
            let others: Vec<usize> = (0..self.nodes.len()).filter(|j| *j != i).collect();
            self.broadcast_to(others.into_iter(), msg.clone());
        }
        let node = &mut self.nodes[i];
        let mut v = ParentCheck(node.parent);
        let actions = match msg {
            Msg::Proposal(p) => node.engine.on_proposal(p, &mut v),
            Msg::Vote(vote) => node.engine.on_vote(vote, &mut v),
        };
        self.handle(i, actions);
    }

    /// Un pas de simulation. Renvoie `false` s'il ne reste rien à faire.
    fn step(&mut self) -> bool {
        if !self.in_flight.is_empty() {
            let k = self.rng.below(self.in_flight.len());
            let (to, msg) = self.in_flight.swap_remove(k);
            self.deliver(to, msg);
            return true;
        }
        let Some(k) = (0..self.timers.len()).min_by_key(|k| self.timers[*k].0) else {
            return false;
        };
        let (at, i, timeout) = self.timers.swap_remove(k);
        self.now = self.now.max(at);
        let node = &mut self.nodes[i];
        let mut v = ParentCheck(node.parent);
        let actions = node.engine.on_timeout(timeout, &mut v);
        self.handle(i, actions);
        true
    }

    fn honest(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(|n| n.behavior == Behavior::Honest)
    }

    /// Fait tourner jusqu'à ce que tous les nœuds honnêtes aient finalisé `target` hauteurs.
    fn run(&mut self, target: u64, budget: usize) {
        for _ in 0..budget {
            // Au-delà de 50 rounds sur une hauteur, la vivacité est perdue : on arrête.
            let stuck = self.honest().any(|n| n.engine.round() > 50);
            if stuck || self.honest().all(|n| n.decisions.len() as u64 >= target) || !self.step() {
                break;
            }
        }
    }

    /// Sûreté : à chaque hauteur, tous les nœuds honnêtes ont finalisé le même bloc.
    fn assert_agreement(&self) {
        let mut seen: BTreeMap<u64, Hash> = BTreeMap::new();
        for node in self.honest() {
            for (h, hash) in &node.decisions {
                let first = seen.entry(*h).or_insert(*hash);
                assert_eq!(first, hash, "deux blocs finalisés à la hauteur {h}");
            }
        }
    }

    fn min_height(&self) -> u64 {
        self.honest()
            .map(|n| n.decisions.len() as u64)
            .min()
            .unwrap_or(0)
    }
}

use Behavior::{Crashed, Equivocator, Honest};

#[test]
fn four_honest_validators_finalize() {
    let mut sim = Sim::new(&[Honest; 4], 1);
    sim.run(10, 200_000);
    sim.assert_agreement();
    assert_eq!(sim.min_height(), 10);
    assert_eq!(
        sim.max_round, 0,
        "sans panne, tout se finalise au premier round"
    );
}

#[test]
fn one_crash_out_of_four_is_tolerated() {
    for seed in 1..=10 {
        let mut sim = Sim::new(&[Honest, Honest, Honest, Crashed], seed);
        sim.run(8, 400_000);
        sim.assert_agreement();
        assert_eq!(sim.min_height(), 8, "graine {seed}");
        // Le validateur en panne est proposeur une hauteur sur quatre : rounds supplémentaires.
        assert!(sim.max_round >= 1);
    }
}

#[test]
fn two_crashes_out_of_four_halt_without_diverging() {
    let mut sim = Sim::new(&[Honest, Honest, Crashed, Crashed], 7);
    sim.run(1, 50_000);
    sim.assert_agreement();
    assert_eq!(
        sim.min_height(),
        0,
        "sans quorum, rien ne doit être finalisé"
    );
}

#[test]
fn seven_validators_with_two_crashes() {
    for seed in 1..=10 {
        let mut sim = Sim::new(
            &[Honest, Crashed, Honest, Honest, Crashed, Honest, Honest],
            seed,
        );
        sim.run(7, 800_000);
        sim.assert_agreement();
        assert_eq!(sim.min_height(), 7, "graine {seed}");
    }
}

#[test]
fn an_equivocating_validator_cannot_break_safety() {
    for seed in 1..=20 {
        let mut sim = Sim::new(&[Equivocator, Honest, Honest, Honest], seed);
        sim.run(8, 400_000);
        sim.assert_agreement();
        assert_eq!(sim.min_height(), 8, "graine {seed}");
    }
}

#[test]
fn two_equivocators_among_seven() {
    for seed in 1..=10 {
        let mut sim = Sim::new(
            &[
                Honest,
                Equivocator,
                Honest,
                Honest,
                Honest,
                Equivocator,
                Honest,
            ],
            seed,
        );
        sim.run(6, 800_000);
        sim.assert_agreement();
        assert_eq!(sim.min_height(), 6, "graine {seed}");
    }
}

// ── Règles d'acceptation du moteur ──────────────────────────────────────────

struct AcceptAll;

impl BlockValidator for AcceptAll {
    fn is_valid(&mut self, _: &Block) -> bool {
        true
    }
}

fn engine_with(n: usize) -> (Vec<KeyPair>, Engine) {
    let keys: Vec<KeyPair> = (0..n).map(|_| KeyPair::generate()).collect();
    let validators = keys.iter().map(KeyPair::public_key).collect();
    (
        keys,
        Engine::new(CHAIN, 1, validators, None, Params::default()),
    )
}

fn block(height: u64, proposer: u16) -> Block {
    Block {
        header: BlockHeader {
            protocol_version: 1,
            chain_id: CHAIN.into(),
            height,
            timestamp: 1,
            prev_hash: [0; 32],
            tx_root: [0; 32],
            state_root: [0; 32],
            validators_hash: [0; 32],
            proposer,
        },
        admin_txs: vec![],
        txs: vec![],
    }
}

fn proposal(round: u32, b: Block) -> Proposal {
    Proposal {
        chain_id: CHAIN.into(),
        height: 1,
        round,
        pol_round: None,
        block: b,
    }
}

fn precommit(hash: Hash) -> Vote {
    Vote {
        chain_id: CHAIN.into(),
        kind: vinx_core::VoteKind::Precommit,
        height: 1,
        round: 0,
        block_hash: Some(hash),
    }
}

/// Quorum de precommits + proposition : décision avec un certificat valide.
#[test]
fn decides_on_proposal_and_precommit_quorum() {
    let (keys, mut e) = engine_with(4);
    let b = block(1, 1); // proposeur de (h=1, r=0) avec n=4 : indice 1
    let hash = b.hash();
    assert!(
        e.on_proposal(proposal(0, b).sign(&keys[1]), &mut AcceptAll)
            .is_empty()
    );
    let mut decided = false;
    for i in [0u16, 2, 3] {
        let actions = e.on_vote(
            SignedVote::new(precommit(hash), i, &keys[usize::from(i)]),
            &mut AcceptAll,
        );
        decided |= actions.iter().any(|a| matches!(a, Action::Decide { .. }));
    }
    assert!(decided && e.is_decided());
}

/// Une proposition signée par un autre que le proposeur du round est ignorée.
#[test]
fn proposal_from_wrong_proposer_is_ignored() {
    let (keys, mut e) = engine_with(4);
    let b = block(1, 1);
    let hash = b.hash();
    e.on_proposal(proposal(0, b).sign(&keys[2]), &mut AcceptAll);
    for i in [0u16, 2, 3] {
        let actions = e.on_vote(
            SignedVote::new(precommit(hash), i, &keys[usize::from(i)]),
            &mut AcceptAll,
        );
        assert!(!actions.iter().any(|a| matches!(a, Action::Decide { .. })));
    }
}

/// Un vote répété, signé par une autre clé ou d'un indice inconnu ne compte pas.
#[test]
fn duplicate_forged_or_unknown_votes_do_not_count() {
    let (keys, mut e) = engine_with(4);
    let b = block(1, 1);
    let hash = b.hash();
    e.on_proposal(proposal(0, b).sign(&keys[1]), &mut AcceptAll);
    let v0 = SignedVote::new(precommit(hash), 0, &keys[0]);
    e.on_vote(v0.clone(), &mut AcceptAll);
    e.on_vote(v0, &mut AcceptAll); // doublon
    e.on_vote(
        SignedVote::new(precommit(hash), 2, &keys[0]),
        &mut AcceptAll,
    ); // mauvaise clé
    e.on_vote(
        SignedVote::new(precommit(hash), 9, &keys[3]),
        &mut AcceptAll,
    ); // indice inconnu
    assert!(!e.is_decided());
    e.on_vote(
        SignedVote::new(precommit(hash), 2, &keys[2]),
        &mut AcceptAll,
    );
    e.on_vote(
        SignedVote::new(precommit(hash), 3, &keys[3]),
        &mut AcceptAll,
    );
    assert!(e.is_decided());
}

/// Un bloc que le nœud juge invalide n'est jamais décidé, même avec un quorum.
#[test]
fn invalid_block_is_never_decided() {
    struct RejectAll;
    impl BlockValidator for RejectAll {
        fn is_valid(&mut self, _: &Block) -> bool {
            false
        }
    }
    let (keys, mut e) = engine_with(4);
    let b = block(1, 1);
    let hash = b.hash();
    e.on_proposal(proposal(0, b).sign(&keys[1]), &mut RejectAll);
    for i in 0..4u16 {
        e.on_vote(
            SignedVote::new(precommit(hash), i, &keys[usize::from(i)]),
            &mut RejectAll,
        );
    }
    assert!(!e.is_decided());
}
