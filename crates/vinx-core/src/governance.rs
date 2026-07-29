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
        /// Unix timestamp (seconds) at which the upgrade activates (ADR 0006).
        activation_ts: u64,
    },
    /// Rotate the admin key to a new address.
    RotateAdmin(Address),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ProtocolVersion;

    #[test]
    fn test_governance_action_encoding_is_canonical() {
        // ADR 0020: the bincode encoding of a GovernanceAction enters the AdminAction
        // transaction payload signed by the admin — it is consensus-critical. Pin that
        // it is deterministic and canonical (re-encoding a decoded value is identical),
        // so no accidental layout change slips through.
        let addr = Address::from_bytes([0x11; 20]);
        let actions = [
            GovernanceAction::AddValidator(addr),
            GovernanceAction::RemoveValidator(addr),
            GovernanceAction::UpdateFeeFloor { atoms: 123_456 },
            GovernanceAction::ScheduleUpgrade {
                version: ProtocolVersion::new(1, 2, 3),
                activation_ts: 999,
            },
            GovernanceAction::RotateAdmin(addr),
        ];
        for a in actions {
            let bytes = bincode::serialize(&a).unwrap();
            // Deterministic: encoding twice yields the same bytes.
            assert_eq!(bytes, bincode::serialize(&a).unwrap());
            // Canonical: decode then re-encode is byte-identical.
            let decoded: GovernanceAction = bincode::deserialize(&bytes).unwrap();
            assert_eq!(bincode::serialize(&decoded).unwrap(), bytes);
            assert_eq!(decoded, a);
        }
    }
}
