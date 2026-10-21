//! Clés publiques et signatures typées (SPEC §2.1).

use crate::CryptoError;
use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::Signer;
use std::fmt;
use std::io::{self, Read, Write};

/// Octet d'algorithme placé devant chaque clé publique et chaque signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Algorithm {
    /// Actif.
    Ed25519 = 0x01,
    /// Réservé : refusé tant qu'une mise à jour du protocole ne l'a pas activé.
    MlDsa44 = 0x02,
}

impl Algorithm {
    /// Algorithmes acceptés par cette version du protocole.
    fn decode_active(byte: u8) -> io::Result<Self> {
        match byte {
            0x01 => Ok(Algorithm::Ed25519),
            0x02 => Err(invalid("algorithme ML-DSA-44 non activé")),
            _ => Err(invalid("algorithme inconnu")),
        }
    }
}

fn invalid(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Clé publique typée. Encodage : `algorithme ‖ octets de la clé`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PublicKey {
    Ed25519([u8; 32]),
}

/// Signature typée. Encodage : `algorithme ‖ octets de la signature`.
#[derive(Clone, PartialEq, Eq)]
pub enum Signature {
    Ed25519([u8; 64]),
}

impl PublicKey {
    pub fn algorithm(&self) -> Algorithm {
        match self {
            PublicKey::Ed25519(_) => Algorithm::Ed25519,
        }
    }

    /// Octets de la clé, sans l'octet d'algorithme.
    pub fn key_bytes(&self) -> &[u8] {
        match self {
            PublicKey::Ed25519(bytes) => bytes,
        }
    }

    /// Encodage canonique `algorithme ‖ clé`, identique à l'encodage borsh.
    pub fn to_tagged_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.key_bytes().len().saturating_add(1));
        out.push(self.algorithm() as u8);
        out.extend_from_slice(self.key_bytes());
        out
    }

    /// Vérifie `signature` sur `message`.
    ///
    /// Ed25519 est vérifié en mode strict : rejet des clés de faible ordre et des
    /// encodages non canoniques, pour qu'une signature soit valide pour une seule
    /// clé et que tous les nœuds rendent le même verdict.
    pub fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), CryptoError> {
        match (self, signature) {
            (PublicKey::Ed25519(pk), Signature::Ed25519(sig)) => {
                let vk = ed25519_dalek::VerifyingKey::from_bytes(pk)
                    .map_err(|_| CryptoError::InvalidKey)?;
                let sig = ed25519_dalek::Signature::from_bytes(sig);
                vk.verify_strict(message, &sig)
                    .map_err(|_| CryptoError::InvalidSignature)
            }
        }
    }
}

impl Signature {
    pub fn algorithm(&self) -> Algorithm {
        match self {
            Signature::Ed25519(_) => Algorithm::Ed25519,
        }
    }
}

impl BorshSerialize for PublicKey {
    fn serialize<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(&[self.algorithm() as u8])?;
        writer.write_all(self.key_bytes())
    }
}

impl BorshDeserialize for PublicKey {
    fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
        match Algorithm::decode_active(u8::deserialize_reader(reader)?)? {
            Algorithm::Ed25519 => Ok(PublicKey::Ed25519(<[u8; 32]>::deserialize_reader(reader)?)),
            Algorithm::MlDsa44 => Err(invalid("algorithme ML-DSA-44 non activé")),
        }
    }
}

impl BorshSerialize for Signature {
    fn serialize<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(&[self.algorithm() as u8])?;
        match self {
            Signature::Ed25519(bytes) => writer.write_all(bytes),
        }
    }
}

impl BorshDeserialize for Signature {
    fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
        match Algorithm::decode_active(u8::deserialize_reader(reader)?)? {
            Algorithm::Ed25519 => Ok(Signature::Ed25519(<[u8; 64]>::deserialize_reader(reader)?)),
            Algorithm::MlDsa44 => Err(invalid("algorithme ML-DSA-44 non activé")),
        }
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({:?}, ", self.algorithm())?;
        for b in self.key_bytes() {
            write!(f, "{b:02x}")?;
        }
        write!(f, ")")
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature({:?})", self.algorithm())
    }
}

/// Paire de clés Ed25519. La clé secrète n'apparaît jamais dans `Debug`.
#[derive(Clone)]
pub struct KeyPair {
    signing_key: ed25519_dalek::SigningKey,
}

impl KeyPair {
    /// Génère une paire depuis le générateur aléatoire du système.
    pub fn generate() -> Self {
        Self {
            signing_key: ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng),
        }
    }

    pub fn from_secret_bytes(secret: &[u8; 32]) -> Self {
        Self {
            signing_key: ed25519_dalek::SigningKey::from_bytes(secret),
        }
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey::Ed25519(self.signing_key.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        Signature::Ed25519(self.signing_key.sign(message).to_bytes())
    }
}

impl fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyPair({:?}, secret: <masquée>)", self.public_key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let kp = KeyPair::generate();
        let sig = kp.sign(b"message");
        assert_eq!(kp.public_key().verify(b"message", &sig), Ok(()));
    }

    #[test]
    fn wrong_message_or_key_is_rejected() {
        let kp = KeyPair::generate();
        let sig = kp.sign(b"message");
        assert_eq!(
            kp.public_key().verify(b"autre", &sig),
            Err(CryptoError::InvalidSignature)
        );
        assert_eq!(
            KeyPair::generate().public_key().verify(b"message", &sig),
            Err(CryptoError::InvalidSignature)
        );
    }

    #[test]
    fn secret_roundtrip() {
        let kp = KeyPair::generate();
        let again = KeyPair::from_secret_bytes(&kp.secret_bytes());
        assert_eq!(kp.public_key(), again.public_key());
    }

    /// Vecteur RFC 8032 §7.1, test 1 : la signature est déterministe et conforme.
    #[test]
    fn rfc8032_vector() {
        let secret: [u8; 32] =
            hex::decode("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
                .unwrap()
                .try_into()
                .unwrap();
        let kp = KeyPair::from_secret_bytes(&secret);
        assert_eq!(
            hex::encode(kp.public_key().key_bytes()),
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        );
        let Signature::Ed25519(sig) = kp.sign(b"");
        assert_eq!(
            hex::encode(sig),
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155\
             5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        );
    }

    /// Une clé de faible ordre ne doit jamais valider une signature (vérification stricte).
    #[test]
    fn small_order_key_is_rejected() {
        let mut small_order = [0u8; 32];
        small_order[0] = 1; // point identité, d'ordre 1
        let pk = PublicKey::Ed25519(small_order);
        let mut sig = [0u8; 64];
        sig[0] = 1; // R = identité, s = 0 : accepté par une vérification non stricte
        assert!(pk.verify(b"m", &Signature::Ed25519(sig)).is_err());
    }

    /// Un `s` non réduit (≥ l) est rejeté : pas de signature malléable.
    #[test]
    fn non_canonical_s_is_rejected() {
        let kp = KeyPair::generate();
        let Signature::Ed25519(mut sig) = kp.sign(b"m");
        // Ajoute l'ordre du groupe l à s (little-endian) : même point, encodage différent.
        let l: [u8; 32] =
            hex::decode("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010")
                .unwrap()
                .try_into()
                .unwrap();
        let mut carry = 0u16;
        for (s, l) in sig[32..].iter_mut().zip(l) {
            let v = u16::from(*s) + u16::from(l) + carry;
            *s = v as u8;
            carry = v >> 8;
        }
        assert!(
            kp.public_key()
                .verify(b"m", &Signature::Ed25519(sig))
                .is_err()
        );
    }

    #[test]
    fn borsh_layout_uses_algorithm_byte() {
        let kp = KeyPair::generate();
        let pk = kp.public_key();
        let bytes = borsh::to_vec(&pk).unwrap();
        assert_eq!(bytes.len(), 33);
        assert_eq!(bytes[0], 0x01);
        assert_eq!(bytes, pk.to_tagged_bytes());
        assert_eq!(borsh::from_slice::<PublicKey>(&bytes).unwrap(), pk);

        let sig = kp.sign(b"m");
        let sig_bytes = borsh::to_vec(&sig).unwrap();
        assert_eq!((sig_bytes.len(), sig_bytes[0]), (65, 0x01));
        assert_eq!(borsh::from_slice::<Signature>(&sig_bytes).unwrap(), sig);
    }

    #[test]
    fn reserved_and_unknown_algorithms_are_rejected() {
        for byte in [0x00, 0x02, 0x03, 0xff] {
            let mut key = vec![byte];
            key.extend_from_slice(&[7u8; 32]);
            assert!(borsh::from_slice::<PublicKey>(&key).is_err());
            let mut sig = vec![byte];
            sig.extend_from_slice(&[7u8; 64]);
            assert!(borsh::from_slice::<Signature>(&sig).is_err());
        }
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut bytes = borsh::to_vec(&KeyPair::generate().public_key()).unwrap();
        bytes.push(0);
        assert!(borsh::from_slice::<PublicKey>(&bytes).is_err());
    }

    #[test]
    fn debug_hides_secret() {
        let kp = KeyPair::from_secret_bytes(&[0xab; 32]);
        let shown = format!("{kp:?}");
        assert!(shown.contains("masquée"));
        assert!(!shown.contains(&"ab".repeat(32)));
    }
}
