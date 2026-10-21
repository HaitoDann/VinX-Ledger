//! Votes et certificats de commit (SPEC §6.1, §7).

use crate::{CoreError, encode};
use borsh::{BorshDeserialize, BorshSerialize};
use vinx_crypto::{Domain, Hash, KeyPair, PublicKey, Signature, tagged};

/// Quorum pour `n` validateurs : `⌊2n/3⌋ + 1`.
pub fn quorum(n: usize) -> usize {
    (n.saturating_mul(2) / 3).saturating_add(1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum VoteKind {
    Prevote = 0x01,
    Precommit = 0x02,
}

/// Vote de consensus. `block_hash = None` est un vote `nil`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Vote {
    pub chain_id: String,
    pub kind: VoteKind,
    pub height: u64,
    pub round: u32,
    pub block_hash: Option<Hash>,
}

impl Vote {
    /// Message signé : `"vinx/vote" ‖ borsh(vote)`.
    pub fn signing_message(&self) -> Vec<u8> {
        tagged(Domain::Vote, &encode(self))
    }

    pub fn sign(&self, keypair: &KeyPair) -> Signature {
        keypair.sign(&self.signing_message())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CommitCertificate {
    pub height: u64,
    pub round: u32,
    pub block_hash: Hash,
    /// `(indice du validateur, signature du precommit)`, indices strictement croissants.
    pub signatures: Vec<(u16, Signature)>,
}

impl CommitCertificate {
    /// Vérifie que le certificat porte les precommits d'un quorum de `validators`
    /// pour ce bloc.
    pub fn verify(&self, chain_id: &str, validators: &[PublicKey]) -> Result<(), CoreError> {
        let bad = CoreError::InvalidCertificate;
        if self.signatures.len() < quorum(validators.len()) {
            return Err(bad("quorum non atteint"));
        }
        let message = Vote {
            chain_id: chain_id.to_owned(),
            kind: VoteKind::Precommit,
            height: self.height,
            round: self.round,
            block_hash: Some(self.block_hash),
        }
        .signing_message();
        let mut previous: Option<u16> = None;
        for (index, signature) in &self.signatures {
            if previous.is_some_and(|p| *index <= p) {
                return Err(bad("indices non strictement croissants"));
            }
            previous = Some(*index);
            let key = validators
                .get(usize::from(*index))
                .ok_or(bad("indice de validateur inconnu"))?;
            key.verify(&message, signature)
                .map_err(|_| bad("signature invalide"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quorum_values() {
        let q: Vec<usize> = (1..=7).map(quorum).collect();
        assert_eq!(q, [1, 2, 3, 3, 4, 5, 5]);
        assert_eq!(quorum(100), 67);
    }

    fn setup(n: usize) -> (Vec<KeyPair>, Vec<PublicKey>) {
        let kps: Vec<KeyPair> = (0..n).map(|_| KeyPair::generate()).collect();
        let pks = kps.iter().map(KeyPair::public_key).collect();
        (kps, pks)
    }

    fn cert(kps: &[KeyPair], signers: &[u16], kind: VoteKind, hash: Hash) -> CommitCertificate {
        let vote = Vote {
            chain_id: "vinx-test".into(),
            kind,
            height: 10,
            round: 0,
            block_hash: Some(hash),
        };
        CommitCertificate {
            height: 10,
            round: 0,
            block_hash: [7; 32],
            signatures: signers
                .iter()
                .map(|&i| (i, vote.sign(&kps[usize::from(i)])))
                .collect(),
        }
    }

    #[test]
    fn valid_quorum_is_accepted() {
        let (kps, pks) = setup(4);
        let c = cert(&kps, &[0, 2, 3], VoteKind::Precommit, [7; 32]);
        assert_eq!(c.verify("vinx-test", &pks), Ok(()));
    }

    #[test]
    fn invalid_certificates_are_rejected() {
        let (kps, pks) = setup(4);
        let p = VoteKind::Precommit;
        // Sous le quorum.
        assert!(
            cert(&kps, &[0, 1], p, [7; 32])
                .verify("vinx-test", &pks)
                .is_err()
        );
        // Prevotes au lieu de precommits.
        assert!(
            cert(&kps, &[0, 1, 2], VoteKind::Prevote, [7; 32])
                .verify("vinx-test", &pks)
                .is_err()
        );
        // Votes pour un autre bloc.
        assert!(
            cert(&kps, &[0, 1, 2], p, [8; 32])
                .verify("vinx-test", &pks)
                .is_err()
        );
        // Autre chaîne.
        assert!(
            cert(&kps, &[0, 1, 2], p, [7; 32])
                .verify("autre", &pks)
                .is_err()
        );
        // Doublon pour gonfler le compte.
        let mut dup = cert(&kps, &[0, 1], p, [7; 32]);
        dup.signatures.push(dup.signatures[1].clone());
        assert!(dup.verify("vinx-test", &pks).is_err());
        // Indice hors de l'ensemble.
        let mut out = cert(&kps, &[0, 1, 2], p, [7; 32]);
        out.signatures[2].0 = 4;
        assert!(out.verify("vinx-test", &pks).is_err());
    }
}
