//! Consensus Tendermint à validateurs autorisés (SPEC §7).
//!
//! Le moteur ([`Engine`]) est une machine à états pure : il reçoit des messages
//! et des expirations de délais, et renvoie des actions. Il n'a ni réseau, ni
//! horloge, ni clé : le nœud signe, diffuse, programme les délais et lui
//! renvoie ses propres messages. Il est donc testable de façon déterministe.

pub mod engine;
pub mod evidence;
pub mod guard;
pub mod header;
pub mod messages;

pub use engine::{Action, BlockValidator, Engine, Timeout, TimeoutKind};
pub use evidence::{Evidence, EvidenceError};
pub use guard::SignGuard;
pub use header::{HeaderError, check_header_link, proposer_index};
pub use messages::{Proposal, SignedProposal, SignedVote};
