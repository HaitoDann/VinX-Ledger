use crate::protocol::ProtocolVersion;
use serde::{Deserialize, Serialize};
use vinx_crypto::Address;

/// Action executed immediately by the admin key.
/// Community proposals happen off-chain; this is the on-chain execution step only.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum GovernanceAction {
    AddValidator(Address),
    RemoveValidator(Address),
    UpdateFeeFloor {
        atoms: u64,
    },
    ScheduleUpgrade {
        version: ProtocolVersion,
        activation_height: u64,
    },
    /// Rotate the admin key to a new address.
    RotateAdmin(Address),
}
