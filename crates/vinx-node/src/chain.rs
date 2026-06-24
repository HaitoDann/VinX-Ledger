use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use vinx_core::{block::GENESIS_PREV_HASH, Block, BlockHeader, Transaction};
use vinx_crypto::{Address, Hash32};

#[derive(Serialize, Deserialize)]
pub struct Chain {
    /// Stored as (block_hash, block) indexed by height.
    blocks: Vec<(Hash32, Block)>,
    /// Maps hex-encoded tx hash -> (block_height, tx_position). Not persisted.
    #[serde(skip)]
    tx_index: HashMap<String, (u64, u32)>,
    /// Maps bech32 address -> ordered list of tx hashes (oldest first). Not persisted.
    #[serde(skip)]
    account_tx_index: HashMap<String, Vec<String>>,
    /// Maps validator addr -> height -> set of block hashes signed (equivocation detection).
    #[serde(skip)]
    slash_evidence: HashMap<String, HashMap<u64, HashSet<Hash32>>>,
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
            tx_index: HashMap::new(),
            account_tx_index: HashMap::new(),
            slash_evidence: HashMap::new(),
        };
        (chain, genesis)
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

    pub fn get_block(&self, height: u64) -> Option<&Block> {
        self.blocks.get(height as usize).map(|(_, b)| b)
    }

    /// Appends a block and returns its hash.
    pub fn push(&mut self, block: Block) -> Hash32 {
        let height = block.header.height;
        for (idx, tx) in block.transactions.iter().enumerate() {
            let hash_hex = hex::encode(tx.hash());
            self.tx_index.insert(hash_hex.clone(), (height, idx as u32));
            self.account_tx_index
                .entry(tx.from.to_string())
                .or_default()
                .push(hash_hex.clone());
            if tx.to != tx.from {
                self.account_tx_index
                    .entry(tx.to.to_string())
                    .or_default()
                    .push(hash_hex);
            }
        }
        let hash = block.hash();
        self.blocks.push((hash, block));
        hash
    }

    /// Clears and repopulates both tx indexes from all stored blocks.
    pub fn rebuild_tx_index(&mut self) {
        self.tx_index.clear();
        self.account_tx_index.clear();
        for (_, block) in &self.blocks {
            let height = block.header.height;
            for (idx, tx) in block.transactions.iter().enumerate() {
                let hash_hex = hex::encode(tx.hash());
                self.tx_index.insert(hash_hex.clone(), (height, idx as u32));
                self.account_tx_index
                    .entry(tx.from.to_string())
                    .or_default()
                    .push(hash_hex.clone());
                if tx.to != tx.from {
                    self.account_tx_index
                        .entry(tx.to.to_string())
                        .or_default()
                        .push(hash_hex);
                }
            }
        }
    }

    /// Total number of transactions involving this address.
    pub fn account_tx_count(&self, addr: &str) -> usize {
        self.account_tx_index.get(addr).map_or(0, |v| v.len())
    }

    /// Returns tx hashes for the given address, newest-first, with pagination.
    pub fn get_account_txs(&self, addr: &str, limit: usize, offset: usize) -> Vec<String> {
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
                    .cloned()
                    .collect()
            }
        }
    }

    /// Looks up a transaction by its hex-encoded hash.
    pub fn get_tx_by_hash(&self, hash: &str) -> Option<(u64, &Block, &Transaction)> {
        let &(height, tx_pos) = self.tx_index.get(hash)?;
        let (_, block) = self.blocks.get(height as usize)?;
        let tx = block.transactions.get(tx_pos as usize)?;
        Some((height, block, tx))
    }

    /// Number of blocks (= tip_height + 1).
    pub fn len(&self) -> usize {
        self.blocks.len()
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
            return block.is_finalized(validator_set);
        }
        false
    }

    /// Records that `validator` signed `block_hash` at `height`.
    /// Returns `true` if equivocation is detected (validator signed a DIFFERENT
    /// block at the same height — a slashable offense).
    pub fn record_signature(&mut self, validator: &str, height: u64, block_hash: Hash32) -> bool {
        let heights = self
            .slash_evidence
            .entry(validator.to_string())
            .or_default();
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
                block.transactions.clear();
            }
        }
        // Rebuild index to remove entries from pruned blocks
        self.rebuild_tx_index();
        tracing::info!(
            compacted = compact_up_to,
            "Chain compacted old transaction data"
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
        let mut chain = Chain {
            blocks: vec![],
            tx_index: HashMap::new(),
            account_tx_index: HashMap::new(),
            slash_evidence: HashMap::new(),
        };
        let addr = "vinx1test000";
        let hash_a = [1u8; 32];
        let hash_b = [2u8; 32];

        assert!(!chain.record_signature(addr, 5, hash_a)); // first sig — ok
        assert!(!chain.record_signature(addr, 5, hash_a)); // same hash — ok (idempotent)
        assert!(chain.record_signature(addr, 5, hash_b)); // different hash — EQUIVOCATION
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
