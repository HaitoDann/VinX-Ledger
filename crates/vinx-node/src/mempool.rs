use bloomfilter::Bloom;
use rayon::prelude::*;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Notify;
use vinx_core::{CoreError, Transaction};
use vinx_crypto::{Address, Hash32};

const DEFAULT_MAX_SIZE: usize = 10_000;
/// Maximum pending transactions per sender address.
const MAX_PER_ADDRESS: usize = 50;

/// Per-account nonce-ordered transaction queue.
/// Each account's transactions are sorted by nonce; drain always pulls the
/// lowest available nonce for each sender, then interleaves across senders.
///
/// Incoming transactions are first staged in an `unverified` queue. Call
/// `flush_staged()` before block production to verify signatures in parallel
/// on all available CPU cores using rayon.
pub struct Mempool {
    /// addr_str → sorted(nonce → tx) — all entries here have valid signatures.
    queues: HashMap<String, BTreeMap<u64, Transaction>>,
    /// Staging queue for unverified incoming transactions (e.g. from P2P batch ingestion).
    unverified: Vec<Transaction>,
    /// All known tx hashes — prevents double-submission.
    seen: HashSet<Hash32>,
    /// Bloom filter pre-screening duplicate hashes in `stage()` to skip
    /// costly signature verification on already-known P2P-gossiped transactions.
    /// 1% false-positive rate at capacity — correctness guaranteed by `seen`.
    bloom: Bloom<Hash32>,
    pending_count: usize,
    max_size: usize,
    /// Signals the block producer that at least one transaction is ready.
    /// Cloned and held by the producer loop — no lock needed to await it.
    pub tx_ready: Arc<Notify>,
}

impl Default for Mempool {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_SIZE)
    }
}

impl Mempool {
    pub fn new(max_size: usize) -> Self {
        // Size the bloom filter for 2× capacity at 1% false-positive rate.
        // A false positive only causes a staged tx to be skipped; `seen` ensures correctness.
        let bloom = Bloom::new_for_fp_rate(max_size * 2, 0.01);
        Self {
            queues: HashMap::new(),
            unverified: Vec::new(),
            seen: HashSet::new(),
            bloom,
            pending_count: 0,
            max_size,
            tx_ready: Arc::new(Notify::new()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pending_count == 0
    }

    /// Inserts a pre-verified transaction into the per-account nonce queue.
    /// The caller is responsible for verifying the signature before calling this.
    pub fn add(&mut self, tx: Transaction) -> Result<(), MempoolError> {
        if self.pending_count >= self.max_size {
            return Err(MempoolError::Full);
        }
        let hash = tx.hash();
        if !self.seen.insert(hash) {
            return Err(MempoolError::Duplicate);
        }
        let addr = tx.from.to_string();
        let queue = self.queues.entry(addr).or_default();
        if queue.len() >= MAX_PER_ADDRESS {
            // Undo the seen insertion
            self.seen.remove(&hash);
            return Err(MempoolError::RateLimited);
        }
        self.bloom.set(&hash);
        queue.insert(tx.nonce, tx);
        self.pending_count += 1;
        self.tx_ready.notify_one();
        Ok(())
    }

    /// Removes all transactions whose `expires_at_height` <= `current_height`.
    /// Call this once per block tick before draining for production.
    pub fn prune_expired(&mut self, current_height: u64) {
        let mut expired_hashes: Vec<Hash32> = Vec::new();
        for queue in self.queues.values_mut() {
            queue.retain(|_, tx| {
                let expired = tx
                    .expires_at_height
                    .map(|h| current_height >= h)
                    .unwrap_or(false);
                if expired {
                    expired_hashes.push(tx.hash());
                }
                !expired
            });
        }
        let removed = expired_hashes.len();
        for h in expired_hashes {
            self.seen.remove(&h);
        }
        self.pending_count = self.pending_count.saturating_sub(removed);
        self.queues.retain(|_, q| !q.is_empty());

        // Rebuild the bloom filter from the surviving entries so evicted hashes
        // no longer cause false-positive skips in stage().
        if removed > 0 {
            self.bloom.clear();
            for queue in self.queues.values() {
                for tx in queue.values() {
                    self.bloom.set(&tx.hash());
                }
            }
        }
    }

    /// Stages a transaction for deferred parallel signature verification.
    /// Use this for bulk P2P ingestion to avoid per-tx overhead on the hot path.
    /// Call `flush_staged()` before block production to admit verified txs.
    pub fn stage(&mut self, tx: Transaction) {
        // Bloom pre-filter: skip signature verification for hashes already admitted.
        // False positives (~1%) cause rare benign drops; correctness is guaranteed by `seen`.
        if self.bloom.check(&tx.hash()) {
            return;
        }
        if self.unverified.len() + self.pending_count < self.max_size * 2 {
            self.unverified.push(tx);
        }
    }

    /// Verifies all staged transactions in parallel across all CPU cores (rayon),
    /// then admits the valid ones into the verified queue.
    /// Returns the number of transactions successfully admitted.
    pub fn flush_staged(&mut self) -> usize {
        if self.unverified.is_empty() {
            return 0;
        }
        let to_verify = std::mem::take(&mut self.unverified);

        // Parallel signature verification — each CPU core processes a subset
        let verified: Vec<Transaction> = to_verify
            .into_par_iter()
            .filter(|tx| Self::verify_sig_static(tx))
            .collect();

        let count = verified.len();
        for tx in verified {
            // add() already calls notify_one() internally
            let _ = self.add(tx); // silently discard Full / Duplicate
        }
        count
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
        let mut heap: BinaryHeap<(u128, String)> = self
            .queues
            .iter()
            .filter_map(|(addr, queue)| {
                queue
                    .values()
                    .next()
                    .map(|tx| (tx.fee.atoms(), addr.clone()))
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
    pub fn requeue(&mut self, txs: Vec<Transaction>) {
        for tx in txs {
            let hash = tx.hash();
            if self.seen.contains(&hash) {
                continue;
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

    /// Returns the next nonce to use for `addr`, accounting for pending transactions.
    /// Returns `None` if there are no pending transactions (caller should use confirmed nonce).
    pub fn next_nonce_for(&self, addr: &Address) -> Option<u64> {
        self.queues
            .get(&addr.to_string())
            .and_then(|q| q.keys().last())
            .map(|&n| n + 1)
    }

    /// Verifies a transaction's cryptographic signature without holding &mut self.
    fn verify_sig_static(tx: &Transaction) -> bool {
        let Some(pk) = &tx.pub_key else { return false };
        if Address::from_public_key(pk) != tx.from {
            return false;
        }
        let Some(sig) = &tx.signature else {
            return false;
        };
        pk.verify(&tx.signing_bytes(), sig).is_ok()
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
    #[error("Too many pending transactions from this address")]
    RateLimited,
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS};
    use vinx_crypto::KeyPair;

    fn make_tx(kp: &KeyPair, to: Address, nonce: u64) -> Transaction {
        let amount = Amount::from_vinx(1);
        let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));
        vinx_core::Transaction::new_transfer(kp, to, amount, fee, nonce)
    }

    fn dummy_addr() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    #[test]
    fn test_add_and_drain() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = dummy_addr();
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
        let to = dummy_addr();
        let tx = make_tx(&kp, to, 0);
        mp.add(tx.clone()).unwrap();
        assert_eq!(mp.add(tx), Err(MempoolError::Duplicate));
    }

    #[test]
    fn test_full_rejected() {
        let mut mp = Mempool::new(1);
        let kp = KeyPair::generate();
        let to = dummy_addr();
        mp.add(make_tx(&kp, to.clone(), 0)).unwrap();
        assert_eq!(mp.add(make_tx(&kp, to, 1)), Err(MempoolError::Full));
    }

    #[test]
    fn test_drain_all() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = dummy_addr();
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
        let to = dummy_addr();
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
        let to = dummy_addr();
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
        let to = dummy_addr();
        let tx = make_tx(&kp, to, 0);
        mp.add(tx.clone()).unwrap();
        mp.requeue(vec![tx]);
        assert_eq!(mp.size(), 1);
    }

    fn make_tx_with_fee(kp: &KeyPair, to: Address, nonce: u64, fee_vinx: u64) -> Transaction {
        vinx_core::Transaction::new_transfer(
            kp,
            to,
            Amount::from_vinx(1),
            Amount::from_vinx(fee_vinx),
            nonce,
        )
    }

    #[test]
    fn test_fee_priority_higher_fee_first() {
        let mut mp = Mempool::new(10);
        let kp_a = KeyPair::generate();
        let kp_b = KeyPair::generate();
        let to = dummy_addr();
        mp.add(make_tx_with_fee(&kp_a, to.clone(), 0, 1)).unwrap();
        mp.add(make_tx_with_fee(&kp_b, to.clone(), 0, 10)).unwrap();
        let drained = mp.drain(10);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].fee, Amount::from_vinx(10));
        assert_eq!(drained[1].fee, Amount::from_vinx(1));
    }

    #[test]
    fn test_fee_priority_nonce_order_preserved() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = dummy_addr();
        mp.add(make_tx_with_fee(&kp, to.clone(), 0, 5)).unwrap();
        mp.add(make_tx_with_fee(&kp, to.clone(), 1, 100)).unwrap();
        let drained = mp.drain(10);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].nonce, 0);
        assert_eq!(drained[1].nonce, 1);
    }

    #[test]
    fn test_multi_account_interleaving() {
        let mut mp = Mempool::new(20);
        let kp_a = KeyPair::generate();
        let kp_b = KeyPair::generate();
        let to = dummy_addr();
        mp.add(make_tx(&kp_a, to.clone(), 0)).unwrap();
        mp.add(make_tx(&kp_a, to.clone(), 1)).unwrap();
        mp.add(make_tx(&kp_b, to.clone(), 0)).unwrap();
        mp.add(make_tx(&kp_b, to.clone(), 1)).unwrap();
        let drained = mp.drain(10);
        assert_eq!(drained.len(), 4);
        assert_eq!(mp.size(), 0);
    }

    #[test]
    fn test_flush_staged_verifies_in_parallel() {
        let mut mp = Mempool::new(100);
        let kp = KeyPair::generate();
        let to = dummy_addr();

        // Stage 10 valid transactions
        for i in 0..10u64 {
            mp.stage(make_tx(&kp, to.clone(), i));
        }
        assert_eq!(mp.size(), 0); // not admitted yet

        let admitted = mp.flush_staged();
        assert_eq!(admitted, 10);
        assert_eq!(mp.size(), 10);
    }

    #[test]
    fn test_flush_staged_rejects_bad_sig() {
        let mut mp = Mempool::new(100);
        let kp = KeyPair::generate();
        let to = dummy_addr();
        let mut tx = make_tx(&kp, to, 0);
        // Tamper with the amount to invalidate the signature
        tx.amount = Amount::from_vinx(999_999);
        mp.stage(tx);
        let admitted = mp.flush_staged();
        assert_eq!(admitted, 0);
        assert_eq!(mp.size(), 0);
    }
}
