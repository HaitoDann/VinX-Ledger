//! Réserve du protocole et gouvernance votée (protocole 8, modèle réserve,
//! docs/PROTOCOLE-8.md, ADR 0003).
//!
//! - **Réserve** : 900 M VINX fixés à la genèse, sans clé. Elle ne sort que
//!   par une demande acceptée, dans la limite de 5 M par mois et de 100 000
//!   par demande (constantes du protocole).
//! - **Propositions** : demandes de dépense (tout compte, avec caution) et
//!   changements de paramètre (proposés par l'admin seul). Vote des
//!   validateurs (plus des 2/3, 7 jours), puis, sauf paramètre technique,
//!   vote des détenteurs (14 jours, VINX bloqués, quorum, majorité).
//! - **Versement** : file par ordre d'acceptation, plafond mensuel.

use crate::table::{Row, Table};
use borsh::{BorshDeserialize, BorshSerialize};
use vinx_core::{Amount, ParamId, UNITS_PER_VINX};
use vinx_crypto::{Address, Hash, PublicKey};

/// Réserve d'une genèse au modèle réserve : 900 000 000 VINX.
pub const GENESIS_RESERVE: Amount = 900_000_000 * UNITS_PER_VINX;
/// Plafond mensuel des versements de la réserve : 5 000 000 VINX.
pub const MONTHLY_CAP: Amount = 5_000_000 * UNITS_PER_VINX;
/// Durée d'un mois de la réserve, en secondes.
pub const MONTH: u64 = 2_629_800;
/// Durée du vote des validateurs : 7 jours.
pub const VALIDATOR_VOTE_PERIOD: u64 = 7 * 86_400;
/// Durée du vote des détenteurs : 14 jours.
pub const HOLDER_VOTE_PERIOD: u64 = 14 * 86_400;
/// Nombre maximal de votants détenteurs par proposition.
pub const MAX_HOLDER_VOTERS: u32 = 10_000;

/// Paramètres de la gouvernance (`SetParam` / `ProposeParam`, `0x16` à `0x18`).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct GovernanceParams {
    /// Caution d'une demande de dépense, en unités.
    pub proposal_deposit: Amount,
    /// Quorum des détenteurs, en dix-millièmes de la circulation.
    pub holder_quorum: u64,
    /// Quorum minimal des détenteurs, en unités.
    pub holder_quorum_min: Amount,
}

impl Default for GovernanceParams {
    fn default() -> Self {
        Self {
            proposal_deposit: 1_000 * UNITS_PER_VINX,
            holder_quorum: 1_000,
            holder_quorum_min: 100_000 * UNITS_PER_VINX,
        }
    }
}

/// Objet d'une proposition.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum ProposalKind {
    Spend {
        beneficiary: Address,
        amount: Amount,
        title: String,
        url: String,
    } = 0,
    Param {
        param: ParamId,
        value: u64,
    } = 1,
}

/// Étape d'une proposition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum Phase {
    /// Vote des validateurs jusqu'à `ends_at` (secondes).
    Validators { ends_at: u64 } = 0,
    /// Vote des détenteurs jusqu'à `ends_at` (secondes).
    Holders { ends_at: u64 } = 1,
    /// Dépense acceptée, en file de versement.
    Queued = 2,
    /// Dépense versée au bloc `at`.
    Paid { at: u64 } = 3,
    /// Paramètre appliqué au bloc `at`.
    Applied { at: u64 } = 4,
    /// Refusée au bloc `at`.
    Rejected { at: u64 } = 5,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Proposal {
    pub kind: ProposalKind,
    /// Auteur : le compte qui a déposé la demande (la trésorerie pour une
    /// proposition de l'admin).
    pub author: Address,
    /// Caution encore bloquée (0 une fois rendue ou versée à la réserve).
    pub deposit: Amount,
    /// Hauteur du dépôt.
    pub submitted_at: u64,
    pub phase: Phase,
    /// Votes des validateurs, par clé ; le dernier vote compte.
    pub validator_votes: Vec<(PublicKey, bool)>,
    /// VINX bloqués pour et contre (vote des détenteurs).
    pub yes: Amount,
    pub no: Amount,
    pub voters: u32,
    /// Quorum exigé, fixé à l'ouverture du vote des détenteurs (unités).
    pub quorum: Amount,
}

impl Row for Proposal {
    const TAG: u8 = 15;
    fn locked(&self) -> u128 {
        u128::from(self.deposit)
    }
}

/// VINX bloqués par un détenteur pour une proposition.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct HolderLock {
    pub amount: Amount,
    pub approve: bool,
}

impl Row for HolderLock {
    const TAG: u8 = 16;
    fn locked(&self) -> u128 {
        u128::from(self.amount)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Governance {
    /// Modèle réserve, fixé à la genèse. Faux : modèle escrow, aucune des
    /// règles de ce module ne s'applique.
    pub enabled: bool,
    pub reserve: Amount,
    /// Début du mois en cours (secondes).
    pub month_start: u64,
    /// Versé depuis le début du mois.
    pub spent_this_month: Amount,
    pub params: GovernanceParams,
    pub proposals: Table<Hash, Proposal>,
    pub locks: Table<(Hash, Address), HolderLock>,
    /// Dépenses acceptées, dans l'ordre d'acceptation.
    pub queue: Vec<Hash>,
    /// Propositions dont un vote est en cours, dans l'ordre de dépôt.
    pub open: Vec<Hash>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GovernanceError {
    #[error("réservé aux chaînes au modèle réserve")]
    NoReserve,
    #[error("montant de la demande hors limites (1 unité à 100 000 VINX)")]
    BadAmount,
    #[error("titre (1 à 96 octets) ou lien (200 octets au plus) invalide")]
    BadText,
    #[error("proposition inconnue")]
    UnknownProposal,
    #[error("cette proposition n'est pas à cette étape de vote")]
    WrongPhase,
    #[error("seul un validateur actif peut voter à cette étape")]
    NotValidator,
    #[error("le bénéficiaire ne vote pas sur sa propre demande")]
    Beneficiary,
    #[error("un vote complété doit garder le même sens")]
    ChangedSide,
    #[error("trop de votants pour cette proposition")]
    TooManyVoters,
    #[error("action réservée au modèle escrow (la réserve se dépense par vote)")]
    EscrowOnly,
}

impl Governance {
    /// VINX bloqués : cautions des propositions et votes des détenteurs.
    pub fn locked(&self) -> u128 {
        self.proposals.locked().saturating_add(self.locks.locked())
    }

    /// Validateurs qui comptent pour une proposition : l'ensemble actif, sans
    /// celui dont l'adresse est bénéficiaire.
    pub fn electorate(kind: &ProposalKind, active: &[PublicKey]) -> Vec<PublicKey> {
        active
            .iter()
            .filter(|k| match kind {
                ProposalKind::Spend { beneficiary, .. } => {
                    Address::from_public_key(k) != *beneficiary
                }
                ProposalKind::Param { .. } => true,
            })
            .cloned()
            .collect()
    }

    /// Plus des 2/3 de l'électorat approuvent.
    pub fn validators_approve(p: &Proposal, electorate: &[PublicKey]) -> bool {
        let yes = electorate
            .iter()
            .filter(|k| p.validator_votes.iter().any(|(v, a)| v == *k && *a))
            .count();
        !electorate.is_empty() && yes.saturating_mul(3) > electorate.len().saturating_mul(2)
    }

    /// Quorum des détenteurs pour une circulation donnée (unités).
    pub fn quorum_for(&self, circulation: u128) -> Amount {
        let share = circulation.saturating_mul(u128::from(self.params.holder_quorum)) / 10_000;
        Amount::try_from(share)
            .unwrap_or(Amount::MAX)
            .max(self.params.holder_quorum_min)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn keys(n: usize) -> Vec<PublicKey> {
        (0..n).map(|_| KeyPair::generate().public_key()).collect()
    }

    fn proposal(kind: ProposalKind) -> Proposal {
        Proposal {
            kind,
            author: Address::treasury(),
            deposit: 0,
            submitted_at: 1,
            phase: Phase::Validators { ends_at: 10 },
            validator_votes: Vec::new(),
            yes: 0,
            no: 0,
            voters: 0,
            quorum: 0,
        }
    }

    #[test]
    fn more_than_two_thirds_of_the_electorate() {
        let k = keys(4);
        let mut p = proposal(ProposalKind::Param {
            param: ParamId::MinFee,
            value: 1,
        });
        p.validator_votes = vec![(k[0].clone(), true), (k[1].clone(), true)];
        assert!(!Governance::validators_approve(&p, &k));
        p.validator_votes.push((k[2].clone(), true));
        assert!(Governance::validators_approve(&p, &k), "3 sur 4");
        p.validator_votes[2].1 = false;
        assert!(!Governance::validators_approve(&p, &k), "vote changé");
        // Un seul validateur : son accord suffit.
        p.validator_votes = vec![(k[0].clone(), true)];
        assert!(Governance::validators_approve(&p, &k[..1]));
    }

    #[test]
    fn a_beneficiary_validator_is_left_out() {
        let k = keys(4);
        let kind = ProposalKind::Spend {
            beneficiary: Address::from_public_key(&k[3]),
            amount: 1,
            title: "x".into(),
            url: String::new(),
        };
        let e = Governance::electorate(&kind, &k);
        assert_eq!(e.len(), 3);
        let mut p = proposal(kind);
        // 2 sur 3 ne suffit pas, il faut plus des 2/3 : les 3.
        p.validator_votes = vec![(k[0].clone(), true), (k[1].clone(), true)];
        assert!(!Governance::validators_approve(&p, &e));
        p.validator_votes.push((k[2].clone(), true));
        assert!(Governance::validators_approve(&p, &e));
    }

    #[test]
    fn quorum_has_a_floor() {
        let g = Governance::default();
        let vinx = u128::from(UNITS_PER_VINX);
        assert_eq!(g.quorum_for(10_000 * vinx), g.params.holder_quorum_min);
        assert_eq!(g.quorum_for(10_000_000 * vinx), 1_000_000 * UNITS_PER_VINX);
    }
}
