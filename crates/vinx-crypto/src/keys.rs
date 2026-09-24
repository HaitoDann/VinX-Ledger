use crate::CryptoError;
use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::Signer;
use rand::rngs::OsRng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone)]
pub struct KeyPair {
    signing_key: ed25519_dalek::SigningKey,
}

/// Account public key, tagged with its key type (ADR 0081 D6).
///
/// The borsh enum tag is the **key-type byte** of the protocol: `0 = Ed25519`. It is
/// committed in signing bytes and in address derivation, so new schemes (ML-DSA
/// post-quantum, P-256 passkeys, native multisig) can be added later as new variants
/// without breaking existing accounts, transactions or wallets. Variants are append-only:
/// never reorder or reuse a tag.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PublicKey {
    /// Tag 0.
    Ed25519([u8; 32]),
}

/// Account signature, tagged with the same key-type byte as [`PublicKey`].
/// Ed25519: 64 bytes, serialized in JSON as a lowercase hex string.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum VinxSignature {
    /// Tag 0.
    Ed25519([u8; 64]),
}

/// Key-type byte of the protocol (ADR 0081 D6) — the borsh tag of [`PublicKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyType {
    Ed25519 = 0,
}

// JSON keeps the pre-D6 shapes for Ed25519 (an array of 32 numbers for the key, a hex
// string for the signature), so the RPC API, the web wallet and the SDK are unchanged.
// A future key type will get an explicit tagged JSON form when it is introduced.
impl Serialize for PublicKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            PublicKey::Ed25519(bytes) => Serialize::serialize(bytes, s),
        }
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(PublicKey::Ed25519(<[u8; 32] as Deserialize>::deserialize(
            d,
        )?))
    }
}

impl Serialize for VinxSignature {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            VinxSignature::Ed25519(bytes) => s.serialize_str(&hex::encode(bytes)),
        }
    }
}

impl<'de> Deserialize<'de> for VinxSignature {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let hex_str = <String as serde::Deserialize>::deserialize(d)?;
        let bytes = hex::decode(&hex_str).map_err(serde::de::Error::custom)?;
        let arr: [u8; 64] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 64-byte signature"))?;
        Ok(VinxSignature::Ed25519(arr))
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
        PublicKey::Ed25519(self.signing_key.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> VinxSignature {
        VinxSignature::Ed25519(self.signing_key.sign(message).to_bytes())
    }
}

impl PublicKey {
    pub fn key_type(&self) -> KeyType {
        match self {
            PublicKey::Ed25519(_) => KeyType::Ed25519,
        }
    }

    /// Raw key material, without the key-type byte.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            PublicKey::Ed25519(bytes) => bytes,
        }
    }

    /// Canonical encoding: key-type byte ‖ key material — what signing bytes and address
    /// derivation commit to.
    pub fn to_tagged_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + self.as_bytes().len());
        out.push(self.key_type() as u8);
        out.extend_from_slice(self.as_bytes());
        out
    }

    pub fn verify(&self, message: &[u8], signature: &VinxSignature) -> Result<(), CryptoError> {
        let (PublicKey::Ed25519(pk), VinxSignature::Ed25519(sig)) = (self, signature);
        let vk = ed25519_dalek::VerifyingKey::from_bytes(pk)
            .map_err(|_| CryptoError::InvalidKeyBytes)?;
        let sig = ed25519_dalek::Signature::from_bytes(sig);
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
        assert_ne!(pk.as_bytes(), &[0u8; 32][..]);
        assert_eq!(pk.key_type(), KeyType::Ed25519);
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
        let pk = PublicKey::Ed25519(small_order);
        // Whatever the signature bytes, a small-order key must never verify.
        let sig = VinxSignature::Ed25519([0u8; 64]);
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

    /// ADR 0081 D6 — the borsh tag is the key-type byte, and JSON keeps its Ed25519 shape.
    #[test]
    fn key_type_byte_is_the_borsh_tag() {
        let kp = KeyPair::generate();
        let pk = kp.public_key();
        let bytes = borsh::to_vec(&pk).unwrap();
        assert_eq!(bytes.len(), 33);
        assert_eq!(bytes[0], KeyType::Ed25519 as u8);
        assert_eq!(&bytes[1..], pk.as_bytes());
        assert_eq!(pk.to_tagged_bytes(), bytes);
        let sig = borsh::to_vec(&kp.sign(b"m")).unwrap();
        assert_eq!((sig.len(), sig[0]), (65, 0));
        // An unknown key type is rejected at decoding.
        let mut unknown = bytes.clone();
        unknown[0] = 0xff;
        assert!(borsh::from_slice::<PublicKey>(&unknown).is_err());
        // JSON: unchanged array-of-32 form.
        let json = serde_json::to_string(&pk).unwrap();
        assert!(json.starts_with('[') && !json.contains("Ed25519"));
        assert_eq!(serde_json::from_str::<PublicKey>(&json).unwrap(), pk);
    }
}
