use crate::transaction::Transaction;
use crate::validator_set::ValidatorSet;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use vinx_crypto::{
    bls_verify_aggregate, sha256, Address, BlsError, BlsPubKey, BlsSignature, Hash32, PublicKey,
    VinxSignature,
};

pub const GENESIS_PREV_HASH: Hash32 = [0u8; 32];

#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
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
        bytes.extend_from_slice(self.validator.as_bytes());
        bytes.extend_from_slice(&self.tx_count.to_be_bytes());
        bytes.extend_from_slice(&self.state_root);
        bytes.extend_from_slice(&self.base_fee.to_be_bytes());
        bytes.extend_from_slice(&self.receipts_root);
        sha256(&bytes)
    }
}

/// One validator's co-signature on a block header hash.
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct BlockSignature {
    /// Address of the signing validator.
    pub validator: Address,
    /// Ed25519 public key — carried alongside the signature so that verification
    /// does not require a separate public-key registry lookup.
    pub pub_key: PublicKey,
    /// Signature over `BlockHeader::hash()`.
    pub signature: VinxSignature,
}

/// Evidence of validator equivocation: two *different* block headers at the **same
/// height**, each carrying a valid Ed25519 signature from the same validator.
///
/// The full headers are included (not just their hashes) so any verifier can
/// recompute `header_a.hash()` / `header_b.hash()`, confirm the heights match and the
/// hashes differ, and check both signatures. This is what makes a slash *provable* —
/// forging evidence would require forging the target's signature over a real header.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlashEvidence {
    pub header_a: BlockHeader,
    pub header_b: BlockHeader,
    pub sig_a: BlockSignature,
    pub sig_b: BlockSignature,
}

#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
    /// Ed25519 co-signatures (pre-ADR-0046 path, retained for equivocation proofs).
    pub signatures: Vec<BlockSignature>,
    /// BLS12-381 aggregate co-signature (G2, 96 bytes). Present when the block was
    /// co-signed with BLS (ADR 0046 Phase 2+). Takes priority over Ed25519 in
    /// `is_finalized`. Absent on pre-BLS blocks deserialized from older storage.
    #[serde(default)]
    pub bls_aggregate: Option<Vec<u8>>,
    /// BLS G1 public keys (48 bytes each) of the validators whose signatures were
    /// aggregated into `bls_aggregate`. Same order used during aggregation.
    #[serde(default)]
    pub bls_cosigner_pks: Vec<Vec<u8>>,
}

impl Block {
    /// Hash of the block header (used as the message validators sign).
    pub fn hash(&self) -> Hash32 {
        self.header.hash()
    }

    pub fn is_genesis(&self) -> bool {
        self.header.height == 0
    }

    /// Verifies the BLS aggregate signature and returns the cosigner count.
    /// Returns `Ok(0)` when `bls_aggregate` is absent; `Err` on invalid aggregate or
    /// on malformed byte lengths (expected 96 bytes for sig, 48 bytes per pubkey).
    pub fn bls_signer_count(&self) -> Result<usize, BlsError> {
        let agg_vec = match &self.bls_aggregate {
            Some(b) => b,
            None => return Ok(0),
        };
        if self.bls_cosigner_pks.is_empty() {
            return Err(BlsError::EmptyAggregate);
        }
        let agg_arr: [u8; 96] = agg_vec
            .as_slice()
            .try_into()
            .map_err(|_| BlsError::InvalidSignature)?;
        let agg_sig = BlsSignature(agg_arr);
        let pks: Vec<BlsPubKey> = self
            .bls_cosigner_pks
            .iter()
            .map(|pk_vec| {
                let arr: [u8; 48] = pk_vec
                    .as_slice()
                    .try_into()
                    .map_err(|_| BlsError::InvalidKey)?;
                BlsPubKey::from_bytes(&arr)
            })
            .collect::<Result<_, _>>()?;
        bls_verify_aggregate(&pks, &agg_sig, &self.header.hash())?;
        Ok(pks.len())
    }

    /// Counts distinct valid co-signatures from registered validators.
    pub fn valid_signer_count(&self, validator_set: &ValidatorSet) -> usize {
        let header_hash = self.hash();
        let mut seen: HashSet<Address> = HashSet::new();
        self.signatures
            .iter()
            .filter(|sig| {
                validator_set.contains(&sig.validator)
                    && Address::from_public_key(&sig.pub_key) == sig.validator
                    && sig.pub_key.verify(&header_hash, &sig.signature).is_ok()
                    && seen.insert(sig.validator)
            })
            .count()
    }

    /// Returns true when this block has enough valid signatures (≥ quorum).
    /// When a BLS aggregate is present it is verified and the cosigner count is used;
    /// otherwise falls back to Ed25519 co-signatures (pre-ADR-0046 blocks).
    pub fn is_finalized(&self, validator_set: &ValidatorSet) -> bool {
        if self.is_genesis() {
            return true;
        }
        let quorum = validator_set.quorum();
        if self.bls_aggregate.is_some() {
            self.bls_signer_count()
                .map(|c| c >= quorum)
                .unwrap_or(false)
        } else {
            self.valid_signer_count(validator_set) >= quorum
        }
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
            bls_aggregate: None,
            bls_cosigner_pks: vec![],
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
            bls_aggregate: None,
            bls_cosigner_pks: vec![],
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
    fn test_block_header_hash_golden_vector() {
        // ADR 0020 t2: BlockHeader::hash() is the exact message validators co-sign. Its
        // manual big-endian layout is consensus-critical across implementations — pin the
        // hash of a fixed header so any layout change (field order, width, extra field) is
        // a conscious, breaking act caught here.
        let h = BlockHeader {
            height: 5,
            prev_hash: [0u8; 32],
            timestamp: 7,
            validator: Address::from_bytes([0x11; 20]),
            tx_count: 2,
            state_root: [0xAB; 32],
            base_fee: 42,
            receipts_root: [0xCD; 32],
        };
        assert_eq!(
            hex::encode(h.hash()),
            "9456af3f0cb75ac60b2a5559e918a9f7eae17198361f5a3cdebc9383e7bf2640"
        );
    }

    #[test]
    fn test_slash_evidence_encoding_is_canonical() {
        // ADR 0020: SlashEvidence enters the SlashValidator transaction payload —
        // consensus-critical. Pin that its bincode encoding is deterministic and
        // canonical (decode then re-encode is byte-identical).
        let kp = KeyPair::generate();
        let v = Address::from_public_key(&kp.public_key());
        let mk = |tag: u8| {
            let h = BlockHeader {
                height: 5,
                prev_hash: GENESIS_PREV_HASH,
                timestamp: 7,
                validator: v,
                tx_count: 0,
                state_root: [tag; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
            };
            let sig = BlockSignature {
                validator: v,
                pub_key: kp.public_key(),
                signature: kp.sign(&h.hash()),
            };
            (h, sig)
        };
        let (header_a, sig_a) = mk(0xAA);
        let (header_b, sig_b) = mk(0xBB);
        let ev = SlashEvidence {
            header_a,
            header_b,
            sig_a,
            sig_b,
        };
        let bytes = bincode::serialize(&ev).unwrap();
        assert_eq!(bytes, bincode::serialize(&ev).unwrap());
        let decoded: SlashEvidence = bincode::deserialize(&bytes).unwrap();
        assert_eq!(bincode::serialize(&decoded).unwrap(), bytes);
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

    #[test]
    fn test_bls_single_sig_finalizes_block() {
        use vinx_crypto::BlsSecretKey;
        let ed_kp = KeyPair::generate();
        let addr = Address::from_public_key(&ed_kp.public_key());
        let vs = ValidatorSet::single(addr.clone());

        let mut block = make_block(1, addr);
        let header_hash = block.hash();

        let bls_sk = BlsSecretKey::generate();
        let bls_sig = bls_sk.sign(&header_hash);
        // Use the public-API aggregate for proper encoding (aggregate of 1 = the sig itself).
        let agg = vinx_crypto::bls_aggregate(&[bls_sig]).unwrap();
        block.bls_aggregate = Some(agg.0.to_vec());
        block.bls_cosigner_pks = vec![bls_sk.public_key().0.to_vec()];

        assert_eq!(block.bls_signer_count().unwrap(), 1);
        assert!(block.is_finalized(&vs));
    }

    #[test]
    fn test_bls_wrong_sig_not_finalized() {
        use vinx_crypto::BlsSecretKey;
        let ed_kp = KeyPair::generate();
        let addr = Address::from_public_key(&ed_kp.public_key());
        let vs = ValidatorSet::single(addr.clone());

        let mut block = make_block(1, addr);
        let header_hash = block.hash();

        let bls_sk = BlsSecretKey::generate();
        let wrong_sk = BlsSecretKey::generate();
        // Sign with one key but advertise a different public key.
        let agg = vinx_crypto::bls_aggregate(&[bls_sk.sign(&header_hash)]).unwrap();
        block.bls_aggregate = Some(agg.0.to_vec());
        block.bls_cosigner_pks = vec![wrong_sk.public_key().0.to_vec()];

        assert!(block.bls_signer_count().is_err());
        assert!(!block.is_finalized(&vs));
    }

    #[test]
    fn test_bls_absent_falls_back_to_ed25519() {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        let vs = ValidatorSet::single(addr.clone());
        let mut block = make_block(1, addr.clone());
        let header_hash = block.hash();
        block.signatures.push(BlockSignature {
            validator: addr,
            pub_key: kp.public_key(),
            signature: kp.sign(&header_hash),
        });
        // No bls_aggregate set: should use Ed25519 path.
        assert!(block.bls_aggregate.is_none());
        assert!(block.is_finalized(&vs));
    }
}
