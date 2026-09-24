//! ADR 0027 — fiabilité & jailing des validateurs (**cœur déterministe, tranche 1**).
//!
//! Fonctions **pures et déterministes** dérivées de faits on-chain — le tour auquel un bloc
//! a été validé et les proposeurs prévus des tours précédents (ADR 0082) — donc calculées
//! **identiquement par tous les nœuds** (prérequis anti-fork ; la latence/uptime subjectifs restent hors consensus,
//! cf. ADR 0022). Le jailing est **non destructif** (le bond reste verrouillé et slashable)
//! et **réversible** (tx `Unjail` après cooldown).
//!
//! **Périmètre tranche 1 :** attribution des manquements de proposition, décision de jailing,
//! set actif, quorum ajusté sur le set actif, éligibilité d'unjail — le tout en fonctions
//! pures testées. Le **câblage dans la transition d'état vivante + la migration meta** est
//! différé (tranche 2, banc multi-nœuds), comme pour le fork-choice (ADR 0031). La règle 2 de
//! l'ADR (fenêtre de co-signatures absentes) est aussi différée à la tranche 2.

use crate::ValidatorSet;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use vinx_crypto::Address;

/// Manquements consécutifs de proposition avant jailing. Indulgent (plusieurs tours) pour ne
/// pas jailer une panne transitoire (garde-fou anti-grief de l'ADR 0027).
pub const MAX_MISSED_PROPOSALS: u32 = 3;

/// Cooldown (en hauteurs) avant qu'un validateur jailé puisse soumettre `Unjail`.
/// Empêche le battement jail↔unjail.
pub const UNJAIL_COOLDOWN_HEIGHTS: u64 = 100;

/// Méta de fiabilité d'un validateur (destinée au meta `WorldState` une fois câblée, comme
/// `pending_unbonds` — dérivée déterministiquement de la séquence de blocs, hors `state_root`).
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct ValidatorReliability {
    /// Manquements **consécutifs** de proposition — remis à 0 dès que ce validateur produit.
    pub missed_proposals: u32,
    /// `Some(h)` ⇒ jailé ; `h` = hauteur au plus tôt où `Unjail` est permis (cooldown).
    pub jailed_until: Option<u64>,
}

impl ValidatorReliability {
    /// Un validateur reste jailé jusqu'à un `Unjail` explicite (le cooldown ne fait que
    /// **gater** quand l'unjail devient permis, il ne libère pas automatiquement).
    pub fn is_jailed(&self) -> bool {
        self.jailed_until.is_some()
    }

    /// L'unjail est permis une fois le cooldown écoulé.
    pub fn can_unjail(&self, height: u64) -> bool {
        self.jailed_until.is_some_and(|until| height >= until)
    }
}

/// Table de fiabilité indexée par adresse (ordre canonique `BTreeMap`, jamais `HashMap`).
pub type ReliabilityMap = BTreeMap<Address, ValidatorReliability>;

/// Vrai si `addr` peut proposer (n'est pas jailé).
pub fn is_eligible(rel: &ReliabilityMap, addr: &Address) -> bool {
    !rel.get(addr).is_some_and(|r| r.is_jailed())
}

/// Set **actif** = validateurs non jailés, dans l'ordre du set. **Plancher de liveness** : si
/// tous seraient jailés, on retombe sur le set complet (on ne vide jamais la rotation).
pub fn active_validators(vs: &ValidatorSet, rel: &ReliabilityMap) -> Vec<Address> {
    let active: Vec<Address> = vs
        .validators()
        .iter()
        .copied()
        .filter(|a| is_eligible(rel, a))
        .collect();
    if active.is_empty() {
        vs.validators().to_vec()
    } else {
        active
    }
}

/// Proposeur prévu pour le tour `round` de la prochaine hauteur (ADR 0082) : priorités de
/// proposeur pondérées par le poids (`ValidatorSet`), jailés sautés.
pub fn proposer_for_round(vs: &ValidatorSet, rel: &ReliabilityMap, round: u32) -> Address {
    *vs.proposer(round, |a| is_eligible(rel, a))
}

/// À la validation d'un bloc de hauteur `height`, validé au tour `round` et proposé par
/// `proposer` (ADR 0082) :
/// - remet à 0 le compteur du proposeur effectif ;
/// - impute un manquement au proposeur prévu de **chaque tour antérieur** `0..round` : si le
///   bloc est validé au tour `r > 0`, les proposeurs des tours précédents n'ont pas fait
///   passer de bloc. Jailé à [`MAX_MISSED_PROPOSALS`] manquements consécutifs.
///
/// `vs` doit être le set **qui a voté** ce bloc, avec ses priorités **avant** l'avancée de
/// cette hauteur. Tout est dérivé de l'en-tête (le tour est garanti par le certificat de
/// validation) : un validateur ne peut plus faire jailer les autres en proposant en avance
/// (VINX-06), puisque seul le proposeur prévu d'un tour peut y proposer. Retourne `true` si
/// ce bloc provoque un jailing.
pub fn on_block_committed(
    rel: &mut ReliabilityMap,
    vs: &ValidatorSet,
    height: u64,
    round: u32,
    proposer: &Address,
) -> bool {
    // Proposeurs manqués, calculés avant toute mutation (le jailing d'un tour antérieur ne
    // doit pas changer l'attribution des suivants au sein du même bloc).
    let missed: Vec<Address> = (0..round)
        .map(|r| proposer_for_round(vs, rel, r))
        .filter(|a| a != proposer)
        .collect();
    rel.entry(*proposer).or_default().missed_proposals = 0;
    let mut jailed = false;
    for m in missed {
        let e = rel.entry(m).or_default();
        if e.jailed_until.is_none() {
            e.missed_proposals = e.missed_proposals.saturating_add(1);
            if e.missed_proposals >= MAX_MISSED_PROPOSALS {
                e.jailed_until = Some(height.saturating_add(UNJAIL_COOLDOWN_HEIGHTS));
                jailed = true;
            }
        }
    }
    jailed
}

/// Sort `addr` de prison (tx `Unjail`, opérateur uniquement) si le cooldown est écoulé :
/// réinitialise les compteurs et le réintègre à la rotation. Retourne `true` en cas de succès.
pub fn try_unjail(rel: &mut ReliabilityMap, addr: &Address, height: u64) -> bool {
    if let Some(r) = rel.get_mut(addr) {
        if r.can_unjail(height) {
            r.jailed_until = None;
            r.missed_proposals = 0;
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn addrs(n: usize) -> Vec<Address> {
        (0..n)
            .map(|_| Address::from_public_key(&KeyPair::generate().public_key()))
            .collect()
    }

    #[test]
    fn round_zero_commit_charges_nobody_and_resets_proposer() {
        let a = addrs(4);
        let vs = ValidatorSet::new(a.clone());
        let mut rel = ReliabilityMap::new();
        rel.entry(a[0]).or_default().missed_proposals = 2;
        let p = proposer_for_round(&vs, &rel, 0);
        assert!(!on_block_committed(&mut rel, &vs, 1, 0, &p));
        assert!(rel.values().all(|r| !r.is_jailed()));
        assert_eq!(rel[&p].missed_proposals, 0);
    }

    #[test]
    fn higher_round_charges_the_skipped_proposers() {
        let a = addrs(4);
        let vs = ValidatorSet::new(a.clone());
        let mut rel = ReliabilityMap::new();
        let r0 = proposer_for_round(&vs, &rel, 0);
        let r1 = proposer_for_round(&vs, &rel, 1);
        let r2 = proposer_for_round(&vs, &rel, 2);
        assert!(!on_block_committed(&mut rel, &vs, 1, 2, &r2));
        assert_eq!(rel[&r0].missed_proposals, 1);
        assert_eq!(rel[&r1].missed_proposals, 1);
        assert_eq!(rel[&r2].missed_proposals, 0);
    }

    #[test]
    fn jails_after_max_consecutive_misses() {
        let a = addrs(4);
        let vs = ValidatorSet::new(a.clone());
        let mut rel = ReliabilityMap::new();
        let dead = proposer_for_round(&vs, &rel, 0);
        let backup = proposer_for_round(&vs, &rel, 1);
        for h in 1..MAX_MISSED_PROPOSALS as u64 {
            assert!(!on_block_committed(&mut rel, &vs, h, 1, &backup));
        }
        assert!(on_block_committed(
            &mut rel,
            &vs,
            MAX_MISSED_PROPOSALS as u64,
            1,
            &backup
        ));
        assert!(rel[&dead].is_jailed());
        // A jailed validator is skipped by the proposer rotation.
        for r in 0..8 {
            assert_ne!(proposer_for_round(&vs, &rel, r), dead);
        }
    }

    #[test]
    fn unjail_gated_by_cooldown_then_resets() {
        let a = addrs(3);
        let mut rel = ReliabilityMap::new();
        rel.insert(
            a[1],
            ValidatorReliability {
                missed_proposals: 3,
                jailed_until: Some(107),
            },
        );
        assert!(!try_unjail(&mut rel, &a[1], 50), "avant cooldown : refusé");
        assert!(rel[&a[1]].is_jailed());
        assert!(
            try_unjail(&mut rel, &a[1], 107),
            "cooldown écoulé : autorisé"
        );
        assert!(!rel[&a[1]].is_jailed());
        assert_eq!(rel[&a[1]].missed_proposals, 0);
    }

    #[test]
    fn liveness_floor_never_empties_rotation() {
        let a = addrs(3);
        let vs = ValidatorSet::new(a.clone());
        let mut rel = ReliabilityMap::new();
        for x in &a {
            rel.insert(
                *x,
                ValidatorReliability {
                    missed_proposals: 3,
                    jailed_until: Some(1),
                },
            );
        }
        assert_eq!(active_validators(&vs, &rel).len(), 3);
        // Everyone jailed: the rotation falls back to the full set rather than stalling.
        let _ = proposer_for_round(&vs, &rel, 0);
    }
}
