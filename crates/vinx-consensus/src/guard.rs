//! Garde anti-double signature (SPEC §7.3).
//!
//! Le nœud consulte le garde avant de signer un vote ou une proposition, et le
//! **persiste sur disque avant d'envoyer** la signature. Après un redémarrage,
//! il ne peut donc jamais signer deux contenus différents pour la même étape.

use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;
use vinx_core::VoteKind;
use vinx_crypto::Hash;

/// Étape signable : prevote, precommit ou proposition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum Step {
    Proposal = 0x00,
    Prevote = 0x01,
    Precommit = 0x02,
}

impl From<VoteKind> for Step {
    fn from(kind: VoteKind) -> Self {
        match kind {
            VoteKind::Prevote => Step::Prevote,
            VoteKind::Precommit => Step::Precommit,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignGuard {
    /// Contenu déjà signé par `(hauteur, round, étape)`. `None` = vote nil.
    signed: BTreeMap<(u64, u32, Step), Option<Hash>>,
}

impl SignGuard {
    /// Autorise la signature si rien n'a été signé à cette étape, ou si c'est
    /// exactement le même contenu (renvoi d'un message déjà émis).
    /// Enregistre la signature lorsqu'elle est autorisée.
    pub fn allow(&mut self, height: u64, round: u32, step: Step, content: Option<Hash>) -> bool {
        match self.signed.get(&(height, round, step)) {
            Some(previous) => *previous == content,
            None => {
                self.signed.insert((height, round, step), content);
                true
            }
        }
    }

    /// Oublie les hauteurs déjà finalisées, pour borner la taille du garde.
    pub fn prune_below(&mut self, height: u64) {
        self.signed.retain(|(h, _, _), _| *h >= height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_signs_two_contents_for_the_same_step() {
        let mut g = SignGuard::default();
        assert!(g.allow(1, 0, Step::Prevote, Some([1; 32])));
        assert!(g.allow(1, 0, Step::Prevote, Some([1; 32])));
        assert!(!g.allow(1, 0, Step::Prevote, Some([2; 32])));
        assert!(!g.allow(1, 0, Step::Prevote, None));
        assert!(g.allow(1, 0, Step::Precommit, None));
        assert!(g.allow(1, 1, Step::Prevote, Some([2; 32])));
    }

    #[test]
    fn survives_a_restart() {
        let mut g = SignGuard::default();
        assert!(g.allow(3, 2, Step::Proposal, Some([5; 32])));
        let bytes = borsh::to_vec(&g).unwrap();
        let mut restored: SignGuard = borsh::from_slice(&bytes).unwrap();
        assert!(!restored.allow(3, 2, Step::Proposal, Some([6; 32])));
        restored.prune_below(4);
        assert!(restored.allow(4, 0, Step::Proposal, Some([6; 32])));
    }
}
