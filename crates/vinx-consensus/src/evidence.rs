//! Preuves d'équivoque (SPEC §7.5).
//!
//! Un validateur honnête ne signe jamais deux contenus différents pour la même
//! étape. Deux messages signés qui se contredisent sont donc une preuve
//! irréfutable de faute (clé compromise ou nœud malveillant), vérifiable par
//! quiconque connaît la clé publique du validateur.

use crate::header::proposer_index;
use crate::messages::{SignedProposal, SignedVote};
use borsh::{BorshDeserialize, BorshSerialize};
use thiserror::Error;
use vinx_core::{VoteKind, encode};
use vinx_crypto::{Hash, PublicKey};

// Les preuves sont rares : la taille de la variante la plus grande importe peu.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum Evidence {
    /// Deux votes du même validateur, même hauteur, round et type, cibles différentes.
    Votes(SignedVote, SignedVote) = 0x01,
    /// Deux propositions du proposeur d'un round, pour des blocs différents.
    Proposals(Box<SignedProposal>, Box<SignedProposal>) = 0x02,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EvidenceError {
    #[error("les deux messages ne portent pas sur la même étape")]
    DifferentStep,
    #[error("les deux messages ont le même contenu")]
    SameContent,
    #[error("validateur inconnu")]
    UnknownValidator,
    #[error("signature invalide")]
    BadSignature,
}

impl Evidence {
    pub fn height(&self) -> u64 {
        match self {
            Evidence::Votes(a, _) => a.vote.height,
            Evidence::Proposals(a, _) => a.proposal.height,
        }
    }

    pub fn round(&self) -> u32 {
        match self {
            Evidence::Votes(a, _) => a.vote.round,
            Evidence::Proposals(a, _) => a.proposal.round,
        }
    }

    /// Type d'étape : `"prevote"`, `"precommit"` ou `"proposition"`.
    pub fn step(&self) -> &'static str {
        match self {
            Evidence::Votes(a, _) => match a.vote.kind {
                VoteKind::Prevote => "prevote",
                VoteKind::Precommit => "precommit",
            },
            Evidence::Proposals(..) => "proposition",
        }
    }

    /// Indice du validateur fautif dans l'ensemble de la hauteur.
    pub fn validator(&self, validators: usize) -> Option<u16> {
        match self {
            Evidence::Votes(a, _) => Some(a.validator),
            Evidence::Proposals(a, _) => {
                proposer_index(a.proposal.height, a.proposal.round, validators)
            }
        }
    }

    /// Identifiant indépendant de l'ordre des deux messages.
    pub fn id(&self) -> Hash {
        let (a, b) = match self {
            Evidence::Votes(a, b) => (encode(a), encode(b)),
            Evidence::Proposals(a, b) => (encode(a), encode(b)),
        };
        let (first, second) = if a <= b { (a, b) } else { (b, a) };
        *blake3::hash(&[first, second].concat()).as_bytes()
    }

    /// Vérifie la preuve contre l'ensemble de validateurs de sa hauteur.
    pub fn verify(&self, chain_id: &str, validators: &[PublicKey]) -> Result<(), EvidenceError> {
        match self {
            Evidence::Votes(a, b) => {
                let (va, vb) = (&a.vote, &b.vote);
                let same_step = a.validator == b.validator
                    && va.chain_id == chain_id
                    && vb.chain_id == chain_id
                    && va.height == vb.height
                    && va.round == vb.round
                    && va.kind == vb.kind;
                if !same_step {
                    return Err(EvidenceError::DifferentStep);
                }
                if va.block_hash == vb.block_hash {
                    return Err(EvidenceError::SameContent);
                }
                let key = validators
                    .get(usize::from(a.validator))
                    .ok_or(EvidenceError::UnknownValidator)?;
                if a.verify(key) && b.verify(key) {
                    Ok(())
                } else {
                    Err(EvidenceError::BadSignature)
                }
            }
            Evidence::Proposals(a, b) => {
                let (pa, pb) = (&a.proposal, &b.proposal);
                let same_step = pa.chain_id == chain_id
                    && pb.chain_id == chain_id
                    && pa.height == pb.height
                    && pa.round == pb.round;
                if !same_step {
                    return Err(EvidenceError::DifferentStep);
                }
                if pa.block.hash() == pb.block.hash() && pa.pol_round == pb.pol_round {
                    return Err(EvidenceError::SameContent);
                }
                let key = proposer_index(pa.height, pa.round, validators.len())
                    .and_then(|i| validators.get(usize::from(i)))
                    .ok_or(EvidenceError::UnknownValidator)?;
                if a.verify(key) && b.verify(key) {
                    Ok(())
                } else {
                    Err(EvidenceError::BadSignature)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::Vote;
    use vinx_crypto::KeyPair;

    fn vote(target: Option<Hash>) -> Vote {
        Vote {
            chain_id: "vinx-test".into(),
            kind: VoteKind::Precommit,
            height: 5,
            round: 1,
            block_hash: target,
        }
    }

    #[test]
    fn conflicting_votes_are_proof() {
        let keys: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
        let pks: Vec<PublicKey> = keys.iter().map(KeyPair::public_key).collect();
        let a = SignedVote::new(vote(Some([1; 32])), 2, &keys[2]);
        let b = SignedVote::new(vote(None), 2, &keys[2]);
        let ev = Evidence::Votes(a.clone(), b.clone());
        assert_eq!(ev.verify("vinx-test", &pks), Ok(()));
        assert_eq!(ev.validator(4), Some(2));
        // L'ordre des messages ne change pas l'identifiant.
        assert_eq!(ev.id(), Evidence::Votes(b, a.clone()).id());
        // Même contenu : pas une preuve.
        assert_eq!(
            Evidence::Votes(a.clone(), a.clone()).verify("vinx-test", &pks),
            Err(EvidenceError::SameContent)
        );
    }

    #[test]
    fn forged_or_mismatched_evidence_is_rejected() {
        let keys: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
        let pks: Vec<PublicKey> = keys.iter().map(KeyPair::public_key).collect();
        // Signée par une autre clé que celle du validateur accusé.
        let a = SignedVote::new(vote(Some([1; 32])), 2, &keys[2]);
        let forged = SignedVote::new(vote(None), 2, &keys[3]);
        assert_eq!(
            Evidence::Votes(a.clone(), forged).verify("vinx-test", &pks),
            Err(EvidenceError::BadSignature)
        );
        // Deux validateurs différents.
        let other = SignedVote::new(vote(None), 3, &keys[3]);
        assert_eq!(
            Evidence::Votes(a.clone(), other).verify("vinx-test", &pks),
            Err(EvidenceError::DifferentStep)
        );
        // Rounds différents : un validateur peut changer d'avis d'un round à l'autre.
        let mut later = vote(None);
        later.round = 2;
        let later = SignedVote::new(later, 2, &keys[2]);
        assert_eq!(
            Evidence::Votes(a, later).verify("vinx-test", &pks),
            Err(EvidenceError::DifferentStep)
        );
    }
}
