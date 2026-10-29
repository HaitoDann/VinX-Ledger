//! Émission vers les validateurs (protocole 3, SPEC §4.4, ADR 0002).
//!
//! Un budget fixé à la genèse (100 M VINX) est versé aux validateurs selon une
//! courbe à demi-vie de 10 ans, calculée sur l'horodatage des blocs :
//! `émis(t) = budget × (1 − 2^(−t / demi-vie))`. Rien n'est créé au-delà de la
//! supply : le budget non encore émis fait partie de l'invariant.
//!
//! À la fin de chaque époque, l'émission due et les frais collectés sont
//! répartis entre les validateurs au prorata des blocs qu'ils ont proposés.

use borsh::{BorshDeserialize, BorshSerialize};
use vinx_core::{Amount, UNITS_PER_VINX};
use vinx_crypto::PublicKey;

/// Budget d'émission de la genèse standard : 100 000 000 VINX.
pub const GENESIS_EMISSION: Amount = 100_000_000 * UNITS_PER_VINX;

/// Demi-vie de la courbe : 10 années de 365,25 jours, en secondes.
pub const HALF_LIFE: u64 = 315_576_000;

/// Durée d'une époque de la genèse standard, en blocs (≈ 24 h à 5 s).
pub const DEFAULT_EPOCH_BLOCKS: u64 = 17_280;

const HALF_LIFE_U128: u128 = HALF_LIFE as u128;

/// Échelle des calculs en virgule fixe : 10¹⁸.
const WAD: u128 = 1_000_000_000_000_000_000;
/// ln 2 × 10¹⁸.
const LN2_WAD: u128 = 693_147_180_559_945_309;

#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Emission {
    /// Origine de la courbe : horodatage Unix de la genèse, en secondes.
    pub start: u64,
    /// Budget total, fixé à la genèse ; 0 avant le protocole 3.
    pub budget: Amount,
    /// Déjà versé aux validateurs.
    pub emitted: Amount,
    /// Frais collectés et reliquats d'arrondi, en attente de répartition.
    pub pot: Amount,
    /// Durée d'une époque, en blocs.
    pub epoch_blocks: u64,
    /// Blocs proposés par validateur depuis le début de l'époque.
    pub proposed: Vec<(PublicKey, u64)>,
}

/// `2^(−t / demi-vie)` × 10¹⁸, en arithmétique entière : le résultat est
/// identique sur toutes les machines, ce que des flottants ne garantiraient pas.
pub fn remaining_fraction(elapsed: u64) -> u128 {
    let halvings = elapsed / HALF_LIFE;
    if halvings >= 64 {
        return 0;
    }
    let rest = u128::from(elapsed % HALF_LIFE);
    // y = ln 2 × reste / demi-vie, dans [0, ln 2[ : e^(−y) par sa série de Taylor.
    let y = LN2_WAD.saturating_mul(rest) / HALF_LIFE_U128;
    let mut sum = WAD;
    let mut term = WAD;
    for k in 1..=40u128 {
        term = (term.saturating_mul(y) / WAD).checked_div(k).unwrap_or(0);
        if term == 0 {
            break;
        }
        sum = if k % 2 == 1 {
            sum.saturating_sub(term)
        } else {
            sum.saturating_add(term)
        };
    }
    sum >> halvings
}

impl Emission {
    pub fn remaining(&self) -> Amount {
        self.budget.saturating_sub(self.emitted)
    }

    /// Total qui devrait avoir été émis à l'instant `now` (secondes).
    pub fn target(&self, now: u64) -> Amount {
        let elapsed = now.saturating_sub(self.start);
        let emitted_fraction = WAD.saturating_sub(remaining_fraction(elapsed));
        let target = u128::from(self.budget).saturating_mul(emitted_fraction) / WAD;
        Amount::try_from(target)
            .unwrap_or(self.budget)
            .min(self.budget)
    }

    /// Rythme d'émission actuel, en unités par jour (pour l'affichage).
    pub fn daily_rate(&self, now: u64) -> Amount {
        self.target(now.saturating_add(86_400))
            .saturating_sub(self.target(now))
    }

    /// Note un bloc proposé par `proposer`.
    pub fn record(&mut self, proposer: &PublicKey) {
        match self.proposed.iter_mut().find(|(k, _)| k == proposer) {
            Some((_, n)) => *n = n.saturating_add(1),
            None => self.proposed.push((proposer.clone(), 1)),
        }
    }

    /// Clôt l'époque à l'instant `now` : émet ce qui est dû, et renvoie la part
    /// de chaque validateur (cagnotte au prorata des blocs proposés). Le reliquat
    /// d'arrondi reste dans la cagnotte. Les compteurs repartent de zéro.
    pub fn close_epoch(&mut self, now: u64) -> Vec<(PublicKey, Amount)> {
        let due = self
            .target(now)
            .saturating_sub(self.emitted)
            .min(self.remaining());
        self.emitted = self.emitted.saturating_add(due);
        self.pot = self.pot.saturating_add(due);
        let blocks: u128 = self.proposed.iter().map(|(_, n)| u128::from(*n)).sum();
        let proposed = std::mem::take(&mut self.proposed);
        if blocks == 0 {
            return Vec::new();
        }
        let pot = u128::from(self.pot);
        let mut paid: Amount = 0;
        let shares: Vec<(PublicKey, Amount)> = proposed
            .into_iter()
            .map(|(key, n)| {
                let share = pot
                    .saturating_mul(u128::from(n))
                    .checked_div(blocks)
                    .and_then(|s| Amount::try_from(s).ok())
                    .unwrap_or(0);
                paid = paid.saturating_add(share);
                (key, share)
            })
            .collect();
        self.pot = self.pot.saturating_sub(paid);
        shares
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::arithmetic_side_effects)] // code de test
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    const YEAR: u64 = HALF_LIFE / 10;

    fn emission() -> Emission {
        Emission {
            start: 1_800_000_000,
            budget: GENESIS_EMISSION,
            epoch_blocks: DEFAULT_EPOCH_BLOCKS,
            ..Emission::default()
        }
    }

    fn close_to(actual: Amount, expected_vinx: f64) -> bool {
        let actual = actual as f64 / UNITS_PER_VINX as f64;
        (actual - expected_vinx).abs() / expected_vinx < 1e-9
    }

    #[test]
    fn curve_matches_the_half_life() {
        let e = emission();
        assert_eq!(e.target(e.start), 0);
        assert_eq!(e.target(e.start + HALF_LIFE), GENESIS_EMISSION / 2);
        assert_eq!(e.target(e.start + 2 * HALF_LIFE), GENESIS_EMISSION / 4 * 3);
        assert!(close_to(e.target(e.start + YEAR), 6_696_700.846));
        assert!(close_to(e.target(e.start + 5 * YEAR), 29_289_321.881));
        assert!(e.target(e.start + 100 * HALF_LIFE) <= GENESIS_EMISSION);
        // Le rythme du premier jour : ~18 977 VINX.
        let day = e.daily_rate(e.start) / UNITS_PER_VINX;
        assert!((18_970..=18_980).contains(&day), "{day}");
    }

    #[test]
    fn curve_is_monotonic() {
        let e = emission();
        let mut last = 0;
        for t in (0..50 * YEAR).step_by(7_919_999) {
            let v = e.target(e.start + t);
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn epoch_shares_follow_proposed_blocks() {
        let mut e = emission();
        e.pot = 1_000; // frais collectés
        let (a, b) = (
            KeyPair::generate().public_key(),
            KeyPair::generate().public_key(),
        );
        for _ in 0..3 {
            e.record(&a);
        }
        e.record(&b);
        let now = e.start + 86_400;
        let due = e.target(now);
        let shares = e.close_epoch(now);
        let total: Amount = shares.iter().map(|(_, s)| s).sum();
        assert_eq!(total + e.pot, due + 1_000, "rien ne se perd");
        let (three, one) = (i128::from(shares[0].1), i128::from(shares[1].1));
        assert!((three - 3 * one).abs() <= 3, "{three} / {one}");
        assert!(e.proposed.is_empty());
        assert_eq!(e.emitted, due);
        // Époque suivante sans bloc : l'émission due attend dans la cagnotte.
        let later = now + 86_400;
        assert!(e.close_epoch(later).is_empty());
        assert_eq!(e.emitted, e.target(later));
    }

    #[test]
    fn emission_never_exceeds_the_budget() {
        let mut e = emission();
        e.record(&KeyPair::generate().public_key());
        e.close_epoch(e.start + 1_000 * HALF_LIFE);
        assert!(e.emitted <= GENESIS_EMISSION);
        assert_eq!(e.remaining(), GENESIS_EMISSION - e.emitted);
    }
}
