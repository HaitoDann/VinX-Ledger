pub mod hash;
pub mod keys;
pub mod address;
pub mod merkle;

pub use hash::{sha256, Hash32};
pub use keys::{KeyPair, PublicKey, VinxSignature};
pub use address::Address;
pub use merkle::{merkle_root, merkle_proof_for, verify_merkle_proof, MerkleProofStep};

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
