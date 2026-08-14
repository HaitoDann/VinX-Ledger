//! ADR 0027 — fiabilité & jailing des validateurs (règle 1 + règle 2).
//!
//! Fonctions **pures et déterministes** dérivées de faits on-chain — le proposeur effectif
//! d'un bloc vs le leader prévu sur le set actif — donc calculées **identiquement par tous
//! les nœuds** (prérequis anti-fork ; la latence/uptime subjectifs restent hors consensus,
//! cf. ADR 0022). Le jailing est **non destructif** (le bond reste verrouillé et slashable)
//! et **réversible** (tx `Unjail` après cooldown).
//!
//! **Règle 1 :** manquements de proposition (leader prévu ≠ proposeur effectif).
//! **Règle 2 :** absence de co-signature sur la fenêtre d'époque — taux < `MIN_COSIGN_PARTICIPATION_BPS`.

use crate::ValidatorSet;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use vinx_crypto::Address;

/// Manquements consécutifs de proposition avant jailing. Indulgent (plusieurs tours) pour ne
/// pas jailer une panne transitoire (garde-fou anti-grief de l'ADR 0027).
pub const MAX_MISSED_PROPOSALS: u32 = 3;

/// Cooldown (en hauteurs) avant qu'un validateur jailé puisse soumettre `Unjail`.
/// Empêche le battement jail↔unjail.
pub const UNJAIL_COOLDOWN_HEIGHTS: u64 = 100;

/// Taux de co-signature minimum (règle 2) exprimé en points de base (10 000 = 100 %).
/// Un validateur actif qui descend en dessous sur une fenêtre d'époque est jailé.
pub const MIN_COSIGN_PARTICIPATION_BPS: u32 = 2_000; // 20 %

/// Nombre minimal de blocs finalisés dans la fenêtre avant d'appliquer la règle 2.
/// Évite les faux positifs en début d'époque (e.g. redémarrage, genèse).
pub const MIN_COSIGN_CHECK_ELIGIBLE: u32 = 20;

/// Méta de fiabilité d'un validateur (destinée au meta `WorldState` une fois câblée, comme
/// `pending_unbonds` — dérivée déterministiquement de la séquence de blocs, hors `state_root`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// Fenêtre de co-signature pour la règle 2 (ADR 0027) — réinitialisée à chaque fermeture
/// d'époque, stockée séparément de `ValidatorReliability` pour préserver la compatibilité
/// bincode de celle-ci.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CosignWindow {
    /// Co-signatures enregistrées depuis la dernière fermeture d'époque.
    pub count: u32,
    /// Blocs finalisés éligibles (validateur actif) depuis la dernière fermeture d'époque.
    pub eligible: u32,
}

/// Map des fenêtres de co-signature indexée par adresse (canonique `BTreeMap`).
pub type CosignWindowMap = BTreeMap<Address, CosignWindow>;

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
/// - si le proposeur diffère du **leader actif prévu**, incrémente le manquement de ce leader
///   et le **jaile** s'il atteint [`MAX_MISSED_PROPOSALS`].
///
/// Fait 100 % déterministe (un créneau inactif ne produit pas de bloc, donc n'est jamais
/// compté). Retourne `true` si ce bloc provoque un jailing.
pub fn on_block_applied(
    rel: &mut ReliabilityMap,
    vs: &ValidatorSet,
    height: u64,
    actual_proposer: &Address,
) -> bool {
    let expected = active_leader_at(vs, rel, height);
    rel.entry(*actual_proposer).or_default().missed_proposals = 0;
    if *actual_proposer != expected {
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

/// Met à jour les fenêtres de co-signature pour tous les validateurs **actifs** (non jailés)
/// lors de la finalisation d'un bloc (règle 2 — ADR 0027).
///
/// `active` = set actif courant (résultat de `active_validators`). Pour chaque membre :
/// - incrémente `eligible` ;
/// - incrémente `count` si l'adresse figure parmi `cosigners`.
///
/// Les validateurs jailés sont exclus (ils ne co-signent pas et ne doivent pas être pénalisés
/// pour des blocs produits sans eux).
pub fn on_block_cosigns(
    windows: &mut CosignWindowMap,
    active: &[Address],
    cosigners: &HashSet<Address>,
) {
    for addr in active {
        let w = windows.entry(*addr).or_default();
        w.eligible = w.eligible.saturating_add(1);
        if cosigners.contains(addr) {
            w.count = w.count.saturating_add(1);
        }
    }
}

/// À la fermeture d'époque : jaile les validateurs dont le taux de co-signature est inférieur
/// à `MIN_COSIGN_PARTICIPATION_BPS` sur la fenêtre écoulée, puis remet toutes les fenêtres à zéro.
///
/// `height` = hauteur courante (utilisée pour calculer `jailed_until`).
/// Retourne `true` si au moins un validateur a été jailé.
pub fn check_and_jail_cosign(
    windows: &mut CosignWindowMap,
    rel: &mut ReliabilityMap,
    vs: &ValidatorSet,
    height: u64,
) -> bool {
    let mut any_jailed = false;
    for addr in vs.validators() {
        let w = windows.entry(*addr).or_default();
        let eligible = w.eligible;
        let count = w.count;
        // Réinitialise la fenêtre pour la prochaine époque.
        *w = CosignWindow::default();
        // Déjà jailé (règle 1) → pas de double-peine.
        if rel.get(addr).is_some_and(|r| r.is_jailed()) {
            continue;
        }
        // Pas assez de données → pas de décision.
        if eligible < MIN_COSIGN_CHECK_ELIGIBLE {
            continue;
        }
        let rate_bps = count as u64 * 10_000 / eligible as u64;
        if (rate_bps as u32) < MIN_COSIGN_PARTICIPATION_BPS {
            let e = rel.entry(*addr).or_default();
            e.jailed_until = Some(height.saturating_add(UNJAIL_COOLDOWN_HEIGHTS));
            any_jailed = true;
        }
    }
    any_jailed
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
    fn missed_attribution_and_reset() {
        let a = addrs(3);
        let vs = ValidatorSet::new(a.clone());
        let mut rel = ReliabilityMap::new();

        // height 1 → leader actif = a[1]. Un backup (a[0]) produit → a[1] manque.
        assert!(!on_block_applied(&mut rel, &vs, 1, &a[0]));
        assert_eq!(rel[&a[1]].missed_proposals, 1);
        // a[0] a produit → son compteur est à 0.
        assert_eq!(rel[&a[0]].missed_proposals, 0);

        // height 4 → leader a[1] à nouveau ; s'il produit cette fois, reset.
        assert!(!on_block_applied(&mut rel, &vs, 4, &a[1]));
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
                !on_block_applied(&mut rel, &vs, h, p),
                "pas encore de jail à h={h}"
            );
        }
        assert_eq!(rel[&a[1]].missed_proposals, 2);
        // height 7 → 3ᵉ manquement de a[1] → jail.
        assert!(
            on_block_applied(&mut rel, &vs, 7, &a[0]),
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

    // ─── Règle 2 : co-signatures absentes ───────────────────────────────────

    #[test]
    fn cosign_window_accumulates_correctly() {
        let a = addrs(3);
        let mut windows = CosignWindowMap::new();
        let cosigners: HashSet<Address> = [a[0], a[2]].iter().copied().collect();
        let active = vec![a[0], a[1], a[2]];

        on_block_cosigns(&mut windows, &active, &cosigners);

        assert_eq!(windows[&a[0]].count, 1);
        assert_eq!(windows[&a[0]].eligible, 1);
        assert_eq!(windows[&a[1]].count, 0);
        assert_eq!(windows[&a[1]].eligible, 1);
        assert_eq!(windows[&a[2]].count, 1);
        assert_eq!(windows[&a[2]].eligible, 1);
    }

    #[test]
    fn check_and_jail_cosign_jails_below_threshold() {
        let a = addrs(2);
        let vs = ValidatorSet::new(a.clone());
        let mut windows = CosignWindowMap::new();
        let mut rel = ReliabilityMap::new();

        // Injecter MIN_COSIGN_CHECK_ELIGIBLE blocs : a[0] co-signe tous, a[1] aucun.
        let n = MIN_COSIGN_CHECK_ELIGIBLE;
        windows.insert(a[0], CosignWindow { count: n, eligible: n });
        windows.insert(a[1], CosignWindow { count: 0, eligible: n });

        let jailed = check_and_jail_cosign(&mut windows, &mut rel, &vs, 200);
        assert!(jailed, "a[1] devrait être jailé");
        assert!(rel[&a[1]].is_jailed());
        assert!(!rel.get(&a[0]).is_some_and(|r| r.is_jailed()), "a[0] ne doit pas être jailé");
        // Fenêtres réinitialisées.
        assert_eq!(windows[&a[0]].eligible, 0);
        assert_eq!(windows[&a[1]].eligible, 0);
    }

    #[test]
    fn check_and_jail_cosign_skips_already_jailed() {
        let a = addrs(1);
        let vs = ValidatorSet::new(a.clone());
        let mut windows = CosignWindowMap::new();
        let mut rel = ReliabilityMap::new();
        // Déjà jailé par la règle 1.
        rel.insert(a[0], ValidatorReliability { missed_proposals: 3, jailed_until: Some(999) });
        windows.insert(a[0], CosignWindow { count: 0, eligible: MIN_COSIGN_CHECK_ELIGIBLE });

        // La règle 2 ne devrait pas modifier le jailed_until existant.
        let jailed = check_and_jail_cosign(&mut windows, &mut rel, &vs, 100);
        assert!(!jailed, "déjà jailé : pas de double-peine");
        assert_eq!(rel[&a[0]].jailed_until, Some(999));
    }

    #[test]
    fn check_and_jail_cosign_skips_insufficient_data() {
        let a = addrs(1);
        let vs = ValidatorSet::new(a.clone());
        let mut windows = CosignWindowMap::new();
        let mut rel = ReliabilityMap::new();
        // Moins de MIN_COSIGN_CHECK_ELIGIBLE blocs.
        windows.insert(a[0], CosignWindow { count: 0, eligible: MIN_COSIGN_CHECK_ELIGIBLE - 1 });

        let jailed = check_and_jail_cosign(&mut windows, &mut rel, &vs, 100);
        assert!(!jailed, "pas assez de données → pas de jail");
    }

    #[test]
    fn check_and_jail_cosign_no_jail_above_threshold() {
        let a = addrs(1);
        let vs = ValidatorSet::new(a.clone());
        let mut windows = CosignWindowMap::new();
        let mut rel = ReliabilityMap::new();
        // Taux exactement au seuil (100% co-signature).
        let n = MIN_COSIGN_CHECK_ELIGIBLE;
        windows.insert(a[0], CosignWindow { count: n, eligible: n });

        let jailed = check_and_jail_cosign(&mut windows, &mut rel, &vs, 100);
        assert!(!jailed);
        assert!(!rel.get(&a[0]).is_some_and(|r| r.is_jailed()));
    }
}
