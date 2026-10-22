//! État global, escrow, application des transactions et invariant de supply (SPEC §4).

pub mod escrow;
pub mod genesis;
pub mod smt;
pub mod state;
pub mod validators;

pub use escrow::{Escrow, Tranche};
pub use genesis::{Genesis, GenesisError};
pub use state::{AdminSet, BlockError, PendingUpgrade, State, TxError};
pub use validators::{ValidatorChange, ValidatorSet, validators_hash};
