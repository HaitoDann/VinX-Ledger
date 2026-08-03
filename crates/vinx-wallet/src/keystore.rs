//! Wallet keystore: on-disk storage of the account's Ed25519 secret key.
//!
//! Two formats coexist:
//!
//! - **v2 (encrypted, default)** — the 32-byte secret is encrypted with
//!   AES-256-GCM under a key derived from a passphrase via Argon2id. The file
//!   stores the KDF parameters, salt, nonce and ciphertext (all hex), so old
//!   files remain readable if defaults change later.
//! - **legacy (plaintext)** — `secret_key_hex` in clear. Still readable for
//!   backward compatibility, with a warning; new saves only produce it when
//!   the user explicitly declines a passphrase.
//!
//! Files are written with `0600` permissions on Unix.

use crate::error::WalletError;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::Path;
use vinx_crypto::{Address, KeyPair};

/// Environment variable checked before prompting for a passphrase — lets
/// scripts and CI unlock an encrypted wallet non-interactively.
pub const PASSPHRASE_ENV: &str = "VINX_WALLET_PASSPHRASE";

const KEYSTORE_VERSION: u32 = 2;
/// Argon2id parameters (OWASP-recommended profile): 19 MiB, 2 iterations, 1 lane.
const ARGON2_M_COST: u32 = 19 * 1024;
const ARGON2_T_COST: u32 = 2;
const ARGON2_P_COST: u32 = 1;

/// Encrypted-secret envelope stored in a v2 keystore file.
#[derive(Serialize, Deserialize, Clone)]
pub struct CryptoParams {
    pub kdf: String,    // "argon2id"
    pub cipher: String, // "aes-256-gcm"
    pub salt_hex: String,
    pub nonce_hex: String,
    pub ciphertext_hex: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

/// On-disk representation. Either `secret_key_hex` (legacy plaintext) or
/// `crypto` (v2 encrypted) is present — never both.
#[derive(Serialize, Deserialize)]
struct KeyStoreFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<u32>,
    address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret_key_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crypto: Option<CryptoParams>,
}

enum Secret {
    Plain([u8; 32]),
    Encrypted(CryptoParams),
}

pub struct KeyStore {
    pub address: String,
    secret: Secret,
}

impl KeyStore {
    pub fn generate() -> (Self, KeyPair) {
        let kp = KeyPair::generate();
        (Self::from_keypair(&kp), kp)
    }

    pub fn from_keypair(kp: &KeyPair) -> Self {
        Self {
            address: Address::from_public_key(&kp.public_key()).to_string(),
            secret: Secret::Plain(kp.secret_bytes()),
        }
    }

    pub fn load(path: &Path) -> Result<Self, WalletError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| WalletError::Keystore(format!("cannot read {}: {}", path.display(), e)))?;
        let file: KeyStoreFile = serde_json::from_str(&content)
            .map_err(|e| WalletError::Keystore(format!("invalid keystore file: {}", e)))?;

        let secret = match (file.crypto, file.secret_key_hex) {
            (Some(crypto), _) => Secret::Encrypted(crypto),
            (None, Some(hex_key)) => {
                eprintln!(
                    "⚠ {} stores its secret key in PLAINTEXT. \
                     Re-create it with a passphrase to encrypt it.",
                    path.display()
                );
                Secret::Plain(decode_secret_hex(&hex_key)?)
            }
            (None, None) => {
                return Err(WalletError::Keystore(
                    "keystore has neither 'crypto' nor 'secret_key_hex'".to_string(),
                ))
            }
        };
        Ok(Self {
            address: file.address,
            secret,
        })
    }

    /// Saves the keystore. `passphrase = Some(..)` encrypts the secret
    /// (v2 format); `None` writes the legacy plaintext format.
    pub fn save(&self, path: &Path, passphrase: Option<&str>) -> Result<(), WalletError> {
        let secret_bytes = match &self.secret {
            Secret::Plain(b) => *b,
            Secret::Encrypted(_) => {
                return Err(WalletError::Keystore(
                    "cannot re-save an encrypted keystore without unlocking it first".to_string(),
                ))
            }
        };

        let file = match passphrase {
            Some(pass) if !pass.is_empty() => KeyStoreFile {
                version: Some(KEYSTORE_VERSION),
                address: self.address.clone(),
                secret_key_hex: None,
                crypto: Some(encrypt_secret(&secret_bytes, pass)?),
            },
            _ => KeyStoreFile {
                version: None,
                address: self.address.clone(),
                secret_key_hex: Some(hex::encode(secret_bytes)),
                crypto: None,
            },
        };

        let json = serde_json::to_string_pretty(&file)?;
        write_private(path, json.as_bytes())?;
        Ok(())
    }

    /// Reconstructs the keypair. Encrypted keystores are unlocked with the
    /// passphrase from `$VINX_WALLET_PASSPHRASE` if set, otherwise an
    /// interactive hidden prompt.
    pub fn to_keypair(&self) -> Result<KeyPair, WalletError> {
        match &self.secret {
            Secret::Plain(bytes) => Ok(KeyPair::from_secret_bytes(bytes)),
            Secret::Encrypted(crypto) => {
                let pass = match std::env::var(PASSPHRASE_ENV) {
                    Ok(p) => p,
                    Err(_) => rpassword::prompt_password("Wallet passphrase: ")
                        .map_err(|e| WalletError::Keystore(format!("passphrase read: {}", e)))?,
                };
                let bytes = decrypt_secret(crypto, &pass)?;
                Ok(KeyPair::from_secret_bytes(&bytes))
            }
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }
}

/// Prompts (twice) for a new wallet passphrase. Returns `None` when the user
/// leaves it empty (plaintext keystore, discouraged). `$VINX_WALLET_PASSPHRASE`
/// short-circuits the prompt for non-interactive use.
pub fn prompt_new_passphrase() -> Result<Option<String>, WalletError> {
    if let Ok(p) = std::env::var(PASSPHRASE_ENV) {
        return Ok(if p.is_empty() { None } else { Some(p) });
    }
    let first = rpassword::prompt_password("Passphrase (empty = UNENCRYPTED): ")
        .map_err(|e| WalletError::Keystore(format!("passphrase read: {}", e)))?;
    if first.is_empty() {
        eprintln!("⚠ No passphrase — the secret key will be stored in PLAINTEXT.");
        return Ok(None);
    }
    let second = rpassword::prompt_password("Confirm passphrase: ")
        .map_err(|e| WalletError::Keystore(format!("passphrase read: {}", e)))?;
    if first != second {
        return Err(WalletError::Keystore(
            "passphrases do not match".to_string(),
        ));
    }
    Ok(Some(first))
}

// ─── Crypto helpers ──────────────────────────────────────────────────────────

fn derive_key(
    pass: &str,
    salt: &[u8],
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
) -> Result<[u8; 32], WalletError> {
    let params = argon2::Params::new(m_cost, t_cost, p_cost, Some(32))
        .map_err(|e| WalletError::Keystore(format!("argon2 params: {}", e)))?;
    let a2 = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; 32];
    a2.hash_password_into(pass.as_bytes(), salt, &mut key)
        .map_err(|e| WalletError::Keystore(format!("key derivation: {}", e)))?;
    Ok(key)
}

fn encrypt_secret(secret: &[u8; 32], pass: &str) -> Result<CryptoParams, WalletError> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut nonce);

    let key = derive_key(pass, &salt, ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| WalletError::Keystore(format!("cipher init: {}", e)))?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), secret.as_slice())
        .map_err(|e| WalletError::Keystore(format!("encryption failed: {}", e)))?;

    Ok(CryptoParams {
        kdf: "argon2id".to_string(),
        cipher: "aes-256-gcm".to_string(),
        salt_hex: hex::encode(salt),
        nonce_hex: hex::encode(nonce),
        ciphertext_hex: hex::encode(ciphertext),
        m_cost: ARGON2_M_COST,
        t_cost: ARGON2_T_COST,
        p_cost: ARGON2_P_COST,
    })
}

fn decrypt_secret(crypto: &CryptoParams, pass: &str) -> Result<[u8; 32], WalletError> {
    if crypto.kdf != "argon2id" || crypto.cipher != "aes-256-gcm" {
        return Err(WalletError::Keystore(format!(
            "unsupported keystore crypto: kdf={} cipher={}",
            crypto.kdf, crypto.cipher
        )));
    }
    let salt = hex::decode(&crypto.salt_hex)
        .map_err(|e| WalletError::Keystore(format!("invalid salt hex: {}", e)))?;
    let nonce = hex::decode(&crypto.nonce_hex)
        .map_err(|e| WalletError::Keystore(format!("invalid nonce hex: {}", e)))?;
    let ciphertext = hex::decode(&crypto.ciphertext_hex)
        .map_err(|e| WalletError::Keystore(format!("invalid ciphertext hex: {}", e)))?;

    let key = derive_key(pass, &salt, crypto.m_cost, crypto.t_cost, crypto.p_cost)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| WalletError::Keystore(format!("cipher init: {}", e)))?;
    let plain = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_slice())
        .map_err(|_| {
            WalletError::Keystore("wrong passphrase (or corrupted keystore)".to_string())
        })?;

    plain
        .try_into()
        .map_err(|_| WalletError::Keystore("decrypted secret is not 32 bytes".to_string()))
}

fn decode_secret_hex(hex_key: &str) -> Result<[u8; 32], WalletError> {
    let bytes = hex::decode(hex_key)
        .map_err(|e| WalletError::Keystore(format!("invalid secret key hex: {}", e)))?;
    bytes
        .try_into()
        .map_err(|_| WalletError::Keystore("secret key must be 32 bytes".to_string()))
}

/// Writes `data` to `path`, restricting permissions to the owner (0600) on Unix.
fn write_private(path: &Path, data: &[u8]) -> Result<(), WalletError> {
    std::fs::write(path, data)
        .map_err(|e| WalletError::Keystore(format!("cannot write {}: {}", path.display(), e)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(name)
    }

    /// Tests that touch `PASSPHRASE_ENV` must not run concurrently.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_generate_and_roundtrip() {
        let (ks, kp) = KeyStore::generate();
        assert!(ks.address.starts_with("vinx1"));
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
    fn test_save_and_load_plaintext() {
        let (ks, kp) = KeyStore::generate();
        let path = tmp("test_wallet_plain.json");
        ks.save(&path, None).unwrap();
        let loaded = KeyStore::load(&path).unwrap();
        assert_eq!(ks.address, loaded.address);
        assert_eq!(kp.public_key(), loaded.to_keypair().unwrap().public_key());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_save_and_load_encrypted() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (ks, kp) = KeyStore::generate();
        let path = tmp("test_wallet_enc.json");
        ks.save(&path, Some("correct horse battery staple"))
            .unwrap();

        // The file must not contain the secret in clear.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains(&hex::encode(kp.secret_bytes())));
        assert!(raw.contains("argon2id"));

        // Unlock via the env var (no interactive prompt in tests).
        std::env::set_var(PASSPHRASE_ENV, "correct horse battery staple");
        let loaded = KeyStore::load(&path).unwrap();
        let kp2 = loaded.to_keypair().unwrap();
        std::env::remove_var(PASSPHRASE_ENV);
        assert_eq!(kp.public_key(), kp2.public_key());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_wrong_passphrase_rejected() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (ks, _) = KeyStore::generate();
        let path = tmp("test_wallet_wrongpass.json");
        ks.save(&path, Some("right")).unwrap();

        std::env::set_var(PASSPHRASE_ENV, "wrong");
        let loaded = KeyStore::load(&path).unwrap();
        let result = loaded.to_keypair();
        std::env::remove_var(PASSPHRASE_ENV);
        assert!(result.is_err());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_legacy_plaintext_format_still_loads() {
        let (ks, kp) = KeyStore::generate();
        let path = tmp("test_wallet_legacy.json");
        // Hand-write the legacy format (no version, no crypto).
        let legacy = serde_json::json!({
            "address": ks.address,
            "secret_key_hex": hex::encode(kp.secret_bytes()),
        });
        std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();
        let loaded = KeyStore::load(&path).unwrap();
        assert_eq!(loaded.address, ks.address);
        assert_eq!(kp.public_key(), loaded.to_keypair().unwrap().public_key());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_load_missing_file() {
        let result = KeyStore::load(Path::new("/nonexistent/wallet.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_hex_rejected() {
        let path = tmp("test_wallet_badhex.json");
        std::fs::write(
            &path,
            r#"{"address":"vinx1abc","secret_key_hex":"not_hex"}"#,
        )
        .unwrap();
        assert!(KeyStore::load(&path).is_err());
        std::fs::remove_file(path).ok();
    }

    #[cfg(unix)]
    #[test]
    fn test_file_permissions_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (ks, _) = KeyStore::generate();
        let path = tmp("test_wallet_perms.json");
        ks.save(&path, None).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_file(path).ok();
    }
}
