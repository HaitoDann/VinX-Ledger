//! Paramètres du protocole (SPEC §11).

use crate::Amount;
use borsh::{BorshDeserialize, BorshSerialize};

/// Paramètres modifiables par `SetParam`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Params {
    pub min_fee: Amount,
    pub min_reserve: Amount,
    pub max_memo_bytes: u64,
    pub max_block_txs: u64,
    pub max_block_bytes: u64,
    pub timeout_propose_ms: u64,
    pub timeout_propose_delta_ms: u64,
    pub timeout_vote_ms: u64,
    pub timeout_vote_delta_ms: u64,
    pub min_block_interval_ms: u64,
}

impl Default for Params {
    /// Valeurs initiales de SPEC §11.
    fn default() -> Self {
        Self {
            min_fee: 1_000_000,
            min_reserve: 100_000_000,
            max_memo_bytes: 32,
            max_block_txs: 5_000,
            max_block_bytes: 4 * 1024 * 1024,
            timeout_propose_ms: 3_000,
            timeout_propose_delta_ms: 1_000,
            timeout_vote_ms: 1_000,
            timeout_vote_delta_ms: 500,
            min_block_interval_ms: 5_000,
        }
    }
}

/// Dérive d'horloge maximale tolérée pour l'horodatage d'un bloc (ms).
pub const MAX_CLOCK_DRIFT_MS: u64 = 15_000;

/// Préavis minimal d'une mise à jour du protocole, en blocs.
pub const MIN_UPGRADE_DELAY: u64 = 17_280;

/// Identifiant d'un paramètre modifiable (SPEC §11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum ParamId {
    MinFee = 0x01,
    MinReserve = 0x02,
    MaxMemoBytes = 0x03,
    MaxBlockTxs = 0x04,
    MaxBlockBytes = 0x05,
    TimeoutPropose = 0x06,
    TimeoutProposeDelta = 0x07,
    TimeoutVote = 0x08,
    TimeoutVoteDelta = 0x09,
    MinBlockInterval = 0x0A,
    /// Protocole 2 : prix d'un an de nom `.vinx`, tenu hors de [`Params`].
    NameFee = 0x0B,
    /// Protocole 5 : prix de la création d'un actif, tenu hors de [`Params`].
    AssetFee = 0x0C,
    /// Protocole 6 : réserve immobilisée par objet possédé, tenue hors de [`Params`].
    ObjectReserve = 0x0D,
    // Protocole 7 (docs/PROTOCOLE-7.md §8), tenus hors de [`Params`].
    /// Caution exigée d'un validateur, en unités.
    ValidatorBond = 0x0E,
    /// Disponibilité minimale pour être récompensé, en dix-millièmes.
    MinAvailability = 0x0F,
    /// Part du pot d'époque versée au prorata des signatures, en dix-millièmes.
    VoteShare = 0x10,
    /// Blocs consécutifs sans signature avant la mise à l'écart.
    AbsenceBlocks = 0x11,
    /// Blocs minimaux entre la mise à l'écart et le retour.
    JailMinBlocks = 0x12,
    /// Délai de restitution de la caution après le départ, en secondes.
    BondUnlockDelay = 0x13,
    /// Délai d'exécution des actions admin, en secondes.
    AdminDelay = 0x14,
    /// Règle de diversité des opérateurs : 0 (désactivée) ou 1.
    Diversity = 0x15,
    // Protocole 8 (docs/PROTOCOLE-8.md), tenus hors de [`Params`].
    /// Caution d'une demande de dépense de la réserve, en unités.
    ProposalDeposit = 0x16,
    /// Quorum du vote des détenteurs, en dix-millièmes de la circulation.
    HolderQuorum = 0x17,
    /// Quorum minimal du vote des détenteurs, en unités.
    HolderQuorumMin = 0x18,
}

impl ParamId {
    /// Paramètres du protocole 8, tenus avec la gouvernance.
    pub fn is_governance(self) -> bool {
        matches!(
            self,
            ParamId::ProposalDeposit | ParamId::HolderQuorum | ParamId::HolderQuorumMin
        )
    }

    /// Protocole 8, modèle réserve : paramètre économique, voté par les
    /// validateurs puis par les détenteurs. Les autres (techniques) sont
    /// votés par les validateurs seuls.
    pub fn is_economic(self) -> bool {
        matches!(
            self,
            ParamId::MinFee
                | ParamId::MinReserve
                | ParamId::ObjectReserve
                | ParamId::NameFee
                | ParamId::AssetFee
                | ParamId::ValidatorBond
                | ParamId::MinAvailability
                | ParamId::VoteShare
                | ParamId::ProposalDeposit
                | ParamId::HolderQuorum
                | ParamId::HolderQuorumMin
        )
    }

    /// Paramètres du protocole 7, tenus dans la table des validateurs.
    pub fn is_staking(self) -> bool {
        matches!(
            self,
            ParamId::ValidatorBond
                | ParamId::MinAvailability
                | ParamId::VoteShare
                | ParamId::AbsenceBlocks
                | ParamId::JailMinBlocks
                | ParamId::BondUnlockDelay
                | ParamId::AdminDelay
                | ParamId::Diversity
        )
    }
}

impl Params {
    /// Sans effet pour [`ParamId::NameFee`] et [`ParamId::AssetFee`], rangés
    /// ailleurs dans l'état.
    pub fn set(&mut self, id: ParamId, value: u64) {
        let slot = match id {
            ParamId::NameFee | ParamId::AssetFee | ParamId::ObjectReserve => return,
            ParamId::ValidatorBond
            | ParamId::MinAvailability
            | ParamId::VoteShare
            | ParamId::AbsenceBlocks
            | ParamId::JailMinBlocks
            | ParamId::BondUnlockDelay
            | ParamId::AdminDelay
            | ParamId::Diversity => return,
            ParamId::ProposalDeposit | ParamId::HolderQuorum | ParamId::HolderQuorumMin => return,
            ParamId::MinFee => &mut self.min_fee,
            ParamId::MinReserve => &mut self.min_reserve,
            ParamId::MaxMemoBytes => &mut self.max_memo_bytes,
            ParamId::MaxBlockTxs => &mut self.max_block_txs,
            ParamId::MaxBlockBytes => &mut self.max_block_bytes,
            ParamId::TimeoutPropose => &mut self.timeout_propose_ms,
            ParamId::TimeoutProposeDelta => &mut self.timeout_propose_delta_ms,
            ParamId::TimeoutVote => &mut self.timeout_vote_ms,
            ParamId::TimeoutVoteDelta => &mut self.timeout_vote_delta_ms,
            ParamId::MinBlockInterval => &mut self.min_block_interval_ms,
        };
        *slot = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_codes_match_spec() {
        assert_eq!(crate::encode(&ParamId::MinFee), [0x01]);
        assert_eq!(crate::encode(&ParamId::MinBlockInterval), [0x0A]);
        assert!(crate::decode::<ParamId>(&[0x00]).is_err());
        assert_eq!(crate::encode(&ParamId::Diversity), [0x15]);
        assert_eq!(crate::encode(&ParamId::HolderQuorumMin), [0x18]);
        assert!(crate::decode::<ParamId>(&[0x19]).is_err());
    }

    #[test]
    fn set_updates_the_right_field() {
        let mut p = Params::default();
        p.set(ParamId::MinBlockInterval, 3_000);
        assert_eq!(p.min_block_interval_ms, 3_000);
        assert_eq!(p.min_fee, Params::default().min_fee);
    }
}
