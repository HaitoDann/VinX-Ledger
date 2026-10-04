use ahash::{AHashMap, AHashSet};

use vinx_core::{block::GENESIS_PREV_HASH, Block, BlockHeader, CommitCert, Transaction};
use vinx_crypto::{Address, Hash32};

/// One stored block with the certificate that committed it (ADR 0082).
#[derive(Clone, Debug, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct ChainRow {
    pub hash: Hash32,
    pub block: Block,
    /// Commit certificate of this block. `None` only for genesis and for the base block
    /// of a snapshot-bootstrapped chain.
    pub commit: Option<CommitCert>,
}

/// What block execution needs to know about the chain: its tip and the recent
/// timestamps of the Median-Time-Past window (ADR 0005). Cheap to clone, so consensus can
/// validate proposals without holding the chain lock.
#[derive(Clone, Debug, PartialEq)]
pub struct Tip {
    pub height: u64,
    pub hash: Hash32,
    pub timestamp: u64,
    /// Commit certificate of the tip block (embedded as `last_commit` by the next block).
    pub commit: Option<CommitCert>,
    /// Timestamps of the last `MEDIAN_TIME_BLOCKS - 1` blocks, oldest first.
    pub recent_timestamps: Vec<u64>,
}

impl Tip {
    /// Median Time Past once a block carrying `next_ts` is appended — the protocol clock
    /// of that block (ADR 0005).
    pub fn median_time_past_with(&self, next_ts: u64) -> u64 {
        let mut ts = self.recent_timestamps.clone();
        ts.push(next_ts);
        ts.sort_unstable();
        ts[ts.len() / 2]
    }
}

/// The committed chain (ADR 0082).
///
/// Every stored block is **final**: a block enters the chain only with a certificate of
/// more than 2/3 of the voting power, so there are no competing branches, no fork-choice
/// and no reorganization. `finalized_height()` is simply the tip.
pub struct Chain {
    rows: Vec<ChainRow>,
    /// Height of the first stored block. Zero for chains starting from genesis;
    /// non-zero for chains bootstrapped from a state snapshot:
    /// `tip_height() = height_base + rows.len() - 1`.
    pub height_base: u64,
    /// Maps raw tx hash -> (block_height, tx_position).
    tx_index: AHashMap<Hash32, (u64, u32)>,
    /// Maps address -> ordered list of tx hashes (oldest first).
    account_tx_index: AHashMap<Address, Vec<Hash32>>,
    /// Heights whose stored row changed since the last persistence flush.
    dirty_heights: AHashSet<u64>,
    /// Set by `prune_before`: stored rows below this height must be deleted.
    pruned_below: Option<u64>,
}

impl Chain {
    pub fn new_with_genesis(validator: Address, timestamp: u64) -> (Self, Block) {
        let genesis = Block {
            header: BlockHeader {
                height: 0,
                round: 0,
                prev_hash: GENESIS_PREV_HASH,
                timestamp,
                validator,
                tx_count: 0,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
                last_commit_hash: [0u8; 32],
                version: 0,
            },
            transactions: vec![],
            last_commit: None,
        };
        let row = ChainRow {
            hash: genesis.hash(),
            block: genesis.clone(),
            commit: None,
        };
        let chain = Self {
            rows: vec![row],
            height_base: 0,
            tx_index: AHashMap::new(),
            account_tx_index: AHashMap::new(),
            dirty_heights: AHashSet::from_iter([0]),
            pruned_below: None,
        };
        (chain, genesis)
    }

    /// Rebuilds a chain from persisted rows (dense, starting at `height_base`). Tx indexes
    /// are restored or rebuilt separately by the caller.
    pub fn from_rows(rows: Vec<ChainRow>, height_base: u64) -> Self {
        Self {
            rows,
            height_base,
            tx_index: AHashMap::new(),
            account_tx_index: AHashMap::new(),
            dirty_heights: AHashSet::new(),
            pruned_below: None,
        }
    }

    /// Bootstraps a chain from a state snapshot: the snapshot block (with its commit
    /// certificate) preceded by its `ancestors`, oldest first. The ancestors carry the
    /// timestamps the protocol clock (median time past, ADR 0005) needs for the next
    /// blocks — without them a snapshot-synced node computes a different clock than its
    /// peers and rejects valid blocks. The caller verifies their hash linkage
    /// (`ancestors_link`).
    pub fn new_from_snapshot(
        ancestors: Vec<Block>,
        block: Block,
        commit: Option<CommitCert>,
    ) -> Self {
        let height = block.header.height;
        let base = height - ancestors.len() as u64;
        let mut rows: Vec<ChainRow> = ancestors
            .into_iter()
            .map(|b| ChainRow {
                hash: b.hash(),
                block: b,
                commit: None,
            })
            .collect();
        rows.push(ChainRow {
            hash: block.hash(),
            block,
            commit,
        });
        let mut chain = Self {
            rows,
            height_base: base,
            tx_index: AHashMap::new(),
            account_tx_index: AHashMap::new(),
            dirty_heights: AHashSet::new(),
            pruned_below: None,
        };
        chain.mark_all_dirty();
        chain.rebuild_tx_index();
        chain
    }

    /// True when `ancestors` (oldest first) are consecutive and hash-linked up to `block`.
    pub fn ancestors_link(ancestors: &[Block], block: &Block) -> bool {
        let mut next = block;
        for a in ancestors.iter().rev() {
            if a.header.height + 1 != next.header.height || a.hash() != next.header.prev_hash {
                return false;
            }
            next = a;
        }
        true
    }

    /// The last `MEDIAN_TIME_BLOCKS - 1` blocks before the tip, oldest first (snapshot
    /// serving).
    pub fn tip_ancestors(&self) -> Vec<Block> {
        let n = self.rows.len().saturating_sub(1);
        let start = n.saturating_sub(vinx_core::amount::MEDIAN_TIME_BLOCKS - 1);
        self.rows[start..n]
            .iter()
            .map(|r| r.block.clone())
            .collect()
    }

    /// Drains and returns the set of heights whose row must be rewritten.
    pub fn take_dirty_heights(&mut self) -> Vec<u64> {
        self.dirty_heights.drain().collect()
    }

    /// Marks every stored row dirty — used by full saves.
    pub fn mark_all_dirty(&mut self) {
        self.dirty_heights =
            (self.height_base..self.height_base + self.rows.len() as u64).collect();
    }

    /// The stored row at `height`, for persistence and sync.
    pub fn row(&self, height: u64) -> Option<&ChainRow> {
        let idx = height.checked_sub(self.height_base)? as usize;
        self.rows.get(idx)
    }

    /// Every stored block is committed, so the finalized height is the tip.
    pub fn finalized_height(&self) -> u64 {
        self.tip_height()
    }

    /// True when `height` is stored (hence final).
    pub fn is_final(&self, height: u64) -> bool {
        height <= self.tip_height()
    }

    /// Height of the latest block (0 = only genesis exists).
    pub fn tip_height(&self) -> u64 {
        self.height_base + (self.rows.len() as u64).saturating_sub(1)
    }

    pub fn tip_hash(&self) -> Hash32 {
        self.rows
            .last()
            .map(|r| r.hash)
            .unwrap_or(GENESIS_PREV_HASH)
    }

    /// Timestamp (unix seconds) of the tip block (ADR 0005 monotonicity bound).
    pub fn tip_timestamp(&self) -> u64 {
        self.rows
            .last()
            .map(|r| r.block.header.timestamp)
            .unwrap_or(0)
    }

    /// Commit certificate of the tip block — embedded as `last_commit` by the next block.
    pub fn tip_commit(&self) -> Option<&CommitCert> {
        self.rows.last().and_then(|r| r.commit.as_ref())
    }

    /// Replaces the tip's certificate with a fuller one for the same block (precommits
    /// keep arriving after the decision; more signers credit more co-signers).
    pub fn upgrade_tip_commit(&mut self, cert: CommitCert) {
        let height = self.tip_height();
        if let Some(row) = self.rows.last_mut() {
            if row.hash == cert.block_hash && row.block.header.height == cert.height {
                let better = row
                    .commit
                    .as_ref()
                    .is_none_or(|c| cert.signers().len() > c.signers().len());
                if better {
                    row.commit = Some(cert);
                    self.dirty_heights.insert(height);
                }
            }
        }
    }

    /// Snapshot of the tip for block execution.
    pub fn tip(&self) -> Tip {
        let n = self.rows.len();
        let start = n.saturating_sub(vinx_core::amount::MEDIAN_TIME_BLOCKS - 1);
        Tip {
            height: self.tip_height(),
            hash: self.tip_hash(),
            timestamp: self.tip_timestamp(),
            commit: self.tip_commit().cloned(),
            recent_timestamps: self.rows[start..]
                .iter()
                .map(|r| r.block.header.timestamp)
                .collect(),
        }
    }

    /// Median Time Past: median of the last `MEDIAN_TIME_BLOCKS` block timestamps
    /// (ADR 0005).
    pub fn median_time_past(&self) -> u64 {
        let n = self.rows.len();
        let start = n.saturating_sub(vinx_core::amount::MEDIAN_TIME_BLOCKS);
        let mut ts: Vec<u64> = self.rows[start..]
            .iter()
            .map(|r| r.block.header.timestamp)
            .collect();
        if ts.is_empty() {
            return 0;
        }
        ts.sort_unstable();
        ts[ts.len() / 2]
    }

    /// Median Time Past once a block carrying `next_ts` is appended — the **protocol
    /// clock** of the block being applied (ADR 0005). Deterministic across nodes.
    pub fn median_time_past_with(&self, next_ts: u64) -> u64 {
        self.tip().median_time_past_with(next_ts)
    }

    pub fn get_block(&self, height: u64) -> Option<&Block> {
        self.row(height).map(|r| &r.block)
    }

    /// Commit certificate of the block at `height`, when stored.
    pub fn get_commit(&self, height: u64) -> Option<&CommitCert> {
        self.row(height).and_then(|r| r.commit.as_ref())
    }

    /// Appends a committed block with its certificate and returns its hash.
    pub fn push(&mut self, block: Block, commit: Option<CommitCert>) -> Hash32 {
        let height = block.header.height;
        for (idx, tx) in block.transactions.iter().enumerate() {
            let tx_hash = tx.hash();
            let sender = tx.sender();
            self.tx_index.insert(tx_hash, (height, idx as u32));
            self.account_tx_index
                .entry(sender)
                .or_default()
                .push(tx_hash);
            if tx.to != sender {
                self.account_tx_index
                    .entry(tx.to)
                    .or_default()
                    .push(tx_hash);
            }
        }
        let hash = block.hash();
        self.rows.push(ChainRow {
            hash,
            block,
            commit,
        });
        self.dirty_heights.insert(height);
        hash
    }

    /// Exports both tx indexes for external persistence.
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
        for row in &self.rows {
            let height = row.block.header.height;
            for (idx, tx) in row.block.transactions.iter().enumerate() {
                let tx_hash = tx.hash();
                let sender = tx.sender();
                self.tx_index.insert(tx_hash, (height, idx as u32));
                self.account_tx_index
                    .entry(sender)
                    .or_default()
                    .push(tx_hash);
                if tx.to != sender {
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
                if offset >= hashes.len() {
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
        let block = self.get_block(height)?;
        let tx = block.transactions.get(tx_pos as usize)?;
        Some((height, block, tx))
    }

    /// Number of stored blocks.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// True when the chain holds no blocks (never the case after genesis).
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Retention pruning (ADR 0083, L3): deletes whole blocks older than `retain_secs`
    /// before `now_ts` — header, transactions and certificate — and advances the chain
    /// base. The tip is always kept. Returns the number of blocks removed; the storage
    /// layer deletes their rows on the next write (`take_pruned_below`).
    pub fn prune_before(&mut self, now_ts: u64, retain_secs: u64) -> u64 {
        let cutoff = now_ts.saturating_sub(retain_secs);
        let keep_from = self
            .rows
            .iter()
            .position(|r| r.block.header.timestamp >= cutoff)
            .unwrap_or(self.rows.len())
            // Never drop the blocks the protocol clock needs (MTP window, tip included).
            .min(
                self.rows
                    .len()
                    .saturating_sub(vinx_core::amount::MEDIAN_TIME_BLOCKS),
            );
        if keep_from == 0 {
            return 0;
        }
        self.rows.drain(..keep_from);
        let removed = keep_from as u64;
        self.height_base += removed;
        self.dirty_heights.retain(|h| *h >= self.height_base);
        self.pruned_below = Some(self.height_base);
        self.rebuild_tx_index();
        tracing::info!(
            removed,
            base = self.height_base,
            "Chain: blocks outside the retention window deleted"
        );
        removed
    }

    /// Height below which stored rows must be deleted, set by `prune_before` and
    /// drained by persistence.
    pub fn take_pruned_below(&mut self) -> Option<u64> {
        self.pruned_below.take()
    }

    /// Lowest height still held (older blocks were pruned or precede a snapshot).
    pub fn base_height(&self) -> u64 {
        self.height_base
    }

    /// Drops transaction data from blocks older than `keep_last` blocks. Headers and
    /// certificates are retained to preserve hash linkage.
    pub fn prune(&mut self, keep_last: u64) {
        let tip = self.tip_height();
        if tip < keep_last {
            return;
        }
        let up_to = (tip - keep_last).saturating_sub(self.height_base) as usize;
        let mut tx_pruned = 0usize;
        for i in 0..up_to {
            if let Some(row) = self.rows.get_mut(i) {
                if !row.block.transactions.is_empty() {
                    tx_pruned += row.block.transactions.len();
                    row.block.transactions.clear();
                    self.dirty_heights.insert(self.height_base + i as u64);
                }
            }
        }
        self.rebuild_tx_index();
        tracing::info!(tip, pruned_below = up_to, tx_pruned, "Chain pruned");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn validator() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    fn block_on(chain: &Chain, ts: u64, txs: Vec<Transaction>) -> Block {
        Block {
            header: BlockHeader {
                height: chain.tip_height() + 1,
                round: 0,
                prev_hash: chain.tip_hash(),
                timestamp: ts,
                validator: validator(),
                tx_count: txs.len() as u32,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
                last_commit_hash: [0u8; 32],
                version: 0,
            },
            transactions: txs,
            last_commit: None,
        }
    }

    #[test]
    fn test_prune_before_drops_old_blocks_but_keeps_the_clock_window() {
        let (mut chain, _) = Chain::new_with_genesis(validator(), 0);
        for h in 1..=50u64 {
            let b = block_on(&chain, h * 10, vec![]);
            chain.push(b, None);
        }
        let tip = chain.tip();
        // Everything before ts 300 goes: heights 0..=29.
        assert_eq!(chain.prune_before(500, 200), 30);
        assert_eq!(chain.base_height(), 30);
        assert_eq!(chain.take_pruned_below(), Some(30));
        assert!(chain.get_block(29).is_none());
        assert_eq!(chain.get_block(30).unwrap().header.timestamp, 300);
        assert_eq!(chain.tip_height(), 50);
        assert_eq!(chain.tip(), tip, "the protocol clock is unchanged");

        // An aggressive window still keeps the MTP window (tip included).
        chain.prune_before(10_000, 0);
        assert_eq!(chain.len(), vinx_core::amount::MEDIAN_TIME_BLOCKS);
        assert_eq!(chain.tip(), tip);
    }

    #[test]
    fn test_snapshot_chain_has_the_same_clock_as_the_full_chain() {
        let (mut chain, _) = Chain::new_with_genesis(validator(), 0);
        for h in 1..=30u64 {
            let b = block_on(&chain, h * 7 + (h % 3), vec![]);
            chain.push(b, None);
        }
        let tip_block = chain.get_block(30).unwrap().clone();
        let ancestors = chain.tip_ancestors();
        assert_eq!(ancestors.len(), vinx_core::amount::MEDIAN_TIME_BLOCKS - 1);
        assert!(Chain::ancestors_link(&ancestors, &tip_block));
        let snap = Chain::new_from_snapshot(ancestors.clone(), tip_block.clone(), None);
        assert_eq!(snap.tip(), chain.tip());
        assert_eq!(
            snap.median_time_past_with(1_000),
            chain.median_time_past_with(1_000)
        );

        // A forged ancestor breaks the hash chain.
        let mut forged = ancestors;
        forged[3].header.timestamp += 1;
        assert!(!Chain::ancestors_link(&forged, &tip_block));
    }

    #[test]
    fn test_genesis_height_is_zero_and_final() {
        let (chain, genesis) = Chain::new_with_genesis(validator(), 0);
        assert_eq!(chain.tip_height(), 0);
        assert_eq!(chain.finalized_height(), 0);
        assert_eq!(chain.tip_hash(), genesis.hash());
        assert_eq!(
            chain.get_block(0).unwrap().header.prev_hash,
            GENESIS_PREV_HASH
        );
        assert!(chain.get_block(999).is_none());
    }

    #[test]
    fn test_push_keeps_the_commit_and_advances_tip() {
        let (mut chain, _) = Chain::new_with_genesis(validator(), 0);
        let b = block_on(&chain, 10, vec![]);
        let cert = CommitCert {
            height: 1,
            round: 0,
            block_hash: b.hash(),
            bitmap: vec![1],
            aggregate: vec![0; 96],
        };
        let h = chain.push(b, Some(cert.clone()));
        assert_eq!(chain.tip_height(), 1);
        assert_eq!(chain.tip_hash(), h);
        assert_eq!(chain.finalized_height(), 1, "every stored block is final");
        assert_eq!(chain.tip_commit(), Some(&cert));
        assert_eq!(chain.get_commit(1), Some(&cert));
    }

    #[test]
    fn test_median_time_past_is_the_median() {
        let (mut chain, _) = Chain::new_with_genesis(validator(), 0);
        for ts in [5, 1, 9, 3, 7] {
            let b = block_on(&chain, ts, vec![]);
            chain.push(b, None);
        }
        // timestamps: 0, 5, 1, 9, 3, 7 → sorted 0 1 3 5 7 9 → median index 3 = 5
        assert_eq!(chain.median_time_past(), 5);
    }

    #[test]
    fn test_median_time_past_with_resists_timestamp_jump() {
        let (mut chain, _) = Chain::new_with_genesis(validator(), 100);
        for i in 1..=10u64 {
            let b = block_on(&chain, 100 + i, vec![]);
            chain.push(b, None);
        }
        let mtp = chain.median_time_past_with(u64::MAX / 2);
        assert!(
            mtp < 200,
            "one far-future header cannot drag the median: {mtp}"
        );
    }

    #[test]
    fn test_prune_drops_tx_data_but_keeps_headers_and_commits() {
        let (mut chain, _) = Chain::new_with_genesis(validator(), 0);
        let kp = KeyPair::generate();
        for i in 1..=5u64 {
            let tx = Transaction::new_transfer(
                &kp,
                validator(),
                vinx_core::Amount::from_vinx(1),
                vinx_core::Amount::from_vinx(1),
                i,
            );
            let b = block_on(&chain, i, vec![tx]);
            let cert = CommitCert {
                height: i,
                round: 0,
                block_hash: b.hash(),
                bitmap: vec![1],
                aggregate: vec![0; 96],
            };
            chain.push(b, Some(cert));
        }
        let hash_before = chain.row(1).unwrap().hash;
        chain.prune(2);
        assert!(chain.get_block(1).unwrap().transactions.is_empty());
        assert_eq!(chain.row(1).unwrap().hash, hash_before, "header hash kept");
        assert!(chain.get_commit(1).is_some(), "commit kept");
        assert_eq!(chain.get_block(5).unwrap().transactions.len(), 1);
    }
}
