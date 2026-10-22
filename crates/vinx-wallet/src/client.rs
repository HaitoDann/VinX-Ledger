//! Client de l'API REST du nœud.

use crate::amount::format_vinx;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use vinx_core::{Amount, SignedAdminTransaction, SignedTransaction, encode};
use vinx_crypto::Address;

pub struct Client {
    base: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub chain_id: String,
    pub height: u64,
    pub admin_nonce: u64,
    pub min_fee: Amount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInfo {
    pub balance: Amount,
    pub nonce: u64,
    pub has_key: bool,
    pub memo_required: bool,
}

fn field<'a>(v: &'a Value, name: &str) -> Result<&'a Value> {
    v.get(name)
        .ok_or_else(|| anyhow!("réponse du nœud sans champ « {name} »"))
}

fn as_u64(v: &Value, name: &str) -> Result<u64> {
    field(v, name)?
        .as_u64()
        .ok_or_else(|| anyhow!("champ « {name} » invalide"))
}

fn amount(v: &Value, name: &str) -> Result<Amount> {
    field(v, name)?
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("montant « {name} » invalide"))
}

impl Client {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
        }
    }

    /// Requête ; renvoie `None` sur 404, le message du nœud sur les autres erreurs.
    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Option<Value>> {
        let url = format!("{}{path}", self.base);
        let req = ureq::request(method, &url);
        let res = match body {
            Some(b) => req.send_json(b),
            None => req.call(),
        };
        match res {
            Ok(r) => Ok(Some(r.into_json().context("réponse JSON invalide")?)),
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(ureq::Error::Status(code, r)) => {
                let msg = r
                    .into_json::<Value>()
                    .ok()
                    .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_default();
                bail!("le nœud a répondu {code} : {msg}")
            }
            Err(e) => Err(anyhow!("nœud injoignable ({url}) : {e}")),
        }
    }

    pub fn status(&self) -> Result<Status> {
        let v = self
            .request("GET", "/status", None)?
            .ok_or_else(|| anyhow!("statut indisponible"))?;
        Ok(Status {
            chain_id: field(&v, "chain_id")?
                .as_str()
                .ok_or_else(|| anyhow!("chain_id invalide"))?
                .to_owned(),
            height: as_u64(&v, "height")?,
            admin_nonce: as_u64(&v, "admin_nonce")?,
            min_fee: amount(field(&v, "params")?, "min_fee")?,
        })
    }

    pub fn account(&self, address: &Address) -> Result<Option<AccountInfo>> {
        let Some(v) = self.request("GET", &format!("/accounts/{address}"), None)? else {
            return Ok(None);
        };
        Ok(Some(AccountInfo {
            balance: amount(&v, "balance")?,
            nonce: as_u64(&v, "nonce")?,
            has_key: !field(&v, "public_key")?.is_null(),
            memo_required: field(&v, "memo_required")?.as_bool().unwrap_or(false),
        }))
    }

    pub fn submit(&self, tx: &SignedTransaction) -> Result<String> {
        self.post_tx("/transactions", hex::encode(encode(tx)))
    }

    pub fn submit_admin(&self, tx: &SignedAdminTransaction) -> Result<String> {
        self.post_tx("/admin-transactions", hex::encode(encode(tx)))
    }

    fn post_tx(&self, path: &str, hex_tx: String) -> Result<String> {
        let v = self
            .request("POST", path, Some(json!({ "tx": hex_tx })))?
            .ok_or_else(|| anyhow!("route inconnue : {path}"))?;
        Ok(field(&v, "id")?.as_str().unwrap_or_default().to_owned())
    }

    pub fn describe_balance(&self, address: &Address) -> Result<String> {
        Ok(match self.account(address)? {
            Some(a) => format!("{} VINX", format_vinx(a.balance)),
            None => "compte inexistant (0 VINX)".to_owned(),
        })
    }
}
