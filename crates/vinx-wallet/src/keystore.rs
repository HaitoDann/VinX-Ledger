use crate::error::WalletError;
use serde::{Deserialize, Serialize};
use std::path::Path;
use vinx_crypto::{Address, KeyPair};

#[derive(Serialize, Deserialize)]
pub struct KeyStore {
    pub address: String,
    pub secret_key_hex: String,
}

impl KeyStore {
    pub fn generate() -> (Self, KeyPair) {
        let kp = KeyPair::generate();
        let address = Address::from_public_key(&kp.public_key()).to_string();
        let secret_key_hex = hex::encode(kp.secret_bytes());
        (Self { address, secret_key_hex }, kp)
    }

    pub fn load(path: &Path) -> Result<Self, WalletError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            WalletError::Keystore(format!("cannot read {}: {}", path.display(), e))
        })?;
        serde_json::from_str(&content).map_err(|e| {
            WalletError::Keystore(format!("invalid keystore file: {}", e))
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), WalletError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json).map_err(|e| {
            WalletError::Keystore(format!("cannot write {}: {}", path.display(), e))
        })
    }

    pub fn to_keypair(&self) -> Result<KeyPair, WalletError> {
        let bytes = hex::decode(&self.secret_key_hex)
            .map_err(|e| WalletError::Keystore(format!("invalid secret key hex: {}", e)))?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| {
            WalletError::Keystore("secret key must be 32 bytes".to_string())
        })?;
        Ok(KeyPair::from_secret_bytes(&arr))
    }

    pub fn address(&self) -> &str {
        &self.address
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_roundtrip() {
        let (ks, kp) = KeyStore::generate();
        assert!(ks.address.starts_with("vinx1"));
        // Reconstruct keypair from keystore
        let kp2 = ks.to_keypair().unwrap();
        assert_eq!(kp.public_key(), kp2.public_key());
    }

    #[test]
    fn test_address_matches_keypair() {
        let (ks, kp) = KeyStore::generate();
        let derived = Address::from_public_key(&kp.public_key()).to_string();
        assert_eq!(ks.address, derived);
    }

    #[test]
    fn test_save_and_load() {
        let (ks, _) = KeyStore::generate();
        let path = std::env::temp_dir().join("test_wallet.json");
        ks.save(&path).unwrap();
        let loaded = KeyStore::load(&path).unwrap();
        assert_eq!(ks.address, loaded.address);
        assert_eq!(ks.secret_key_hex, loaded.secret_key_hex);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_load_missing_file() {
        let result = KeyStore::load(Path::new("/nonexistent/wallet.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_hex_rejected() {
        let ks = KeyStore {
            address: "vinx1abc".to_string(),
            secret_key_hex: "not_hex".to_string(),
        };
        assert!(ks.to_keypair().is_err());
    }
}
