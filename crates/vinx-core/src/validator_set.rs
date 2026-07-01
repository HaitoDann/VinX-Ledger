use serde::{Deserialize, Serialize};
use vinx_crypto::Address;

/// Ordered set of authorized PoA validators.
///
/// Ordering determines the deterministic round-robin leader rotation:
/// `leader_at(height) = validators[height % len]`.
///
/// Quorum = ceil(2n/3), which ensures >66% of the set must co-sign every block.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidatorSet {
    validators: Vec<Address>,
}

impl ValidatorSet {
    pub fn new(validators: Vec<Address>) -> Self {
        assert!(!validators.is_empty(), "validator set cannot be empty");
        Self { validators }
    }

    pub fn single(validator: Address) -> Self {
        Self::new(vec![validator])
    }

    /// Round-robin leader for `height`.
    pub fn leader_at(&self, height: u64) -> &Address {
        &self.validators[(height as usize) % self.validators.len()]
    }

    /// Minimum signatures required to finalize a block: ceil(2n/3).
    ///
    /// | n  | quorum | %     |
    /// |----|--------|-------|
    /// |  1 |   1    | 100%  |
    /// |  3 |   2    |  67%  |
    /// |  5 |   4    |  80%  |
    /// |  9 |   6    |  67%  |
    /// | 21 |  14    |  67%  |
    pub fn quorum(&self) -> usize {
        let n = self.validators.len();
        (2 * n).div_ceil(3)
    }

    pub fn contains(&self, addr: &Address) -> bool {
        self.validators.contains(addr)
    }

    pub fn validators(&self) -> &[Address] {
        &self.validators
    }

    pub fn len(&self) -> usize {
        self.validators.len()
    }

    pub fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }

    /// Appends a new validator. Returns false if the address is already present.
    pub fn add(&mut self, addr: Address) -> bool {
        if self.validators.contains(&addr) {
            return false;
        }
        self.validators.push(addr);
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
            true
        } else {
            false
        }
    }
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
        let vs = ValidatorSet::single(a.clone());
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
