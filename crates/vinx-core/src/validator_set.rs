use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use vinx_crypto::Address;

/// Maximum share of the total voting power a single validator may hold, in basis points
/// (ADR 0081 C4): 10 %. Below ten validators the cap cannot be met, so it relaxes to an
/// equal share (`1/n`) — nobody ever holds more than `max(10 %, 1/n)` of the votes.
pub const MAX_VOTING_POWER_SHARE_BPS: u64 = 1_000;

/// Ordered, weighted set of active validators (ADR 0081 C4).
///
/// Each validator has a **voting power** derived from its bond and capped at
/// [`MAX_VOTING_POWER_SHARE_BPS`] of the total. Quorum is measured in power, not in
/// heads: a decision needs **strictly more than 2/3** of the total power (the standard
/// BFT bound, safe while faulty power stays below 1/3).
///
/// The set also carries **proposer priorities** (the Tendermint proposer-selection
/// algorithm): each step every validator gains its power, the highest priority proposes
/// and pays back the total. Over time each validator proposes in proportion to its power,
/// deterministically, with no randomness to grind.
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ValidatorSet {
    validators: Vec<Address>,
    /// Voting power of `validators[i]`, parallel vector. Always ≥ 1.
    powers: Vec<u64>,
    /// Proposer priority of `validators[i]`, parallel vector (committed with the set).
    priorities: Vec<i128>,
}

impl ValidatorSet {
    /// Equal-power set (power 1 each). Used for genesis and tests.
    pub fn new(validators: Vec<Address>) -> Self {
        let powers = vec![1; validators.len()];
        Self::with_powers(validators, powers)
    }

    /// Builds a set from raw stakes (any unit), applying the voting-power cap.
    pub fn with_stakes(validators: Vec<Address>, stakes: Vec<u64>) -> Self {
        let powers = cap_powers(&stakes);
        Self::with_powers(validators, powers)
    }

    fn with_powers(validators: Vec<Address>, powers: Vec<u64>) -> Self {
        assert!(!validators.is_empty(), "validator set cannot be empty");
        assert_eq!(validators.len(), powers.len());
        let powers = powers.into_iter().map(|p| p.max(1)).collect::<Vec<_>>();
        let priorities = vec![0; validators.len()];
        Self {
            validators,
            powers,
            priorities,
        }
    }

    pub fn single(validator: Address) -> Self {
        Self::new(vec![validator])
    }

    /// Round-robin leader for `height` (legacy schedule, unweighted).
    pub fn leader_at(&self, height: u64) -> &Address {
        &self.validators[(height as usize) % self.validators.len()]
    }

    /// Legacy head-count quorum `⌈2n/3⌉`. Superseded by [`Self::quorum_power`].
    pub fn quorum(&self) -> usize {
        let n = self.validators.len();
        (2 * n).div_ceil(3)
    }

    /// Voting power of the validator at `idx`.
    pub fn power_at(&self, idx: usize) -> u64 {
        self.powers.get(idx).copied().unwrap_or(0)
    }

    /// Voting power of `addr`, 0 when not in the set.
    pub fn power_of(&self, addr: &Address) -> u64 {
        self.index_of(addr).map(|i| self.powers[i]).unwrap_or(0)
    }

    /// Sum of all voting powers.
    pub fn total_power(&self) -> u64 {
        self.powers.iter().sum()
    }

    /// Minimum power for a decision: strictly more than 2/3 of the total.
    pub fn quorum_power(&self) -> u64 {
        self.total_power() * 2 / 3 + 1
    }

    /// Minimum power that is guaranteed to include an honest validator: strictly more
    /// than 1/3 of the total (Tendermint's `f + 1`).
    pub fn one_third_plus(&self) -> u64 {
        self.total_power() / 3 + 1
    }

    /// Total power of the validators at `indices` (duplicates and out-of-range ignored).
    pub fn power_of_indices(&self, indices: impl IntoIterator<Item = usize>) -> u64 {
        let mut seen = vec![false; self.validators.len()];
        let mut total = 0;
        for i in indices {
            if i < seen.len() && !seen[i] {
                seen[i] = true;
                total += self.powers[i];
            }
        }
        total
    }

    /// Proposer index for round `round` of the next height. Validators for which
    /// `eligible` returns false (e.g. jailed) are skipped; if none is eligible, everyone is.
    ///
    /// Pure: runs the priority algorithm on a copy, `round + 1` steps from the committed
    /// priorities. Call [`Self::advance_proposer_priority`] once per committed block.
    pub fn proposer_index(&self, round: u32, eligible: impl Fn(&Address) -> bool) -> usize {
        let mask: Vec<bool> = self.validators.iter().map(&eligible).collect();
        let mask = if mask.iter().any(|m| *m) {
            mask
        } else {
            vec![true; self.validators.len()]
        };
        let mut prio = self.priorities.clone();
        let mut chosen = 0;
        for _ in 0..=round {
            chosen = self.step(&mut prio, &mask);
        }
        chosen
    }

    /// Proposer address for round `round` of the next height (see [`Self::proposer_index`]).
    pub fn proposer(&self, round: u32, eligible: impl Fn(&Address) -> bool) -> &Address {
        &self.validators[self.proposer_index(round, eligible)]
    }

    /// Commits one proposer-priority step (the round-0 step of the height just committed).
    pub fn advance_proposer_priority(&mut self, eligible: impl Fn(&Address) -> bool) {
        let mask: Vec<bool> = self.validators.iter().map(&eligible).collect();
        let mask = if mask.iter().any(|m| *m) {
            mask
        } else {
            vec![true; self.validators.len()]
        };
        let mut prio = std::mem::take(&mut self.priorities);
        self.step(&mut prio, &mask);
        self.priorities = prio;
    }

    /// One step of the proposer algorithm: every eligible validator gains its power, the
    /// highest priority (lowest index on ties) is chosen and pays back the eligible total.
    fn step(&self, prio: &mut [i128], mask: &[bool]) -> usize {
        let total: i128 = self
            .powers
            .iter()
            .zip(mask)
            .filter(|(_, m)| **m)
            .map(|(p, _)| *p as i128)
            .sum();
        let mut best: Option<usize> = None;
        for i in 0..self.validators.len() {
            if !mask[i] {
                continue;
            }
            prio[i] += self.powers[i] as i128;
            if best.is_none_or(|b| prio[i] > prio[b]) {
                best = Some(i);
            }
        }
        let b = best.expect("at least one eligible validator");
        prio[b] -= total;
        b
    }

    pub fn contains(&self, addr: &Address) -> bool {
        self.validators.contains(addr)
    }

    pub fn validators(&self) -> &[Address] {
        &self.validators
    }

    pub fn powers(&self) -> &[u64] {
        &self.powers
    }

    pub fn len(&self) -> usize {
        self.validators.len()
    }

    pub fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }

    /// Appends a new validator with power 1. Returns false if already present. Callers
    /// that know stakes rebuild the set with [`Self::with_stakes`] instead.
    pub fn add(&mut self, addr: Address) -> bool {
        if self.validators.contains(&addr) {
            return false;
        }
        self.validators.push(addr);
        self.powers.push(1);
        self.priorities.push(0);
        true
    }

    /// Returns the index of `addr` in the validator list, or None if not present.
    pub fn index_of(&self, addr: &Address) -> Option<usize> {
        self.validators.iter().position(|a| a == addr)
    }

    /// Round-robin leader index for `height`.
    pub fn leader_idx_at(&self, height: u64) -> usize {
        (height as usize) % self.validators.len()
    }

    /// Removes a validator by address. Returns false if not found.
    /// Will not remove the last validator (preserves the invariant of non-empty set).
    pub fn remove(&mut self, addr: &Address) -> bool {
        if self.validators.len() <= 1 {
            return false;
        }
        if let Some(pos) = self.validators.iter().position(|a| a == addr) {
            self.validators.remove(pos);
            self.powers.remove(pos);
            self.priorities.remove(pos);
            true
        } else {
            false
        }
    }
}

/// Applies the voting-power cap (ADR 0081 C4) by water-filling: the cap `c` is the fixed
/// point of `c = share × Σ min(stake, c)`, with `share = max(10 %, 1/n)`. Pure integer
/// arithmetic, deterministic. Every resulting power is ≥ 1.
pub fn cap_powers(stakes: &[u64]) -> Vec<u64> {
    let n = stakes.len() as u128;
    if n == 0 {
        return vec![];
    }
    let stakes: Vec<u128> = stakes.iter().map(|s| (*s).max(1) as u128).collect();
    // share = num / den = max(MAX_SHARE_BPS / 10_000, 1 / n)
    let (num, den) = if n * MAX_VOTING_POWER_SHARE_BPS as u128 >= 10_000 {
        (MAX_VOTING_POWER_SHARE_BPS as u128, 10_000u128)
    } else {
        (1, n)
    };
    let mut cap: u128 = stakes.iter().sum::<u128>() * num / den;
    loop {
        let capped: u128 = stakes.iter().map(|s| (*s).min(cap)).sum();
        let next = (capped * num / den).max(1);
        if next >= cap {
            break;
        }
        cap = next;
    }
    stakes
        .iter()
        .map(|s| (*s).min(cap).max(1).min(u64::MAX as u128) as u64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn addr() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    fn addrs(n: usize) -> Vec<Address> {
        (0..n).map(|_| addr()).collect()
    }

    #[test]
    fn test_quorum_power_is_strictly_more_than_two_thirds() {
        for (n, q) in [(1, 1), (3, 3), (4, 3), (7, 5), (10, 7), (100, 67)] {
            let vs = ValidatorSet::new(addrs(n));
            assert_eq!(vs.quorum_power(), q, "n = {n}");
            assert!(3 * vs.quorum_power() > 2 * vs.total_power());
        }
    }

    #[test]
    fn test_power_cap_ten_percent() {
        // One whale and many small validators: the whale is cut to ≤ 10 % of the total.
        let mut stakes = vec![1_000u64; 20];
        stakes.push(1_000_000);
        let powers = cap_powers(&stakes);
        let total: u64 = powers.iter().sum();
        let whale = *powers.last().unwrap();
        assert!(whale * 10 <= total + 10, "whale {whale} of {total}");
        assert!(
            powers[..20].iter().all(|p| *p == 1_000),
            "small stakes untouched"
        );
    }

    #[test]
    fn test_power_cap_relaxes_to_equal_share_below_ten() {
        // n = 4 → cap = 1/4: a whale cannot exceed an equal share.
        let powers = cap_powers(&[10, 10, 10, 1_000]);
        let total: u64 = powers.iter().sum();
        assert!(powers.iter().all(|p| p * 4 <= total + 4), "{powers:?}");
        // Equal stakes stay equal.
        assert_eq!(cap_powers(&[5, 5, 5]), vec![5, 5, 5]);
    }

    #[test]
    fn test_proposer_frequency_follows_power() {
        // 11 validators of stake 10 and one of stake 12 (under the 10 % cap): over one
        // full cycle of 10 × total steps, each proposes exactly 10 × its power.
        let a = addrs(12);
        let mut stakes = vec![10u64; 11];
        stakes.push(12);
        let mut vs = ValidatorSet::with_stakes(a, stakes);
        assert_eq!(vs.total_power(), 122);
        let mut counts = [0usize; 12];
        for _ in 0..1_220 {
            counts[vs.proposer_index(0, |_| true)] += 1;
            vs.advance_proposer_priority(|_| true);
        }
        assert!(counts[..11].iter().all(|c| *c == 100), "{counts:?}");
        assert_eq!(counts[11], 120);
    }

    #[test]
    fn test_proposer_rounds_rotate_and_skip_ineligible() {
        let a = addrs(3);
        let vs = ValidatorSet::new(a.clone());
        let r0 = vs.proposer_index(0, |_| true);
        let r1 = vs.proposer_index(1, |_| true);
        assert_ne!(r0, r1, "the next round has another proposer");
        let jailed = a[r0];
        let p = vs.proposer(0, |x| *x != jailed);
        assert_ne!(*p, jailed, "an ineligible validator never proposes");
    }

    #[test]
    fn test_quorum_single() {
        assert_eq!(ValidatorSet::new(addrs(1)).quorum(), 1);
    }

    #[test]
    fn test_quorum_three() {
        assert_eq!(ValidatorSet::new(addrs(3)).quorum(), 2);
    }

    #[test]
    fn test_quorum_five() {
        assert_eq!(ValidatorSet::new(addrs(5)).quorum(), 4);
    }

    #[test]
    fn test_quorum_nine() {
        assert_eq!(ValidatorSet::new(addrs(9)).quorum(), 6);
    }

    #[test]
    fn test_quorum_twentyone() {
        assert_eq!(ValidatorSet::new(addrs(21)).quorum(), 14);
    }

    #[test]
    fn test_leader_rotation_round_robin() {
        let addresses = addrs(3);
        let vs = ValidatorSet::new(addresses.clone());
        assert_eq!(vs.leader_at(0), &addresses[0]);
        assert_eq!(vs.leader_at(1), &addresses[1]);
        assert_eq!(vs.leader_at(2), &addresses[2]);
        assert_eq!(vs.leader_at(3), &addresses[0]);
        assert_eq!(vs.leader_at(100), &addresses[1]); // 100 % 3 = 1
    }

    #[test]
    fn test_single_validator_always_leader() {
        let a = addr();
        let vs = ValidatorSet::single(a);
        for h in [0, 1, 99, 1000] {
            assert_eq!(vs.leader_at(h), &a);
        }
    }

    #[test]
    fn test_contains() {
        let addresses = addrs(2);
        let vs = ValidatorSet::new(addresses.clone());
        assert!(vs.contains(&addresses[0]));
        assert!(vs.contains(&addresses[1]));
        assert!(!vs.contains(&addr()));
    }
}
