//! Fichier de genèse en JSON (SPEC §12).

use crate::encoding::{amount_from_str, amount_to_string, key_from_hex, key_to_hex};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::Path;
use vinx_core::Params;
use vinx_state::{Escrow, Genesis, Tranche};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenesisFile {
    pub chain_id: String,
    /// Horodatage Unix, en secondes.
    pub genesis_time: u64,
    pub protocol_version: u32,
    /// Clés publiques des trois signataires admin.
    pub admin: Vec<String>,
    pub validators: Vec<String>,
    /// En unités (1 VINX = 10⁹ unités).
    pub treasury_balance: String,
    pub escrow: EscrowFile,
    pub params: ParamsFile,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EscrowFile {
    pub period: u64,
    pub tranches: Vec<TrancheFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrancheFile {
    pub amount: String,
    pub matures_at: u64,
}

/// Paramètres de SPEC §11. Montants en unités, durées en millisecondes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamsFile {
    pub min_fee: String,
    pub min_reserve: String,
    pub max_memo_bytes: u64,
    pub max_block_txs: u64,
    pub max_block_bytes: u64,
    pub timeout_propose_ms: u64,
    pub timeout_propose_delta_ms: u64,
    pub timeout_vote_ms: u64,
    pub timeout_vote_delta_ms: u64,
    pub min_block_interval_ms: u64,
}

impl GenesisFile {
    pub fn from_genesis(g: &Genesis) -> Self {
        let p = &g.params;
        Self {
            chain_id: g.chain_id.clone(),
            genesis_time: g.genesis_time,
            protocol_version: g.protocol_version,
            admin: g.admin.iter().map(key_to_hex).collect(),
            validators: g.validators.iter().map(key_to_hex).collect(),
            treasury_balance: amount_to_string(g.treasury_balance),
            escrow: EscrowFile {
                period: g.escrow.period,
                tranches: g
                    .escrow
                    .tranches
                    .iter()
                    .map(|t| TrancheFile {
                        amount: amount_to_string(t.amount),
                        matures_at: t.matures_at,
                    })
                    .collect(),
            },
            params: ParamsFile {
                min_fee: amount_to_string(p.min_fee),
                min_reserve: amount_to_string(p.min_reserve),
                max_memo_bytes: p.max_memo_bytes,
                max_block_txs: p.max_block_txs,
                max_block_bytes: p.max_block_bytes,
                timeout_propose_ms: p.timeout_propose_ms,
                timeout_propose_delta_ms: p.timeout_propose_delta_ms,
                timeout_vote_ms: p.timeout_vote_ms,
                timeout_vote_delta_ms: p.timeout_vote_delta_ms,
                min_block_interval_ms: p.min_block_interval_ms,
            },
        }
    }

    pub fn to_genesis(&self) -> Result<Genesis> {
        let admin: Vec<_> = self
            .admin
            .iter()
            .map(|k| key_from_hex(k))
            .collect::<Result<_>>()?;
        let admin: [_; 3] = admin
            .try_into()
            .map_err(|_| anyhow!("il faut exactement 3 signataires admin"))?;
        let tranches = self
            .escrow
            .tranches
            .iter()
            .map(|t| {
                Ok(Tranche {
                    amount: amount_from_str(&t.amount)?,
                    matures_at: t.matures_at,
                })
            })
            .collect::<Result<VecDeque<_>>>()?;
        let p = &self.params;
        Ok(Genesis {
            chain_id: self.chain_id.clone(),
            genesis_time: self.genesis_time,
            protocol_version: self.protocol_version,
            admin,
            validators: self
                .validators
                .iter()
                .map(|k| key_from_hex(k))
                .collect::<Result<_>>()?,
            treasury_balance: amount_from_str(&self.treasury_balance)?,
            escrow: Escrow {
                tranches,
                period: self.escrow.period,
            },
            params: Params {
                min_fee: amount_from_str(&p.min_fee)?,
                min_reserve: amount_from_str(&p.min_reserve)?,
                max_memo_bytes: p.max_memo_bytes,
                max_block_txs: p.max_block_txs,
                max_block_bytes: p.max_block_bytes,
                timeout_propose_ms: p.timeout_propose_ms,
                timeout_propose_delta_ms: p.timeout_propose_delta_ms,
                timeout_vote_ms: p.timeout_vote_ms,
                timeout_vote_delta_ms: p.timeout_vote_delta_ms,
                min_block_interval_ms: p.min_block_interval_ms,
            },
        })
    }

    pub fn load(path: &Path) -> Result<Genesis> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("lecture de {}", path.display()))?;
        let file: GenesisFile = serde_json::from_str(&text).context("genèse : JSON invalide")?;
        file.to_genesis()
    }

    pub fn save(g: &Genesis, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(&Self::from_genesis(g))?;
        std::fs::write(path, text).with_context(|| format!("écriture de {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    #[test]
    fn json_roundtrip_preserves_the_genesis() {
        let k = || KeyPair::generate().public_key();
        let g =
            Genesis::standard("vinx-devnet-1", 1_800_000_000, [k(), k(), k()], vec![k()]).unwrap();
        let json = serde_json::to_string(&GenesisFile::from_genesis(&g)).unwrap();
        let back: GenesisFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.to_genesis().unwrap(), g);
        assert!(back.to_genesis().unwrap().into_state().is_ok());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let k = || KeyPair::generate().public_key();
        let g = Genesis::standard("vinx-devnet-1", 1, [k(), k(), k()], vec![k()]).unwrap();
        let mut v = serde_json::to_value(GenesisFile::from_genesis(&g)).unwrap();
        v["mint"] = serde_json::json!("1000");
        assert!(serde_json::from_value::<GenesisFile>(v).is_err());
    }
}
