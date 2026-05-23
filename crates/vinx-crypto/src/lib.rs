pub mod hash;
pub mod keys;
pub mod address;

pub use hash::{sha256, Hash32};
pub use keys::{KeyPair, PublicKey, VinxSignature};
pub use address::Address;

use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum CryptoError {
    #[error("Invalid key bytes")]
    InvalidKeyBytes,
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[error("Signature verification failed")]
    InvalidSignature,
}
