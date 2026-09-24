//! ADR 0027 — fiabilité & jailing des validateurs (**cœur déterministe, tranche 1**).
//!
//! Fonctions **pures et déterministes** dérivées de faits on-chain — le proposeur effectif
//! d'un bloc vs le leader prévu sur le set actif — donc calculées **identiquement par tous
//! les nœuds** (prérequis anti-fork ; la latence/uptime subjectifs restent hors consensus,
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

/// Set **actif** = validateurs non jailés, dans l'ordre du set. **Plancher de liveness** : si
/// tous seraient jailés, on retombe sur le set complet (on ne vide jamais la rotation).
pub fn active_validators(vs: &ValidatorSet, rel: &ReliabilityMap) -> Vec<Address> {
    let active: Vec<Address> = vs
        .validators()
        .iter()
        .copied()
        .filter(|a| !rel.get(a).is_some_and(|r| r.is_jailed()))
        .collect();
    if active.is_empty() {
        vs.validators().to_vec()
    } else {
        active
    }
}

/// Leader round-robin **sur le set actif** (les jailés sont sautés).
pub fn active_leader_at(vs: &ValidatorSet, rel: &ReliabilityMap, height: u64) -> Address {
    let active = active_validators(vs, rel);
    active[(height as usize) % active.len()]
}

/// Quorum `⌈2n/3⌉` calculé sur le set **actif** — dénominateur ajusté pour qu'un validateur
/// jailé ne bloque **pas** la finalité (ADR 0027).
pub fn active_quorum(vs: &ValidatorSet, rel: &ReliabilityMap) -> usize {
    let n = active_validators(vs, rel).len();
    (2 * n).div_ceil(3)
}

/// À l'application du bloc de hauteur `height` produit par `actual_proposer` :
/// - remet à 0 le compteur du **proposeur effectif** (production réussie) ;
/// - si le proposeur diffère du **leader actif prévu** *et* que le créneau du leader est
///   écoulé (`slot_timeout_secs` depuis le bloc précédent), incrémente le manquement de ce
///   leader et le **jaile** s'il atteint [`MAX_MISSED_PROPOSALS`].
///
/// Fait 100 % déterministe (un créneau inactif ne produit pas de bloc, donc n'est jamais
/// compté). Retourne `true` si ce bloc provoque un jailing.
pub fn on_block_applied(
    rel: &mut ReliabilityMap,
    vs: &ValidatorSet,
    height: u64,
    actual_proposer: &Address,
    block_ts: u64,
    prev_block_ts: u64,
    slot_timeout_secs: u64,
) -> bool {
    let expected = active_leader_at(vs, rel, height);
    rel.entry(*actual_proposer).or_default().missed_proposals = 0;
    // VINX-06 : ne compter un manquement que si le leader a **réellement** laissé passer
    // son tour. Sans cette condition, un validateur unique qui propose systématiquement
    // avant le leader prévu jaile tout le set honnête en trois tours — les blocs d'un
    // non-leader sont acceptés par le chemin P2P sans aucune contrainte de créneau.
    // `block_ts`/`prev_block_ts` sont des quantités d'en-tête, donc déterministes.
    let slot_elapsed = block_ts.saturating_sub(prev_block_ts) >= slot_timeout_secs;
    if *actual_proposer != expected && slot_elapsed {
        let e = rel.entry(expected).or_default();
        if e.jailed_until.is_none() {
            e.missed_proposals = e.missed_proposals.saturating_add(1);
            if e.missed_proposals >= MAX_MISSED_PROPOSALS {
                e.jailed_until = Some(height.saturating_add(UNJAIL_COOLDOWN_HEIGHTS));
                return true;
            }
        }
    }
    false
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
    const SLOT_TIMEOUT_SECS: u64 =
        crate::amount::slot_timeout_secs(crate::amount::DEFAULT_BLOCK_TIME_SECS);
    use vinx_crypto::KeyPair;

    fn addrs(n: usize) -> Vec<Address> {
        (0..n)
            .map(|_| Address::from_public_key(&KeyPair::generate().public_key()))
            .collect()
    }

    #[test]
    fn missed_attribution_and_reset() {
        let a = addrs(3);
        let vs = ValidatorSet::new(a.clone());
        let mut rel = ReliabilityMap::new();

        // height 1 → leader actif = a[1]. Un backup (a[0]) produit → a[1] manque.
        assert!(!on_block_applied(
            &mut rel,
            &vs,
            1,
            &a[0],
            SLOT_TIMEOUT_SECS,
            0,
            SLOT_TIMEOUT_SECS,
        ));
        assert_eq!(rel[&a[1]].missed_proposals, 1);
        // a[0] a produit → son compteur est à 0.
        assert_eq!(rel[&a[0]].missed_proposals, 0);

        // height 4 → leader a[1] à nouveau ; s'il produit cette fois, reset.
        assert!(!on_block_applied(
            &mut rel,
            &vs,
            4,
            &a[1],
            SLOT_TIMEOUT_SECS,
            0,
            SLOT_TIMEOUT_SECS,
        ));
        assert_eq!(
            rel[&a[1]].missed_proposals, 0,
            "une production réussie remet à 0"
        );
    }

    #[test]
    fn jails_after_max_consecutive_misses() {
        let a = addrs(3);
        let vs = ValidatorSet::new(a.clone());
        let mut rel = ReliabilityMap::new();

        // a[1] est mort : à ses créneaux (heights 1,4,7) un backup a[0] produit.
        // Aux autres hauteurs, le leader prévu produit normalement.
        let plan = [
            (1, &a[0]),
            (2, &a[2]),
            (3, &a[0]),
            (4, &a[0]),
            (5, &a[2]),
            (6, &a[0]),
        ];
        for (h, p) in plan {
            assert!(
                !on_block_applied(&mut rel, &vs, h, p, SLOT_TIMEOUT_SECS, 0, SLOT_TIMEOUT_SECS),
                "pas encore de jail à h={h}"
            );
        }
        assert_eq!(rel[&a[1]].missed_proposals, 2);
        // height 7 → 3ᵉ manquement de a[1] → jail.
        assert!(
            on_block_applied(
                &mut rel,
                &vs,
                7,
                &a[0],
                SLOT_TIMEOUT_SECS,
                0,
                SLOT_TIMEOUT_SECS
            ),
            "jail au 3ᵉ manquement"
        );
        assert!(rel[&a[1]].is_jailed());
        assert_eq!(rel[&a[1]].jailed_until, Some(7 + UNJAIL_COOLDOWN_HEIGHTS));
    }

    #[test]
    fn active_set_and_quorum_exclude_jailed() {
        let a = addrs(3);
        let vs = ValidatorSet::new(a.clone());
        assert_eq!(vs.quorum(), 2);

        let mut rel = ReliabilityMap::new();
        rel.insert(
            a[1],
            ValidatorReliability {
                missed_proposals: 3,
                jailed_until: Some(107),
            },
        );

        let active = active_validators(&vs, &rel);
        assert_eq!(
            active,
            vec![a[0], a[2]],
            "le jailé est retiré de la rotation"
        );
        assert_eq!(
            active_quorum(&vs, &rel),
            2,
            "quorum sur set actif de 2 = ceil(4/3)=2"
        );
        // rotation sur le set actif (2 membres) : jamais le jailé.
        assert_eq!(active_leader_at(&vs, &rel, 8), a[0]);
        assert_eq!(active_leader_at(&vs, &rel, 9), a[2]);
        assert_ne!(active_leader_at(&vs, &rel, 100), a[1]);
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
        assert_eq!(
            rel[&a[1]].missed_proposals, 0,
            "compteurs réinitialisés à l'unjail"
        );
    }

    #[test]
    fn liveness_floor_never_empties_rotation() {
        let a = addrs(3);
        let vs = ValidatorSet::new(a.clone());
        let mut rel = ReliabilityMap::new();
        // Tous jailés (cas dégénéré) → repli sur le set complet (rotation jamais vide).
        for x in &a {
            rel.insert(
                *x,
                ValidatorReliability {
                    missed_proposals: 3,
                    jailed_until: Some(1),
                },
            );
        }
        assert_eq!(
            active_validators(&vs, &rel).len(),
            3,
            "plancher : set complet si tous jailés"
        );
        assert!(active_quorum(&vs, &rel) >= 1);
    }

    /// VINX-06 — a validator that simply proposes *before* the scheduled leader, at
    /// normal cadence, must not be able to jail the honest set. Before the slot-timeout
    /// condition, 3 pre-emptive blocks jailed each honest leader in turn, and a single
    /// byzantine validator ended up alone in the rotation.
    #[test]
    fn preemptive_proposer_cannot_jail_the_honest_set() {
        let a = addrs(5);
        let vs = ValidatorSet::new(a.clone());
        let attacker = a[0];
        let mut rel = ReliabilityMap::new();

        // 60 blocks all proposed by the attacker, each arriving at the normal cadence
        // (well under SLOT_TIMEOUT_SECS after the previous one).
        let mut prev_ts = 1_000u64;
        for h in 1..=60u64 {
            let ts = prev_ts + 12;
            assert!(
                !on_block_applied(&mut rel, &vs, h, &attacker, ts, prev_ts, SLOT_TIMEOUT_SECS),
                "a block at normal cadence must never jail the scheduled leader"
            );
            prev_ts = ts;
        }
        for v in &a {
            assert!(
                !rel.get(v).map(|r| r.is_jailed()).unwrap_or(false),
                "no honest validator may be jailed by a pre-emptive proposer"
            );
        }
        assert_eq!(
            active_validators(&vs, &rel).len(),
            5,
            "the whole set must remain in the rotation"
        );
    }

    /// The counterpart: a leader that genuinely lets its slot elapse is still charged,
    /// so the fix does not disable liveness accounting.
    #[test]
    fn genuinely_absent_leader_is_still_jailed() {
        let a = addrs(3);
        let vs = ValidatorSet::new(a.clone());
        let mut rel = ReliabilityMap::new();

        // One validator proposes every block, each arriving well after the scheduled
        // leader's slot elapsed — the legitimate slot-skip case.
        let proposer = a[0];
        let mut jailed_any = false;
        let mut prev_ts = 0u64;
        for h in 1..=12u64 {
            let ts = prev_ts + SLOT_TIMEOUT_SECS + 1;
            jailed_any |=
                on_block_applied(&mut rel, &vs, h, &proposer, ts, prev_ts, SLOT_TIMEOUT_SECS);
            prev_ts = ts;
        }
        assert!(
            jailed_any,
            "a leader that repeatedly misses an elapsed slot must still be jailed"
        );
    }
}
