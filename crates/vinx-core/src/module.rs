use serde::{Deserialize, Serialize};
use vinx_crypto::{Address, Hash32};

/// One beneficiary in a module's fee-split table (ADR 0039).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FeeRecipient {
    pub address: Address,
    /// Fraction of the escrow payment owed to this recipient, in basis points
    /// (10 000 = 100%). The sum of all recipients' shares must be ≤ 10 000.
    pub share_bps: u16,
}

/// Fee distribution schedule registered by a module operator (ADR 0039).
/// On escrow release, each recipient receives `floor(amount × share_bps / 10_000)`;
/// the integer residual goes to `operator_address`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FeeSchedule {
    /// Indicative base price for one unit of service — not enforced by the L1,
    /// used as a reference for clients choosing between modules.
    pub base_fee_atoms: u128,
    /// Ordered list of beneficiaries and their basis-point shares.
    pub recipients: Vec<FeeRecipient>,
    /// Receives the integer-division residual from every escrow release.
    /// Also the address for on-chain operator interactions (anchoring, release).
    pub operator_address: Address,
}

/// A pending service-payment escrow locked on-chain (ADR 0039).
/// Created by a `ModuleEscrow` transaction; removed by `EscrowRelease` (release)
/// or `ModuleEscrowRefund` (client timeout refund).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EscrowEntry {
    /// Deterministic ID: `sha256(client || module_id || nonce_le8)`.
    pub escrow_id: Hash32,
    pub client: Address,
    pub module_id: Hash32,
    /// Atoms locked in escrow — debited from client at creation, distributed on release.
    pub amount_atoms: u128,
    /// Unix timestamp (seconds) of the block that included the `ModuleEscrow` tx.
    pub created_ts: u64,
    /// Seconds the client agreed to wait for delivery (bounded by protocol constants).
    pub timeout_secs: u64,
    /// Hash of the off-chain service parameters the client committed to at escrow creation.
    pub service_params_hash: Hash32,
}

/// A module-registry operation (ADR 0010), carried as the bincode payload of an
/// [`crate::TransactionType::AnchorState`] transaction.
///
/// VinX stays a **pure-currency L1**: it never executes module logic. It only records a
/// **bonded anchor** — an operator posts a bond and commits successive Merkle roots
/// (`anchor_head`) of their off-chain module's state. Fraud adjudication against that bond
/// is out of scope here (see ADR 0023); this ADR is the registry + anchoring primitive only.
///
/// Appended-variant discipline: the enum is versioned by bincode variant index, and this
/// type is new, so ordering here is free — but keep `Register` first for stable golden
/// vectors (ADR 0020). New variants are always appended last.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ModuleOp {
    /// Register a new module under `module_id`, locking `bond_atoms` from the operator
    /// (the transaction sender). Fails if the id is already taken or the bond is too small.
    Register { module_id: Hash32, bond_atoms: u128 },
    /// Advance the module's committed state to `anchor_head`. Operator-only.
    Anchor {
        module_id: Hash32,
        anchor_head: Hash32,
    },
    /// Deregister the module and return its bond to the operator's balance. Operator-only.
    Deregister { module_id: Hash32 },
    /// Register or update the module's fee-distribution schedule (ADR 0039).
    /// Operator-only. `sum(share_bps) ≤ 10 000`; the integer residual goes to
    /// `fee_schedule.operator_address`.  Appended at index 3 — never reorder.
    SetFeeSchedule {
        module_id: Hash32,
        fee_schedule: FeeSchedule,
    },
    /// Deterministic escrow-release trigger (ADR 0039). When the operator anchors a
    /// state root that includes this leaf, the L1 atomically distributes
    /// `pending_escrows[escrow_id].amount_atoms` per the module's `FeeSchedule` and
    /// removes the entry. Appended at index 4 — never reorder.
    EscrowRelease { escrow_id: Hash32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_op_encoding_is_canonical() {
        // ADR 0020: ModuleOp enters the AnchorState transaction payload (signed) — pin that
        // its bincode encoding is deterministic and canonical (decode then re-encode is
        // byte-identical).
        let id = [0x33u8; 32];
        let ops = [
            ModuleOp::Register {
                module_id: id,
                bond_atoms: 1_000_000_000_000_000_000_000,
            },
            ModuleOp::Anchor {
                module_id: id,
                anchor_head: [0x44u8; 32],
            },
            ModuleOp::Deregister { module_id: id },
        ];
        for op in ops {
            let bytes = bincode::serialize(&op).unwrap();
            assert_eq!(bytes, bincode::serialize(&op).unwrap());
            let decoded: ModuleOp = bincode::deserialize(&bytes).unwrap();
            assert_eq!(bincode::serialize(&decoded).unwrap(), bytes);
            assert_eq!(decoded, op);
        }
    }

    #[test]
    fn test_module_op_golden_vectors() {
        // ADR 0020 t2: pin the EXACT bincode bytes of every ModuleOp variant so a
        // discriminant shift or field reorder can never slip through unsigned.
        let id = [0x33u8; 32];
        let head = [0x44u8; 32];
        let cases: [(ModuleOp, &str); 3] = [
            (
                ModuleOp::Register {
                    module_id: id,
                    bond_atoms: 1_000_000_000_000_000_000_000,
                },
                "0000000033333333333333333333333333333333333333333333333333333333333333330000a0dec5adc9353600000000000000",
            ),
            (
                ModuleOp::Anchor {
                    module_id: id,
                    anchor_head: head,
                },
                "0100000033333333333333333333333333333333333333333333333333333333333333334444444444444444444444444444444444444444444444444444444444444444",
            ),
            (
                ModuleOp::Deregister { module_id: id },
                "020000003333333333333333333333333333333333333333333333333333333333333333",
            ),
        ];
        for (op, hex_want) in cases {
            assert_eq!(hex::encode(bincode::serialize(&op).unwrap()), hex_want);
        }
    }
}
