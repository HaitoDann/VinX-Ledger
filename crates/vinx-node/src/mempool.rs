use ahash::{AHashMap, AHashSet};
use bloomfilter::Bloom;
use rayon::prelude::*;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::sync::Arc;
use tokio::sync::Notify;
use vinx_core::{CoreError, Transaction};
use vinx_crypto::{Address, Hash32};
use vinx_state::WorldState;

const DEFAULT_MAX_SIZE: usize = 100_000;
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
    /// sender `Address` → sorted(nonce → tx) — all entries here have valid signatures.
    /// Keyed by the raw 20-byte address (Copy), hashed with ahash — no bech32
    /// String allocation per insert/lookup on the hot path.
    queues: AHashMap<Address, BTreeMap<u64, Transaction>>,
    /// Staging queue for unverified incoming transactions (e.g. from P2P batch ingestion).
    unverified: Vec<Transaction>,
    /// All known tx hashes — prevents double-submission.
    seen: AHashSet<Hash32>,
    /// Bloom filter pre-screening duplicate hashes in `stage()` to skip
    /// costly signature verification on already-known P2P-gossiped transactions.
    /// 1% false-positive rate at capacity — correctness guaranteed by `seen`.
    bloom: Bloom<Hash32>,
    /// Minimum acceptable nonce per sender — updated after each block is applied.
    /// Transactions with nonce < min_nonce are rejected immediately in add().
    min_nonce: AHashMap<Address, u64>,
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
            queues: AHashMap::new(),
            unverified: Vec::new(),
            seen: AHashSet::new(),
            bloom,
            min_nonce: AHashMap::new(),
            pending_count: 0,
            max_size,
            tx_ready: Arc::new(Notify::new()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pending_count == 0
    }

    /// Sum of the worst-case balance debits of every transaction already queued
    /// for `addr` (bounded by MAX_PER_ADDRESS, so at most a 50-entry walk).
    /// Used by stateful admission: the sender's balance must cover the whole
    /// queue plus the incoming transaction, not just each one individually —
    /// otherwise one funded fee could back 50 unpayable high-priority entries.
    pub fn queued_cost_atoms(&self, addr: &Address) -> u128 {
        self.queues.get(addr).map_or(0, |queue| {
            queue
                .values()
                .map(|tx| tx.admission_cost_atoms())
                .fold(0u128, |acc, c| acc.saturating_add(c))
        })
    }

    /// Inserts a pre-verified transaction into the per-account nonce queue.
    /// The caller is responsible for verifying the signature before calling this.
    pub fn add(&mut self, tx: Transaction) -> Result<(), MempoolError> {
        let addr = tx.from;

        // Reject transactions whose nonce is already consumed by a confirmed block.
        let min = self.min_nonce.get(&addr).copied().unwrap_or(0);
        if tx.nonce < min {
            return Err(MempoolError::StaleNonce);
        }

        if self.pending_count >= self.max_size && !self.try_evict_for(&tx) {
            return Err(MempoolError::Full);
        }

        let hash = tx.hash();

        // Exact duplicate check (same hash = same tx).
        if self.seen.contains(&hash) {
            return Err(MempoolError::Duplicate);
        }

        let queue = self.queues.entry(addr).or_default();
        if queue.len() >= MAX_PER_ADDRESS && !queue.contains_key(&tx.nonce) {
            return Err(MempoolError::RateLimited);
        }

        // Fee bump: if a pending tx exists at the same nonce, replace only if fee is higher.
        if let Some(existing) = queue.get(&tx.nonce) {
            if tx.fee > existing.fee {
                let old_hash = existing.hash();
                self.seen.remove(&old_hash);
                self.seen.insert(hash);
                self.bloom.set(&hash);
                queue.insert(tx.nonce, tx);
                // pending_count unchanged — one tx replaced another
                self.tx_ready.notify_one();
                return Ok(());
            } else {
                return Err(MempoolError::NonceTaken);
            }
        }

        self.seen.insert(hash);

        self.bloom.set(&hash);
        queue.insert(tx.nonce, tx);
        self.pending_count += 1;
        self.tx_ready.notify_one();
        Ok(())
    }

    /// Updates the minimum acceptable nonce per sender after a block is applied,
    /// and immediately removes any pending transactions whose nonce is now stale.
    /// Called by the block producer after each successful block.
    pub fn update_confirmed_nonces(&mut self, confirmed: &HashMap<Address, u64>) {
        let mut removed = 0usize;
        for (addr, &next_nonce) in confirmed {
            // Always advance — never go backwards.
            let entry = self.min_nonce.entry(*addr).or_insert(0);
            if next_nonce > *entry {
                *entry = next_nonce;
            }
            // Flush any pending txs that are now below the confirmed nonce.
            if let Some(queue) = self.queues.get_mut(addr) {
                let stale: Vec<u64> = queue.keys().copied().filter(|&n| n < next_nonce).collect();
                for nonce in stale {
                    if let Some(tx) = queue.remove(&nonce) {
                        self.seen.remove(&tx.hash());
                        removed += 1;
                    }
                }
            }
        }
        if removed > 0 {
            self.pending_count = self.pending_count.saturating_sub(removed);
            self.queues.retain(|_, q| !q.is_empty());
            // Rebuild bloom so evicted hashes stop causing false-positive skips.
            self.bloom.clear();
            for queue in self.queues.values() {
                for tx in queue.values() {
                    self.bloom.set(&tx.hash());
                }
            }
        }
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
            .filter(Self::verify_sig_static)
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
        let mut heap: BinaryHeap<(u128, Address)> = self
            .queues
            .iter()
            .filter_map(|(addr, queue)| queue.values().next().map(|tx| (tx.fee.atoms(), *addr)))
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
                heap.push((next_tx.fee.atoms(), addr));
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
            self.queues.entry(tx.from).or_default().insert(tx.nonce, tx);
            self.pending_count += 1;
        }
    }

    /// Tente d'évincer la transaction avec le fee le plus bas si son fee est
    /// strictement inférieur au fee de `incoming`. Retourne `true` si une
    /// éviction a eu lieu (et le slot est maintenant libre).
    fn try_evict_for(&mut self, incoming: &Transaction) -> bool {
        let mut min_fee = incoming.fee.atoms();
        let mut victim: Option<(Address, u64)> = None;

        for (addr, queue) in &self.queues {
            for (&nonce, tx) in queue {
                if tx.fee.atoms() < min_fee {
                    min_fee = tx.fee.atoms();
                    victim = Some((*addr, nonce));
                }
            }
        }

        let (addr, nonce) = match victim {
            Some(v) => v,
            None => return false,
        };
        let queue = self.queues.get_mut(&addr).unwrap();
        let evicted = queue.remove(&nonce).unwrap();
        self.seen.remove(&evicted.hash());
        if queue.is_empty() {
            self.queues.remove(&addr);
        }
        self.pending_count -= 1;
        true
    }

    pub fn size(&self) -> usize {
        self.pending_count
    }

    /// Returns references to all pending (verified) transactions across all accounts.
    pub fn pending_txs(&self) -> Vec<&Transaction> {
        self.queues.values().flat_map(|q| q.values()).collect()
    }

    /// Looks up a set of transaction hashes and returns clones of matching transactions
    /// from the pending pool.  Used by the compact-block responder (ADR 0037).
    pub fn get_by_hashes(&self, hashes: &[[u8; 32]]) -> Vec<Transaction> {
        // VINX-09: `hashes.contains()` inside the mempool scan made this O(mempool × hashes)
        // with a SHA-256 per comparison. Hash the request set once instead.
        let wanted: AHashSet<[u8; 32]> = hashes.iter().copied().collect();
        let mut out = Vec::with_capacity(hashes.len());
        for tx in self.queues.values().flat_map(|q| q.values()) {
            if wanted.contains(&tx.hash()) {
                out.push(tx.clone());
                if out.len() == wanted.len() {
                    break;
                }
            }
        }
        out
    }

    /// Resolves `hashes` against the pending pool in a single scan (ADR 0037, VINX-09).
    /// Returns the transactions found (in request order) and the hashes still missing.
    pub fn resolve_hashes(&self, hashes: &[[u8; 32]]) -> (Vec<Transaction>, Vec<[u8; 32]>) {
        let mut found: AHashMap<[u8; 32], Transaction> = AHashMap::new();
        let wanted: AHashSet<[u8; 32]> = hashes.iter().copied().collect();
        for tx in self.queues.values().flat_map(|q| q.values()) {
            let h = tx.hash();
            if wanted.contains(&h) {
                found.insert(h, tx.clone());
            }
        }
        let mut resolved = Vec::with_capacity(found.len());
        let mut missing = Vec::new();
        for h in hashes {
            match found.get(h) {
                Some(tx) => resolved.push(tx.clone()),
                None => missing.push(*h),
            }
        }
        (resolved, missing)
    }

    /// Returns the next nonce to use for `addr`, accounting for pending transactions.
    /// Returns `None` if there are no pending transactions (caller should use confirmed nonce).
    pub fn next_nonce_for(&self, addr: &Address) -> Option<u64> {
        self.queues
            .get(addr)
            .and_then(|q| q.keys().last())
            .map(|&n| n + 1)
    }

    /// Verifies a transaction's cryptographic signature without holding &mut self.
    ///
    /// VINX-03 / VX-RED-005: this used to check the *sender* signature only. A sponsored
    /// transaction whose `sponsor_signature` was absent or forged therefore entered the
    /// "verified" queue, and the producer applied it with `apply_transaction_trusted`
    /// (which skips crypto by contract) — debiting `fee` from an account that never
    /// consented, with no upper bound on `fee`. Peers, meanwhile, re-check the sponsor via
    /// `verify_tx_signature_pure` and reject the block, so the producer also forked itself.
    ///
    /// There must be exactly one definition of cryptographic validity. This delegates to it.
    fn verify_sig_static(tx: &Transaction) -> bool {
        WorldState::verify_tx_signature_pure(tx).is_ok()
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
    #[error("Transaction nonce already consumed by a confirmed block")]
    StaleNonce,
    #[error(
        "A pending transaction already occupies this nonce; submit with a higher fee to replace it"
    )]
    NonceTaken,
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
        mp.add(make_tx(&kp, to, 0)).unwrap();
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
        mp.add(make_tx(&kp, to, 0)).unwrap();
        assert_eq!(mp.add(make_tx(&kp, to, 1)), Err(MempoolError::Full));
    }

    #[test]
    fn test_drain_all() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = dummy_addr();
        for i in 0..5 {
            mp.add(make_tx(&kp, to, i)).unwrap();
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
        mp.add(make_tx(&kp, to, 2)).unwrap();
        mp.add(make_tx(&kp, to, 0)).unwrap();
        mp.add(make_tx(&kp, to, 1)).unwrap();

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
    fn test_fee_eviction_when_full() {
        let mut mp = Mempool::new(1);
        let kp_low = KeyPair::generate();
        let kp_high = KeyPair::generate();
        let to = dummy_addr();
        // Fill mempool with a low-fee tx
        mp.add(make_tx_with_fee(&kp_low, to, 0, 1)).unwrap();
        assert_eq!(mp.size(), 1);
        // Higher-fee tx should evict the low-fee one
        mp.add(make_tx_with_fee(&kp_high, to, 0, 10)).unwrap();
        assert_eq!(mp.size(), 1);
        // The remaining tx should have the high fee
        let drained = mp.drain(10);
        assert_eq!(drained[0].fee, Amount::from_vinx(10));
    }

    #[test]
    fn test_no_eviction_when_incoming_fee_not_higher() {
        let mut mp = Mempool::new(1);
        let kp = KeyPair::generate();
        let to = dummy_addr();
        mp.add(make_tx_with_fee(&kp, to, 0, 5)).unwrap();
        // Same fee — no eviction, returns Full
        let kp2 = KeyPair::generate();
        assert_eq!(
            mp.add(make_tx_with_fee(&kp2, to, 0, 5)),
            Err(MempoolError::Full)
        );
        assert_eq!(mp.size(), 1);
    }

    #[test]
    fn test_fee_priority_higher_fee_first() {
        let mut mp = Mempool::new(10);
        let kp_a = KeyPair::generate();
        let kp_b = KeyPair::generate();
        let to = dummy_addr();
        mp.add(make_tx_with_fee(&kp_a, to, 0, 1)).unwrap();
        mp.add(make_tx_with_fee(&kp_b, to, 0, 10)).unwrap();
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
        mp.add(make_tx_with_fee(&kp, to, 0, 5)).unwrap();
        mp.add(make_tx_with_fee(&kp, to, 1, 100)).unwrap();
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
        mp.add(make_tx(&kp_a, to, 0)).unwrap();
        mp.add(make_tx(&kp_a, to, 1)).unwrap();
        mp.add(make_tx(&kp_b, to, 0)).unwrap();
        mp.add(make_tx(&kp_b, to, 1)).unwrap();
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
            mp.stage(make_tx(&kp, to, i));
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

    #[test]
    fn test_stale_nonce_rejected() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = dummy_addr();
        let addr = vinx_crypto::Address::from_public_key(&kp.public_key());
        let mut confirmed = std::collections::HashMap::new();
        confirmed.insert(addr, 2u64);
        mp.update_confirmed_nonces(&confirmed);
        assert_eq!(mp.add(make_tx(&kp, to, 0)), Err(MempoolError::StaleNonce));
        assert_eq!(mp.add(make_tx(&kp, to, 1)), Err(MempoolError::StaleNonce));
        mp.add(make_tx(&kp, to, 2)).unwrap();
        assert_eq!(mp.size(), 1);
    }

    #[test]
    fn test_fee_bump_replaces_same_nonce() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = dummy_addr();
        mp.add(make_tx_with_fee(&kp, to, 0, 1)).unwrap();
        mp.add(make_tx_with_fee(&kp, to, 0, 10)).unwrap();
        assert_eq!(mp.size(), 1);
        let drained = mp.drain(10);
        assert_eq!(drained[0].fee, Amount::from_vinx(10));
    }

    #[test]
    fn test_nonce_taken_same_fee() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = dummy_addr();
        mp.add(make_tx_with_fee(&kp, to, 0, 5)).unwrap();
        // Different amount → different hash, but same nonce and same fee → NonceTaken
        let tx2 = vinx_core::Transaction::new_transfer(
            &kp,
            to,
            Amount::from_vinx(2),
            Amount::from_vinx(5),
            0,
        );
        assert_eq!(mp.add(tx2), Err(MempoolError::NonceTaken));
        assert_eq!(mp.size(), 1);
    }

    #[test]
    fn test_update_confirmed_nonces_prunes_queue() {
        let mut mp = Mempool::new(10);
        let kp = KeyPair::generate();
        let to = dummy_addr();
        mp.add(make_tx(&kp, to, 0)).unwrap();
        mp.add(make_tx(&kp, to, 1)).unwrap();
        mp.add(make_tx(&kp, to, 2)).unwrap();
        assert_eq!(mp.size(), 3);
        let addr = vinx_crypto::Address::from_public_key(&kp.public_key());
        let mut confirmed = std::collections::HashMap::new();
        confirmed.insert(addr, 2u64);
        mp.update_confirmed_nonces(&confirmed);
        assert_eq!(mp.size(), 1);
        let drained = mp.drain(10);
        assert_eq!(drained[0].nonce, 2);
    }
}
