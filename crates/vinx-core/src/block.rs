use crate::consensus::CommitCert;
use crate::transaction::Transaction;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use vinx_crypto::{hash256, Address, Hash32};

pub const GENESIS_PREV_HASH: Hash32 = [0u8; 32];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct BlockHeader {
    pub height: u64,
    /// Consensus round in which this block was **built** by its proposer (ADR 0082). A
    /// round above 0 means the proposers of the earlier rounds did not get a block
    /// through — the fact from which missed proposals are charged, deterministically.
    /// The block may be re-proposed and committed at a later round (its certificate's).
    pub round: u32,
    pub prev_hash: Hash32,
    /// Unix timestamp in seconds.
    pub timestamp: u64,
    /// The proposer of this block.
    pub validator: Address,
    pub tx_count: u32,
    /// Merkle root of the account state after this block.
    pub state_root: Hash32,
    /// Dynamic fee floor at block production time, in atoms.
    pub base_fee: u64,
    /// Root of the ordered transaction hashes of this block.
    pub receipts_root: Hash32,
    /// Hash of `Block::last_commit` (all-zero when absent), so the proposer cannot swap
    /// the certificate of the previous block after proposing.
    pub last_commit_hash: Hash32,
}

impl BlockHeader {
    pub fn hash(&self) -> Hash32 {
        let mut bytes = Vec::with_capacity(212);
        bytes.extend_from_slice(&self.height.to_be_bytes());
        bytes.extend_from_slice(&self.round.to_be_bytes());
        bytes.extend_from_slice(&self.prev_hash);
        bytes.extend_from_slice(&self.timestamp.to_be_bytes());
        bytes.extend_from_slice(self.validator.as_bytes());
        bytes.extend_from_slice(&self.tx_count.to_be_bytes());
        bytes.extend_from_slice(&self.state_root);
        bytes.extend_from_slice(&self.base_fee.to_be_bytes());
        bytes.extend_from_slice(&self.receipts_root);
        bytes.extend_from_slice(&self.last_commit_hash);
        hash256(&bytes)
    }
}

/// A block of the chain (ADR 0082).
///
/// A block is only ever stored once **committed**: its own [`CommitCert`] travels next to
/// it (chain rows, sync), and the certificate of the *previous* block is embedded here as
/// `last_commit`. State transitions read `last_commit` to reward the validators that
/// precommitted the previous block — so rewards derive from data every node holds, not
/// from which signatures a given node happened to receive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
    /// Commit certificate of block `height - 1`. `None` only for heights 0 and 1.
    pub last_commit: Option<CommitCert>,
}

impl Block {
    /// Hash of the block header — what proposals and votes refer to.
    pub fn hash(&self) -> Hash32 {
        self.header.hash()
    }

    pub fn is_genesis(&self) -> bool {
        self.header.height == 0
    }

    /// Hash to put in `header.last_commit_hash` for `cert`.
    pub fn last_commit_hash_of(cert: Option<&CommitCert>) -> Hash32 {
        cert.map(CommitCert::hash).unwrap_or([0u8; 32])
    }

    /// True when `header.last_commit_hash` matches the embedded certificate.
    pub fn last_commit_matches(&self) -> bool {
        self.header.last_commit_hash == Self::last_commit_hash_of(self.last_commit.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn dummy_addr() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    fn make_block(height: u64, proposer: Address) -> Block {
        Block {
            header: BlockHeader {
                height,
                round: 0,
                prev_hash: GENESIS_PREV_HASH,
                timestamp: 1_700_000_000 + height,
                validator: proposer,
                tx_count: 0,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
                last_commit_hash: [0u8; 32],
            },
            transactions: vec![],
            last_commit: None,
        }
    }

    #[test]
    fn test_block_hash_is_deterministic() {
        let b = make_block(1, dummy_addr());
        assert_eq!(b.hash(), b.hash());
    }

    #[test]
    fn test_every_header_field_is_hashed() {
        let base = make_block(1, dummy_addr());
        #[allow(clippy::type_complexity)]
        let mutations: Vec<Box<dyn Fn(&mut BlockHeader)>> = vec![
            Box::new(|h| h.height += 1),
            Box::new(|h| h.round += 1),
            Box::new(|h| h.prev_hash[0] ^= 1),
            Box::new(|h| h.timestamp += 1),
            Box::new(|h| h.validator = Address::from_bytes([9; 20])),
            Box::new(|h| h.tx_count += 1),
            Box::new(|h| h.state_root[0] ^= 1),
            Box::new(|h| h.base_fee += 1),
            Box::new(|h| h.receipts_root[0] ^= 1),
            Box::new(|h| h.last_commit_hash[0] ^= 1),
        ];
        for (i, m) in mutations.iter().enumerate() {
            let mut b = base.clone();
            m(&mut b.header);
            assert_ne!(b.hash(), base.hash(), "header field #{i} must be hashed");
        }
    }

    #[test]
    fn test_genesis_flags() {
        assert!(make_block(0, dummy_addr()).is_genesis());
        assert!(!make_block(1, dummy_addr()).is_genesis());
    }

    #[test]
    fn test_last_commit_hash_binds_the_certificate() {
        let mut b = make_block(2, dummy_addr());
        assert!(b.last_commit_matches());
        b.last_commit = Some(CommitCert {
            height: 1,
            round: 0,
            block_hash: [1; 32],
            bitmap: vec![1],
            aggregate: vec![0; 96],
        });
        assert!(
            !b.last_commit_matches(),
            "a swapped certificate is detected"
        );
        b.header.last_commit_hash = Block::last_commit_hash_of(b.last_commit.as_ref());
        assert!(b.last_commit_matches());
    }

    #[test]
    fn test_block_header_hash_golden_vector() {
        // ADR 0020: the header hash is what every vote signs. Any external implementation
        // must reproduce this exact value; changing it is a consensus-breaking change.
        let h = BlockHeader {
            height: 1,
            round: 2,
            prev_hash: [0x11; 32],
            timestamp: 1_700_000_000,
            validator: Address::from_bytes([0x22; 20]),
            tx_count: 3,
            state_root: [0x33; 32],
            base_fee: 100_000,
            receipts_root: [0x44; 32],
            last_commit_hash: [0x55; 32],
        };
        assert_eq!(
            hex::encode(h.hash()),
            "2e247f90a21e292b61cf77122ecc07acd3c1e21881474feb802cbb9a8f6dd22d"
        );
    }
}
