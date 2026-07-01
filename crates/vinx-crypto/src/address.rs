use crate::{hash::sha256, CryptoError, PublicKey};
use bech32::{self, FromBase32, ToBase32, Variant};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

pub const BECH32_HRP: &str = "vinx";

/// Length in bytes of an address payload (first 20 bytes of SHA-256(pubkey)).
pub const ADDRESS_LEN: usize = 20;

/// A VinX Ledger address: the raw 20-byte payload (`SHA-256(public_key)[..20]`).
///
/// The canonical form is the raw bytes — signatures ([`crate::Address::as_bytes`]
/// via `signing_bytes`), Merkle leaf hashing and on-disk/wire encodings all operate
/// on them directly. Bech32 (`vinx1...`) is purely a display/transport encoding
/// applied at the edges (RPC JSON, CLI, logs). Storing the bytes makes `Address`
/// `Copy`, 20 bytes inline with no heap allocation, and cheap to hash and compare.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address([u8; ADDRESS_LEN]);

impl Address {
    pub fn from_public_key(pk: &PublicKey) -> Self {
        let hash = sha256(pk.as_bytes());
        let mut payload = [0u8; ADDRESS_LEN];
        payload.copy_from_slice(&hash[..ADDRESS_LEN]);
        Address(payload)
    }

    /// Builds an address directly from its 20-byte payload.
    pub fn from_bytes(bytes: [u8; ADDRESS_LEN]) -> Self {
        Address(bytes)
    }

    /// Returns the canonical all-zeros placeholder address (`vinx1qqqq...`).
    /// Useful as a sentinel / uninitialized value.  Never holds real funds.
    pub fn zero() -> Self {
        Address([0u8; ADDRESS_LEN])
    }

    pub fn from_bech32(s: &str) -> Result<Self, CryptoError> {
        let (hrp, data_u5, variant) =
            bech32::decode(s).map_err(|e| CryptoError::InvalidAddress(e.to_string()))?;
        if hrp != BECH32_HRP {
            return Err(CryptoError::InvalidAddress(format!(
                "expected HRP '{}', got '{}'",
                BECH32_HRP, hrp
            )));
        }
        if variant != Variant::Bech32 {
            return Err(CryptoError::InvalidAddress(
                "expected Bech32 variant".to_string(),
            ));
        }
        let payload = Vec::<u8>::from_base32(&data_u5)
            .map_err(|e| CryptoError::InvalidAddress(e.to_string()))?;
        if payload.len() != ADDRESS_LEN {
            return Err(CryptoError::InvalidAddress(format!(
                "expected {}-byte payload, got {}",
                ADDRESS_LEN,
                payload.len()
            )));
        }
        let mut bytes = [0u8; ADDRESS_LEN];
        bytes.copy_from_slice(&payload);
        Ok(Address(bytes))
    }

    /// Returns the raw 20-byte payload — the canonical form used for signing,
    /// hashing and binary encodings.
    pub fn as_bytes(&self) -> &[u8; ADDRESS_LEN] {
        &self.0
    }

    /// Encodes the address to its bech32 string form (`vinx1...`).
    /// Allocates — call only at display/transport boundaries, never in hot loops.
    pub fn to_bech32(&self) -> String {
        bech32::encode(BECH32_HRP, self.0.to_base32(), Variant::Bech32)
            .expect("bech32 encoding is infallible for valid inputs")
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_bech32())
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({})", self.to_bech32())
    }
}

impl FromStr for Address {
    type Err = CryptoError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Address::from_bech32(s)
    }
}

// serde is format-aware: human-readable formats (JSON, used by the RPC API and CLI)
// carry the bech32 string `"vinx1..."`, while binary formats (bincode on disk) carry
// the raw 20 bytes. Borsh (P2P wire) is always the raw 20 bytes.
impl Serialize for Address {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.to_bech32())
        } else {
            serde::Serialize::serialize(&self.0, serializer)
        }
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            let s = <String as Deserialize>::deserialize(deserializer)?;
            Address::from_bech32(&s).map_err(serde::de::Error::custom)
        } else {
            let bytes = <[u8; ADDRESS_LEN] as Deserialize>::deserialize(deserializer)?;
            Ok(Address(bytes))
        }
    }
}

impl BorshSerialize for Address {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(&self.0)
    }
}

impl BorshDeserialize for Address {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut bytes = [0u8; ADDRESS_LEN];
        reader.read_exact(&mut bytes)?;
        Ok(Address(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::KeyPair;

    #[test]
    fn test_address_starts_with_vinx1() {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        assert!(addr.to_bech32().starts_with("vinx1"), "address: {}", addr);
    }

    #[test]
    fn test_address_is_deterministic() {
        let kp = KeyPair::generate();
        let pk = kp.public_key();
        let a1 = Address::from_public_key(&pk);
        let a2 = Address::from_public_key(&pk);
        assert_eq!(a1, a2);
    }

    #[test]
    fn test_different_keys_different_addresses() {
        let addr1 = Address::from_public_key(&KeyPair::generate().public_key());
        let addr2 = Address::from_public_key(&KeyPair::generate().public_key());
        assert_ne!(addr1, addr2);
    }

    #[test]
    fn test_address_roundtrip() {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        let parsed = Address::from_bech32(&addr.to_bech32()).unwrap();
        assert_eq!(addr, parsed);
        assert_eq!(addr.as_bytes(), parsed.as_bytes());
    }

    #[test]
    fn test_from_public_key_is_sha256_prefix() {
        let kp = KeyPair::generate();
        let pk = kp.public_key();
        let addr = Address::from_public_key(&pk);
        assert_eq!(addr.as_bytes(), &sha256(pk.as_bytes())[..ADDRESS_LEN]);
    }

    #[test]
    fn test_invalid_hrp_rejected() {
        // A valid bech32 string but wrong HRP
        let err = Address::from_bech32("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        assert!(err.is_err());
    }

    #[test]
    fn test_malformed_address_rejected() {
        assert!(Address::from_bech32("not_an_address").is_err());
        assert!(Address::from_bech32("").is_err());
    }

    #[test]
    fn test_address_display() {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        assert_eq!(format!("{}", addr), addr.to_bech32());
    }

    // Format contract for the 20-byte representation:
    // - JSON (human-readable) carries the bech32 string, so the RPC API and the
    //   TypeScript SDK are unaffected.
    // - bincode (on disk) and borsh (P2P wire) carry the raw 20 bytes.

    #[test]
    fn test_json_is_the_bech32_string() {
        let addr = Address::from_public_key(&KeyPair::generate().public_key());
        let json = serde_json::to_string(&addr).unwrap();
        assert_eq!(json, format!("\"{}\"", addr.to_bech32()));
        let back: Address = serde_json::from_str(&json).unwrap();
        assert_eq!(addr, back);
    }

    #[test]
    fn test_bincode_is_raw_20_bytes() {
        let addr = Address::from_public_key(&KeyPair::generate().public_key());
        let bytes = bincode::serialize(&addr).unwrap();
        assert_eq!(bytes.len(), ADDRESS_LEN);
        assert_eq!(bytes.as_slice(), addr.as_bytes());
        let back: Address = bincode::deserialize(&bytes).unwrap();
        assert_eq!(addr, back);
    }

    #[test]
    fn test_borsh_is_raw_20_bytes() {
        let addr = Address::from_public_key(&KeyPair::generate().public_key());
        let bytes = borsh::to_vec(&addr).unwrap();
        assert_eq!(bytes.len(), ADDRESS_LEN);
        assert_eq!(bytes.as_slice(), addr.as_bytes());
        let back: Address = borsh::from_slice(&bytes).unwrap();
        assert_eq!(addr, back);
    }

    #[test]
    fn test_bech32_vector_for_js_cross_check() {
        // These exact strings are decoded by the web UI's bech32Decode20 (rpc/ui.rs)
        // back to the raw payloads below; keeping this stable guarantees the browser
        // signer and the node agree on the 20-byte address bytes.
        let a = Address::from_bech32("vinx1zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3feqld3").unwrap();
        assert_eq!(a.as_bytes(), &[0x11u8; 20]);
        assert_eq!(Address::from_bytes([0x11u8; 20]).to_bech32(), a.to_bech32());
    }

    #[test]
    fn test_address_is_copy() {
        // Compile-time proof that Address is Copy: used by value without a move error.
        let addr = Address::zero();
        let a = addr;
        let b = addr;
        assert_eq!(a, b);
    }
}
