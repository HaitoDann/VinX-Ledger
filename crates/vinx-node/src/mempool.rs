use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
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

    /// Returns up to `limit` transactions ordered by fee (highest first), with
    /// nonce order preserved within each sender account.
    ///
    /// Algorithm: maintain a max-heap keyed on the fee of each account's
    /// lowest-nonce (ready) tx.  On each step, pop the highest-fee head, emit
    /// it, then push the next head from the same account if one exists.
    pub fn drain(&mut self, limit: usize) -> Vec<Transaction> {
        let mut result = Vec::with_capacity(limit);

        // (fee_atoms, addr) — BinaryHeap is a max-heap, so highest fee first.
        // For equal fees the addr tiebreaks deterministically.
        let mut heap: BinaryHeap<(u128, String)> = self
            .queues
            .iter()
            .filter_map(|(addr, queue)| {
                queue.values().next().map(|tx| (tx.fee.atoms(), addr.clone()))
            })
            .collect();

        while result.len() < limit {
            let Some((_, addr)) = heap.pop() else { break };

            let queue = match self.queues.get_mut(&addr) {
                Some(q) if !q.is_empty() => q,
                _ => continue,
            };

            let (&nonce, _) = queue.iter().next().unwrap();
            let tx = queue.remove(&nonce).unwrap();
            self.seen.remove(&tx.hash());
            self.pending_count -= 1;

            // Push next head from the same account if available
            if let Some(next_tx) = queue.values().next() {
                heap.push((next_tx.fee.atoms(), addr.clone()));
            }

            result.push(tx);
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

    fn make_tx_with_fee(kp: &KeyPair, to: Address, nonce: u64, fee_vinx: u64) -> Transaction {
        let amount = Amount::from_vinx(1);
        let fee = Amount::from_vinx(fee_vinx);
        vinx_core::Transaction::new_transfer(kp, to, amount, fee, nonce)
    }

    #[test]
    fn test_fee_priority_higher_fee_first() {
        let mut mp = Mempool::new(10);
        let kp_a = KeyPair::generate();
        let kp_b = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());

        // A pays fee 1, B pays fee 10 — B should come first
        mp.add(make_tx_with_fee(&kp_a, to.clone(), 0, 1)).unwrap();
        mp.add(make_tx_with_fee(&kp_b, to.clone(), 0, 10)).unwrap();

        let drained = mp.drain(10);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].fee, Amount::from_vinx(10)); // B first
        assert_eq!(drained[1].fee, Amount::from_vinx(1));  // A second
    }

    #[test]
    fn test_fee_priority_nonce_order_preserved() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());

        // Same sender, nonces 0 and 1 with different fees
        // Nonce order must be respected regardless of fee
        mp.add(make_tx_with_fee(&kp, to.clone(), 0, 5)).unwrap();
        mp.add(make_tx_with_fee(&kp, to.clone(), 1, 100)).unwrap(); // higher fee but nonce=1

        let drained = mp.drain(10);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].nonce, 0); // nonce 0 must come first
        assert_eq!(drained[1].nonce, 1);
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
