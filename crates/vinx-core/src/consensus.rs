//! BFT consensus message types (ADR 0082).
//!
//! VinX commits a block only once validators holding **strictly more than 2/3 of the
//! voting power** have precommitted it. This module defines what they sign and what a
//! committed block carries as proof:
//!
//! - [`SignedVote`]: one validator's prevote or precommit for `(height, round, value)`,
//!   signed with its registered BLS key. `value = None` is a vote for *nil*.
//! - [`CommitCert`]: the aggregate of the precommits that committed a block — one BLS
//!   signature (96 bytes) plus a bitmap of signers, verifiable by anyone who knows the
//!   validator set of that height.
//! - [`VoteEquivocation`]: two conflicting votes by the same validator for the same
//!   `(kind, height, round)` — cryptographic proof of misbehaviour, slashable.
//!
//! Everything signed here is domain-separated and bound to the chain id, so a signature
//! can never be replayed across message kinds or networks.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use vinx_crypto::{
    bls_aggregate, bls_verify, bls_verify_aggregate, hash256, Address, BlsError, BlsPubKey,
    BlsSignature, Hash32,
};

use crate::validator_set::ValidatorSet;

/// Domain tag of every consensus signature (votes and proposals).
pub const CONSENSUS_SIGN_DST: &[u8] = b"VINX_CONSENSUS_V1";

/// The two voting steps of a round.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub enum VoteKind {
    Prevote,
    Precommit,
}

impl VoteKind {
    fn tag(self) -> u8 {
        match self {
            VoteKind::Prevote => 1,
            VoteKind::Precommit => 2,
        }
    }
}

/// Tag byte of a proposal signature (distinct from any vote kind).
const PROPOSAL_TAG: u8 = 3;

/// Canonical bytes a validator signs for a vote.
///
/// `DST ‖ chain_id(4 BE) ‖ kind(1) ‖ height(8 BE) ‖ round(4 BE) ‖ value(0 | 1 ‖ hash[32])`
pub fn vote_sign_bytes(
    chain_id: u32,
    kind: VoteKind,
    height: u64,
    round: u32,
    value: Option<&Hash32>,
) -> Vec<u8> {
    let mut b = Vec::with_capacity(CONSENSUS_SIGN_DST.len() + 4 + 1 + 8 + 4 + 33);
    b.extend_from_slice(CONSENSUS_SIGN_DST);
    b.extend_from_slice(&chain_id.to_be_bytes());
    b.push(kind.tag());
    b.extend_from_slice(&height.to_be_bytes());
    b.extend_from_slice(&round.to_be_bytes());
    match value {
        Some(h) => {
            b.push(1);
            b.extend_from_slice(h);
        }
        None => b.push(0),
    }
    b
}

/// Canonical bytes a proposer signs for a proposal. `pol_round` is the round of the
/// proof-of-lock the proposer re-proposes from (`None` for a fresh block).
///
/// `DST ‖ chain_id ‖ 3 ‖ height ‖ round ‖ pol(0 | 1 ‖ round) ‖ block_hash`
pub fn proposal_sign_bytes(
    chain_id: u32,
    height: u64,
    round: u32,
    pol_round: Option<u32>,
    block_hash: &Hash32,
) -> Vec<u8> {
    let mut b = Vec::with_capacity(CONSENSUS_SIGN_DST.len() + 4 + 1 + 8 + 4 + 5 + 32);
    b.extend_from_slice(CONSENSUS_SIGN_DST);
    b.extend_from_slice(&chain_id.to_be_bytes());
    b.push(PROPOSAL_TAG);
    b.extend_from_slice(&height.to_be_bytes());
    b.extend_from_slice(&round.to_be_bytes());
    match pol_round {
        Some(r) => {
            b.push(1);
            b.extend_from_slice(&r.to_be_bytes());
        }
        None => b.push(0),
    }
    b.extend_from_slice(block_hash);
    b
}

/// One validator's signed vote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct SignedVote {
    pub kind: VoteKind,
    pub height: u64,
    pub round: u32,
    /// Block hash voted for, or `None` for nil.
    pub value: Option<Hash32>,
    /// The voting validator (its account address, as in the validator set).
    pub validator: Address,
    /// BLS G2 signature (96 bytes) over [`vote_sign_bytes`].
    pub signature: Vec<u8>,
}

impl SignedVote {
    pub fn sign_bytes(&self, chain_id: u32) -> Vec<u8> {
        vote_sign_bytes(
            chain_id,
            self.kind,
            self.height,
            self.round,
            self.value.as_ref(),
        )
    }

    /// Verifies the signature against `pk` (the validator's registered BLS key).
    pub fn verify(&self, chain_id: u32, pk: &BlsPubKey) -> Result<(), BlsError> {
        let sig: [u8; 96] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| BlsError::InvalidSignature)?;
        bls_verify(pk, &BlsSignature(sig), &self.sign_bytes(chain_id))
    }
}

/// Proof that a block was committed: the aggregated precommits of strictly more than 2/3
/// of the voting power, all for `(height, round, block_hash)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct CommitCert {
    pub height: u64,
    pub round: u32,
    pub block_hash: Hash32,
    /// Bit `i` set ⇔ the validator at index `i` of the voting set precommitted.
    pub bitmap: Vec<u8>,
    /// BLS aggregate (96 bytes) of the signers' precommit signatures.
    pub aggregate: Vec<u8>,
}

/// Why a commit certificate was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CertError {
    #[error("certificate is for another block or height")]
    WrongTarget,
    #[error("bitmap references a validator outside the set")]
    BitmapOutOfRange,
    #[error("a signer has no registered BLS key")]
    MissingKey,
    #[error("two signers share the same BLS key")]
    DuplicateKey,
    #[error("signed power {signed} below the quorum {quorum}")]
    InsufficientPower { signed: u64, quorum: u64 },
    #[error("aggregate signature is invalid")]
    BadSignature,
}

/// Sets bit `idx` of `bitmap`, growing it as needed.
pub fn bitmap_set(bitmap: &mut Vec<u8>, idx: usize) {
    let byte = idx / 8;
    if bitmap.len() <= byte {
        bitmap.resize(byte + 1, 0);
    }
    bitmap[byte] |= 1 << (idx % 8);
}

/// True when bit `idx` of `bitmap` is set.
pub fn bitmap_has(bitmap: &[u8], idx: usize) -> bool {
    bitmap
        .get(idx / 8)
        .is_some_and(|b| b & (1 << (idx % 8)) != 0)
}

/// Indices of every set bit of `bitmap`, ascending.
pub fn bitmap_indices(bitmap: &[u8]) -> Vec<usize> {
    (0..bitmap.len() * 8)
        .filter(|i| bitmap_has(bitmap, *i))
        .collect()
}

impl CommitCert {
    /// Builds a certificate from precommits `(validator_index, signature)` for one value.
    /// Signatures are aggregated in ascending index order.
    pub fn from_precommits(
        height: u64,
        round: u32,
        block_hash: Hash32,
        mut precommits: Vec<(usize, BlsSignature)>,
    ) -> Result<Self, BlsError> {
        precommits.sort_by_key(|(i, _)| *i);
        precommits.dedup_by_key(|(i, _)| *i);
        let mut bitmap = Vec::new();
        for (i, _) in &precommits {
            bitmap_set(&mut bitmap, *i);
        }
        let sigs: Vec<BlsSignature> = precommits.into_iter().map(|(_, s)| s).collect();
        let agg = bls_aggregate(&sigs)?;
        Ok(Self {
            height,
            round,
            block_hash,
            bitmap,
            aggregate: agg.0.to_vec(),
        })
    }

    /// Hash committed in the next block's header (`last_commit_hash`).
    pub fn hash(&self) -> Hash32 {
        hash256(&borsh::to_vec(self).expect("CommitCert serializes"))
    }

    /// Signer indices (ascending).
    pub fn signers(&self) -> Vec<usize> {
        bitmap_indices(&self.bitmap)
    }

    /// Verifies the certificate for block `(height, block_hash)` against the voting set of
    /// that height and its registered BLS keys (`keys[i]` for `vs.validators()[i]`).
    /// Returns the signed power on success.
    pub fn verify(
        &self,
        chain_id: u32,
        height: u64,
        block_hash: &Hash32,
        vs: &ValidatorSet,
        keys: &[Option<[u8; 48]>],
    ) -> Result<u64, CertError> {
        if self.height != height || &self.block_hash != block_hash {
            return Err(CertError::WrongTarget);
        }
        let signers = self.signers();
        if signers.iter().any(|i| *i >= vs.len()) {
            return Err(CertError::BitmapOutOfRange);
        }
        let signed = vs.power_of_indices(signers.iter().copied());
        let quorum = vs.quorum_power();
        if signed < quorum {
            return Err(CertError::InsufficientPower { signed, quorum });
        }
        let mut pks: Vec<BlsPubKey> = Vec::with_capacity(signers.len());
        for i in &signers {
            let raw = keys
                .get(*i)
                .copied()
                .flatten()
                .ok_or(CertError::MissingKey)?;
            let pk = BlsPubKey::from_bytes(&raw).map_err(|_| CertError::MissingKey)?;
            // One bit = one independent key: the same key twice would count one signature
            // twice (VINX-11).
            if pks.iter().any(|p| p.0 == pk.0) {
                return Err(CertError::DuplicateKey);
            }
            pks.push(pk);
        }
        let agg: [u8; 96] = self
            .aggregate
            .as_slice()
            .try_into()
            .map_err(|_| CertError::BadSignature)?;
        let msg = vote_sign_bytes(
            chain_id,
            VoteKind::Precommit,
            self.height,
            self.round,
            Some(&self.block_hash),
        );
        bls_verify_aggregate(&pks, &BlsSignature(agg), &msg)
            .map_err(|_| CertError::BadSignature)?;
        Ok(signed)
    }
}

/// Two conflicting votes by the same validator: same kind, height and round, different
/// values. Honest validators never produce this (their signing state forbids it), so it
/// is proof of equivocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct VoteEquivocation {
    pub vote_a: SignedVote,
    pub vote_b: SignedVote,
}

impl VoteEquivocation {
    /// Structural check (no signatures): the two votes really conflict.
    pub fn is_conflicting(&self) -> bool {
        let (a, b) = (&self.vote_a, &self.vote_b);
        a.validator == b.validator
            && a.kind == b.kind
            && a.height == b.height
            && a.round == b.round
            && a.value != b.value
    }

    /// Full check: conflicting, and both signatures valid under `pk`.
    pub fn verify(&self, chain_id: u32, pk: &BlsPubKey) -> bool {
        self.is_conflicting()
            && self.vote_a.verify(chain_id, pk).is_ok()
            && self.vote_b.verify(chain_id, pk).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::{BlsSecretKey, KeyPair};

    fn keys(n: usize) -> (Vec<BlsSecretKey>, Vec<Address>) {
        let sks: Vec<BlsSecretKey> = (0..n)
            .map(|i| {
                let mut seed = [0u8; 32];
                seed[0] = i as u8 + 1;
                BlsSecretKey::from_bytes(&seed).unwrap()
            })
            .collect();
        let addrs = (0..n)
            .map(|_| Address::from_public_key(&KeyPair::generate().public_key()))
            .collect();
        (sks, addrs)
    }

    fn precommit(sk: &BlsSecretKey, h: u64, r: u32, v: &Hash32) -> BlsSignature {
        sk.sign(&vote_sign_bytes(7, VoteKind::Precommit, h, r, Some(v)))
    }

    fn registry(sks: &[BlsSecretKey]) -> Vec<Option<[u8; 48]>> {
        sks.iter().map(|s| Some(s.public_key().0)).collect()
    }

    #[test]
    fn sign_bytes_are_domain_separated() {
        let h = [9u8; 32];
        let pv = vote_sign_bytes(7, VoteKind::Prevote, 1, 0, Some(&h));
        let pc = vote_sign_bytes(7, VoteKind::Precommit, 1, 0, Some(&h));
        let other_chain = vote_sign_bytes(8, VoteKind::Prevote, 1, 0, Some(&h));
        let nil = vote_sign_bytes(7, VoteKind::Prevote, 1, 0, None);
        let prop = proposal_sign_bytes(7, 1, 0, None, &h);
        let all = [&pv, &pc, &other_chain, &nil, &prop];
        for i in 0..all.len() {
            for j in 0..all.len() {
                if i != j {
                    assert_ne!(all[i], all[j]);
                }
            }
        }
    }

    #[test]
    fn cert_with_quorum_verifies() {
        let (sks, addrs) = keys(4);
        let vs = ValidatorSet::new(addrs);
        let v = [1u8; 32];
        let pcs = (0..3).map(|i| (i, precommit(&sks[i], 5, 2, &v))).collect();
        let cert = CommitCert::from_precommits(5, 2, v, pcs).unwrap();
        assert_eq!(cert.verify(7, 5, &v, &vs, &registry(&sks)), Ok(3));
    }

    #[test]
    fn cert_below_quorum_is_refused() {
        let (sks, addrs) = keys(4);
        let vs = ValidatorSet::new(addrs);
        let v = [1u8; 32];
        let pcs = (0..2).map(|i| (i, precommit(&sks[i], 5, 0, &v))).collect();
        let cert = CommitCert::from_precommits(5, 0, v, pcs).unwrap();
        assert!(matches!(
            cert.verify(7, 5, &v, &vs, &registry(&sks)),
            Err(CertError::InsufficientPower { .. })
        ));
    }

    #[test]
    fn cert_with_foreign_keys_or_wrong_target_is_refused() {
        let (sks, addrs) = keys(4);
        let vs = ValidatorSet::new(addrs);
        let v = [1u8; 32];
        // Signed by keys nobody registered.
        let (foreign, _) = keys(8);
        let pcs = (0..3)
            .map(|i| (i, precommit(&foreign[i + 4], 5, 0, &v)))
            .collect();
        let cert = CommitCert::from_precommits(5, 0, v, pcs).unwrap();
        assert_eq!(
            cert.verify(7, 5, &v, &vs, &registry(&sks)),
            Err(CertError::BadSignature)
        );
        // Honest cert presented for another block / height / chain.
        let pcs = (0..3).map(|i| (i, precommit(&sks[i], 5, 0, &v))).collect();
        let cert = CommitCert::from_precommits(5, 0, v, pcs).unwrap();
        assert_eq!(
            cert.verify(7, 5, &[2u8; 32], &vs, &registry(&sks)),
            Err(CertError::WrongTarget)
        );
        assert_eq!(
            cert.verify(7, 6, &v, &vs, &registry(&sks)),
            Err(CertError::WrongTarget)
        );
        assert_eq!(
            cert.verify(8, 5, &v, &vs, &registry(&sks)),
            Err(CertError::BadSignature)
        );
    }

    #[test]
    fn cert_counts_power_not_heads() {
        // 20 validators: ten of stake 30 (75 % of the power) and ten of stake 10 (25 %).
        // Half the heads can commit when they hold > 2/3 of the power; the other half
        // cannot. (Under the 10 % cap, no fewer than 7 validators can ever reach 2/3.)
        let (sks, addrs) = keys(20);
        let mut stakes = vec![30u64; 10];
        stakes.extend(vec![10u64; 10]);
        let vs = ValidatorSet::with_stakes(addrs, stakes);
        let v = [3u8; 32];
        let big: Vec<_> = (0..10).map(|i| (i, precommit(&sks[i], 1, 0, &v))).collect();
        let small: Vec<_> = (10..20)
            .map(|i| (i, precommit(&sks[i], 1, 0, &v)))
            .collect();
        let reg = registry(&sks);
        let heavy = CommitCert::from_precommits(1, 0, v, big).unwrap();
        assert_eq!(heavy.verify(7, 1, &v, &vs, &reg), Ok(300));
        let light = CommitCert::from_precommits(1, 0, v, small).unwrap();
        assert!(light.verify(7, 1, &v, &vs, &reg).is_err());
    }

    #[test]
    fn equivocation_is_detected_and_verified() {
        let (sks, addrs) = keys(1);
        let mk = |value: Option<Hash32>| {
            let mut v = SignedVote {
                kind: VoteKind::Prevote,
                height: 3,
                round: 1,
                value,
                validator: addrs[0],
                signature: vec![],
            };
            v.signature = sks[0].sign(&v.sign_bytes(7)).0.to_vec();
            v
        };
        let ev = VoteEquivocation {
            vote_a: mk(Some([1; 32])),
            vote_b: mk(None),
        };
        assert!(ev.verify(7, &sks[0].public_key()));
        let same = VoteEquivocation {
            vote_a: mk(Some([1; 32])),
            vote_b: mk(Some([1; 32])),
        };
        assert!(
            !same.verify(7, &sks[0].public_key()),
            "identical votes are not a conflict"
        );
    }
}
