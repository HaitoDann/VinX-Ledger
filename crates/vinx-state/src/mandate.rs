//! Mandats de prélèvement (protocole 4, SPEC §5.9).
//!
//! Le payeur autorise un bénéficiaire à prélever jusqu'à un montant par
//! période. Le bénéficiaire prélève lui-même (`Collect`) ; le payeur peut
//! révoquer le mandat à tout moment.

use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use vinx_core::Amount;
use vinx_crypto::Address;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Mandate {
    pub max_per_period: Amount,
    /// Durée d'une période, en secondes.
    pub period: u64,
    /// Début de la première période (horodatage du bloc d'octroi).
    pub start: u64,
    pub expires_at: Option<u64>,
    /// Période à laquelle se rapporte `used`.
    pub period_index: u64,
    /// Déjà prélevé pendant `period_index`.
    pub used: Amount,
}

/// Mandats par (payeur, bénéficiaire).
pub type Mandates = Arc<BTreeMap<(Address, Address), Mandate>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MandateError {
    #[error("mandat invalide : bénéficiaire, montant, période (1 à 366 jours) ou échéance")]
    Invalid,
    #[error("aucun mandat de ce payeur pour ce bénéficiaire")]
    Unknown,
    #[error("mandat expiré")]
    Expired,
    #[error("montant au-delà de ce que le mandat permet pour cette période")]
    Exceeded,
}

impl Mandate {
    /// Prélève `amount` à l'instant `now` : vérifie l'échéance et la limite de
    /// la période en cours, et met à jour le compteur.
    pub fn collect(&mut self, now: u64, amount: Amount) -> Result<(), MandateError> {
        if self.expires_at.is_some_and(|e| now >= e) {
            return Err(MandateError::Expired);
        }
        let index = now
            .saturating_sub(self.start)
            .checked_div(self.period)
            .ok_or(MandateError::Invalid)?;
        if index != self.period_index {
            self.period_index = index;
            self.used = 0;
        }
        let total = self
            .used
            .checked_add(amount)
            .ok_or(MandateError::Exceeded)?;
        if total > self.max_per_period {
            return Err(MandateError::Exceeded);
        }
        self.used = total;
        Ok(())
    }

    /// Reste prélevable pendant la période qui contient `now`.
    pub fn available(&self, now: u64) -> Amount {
        let index = now
            .saturating_sub(self.start)
            .checked_div(self.period)
            .unwrap_or(0);
        let used = if index == self.period_index {
            self.used
        } else {
            0
        };
        self.max_per_period.saturating_sub(used)
    }
}
