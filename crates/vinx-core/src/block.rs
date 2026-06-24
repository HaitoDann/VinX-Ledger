use crate::transaction::Transaction;
use crate::validator_set::ValidatorSet;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use vinx_crypto::{sha256, Address, Hash32, PublicKey, VinxSignature};

pub const GENESIS_PREV_HASH: Hash32 = [0u8; 32];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockHeader {
    pub height: u64,
    pub prev_hash: Hash32,
    /// Unix timestamp in seconds.
    pub timestamp: u64,
    pub validator: Address,
    pub tx_count: u32,
    /// Merkle root of the account state after this block.
    pub state_root: Hash32,
    /// Dynamic fee floor at block production time, in atoms.
    #[serde(default)]
    pub base_fee: u64,
    /// SHA-256 Merkle root of transaction receipts in this block.
    #[serde(default)]
    pub receipts_root: Hash32,
}

impl BlockHeader {
    pub fn hash(&self) -> Hash32 {
        let mut bytes = Vec::with_capacity(176);
        bytes.extend_from_slice(&self.height.to_be_bytes());
        bytes.extend_from_slice(&self.prev_hash);
        bytes.extend_from_slice(&self.timestamp.to_be_bytes());
        bytes.extend_from_slice(self.validator.as_str().as_bytes());
        bytes.extend_from_slice(&self.tx_count.to_be_bytes());
        bytes.extend_from_slice(&self.state_root);
        bytes.extend_from_slice(&self.base_fee.to_be_bytes());
        bytes.extend_from_slice(&self.receipts_root);
        sha256(&bytes)
    }
}

/// One validator's co-signature on a block header hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockSignature {
    /// Address of the signing validator.
    pub validator: Address,
    /// Ed25519 public key — carried alongside the signature so that verification
    /// does not require a separate public-key registry lookup.
    pub pub_key: PublicKey,
    /// Signature over `BlockHeader::hash()`.
    pub signature: VinxSignature,
}

/// Evidence of validator equivocation: two valid signatures by the same validator
/// on different block hashes at the same height. Used in SlashValidator transactions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlashEvidence {
    pub height: u64,
    pub sig_a: BlockSignature,
    pub sig_b: BlockSignature,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
    /// Co-signatures from validators. A block is finalized once
    /// `valid_signer_count(validator_set) >= validator_set.quorum()`.
    pub signatures: Vec<BlockSignature>,
}

impl Block {
    /// Hash of the block header (used as the message validators sign).
    pub fn hash(&self) -> Hash32 {
        self.header.hash()
    }

    pub fn is_genesis(&self) -> bool {
        self.header.height == 0
    }

    /// Counts distinct valid co-signatures from registered validators.
    pub fn valid_signer_count(&self, validator_set: &ValidatorSet) -> usize {
        let header_hash = self.hash();
        let mut seen: HashSet<String> = HashSet::new();
        self.signatures
            .iter()
            .filter(|sig| {
                validator_set.contains(&sig.validator)
                    && Address::from_public_key(&sig.pub_key) == sig.validator
                    && sig.pub_key.verify(&header_hash, &sig.signature).is_ok()
                    && seen.insert(sig.validator.as_str().to_string())
            })
            .count()
    }

    /// Returns true when this block has enough valid signatures (≥ quorum).
    /// The genesis block (height 0) is always considered finalized.
    pub fn is_finalized(&self, validator_set: &ValidatorSet) -> bool {
        self.is_genesis() || self.valid_signer_count(validator_set) >= validator_set.quorum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn dummy_addr() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    fn make_genesis_header() -> BlockHeader {
        BlockHeader {
            height: 0,
            prev_hash: GENESIS_PREV_HASH,
            timestamp: 1_748_736_000,
            validator: dummy_addr(),
            tx_count: 0,
            state_root: [0u8; 32],
            base_fee: 0,
            receipts_root: [0u8; 32],
        }
    }

    fn make_block(height: u64, proposer: Address) -> Block {
        Block {
            header: BlockHeader {
                height,
                prev_hash: [0u8; 32],
                timestamp: 0,
                validator: proposer,
                tx_count: 0,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
            },
            transactions: vec![],
            signatures: vec![],
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
            signatures: vec![],
        };
        assert!(block.is_genesis());
        assert_eq!(block.header.prev_hash, GENESIS_PREV_HASH);
    }

    #[test]
    fn test_genesis_prev_hash_is_zero() {
        assert_eq!(GENESIS_PREV_HASH, [0u8; 32]);
    }

    #[test]
    fn test_genesis_always_finalized() {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        let vs = ValidatorSet::single(addr.clone());
        let genesis = make_block(0, addr);
        assert!(genesis.is_finalized(&vs));
    }

    #[test]
    fn test_block_finalized_single_validator() {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        let vs = ValidatorSet::single(addr.clone());
        let mut block = make_block(1, addr.clone());

        assert!(!block.is_finalized(&vs));

        let header_hash = block.hash();
        block.signatures.push(BlockSignature {
            validator: addr,
            pub_key: kp.public_key(),
            signature: kp.sign(&header_hash),
        });
        assert!(block.is_finalized(&vs));
    }

    #[test]
    fn test_pubkey_mismatch_signature_rejected() {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        let vs = ValidatorSet::single(addr.clone());
        let mut block = make_block(1, addr.clone());

        let wrong_kp = KeyPair::generate();
        let header_hash = block.hash();
        block.signatures.push(BlockSignature {
            validator: addr,
            pub_key: wrong_kp.public_key(), // address won't match
            signature: wrong_kp.sign(&header_hash),
        });
        assert!(!block.is_finalized(&vs));
    }

    #[test]
    fn test_duplicate_signatures_counted_once() {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        let other = Address::from_public_key(&KeyPair::generate().public_key());
        let vs = ValidatorSet::new(vec![addr.clone(), other]); // quorum = 2

        let mut block = make_block(1, addr.clone());
        let header_hash = block.hash();

        for _ in 0..3 {
            block.signatures.push(BlockSignature {
                validator: addr.clone(),
                pub_key: kp.public_key(),
                signature: kp.sign(&header_hash),
            });
        }

        assert_eq!(block.valid_signer_count(&vs), 1);
        assert!(!block.is_finalized(&vs)); // quorum=2, only 1 unique signer
    }

    #[test]
    fn test_non_validator_signature_ignored() {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        let vs = ValidatorSet::single(addr.clone());

        let outsider = KeyPair::generate();
        let outsider_addr = Address::from_public_key(&outsider.public_key());
        let mut block = make_block(1, addr);
        let header_hash = block.hash();
        block.signatures.push(BlockSignature {
            validator: outsider_addr,
            pub_key: outsider.public_key(),
            signature: outsider.sign(&header_hash),
        });

        assert_eq!(block.valid_signer_count(&vs), 0);
        assert!(!block.is_finalized(&vs));
    }
}
