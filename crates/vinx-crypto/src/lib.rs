pub mod address;
pub mod bls;
pub mod hash;
pub mod keys;
pub mod merkle;

pub use address::Address;
pub use bls::{
    aggregate as bls_aggregate, verify as bls_verify, verify_aggregate as bls_verify_aggregate,
    BlsError, BlsPubKey, BlsSecretKey, BlsSignature, BLS_COSIG_DST, BLS_POP_DST,
};
pub use hash::{sha256, Hash32};
pub use keys::{KeyPair, PublicKey, VinxSignature};
pub use merkle::{
    merkle_proof_for, merkle_root, verify_merkle_proof, IncrementalMerkleTree, MerkleProofStep,
};

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
