//! BFT consensus engine (ADR 0082) — Tendermint, as a pure state machine.
//!
//! One [`Engine`] drives one height. It follows Algorithm 1 of *"The latest gossip on BFT
//! consensus"* (Buchman, Kwon, Milosevic, 2018): in each round the scheduled proposer
//! proposes a block, validators **prevote**, then **precommit**, and the block is
//! committed once precommits of strictly more than 2/3 of the voting power agree on it.
//! Locking (`locked`) and the valid value (`valid`) make it safe across rounds: two
//! different blocks can never both be committed at one height while faulty power stays
//! below 1/3. A silent proposer only costs a round (timeouts), never safety.
//!
//! The engine does no I/O and reads no clock: inputs are messages and fired timeouts,
//! outputs are messages to broadcast, timeouts to schedule and the commit. Block building
//! and validation are delegated to a [`Host`]. This makes it deterministic and testable
//! in simulation — see the tests at the bottom and `tests/consensus_sim.rs`.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use vinx_core::consensus::proposal_sign_bytes;
use vinx_core::{Block, CommitCert, SignedVote, ValidatorSet, VoteEquivocation, VoteKind};
use vinx_crypto::{bls_verify, Address, BlsPubKey, BlsSecretKey, BlsSignature, Hash32};

/// A signed block proposal for `(height, round)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Proposal {
    pub height: u64,
    pub round: u32,
    /// Round of the proof-of-lock this block is re-proposed from (`None` = fresh block).
    pub pol_round: Option<u32>,
    pub block: Block,
    /// Proposer's BLS signature (96 bytes) over [`proposal_sign_bytes`].
    pub signature: Vec<u8>,
}

impl Proposal {
    pub fn sign_bytes(&self, chain_id: u32) -> Vec<u8> {
        proposal_sign_bytes(
            chain_id,
            self.height,
            self.round,
            self.pol_round,
            &self.block.hash(),
        )
    }

    pub fn verify(&self, chain_id: u32, pk: &BlsPubKey) -> bool {
        let Ok(sig) = <[u8; 96]>::try_from(self.signature.as_slice()) else {
            return false;
        };
        bls_verify(pk, &BlsSignature(sig), &self.sign_bytes(chain_id)).is_ok()
    }
}

/// Which step a timeout belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimeoutKind {
    Propose,
    Prevote,
    Precommit,
}

/// Engine inputs.
#[derive(Clone, Debug)]
pub enum Input {
    Proposal(Proposal),
    Vote(SignedVote),
    Timeout {
        kind: TimeoutKind,
        round: u32,
    },
    /// A block committed elsewhere, with its certificate (gossip or sync). Accepted once
    /// the certificate verifies against this height's voting set and the block is valid —
    /// how a node that missed the proposal still decides.
    Committed {
        block: Block,
        cert: CommitCert,
    },
}

/// Engine outputs.
#[derive(Clone, Debug)]
pub enum Output {
    BroadcastProposal(Proposal),
    BroadcastVote(SignedVote),
    ScheduleTimeout {
        kind: TimeoutKind,
        round: u32,
        after: Duration,
    },
    /// The height is decided: `block`, committed by `cert`.
    Commit {
        block: Block,
        cert: CommitCert,
    },
    /// A validator signed two conflicting votes — slashable evidence.
    Evidence(VoteEquivocation),
}

/// What the engine needs from the node.
pub trait Host {
    /// Builds a fresh block for `round` of the current height (we are its proposer).
    fn build_block(&mut self, round: u32) -> Option<Block>;
    /// Full validation of a proposed block for the current height.
    fn validate_block(&mut self, block: &Block) -> bool;
    /// Durable double-sign guard: return `true` only if signing this vote cannot conflict
    /// with anything we signed before (same kind/height/round, other value).
    fn may_sign(&mut self, vote: &SignedVote) -> bool;
}

/// Timeout schedule. Each round waits a little longer, so the network eventually has
/// enough time whatever its (unknown) delays — the partial-synchrony liveness argument.
#[derive(Clone, Debug)]
pub struct Timeouts {
    pub propose: Duration,
    pub propose_delta: Duration,
    pub vote: Duration,
    pub vote_delta: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            propose: Duration::from_millis(3_000),
            propose_delta: Duration::from_millis(1_000),
            vote: Duration::from_millis(1_000),
            vote_delta: Duration::from_millis(500),
        }
    }
}

impl Timeouts {
    fn propose(&self, round: u32) -> Duration {
        self.propose + self.propose_delta * round
    }
    fn vote(&self, round: u32) -> Duration {
        self.vote + self.vote_delta * round
    }
}

/// Static parameters of one height.
pub struct HeightParams {
    pub chain_id: u32,
    pub height: u64,
    /// The voting set of this height, with its proposer priorities.
    pub validators: ValidatorSet,
    /// Registered BLS key of each validator (`keys[i]` ↔ `validators.validators()[i]`).
    pub keys: Vec<Option<BlsPubKey>>,
    /// Validators skipped by the proposer rotation (jailed).
    pub jailed: HashSet<Address>,
    /// Our identity, when we are a validator of this height.
    pub me: Option<(Address, BlsSecretKey)>,
    pub timeouts: Timeouts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Propose,
    Prevote,
    Precommit,
}

/// Votes of one `(round, kind)`.
#[derive(Default)]
struct VoteSet {
    by_validator: HashMap<usize, SignedVote>,
    power_for: HashMap<Option<Hash32>, u64>,
    total: u64,
}

impl VoteSet {
    fn power(&self, value: &Option<Hash32>) -> u64 {
        self.power_for.get(value).copied().unwrap_or(0)
    }
}

/// The consensus state machine for one height.
pub struct Engine {
    p: HeightParams,
    my_index: Option<usize>,
    round: u32,
    step: Step,
    locked: Option<(u32, Hash32)>,
    valid: Option<(u32, Hash32)>,
    /// Blocks seen in proposals, by hash.
    blocks: HashMap<Hash32, Block>,
    /// Validation verdicts, by block hash.
    validity: HashMap<Hash32, bool>,
    /// First proposal of the rightful proposer, per round.
    proposals: HashMap<u32, Proposal>,
    votes: HashMap<(u32, VoteKind), VoteSet>,
    prevote_timeout_armed: HashSet<u32>,
    precommit_timeout_armed: HashSet<u32>,
    /// Rounds for which the "polka for the proposal" rule (line 36) already fired.
    polka_seen: HashSet<u32>,
    decided: Option<(u32, Hash32)>,
    started: bool,
}

impl Engine {
    pub fn new(p: HeightParams) -> Self {
        let my_index = p.me.as_ref().and_then(|(a, _)| p.validators.index_of(a));
        Self {
            p,
            my_index,
            round: 0,
            step: Step::Propose,
            locked: None,
            valid: None,
            blocks: HashMap::new(),
            validity: HashMap::new(),
            proposals: HashMap::new(),
            votes: HashMap::new(),
            prevote_timeout_armed: HashSet::new(),
            precommit_timeout_armed: HashSet::new(),
            polka_seen: HashSet::new(),
            decided: None,
            started: false,
        }
    }

    pub fn height(&self) -> u64 {
        self.p.height
    }
    pub fn round(&self) -> u32 {
        self.round
    }
    pub fn step(&self) -> Step {
        self.step
    }
    pub fn is_decided(&self) -> bool {
        self.decided.is_some()
    }
    pub fn validators(&self) -> &ValidatorSet {
        &self.p.validators
    }

    /// Restores the Tendermint lock after a restart: the highest-round value we
    /// precommitted at this height (read from the durable signing record). Without it a
    /// restarted validator could prevote a conflicting block in a later round.
    pub fn restore_lock(&mut self, round: u32, hash: Hash32) {
        if self.locked.is_none_or(|(r, _)| round > r) {
            self.locked = Some((round, hash));
        }
    }

    /// Scheduled proposer of `round` at this height.
    pub fn proposer(&self, round: u32) -> Address {
        let jailed = &self.p.jailed;
        *self.p.validators.proposer(round, |a| !jailed.contains(a))
    }

    /// Starts round 0. Call once.
    pub fn start(&mut self, host: &mut dyn Host) -> Vec<Output> {
        let mut out = Vec::new();
        if !self.started {
            self.started = true;
            self.start_round(0, host, &mut out);
            self.run_rules(host, &mut out);
        }
        out
    }

    /// Feeds one input; returns the resulting outputs.
    pub fn handle(&mut self, input: Input, host: &mut dyn Host) -> Vec<Output> {
        let mut out = Vec::new();
        match input {
            Input::Proposal(p) => self.on_proposal(p, host, &mut out),
            Input::Vote(v) => self.on_vote(v, &mut out),
            Input::Timeout { kind, round } => self.on_timeout(kind, round, host, &mut out),
            Input::Committed { block, cert } => self.on_committed(block, cert, host, &mut out),
        }
        if self.started {
            self.run_rules(host, &mut out);
        }
        out
    }

    /// Our own messages of the current round (proposal and votes), for periodic
    /// re-broadcast: gossip is not reliable, and a peer that reconnects after a partition
    /// must still receive them (Tendermint re-gossips votes for the same reason).
    pub fn rebroadcast(&self) -> Vec<Output> {
        let mut out = Vec::new();
        if let Some(p) = self.proposals.get(&self.round) {
            if self.p.me.as_ref().map(|(a, _)| *a) == Some(self.proposer(self.round)) {
                out.push(Output::BroadcastProposal(p.clone()));
            }
        }
        if let Some(i) = self.my_index {
            for kind in [VoteKind::Prevote, VoteKind::Precommit] {
                if let Some(v) = self
                    .votes
                    .get(&(self.round, kind))
                    .and_then(|s| s.by_validator.get(&i))
                {
                    out.push(Output::BroadcastVote(v.clone()));
                }
            }
        }
        out
    }

    /// The best commit certificate known for the decided block — precommits keep being
    /// collected after the decision, so the next block can credit more co-signers.
    pub fn best_commit(&self) -> Option<CommitCert> {
        let (round, hash) = self.decided?;
        self.build_cert(round, hash)
    }

    // ── inputs ────────────────────────────────────────────────────────────────

    fn on_proposal(&mut self, prop: Proposal, host: &mut dyn Host, out: &mut Vec<Output>) {
        if prop.height != self.p.height || prop.block.header.height != self.p.height {
            return;
        }
        if prop.pol_round.is_some_and(|vr| vr >= prop.round) {
            return;
        }
        if self.proposals.contains_key(&prop.round) {
            return; // first proposal of the rightful proposer wins
        }
        let proposer = self.proposer(prop.round);
        let Some(idx) = self.p.validators.index_of(&proposer) else {
            return;
        };
        let Some(Some(pk)) = self.p.keys.get(idx) else {
            return;
        };
        if !prop.verify(self.p.chain_id, pk) {
            return;
        }
        let hash = prop.block.hash();
        self.blocks
            .entry(hash)
            .or_insert_with(|| prop.block.clone());
        self.validity
            .entry(hash)
            .or_insert_with(|| host.validate_block(&prop.block));
        self.proposals.insert(prop.round, prop);
        let _ = out;
    }

    fn on_committed(
        &mut self,
        block: Block,
        cert: CommitCert,
        host: &mut dyn Host,
        out: &mut Vec<Output>,
    ) {
        if self.decided.is_some() || block.header.height != self.p.height {
            return;
        }
        let hash = block.hash();
        let raw: Vec<Option<[u8; 48]>> = self
            .p
            .keys
            .iter()
            .map(|k| k.as_ref().map(|k| k.0))
            .collect();
        if cert
            .verify(
                self.p.chain_id,
                self.p.height,
                &hash,
                &self.p.validators,
                &raw,
            )
            .is_err()
        {
            return;
        }
        self.validity
            .entry(hash)
            .or_insert_with(|| host.validate_block(&block));
        if !self.is_valid(&hash) {
            return;
        }
        self.decided = Some((cert.round, hash));
        self.blocks.insert(hash, block.clone());
        out.push(Output::Commit { block, cert });
    }

    fn on_vote(&mut self, vote: SignedVote, out: &mut Vec<Output>) {
        if vote.height != self.p.height {
            return;
        }
        let Some(idx) = self.p.validators.index_of(&vote.validator) else {
            return;
        };
        let Some(Some(pk)) = self.p.keys.get(idx) else {
            return;
        };
        let set = self.votes.entry((vote.round, vote.kind)).or_default();
        if let Some(prev) = set.by_validator.get(&idx) {
            if prev.value != vote.value && vote.verify(self.p.chain_id, pk).is_ok() {
                out.push(Output::Evidence(VoteEquivocation {
                    vote_a: prev.clone(),
                    vote_b: vote,
                }));
            }
            return; // one vote per validator per (round, kind); a conflict is not counted
        }
        if vote.verify(self.p.chain_id, pk).is_err() {
            return;
        }
        let power = self.p.validators.power_at(idx);
        *set.power_for.entry(vote.value).or_default() += power;
        set.total += power;
        set.by_validator.insert(idx, vote);
    }

    fn on_timeout(
        &mut self,
        kind: TimeoutKind,
        round: u32,
        host: &mut dyn Host,
        out: &mut Vec<Output>,
    ) {
        if self.decided.is_some() || round != self.round {
            return;
        }
        match kind {
            TimeoutKind::Propose if self.step == Step::Propose => {
                self.vote(VoteKind::Prevote, None, host, out);
                self.step = Step::Prevote;
            }
            TimeoutKind::Prevote if self.step == Step::Prevote => {
                self.vote(VoteKind::Precommit, None, host, out);
                self.step = Step::Precommit;
            }
            TimeoutKind::Precommit => {
                self.start_round(round + 1, host, out);
            }
            _ => {}
        }
    }

    // ── actions ───────────────────────────────────────────────────────────────

    fn start_round(&mut self, round: u32, host: &mut dyn Host, out: &mut Vec<Output>) {
        self.round = round;
        self.step = Step::Propose;
        if self.p.me.as_ref().map(|(a, _)| *a) == Some(self.proposer(round)) {
            let (block, pol_round) = match self.valid {
                Some((vr, vh)) => (self.blocks.get(&vh).cloned(), Some(vr)),
                None => (host.build_block(round), None),
            };
            if let (Some(block), Some((_, sk))) = (block, self.p.me.as_ref()) {
                let mut prop = Proposal {
                    height: self.p.height,
                    round,
                    pol_round,
                    block,
                    signature: vec![],
                };
                prop.signature = sk.sign(&prop.sign_bytes(self.p.chain_id)).0.to_vec();
                out.push(Output::BroadcastProposal(prop.clone()));
                self.on_proposal(prop, host, out);
            }
        }
        out.push(Output::ScheduleTimeout {
            kind: TimeoutKind::Propose,
            round,
            after: self.p.timeouts.propose(round),
        });
        // Liveness fallbacks. Tendermint arms the prevote/precommit timeouts only after
        // seeing 2/3 of the power vote, assuming reliable gossip. Gossip is best-effort
        // here (a partition drops messages, gossipsub de-duplicates re-sends), so a node
        // could wait forever in a round nobody else is in. These later timeouts guarantee
        // it moves on. Changing round never endangers safety — locks alone ensure it.
        let base = self.p.timeouts.propose(round);
        let vote = self.p.timeouts.vote(round);
        out.push(Output::ScheduleTimeout {
            kind: TimeoutKind::Prevote,
            round,
            after: base + vote * 3,
        });
        out.push(Output::ScheduleTimeout {
            kind: TimeoutKind::Precommit,
            round,
            after: base + vote * 6,
        });
    }

    /// Signs and broadcasts our vote (if we are a validator and the guard allows it),
    /// and counts it locally.
    fn vote(
        &mut self,
        kind: VoteKind,
        value: Option<Hash32>,
        host: &mut dyn Host,
        out: &mut Vec<Output>,
    ) {
        let Some((addr, sk)) = self.p.me.as_ref() else {
            return;
        };
        if self.my_index.is_none() {
            return;
        }
        let mut v = SignedVote {
            kind,
            height: self.p.height,
            round: self.round,
            value,
            validator: *addr,
            signature: vec![],
        };
        if !host.may_sign(&v) {
            return;
        }
        v.signature = sk.sign(&v.sign_bytes(self.p.chain_id)).0.to_vec();
        out.push(Output::BroadcastVote(v.clone()));
        self.on_vote(v, out);
    }

    fn build_cert(&self, round: u32, hash: Hash32) -> Option<CommitCert> {
        let set = self.votes.get(&(round, VoteKind::Precommit))?;
        let sigs: Vec<(usize, BlsSignature)> = set
            .by_validator
            .iter()
            .filter(|(_, v)| v.value == Some(hash))
            .filter_map(|(i, v)| {
                <[u8; 96]>::try_from(v.signature.as_slice())
                    .ok()
                    .map(|s| (*i, BlsSignature(s)))
            })
            .collect();
        CommitCert::from_precommits(self.p.height, round, hash, sigs).ok()
    }

    // ── rules ─────────────────────────────────────────────────────────────────

    fn quorum(&self) -> u64 {
        self.p.validators.quorum_power()
    }

    fn power(&self, round: u32, kind: VoteKind, value: &Option<Hash32>) -> u64 {
        self.votes
            .get(&(round, kind))
            .map(|s| s.power(value))
            .unwrap_or(0)
    }

    fn total(&self, round: u32, kind: VoteKind) -> u64 {
        self.votes.get(&(round, kind)).map(|s| s.total).unwrap_or(0)
    }

    fn is_valid(&self, hash: &Hash32) -> bool {
        self.validity.get(hash).copied().unwrap_or(false)
    }

    /// Re-evaluates every "upon" rule until nothing changes.
    fn run_rules(&mut self, host: &mut dyn Host, out: &mut Vec<Output>) {
        loop {
            if self.decided.is_some() {
                return;
            }
            let before = (self.round, self.step, out.len());
            self.rule_commit(out);
            if self.decided.is_some() {
                return;
            }
            self.rule_skip_round(host, out);
            self.rule_proposal(host, out);
            self.rule_prevote_quorum(host, out);
            self.rule_precommit_any(out);
            if before == (self.round, self.step, out.len()) {
                return;
            }
        }
    }

    /// Lines 22 and 28: prevote on the proposal of the current round.
    fn rule_proposal(&mut self, host: &mut dyn Host, out: &mut Vec<Output>) {
        if self.step != Step::Propose {
            return;
        }
        let Some(prop) = self.proposals.get(&self.round) else {
            return;
        };
        let hash = prop.block.hash();
        let valid = self.is_valid(&hash);
        let choice = match prop.pol_round {
            None => {
                let ok = valid && self.locked.is_none_or(|(_, lh)| lh == hash);
                Some(if ok { Some(hash) } else { None })
            }
            Some(vr) => {
                if self.power(vr, VoteKind::Prevote, &Some(hash)) >= self.quorum() {
                    let ok = valid && self.locked.is_none_or(|(lr, lh)| lr <= vr || lh == hash);
                    Some(if ok { Some(hash) } else { None })
                } else {
                    None // wait for the proof-of-lock (or the propose timeout)
                }
            }
        };
        if let Some(value) = choice {
            self.vote(VoteKind::Prevote, value, host, out);
            self.step = Step::Prevote;
        }
    }

    /// Lines 34, 36 and 44.
    fn rule_prevote_quorum(&mut self, host: &mut dyn Host, out: &mut Vec<Output>) {
        let r = self.round;
        let q = self.quorum();
        // Line 34: 2/3 of any prevotes → arm the prevote timeout (once).
        if self.step == Step::Prevote
            && self.total(r, VoteKind::Prevote) >= q
            && self.prevote_timeout_armed.insert(r)
        {
            out.push(Output::ScheduleTimeout {
                kind: TimeoutKind::Prevote,
                round: r,
                after: self.p.timeouts.vote(r),
            });
        }
        // Line 36: proposal + 2/3 prevotes for it, step >= prevote, once per round.
        if self.step >= Step::Prevote && !self.polka_seen.contains(&r) {
            if let Some(hash) = self.proposals.get(&r).map(|p| p.block.hash()) {
                if self.is_valid(&hash) && self.power(r, VoteKind::Prevote, &Some(hash)) >= q {
                    self.polka_seen.insert(r);
                    if self.step == Step::Prevote {
                        self.locked = Some((r, hash));
                        self.vote(VoteKind::Precommit, Some(hash), host, out);
                        self.step = Step::Precommit;
                    }
                    self.valid = Some((r, hash));
                }
            }
        }
        // Line 44: 2/3 prevotes for nil → precommit nil.
        if self.step == Step::Prevote && self.power(r, VoteKind::Prevote, &None) >= q {
            self.vote(VoteKind::Precommit, None, host, out);
            self.step = Step::Precommit;
        }
    }

    /// Line 47: 2/3 of any precommits → arm the precommit timeout (once).
    fn rule_precommit_any(&mut self, out: &mut Vec<Output>) {
        let r = self.round;
        if self.total(r, VoteKind::Precommit) >= self.quorum()
            && self.precommit_timeout_armed.insert(r)
        {
            out.push(Output::ScheduleTimeout {
                kind: TimeoutKind::Precommit,
                round: r,
                after: self.p.timeouts.vote(r),
            });
        }
    }

    /// Line 49: 2/3 precommits for a known valid block in any round → commit.
    fn rule_commit(&mut self, out: &mut Vec<Output>) {
        let q = self.quorum();
        let mut found: Option<(u32, Hash32)> = None;
        for ((round, kind), set) in &self.votes {
            if *kind != VoteKind::Precommit {
                continue;
            }
            for (value, power) in &set.power_for {
                if let Some(h) = value {
                    if *power >= q && self.blocks.contains_key(h) && self.is_valid(h) {
                        found = Some((*round, *h));
                    }
                }
            }
        }
        if let Some((round, hash)) = found {
            if let Some(cert) = self.build_cert(round, hash) {
                self.decided = Some((round, hash));
                out.push(Output::Commit {
                    block: self.blocks[&hash].clone(),
                    cert,
                });
            }
        }
    }

    /// Line 55: more than 1/3 of the power is already in a later round → join it.
    fn rule_skip_round(&mut self, host: &mut dyn Host, out: &mut Vec<Output>) {
        let threshold = self.p.validators.one_third_plus();
        let mut target: Option<u32> = None;
        let rounds: HashSet<u32> = self
            .votes
            .keys()
            .map(|(r, _)| *r)
            .filter(|r| *r > self.round)
            .collect();
        for r in rounds {
            let mut seen: HashSet<usize> = HashSet::new();
            for kind in [VoteKind::Prevote, VoteKind::Precommit] {
                if let Some(set) = self.votes.get(&(r, kind)) {
                    seen.extend(set.by_validator.keys().copied());
                }
            }
            let power = self.p.validators.power_of_indices(seen);
            if power >= threshold && target.is_none_or(|t| r > t) {
                target = Some(r);
            }
        }
        if let Some(r) = target {
            self.start_round(r, host, out);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Deterministic multi-node simulation of the engine alone (blocks are opaque).
    //! The full-stack simulation with real execution lives in `tests/consensus_sim.rs`.
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use vinx_core::BlockHeader;
    use vinx_crypto::KeyPair;

    const CHAIN: u32 = 7;

    struct MockHost {
        me: usize,
        /// Blocks this node refuses (to model a byzantine or buggy proposer).
        reject: HashSet<Hash32>,
        signed: HashMap<(VoteKind, u32), Option<Hash32>>,
    }

    impl Host for MockHost {
        fn build_block(&mut self, round: u32) -> Option<Block> {
            Some(mk_block(self.me as u8, round))
        }
        fn validate_block(&mut self, block: &Block) -> bool {
            !self.reject.contains(&block.hash())
        }
        fn may_sign(&mut self, v: &SignedVote) -> bool {
            match self.signed.get(&(v.kind, v.round)) {
                Some(prev) => *prev == v.value,
                None => {
                    self.signed.insert((v.kind, v.round), v.value);
                    true
                }
            }
        }
    }

    fn mk_block(tag: u8, round: u32) -> Block {
        Block {
            header: BlockHeader {
                height: 1,
                round,
                prev_hash: [0; 32],
                timestamp: 1,
                validator: Address::from_bytes([tag; 20]),
                tx_count: 0,
                state_root: [tag; 32],
                base_fee: 0,
                receipts_root: [0; 32],
                last_commit_hash: [0; 32],
            },
            transactions: vec![],
            last_commit: None,
        }
    }

    fn bls(i: usize) -> BlsSecretKey {
        let mut seed = [0u8; 32];
        seed[0] = i as u8 + 1;
        BlsSecretKey::from_bytes(&seed).unwrap()
    }

    /// A simulated network of `n` nodes. Messages are delivered in an order driven by a
    /// seeded PRNG; timeouts fire only when no message is left in flight.
    struct Net {
        engines: Vec<Engine>,
        hosts: Vec<MockHost>,
        queue: VecDeque<(usize, Input)>,
        /// Messages across the partition, delivered when it heals (delayed, not lost).
        held: Vec<(usize, Input)>,
        timers: BTreeMap<(u64, usize), (TimeoutKind, u32)>,
        now: u64,
        crashed: HashSet<usize>,
        /// Partition: `group[i]` — messages only flow within a group.
        group: Vec<u8>,
        commits: Vec<Option<(Hash32, CommitCert)>>,
        evidence: Vec<VoteEquivocation>,
        rng: u64,
    }

    impl Net {
        fn new(n: usize, seed: u64) -> Self {
            let addrs: Vec<Address> = (0..n)
                .map(|_| Address::from_public_key(&KeyPair::generate().public_key()))
                .collect();
            let vs = ValidatorSet::new(addrs.clone());
            let keys: Vec<Option<BlsPubKey>> = (0..n).map(|i| Some(bls(i).public_key())).collect();
            let engines = (0..n)
                .map(|i| {
                    Engine::new(HeightParams {
                        chain_id: CHAIN,
                        height: 1,
                        validators: vs.clone(),
                        keys: keys.clone(),
                        jailed: HashSet::new(),
                        me: Some((addrs[i], bls(i))),
                        timeouts: Timeouts::default(),
                    })
                })
                .collect();
            let hosts = (0..n)
                .map(|i| MockHost {
                    me: i,
                    reject: HashSet::new(),
                    signed: HashMap::new(),
                })
                .collect();
            Self {
                engines,
                hosts,
                queue: VecDeque::new(),
                held: Vec::new(),
                timers: BTreeMap::new(),
                now: 0,
                crashed: HashSet::new(),
                group: vec![0; n],
                commits: vec![None; n],
                evidence: vec![],
                rng: seed | 1,
            }
        }

        fn next_rand(&mut self) -> u64 {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            self.rng
        }

        fn dispatch(&mut self, from: usize, outs: Vec<Output>) {
            for o in outs {
                match o {
                    Output::BroadcastProposal(p) => {
                        for to in 0..self.engines.len() {
                            if to != from {
                                self.send(from, to, Input::Proposal(p.clone()));
                            }
                        }
                    }
                    Output::BroadcastVote(v) => {
                        for to in 0..self.engines.len() {
                            if to != from {
                                self.send(from, to, Input::Vote(v.clone()));
                            }
                        }
                    }
                    Output::ScheduleTimeout { kind, round, after } => {
                        let at = self.now + after.as_millis() as u64;
                        self.timers.insert((at, from), (kind, round));
                    }
                    Output::Commit { block, cert } => {
                        self.commits[from] = Some((block.hash(), cert.clone()));
                        for to in 0..self.engines.len() {
                            if to != from {
                                let input = Input::Committed {
                                    block: block.clone(),
                                    cert: cert.clone(),
                                };
                                self.send(from, to, input);
                            }
                        }
                    }
                    Output::Evidence(e) => self.evidence.push(e),
                }
            }
        }

        fn send(&mut self, from: usize, to: usize, input: Input) {
            if self.group[to] == self.group[from] {
                self.queue.push_back((to, input));
            } else {
                self.held.push((to, input));
            }
        }

        fn heal(&mut self) {
            self.group = vec![0; self.engines.len()];
            self.queue.extend(self.held.drain(..));
        }

        fn start(&mut self) {
            for i in 0..self.engines.len() {
                if self.crashed.contains(&i) {
                    continue;
                }
                let outs = self.engines[i].start(&mut self.hosts[i]);
                self.dispatch(i, outs);
            }
        }

        /// Runs until every live node committed or `max_ms` of simulated time elapsed.
        fn run(&mut self, max_ms: u64) {
            while self.now <= max_ms {
                if (0..self.engines.len())
                    .all(|i| self.crashed.contains(&i) || self.commits[i].is_some())
                {
                    return;
                }
                if !self.queue.is_empty() {
                    // Random delivery order.
                    let k = (self.next_rand() as usize) % self.queue.len();
                    let (to, input) = self.queue.remove(k).unwrap();
                    if self.crashed.contains(&to) {
                        continue;
                    }
                    let outs = self.engines[to].handle(input, &mut self.hosts[to]);
                    self.dispatch(to, outs);
                    continue;
                }
                let Some((&(at, node), &(kind, round))) = self.timers.iter().next() else {
                    return;
                };
                self.timers.remove(&(at, node));
                self.now = self.now.max(at);
                if self.crashed.contains(&node) {
                    continue;
                }
                let outs = self.engines[node]
                    .handle(Input::Timeout { kind, round }, &mut self.hosts[node]);
                self.dispatch(node, outs);
            }
        }

        fn committed_hashes(&self) -> HashSet<Hash32> {
            self.commits.iter().flatten().map(|(h, _)| *h).collect()
        }
    }

    fn assert_safe_and_live(net: &Net) {
        let hashes = net.committed_hashes();
        assert_eq!(hashes.len(), 1, "exactly one block committed: {hashes:?}");
        for i in 0..net.engines.len() {
            if !net.crashed.contains(&i) {
                assert!(net.commits[i].is_some(), "node {i} did not commit");
            }
        }
    }

    #[test]
    fn four_honest_nodes_commit_one_block_in_round_zero() {
        for seed in 1..40 {
            let mut net = Net::new(4, seed);
            net.start();
            net.run(60_000);
            assert_safe_and_live(&net);
            let (_, cert) = net.commits[0].clone().unwrap();
            assert_eq!(
                cert.round, 0,
                "seed {seed}: no timeout needed with all honest"
            );
        }
    }

    #[test]
    fn one_crashed_node_out_of_four_does_not_stop_the_chain() {
        for crashed in 0..4 {
            let mut net = Net::new(4, 7 + crashed as u64);
            net.crashed.insert(crashed);
            net.start();
            net.run(120_000);
            assert_safe_and_live(&net);
        }
    }

    #[test]
    fn crashed_proposer_costs_a_round_not_safety() {
        let mut net = Net::new(4, 3);
        let p0 = net.engines[0].proposer(0);
        let idx = net.engines[0].validators().index_of(&p0).unwrap();
        net.crashed.insert(idx);
        net.start();
        net.run(120_000);
        assert_safe_and_live(&net);
        let (_, cert) = net.commits.iter().flatten().next().unwrap().clone();
        assert!(
            cert.round >= 1,
            "the silent proposer's round is skipped by timeout"
        );
    }

    #[test]
    fn two_crashed_nodes_out_of_four_halt_without_committing() {
        let mut net = Net::new(4, 5);
        net.crashed.insert(1);
        net.crashed.insert(2);
        net.start();
        net.run(60_000);
        assert!(
            net.committed_hashes().is_empty(),
            "without > 2/3 of the power nothing may be committed"
        );
    }

    #[test]
    fn partition_two_two_commits_nothing_then_heals() {
        let mut net = Net::new(4, 11);
        net.group = vec![0, 0, 1, 1];
        net.start();
        net.run(30_000);
        assert!(net.committed_hashes().is_empty(), "no side has a quorum");
        // Heal: messages flow again; timers keep the rounds moving.
        net.heal();
        net.run(600_000);
        assert_safe_and_live(&net);
    }

    #[test]
    fn invalid_proposal_is_rejected_and_the_next_round_commits() {
        let mut net = Net::new(4, 13);
        // Every honest node refuses the round-0 proposer's block.
        let p0 = net.engines[0].proposer(0);
        let idx = net.engines[0].validators().index_of(&p0).unwrap();
        let bad = mk_block(idx as u8, 0).hash();
        for h in net.hosts.iter_mut() {
            h.reject.insert(bad);
        }
        net.start();
        net.run(120_000);
        assert_safe_and_live(&net);
        assert!(!net.committed_hashes().contains(&bad));
    }

    #[test]
    fn random_schedules_never_commit_two_blocks() {
        // Safety under adversarial scheduling: random delivery order, a crashed node and
        // a transient partition, over many seeds.
        for seed in 1..60u64 {
            let mut net = Net::new(4, seed * 7919);
            net.crashed.insert((seed % 4) as usize);
            net.group = vec![0, 1, 0, 1];
            net.start();
            net.run(5_000);
            net.heal();
            net.run(600_000);
            assert!(
                net.committed_hashes().len() <= 1,
                "seed {seed}: conflicting commits"
            );
            assert_safe_and_live(&net);
        }
    }

    #[test]
    fn double_vote_produces_evidence_and_is_not_counted_twice() {
        let mut net = Net::new(4, 17);
        net.start();
        // Node 3 turns byzantine: it sends a prevote for nil *and* for a fake block.
        let (addr, sk) = net.engines[3].p.me.clone().unwrap();
        for value in [None, Some([0xEE; 32])] {
            let mut v = SignedVote {
                kind: VoteKind::Prevote,
                height: 1,
                round: 0,
                value,
                validator: addr,
                signature: vec![],
            };
            v.signature = sk.sign(&v.sign_bytes(CHAIN)).0.to_vec();
            for to in 0..3 {
                net.queue.push_back((to, Input::Vote(v.clone())));
            }
        }
        net.crashed.insert(3); // it sends nothing else
        net.run(120_000);
        assert_safe_and_live(&net);
        assert!(!net.evidence.is_empty(), "equivocation must be reported");
        let ev = &net.evidence[0];
        assert!(ev.verify(CHAIN, &bls(3).public_key()));
    }

    #[test]
    fn forged_votes_and_proposals_are_ignored() {
        let mut net = Net::new(4, 19);
        // An outsider signs precommits for its own block with keys nobody registered.
        let outsider = mk_block(99, 0);
        for i in 0..4 {
            let mut v = SignedVote {
                kind: VoteKind::Precommit,
                height: 1,
                round: 0,
                value: Some(outsider.hash()),
                validator: net.engines[0].validators().validators()[i],
                signature: vec![],
            };
            v.signature = bls(50 + i).sign(&v.sign_bytes(CHAIN)).0.to_vec();
            for to in 0..4 {
                net.queue.push_back((to, Input::Vote(v.clone())));
            }
        }
        let mut fake = Proposal {
            height: 1,
            round: 0,
            pol_round: None,
            block: outsider.clone(),
            signature: vec![],
        };
        fake.signature = bls(60).sign(&fake.sign_bytes(CHAIN)).0.to_vec();
        for to in 0..4 {
            net.queue.push_back((to, Input::Proposal(fake.clone())));
        }
        net.start();
        net.run(120_000);
        assert_safe_and_live(&net);
        assert!(!net.committed_hashes().contains(&outsider.hash()));
    }

    /// The classic attack: the round-0 proposer is byzantine. It proposes block A to one
    /// honest node and block B to the two others, and prevotes/precommits A to the first
    /// and B to the others, in every round. With 1 byzantine out of 4, no two honest
    /// nodes may ever commit different blocks, whatever the delivery order.
    #[test]
    fn equivocating_proposer_never_splits_the_chain() {
        for seed in 1..80u64 {
            let mut net = Net::new(4, seed * 104_729);
            let byz_addr = net.engines[0].proposer(0);
            let byz = net.engines[0].validators().index_of(&byz_addr).unwrap();
            let honest: Vec<usize> = (0..4).filter(|i| *i != byz).collect();
            net.crashed.insert(byz); // its engine does not run; we script it
            let (addr, sk) = net.engines[byz].p.me.clone().unwrap();
            let a = mk_block(0xA0, 0);
            let b = mk_block(0xB0, 0);
            let targets = |x: &Block| -> Vec<usize> {
                if x.hash() == a.hash() {
                    vec![honest[0]]
                } else {
                    vec![honest[1], honest[2]]
                }
            };
            for blk in [&a, &b] {
                let mut prop = Proposal {
                    height: 1,
                    round: 0,
                    pol_round: None,
                    block: blk.clone(),
                    signature: vec![],
                };
                prop.signature = sk.sign(&prop.sign_bytes(CHAIN)).0.to_vec();
                for to in targets(blk) {
                    net.queue.push_back((to, Input::Proposal(prop.clone())));
                }
                for round in 0..6 {
                    for kind in [VoteKind::Prevote, VoteKind::Precommit] {
                        let mut v = SignedVote {
                            kind,
                            height: 1,
                            round,
                            value: Some(blk.hash()),
                            validator: addr,
                            signature: vec![],
                        };
                        v.signature = sk.sign(&v.sign_bytes(CHAIN)).0.to_vec();
                        for to in targets(blk) {
                            net.queue.push_back((to, Input::Vote(v.clone())));
                        }
                    }
                }
            }
            net.start();
            net.run(900_000);
            assert!(
                net.committed_hashes().len() <= 1,
                "seed {seed}: honest nodes committed different blocks"
            );
            assert_safe_and_live(&net);
        }
    }

    #[test]
    fn forged_committed_block_is_refused() {
        let mut net = Net::new(4, 23);
        let outsider = mk_block(77, 0);
        // Certificate aggregated from keys nobody registered.
        let sigs = (0..3)
            .map(|i| {
                let msg = vinx_core::consensus::vote_sign_bytes(
                    CHAIN,
                    VoteKind::Precommit,
                    1,
                    0,
                    Some(&outsider.hash()),
                );
                (i, bls(40 + i).sign(&msg))
            })
            .collect();
        let cert = CommitCert::from_precommits(1, 0, outsider.hash(), sigs).unwrap();
        for to in 0..4 {
            net.queue.push_back((
                to,
                Input::Committed {
                    block: outsider.clone(),
                    cert: cert.clone(),
                },
            ));
        }
        net.start();
        net.run(120_000);
        assert_safe_and_live(&net);
        assert!(!net.committed_hashes().contains(&outsider.hash()));
    }

    /// Scripted attack against **locking** (the reason Tendermint is safe across rounds).
    ///
    /// Validators v0, v2, v3 are honest; v1 is byzantine and proposes round 1.
    /// Round 0: v0 proposes A. v0 and v2 see a polka for A and lock on it; v0 then sees
    /// precommits of v2 and v1 and commits A. v3 never sees the polka and precommits nil.
    /// v2 and v3 move to round 1, where v1 proposes a fresh block B and prevotes it.
    /// Without locking, v2 and v3 would prevote B, and with v1 reach a polka and commit B —
    /// a second block at a height where v0 committed A. With locking, v2 prevotes nil and B
    /// can never gather a quorum.
    #[test]
    fn locking_prevents_a_conflicting_commit_in_a_later_round() {
        let mut net = Net::new(4, 1);
        let addrs = net.engines[0].validators().validators().to_vec();
        let p0 = net.engines[0]
            .validators()
            .index_of(&net.engines[0].proposer(0))
            .unwrap();
        let p1 = net.engines[0]
            .validators()
            .index_of(&net.engines[0].proposer(1))
            .unwrap();
        assert_ne!(p0, p1);
        let byz = p1;
        let others: Vec<usize> = (0..4).filter(|i| *i != p0 && *i != byz).collect();
        let (committer, locker, niler) = (p0, others[0], others[1]);
        let z_sk = bls(byz);

        let zvote = |kind, round, value: Option<Hash32>| {
            let mut v = SignedVote {
                kind,
                height: 1,
                round,
                value,
                validator: addrs[byz],
                signature: vec![],
            };
            v.signature = z_sk.sign(&v.sign_bytes(CHAIN)).0.to_vec();
            v
        };
        // Drives one node with one input; returns its outputs (not dispatched).
        fn step(net: &mut Net, i: usize, input: Input) -> Vec<Output> {
            net.engines[i].handle(input, &mut net.hosts[i])
        }
        fn votes(outs: &[Output], kind: VoteKind) -> Vec<SignedVote> {
            outs.iter()
                .filter_map(|o| match o {
                    Output::BroadcastVote(v) if v.kind == kind => Some(v.clone()),
                    _ => None,
                })
                .collect()
        }
        fn commits(outs: &[Output]) -> Vec<Hash32> {
            outs.iter()
                .filter_map(|o| match o {
                    Output::Commit { block, .. } => Some(block.hash()),
                    _ => None,
                })
                .collect()
        }

        // ── Round 0 ──
        let mut all_outs: Vec<Vec<Output>> = vec![vec![]; 4];
        for i in [committer, locker, niler] {
            let o = net.engines[i].start(&mut net.hosts[i]);
            all_outs[i].extend(o);
        }
        let prop_a = all_outs[committer]
            .iter()
            .find_map(|o| match o {
                Output::BroadcastProposal(p) => Some(p.clone()),
                _ => None,
            })
            .unwrap();
        let a = prop_a.block.hash();
        let pv_locker = votes(
            &step(&mut net, locker, Input::Proposal(prop_a.clone())),
            VoteKind::Prevote,
        );
        let _pv_niler = votes(
            &step(&mut net, niler, Input::Proposal(prop_a.clone())),
            VoteKind::Prevote,
        );
        let pv_committer = votes(&all_outs[committer], VoteKind::Prevote);
        assert_eq!(pv_committer[0].value, Some(a));

        // committer: polka (itself, locker, byz) → precommit A.
        step(&mut net, committer, Input::Vote(pv_locker[0].clone()));
        let o = step(
            &mut net,
            committer,
            Input::Vote(zvote(VoteKind::Prevote, 0, Some(a))),
        );
        let pc_committer = votes(&o, VoteKind::Precommit);
        assert_eq!(pc_committer[0].value, Some(a));
        // locker: polka (itself, committer, byz) → precommit A (locks A).
        step(&mut net, locker, Input::Vote(pv_committer[0].clone()));
        let o = step(
            &mut net,
            locker,
            Input::Vote(zvote(VoteKind::Prevote, 0, Some(a))),
        );
        let pc_locker = votes(&o, VoteKind::Precommit);
        assert_eq!(pc_locker[0].value, Some(a));
        // niler: sees A from committer and nil from byz → no polka; prevote timeout → nil.
        step(&mut net, niler, Input::Vote(pv_committer[0].clone()));
        step(
            &mut net,
            niler,
            Input::Vote(zvote(VoteKind::Prevote, 0, None)),
        );
        let o = step(
            &mut net,
            niler,
            Input::Timeout {
                kind: TimeoutKind::Prevote,
                round: 0,
            },
        );
        let pc_niler = votes(&o, VoteKind::Precommit);
        assert_eq!(pc_niler[0].value, None);

        // committer: precommits A from locker and byz → commits A.
        step(&mut net, committer, Input::Vote(pc_locker[0].clone()));
        let o = step(
            &mut net,
            committer,
            Input::Vote(zvote(VoteKind::Precommit, 0, Some(a))),
        );
        assert_eq!(commits(&o), vec![a], "the committer decides A in round 0");

        // locker and niler move to round 1 (byz tells them it precommitted nil).
        for (i, other) in [(locker, pc_niler[0].clone()), (niler, pc_locker[0].clone())] {
            step(&mut net, i, Input::Vote(other));
            step(
                &mut net,
                i,
                Input::Vote(zvote(VoteKind::Precommit, 0, None)),
            );
            step(
                &mut net,
                i,
                Input::Timeout {
                    kind: TimeoutKind::Precommit,
                    round: 0,
                },
            );
            assert_eq!(net.engines[i].round(), 1);
        }

        // ── Round 1: byz proposes B ──
        let b = mk_block(0xBB, 1);
        let mut prop_b = Proposal {
            height: 1,
            round: 1,
            pol_round: None,
            block: b.clone(),
            signature: vec![],
        };
        prop_b.signature = z_sk.sign(&prop_b.sign_bytes(CHAIN)).0.to_vec();
        let pv1_locker = votes(
            &step(&mut net, locker, Input::Proposal(prop_b.clone())),
            VoteKind::Prevote,
        );
        let pv1_niler = votes(
            &step(&mut net, niler, Input::Proposal(prop_b.clone())),
            VoteKind::Prevote,
        );
        assert_eq!(
            pv1_locker[0].value, None,
            "a validator locked on A must not prevote B"
        );
        assert_eq!(pv1_niler[0].value, Some(b.hash()));

        // Everyone hears everything of round 1, plus byz's votes for B.
        let mut decided = vec![];
        for (i, others) in [
            (locker, pv1_niler[0].clone()),
            (niler, pv1_locker[0].clone()),
        ] {
            let mut o = step(&mut net, i, Input::Vote(others));
            o.extend(step(
                &mut net,
                i,
                Input::Vote(zvote(VoteKind::Prevote, 1, Some(b.hash()))),
            ));
            o.extend(step(
                &mut net,
                i,
                Input::Vote(zvote(VoteKind::Precommit, 1, Some(b.hash()))),
            ));
            decided.extend(commits(&o));
        }
        assert!(
            !decided.contains(&b.hash()),
            "B must never be committed at a height where A was committed"
        );
    }

    #[test]
    fn single_validator_commits_alone() {
        let mut net = Net::new(1, 1);
        net.start();
        net.run(1_000);
        assert_safe_and_live(&net);
    }
}
