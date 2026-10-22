//! Représentations texte des types du protocole, pour le JSON et la ligne de commande.

use anyhow::{Context, Result, anyhow};
use vinx_crypto::{Hash, PublicKey};

/// Clé publique en hexadécimal, octet d'algorithme compris (`01…` pour Ed25519).
pub fn key_to_hex(key: &PublicKey) -> String {
    hex::encode(key.to_tagged_bytes())
}

pub fn key_from_hex(s: &str) -> Result<PublicKey> {
    let bytes = hex::decode(s.trim()).context("clé publique : hexadécimal invalide")?;
    borsh::from_slice(&bytes).map_err(|_| anyhow!("clé publique invalide ou algorithme non actif"))
}

pub fn hash_to_hex(hash: &Hash) -> String {
    hex::encode(hash)
}

/// Montant en unités, encodé en chaîne : les entiers de plus de 2⁵³ perdent
/// leur précision en JavaScript.
pub fn amount_to_string(amount: u64) -> String {
    amount.to_string()
}

pub fn amount_from_str(s: &str) -> Result<u64> {
    s.parse().with_context(|| format!("montant invalide : {s}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    #[test]
    fn key_roundtrip_and_rejections() {
        let pk = KeyPair::generate().public_key();
        let s = key_to_hex(&pk);
        assert!(s.starts_with("01"));
        assert_eq!(key_from_hex(&s).unwrap(), pk);
        assert!(key_from_hex("02").is_err());
        assert!(key_from_hex("zz").is_err());
        assert!(key_from_hex(&format!("{s}00")).is_err());
    }
}
