//! Signatures de transactions déjà vérifiées.
//!
//! Une même transaction est vérifiée à son entrée dans le mempool, à chaque
//! reconstruction du mempool, par le proposeur puis par chaque validateur :
//! sans mémoire, jusqu'à quatre fois sur un même nœud. La vérification Ed25519
//! représente l'essentiel du coût d'un bloc.
//!
//! Le cache retient des paires (identifiant de transaction, clé) dont la
//! signature a été vérifiée. L'identifiant couvre la transaction entière,
//! signature comprise ; la clé est comparée à celle qu'exige l'état au moment
//! de l'application (une rotation de clé dans le même bloc en change). Une
//! entrée ne dit donc qu'une chose toujours vraie : « cette signature est
//! valide pour cette clé ». Le résultat de [`crate::State::apply_tx`] ne dépend
//! pas du contenu du cache, seulement sa durée.
//!
//! [`preverify`] vérifie un lot sur tous les cœurs, avec la même vérification
//! stricte qu'une transaction isolée (pas de vérification « par lot » Ed25519,
//! qui n'accepte pas exactement les mêmes signatures).

use crate::State;
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use vinx_core::{CoreError, SignedTransaction};
use vinx_crypto::{Hash, PublicKey};

/// Entrées par génération. Deux générations sont gardées : environ 2 × 100 000
/// transactions, une vingtaine de mégaoctets.
const GENERATION: usize = 100_000;

#[derive(Default)]
struct Generations {
    current: HashMap<Hash, PublicKey>,
    previous: HashMap<Hash, PublicKey>,
}

impl Generations {
    fn contains(&self, id: &Hash, key: &PublicKey) -> bool {
        self.current.get(id).or_else(|| self.previous.get(id)) == Some(key)
    }

    fn insert(&mut self, id: Hash, key: PublicKey) {
        if self.current.len() >= GENERATION {
            self.previous = std::mem::take(&mut self.current);
        }
        self.current.insert(id, key);
    }
}

fn cache() -> std::sync::MutexGuard<'static, Generations> {
    static CACHE: OnceLock<Mutex<Generations>> = OnceLock::new();
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Vérifie la signature de `stx` avec `key`, sauf si c'est déjà fait.
pub fn verify(stx: &SignedTransaction, key: &PublicKey) -> Result<(), CoreError> {
    let id = stx.id();
    if cache().contains(&id, key) {
        return Ok(());
    }
    stx.verify_signature(key)?;
    cache().insert(id, key.clone());
    Ok(())
}

/// Vérifie en parallèle les signatures de `txs` et retient celles qui sont
/// valides. La clé de chaque transaction est celle jointe, ou à défaut celle du
/// compte dans `state`. Ne rejette rien : une transaction dont la signature
/// n'est pas retenue sera simplement vérifiée, et refusée, à son application.
pub fn preverify(state: &State, txs: &[SignedTransaction]) {
    let todo: Vec<(Hash, PublicKey, &SignedTransaction)> = {
        let c = cache();
        txs.iter()
            .filter_map(|stx| {
                let key = stx
                    .public_key
                    .clone()
                    .or_else(|| state.account(&stx.tx.sender)?.key.clone())?;
                let id = stx.id();
                (!c.contains(&id, &key)).then_some((id, key, stx))
            })
            .collect()
    };
    if todo.is_empty() {
        return;
    }
    let verified: Vec<(Hash, PublicKey)> = todo
        .into_par_iter()
        .filter(|(_, key, stx)| stx.verify_signature(key).is_ok())
        .map(|(id, key, _)| (id, key))
        .collect();
    let mut c = cache();
    for (id, key) in verified {
        c.insert(id, key);
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)] // code de test
mod tests {
    use super::*;
    use vinx_core::{Transaction, TxBody};
    use vinx_crypto::{Address, KeyPair};

    fn tx(kp: &KeyPair, nonce: u64) -> SignedTransaction {
        Transaction {
            chain_id: "vinx-test".into(),
            sender: Address::from_public_key(&kp.public_key()),
            nonce,
            fee: 1,
            body: TxBody::SetMemoRequired { required: true },
        }
        .sign(kp, true)
    }

    #[test]
    fn a_cached_signature_is_only_valid_for_its_key() {
        let (kp, other) = (KeyPair::generate(), KeyPair::generate());
        let stx = tx(&kp, 1);
        assert!(verify(&stx, &kp.public_key()).is_ok());
        assert!(cache().contains(&stx.id(), &kp.public_key()));
        // Déjà en cache pour une autre clé : la signature est revérifiée, et refusée.
        assert!(verify(&stx, &other.public_key()).is_err());
    }

    #[test]
    fn a_bad_signature_is_never_cached() {
        let kp = KeyPair::generate();
        let mut stx = tx(&kp, 1);
        stx.tx.nonce = 2; // la signature ne couvre plus la transaction
        let k = vinx_state_genesis();
        preverify(&k, std::slice::from_ref(&stx));
        assert!(!cache().contains(&stx.id(), &kp.public_key()));
        assert!(verify(&stx, &kp.public_key()).is_err());
    }

    #[test]
    fn generations_are_bounded() {
        let mut g = Generations::default();
        let key = KeyPair::generate().public_key();
        for i in 0..(2 * GENERATION + 1) {
            let mut id = [0u8; 32];
            id[..8].copy_from_slice(&(i as u64).to_le_bytes());
            g.insert(id, key.clone());
        }
        assert!(g.current.len() + g.previous.len() <= 2 * GENERATION);
        assert!(
            !g.contains(&[0u8; 32], &key),
            "les plus anciennes sont oubliées"
        );
    }

    fn vinx_state_genesis() -> State {
        let k = || KeyPair::generate().public_key();
        crate::Genesis::legacy("vinx-test", 1, [k(), k(), k()], vec![k()])
            .unwrap()
            .into_state()
            .unwrap()
    }
}
