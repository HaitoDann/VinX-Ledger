//! Ensemble de validateurs et changements programmés (SPEC §7.1, §7.4).

use borsh::{BorshDeserialize, BorshSerialize};
use vinx_core::encode;
use vinx_crypto::{Domain, Hash, PublicKey, hash_tagged};

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum ValidatorChange {
    Add(PublicKey) = 0x01,
    Remove(PublicKey) = 0x02,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PendingChange {
    pub effective_height: u64,
    pub change: ValidatorChange,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ValidatorSet {
    /// Ensemble actif, ordonné. Une voix par validateur.
    pub active: Vec<PublicKey>,
    /// Changements en attente, dans l'ordre où ils ont été programmés.
    pub pending: Vec<PendingChange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValidatorError {
    #[error("ce validateur est déjà présent")]
    AlreadyPresent,
    #[error("ce validateur n'est pas dans l'ensemble")]
    NotPresent,
    #[error("l'ensemble de validateurs ne peut pas être vide")]
    WouldBeEmpty,
}

fn apply(set: &mut Vec<PublicKey>, change: &ValidatorChange) {
    match change {
        ValidatorChange::Add(key) => {
            if !set.contains(key) {
                set.push(key.clone());
            }
        }
        ValidatorChange::Remove(key) => set.retain(|k| k != key),
    }
}

impl ValidatorSet {
    pub fn new(active: Vec<PublicKey>) -> Self {
        Self {
            active,
            pending: Vec::new(),
        }
    }

    /// Ensemble qui valide la hauteur `height`.
    pub fn at_height(&self, height: u64) -> Vec<PublicKey> {
        let mut set = self.active.clone();
        for p in self.pending.iter().filter(|p| p.effective_height <= height) {
            apply(&mut set, &p.change);
        }
        set
    }

    /// Ensemble une fois tous les changements en attente appliqués.
    fn projected(&self) -> Vec<PublicKey> {
        self.at_height(u64::MAX)
    }

    /// Vérifie qu'un changement est cohérent avec l'ensemble projeté.
    pub fn check(&self, change: &ValidatorChange) -> Result<(), ValidatorError> {
        let projected = self.projected();
        match change {
            ValidatorChange::Add(key) if projected.contains(key) => {
                Err(ValidatorError::AlreadyPresent)
            }
            ValidatorChange::Remove(key) if !projected.contains(key) => {
                Err(ValidatorError::NotPresent)
            }
            ValidatorChange::Remove(_) if projected.len() <= 1 => Err(ValidatorError::WouldBeEmpty),
            _ => Ok(()),
        }
    }

    pub fn schedule(&mut self, change: ValidatorChange, effective_height: u64) {
        self.pending.push(PendingChange {
            effective_height,
            change,
        });
    }

    /// Applique les changements effectifs à `height`.
    pub fn advance_to(&mut self, height: u64) {
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| p.effective_height <= height);
        for p in &due {
            apply(&mut self.active, &p.change);
        }
        self.pending = later;
    }
}

/// `H("vinx/validators" ‖ borsh(liste))`, porté par `validators_hash`.
pub fn validators_hash(set: &[PublicKey]) -> Hash {
    hash_tagged(Domain::Validators, &encode(&set.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn keys(n: usize) -> Vec<PublicKey> {
        (0..n).map(|_| KeyPair::generate().public_key()).collect()
    }

    #[test]
    fn changes_take_effect_at_their_height() {
        let k = keys(3);
        let mut v = ValidatorSet::new(vec![k[0].clone(), k[1].clone()]);
        v.schedule(ValidatorChange::Add(k[2].clone()), 12);
        assert_eq!(v.at_height(11).len(), 2);
        assert_eq!(v.at_height(12).len(), 3);
        v.advance_to(11);
        assert_eq!(v.active.len(), 2);
        v.advance_to(12);
        assert_eq!(v.active, k);
        assert!(v.pending.is_empty());
    }

    #[test]
    fn checks_use_the_projected_set() {
        let k = keys(2);
        let mut v = ValidatorSet::new(vec![k[0].clone()]);
        assert_eq!(
            v.check(&ValidatorChange::Remove(k[0].clone())),
            Err(ValidatorError::WouldBeEmpty)
        );
        assert_eq!(
            v.check(&ValidatorChange::Add(k[0].clone())),
            Err(ValidatorError::AlreadyPresent)
        );
        assert_eq!(
            v.check(&ValidatorChange::Remove(k[1].clone())),
            Err(ValidatorError::NotPresent)
        );
        v.schedule(ValidatorChange::Add(k[1].clone()), 5);
        assert_eq!(
            v.check(&ValidatorChange::Add(k[1].clone())),
            Err(ValidatorError::AlreadyPresent)
        );
        assert_eq!(v.check(&ValidatorChange::Remove(k[0].clone())), Ok(()));
    }
}
