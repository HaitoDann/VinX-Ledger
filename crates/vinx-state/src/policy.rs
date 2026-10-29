//! Règles de compte : multisig et plafond journalier (protocole 4, SPEC §5.8).
//!
//! Un compte sans politique se dépense avec sa seule clé, comme avant. Avec
//! une politique :
//! - `threshold` approbations de ses signataires autorisent tout ;
//! - sans elles, si un plafond est fixé, la clé du compte peut dépenser jusqu'à
//!   `daily_limit` par jour UTC, mais pas changer la politique ni la clé ;
//! - sans plafond, toute transaction exige les approbations (multisig pur).

use crate::table::{Row, Table};
use borsh::{BorshDeserialize, BorshSerialize};
use vinx_core::{Amount, Approval, MAX_POLICY_SIGNERS, PolicyConfig};
use vinx_crypto::Address;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Policy {
    pub config: PolicyConfig,
    /// Jour (horodatage / 86 400) auquel se rapporte `spent`.
    pub day: u64,
    /// Dépensé sans approbation pendant `day`.
    pub spent: Amount,
}

/// Politiques des comptes qui en ont une.
pub type Policies = Table<Address, Policy>;

impl Row for Policy {
    const TAG: u8 = 3;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error(
        "politique invalide : 1 à 10 signataires distincts, seuil entre 1 et leur nombre, plafond seulement avec un seuil"
    )]
    InvalidConfig,
    #[error("approbations invalides (indices, signatures)")]
    InvalidApprovals,
    #[error("approbations insuffisantes : ce compte exige les signatures de ses signataires")]
    ApprovalsMissing,
    #[error("plafond journalier dépassé : il faut l'approbation d'un signataire")]
    DailyLimitExceeded,
}

/// `None` : la configuration retire la politique (tout à zéro).
pub fn validate(config: &PolicyConfig) -> Result<Option<()>, PolicyError> {
    let empty = config.signers.is_empty() && config.threshold == 0 && config.daily_limit.is_none();
    if empty {
        return Ok(None);
    }
    let n = config.signers.len();
    let distinct = config.signers.iter().enumerate().all(|(i, k)| {
        !config
            .signers
            .iter()
            .skip(i.saturating_add(1))
            .any(|o| o == k)
    });
    let ok = (1..=MAX_POLICY_SIGNERS).contains(&n)
        && distinct
        && config.threshold >= 1
        && usize::from(config.threshold) <= n;
    if ok {
        Ok(Some(()))
    } else {
        Err(PolicyError::InvalidConfig)
    }
}

/// Nombre d'approbations valides de `approvals` sur `message`, pour `config`.
/// Les indices doivent être strictement croissants.
pub fn count_approvals(
    config: &PolicyConfig,
    approvals: &[Approval],
    message: &[u8],
) -> Result<u8, PolicyError> {
    let mut last: Option<u8> = None;
    for a in approvals {
        if last.is_some_and(|l| a.signer_index <= l) {
            return Err(PolicyError::InvalidApprovals);
        }
        last = Some(a.signer_index);
        let key = config
            .signers
            .get(usize::from(a.signer_index))
            .ok_or(PolicyError::InvalidApprovals)?;
        key.verify(message, &a.signature)
            .map_err(|_| PolicyError::InvalidApprovals)?;
    }
    u8::try_from(approvals.len()).map_err(|_| PolicyError::InvalidApprovals)
}

impl Policy {
    /// Ajoute `amount` aux dépenses sans approbation du jour `day` ; refuse au-delà du plafond.
    pub fn spend_unapproved(&mut self, day: u64, amount: Amount) -> Result<(), PolicyError> {
        let limit = self
            .config
            .daily_limit
            .ok_or(PolicyError::ApprovalsMissing)?;
        if self.day != day {
            self.day = day;
            self.spent = 0;
        }
        let total = self
            .spent
            .checked_add(amount)
            .ok_or(PolicyError::DailyLimitExceeded)?;
        if total > limit {
            return Err(PolicyError::DailyLimitExceeded);
        }
        self.spent = total;
        Ok(())
    }
}
