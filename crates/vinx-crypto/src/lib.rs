pub mod address;
pub mod bls;
pub mod hash;
pub mod keys;
pub mod merkle;
pub mod smt;
pub mod tx_tree;

pub use address::Address;
pub use bls::{
    aggregate as bls_aggregate, verify as bls_verify, verify_aggregate as bls_verify_aggregate,
    BlsError, BlsPubKey, BlsSecretKey, BlsSignature, BLS_COSIG_DST, BLS_POP_DST,
};
pub use hash::{hash256, Hash32};
pub use keys::{KeyPair, KeyType, PublicKey, VinxSignature};
pub use merkle::{
    merkle_proof_for, merkle_root, verify_merkle_proof, IncrementalMerkleTree, MerkleProofStep,
};

pub use smt::{SmtProof, SparseMerkleTree, SMT_EMPTY};
pub use tx_tree::{tx_proof, tx_root, verify_tx_proof};

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
