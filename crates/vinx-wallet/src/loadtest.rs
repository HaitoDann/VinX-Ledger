//! Load test: submits hundreds of real, signed transfers and measures how the network
//! absorbs them (inclusion time, transactions per block, effective throughput).
//!
//! The mempool admits at most 50 pending transactions per sender, so the load is spread
//! over temporary sender accounts (40 transfers each), funded first by `bank`. Each
//! transfer pays 0.01 VINX to a fresh address — the state grows like real usage.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::Value;
use vinx_core::{Amount, Transaction};
use vinx_crypto::{Address, KeyPair};

use crate::client::RpcClient;
use crate::error::WalletError;

const PER_SENDER: usize = 40;
const BATCH: usize = 100;
const AMOUNT_ATOMS: u128 = 10_000_000; // 0.01 VINX

fn num(v: &Value, k: &str) -> u64 {
    v[k].as_u64()
        .or_else(|| v[k].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

async fn nonce_of(client: &RpcClient, addr: &str) -> u64 {
    client
        .get_json(&format!("/account/{addr}"))
        .await
        .map(|a| num(&a, "nonce"))
        .unwrap_or(0)
}

/// Submits `txs` in batches; returns the hashes the node accepted and the rejection
/// reasons it gave (deduplicated, with counts).
async fn submit_all(
    client: &RpcClient,
    txs: &[Transaction],
) -> Result<(Vec<String>, Vec<(String, usize)>), WalletError> {
    let mut accepted = vec![];
    let mut errors: Vec<(String, usize)> = vec![];
    for chunk in txs.chunks(BATCH) {
        let r = client.submit_batch(chunk).await?;
        for res in r["results"].as_array().into_iter().flatten() {
            if res["accepted"].as_bool() == Some(true) {
                accepted.push(res["tx_hash"].as_str().unwrap_or_default().to_string());
            } else {
                let e = res["error"].as_str().unwrap_or("?").to_string();
                match errors.iter_mut().find(|(m, _)| *m == e) {
                    Some((_, n)) => *n += 1,
                    None => errors.push((e, 1)),
                }
            }
        }
    }
    Ok((accepted, errors))
}

pub async fn run(
    client: &RpcClient,
    bank: &KeyPair,
    bank_addr: &str,
    count: usize,
) -> Result<(), WalletError> {
    let health = client.get_json("/health").await?;
    let chain_id = num(&health, "chain_id") as u32;
    let block_time = num(&health, "block_time_secs").max(1);
    let stats = client.get_json("/network/stats").await?;
    let fee = Amount::from_atoms(
        stats["base_fee_atoms"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(vinx_core::amount::DEFAULT_FEE_FLOOR_ATOMS),
    );
    let amount = Amount::from_atoms(AMOUNT_ATOMS);
    let senders: Vec<KeyPair> = (0..count.div_ceil(PER_SENDER))
        .map(|_| KeyPair::generate())
        .collect();
    let sign = |mut tx: Transaction, kp: &KeyPair| {
        tx.chain_id = chain_id;
        tx.sign(kp);
        tx
    };

    // ── 1. Fund the temporary senders, in waves the mempool accepts ────────────
    let per_sender_cost = (PER_SENDER as u128) * (AMOUNT_ATOMS + fee.atoms()) + 1_000_000_000;
    let need = per_sender_cost * senders.len() as u128;
    println!(
        "Préparation : {} expéditeurs temporaires, {} VINX prélevés sur {}",
        senders.len(),
        need / 1_000_000_000 + 1,
        bank_addr
    );
    let mut bank_nonce = nonce_of(client, bank_addr).await;
    for wave in senders.chunks(PER_SENDER) {
        let txs: Vec<Transaction> = wave
            .iter()
            .map(|kp| {
                let to = Address::from_public_key(&kp.public_key());
                let tx = Transaction::new_transfer(
                    bank,
                    to,
                    Amount::from_atoms(per_sender_cost),
                    fee,
                    bank_nonce,
                );
                bank_nonce += 1;
                sign(tx, bank)
            })
            .collect();
        let (ok, errs) = submit_all(client, &txs).await?;
        if ok.len() != txs.len() {
            return Err(WalletError::NodeError(format!(
                "financement refusé : {errs:?} (le portefeuille a-t-il assez de VINX ?)"
            )));
        }
        let deadline = Instant::now() + Duration::from_secs(block_time * 20);
        while nonce_of(client, bank_addr).await < bank_nonce {
            if Instant::now() > deadline {
                return Err(WalletError::NodeError(
                    "le financement n'est pas inclus — la chaîne avance-t-elle ?".into(),
                ));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    // ── 2. Sign every transfer, then submit them all at once ──────────────────
    let mut load = Vec::with_capacity(count);
    for (i, kp) in senders.iter().enumerate() {
        let n = PER_SENDER.min(count - i * PER_SENDER);
        for nonce in 0..n as u64 {
            let to = Address::from_public_key(&KeyPair::generate().public_key());
            load.push(sign(
                Transaction::new_transfer(kp, to, amount, fee, nonce),
                kp,
            ));
        }
    }
    let start_height = num(&client.get_json("/health").await?, "height");
    println!("Envoi de {} transferts signés…", load.len());
    let t0 = Instant::now();
    let (accepted, errors) = submit_all(client, &load).await?;
    let submit_secs = t0.elapsed().as_secs_f64();
    println!(
        "  {} acceptés par le nœud en {:.1} s{}",
        accepted.len(),
        submit_secs,
        if errors.is_empty() {
            String::new()
        } else {
            format!(", {} refusés : {errors:?}", load.len() - accepted.len())
        }
    );

    // ── 3. Follow the blocks until every accepted transfer is included ───────
    let mut pending: HashSet<String> = accepted.iter().cloned().collect();
    let total = pending.len();
    let mut next = start_height + 1;
    let (mut blocks, mut max_per_block, mut last_incl) = (0u64, 0usize, t0.elapsed());
    let deadline = Instant::now() + Duration::from_secs(block_time * 60 + 60);
    while !pending.is_empty() && Instant::now() < deadline {
        let tip = num(&client.get_json("/health").await?, "height");
        while next <= tip {
            let b = client.get_json(&format!("/block/{next}")).await?;
            let mut here = 0;
            for t in b["transactions"].as_array().into_iter().flatten() {
                if pending.remove(t["hash"].as_str().unwrap_or_default()) {
                    here += 1;
                }
            }
            if here > 0 {
                blocks += 1;
                max_per_block = max_per_block.max(here);
                last_incl = t0.elapsed();
                println!(
                    "  bloc #{next} : {here} transferts (reste {})",
                    pending.len()
                );
            }
            next += 1;
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }

    let included = total - pending.len();
    let secs = last_incl.as_secs_f64().max(0.001);
    println!();
    println!("Résultat");
    println!("  transferts inclus     : {included} / {total}");
    println!("  blocs utilisés        : {blocks}  (max {max_per_block} transferts dans un bloc)");
    println!("  durée envoi → inclusion du dernier : {secs:.1} s");
    println!(
        "  débit effectif        : {:.1} transferts/s  (temps de bloc {} s)",
        included as f64 / secs,
        block_time
    );
    if !pending.is_empty() {
        println!("  ⚠ {} transferts non inclus à temps", pending.len());
    }
    Ok(())
}
