//! Hachage BLAKE3, signatures typées et adresses Bech32m (SPEC §2).

pub mod address;
pub mod hash;
pub mod keys;
pub mod merkle;

pub use address::{ADDRESS_LEN, Address};
pub use hash::{Domain, Hash, hash_tagged, hash_tagged_parts, tagged};
pub use keys::{Algorithm, KeyPair, PublicKey, Signature};
pub use merkle::{MerkleProof, Side, merkle_proof, merkle_root, verify_merkle_proof};

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("clé publique invalide")]
    InvalidKey,
    #[error("signature invalide")]
    InvalidSignature,
    #[error("adresse invalide : {0}")]
    InvalidAddress(String),
}
