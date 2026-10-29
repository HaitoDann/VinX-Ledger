//! Comptes à règles (protocole 4) : propositions approuvées par les
//! signataires du compte, sur le modèle des propositions admin.
//!
//! Une proposition est un fichier JSON : la transaction, sans enveloppe, et
//! les approbations déjà réunies. Chaque signataire l'approuve avec sa clé,
//! puis le détenteur de la clé du compte l'envoie.

use crate::client::Client;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use vinx_core::{Approval, PolicyConfig, Transaction, TxBody, decode, encode};
use vinx_crypto::{Address, KeyPair, PublicKey};
use vinx_keystore::encoding::{key_from_hex, key_to_hex};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalFile {
    /// Transaction (corps non enveloppé), en borsh puis hexadécimal.
    tx: String,
    /// Approbations réunies, en borsh puis hexadécimal.
    approvals: String,
    /// Description lisible, pour relecture (non signée).
    description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub tx: Transaction,
    pub approvals: Vec<Approval>,
}

/// Politique du compte `address` selon le nœud.
pub fn policy(client: &Client, address: &Address) -> Result<Option<PolicyConfig>> {
    let Some(account) = client.get_json(&format!("/accounts/{address}"))? else {
        bail!("le compte {address} n'existe pas encore");
    };
    let Some(p) = account.get("policy").filter(|p| !p.is_null()) else {
        return Ok(None);
    };
    let signers = p
        .get("signers")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("politique illisible"))?
        .iter()
        .map(|s| {
            key_from_hex(
                s.get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("signataire illisible"))?,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let threshold = p
        .get("threshold")
        .and_then(Value::as_u64)
        .and_then(|t| u8::try_from(t).ok())
        .ok_or_else(|| anyhow!("seuil illisible"))?;
    let daily_limit = match p.get("daily_limit").and_then(Value::as_str) {
        Some(s) => Some(s.parse().map_err(|_| anyhow!("plafond illisible"))?),
        None => None,
    };
    Ok(Some(PolicyConfig {
        signers,
        threshold,
        daily_limit,
    }))
}

/// Prépare `body` pour le compte `from`, au prochain nonce, sans approbation.
pub fn propose(client: &Client, from: Address, body: TxBody) -> Result<Proposal> {
    let status = client.status()?;
    let account = client
        .account(&from)?
        .ok_or_else(|| anyhow!("le compte {from} n'existe pas encore"))?;
    let fee = body.min_fee(client.recommended_fee()?);
    Ok(Proposal {
        tx: Transaction {
            chain_id: status.chain_id,
            sender: from,
            nonce: account
                .nonce
                .checked_add(1)
                .ok_or_else(|| anyhow!("nonce épuisé"))?,
            fee,
            body,
        },
        approvals: Vec::new(),
    })
}

/// Ajoute l'approbation de `keypair`, dont l'indice est lu dans `signers`.
pub fn approve(p: &mut Proposal, signers: &[PublicKey], keypair: &KeyPair) -> Result<()> {
    let key = keypair.public_key();
    let index = signers.iter().position(|s| *s == key).ok_or_else(|| {
        anyhow!(
            "cette clé ({}) n'est pas signataire du compte",
            key_to_hex(&key)
        )
    })?;
    let index = u8::try_from(index)?;
    if p.approvals.iter().any(|a| a.signer_index == index) {
        bail!("ce signataire a déjà approuvé");
    }
    p.approvals.push(p.tx.approve(index, keypair));
    p.approvals.sort_by_key(|a| a.signer_index);
    Ok(())
}

/// Enveloppe la proposition avec ses approbations, signe avec la clé du compte, envoie.
pub fn submit(
    client: &Client,
    p: &Proposal,
    account_key: &KeyPair,
    has_key: bool,
) -> Result<String> {
    let mut tx = p.tx.clone();
    tx.body = TxBody::Approved {
        body: Box::new(p.tx.body.clone()),
        approvals: p.approvals.clone(),
    };
    client.submit(&tx.sign(account_key, !has_key))
}

pub fn save(p: &Proposal, path: &Path) -> Result<()> {
    let file = ProposalFile {
        tx: hex::encode(encode(&p.tx)),
        approvals: hex::encode(encode(&p.approvals)),
        description: format!(
            "{:?} (compte {}, nonce {}, chaîne {})",
            p.tx.body, p.tx.sender, p.tx.nonce, p.tx.chain_id
        ),
    };
    std::fs::write(path, serde_json::to_string_pretty(&file)?)
        .with_context(|| format!("écriture de {}", path.display()))
}

pub fn load(path: &Path) -> Result<Proposal> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("lecture de {}", path.display()))?;
    let file: ProposalFile =
        serde_json::from_str(&text).context("fichier de proposition invalide")?;
    let tx = decode(&hex::decode(&file.tx).context("hexadécimal invalide")?)
        .map_err(|_| anyhow!("transaction mal encodée"))?;
    let approvals = decode(&hex::decode(&file.approvals).context("hexadécimal invalide")?)
        .map_err(|_| anyhow!("approbations mal encodées"))?;
    Ok(Proposal { tx, approvals })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // code de test
mod tests {
    use super::*;

    #[test]
    fn approvals_and_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let keys: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
        let signers: Vec<PublicKey> = keys.iter().map(KeyPair::public_key).collect();
        let mut p = Proposal {
            tx: Transaction {
                chain_id: "c".into(),
                sender: Address::treasury(),
                nonce: 3,
                fee: 1,
                body: TxBody::Attest { digest: [1; 32] },
            },
            approvals: Vec::new(),
        };
        approve(&mut p, &signers, &keys[2]).unwrap();
        approve(&mut p, &signers, &keys[0]).unwrap();
        assert!(approve(&mut p, &signers, &keys[0]).is_err());
        assert!(approve(&mut p, &signers, &KeyPair::generate()).is_err());
        assert_eq!(
            p.approvals
                .iter()
                .map(|a| a.signer_index)
                .collect::<Vec<_>>(),
            [0, 2]
        );
        let path = dir.path().join("p.json");
        save(&p, &path).unwrap();
        assert_eq!(load(&path).unwrap(), p);
    }
}
