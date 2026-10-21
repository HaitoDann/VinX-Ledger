//! Adresses (SPEC §2.2).

use crate::hash::{Domain, hash_tagged};
use crate::{CryptoError, PublicKey};
use bech32::{FromBase32, ToBase32, Variant};
use borsh::{BorshDeserialize, BorshSerialize};
use std::fmt;
use std::str::FromStr;

/// Préfixe Bech32m des adresses.
pub const HRP: &str = "vinx";

/// Longueur d'une adresse en octets.
pub const ADDRESS_LEN: usize = 20;

/// Adresse de 20 octets. Le Bech32m `vinx1…` n'est qu'une forme d'affichage.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub struct Address([u8; ADDRESS_LEN]);

impl Address {
    /// `H("vinx/address" ‖ algorithme ‖ clé)[0..20]`.
    pub fn from_public_key(key: &PublicKey) -> Self {
        Self::derive(&key.to_tagged_bytes())
    }

    /// Adresse de la trésorerie : `H("vinx/address" ‖ "treasury")[0..20]`.
    ///
    /// Aucune clé ne peut la produire : une clé typée commence par un octet
    /// d'algorithme (0x01, 0x02), jamais par `t`.
    pub fn treasury() -> Self {
        Self::derive(b"treasury")
    }

    fn derive(input: &[u8]) -> Self {
        let hash = hash_tagged(Domain::Address, input);
        let mut out = [0u8; ADDRESS_LEN];
        for (o, h) in out.iter_mut().zip(hash) {
            *o = h;
        }
        Address(out)
    }

    pub const fn from_bytes(bytes: [u8; ADDRESS_LEN]) -> Self {
        Address(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; ADDRESS_LEN] {
        &self.0
    }

    pub fn to_bech32(&self) -> String {
        // L'encodage ne peut échouer qu'avec un préfixe invalide ; HRP est constant et valide.
        bech32::encode(HRP, self.0.to_base32(), Variant::Bech32m).unwrap_or_default()
    }

    /// Décode `vinx1…`. Refuse un autre préfixe, la variante Bech32 (non m),
    /// la casse mixte et toute longueur autre que 20 octets.
    pub fn from_bech32(s: &str) -> Result<Self, CryptoError> {
        let bad = |m: String| CryptoError::InvalidAddress(m);
        let (hrp, data, variant) = bech32::decode(s).map_err(|e| bad(e.to_string()))?;
        if hrp != HRP {
            return Err(bad(format!("préfixe « {hrp} » au lieu de « {HRP} »")));
        }
        if variant != Variant::Bech32m {
            return Err(bad("variante Bech32 au lieu de Bech32m".into()));
        }
        let payload = Vec::<u8>::from_base32(&data).map_err(|e| bad(e.to_string()))?;
        let bytes: [u8; ADDRESS_LEN] = payload
            .try_into()
            .map_err(|p: Vec<u8>| bad(format!("{} octets au lieu de {ADDRESS_LEN}", p.len())))?;
        Ok(Address(bytes))
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_bech32())
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({self})")
    }
}

impl FromStr for Address {
    type Err = CryptoError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_bech32(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyPair;

    #[test]
    fn derivation_is_deterministic_and_typed() {
        let pk = KeyPair::from_secret_bytes(&[1; 32]).public_key();
        let a = Address::from_public_key(&pk);
        assert_eq!(a, Address::from_public_key(&pk));
        let expected = hash_tagged(Domain::Address, &pk.to_tagged_bytes());
        assert_eq!(a.as_bytes()[..], expected[..ADDRESS_LEN]);
    }

    #[test]
    fn bech32m_roundtrip() {
        let a = Address::from_public_key(&KeyPair::generate().public_key());
        let s = a.to_string();
        assert!(s.starts_with("vinx1"));
        assert_eq!(s.parse::<Address>().unwrap(), a);
        assert_eq!(s.to_uppercase().parse::<Address>().unwrap(), a);
    }

    #[test]
    fn treasury_is_stable_and_distinct() {
        assert_eq!(Address::treasury(), Address::treasury());
        let a = Address::from_public_key(&KeyPair::generate().public_key());
        assert_ne!(a, Address::treasury());
    }

    #[test]
    fn typo_is_detected() {
        let s = Address::treasury().to_string();
        let mut chars: Vec<char> = s.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == 'q' { 'p' } else { 'q' };
        let typo: String = chars.into_iter().collect();
        assert!(typo.parse::<Address>().is_err());
    }

    #[test]
    fn wrong_prefix_variant_or_length_is_rejected() {
        let data = [7u8; ADDRESS_LEN].to_base32();
        let other = bech32::encode("btc", &data, Variant::Bech32m).unwrap();
        assert!(other.parse::<Address>().is_err());
        let legacy = bech32::encode(HRP, &data, Variant::Bech32).unwrap();
        assert!(legacy.parse::<Address>().is_err());
        let short = bech32::encode(HRP, [7u8; 19].to_base32(), Variant::Bech32m).unwrap();
        assert!(short.parse::<Address>().is_err());
        let ok = bech32::encode(HRP, &data, Variant::Bech32m).unwrap();
        assert_eq!(ok.parse::<Address>().unwrap(), Address::from_bytes([7; 20]));
    }

    #[test]
    fn mixed_case_is_rejected() {
        let s = Address::treasury().to_string();
        let mixed = format!("V{}", &s[1..]);
        assert!(mixed.parse::<Address>().is_err());
    }
}
