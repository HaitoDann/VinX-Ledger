use std::collections::{BTreeMap, HashMap, HashSet};
use vinx_core::{CoreError, Transaction};
use vinx_crypto::Hash32;

const DEFAULT_MAX_SIZE: usize = 10_000;

/// Per-account nonce-ordered transaction queue.
/// Each account's transactions are sorted by nonce; drain always pulls the
/// lowest available nonce for each sender, then interleaves across senders.
pub struct Mempool {
    /// addr_str → sorted(nonce → tx)
    queues: HashMap<String, BTreeMap<u64, Transaction>>,
    /// All known tx hashes — prevents double-submission.
    seen: HashSet<Hash32>,
    pending_count: usize,
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
            queues: HashMap::new(),
            seen: HashSet::new(),
            pending_count: 0,
            max_size,
        }
    }

    /// Inserts a transaction into the per-account nonce queue.
    pub fn add(&mut self, tx: Transaction) -> Result<(), MempoolError> {
        if self.pending_count >= self.max_size {
            return Err(MempoolError::Full);
        }
        let hash = tx.hash();
        if !self.seen.insert(hash) {
            return Err(MempoolError::Duplicate);
        }
        let addr = tx.from.to_string();
        self.queues.entry(addr).or_default().insert(tx.nonce, tx);
        self.pending_count += 1;
        Ok(())
    }

    /// Returns up to `limit` transactions in per-account nonce order.
    /// For each account, nonces are emitted lowest-first; accounts are
    /// interleaved in round-robin fashion.
    pub fn drain(&mut self, limit: usize) -> Vec<Transaction> {
        let mut result = Vec::with_capacity(limit);

        // Snapshot the address list to drive the round-robin.
        let addrs: Vec<String> = self.queues.keys().cloned().collect();

        let mut made_progress = true;
        while made_progress && result.len() < limit {
            made_progress = false;
            for addr in &addrs {
                if result.len() >= limit {
                    break;
                }
                if let Some(queue) = self.queues.get_mut(addr) {
                    if let Some((&nonce, _)) = queue.iter().next() {
                        let tx = queue.remove(&nonce).unwrap();
                        self.seen.remove(&tx.hash());
                        self.pending_count -= 1;
                        result.push(tx);
                        made_progress = true;
                    }
                }
            }
        }

        self.queues.retain(|_, q| !q.is_empty());
        result
    }

    /// Re-inserts transactions that were dequeued for a block but could not be
    /// applied yet (e.g. future-nonce relative to what the state accepted).
    /// Silently skips duplicates already present in the queue.
    pub fn requeue(&mut self, txs: Vec<Transaction>) {
        for tx in txs {
            let hash = tx.hash();
            if self.seen.contains(&hash) {
                continue; // already queued
            }
            if self.pending_count >= self.max_size {
                tracing::warn!("Mempool full — dropping requeued tx {}", hex::encode(hash));
                break;
            }
            self.seen.insert(hash);
            self.queues
                .entry(tx.from.to_string())
                .or_default()
                .insert(tx.nonce, tx);
            self.pending_count += 1;
        }
    }

    pub fn size(&self) -> usize {
        self.pending_count
    }
}

/// Returns `true` if this `CoreError` indicates the transaction nonce is in
/// the future (tx.nonce > expected), meaning the tx may succeed once
/// predecessor transactions are processed.
pub fn is_future_nonce(err: &CoreError) -> bool {
    matches!(err, CoreError::InvalidNonce { expected, got } if *got > *expected)
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

    #[test]
    fn test_nonce_order_preserved_when_inserted_out_of_order() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());
        // Insert nonces 2, 0, 1 out of order
        mp.add(make_tx(&kp, to.clone(), 2)).unwrap();
        mp.add(make_tx(&kp, to.clone(), 0)).unwrap();
        mp.add(make_tx(&kp, to.clone(), 1)).unwrap();

        let drained = mp.drain(10);
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].nonce, 0);
        assert_eq!(drained[1].nonce, 1);
        assert_eq!(drained[2].nonce, 2);
    }

    #[test]
    fn test_requeue_puts_tx_back() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());
        let tx = make_tx(&kp, to, 5);
        mp.add(tx.clone()).unwrap();
        let drained = mp.drain(10);
        assert_eq!(mp.size(), 0);

        mp.requeue(drained);
        assert_eq!(mp.size(), 1);
    }

    #[test]
    fn test_requeue_skips_duplicate() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());
        let tx = make_tx(&kp, to, 0);
        mp.add(tx.clone()).unwrap();
        mp.requeue(vec![tx]); // already in queue
        assert_eq!(mp.size(), 1);
    }

    #[test]
    fn test_multi_account_interleaving() {
        let mut mp = Mempool::new(20);
        let kp_a = KeyPair::generate();
        let kp_b = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());

        // A has nonces 0,1 — B has nonces 0,1
        mp.add(make_tx(&kp_a, to.clone(), 0)).unwrap();
        mp.add(make_tx(&kp_a, to.clone(), 1)).unwrap();
        mp.add(make_tx(&kp_b, to.clone(), 0)).unwrap();
        mp.add(make_tx(&kp_b, to.clone(), 1)).unwrap();

        let drained = mp.drain(10);
        assert_eq!(drained.len(), 4);
        assert_eq!(mp.size(), 0);
    }
}
