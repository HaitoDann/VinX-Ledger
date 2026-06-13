pub mod address;
pub mod hash;
pub mod keys;
pub mod merkle;

pub use address::Address;
pub use hash::{sha256, Hash32};
pub use keys::{KeyPair, PublicKey, VinxSignature};
pub use merkle::{merkle_proof_for, merkle_root, verify_merkle_proof, MerkleProofStep};

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
