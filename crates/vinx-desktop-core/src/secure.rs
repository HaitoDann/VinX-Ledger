//! Recovery phrase and password-protected wallet file for the desktop app.
//!
//! Both are compatible with the `vinx-wallet` CLI: the key derived from a 12-word phrase
//! is the same (`BIP-39 seed[..32]`, empty passphrase), and the file is the CLI's v2
//! keystore (Argon2id + AES-256-GCM). A wallet made in one opens in the other.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::Path;
use vinx_crypto::{Address, KeyPair};

use crate::CoreError;

const M_COST: u32 = 19 * 1024;
const T_COST: u32 = 2;
const P_COST: u32 = 1;

fn err(m: impl std::fmt::Display) -> CoreError {
    CoreError::Keystore(m.to_string())
}

/// A new wallet: its 12 words and the key they give.
pub fn new_phrase() -> (String, KeyPair) {
    let mut entropy = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut entropy);
    let m = bip39::Mnemonic::from_entropy(&entropy).expect("16 bytes is a valid entropy");
    let kp = key_from_mnemonic(&m);
    (m.to_string(), kp)
}

/// The key of a recovery phrase (spaces and case are forgiven).
pub fn key_from_phrase(phrase: &str) -> Result<KeyPair, CoreError> {
    let words = phrase
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    let m: bip39::Mnemonic = words
        .parse()
        .map_err(|_| err("phrase de récupération invalide (12 mots attendus)"))?;
    Ok(key_from_mnemonic(&m))
}

fn key_from_mnemonic(m: &bip39::Mnemonic) -> KeyPair {
    let seed = m.to_seed("");
    let mut key = [0u8; 32];
    key.copy_from_slice(&seed[..32]);
    KeyPair::from_secret_bytes(&key)
}

#[derive(Serialize, Deserialize)]
struct Crypto {
    kdf: String,
    cipher: String,
    salt_hex: String,
    nonce_hex: String,
    ciphertext_hex: String,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
}

#[derive(Serialize, Deserialize)]
struct File {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<u32>,
    address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret_key_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    crypto: Option<Crypto>,
}

fn derive(pass: &str, salt: &[u8], m: u32, t: u32, p: u32) -> Result<[u8; 32], CoreError> {
    let params = argon2::Params::new(m, t, p, Some(32)).map_err(err)?;
    let a2 = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; 32];
    a2.hash_password_into(pass.as_bytes(), salt, &mut key)
        .map_err(err)?;
    Ok(key)
}

/// Writes `kp` encrypted under `password` (owner-only permissions on Unix).
pub fn save_wallet(path: &Path, kp: &KeyPair, password: &str) -> Result<(), CoreError> {
    if password.chars().count() < 8 {
        return Err(err("le mot de passe doit faire au moins 8 caractères"));
    }
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let key = derive(password, &salt, M_COST, T_COST, P_COST)?;
    let ct = Aes256Gcm::new_from_slice(&key)
        .map_err(err)?
        .encrypt(Nonce::from_slice(&nonce), kp.secret_bytes().as_slice())
        .map_err(err)?;
    let file = File {
        version: Some(2),
        address: Address::from_public_key(&kp.public_key()).to_string(),
        secret_key_hex: None,
        crypto: Some(Crypto {
            kdf: "argon2id".into(),
            cipher: "aes-256-gcm".into(),
            salt_hex: hex::encode(salt),
            nonce_hex: hex::encode(nonce),
            ciphertext_hex: hex::encode(ct),
            m_cost: M_COST,
            t_cost: T_COST,
            p_cost: P_COST,
        }),
    };
    let json = serde_json::to_string_pretty(&file).map_err(err)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, json).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(err)
}

/// The address stored in a wallet file, readable without the password.
pub fn wallet_address(path: &Path) -> Result<String, CoreError> {
    let f: File =
        serde_json::from_str(&std::fs::read_to_string(path).map_err(err)?).map_err(err)?;
    Ok(f.address)
}

/// Opens a wallet file with its password (plaintext CLI files open without one).
pub fn open_wallet(path: &Path, password: &str) -> Result<KeyPair, CoreError> {
    let f: File =
        serde_json::from_str(&std::fs::read_to_string(path).map_err(err)?).map_err(err)?;
    let secret: [u8; 32] = match (f.crypto, f.secret_key_hex) {
        (Some(c), _) => {
            if c.kdf != "argon2id" || c.cipher != "aes-256-gcm" {
                return Err(err("format de portefeuille non pris en charge"));
            }
            let h = |s: &str| hex::decode(s).map_err(err);
            let key = derive(password, &h(&c.salt_hex)?, c.m_cost, c.t_cost, c.p_cost)?;
            let pt = Aes256Gcm::new_from_slice(&key)
                .map_err(err)?
                .decrypt(
                    Nonce::from_slice(&h(&c.nonce_hex)?),
                    h(&c.ciphertext_hex)?.as_slice(),
                )
                .map_err(|_| err("mot de passe incorrect"))?;
            pt.try_into().map_err(|_| err("clé corrompue"))?
        }
        (None, Some(hexkey)) => hex::decode(hexkey)
            .map_err(err)?
            .try_into()
            .map_err(|_| err("clé corrompue"))?,
        (None, None) => return Err(err("fichier de portefeuille vide")),
    };
    let kp = KeyPair::from_secret_bytes(&secret);
    if Address::from_public_key(&kp.public_key()).to_string() != f.address {
        return Err(err("fichier de portefeuille incohérent"));
    }
    Ok(kp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_restores_the_same_key() {
        let (phrase, kp) = new_phrase();
        assert_eq!(phrase.split(' ').count(), 12);
        let again = key_from_phrase(&format!("  {}  ", phrase.to_uppercase())).unwrap();
        assert_eq!(again.secret_bytes(), kp.secret_bytes());
        assert!(key_from_phrase("pas une phrase").is_err());
    }

    #[test]
    fn wallet_file_roundtrip_and_wrong_password() {
        let dir = std::env::temp_dir().join(format!("vinx-dc-{}", std::process::id()));
        let path = dir.join("wallet.json");
        let (_, kp) = new_phrase();
        assert!(save_wallet(&path, &kp, "court").is_err());
        save_wallet(&path, &kp, "un bon mot de passe").unwrap();
        assert_eq!(
            wallet_address(&path).unwrap(),
            Address::from_public_key(&kp.public_key()).to_string()
        );
        let back = open_wallet(&path, "un bon mot de passe").unwrap();
        assert_eq!(back.secret_bytes(), kp.secret_bytes());
        assert!(open_wallet(&path, "mauvais mot de passe").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
