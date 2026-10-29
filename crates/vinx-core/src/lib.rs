//! Types du protocole : comptes, transactions et blocs (SPEC §3, §5, §6).

pub mod account;
pub mod admin;
pub mod amount;
pub mod block;
pub mod consensus;
pub mod params;
pub mod transaction;

pub use account::Account;
pub use admin::{
    ADMIN_SIGNERS, ADMIN_THRESHOLD, AdminAction, AdminTransaction, Approval, SignedAdminTransaction,
};
pub use amount::{Amount, TOTAL_SUPPLY, UNITS_PER_VINX};
pub use block::{Block, BlockHeader};
pub use consensus::{CommitCertificate, Vote, VoteKind, quorum};
pub use params::{ParamId, Params};
pub use transaction::{
    MAX_BATCH_PAYMENTS, MAX_MANDATE_PERIOD, MAX_POLICY_SIGNERS, MIN_MANDATE_PERIOD, Payment,
    PolicyConfig, SignedTransaction, Transaction, TxBody, deal_id,
};
pub use vinx_crypto::{Address, Hash, PublicKey, Signature};

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("encodage invalide")]
    Encoding,
    #[error("signature invalide")]
    InvalidSignature,
    #[error("approbations admin invalides : {0}")]
    InvalidApprovals(&'static str),
    #[error("certificat de commit invalide : {0}")]
    InvalidCertificate(&'static str),
}

/// Encodage borsh canonique d'une valeur.
pub fn encode<T: borsh::BorshSerialize>(value: &T) -> Vec<u8> {
    // L'écriture dans un Vec ne peut pas échouer.
    borsh::to_vec(value).unwrap_or_default()
}

/// Décodage borsh strict : refuse les octets en trop et toute valeur inconnue.
pub fn decode<T: borsh::BorshDeserialize>(bytes: &[u8]) -> Result<T, CoreError> {
    borsh::from_slice(bytes).map_err(|_| CoreError::Encoding)
}
