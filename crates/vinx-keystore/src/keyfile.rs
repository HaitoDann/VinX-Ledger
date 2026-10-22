//! Clés chiffrées sur disque : Argon2id + XChaCha20-Poly1305.
//!
//! La clé secrète Ed25519 est chiffrée avec une clé dérivée du mot de passe.
//! Une copie volée du fichier ou d'une sauvegarde est inutilisable sans lui.

use anyhow::{Context, Result, anyhow, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::path::Path;
use vinx_crypto::KeyPair;

use crate::encoding::{key_from_hex, key_to_hex};

/// Coûts Argon2id par défaut : 64 Mio de mémoire, 3 passes.
pub const DEFAULT_KDF: Kdf = Kdf {
    m_cost_kib: 65_536,
    t_cost: 3,
    p_cost: 1,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kdf {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyFile {
    pub version: u8,
    /// Pour vérifier sans mot de passe à quelle clé correspond le fichier.
    pub public_key: String,
    pub kdf: Kdf,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

fn derive(password: &[u8], salt: &[u8], kdf: Kdf) -> Result<[u8; 32]> {
    let params = Params::new(kdf.m_cost_kib, kdf.t_cost, kdf.p_cost, Some(32))
        .map_err(|e| anyhow!("paramètres Argon2 invalides : {e}"))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt, &mut key)
        .map_err(|e| anyhow!("Argon2 : {e}"))?;
    Ok(key)
}

impl KeyFile {
    pub fn encrypt(keypair: &KeyPair, password: &str, kdf: Kdf) -> Result<Self> {
        if password.is_empty() {
            bail!("le mot de passe ne peut pas être vide");
        }
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 24];
        OsRng.fill_bytes(&mut salt);
        OsRng.fill_bytes(&mut nonce);
        let key = derive(password.as_bytes(), &salt, kdf)?;
        let cipher = XChaCha20Poly1305::new(&key.into());
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                keypair.secret_bytes().as_slice(),
            )
            .map_err(|_| anyhow!("échec du chiffrement"))?;
        Ok(Self {
            version: 1,
            public_key: key_to_hex(&keypair.public_key()),
            kdf,
            salt: hex::encode(salt),
            nonce: hex::encode(nonce),
            ciphertext: hex::encode(ciphertext),
        })
    }

    pub fn decrypt(&self, password: &str) -> Result<KeyPair> {
        if self.version != 1 {
            bail!("version de fichier de clé inconnue : {}", self.version);
        }
        let salt = hex::decode(&self.salt)?;
        let nonce: [u8; 24] = hex::decode(&self.nonce)?
            .try_into()
            .map_err(|_| anyhow!("nonce invalide"))?;
        let key = derive(password.as_bytes(), &salt, self.kdf)?;
        let plain = XChaCha20Poly1305::new(&key.into())
            .decrypt(
                XNonce::from_slice(&nonce),
                hex::decode(&self.ciphertext)?.as_slice(),
            )
            .map_err(|_| anyhow!("mot de passe incorrect ou fichier altéré"))?;
        let secret: [u8; 32] = plain
            .try_into()
            .map_err(|_| anyhow!("clé secrète de taille invalide"))?;
        let keypair = KeyPair::from_secret_bytes(&secret);
        if keypair.public_key() != key_from_hex(&self.public_key)? {
            bail!("la clé déchiffrée ne correspond pas à la clé publique du fichier");
        }
        Ok(keypair)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        write_private(path, text.as_bytes())
            .with_context(|| format!("écriture de {}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("lecture de {}", path.display()))?;
        serde_json::from_str(&text).context("fichier de clé invalide")
    }
}

/// Écrit un fichier lisible par son seul propriétaire (droits 600 sous Unix).
/// Refuse d'écraser un fichier existant.
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(data)
}

/// Mot de passe depuis `VINX_KEY_PASSWORD`, ou depuis le fichier indiqué.
pub fn password(file: Option<&Path>) -> Result<String> {
    if let Some(path) = file {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("lecture de {}", path.display()))?;
        return Ok(text.trim_end_matches(['\n', '\r']).to_owned());
    }
    std::env::var("VINX_KEY_PASSWORD").map_err(|_| {
        anyhow!("mot de passe requis : définir VINX_KEY_PASSWORD ou passer --password-file")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Coûts réduits pour garder les tests rapides.
    const FAST: Kdf = Kdf {
        m_cost_kib: 8,
        t_cost: 1,
        p_cost: 1,
    };

    #[test]
    fn roundtrip_and_wrong_password() {
        let kp = KeyPair::generate();
        let file = KeyFile::encrypt(&kp, "correct horse", FAST).unwrap();
        assert!(!file.ciphertext.contains(&hex::encode(kp.secret_bytes())));
        assert_eq!(
            file.decrypt("correct horse").unwrap().public_key(),
            kp.public_key()
        );
        assert!(file.decrypt("mauvais").is_err());
        assert!(KeyFile::encrypt(&kp, "", FAST).is_err());
    }

    #[test]
    fn tampering_is_detected() {
        let kp = KeyPair::generate();
        let mut file = KeyFile::encrypt(&kp, "pw", FAST).unwrap();
        let mut ct = hex::decode(&file.ciphertext).unwrap();
        if let Some(b) = ct.first_mut() {
            *b ^= 1;
        }
        file.ciphertext = hex::encode(ct);
        assert!(file.decrypt("pw").is_err());
    }

    #[test]
    fn saved_file_is_private_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.json");
        let file = KeyFile::encrypt(&KeyPair::generate(), "pw", FAST).unwrap();
        file.save(&path).unwrap();
        assert!(file.save(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(KeyFile::load(&path).unwrap().decrypt("pw").is_ok());
    }
}
