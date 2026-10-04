//! Payment receipts (ADR 0083, L6): fetched from a node, verified locally, and kept by
//! the wallet — they stay verifiable after nodes prune the block.

use serde_json::Value;
use vinx_core::BlockHeader;
use vinx_crypto::{verify_tx_proof, Address, Hash32};

fn hash32(v: &Value, what: &str) -> Result<Hash32, String> {
    let s = v.as_str().ok_or(format!("{what}: missing"))?;
    hex::decode(s)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(format!("{what}: not a 32-byte hex hash"))
}

fn uint(v: &Value, what: &str) -> Result<u64, String> {
    v.as_u64().ok_or(format!("{what}: missing"))
}

/// Checks that `receipt` proves transaction `tx_hash`: its inclusion proof leads to the
/// header's `receipts_root`, and the header hashes to `block_hash`. Returns the height.
///
/// That the block is committed rests on the embedded quorum certificate (checked by the
/// node that served it) or on comparing `block_hash` with a node you trust.
pub fn verify(receipt: &Value, tx_hash: &str) -> Result<u64, String> {
    let tx = hash32(&Value::String(tx_hash.to_lowercase()), "tx hash")?;
    if hash32(&receipt["tx_hash"], "receipt tx_hash")? != tx {
        return Err("the receipt is for another transaction".into());
    }
    let h = &receipt["header"];
    let header = BlockHeader {
        height: uint(&h["height"], "height")?,
        round: uint(&h["round"], "round")? as u32,
        prev_hash: hash32(&h["prev_hash"], "prev_hash")?,
        timestamp: uint(&h["timestamp"], "timestamp")?,
        validator: h["validator"]
            .as_str()
            .and_then(|s| s.parse::<Address>().ok())
            .ok_or("validator: invalid address")?,
        tx_count: uint(&h["tx_count"], "tx_count")? as u32,
        state_root: hash32(&h["state_root"], "state_root")?,
        base_fee: uint(&h["base_fee"], "base_fee")?,
        receipts_root: hash32(&h["receipts_root"], "receipts_root")?,
        last_commit_hash: hash32(&h["last_commit_hash"], "last_commit_hash")?,
        version: h["version"].as_u64().unwrap_or(0) as u32,
    };
    let siblings = receipt["siblings"]
        .as_array()
        .ok_or("siblings: missing")?
        .iter()
        .map(|s| hash32(s, "sibling"))
        .collect::<Result<Vec<_>, _>>()?;
    let index = uint(&receipt["index"], "index")? as usize;
    if !verify_tx_proof(
        &tx,
        index,
        header.tx_count as usize,
        &siblings,
        &header.receipts_root,
    ) {
        return Err("inclusion proof does not match the block's transaction root".into());
    }
    if header.hash() != hash32(&receipt["block_hash"], "block_hash")? {
        return Err("the header does not hash to block_hash".into());
    }
    Ok(header.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Receipt produced by a real node (vinx-node integration test).
    const FIXTURE: &str =
        include_str!("../../../sdk/vinx-sdk/src/__fixtures__/payment_receipt.json");

    #[test]
    fn verifies_a_node_receipt_and_rejects_tampering() {
        let r: Value = serde_json::from_str(FIXTURE).unwrap();
        let tx = r["tx_hash"].as_str().unwrap().to_string();
        assert_eq!(verify(&r, &tx), Ok(1));

        assert!(verify(&r, &"ab".repeat(32)).is_err());
        let mut bad = r.clone();
        bad["header"]["timestamp"] = (r["header"]["timestamp"].as_u64().unwrap() + 1).into();
        assert!(verify(&bad, &tx).is_err());
        let mut bad = r.clone();
        bad["siblings"][0] = "00".repeat(32).into();
        assert!(verify(&bad, &tx).is_err());
    }
}
