use serde::{Deserialize, Serialize};
use vinx_crypto::Address;
use crate::protocol::ProtocolVersion;
use crate::amount::Amount;

/// Voting period: ~72 hours at 10 s/block.
pub const GOVERNANCE_VOTING_PERIOD_BLOCKS: u64 = 25_920;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CoffreCondition {
    MicaCasp,
    ExternalAudit,
    PublicPolicy,
}

/// Action to execute when a governance proposal passes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum GovernanceAction {
    AddValidator(Address),
    RemoveValidator(Address),
    UpdateFeeFloor { atoms: u64 },
    ScheduleUpgrade { version: ProtocolVersion, activation_height: u64 },
    /// Allocate some of the melt pool back into the staking pool.
    ReleaseMeltToStaking { amount: Amount },
    /// Rotate the admin key to a new address. Requires governance quorum.
    RotateAdmin(Address),
    /// Mark one of the 3 Coffre Maturité unlock conditions as met.
    MarkCoffreCondition(CoffreCondition),
    /// Transfer Coffre Maturité to staking pool. Requires all 3 conditions to be met.
    UnlockCoffre,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ProposalStatus {
    Active,
    Passed,
    Rejected,
    Executed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    pub id: u64,
    pub proposer: Address,
    pub description: String,
    pub action: GovernanceAction,
    pub submitted_at: u64,
    pub voting_ends_at: u64,
    /// Validator addresses that voted YES.
    pub yes_votes: Vec<String>,
    /// Validator addresses that voted NO.
    pub no_votes: Vec<String>,
    pub status: ProposalStatus,
}

impl Proposal {
    pub fn yes_count(&self) -> usize { self.yes_votes.len() }
    pub fn no_count(&self) -> usize { self.no_votes.len() }
    pub fn has_voted(&self, addr: &str) -> bool {
        self.yes_votes.iter().any(|v| v == addr) || self.no_votes.iter().any(|v| v == addr)
    }
    /// Quorum = same threshold as PoA finality: ceil(2n/3).
    pub fn has_quorum(&self, n_validators: usize) -> bool {
        if n_validators == 0 { return false; }
        let q = (2 * n_validators + 2) / 3;
        self.yes_count() >= q
    }
    /// True when quorum is mathematically unreachable regardless of remaining votes.
    pub fn is_dead(&self, n_validators: usize) -> bool {
        let q = (2 * n_validators + 2) / 3;
        let remaining = n_validators.saturating_sub(self.yes_count() + self.no_count());
        self.yes_count() + remaining < q
    }
}

/// Encoded in Transaction.payload for SubmitProposal txs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubmitProposalPayload {
    pub description: String,
    pub action: GovernanceAction,
    /// Number of blocks for the voting window (default GOVERNANCE_VOTING_PERIOD_BLOCKS).
    pub voting_period_blocks: u64,
}

/// Encoded in Transaction.payload for VoteProposal txs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VotePayload {
    pub proposal_id: u64,
    pub approve: bool,
}
