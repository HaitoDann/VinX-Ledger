use ahash::{AHashMap, AHashSet};

use serde::{Deserialize, Serialize};
use vinx_core::{block::GENESIS_PREV_HASH, Block, BlockHeader, Transaction};
use vinx_crypto::{Address, Hash32};

#[derive(Serialize, Deserialize)]
pub struct Chain {
    /// Stored as (block_hash, block) indexed by height.
    blocks: Vec<(Hash32, Block)>,
    /// Highest height whose block has reached quorum co-signatures (ADR 0002).
    /// Everything at or below is **final** — never reorganized. Genesis (0) is final.
    #[serde(default)]
    finalized_height: u64,
    /// Maps raw tx hash -> (block_height, tx_position). Not persisted via serde
    /// (rebuilt or imported by Storage). Keyed by the 32-byte hash directly —
    /// no hex allocation per insert/lookup — hashed with ahash on the hot path.
    #[serde(skip)]
    tx_index: AHashMap<Hash32, (u64, u32)>,
    /// Maps address -> ordered list of tx hashes (oldest first). Not persisted via
    /// serde. Keyed by the raw 20-byte `Address` (Copy) rather than a bech32 String.
    #[serde(skip)]
    account_tx_index: AHashMap<Address, Vec<Hash32>>,
    /// Maps validator addr -> height -> set of block hashes signed (equivocation detection).
    #[serde(skip)]
    slash_evidence: AHashMap<Address, AHashMap<u64, AHashSet<Hash32>>>,
    /// Heights whose stored block changed since the last persistence flush (new
    /// block, co-signature landed, tx/sig data pruned). Drained by
    /// `take_dirty_heights` so Storage writes only those rows instead of
    /// re-serializing the whole chain on every persist.
    #[serde(skip)]
    dirty_heights: AHashSet<u64>,
}

impl Chain {
    pub fn new_with_genesis(validator: Address, timestamp: u64) -> (Self, Block) {
        let genesis = Block {
            header: BlockHeader {
                height: 0,
                prev_hash: GENESIS_PREV_HASH,
                timestamp,
                validator,
                tx_count: 0,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
            },
            transactions: vec![],
            signatures: vec![],
        };
        let hash = genesis.hash();
        let chain = Self {
            blocks: vec![(hash, genesis.clone())],
            finalized_height: 0,
            tx_index: AHashMap::new(),
            account_tx_index: AHashMap::new(),
            slash_evidence: AHashMap::new(),
            dirty_heights: AHashSet::from_iter([0]),
        };
        (chain, genesis)
    }

    /// Rebuilds a chain from persisted parts: the per-height block rows (dense,
    /// starting at genesis) and the finalized-height watermark. Tx indexes are
    /// restored or rebuilt separately by the caller.
    pub fn from_parts(blocks: Vec<(Hash32, Block)>, finalized_height: u64) -> Self {
        Self {
            blocks,
            finalized_height,
            tx_index: AHashMap::new(),
            account_tx_index: AHashMap::new(),
            slash_evidence: AHashMap::new(),
            dirty_heights: AHashSet::new(),
        }
    }

    /// Drains and returns the set of heights whose block row must be rewritten.
    pub fn take_dirty_heights(&mut self) -> Vec<u64> {
        self.dirty_heights.drain().collect()
    }

    /// Marks every stored block dirty — used by full saves (genesis bootstrap,
    /// snapshot import) so the whole blocks table is rewritten.
    pub fn mark_all_dirty(&mut self) {
        self.dirty_heights = (0..self.blocks.len() as u64).collect();
    }

    /// Borrow of the stored `(hash, block)` row at `height`, for persistence.
    pub fn block_row(&self, height: u64) -> Option<&(Hash32, Block)> {
        self.blocks.get(height as usize)
    }

    /// Highest final (quorum-signed) height. Everything at or below is irreversible.
    pub fn finalized_height(&self) -> u64 {
        self.finalized_height
    }

    /// True when `height` is final (quorum-signed and never reorganizable).
    pub fn is_final(&self, height: u64) -> bool {
        height <= self.finalized_height
    }

    /// Advances `finalized_height` over the contiguous prefix of quorum-signed blocks
    /// above the current mark (ADR 0002). Called after producing a block and after a
    /// co-signature lands. Finality is prefix-closed: it stops at the first block that
    /// has not yet reached quorum. Returns the new finalized height.
    pub fn advance_finality(&mut self, validator_set: &vinx_core::ValidatorSet) -> u64 {
        let tip = self.tip_height();
        while self.finalized_height < tip {
            let next = self.finalized_height + 1;
            match self.get_block(next) {
                Some(b) if b.is_finalized(validator_set) => self.finalized_height = next,
                _ => break,
            }
        }
        self.finalized_height
    }

    /// Height of the latest block (0 = only genesis exists).
    pub fn tip_height(&self) -> u64 {
        (self.blocks.len() as u64).saturating_sub(1)
    }

    pub fn tip_hash(&self) -> Hash32 {
        self.blocks
            .last()
            .map(|(h, _)| *h)
            .unwrap_or(GENESIS_PREV_HASH)
    }

    /// Timestamp (unix seconds) of the tip block. Used for the monotonicity bound on
    /// incoming block timestamps (ADR 0005).
    pub fn tip_timestamp(&self) -> u64 {
        self.blocks
            .last()
            .map(|(_, b)| b.header.timestamp)
            .unwrap_or(0)
    }

    /// Median Time Past: median of the last `MEDIAN_TIME_BLOCKS` block timestamps
    /// (ADR 0005). A single producer cannot make this reference jump — it is a median,
    /// so it resists timestamp manipulation. Intended reference for time-sensitive
    /// comparisons (emission, unbonding) once wired onto it.
    pub fn median_time_past(&self) -> u64 {
        let n = self.blocks.len();
        let start = n.saturating_sub(vinx_core::amount::MEDIAN_TIME_BLOCKS);
        let mut ts: Vec<u64> = self.blocks[start..]
            .iter()
            .map(|(_, b)| b.header.timestamp)
            .collect();
        if ts.is_empty() {
            return 0;
        }
        ts.sort_unstable();
        ts[ts.len() / 2]
    }

    pub fn get_block(&self, height: u64) -> Option<&Block> {
        self.blocks.get(height as usize).map(|(_, b)| b)
    }

    /// Appends a block and returns its hash.
    pub fn push(&mut self, block: Block) -> Hash32 {
        let height = block.header.height;
        for (idx, tx) in block.transactions.iter().enumerate() {
            let tx_hash = tx.hash();
            self.tx_index.insert(tx_hash, (height, idx as u32));
            self.account_tx_index
                .entry(tx.from)
                .or_default()
                .push(tx_hash);
            if tx.to != tx.from {
                self.account_tx_index
                    .entry(tx.to)
                    .or_default()
                    .push(tx_hash);
            }
        }
        let hash = block.hash();
        self.blocks.push((hash, block));
        self.dirty_heights.insert(height);
        hash
    }

    /// Exports both tx indexes for external persistence (called by Storage::save).
    #[allow(clippy::type_complexity)]
    pub fn export_tx_indexes(
        &self,
    ) -> (
        &AHashMap<Hash32, (u64, u32)>,
        &AHashMap<Address, Vec<Hash32>>,
    ) {
        (&self.tx_index, &self.account_tx_index)
    }

    /// Replaces both tx indexes from a previously persisted snapshot.
    /// Faster than `rebuild_tx_index` — O(1) deserialization vs O(blocks × txs).
    pub fn import_tx_indexes(
        &mut self,
        tx_index: AHashMap<Hash32, (u64, u32)>,
        account_tx_index: AHashMap<Address, Vec<Hash32>>,
    ) {
        self.tx_index = tx_index;
        self.account_tx_index = account_tx_index;
    }

    /// Clears and repopulates both tx indexes from all stored blocks.
    pub fn rebuild_tx_index(&mut self) {
        self.tx_index.clear();
        self.account_tx_index.clear();
        for (_, block) in &self.blocks {
            let height = block.header.height;
            for (idx, tx) in block.transactions.iter().enumerate() {
                let tx_hash = tx.hash();
                self.tx_index.insert(tx_hash, (height, idx as u32));
                self.account_tx_index
                    .entry(tx.from)
                    .or_default()
                    .push(tx_hash);
                if tx.to != tx.from {
                    self.account_tx_index
                        .entry(tx.to)
                        .or_default()
                        .push(tx_hash);
                }
            }
        }
    }

    /// Total number of transactions involving this address.
    pub fn account_tx_count(&self, addr: &Address) -> usize {
        self.account_tx_index.get(addr).map_or(0, |v| v.len())
    }

    /// Returns tx hashes for the given address, newest-first, with pagination.
    pub fn get_account_txs(&self, addr: &Address, limit: usize, offset: usize) -> Vec<Hash32> {
        match self.account_tx_index.get(addr) {
            None => vec![],
            Some(hashes) => {
                let len = hashes.len();
                if offset >= len {
                    return vec![];
                }
                hashes
                    .iter()
                    .rev()
                    .skip(offset)
                    .take(limit)
                    .copied()
                    .collect()
            }
        }
    }

    /// Looks up a transaction by its raw 32-byte hash.
    pub fn get_tx_by_hash(&self, hash: &Hash32) -> Option<(u64, &Block, &Transaction)> {
        let &(height, tx_pos) = self.tx_index.get(hash)?;
        let (_, block) = self.blocks.get(height as usize)?;
        let tx = block.transactions.get(tx_pos as usize)?;
        Some((height, block, tx))
    }

    /// Number of blocks (= tip_height + 1).
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// True when the chain holds no blocks (never the case after genesis).
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Adds a co-signature to an already-stored block.
    /// Returns `true` if the block is now finalized (≥ quorum valid signatures).
    pub fn add_co_signature(
        &mut self,
        height: u64,
        signature: vinx_core::BlockSignature,
        validator_set: &vinx_core::ValidatorSet,
    ) -> bool {
        if let Some((_, block)) = self.blocks.get_mut(height as usize) {
            block.signatures.push(signature);
            self.dirty_heights.insert(height);
            return block.is_finalized(validator_set);
        }
        false
    }

    /// Records that `validator` signed `block_hash` at `height`.
    /// Returns `true` if equivocation is detected (validator signed a DIFFERENT
    /// block at the same height — a slashable offense).
    pub fn record_signature(
        &mut self,
        validator: &Address,
        height: u64,
        block_hash: Hash32,
    ) -> bool {
        let heights = self.slash_evidence.entry(*validator).or_default();
        let hashes = heights.entry(height).or_default();
        if !hashes.is_empty() && !hashes.contains(&block_hash) {
            return true; // double-sign detected
        }
        hashes.insert(block_hash);
        false
    }

    /// Compacts transaction data from blocks older than `keep_last` blocks.
    /// Block headers and hashes are retained to preserve chain integrity.
    /// This reduces memory/disk usage without breaking hash linkage verification.
    pub fn compact_old_txs(&mut self, keep_last: u64) {
        let tip = self.tip_height();
        if tip < keep_last {
            return;
        }
        let compact_up_to = (tip - keep_last) as usize;
        for i in 0..compact_up_to {
            if let Some((_, block)) = self.blocks.get_mut(i) {
                // Only touch (and re-persist) blocks that still had data — repeated
                // compaction passes must not mark the whole history dirty again.
                if !block.transactions.is_empty() {
                    block.transactions.clear();
                    self.dirty_heights.insert(i as u64);
                }
            }
        }
        // Rebuild index to remove entries from pruned blocks
        self.rebuild_tx_index();
        tracing::info!(
            compacted = compact_up_to,
            "Chain compacted old transaction data"
        );
    }

    /// Full pruning pass — runs every PRUNE_INTERVAL blocks.
    ///
    /// Three things are cleaned up:
    /// 1. Transaction data older than `keep_last` blocks (largest space consumer).
    /// 2. Signatures on finalized blocks older than `keep_last` (verified, no longer needed).
    /// 3. Slash evidence older than `keep_last * 2` (equivocation window is well past).
    ///
    /// Block headers (height, prev_hash, state_root, validator…) are NEVER dropped —
    /// they are needed for hash-chain integrity and light-client sync proofs.
    pub fn prune(&mut self, keep_last: u64) {
        let tip = self.tip_height();
        if tip < keep_last {
            return;
        }
        let prune_up_to = (tip - keep_last) as usize;

        let mut tx_pruned = 0usize;
        let mut sig_pruned = 0usize;

        for i in 0..prune_up_to {
            if let Some((_, block)) = self.blocks.get_mut(i) {
                // Same rule as compaction: already-empty blocks stay untouched so
                // periodic prune passes only re-persist the newly pruned window.
                if !block.transactions.is_empty() || !block.signatures.is_empty() {
                    tx_pruned += block.transactions.len();
                    block.transactions.clear();
                    sig_pruned += block.signatures.len();
                    block.signatures.clear();
                    self.dirty_heights.insert(i as u64);
                }
            }
        }

        // Remove slash evidence older than 2× the retention window
        let evidence_cutoff = tip.saturating_sub(keep_last * 2);
        self.slash_evidence.retain(|_, heights| {
            heights.retain(|&h, _| h > evidence_cutoff);
            !heights.is_empty()
        });

        // Rebuild tx index to remove stale entries
        self.rebuild_tx_index();

        tracing::info!(
            tip,
            pruned_below = prune_up_to,
            tx_pruned,
            sig_pruned,
            "Chain pruned — headers retained, old tx/sig data dropped"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn validator() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    #[test]
    fn test_genesis_height_is_zero() {
        let (chain, _) = Chain::new_with_genesis(validator(), 0);
        assert_eq!(chain.tip_height(), 0);
    }

    #[test]
    fn test_genesis_prev_hash_is_zero() {
        let (chain, _) = Chain::new_with_genesis(validator(), 0);
        let genesis = chain.get_block(0).unwrap();
        assert_eq!(genesis.header.prev_hash, GENESIS_PREV_HASH);
    }

    #[test]
    fn test_tip_hash_matches_genesis_block() {
        let (chain, genesis) = Chain::new_with_genesis(validator(), 0);
        assert_eq!(chain.tip_hash(), genesis.hash());
    }

    #[test]
    fn test_get_block_out_of_range() {
        let (chain, _) = Chain::new_with_genesis(validator(), 0);
        assert!(chain.get_block(999).is_none());
    }

    #[test]
    fn test_equivocation_detection() {
        let mut chain = Chain::from_parts(vec![], 0);
        let addr = Address::from_public_key(&KeyPair::generate().public_key());
        let hash_a = [1u8; 32];
        let hash_b = [2u8; 32];

        assert!(!chain.record_signature(&addr, 5, hash_a)); // first sig — ok
        assert!(!chain.record_signature(&addr, 5, hash_a)); // same hash — ok (idempotent)
        assert!(chain.record_signature(&addr, 5, hash_b)); // different hash — EQUIVOCATION
    }

    // Builds a block at `height` signed by each of `signers` (quorum evidence).
    fn signed_block(height: u64, prev: Hash32, proposer: Address, signers: &[&KeyPair]) -> Block {
        let header = BlockHeader {
            height,
            prev_hash: prev,
            timestamp: height,
            validator: proposer,
            tx_count: 0,
            state_root: [0u8; 32],
            base_fee: 0,
            receipts_root: [0u8; 32],
        };
        let hash = header.hash();
        let signatures = signers
            .iter()
            .map(|kp| vinx_core::BlockSignature {
                validator: Address::from_public_key(&kp.public_key()),
                pub_key: kp.public_key(),
                signature: kp.sign(&hash),
            })
            .collect();
        Block {
            header,
            transactions: vec![],
            signatures,
        }
    }

    #[test]
    fn test_finality_advances_with_quorum() {
        // Single validator: its own signature already meets quorum → block 1 final.
        let kp = KeyPair::generate();
        let v = Address::from_public_key(&kp.public_key());
        let vs = vinx_core::ValidatorSet::single(v);
        let (mut chain, _) = Chain::new_with_genesis(v, 0);
        assert_eq!(chain.finalized_height(), 0);

        let b1 = signed_block(1, chain.tip_hash(), v, &[&kp]);
        chain.push(b1);
        assert_eq!(chain.finalized_height(), 0); // not advanced until we ask
        chain.advance_finality(&vs);
        assert_eq!(chain.finalized_height(), 1);
        assert!(chain.is_final(1));
        assert!(!chain.is_final(2));
    }

    #[test]
    fn test_median_time_past_is_the_median() {
        let kp = KeyPair::generate();
        let v = Address::from_public_key(&kp.public_key());
        let (mut chain, _) = Chain::new_with_genesis(v, 0); // genesis ts = 0
        for h in 1..=5 {
            let b = signed_block(h, chain.tip_hash(), v, &[&kp]); // ts = h
            chain.push(b);
        }
        // timestamps {0,1,2,3,4,5} → median (index 3) = 3
        assert_eq!(chain.median_time_past(), 3);
    }

    #[test]
    fn test_finality_stops_below_quorum() {
        // Two validators (quorum 2): a block with only the proposer's sig is NOT final.
        let kp = KeyPair::generate();
        let v = Address::from_public_key(&kp.public_key());
        let other = Address::from_public_key(&KeyPair::generate().public_key());
        let vs = vinx_core::ValidatorSet::new(vec![v, other]);
        let (mut chain, _) = Chain::new_with_genesis(v, 0);

        let b1 = signed_block(1, chain.tip_hash(), v, &[&kp]); // 1 of 2 sigs
        chain.push(b1);
        chain.advance_finality(&vs);
        assert_eq!(chain.finalized_height(), 0); // below quorum → not final
    }

    #[test]
    fn test_prune_drops_tx_and_sig_data_but_keeps_headers() {
        let v = validator();
        let (mut chain, _) = Chain::new_with_genesis(v.clone(), 0);
        for h in 1u64..=10 {
            let block = Block {
                header: BlockHeader {
                    height: h,
                    prev_hash: chain.tip_hash(),
                    timestamp: h,
                    validator: v.clone(),
                    tx_count: 0,
                    state_root: [0u8; 32],
                    base_fee: 0,
                    receipts_root: [0u8; 32],
                },
                transactions: vec![],
                signatures: vec![],
            };
            chain.push(block);
        }
        // Prune keeping last 3 blocks (height 8, 9, 10); blocks 0-7 are compacted
        chain.prune(3);
        // All headers still accessible
        for h in 0u64..=10 {
            assert!(
                chain.get_block(h).is_some(),
                "block {h} missing after prune"
            );
        }
        assert_eq!(chain.tip_height(), 10);
    }

    #[test]
    fn test_prune_noop_when_chain_shorter_than_keep_last() {
        let v = validator();
        let (mut chain, _) = Chain::new_with_genesis(v.clone(), 0);
        // Only genesis — prune with keep_last=100 should be a no-op
        chain.prune(100);
        assert_eq!(chain.tip_height(), 0);
        assert!(chain.get_block(0).is_some());
    }

    #[test]
    fn test_compact_old_txs_preserves_headers() {
        let v = validator();
        let (mut chain, _) = Chain::new_with_genesis(v.clone(), 0);
        // Push 5 empty blocks
        for h in 1u64..=5 {
            let block = Block {
                header: BlockHeader {
                    height: h,
                    prev_hash: chain.tip_hash(),
                    timestamp: h,
                    validator: v.clone(),
                    tx_count: 0,
                    state_root: [0u8; 32],
                    base_fee: 0,
                    receipts_root: [0u8; 32],
                },
                transactions: vec![],
                signatures: vec![],
            };
            chain.push(block);
        }
        // Compact keeping only the last 2 blocks
        chain.compact_old_txs(2);
        // Headers still accessible
        assert!(chain.get_block(0).is_some());
        assert!(chain.get_block(4).is_some());
        assert_eq!(chain.tip_height(), 5);
    }
}
