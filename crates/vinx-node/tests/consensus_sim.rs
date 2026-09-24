//! Full-stack consensus simulation (ADR 0082): several validators, each running the real
//! BFT engine on top of the real block execution, connected by a deterministic in-memory
//! network with a virtual clock. Covers liveness (n=4, one crash), safety (two crashes
//! halt, never two different commits), and the validation of committed blocks
//! (forged certificate, wrong proposer, swapped `last_commit`, future timestamp).

use std::collections::{BTreeMap, HashSet};

use vinx_core::consensus::{CommitCert, SignedVote, VoteKind};
use vinx_core::{Block, PoolStatus, ValidatorPoolEntry};
use vinx_crypto::{Address, BlsPubKey, BlsSecretKey, Hash32, KeyPair};
use vinx_node::bft::{Engine, HeightParams, Host, Input, Output, Timeouts};
use vinx_node::chain::{Chain, Tip};
use vinx_node::execution;
use vinx_state::{create_genesis_state, GenesisBlsKey, GenesisConfig, WorldState};

const CHAIN: u32 = vinx_core::CHAIN_ID_DEVNET;

struct Val {
    addr: Address,
    sk: BlsSecretKey,
}

fn genesis(n: usize) -> (WorldState, Vec<Val>) {
    let vals: Vec<Val> = (0..n)
        .map(|_| {
            let kp = KeyPair::generate();
            Val {
                addr: Address::from_public_key(&kp.public_key()),
                sk: BlsSecretKey::generate(),
            }
        })
        .collect();
    let admin = Address::from_public_key(&KeyPair::generate().public_key());
    let mut state = create_genesis_state(&GenesisConfig {
        chain_id: CHAIN,
        admin_address: admin,
        validator_address: vals[0].addr,
        validator_bls: GenesisBlsKey::from_secret(&vals[0].sk, &vals[0].addr, CHAIN),
    });
    for v in &vals[1..] {
        let key = GenesisBlsKey::from_secret(&v.sk, &v.addr, CHAIN);
        let mut entry = ValidatorPoolEntry::new(0, 0);
        entry.status = PoolStatus::Active;
        entry.bls_pub_key = Some(key.pub_key.to_vec());
        entry.bls_pop = Some(key.pop.to_vec());
        state.validator_pool.insert(v.addr, entry);
    }
    state.validator_set = state.weighted_validator_set(vals.iter().map(|v| v.addr).collect());
    (state, vals)
}

/// Host backed by the real execution path, like the node's `NodeHost`.
struct SimHost {
    base: WorldState,
    tip: Tip,
    me: Address,
    now: u64,
    post: BTreeMap<Hash32, WorldState>,
    signed: HashSet<(u32, u8)>,
}

impl Host for SimHost {
    fn build_block(&mut self, round: u32) -> Option<Block> {
        let (b, s, _) =
            execution::build_block(&self.base, &self.tip, &[], self.me, round, self.now).ok()?;
        self.post.insert(b.hash(), s);
        Some(b)
    }
    fn validate_block(&mut self, block: &Block) -> bool {
        match execution::execute_block(&self.base, &self.tip, block, self.now) {
            Ok(s) => {
                self.post.insert(block.hash(), s);
                true
            }
            Err(_) => false,
        }
    }
    fn may_sign(&mut self, v: &SignedVote) -> bool {
        self.signed.insert((v.round, v.kind as u8))
    }
}

struct SimNode {
    val_idx: usize,
    state: WorldState,
    chain: Chain,
    engine: Engine,
    host: SimHost,
    commits: Vec<(Block, CommitCert)>,
}

fn params(state: &WorldState, height: u64, me: Option<(Address, BlsSecretKey)>) -> HeightParams {
    let vs = state.validator_set.clone();
    let keys = state
        .indexed_bls_keys(&vs)
        .into_iter()
        .map(|k| k.and_then(|b| BlsPubKey::from_bytes(&b).ok()))
        .collect();
    let jailed = state
        .reliability
        .iter()
        .filter(|(_, r)| r.is_jailed())
        .map(|(a, _)| *a)
        .collect();
    HeightParams {
        chain_id: CHAIN,
        height,
        validators: vs,
        keys,
        jailed,
        me,
        timeouts: Timeouts::default(),
    }
}

impl SimNode {
    fn new(state: WorldState, val_idx: usize, vals: &[Val], now: u64) -> Self {
        let (chain, _) = Chain::new_with_genesis(vals[0].addr, 0);
        let tip = chain.tip();
        let me = (vals[val_idx].addr, vals[val_idx].sk.clone());
        SimNode {
            val_idx,
            engine: Engine::new(params(&state, 1, Some(me))),
            host: SimHost {
                base: state.clone(),
                tip,
                me: vals[val_idx].addr,
                now,
                post: BTreeMap::new(),
                signed: HashSet::new(),
            },
            state,
            chain,
            commits: vec![],
        }
    }

    fn next_height(&mut self, vals: &[Val], now: u64) {
        let h = self.chain.tip_height() + 1;
        let me = (vals[self.val_idx].addr, vals[self.val_idx].sk.clone());
        self.engine = Engine::new(params(&self.state, h, Some(me)));
        self.host = SimHost {
            base: self.state.clone(),
            tip: self.chain.tip(),
            me: vals[self.val_idx].addr,
            now,
            post: BTreeMap::new(),
            signed: HashSet::new(),
        };
    }
}

#[derive(Clone)]
enum Ev {
    Deliver(usize, Input),
    Timeout(usize, u64, Input),
}

/// Deterministic network: events ordered by virtual time (ms), then FIFO.
struct Net {
    nodes: Vec<SimNode>,
    vals: Vec<Val>,
    online: Vec<bool>,
    queue: BTreeMap<(u64, u64), Ev>,
    seq: u64,
    clock_ms: u64,
    genesis: WorldState,
}

impl Net {
    fn new(n: usize) -> Self {
        let (state, vals) = genesis(n);
        let now = 12;
        let nodes = (0..n)
            .map(|i| SimNode::new(state.clone(), i, &vals, now))
            .collect::<Vec<_>>();
        let nodes = nodes.into_iter().collect();
        Net {
            nodes,
            vals,
            online: vec![true; n],
            queue: BTreeMap::new(),
            seq: 0,
            clock_ms: 0,
            genesis: state,
        }
    }

    fn push(&mut self, at: u64, ev: Ev) {
        self.seq += 1;
        self.queue.insert((at, self.seq), ev);
    }

    fn process(&mut self, i: usize, outs: Vec<Output>) {
        for o in outs {
            match o {
                Output::BroadcastProposal(p) => self.broadcast(i, Input::Proposal(p)),
                Output::BroadcastVote(v) => self.broadcast(i, Input::Vote(v)),
                Output::ScheduleTimeout { kind, round, after } => {
                    let h = self.nodes[i].engine.height();
                    let at = self.clock_ms + after.as_millis() as u64;
                    self.push(at, Ev::Timeout(i, h, Input::Timeout { kind, round }));
                }
                Output::Commit { block, cert } => {
                    self.broadcast(
                        i,
                        Input::Committed {
                            block: block.clone(),
                            cert: cert.clone(),
                        },
                    );
                    self.apply_commit(i, block, cert);
                }
                Output::Evidence(_) => {}
            }
        }
    }

    fn apply_commit(&mut self, i: usize, block: Block, cert: CommitCert) {
        let node = &mut self.nodes[i];
        if block.header.height != node.chain.tip_height() + 1 {
            return;
        }
        let post = node
            .host
            .post
            .remove(&block.hash())
            .expect("a committed block was validated first");
        node.state = post;
        node.chain.push(block.clone(), Some(cert.clone()));
        node.commits.push((block, cert));
        let now = node.chain.tip_timestamp() + node.state.block_time_secs;
        node.next_height(&self.vals, now);
        let n = &mut self.nodes[i];
        let outs = n.engine.start(&mut n.host);
        self.process(i, outs);
    }

    fn broadcast(&mut self, from: usize, input: Input) {
        for j in 0..self.nodes.len() {
            if j != from {
                self.push(self.clock_ms + 50, Ev::Deliver(j, input.clone()));
            }
        }
    }

    fn start(&mut self) {
        for i in 0..self.nodes.len() {
            if self.online[i] {
                let n = &mut self.nodes[i];
                let outs = n.engine.start(&mut n.host);
                self.process(i, outs);
            }
        }
    }

    /// Runs until every online node reached `height` or the virtual deadline passes.
    fn run_until(&mut self, height: u64, deadline_ms: u64) {
        while let Some((&(at, seq), _)) = self.queue.iter().next() {
            if at > deadline_ms || self.all_online_at(height) {
                break;
            }
            let ev = self.queue.remove(&(at, seq)).unwrap();
            self.clock_ms = at;
            let (i, input) = match ev {
                Ev::Deliver(i, inp) => (i, inp),
                Ev::Timeout(i, h, inp) => {
                    if self.nodes[i].engine.height() != h {
                        continue;
                    }
                    (i, inp)
                }
            };
            if !self.online[i] {
                continue;
            }
            let n = &mut self.nodes[i];
            let outs = n.engine.handle(input, &mut n.host);
            self.process(i, outs);
        }
    }

    fn all_online_at(&self, height: u64) -> bool {
        self.nodes
            .iter()
            .zip(&self.online)
            .filter(|(_, on)| **on)
            .all(|(n, _)| n.chain.tip_height() >= height)
    }

    fn assert_agreement(&self) {
        let online: Vec<&SimNode> = self
            .nodes
            .iter()
            .zip(&self.online)
            .filter(|(_, on)| **on)
            .map(|(n, _)| n)
            .collect();
        let min = online.iter().map(|n| n.chain.tip_height()).min().unwrap();
        for h in 1..=min {
            let hashes: HashSet<Hash32> = online
                .iter()
                .map(|n| n.chain.get_block(h).unwrap().hash())
                .collect();
            assert_eq!(hashes.len(), 1, "conflicting commits at height {h}");
        }
        // Same blocks ⇒ same state (rewards, jailing and base fee are deterministic).
        let roots: HashSet<Hash32> = online
            .iter()
            .filter(|n| n.chain.tip_height() == min)
            .map(|n| n.state.clone().compute_state_root())
            .collect();
        assert_eq!(roots.len(), 1, "diverging states at height {min}");
    }
}

#[test]
fn four_validators_commit_and_agree() {
    let mut net = Net::new(4);
    net.start();
    net.run_until(5, 600_000);
    assert!(net.all_online_at(5), "n=4 must make progress");
    net.assert_agreement();
    // From height 2, every block embeds the previous certificate (co-signer rewards).
    for (b, _) in &net.nodes[0].commits[1..] {
        assert!(b.last_commit.is_some());
    }
}

#[test]
fn one_crash_is_tolerated_and_the_absent_proposer_is_charged() {
    let mut net = Net::new(4);
    net.online[3] = false;
    let crashed = net.vals[3].addr;
    net.start();
    net.run_until(8, 3_600_000);
    assert!(net.all_online_at(8), "3 of 4 must keep committing");
    net.assert_agreement();
    let rel = net.nodes[0].state.reliability.get(&crashed).cloned();
    let charged = rel
        .map(|r| r.missed_proposals > 0 || r.is_jailed())
        .unwrap_or(false);
    assert!(
        charged,
        "the crashed proposer's skipped rounds must be recorded"
    );
}

#[test]
fn two_crashes_halt_without_committing() {
    let mut net = Net::new(4);
    net.online[2] = false;
    net.online[3] = false;
    net.start();
    net.run_until(1, 600_000);
    for n in &net.nodes {
        assert_eq!(
            n.chain.tip_height(),
            0,
            "no quorum ⇒ no commit (safety first)"
        );
    }
}

/// A committed block 2 from the honest network, plus the state and tip it applies to.
fn committed_pair() -> (Net, WorldState, Tip, Block, CommitCert) {
    let mut net = Net::new(4);
    net.start();
    net.run_until(2, 600_000);
    let (b1, c1) = net.nodes[0].commits[0].clone();
    let (b2, c2) = net.nodes[0].commits[1].clone();
    let (mut chain, _) = Chain::new_with_genesis(net.vals[0].addr, 0);
    let s1 = execution::execute_block(&net.genesis, &chain.tip(), &b1, 1_000).unwrap();
    chain.push(b1, Some(c1));
    let tip = chain.tip();
    (net, s1, tip, b2, c2)
}

#[test]
fn honest_committed_block_is_accepted() {
    let (_, s1, tip, b2, c2) = committed_pair();
    execution::execute_committed(&s1, &tip, &b2, &c2, 1_000).expect("honest block");
}

#[test]
fn forged_certificate_from_outsiders_is_rejected() {
    let (_, s1, tip, b2, c2) = committed_pair();
    let outsiders: Vec<BlsSecretKey> = (0..4).map(|_| BlsSecretKey::generate()).collect();
    let bytes = vinx_core::consensus::vote_sign_bytes(
        CHAIN,
        VoteKind::Precommit,
        2,
        c2.round,
        Some(&b2.hash()),
    );
    let sigs = outsiders
        .iter()
        .enumerate()
        .map(|(i, sk)| (i, sk.sign(&bytes)))
        .collect();
    let forged = CommitCert::from_precommits(2, c2.round, b2.hash(), sigs).unwrap();
    assert!(execution::execute_committed(&s1, &tip, &b2, &forged, 1_000).is_err());
    // A genuine certificate for another block does not transfer either.
    let mut other = b2.clone();
    other.header.timestamp += 1;
    assert!(execution::execute_committed(&s1, &tip, &other, &c2, 1_000).is_err());
}

#[test]
fn block_from_the_wrong_proposer_is_rejected() {
    let (net, s1, tip, b2, _) = committed_pair();
    let wrong = net
        .vals
        .iter()
        .map(|v| v.addr)
        .find(|a| *a != b2.header.validator)
        .unwrap();
    assert!(
        execution::build_block(&s1, &tip, &[], wrong, b2.header.round, b2.header.timestamp)
            .is_err(),
        "only the round's proposer may build"
    );
    let mut forged = b2.clone();
    forged.header.validator = wrong;
    assert!(execution::execute_block(&s1, &tip, &forged, 1_000).is_err());
}

#[test]
fn swapped_last_commit_is_rejected() {
    let (_, s1, tip, b2, _) = committed_pair();
    let mut bad = b2.clone();
    bad.last_commit = None;
    assert!(execution::execute_block(&s1, &tip, &bad, 1_000).is_err());
}

#[test]
fn future_timestamp_is_rejected() {
    let (_, s1, tip, b2, _) = committed_pair();
    let now = b2.header.timestamp - vinx_core::amount::MAX_CLOCK_DRIFT_SECS - 1;
    assert!(execution::execute_block(&s1, &tip, &b2, now).is_err());
}
