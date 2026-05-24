use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum CoreError {
    #[error("Insufficient balance")]
    InsufficientBalance,
    #[error("Invalid nonce: expected {expected}, got {got}")]
    InvalidNonce { expected: u64, got: u64 },
    #[error("Account is frozen")]
    AccountFrozen,
    #[error("Supply cap exceeded")]
    SupplyCapExceeded,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Public key does not match sender address")]
    PubKeyMismatch,
    #[error("Amount overflow")]
    AmountOverflow,
    #[error("Invalid transaction: {0}")]
    InvalidTransaction(String),
    #[error("Crypto error: {0}")]
    Crypto(String),
}

impl From<vinx_crypto::CryptoError> for CoreError {
    fn from(e: vinx_crypto::CryptoError) -> Self {
        CoreError::Crypto(e.to_string())
    }
}
