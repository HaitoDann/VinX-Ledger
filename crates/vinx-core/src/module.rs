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
