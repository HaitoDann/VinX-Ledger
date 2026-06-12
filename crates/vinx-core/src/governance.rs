use serde::{Deserialize, Serialize};
use vinx_crypto::Address;
use crate::protocol::ProtocolVersion;
use crate::amount::Amount;

/// Action executed immediately by the VinX Labs admin key.
/// Community proposals happen off-chain; this is the on-chain execution step only.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum GovernanceAction {
    AddValidator(Address),
    RemoveValidator(Address),
    UpdateFeeFloor { atoms: u64 },
    ScheduleUpgrade { version: ProtocolVersion, activation_height: u64 },
    /// Transfer an amount from the melt pool to the distribution pool (pool des jetons à distribuer).
    ReleaseMeltToDistribution { amount: Amount },
    /// Rotate the admin key to a new address.
    RotateAdmin(Address),
    /// Mark one of the 3 Coffre Maturité unlock conditions as met.
    MarkCoffreCondition(CoffreCondition),
    /// Transfer Coffre Maturité to staking pool (requires all 3 conditions to be true).
    UnlockCoffre,
}

/// The three conditions required before the Coffre Maturité can be unlocked.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CoffreCondition {
    MicaCasp,
    ExternalAudit,
    PublicPolicy,
}
