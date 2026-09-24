use thiserror::Error;

#[derive(Debug, Error)]
pub enum WalletError {
    #[error("Invalid amount: {0}")]
    InvalidAmount(String),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[error("Keystore error: {0}")]
    Keystore(String),
    #[error("Node unreachable at {0}: {1}")]
    NodeUnreachable(String, String),
    #[error("Node returned error: {0}")]
    NodeError(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("Invalid receipt: {0}")]
    Receipt(String),
}

impl From<std::io::Error> for WalletError {
    fn from(e: std::io::Error) -> Self {
        WalletError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for WalletError {
    fn from(e: serde_json::Error) -> Self {
        WalletError::Keystore(e.to_string())
    }
}

impl From<vinx_crypto::CryptoError> for WalletError {
    fn from(e: vinx_crypto::CryptoError) -> Self {
        WalletError::InvalidAddress(e.to_string())
    }
}
