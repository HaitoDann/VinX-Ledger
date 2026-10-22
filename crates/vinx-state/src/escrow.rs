//! Escrow et libérations plafonnées (SPEC §4.2, ADR 0001 §4).

use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::VecDeque;
use vinx_core::Amount;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Tranche {
    pub amount: Amount,
    /// Horodatage Unix, en secondes.
    pub matures_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Escrow {
    /// Ordonnées par échéance croissante.
    pub tranches: VecDeque<Tranche>,
    /// Durée d'une période, en secondes.
    pub period: u64,
}

/// Période de la genèse : un douzième d'année de 365,25 jours.
pub const GENESIS_PERIOD: u64 = 2_629_800;

/// Nombre de tranches à la genèse (10 ans).
pub const GENESIS_TRANCHES: u64 = 120;

/// Montant d'une tranche à la genèse : 7 500 000 VINX.
pub const GENESIS_TRANCHE_AMOUNT: Amount = 7_500_000 * vinx_core::UNITS_PER_VINX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscrowError {
    NoTranche,
    NotMatured,
    AboveCap,
    Overflow,
}

impl Escrow {
    /// Escrow de la genèse : la tranche `i` échoit à `genesis_time + (i + 1) × period`.
    pub fn genesis(genesis_time: u64) -> Option<Self> {
        let mut tranches = VecDeque::new();
        let mut matures_at = genesis_time;
        for _ in 0..GENESIS_TRANCHES {
            matures_at = matures_at.checked_add(GENESIS_PERIOD)?;
            tranches.push_back(Tranche {
                amount: GENESIS_TRANCHE_AMOUNT,
                matures_at,
            });
        }
        Some(Self {
            tranches,
            period: GENESIS_PERIOD,
        })
    }

    pub fn total(&self) -> u128 {
        self.tranches.iter().map(|t| u128::from(t.amount)).sum()
    }

    /// Échéance d'une tranche ajoutée maintenant en fin de liste.
    fn next_maturity(&self, fallback: u64) -> Option<u64> {
        let last = self.tranches.back().map_or(fallback, |t| t.matures_at);
        last.checked_add(self.period)
    }

    /// Vérifie qu'on peut libérer `amount` de la première tranche à `now` (secondes).
    pub fn check_release(&self, amount: Amount, now: u64) -> Result<(), EscrowError> {
        let first = self.tranches.front().ok_or(EscrowError::NoTranche)?;
        if first.matures_at > now {
            return Err(EscrowError::NotMatured);
        }
        if amount > first.amount {
            return Err(EscrowError::AboveCap);
        }
        let remainder = first.amount.saturating_sub(amount);
        if remainder > 0 {
            self.next_maturity(first.matures_at)
                .ok_or(EscrowError::Overflow)?;
        }
        Ok(())
    }

    /// Libère `amount` de la première tranche ; le reliquat part en fin de liste.
    /// Renvoie le montant libéré. Appeler [`Self::check_release`] d'abord.
    pub fn release(&mut self, amount: Amount, now: u64) -> Result<Amount, EscrowError> {
        self.check_release(amount, now)?;
        let first = self.tranches.pop_front().ok_or(EscrowError::NoTranche)?;
        let remainder = first.amount.saturating_sub(amount);
        if remainder > 0 {
            let matures_at = self
                .next_maturity(first.matures_at)
                .ok_or(EscrowError::Overflow)?;
            self.tranches.push_back(Tranche {
                amount: remainder,
                matures_at,
            });
        }
        Ok(amount)
    }

    /// Ajoute une tranche de `amount` en fin de liste (retour depuis la trésorerie).
    pub fn push_back(&mut self, amount: Amount, now: u64) -> Result<(), EscrowError> {
        let matures_at = self.next_maturity(now).ok_or(EscrowError::Overflow)?;
        self.tranches.push_back(Tranche { amount, matures_at });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::UNITS_PER_VINX;

    const T0: u64 = 1_800_000_000;

    #[test]
    fn genesis_schedule_matches_spec() {
        let e = Escrow::genesis(T0).unwrap();
        assert_eq!(e.tranches.len(), 120);
        assert_eq!(e.total(), u128::from(900_000_000 * UNITS_PER_VINX));
        assert_eq!(e.tranches[0].matures_at, T0 + GENESIS_PERIOD);
        // 120 périodes = exactement 10 années de 365,25 jours.
        assert_eq!(e.tranches[119].matures_at, T0 + 10 * 36_525 * 864);
    }

    #[test]
    fn cannot_release_before_maturity_or_above_cap() {
        let mut e = Escrow::genesis(T0).unwrap();
        let due = T0 + GENESIS_PERIOD;
        assert_eq!(e.release(1, due - 1), Err(EscrowError::NotMatured));
        assert_eq!(
            e.release(GENESIS_TRANCHE_AMOUNT + 1, due),
            Err(EscrowError::AboveCap)
        );
        assert_eq!(e.tranches.len(), 120);
    }

    #[test]
    fn partial_release_moves_remainder_to_the_end() {
        let mut e = Escrow::genesis(T0).unwrap();
        let due = T0 + GENESIS_PERIOD;
        let last = e.tranches[119].matures_at;
        e.release(GENESIS_TRANCHE_AMOUNT / 3, due).unwrap();
        assert_eq!(e.tranches.len(), 120);
        let back = e.tranches.back().unwrap();
        assert_eq!(
            back.amount,
            GENESIS_TRANCHE_AMOUNT - GENESIS_TRANCHE_AMOUNT / 3
        );
        assert_eq!(back.matures_at, last + GENESIS_PERIOD);
        // La tranche suivante n'est pas échue : on ne peut pas accélérer.
        assert_eq!(e.release(1, due), Err(EscrowError::NotMatured));
    }

    #[test]
    fn zero_release_postpones_the_whole_tranche() {
        let mut e = Escrow::genesis(T0).unwrap();
        let total = e.total();
        e.release(0, T0 + GENESIS_PERIOD).unwrap();
        assert_eq!(e.total(), total);
        assert_eq!(e.tranches.len(), 120);
    }

    #[test]
    fn full_release_removes_the_tranche() {
        let mut e = Escrow::genesis(T0).unwrap();
        e.release(GENESIS_TRANCHE_AMOUNT, T0 + GENESIS_PERIOD)
            .unwrap();
        assert_eq!(e.tranches.len(), 119);
    }

    #[test]
    fn push_back_extends_the_calendar() {
        let mut e = Escrow::genesis(T0).unwrap();
        let last = e.tranches.back().unwrap().matures_at;
        e.push_back(5, T0).unwrap();
        assert_eq!(e.tranches.back().unwrap().matures_at, last + GENESIS_PERIOD);
        let mut empty = Escrow {
            tranches: VecDeque::new(),
            period: 10,
        };
        empty.push_back(5, 1_000).unwrap();
        assert_eq!(empty.tranches[0].matures_at, 1_010);
    }
}
