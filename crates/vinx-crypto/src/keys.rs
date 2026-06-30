use crate::CryptoError;
use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{Signer, Verifier};
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
        vk.verify(message, &sig)
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
}
