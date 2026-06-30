use crate::{hash::sha256, CryptoError, PublicKey};
use bech32::{self, FromBase32, ToBase32, Variant};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub const BECH32_HRP: &str = "vinx";

/// A VinX Ledger address in Bech32 format: `vinx1...`
/// Derived from the first 20 bytes of SHA-256(public_key).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Address(String);

impl Address {
    pub fn from_public_key(pk: &PublicKey) -> Self {
        let hash = sha256(pk.as_bytes());
        let payload = &hash[..20];
        let encoded = bech32::encode(BECH32_HRP, payload.to_base32(), Variant::Bech32)
            .expect("bech32 encoding is infallible for valid inputs");
        Address(encoded)
    }

    /// Returns the canonical all-zeros placeholder address (`vinx1qqqq...`).
    /// Useful as a sentinel / uninitialized value.  Never holds real funds.
    pub fn zero() -> Self {
        let payload = [0u8; 20];
        let encoded = bech32::encode(BECH32_HRP, payload.to_base32(), Variant::Bech32)
            .expect("bech32 encoding is infallible");
        Address(encoded)
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
        if payload.len() != 20 {
            return Err(CryptoError::InvalidAddress(format!(
                "expected 20-byte payload, got {}",
                payload.len()
            )));
        }
        Ok(Address(s.to_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Address {
    type Err = CryptoError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Address::from_bech32(s)
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
        assert!(addr.as_str().starts_with("vinx1"), "address: {}", addr);
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
        let parsed = Address::from_bech32(addr.as_str()).unwrap();
        assert_eq!(addr, parsed);
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
        assert_eq!(format!("{}", addr), addr.as_str());
    }
}
