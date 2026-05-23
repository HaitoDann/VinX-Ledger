use crate::transaction::Transaction;
use serde::{Deserialize, Serialize};
use vinx_crypto::{sha256, Address, Hash32};

pub const GENESIS_PREV_HASH: Hash32 = [0u8; 32];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockHeader {
    pub height: u64,
    pub prev_hash: Hash32,
    /// Unix timestamp in seconds.
    pub timestamp: u64,
    pub validator: Address,
    pub tx_count: u32,
    /// Merkle root of the account state after this block (placeholder for now).
    pub state_root: Hash32,
}

impl BlockHeader {
    pub fn hash(&self) -> Hash32 {
        let mut bytes = Vec::with_capacity(128);
        bytes.extend_from_slice(&self.height.to_be_bytes());
        bytes.extend_from_slice(&self.prev_hash);
        bytes.extend_from_slice(&self.timestamp.to_be_bytes());
        bytes.extend_from_slice(self.validator.as_str().as_bytes());
        bytes.extend_from_slice(&self.tx_count.to_be_bytes());
        bytes.extend_from_slice(&self.state_root);
        sha256(&bytes)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
}

impl Block {
    pub fn hash(&self) -> Hash32 {
        self.header.hash()
    }

    pub fn is_genesis(&self) -> bool {
        self.header.height == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn dummy_validator() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    fn make_genesis_header() -> BlockHeader {
        BlockHeader {
            height: 0,
            prev_hash: GENESIS_PREV_HASH,
            timestamp: 1_748_736_000,
            validator: dummy_validator(),
            tx_count: 0,
            state_root: [0u8; 32],
        }
    }

    #[test]
    fn test_block_hash_is_deterministic() {
        let h = make_genesis_header();
        assert_eq!(h.hash(), h.hash());
    }

    #[test]
    fn test_different_heights_different_hashes() {
        let mut h1 = make_genesis_header();
        let mut h2 = make_genesis_header();
        h1.height = 0;
        h2.height = 1;
        assert_ne!(h1.hash(), h2.hash());
    }

    #[test]
    fn test_genesis_block_flags() {
        let block = Block {
            header: make_genesis_header(),
            transactions: vec![],
        };
        assert!(block.is_genesis());
        assert_eq!(block.header.prev_hash, GENESIS_PREV_HASH);
    }

    #[test]
    fn test_genesis_prev_hash_is_zero() {
        assert_eq!(GENESIS_PREV_HASH, [0u8; 32]);
    }
}
