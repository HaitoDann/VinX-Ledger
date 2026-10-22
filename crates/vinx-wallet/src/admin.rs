//! Transactions admin multisig : proposition, approbations, envoi.
//!
//! Une proposition circule sous forme de fichier JSON entre les signataires.
//! Chacun y ajoute son approbation avec sa propre clé ; dès que deux
//! approbations sont réunies, n'importe qui peut l'envoyer au nœud.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;
use vinx_core::{
    ADMIN_SIGNERS, AdminAction, AdminTransaction, SignedAdminTransaction, decode, encode,
};
use vinx_crypto::KeyPair;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalFile {
    /// Transaction admin et approbations, encodées en borsh puis en hexadécimal.
    tx: String,
    /// Description lisible, pour relecture par les signataires (non signée).
    description: String,
}

pub fn propose(chain_id: String, nonce: u64, action: AdminAction) -> SignedAdminTransaction {
    SignedAdminTransaction {
        tx: AdminTransaction {
            chain_id,
            nonce,
            action,
        },
        approvals: Vec::new(),
    }
}

/// Ajoute l'approbation du signataire `index`, en gardant les indices triés.
pub fn approve(tx: &mut SignedAdminTransaction, index: u8, keypair: &KeyPair) -> Result<()> {
    if usize::from(index) >= ADMIN_SIGNERS {
        bail!(
            "indice de signataire invalide : {index} (0 à {})",
            ADMIN_SIGNERS.saturating_sub(1)
        );
    }
    if tx.approvals.iter().any(|a| a.signer_index == index) {
        bail!("le signataire {index} a déjà approuvé");
    }
    tx.approvals.push(tx.tx.approve(index, keypair));
    tx.approvals.sort_by_key(|a| a.signer_index);
    Ok(())
}

pub fn save(tx: &SignedAdminTransaction, path: &Path) -> Result<()> {
    let file = ProposalFile {
        tx: hex::encode(encode(tx)),
        description: format!(
            "{:?} (nonce {}, chaîne {})",
            tx.tx.action, tx.tx.nonce, tx.tx.chain_id
        ),
    };
    std::fs::write(path, serde_json::to_string_pretty(&file)?)
        .with_context(|| format!("écriture de {}", path.display()))
}

pub fn load(path: &Path) -> Result<SignedAdminTransaction> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("lecture de {}", path.display()))?;
    let file: ProposalFile =
        serde_json::from_str(&text).context("fichier de proposition invalide")?;
    let bytes = hex::decode(&file.tx).context("hexadécimal invalide")?;
    decode(&bytes).map_err(|_| anyhow!("proposition mal encodée"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvals_are_sorted_unique_and_bounded() {
        let keys: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
        let mut tx = propose("c".into(), 1, AdminAction::EscrowRelease { amount: 1 });
        approve(&mut tx, 2, &keys[2]).unwrap();
        approve(&mut tx, 0, &keys[0]).unwrap();
        assert!(approve(&mut tx, 0, &keys[0]).is_err());
        assert!(approve(&mut tx, 3, &keys[1]).is_err());
        let idx: Vec<u8> = tx.approvals.iter().map(|a| a.signer_index).collect();
        assert_eq!(idx, [0, 2]);
        let signers = [
            keys[0].public_key(),
            keys[1].public_key(),
            keys[2].public_key(),
        ];
        assert!(tx.verify_approvals(&signers).is_ok());
    }

    #[test]
    fn file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let mut tx = propose("c".into(), 4, AdminAction::EscrowReturn { amount: 9 });
        approve(&mut tx, 1, &KeyPair::generate()).unwrap();
        save(&tx, &path).unwrap();
        assert_eq!(load(&path).unwrap(), tx);
    }
}
