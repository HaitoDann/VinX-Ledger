//! État global, escrow, application des transactions et invariant de supply (SPEC §4).

pub mod accounts;
pub mod emission;
pub mod escrow;
pub mod genesis;
pub mod registry;
pub mod sigcache;
pub mod smt;
pub mod state;
pub mod validators;

pub use accounts::Accounts;
pub use emission::Emission;
pub use escrow::{Escrow, Tranche};
pub use genesis::{Genesis, GenesisError};
pub use registry::{Deal, NameRecord, Registry};
pub use state::{AdminSet, BlockError, PendingUpgrade, State, TxError};
pub use validators::{ValidatorChange, ValidatorSet, validators_hash};
