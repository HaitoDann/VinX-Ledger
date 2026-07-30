use serde::{Deserialize, Serialize};
use vinx_crypto::Hash32;

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
/// vectors (ADR 0020).
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
