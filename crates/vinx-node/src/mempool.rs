use std::collections::HashSet;
use vinx_core::Transaction;
use vinx_crypto::Hash32;

const DEFAULT_MAX_SIZE: usize = 10_000;

pub struct Mempool {
    pending: Vec<Transaction>,
    seen: HashSet<Hash32>, // prevents double-submission
    max_size: usize,
}

impl Default for Mempool {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_SIZE)
    }
}

impl Mempool {
    pub fn new(max_size: usize) -> Self {
        Self {
            pending: Vec::new(),
            seen: HashSet::new(),
            max_size,
        }
    }

    pub fn add(&mut self, tx: Transaction) -> Result<(), MempoolError> {
        if self.pending.len() >= self.max_size {
            return Err(MempoolError::Full);
        }
        let hash = tx.hash();
        if !self.seen.insert(hash) {
            return Err(MempoolError::Duplicate);
        }
        self.pending.push(tx);
        Ok(())
    }

    /// Removes and returns up to `limit` transactions in FIFO order.
    pub fn drain(&mut self, limit: usize) -> Vec<Transaction> {
        let count = limit.min(self.pending.len());
        let drained: Vec<Transaction> = self.pending.drain(..count).collect();
        for tx in &drained {
            self.seen.remove(&tx.hash());
        }
        drained
    }

    pub fn size(&self) -> usize {
        self.pending.len()
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MempoolError {
    #[error("Mempool is full")]
    Full,
    #[error("Transaction already submitted")]
    Duplicate,
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS};
    use vinx_crypto::{Address, KeyPair};

    fn make_tx(kp: &KeyPair, to: Address, nonce: u64) -> Transaction {
        let amount = Amount::from_vinx(1);
        let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));
        vinx_core::Transaction::new_transfer(kp, to, amount, fee, nonce)
    }

    #[test]
    fn test_add_and_drain() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());
        mp.add(make_tx(&kp, to.clone(), 0)).unwrap();
        mp.add(make_tx(&kp, to, 1)).unwrap();
        assert_eq!(mp.size(), 2);
        let drained = mp.drain(1);
        assert_eq!(drained.len(), 1);
        assert_eq!(mp.size(), 1);
    }

    #[test]
    fn test_duplicate_rejected() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());
        let tx = make_tx(&kp, to, 0);
        mp.add(tx.clone()).unwrap();
        assert_eq!(mp.add(tx), Err(MempoolError::Duplicate));
    }

    #[test]
    fn test_full_rejected() {
        let mut mp = Mempool::new(1);
        let kp = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());
        mp.add(make_tx(&kp, to.clone(), 0)).unwrap();
        assert_eq!(mp.add(make_tx(&kp, to, 1)), Err(MempoolError::Full));
    }

    #[test]
    fn test_drain_all() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());
        for i in 0..5 {
            mp.add(make_tx(&kp, to.clone(), i)).unwrap();
        }
        let drained = mp.drain(100);
        assert_eq!(drained.len(), 5);
        assert_eq!(mp.size(), 0);
    }
}
