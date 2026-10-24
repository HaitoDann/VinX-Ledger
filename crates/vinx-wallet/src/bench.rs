//! Test de charge : mesure le débit de transactions finalisées par le réseau.
//!
//! Des comptes jetables, financés par un compte existant, s'envoient en boucle
//! des montants minimes. Chaque compte garde au plus [`WINDOW`] transactions en
//! attente, sous la limite par compte du mempool. Le débit est mesuré sur les
//! blocs finalisés, pas sur les envois.

use crate::client::Client;
use anyhow::{Result, bail};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use vinx_core::{Amount, Transaction, TxBody};
use vinx_crypto::{Address, KeyPair};

/// Transactions en attente par compte (le mempool en accepte 100).
const WINDOW: u64 = 90;
/// Délai au-delà duquel un compte sans progrès renvoie depuis son nonce confirmé.
const STALL: Duration = Duration::from_secs(15);

pub struct Config {
    pub accounts: usize,
    /// Montant versé à chaque compte jetable.
    pub fund: Amount,
    pub duration: Duration,
    pub threads: usize,
}

#[derive(Debug, Default)]
pub struct Report {
    pub submitted: u64,
    pub rejected: u64,
    pub first_error: Option<String>,
    pub blocks: u64,
    pub finalized: u64,
    pub seconds: f64,
    pub peak_block: u64,
}

impl Report {
    pub fn tps(&self) -> f64 {
        if self.seconds > 0.0 {
            self.finalized as f64 / self.seconds
        } else {
            0.0
        }
    }
}

struct Bench {
    chain_id: String,
    fee: Amount,
}

impl Bench {
    fn transfer(
        &self,
        from: &KeyPair,
        nonce: u64,
        to: Address,
        amount: Amount,
        first: bool,
    ) -> vinx_core::SignedTransaction {
        Transaction {
            chain_id: self.chain_id.clone(),
            sender: Address::from_public_key(&from.public_key()),
            nonce,
            fee: self.fee,
            body: TxBody::Transfer {
                to,
                amount,
                memo: None,
            },
        }
        .sign(from, first)
    }
}

/// Nonce confirmé d'un compte (0 s'il n'existe pas encore).
fn confirmed_nonce(client: &Client, address: &Address) -> Result<u64> {
    Ok(client.account(address)?.map_or(0, |a| a.nonce))
}

/// Attend que le nonce confirmé de `address` atteigne `nonce`.
fn wait_nonce(client: &Client, address: &Address, nonce: u64, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while confirmed_nonce(client, address)? < nonce {
        if start.elapsed() > timeout {
            bail!("transactions non finalisées après {} s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

pub fn run(client: &Client, funder: &KeyPair, cfg: &Config, log: &dyn Fn(&str)) -> Result<Report> {
    if cfg.accounts < 2 {
        bail!("il faut au moins 2 comptes");
    }
    let status = client.status()?;
    let bench = Bench {
        chain_id: status.chain_id,
        fee: status.min_fee,
    };
    let funder_addr = Address::from_public_key(&funder.public_key());
    let Some(account) = client.account(&funder_addr)? else {
        bail!("le compte {funder_addr} n'existe pas");
    };
    let accounts_u64 = u64::try_from(cfg.accounts)?;
    let needed = cfg
        .fund
        .checked_add(bench.fee)
        .and_then(|per| per.checked_mul(accounts_u64))
        .ok_or_else(|| anyhow::anyhow!("montant de financement trop grand"))?;
    if account.balance < needed {
        bail!(
            "solde insuffisant : {} comptes × ({} + frais) demandent {}, le compte en a {}",
            cfg.accounts,
            crate::amount::format_vinx(cfg.fund),
            crate::amount::format_vinx(needed),
            crate::amount::format_vinx(account.balance)
        );
    }

    // Financement, par lots de WINDOW (limite par compte du mempool).
    let keys: Vec<KeyPair> = (0..cfg.accounts).map(|_| KeyPair::generate()).collect();
    let addrs: Vec<Address> = keys
        .iter()
        .map(|k| Address::from_public_key(&k.public_key()))
        .collect();
    log(&format!("financement de {} comptes…", cfg.accounts));
    let mut nonce = account.nonce;
    let mut first = !account.has_key;
    for chunk in addrs.chunks(usize::try_from(WINDOW)?) {
        for to in chunk {
            nonce = nonce.saturating_add(1);
            client.submit(&bench.transfer(funder, nonce, *to, cfg.fund, first))?;
            first = false;
        }
        wait_nonce(client, &funder_addr, nonce, Duration::from_secs(120))?;
    }

    let start_height = client.status()?.height;
    let submitted = AtomicU64::new(0);
    let rejected = AtomicU64::new(0);
    let first_error = std::sync::Mutex::new(None::<String>);
    let deadline = Instant::now()
        .checked_add(cfg.duration)
        .unwrap_or_else(Instant::now);
    log(&format!(
        "envoi pendant {} s depuis {} comptes, {} fils…",
        cfg.duration.as_secs(),
        cfg.accounts,
        cfg.threads
    ));

    let indices: Vec<usize> = (0..cfg.accounts).collect();
    let per_thread = cfg.accounts.div_ceil(cfg.threads.max(1));
    std::thread::scope(|scope| {
        for slice in indices.chunks(per_thread.max(1)) {
            let (client, bench, keys, addrs) = (client.clone(), &bench, &keys, &addrs);
            let (submitted, rejected, first_error) = (&submitted, &rejected, &first_error);
            scope.spawn(move || {
                // Par compte : prochain nonce à envoyer, nonce confirmé, dernier progrès.
                let mut next: Vec<u64> = vec![1; slice.len()];
                let mut confirmed: Vec<u64> = vec![0; slice.len()];
                let mut progress: Vec<Instant> = vec![Instant::now(); slice.len()];
                while Instant::now() < deadline {
                    let mut sent_any = false;
                    for (slot, &i) in slice.iter().enumerate() {
                        let (Some(key), Some(n), Some(c), Some(p)) = (
                            keys.get(i),
                            next.get_mut(slot),
                            confirmed.get_mut(slot),
                            progress.get_mut(slot),
                        ) else {
                            continue;
                        };
                        let addr = addrs.get(i).copied().unwrap_or(Address::treasury());
                        if let Ok(now) = confirmed_nonce(&client, &addr) {
                            if now > *c {
                                *c = now;
                                *p = Instant::now();
                            }
                        }
                        if p.elapsed() > STALL {
                            // Une transaction a été perdue : on reprend au nonce confirmé.
                            *n = c.saturating_add(1);
                            *p = Instant::now();
                        }
                        let to = addrs
                            .get(i.saturating_add(1).checked_rem(addrs.len()).unwrap_or(0))
                            .copied()
                            .unwrap_or(addr);
                        while *n <= c.saturating_add(WINDOW) && Instant::now() < deadline {
                            let tx = bench.transfer(key, *n, to, 1, *n == 1);
                            match client.submit(&tx) {
                                Ok(_) => {
                                    submitted.fetch_add(1, Ordering::Relaxed);
                                    sent_any = true;
                                    *n = n.saturating_add(1);
                                }
                                // Déjà en attente : renvoi après une reprise, rien de perdu.
                                Err(e) if e.to_string().contains("déjà en attente") => {
                                    *n = n.saturating_add(1);
                                }
                                Err(e) => {
                                    rejected.fetch_add(1, Ordering::Relaxed);
                                    if let Ok(mut f) = first_error.lock() {
                                        f.get_or_insert_with(|| e.to_string());
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    if !sent_any {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            });
        }
    });

    log("envoi terminé, attente de la vidange du mempool…");
    let drain = Instant::now();
    while drain.elapsed() < Duration::from_secs(60) {
        let pending = client
            .status_json()?
            .get("pending_transactions")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if pending == 0 {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let end_height = client.status()?.height;

    let mut report = Report {
        submitted: submitted.load(Ordering::Relaxed),
        rejected: rejected.load(Ordering::Relaxed),
        first_error: first_error.into_inner().ok().flatten(),
        ..Report::default()
    };
    let Some((start_ts, _)) = client.block_summary(start_height)? else {
        bail!("bloc {start_height} introuvable");
    };
    // Fenêtre mesurée : du bloc qui précède le premier bloc chargé au dernier
    // bloc chargé, pour ne compter ni le démarrage ni la vidange à vide.
    let (mut prev_ts, mut first_ts, mut last_ts) = (start_ts, None, start_ts);
    for h in start_height.saturating_add(1)..=end_height {
        let Some((ts, txs)) = client.block_summary(h)? else {
            continue;
        };
        if txs > 0 {
            first_ts.get_or_insert(prev_ts);
            last_ts = ts;
            report.blocks = report.blocks.saturating_add(1);
            report.finalized = report.finalized.saturating_add(txs);
            report.peak_block = report.peak_block.max(txs);
        }
        prev_ts = ts;
    }
    report.seconds = last_ts.saturating_sub(first_ts.unwrap_or(last_ts)) as f64 / 1000.0;
    Ok(report)
}
