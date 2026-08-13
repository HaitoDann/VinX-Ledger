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
    /// Replace the admin authority with a K-of-M committee (ADR 0011): `threshold`
    /// signatures among `signers` are required to enact any governance action. A single
    /// signer with `threshold == 1` is equivalent to the legacy single admin key.
    SetAdminPolicy {
        signers: Vec<Address>,
        threshold: u16,
    },
    /// Update the minimum validator bond (ADR 0038). Governable within
    /// [MIN_BOND_HARD_FLOOR, MAX_BOND_HARD_CAP], changes capped at ±BOND_STEP_BPS (25 %)
    /// per modification, with BOND_COOLDOWN_SECS (7 days) between modifications.
    ///
    /// Appended after discriminant 5 (SetAdminPolicy) — discriminant 6.
    UpdateMinValidatorBond {
        atoms: u128,
    },
    /// Update the target active-set size N (ADR 0038). Changes limited to ±ACTIVE_SET_STEP
    /// per modification, with ACTIVE_SET_COOLDOWN_SECS (7 days) between modifications.
    /// N must remain ≥ MIN_ACTIVE_SET_SIZE (5).
    ///
    /// Appended after UpdateMinValidatorBond — discriminant 7.
    UpdateActiveSetSize {
        new_size: u32,
    },
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
            GovernanceAction::SetAdminPolicy {
                signers: vec![addr, Address::from_bytes([0x22; 20])],
                threshold: 2,
            },
            GovernanceAction::UpdateMinValidatorBond { atoms: 123_456 },
            GovernanceAction::UpdateActiveSetSize { new_size: 23 },
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

    #[test]
    fn test_governance_action_golden_vectors() {
        // ADR 0020 t2: pin the EXACT bincode bytes of every GovernanceAction variant.
        // Stronger than the round-trip test above — this catches a discriminant shift or a
        // field reorder that is still internally consistent, which would silently break
        // cross-implementation signature verification of the AdminAction payload.
        let a = Address::from_bytes([0x11; 20]);
        let b = Address::from_bytes([0x22; 20]);
        let cases: [(GovernanceAction, &str); 8] = [
            (
                GovernanceAction::AddValidator(a),
                "000000001111111111111111111111111111111111111111",
            ),
            (
                GovernanceAction::RemoveValidator(a),
                "010000001111111111111111111111111111111111111111",
            ),
            (
                GovernanceAction::UpdateFeeFloor { atoms: 123_456 },
                "0200000040e2010000000000",
            ),
            (
                GovernanceAction::ScheduleUpgrade {
                    version: ProtocolVersion::new(1, 2, 3),
                    activation_ts: 999,
                },
                "03000000010002000300e703000000000000",
            ),
            (
                GovernanceAction::RotateAdmin(a),
                "040000001111111111111111111111111111111111111111",
            ),
            (
                GovernanceAction::SetAdminPolicy {
                    signers: vec![a, b],
                    threshold: 2,
                },
                "050000000200000000000000111111111111111111111111111111111111111122222222222222222222222222222222222222220200",
            ),
            (
                GovernanceAction::UpdateMinValidatorBond { atoms: 123_456 },
                "0600000040e20100000000000000000000000000",
            ),
            (
                GovernanceAction::UpdateActiveSetSize { new_size: 23 },
                "0700000017000000",
            ),
        ];
        for (action, hex_want) in cases {
            assert_eq!(hex::encode(bincode::serialize(&action).unwrap()), hex_want);
        }
    }
}
