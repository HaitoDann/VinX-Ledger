use crate::CryptoError;
use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::Signer;
use rand::rngs::OsRng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone)]
pub struct KeyPair {
    signing_key: ed25519_dalek::SigningKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct PublicKey(pub(crate) [u8; 32]);

/// Ed25519 signature (64 bytes), serialized as a lowercase hex string.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct VinxSignature(pub(crate) [u8; 64]);

impl Serialize for VinxSignature {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for VinxSignature {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let hex_str = <String as serde::Deserialize>::deserialize(d)?;
        let bytes = hex::decode(&hex_str).map_err(serde::de::Error::custom)?;
        let arr: [u8; 64] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 64-byte signature"))?;
        Ok(VinxSignature(arr))
    }
}

impl KeyPair {
    pub fn generate() -> Self {
        Self {
            signing_key: ed25519_dalek::SigningKey::generate(&mut OsRng),
        }
    }

    pub fn from_secret_bytes(bytes: &[u8; 32]) -> Self {
        Self {
            signing_key: ed25519_dalek::SigningKey::from_bytes(bytes),
        }
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.signing_key.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> VinxSignature {
        VinxSignature(self.signing_key.sign(message).to_bytes())
    }
}

impl PublicKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn verify(&self, message: &[u8], signature: &VinxSignature) -> Result<(), CryptoError> {
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&self.0)
            .map_err(|_| CryptoError::InvalidKeyBytes)?;
        let sig = ed25519_dalek::Signature::from_bytes(&signature.0);
        // VINX-24: `verify_strict`, not `verify`. The permissive path accepts small-order
        // public keys and mixed-order components, so a single signature can verify under
        // more than one public key. On a chain where `Address` is derived from the public
        // key, that weakens the "one signature, one signer" property the whole transaction
        // model rests on. Strict verification also rejects non-canonical encodings, which
        // keeps signature validity identical on every node — a divergence here is a fork.
        vk.verify_strict(message, &sig)
            .map_err(|_| CryptoError::InvalidSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_keypair() {
        let kp = KeyPair::generate();
        let pk = kp.public_key();
        assert_ne!(pk.as_bytes(), &[0u8; 32]);
    }

    #[test]
    fn test_sign_and_verify() {
        let kp = KeyPair::generate();
        let pk = kp.public_key();
        let message = b"transfer 100 VinX to vinx1abc";
        let sig = kp.sign(message);
        assert!(pk.verify(message, &sig).is_ok());
    }

    #[test]
    fn test_verify_fails_wrong_message() {
        let kp = KeyPair::generate();
        let pk = kp.public_key();
        let sig = kp.sign(b"original message");
        assert_eq!(
            pk.verify(b"tampered message", &sig),
            Err(CryptoError::InvalidSignature)
        );
    }

    #[test]
    fn test_verify_fails_wrong_key() {
        let kp1 = KeyPair::generate();
        let kp2 = KeyPair::generate();
        let msg = b"pay alice 50 VinX";
        let sig = kp1.sign(msg);
        assert_eq!(
            kp2.public_key().verify(msg, &sig),
            Err(CryptoError::InvalidSignature)
        );
    }

    #[test]
    fn test_roundtrip_secret_bytes() {
        let kp = KeyPair::generate();
        let secret = kp.secret_bytes();
        let kp2 = KeyPair::from_secret_bytes(&secret);
        assert_eq!(kp.public_key(), kp2.public_key());
    }

    #[test]
    fn test_signature_is_deterministic() {
        let kp = KeyPair::generate();
        let msg = b"same message";
        let s1 = kp.sign(msg);
        let s2 = kp.sign(msg);
        assert_eq!(s1, s2);
    }

    /// VINX-24 — verification must be strict. `verify` (non-strict) accepts small-order
    /// public keys and mixed-order components, so one signature can verify under more
    /// than one public key; `verify_strict` rejects them. A signature must also stay
    /// valid or invalid identically on every node, or validity itself forks.
    #[test]
    fn small_order_public_key_is_rejected() {
        // The canonical small-order Edwards point of order 8.
        let small_order = [
            0xc7u8, 0x17, 0x6a, 0x70, 0x3d, 0x4d, 0xd8, 0x4f, 0xba, 0x3c, 0x0b, 0x76, 0x0d, 0x10,
            0x67, 0x0f, 0x2a, 0x20, 0x53, 0xfa, 0x2c, 0x39, 0xcc, 0xc6, 0x4e, 0xc7, 0xfd, 0x77,
            0x92, 0xac, 0x03, 0x7a,
        ];
        let pk = PublicKey(small_order);
        // Whatever the signature bytes, a small-order key must never verify.
        let sig = VinxSignature([0u8; 64]);
        assert!(pk.verify(b"any message", &sig).is_err());
    }

    /// Strictness must not break ordinary signatures.
    #[test]
    fn honest_signature_still_verifies_under_strict() {
        let kp = KeyPair::generate();
        let msg = b"vinx strict verification";
        let sig = kp.sign(msg);
        kp.public_key().verify(msg, &sig).expect("valid signature");
        assert!(kp.public_key().verify(b"other message", &sig).is_err());
    }
}
