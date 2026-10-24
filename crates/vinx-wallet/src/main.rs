//! Wallet VinX Ledger en ligne de commande.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use vinx_core::{AdminAction, Amount, ParamId};
use vinx_crypto::{Address, KeyPair};
use vinx_keystore::encoding::{key_from_hex, key_to_hex};
use vinx_keystore::keyfile::{DEFAULT_KDF, KeyFile, password};
use vinx_wallet::amount::{format_vinx, parse_vinx};
use vinx_wallet::client::Client;
use vinx_wallet::{admin, bench, ops, preflight};

#[derive(Parser)]
#[command(name = "vinx-wallet", version, about = "Wallet VinX Ledger")]
struct Cli {
    /// Adresse de l'API du nœud.
    #[arg(long, global = true, default_value = "http://127.0.0.1:8545")]
    node: String,
    /// Fichier contenant le mot de passe des clés (sinon : variable VINX_KEY_PASSWORD).
    #[arg(long, global = true)]
    password_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Crée une clé chiffrée.
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Affiche l'adresse et la clé publique d'un fichier de clé (sans mot de passe).
    Address {
        #[arg(long)]
        key: PathBuf,
    },
    /// État du réseau.
    Status,
    /// Solde d'une adresse.
    Balance { address: Address },
    /// Envoie des VINX.
    Send {
        #[arg(long)]
        key: PathBuf,
        /// Compte émetteur, si la clé a été changée par rotation.
        #[arg(long)]
        from: Option<Address>,
        #[arg(long)]
        to: Address,
        /// Montant en VINX, jusqu'à 9 décimales (ex. 12.5).
        #[arg(long)]
        amount: String,
        #[arg(long)]
        memo: Option<String>,
        /// Frais en VINX (par défaut : les frais conseillés par le nœud).
        #[arg(long)]
        fee: Option<String>,
    },
    /// Exige (on) ou non (off) un mémo sur les paiements reçus.
    MemoRequired {
        #[arg(long)]
        key: PathBuf,
        /// Compte émetteur, si la clé a été changée par rotation.
        #[arg(long)]
        from: Option<Address>,
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Remplace la clé du compte par celle d'un autre fichier ; l'adresse ne change pas.
    RotateKey {
        #[arg(long)]
        key: PathBuf,
        /// Compte émetteur, si la clé a été changée par rotation.
        #[arg(long)]
        from: Option<Address>,
        #[arg(long)]
        new_key: PathBuf,
    },
    /// Transactions admin multisig.
    #[command(subcommand)]
    Admin(AdminCommand),
    /// Vérifie que de futurs validateurs sont prêts, avant de les ajouter.
    ///
    /// `--node` désigne un nœud du réseau actuel (la référence) ; chaque
    /// `--new-node` est l'API d'un futur validateur, démarré avec sa clé.
    Preflight {
        #[arg(long = "new-node", required = true, num_args = 1)]
        new_nodes: Vec<String>,
    },
    /// Test de charge : mesure le débit de transactions finalisées.
    ///
    /// Crée des comptes jetables financés par `--key`, leur fait s'envoyer des
    /// montants minimes pendant `--duration` secondes, puis compte les
    /// transactions finalisées. À lancer sur un réseau de test, de préférence
    /// sur la machine du nœud (pas à travers un tunnel SSH).
    Bench {
        /// Compte qui finance les comptes jetables.
        #[arg(long)]
        key: PathBuf,
        /// Nombre de comptes jetables (chacun garde au plus 90 transactions en attente).
        #[arg(long, default_value_t = 200)]
        accounts: usize,
        /// VINX versés à chaque compte jetable.
        #[arg(long, default_value = "1")]
        fund: String,
        /// Durée de l'envoi, en secondes.
        #[arg(long, default_value_t = 60)]
        duration: u64,
        /// Fils d'envoi en parallèle.
        #[arg(long, default_value_t = 8)]
        threads: usize,
    },
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Affiche la gouvernance : multisig, validateurs, paramètres, escrow, trésorerie.
    Status,
    /// Prépare une proposition (sans approbation) dans un fichier.
    Propose {
        #[arg(long)]
        out: PathBuf,
        /// Nonce admin (par défaut : le prochain, lu sur le nœud).
        #[arg(long)]
        nonce: Option<u64>,
        #[command(subcommand)]
        action: ActionArg,
    },
    /// Ajoute l'approbation d'un signataire à une proposition.
    Approve {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        key: PathBuf,
        /// Indice du signataire dans la genèse (0, 1 ou 2).
        #[arg(long)]
        index: u8,
    },
    /// Affiche une proposition et ses approbations.
    Show {
        #[arg(long)]
        file: PathBuf,
    },
    /// Envoie une proposition approuvée au nœud.
    Submit {
        #[arg(long)]
        file: PathBuf,
    },
}

#[derive(Subcommand)]
enum ActionArg {
    /// Libère un montant de la première tranche échue de l'escrow.
    EscrowRelease(AmountArg),
    /// Renvoie des VINX de la trésorerie en fin de calendrier d'escrow.
    EscrowReturn(AmountArg),
    /// Paiement depuis la trésorerie.
    TreasuryTransfer {
        #[arg(long)]
        to: Address,
        #[arg(long)]
        amount: String,
        #[arg(long)]
        memo: Option<String>,
    },
    /// Ajoute un validateur (clé publique en hexadécimal).
    AddValidator {
        #[arg(long)]
        key: String,
    },
    /// Retire un validateur.
    RemoveValidator {
        #[arg(long)]
        key: String,
    },
    /// Modifie un paramètre (valeur en unités ou en millisecondes, voir SPEC §11).
    SetParam {
        #[arg(long)]
        param: String,
        #[arg(long)]
        value: u64,
    },
    /// Remplace un signataire admin.
    ReplaceSigner {
        #[arg(long)]
        index: u8,
        #[arg(long)]
        new_key: String,
    },
    /// Programme une mise à jour du protocole.
    ScheduleUpgrade {
        #[arg(long)]
        version: u32,
        #[arg(long)]
        height: u64,
    },
}

#[derive(Args)]
struct AmountArg {
    /// Montant en VINX.
    #[arg(long)]
    amount: String,
}

fn param_id(name: &str) -> Result<ParamId> {
    Ok(match name {
        "min_fee" => ParamId::MinFee,
        "min_reserve" => ParamId::MinReserve,
        "max_memo_bytes" => ParamId::MaxMemoBytes,
        "max_block_txs" => ParamId::MaxBlockTxs,
        "max_block_bytes" => ParamId::MaxBlockBytes,
        "timeout_propose" => ParamId::TimeoutPropose,
        "timeout_propose_delta" => ParamId::TimeoutProposeDelta,
        "timeout_vote" => ParamId::TimeoutVote,
        "timeout_vote_delta" => ParamId::TimeoutVoteDelta,
        "min_block_interval" => ParamId::MinBlockInterval,
        other => bail!("paramètre inconnu : {other}"),
    })
}

fn action(arg: ActionArg) -> Result<AdminAction> {
    let memo = |m: Option<String>| m.map(String::into_bytes);
    Ok(match arg {
        ActionArg::EscrowRelease(a) => AdminAction::EscrowRelease {
            amount: parse_vinx(&a.amount)?,
        },
        ActionArg::EscrowReturn(a) => AdminAction::EscrowReturn {
            amount: parse_vinx(&a.amount)?,
        },
        ActionArg::TreasuryTransfer {
            to,
            amount,
            memo: m,
        } => AdminAction::TreasuryTransfer {
            to,
            amount: parse_vinx(&amount)?,
            memo: memo(m),
        },
        ActionArg::AddValidator { key } => AdminAction::AddValidator {
            key: key_from_hex(&key)?,
        },
        ActionArg::RemoveValidator { key } => AdminAction::RemoveValidator {
            key: key_from_hex(&key)?,
        },
        ActionArg::SetParam { param, value } => AdminAction::SetParam {
            param: param_id(&param)?,
            value,
        },
        ActionArg::ReplaceSigner { index, new_key } => AdminAction::ReplaceSigner {
            index,
            new_key: key_from_hex(&new_key)?,
        },
        ActionArg::ScheduleUpgrade { version, height } => AdminAction::ScheduleUpgrade {
            version,
            activation_height: height,
        },
    })
}

/// Champ JSON par chemin (`/a/b`), `null` s'il manque.
fn at<'a>(v: &'a serde_json::Value, path: &str) -> &'a serde_json::Value {
    v.pointer(path).unwrap_or(&serde_json::Value::Null)
}

fn units(v: &serde_json::Value) -> String {
    v.as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .map_or_else(|| v.to_string(), |u| format!("{} VINX", format_vinx(u)))
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|k| k.as_str().unwrap_or_default().to_owned())
        .collect()
}

fn print_governance(g: &serde_json::Value) {
    println!("protocole          version {}", at(g, "/protocol_version"));
    if !at(g, "/upgrade").is_null() {
        println!(
            "mise à jour        version {} à la hauteur {}",
            at(g, "/upgrade/version"),
            at(g, "/upgrade/activation_height")
        );
    }
    let signers = strings(at(g, "/admin/signers"));
    println!(
        "multisig           {} sur {}, nonce {}",
        at(g, "/admin/threshold"),
        signers.len(),
        at(g, "/admin/nonce")
    );
    for (i, k) in signers.iter().enumerate() {
        println!("  signataire {i}     {k}");
    }
    for (i, k) in strings(at(g, "/validators/active")).iter().enumerate() {
        println!("  validateur {}     {k}", i.saturating_add(1));
    }
    for p in at(g, "/validators/pending")
        .as_array()
        .into_iter()
        .flatten()
    {
        println!(
            "  en attente       {} de {} à la hauteur {}",
            at(p, "/change").as_str().unwrap_or_default(),
            at(p, "/key").as_str().unwrap_or_default(),
            at(p, "/effective_height")
        );
    }
    println!("trésorerie         {}", units(at(g, "/treasury/balance")));
    println!(
        "escrow             {}, {} tranche(s)",
        units(at(g, "/escrow/total")),
        at(g, "/escrow/tranches")
    );
    if !at(g, "/escrow/next").is_null() {
        let due = at(g, "/escrow/next/matures_at").as_u64().unwrap_or(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let when = match due.checked_sub(now) {
            None | Some(0) => "libérable maintenant".to_owned(),
            Some(s) if s < 3_600 => format!("échéance dans {} min", s.div_ceil(60)),
            Some(s) if s < 172_800 => format!("échéance dans {} h", s.div_ceil(3_600)),
            Some(s) => format!("échéance dans {} jours", s.div_ceil(86_400)),
        };
        println!(
            "  prochaine        {}, {when}",
            units(at(g, "/escrow/next/amount"))
        );
    }
    println!(
        "paramètres         frais min. {}, réserve {}, bloc {} ms, délais {} / {} ms",
        units(at(g, "/params/min_fee")),
        units(at(g, "/params/min_reserve")),
        at(g, "/params/min_block_interval_ms"),
        at(g, "/params/timeout_propose_ms"),
        at(g, "/params/timeout_vote_ms")
    );
}

fn load_key(path: &Path, password_file: Option<&Path>) -> Result<KeyPair> {
    KeyFile::load(path)?.decrypt(&password(password_file)?)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::new(&cli.node);
    let pwf = cli.password_file.as_deref();

    match cli.command {
        Command::Keygen { out } => {
            let keypair = KeyPair::generate();
            KeyFile::encrypt(&keypair, &password(pwf)?, DEFAULT_KDF)?.save(&out)?;
            println!(
                "adresse     {}",
                Address::from_public_key(&keypair.public_key())
            );
            println!("clé         {}", key_to_hex(&keypair.public_key()));
        }
        Command::Address { key } => {
            let pk = key_from_hex(&KeyFile::load(&key)?.public_key)?;
            println!("adresse     {}", Address::from_public_key(&pk));
            println!("clé         {}", key_to_hex(&pk));
        }
        Command::Status => {
            let s = client.status()?;
            println!("chaîne      {}", s.chain_id);
            println!("hauteur     {}", s.height);
            println!("frais min.  {} VINX", format_vinx(s.min_fee));
            println!(
                "conseillés  {} VINX",
                format_vinx(client.recommended_fee()?)
            );
            println!("nonce admin {}", s.admin_nonce);
        }
        Command::Balance { address } => println!("{}", client.describe_balance(&address)?),
        Command::Send {
            key,
            from,
            to,
            amount,
            memo,
            fee,
        } => {
            let keypair = load_key(&key, pwf)?;
            let fee: Option<Amount> = fee.as_deref().map(parse_vinx).transpose()?;
            let id = ops::transfer(&client, &keypair, from, to, parse_vinx(&amount)?, memo, fee)?;
            println!("transaction {id}");
        }
        Command::MemoRequired { key, from, state } => {
            let id = ops::set_memo_required(&client, &load_key(&key, pwf)?, from, state == "on")?;
            println!("transaction {id}");
        }
        Command::RotateKey { key, from, new_key } => {
            let keypair = load_key(&key, pwf)?;
            let new = key_from_hex(&KeyFile::load(&new_key)?.public_key)?;
            println!(
                "transaction {}",
                ops::rotate_key(&client, &keypair, from, new)?
            );
        }
        Command::Bench {
            key,
            accounts,
            fund,
            duration,
            threads,
        } => {
            let keypair = load_key(&key, pwf)?;
            let cfg = bench::Config {
                accounts,
                fund: parse_vinx(&fund)?,
                duration: std::time::Duration::from_secs(duration),
                threads,
            };
            let r = bench::run(&client, &keypair, &cfg, &|m| println!("{m}"))?;
            println!();
            println!("envoyées         {}", r.submitted);
            println!("refusées         {}", r.rejected);
            if let Some(e) = &r.first_error {
                println!("  premier refus  {e}");
            }
            println!(
                "finalisées       {} en {} bloc(s), {:.1} s",
                r.finalized, r.blocks, r.seconds
            );
            println!("débit moyen      {:.0} transactions/s", r.tps());
            println!("bloc le plus plein  {} transactions", r.peak_block);
        }
        Command::Preflight { new_nodes } => {
            let nodes: Vec<Client> = new_nodes.iter().map(Client::new).collect();
            let (reference, report) = preflight::run(&client, &nodes)?;
            println!(
                "référence     {} — chaîne {}, hauteur {}, {} validateur(s)",
                reference.url,
                reference.chain_id,
                reference.height,
                reference.validators.len()
            );
            for e in &report.errors {
                println!("  ✗ {e}");
            }
            for w in &report.warnings {
                println!("  ! {w}");
            }
            if !report.errors.is_empty() {
                bail!(
                    "{} problème(s) à corriger avant l'ajout",
                    report.errors.len()
                );
            }
            if report.keys_to_add.is_empty() {
                println!("rien à ajouter");
                return Ok(());
            }
            let n = report.final_validators;
            println!(
                "  ✓ {} nœud(s) prêt(s) ; après l'ajout : {n} validateurs, {} doivent voter, {} panne(s) tolérée(s)",
                report.keys_to_add.len(),
                preflight::quorum(n),
                n.saturating_sub(preflight::quorum(n))
            );
            let nonce = client.status()?.admin_nonce;
            println!(
                "\nCommandes, à envoyer à la suite (ils voteront deux blocs après l'inclusion) :"
            );
            for (i, key) in report.keys_to_add.iter().enumerate() {
                let next = nonce.saturating_add(1).saturating_add(i as u64);
                println!(
                    "  vinx-wallet --node {} admin propose --out add-{i}.json --nonce {next} add-validator --key {key}",
                    reference.url
                );
            }
            println!("  puis, pour chaque fichier : deux « admin approve », et « admin submit »");
        }
        Command::Admin(cmd) => match cmd {
            AdminCommand::Status => print_governance(&client.governance()?),
            AdminCommand::Propose {
                out,
                nonce,
                action: a,
            } => {
                let status = client.status()?;
                let nonce = match nonce {
                    Some(n) => n,
                    None => status
                        .admin_nonce
                        .checked_add(1)
                        .context("nonce admin épuisé")?,
                };
                let tx = admin::propose(status.chain_id, nonce, action(a)?);
                admin::save(&tx, &out)?;
                println!(
                    "proposition {:?}, nonce {nonce} → {}",
                    tx.tx.action,
                    out.display()
                );
            }
            AdminCommand::Approve { file, key, index } => {
                let mut tx = admin::load(&file)?;
                admin::approve(&mut tx, index, &load_key(&key, pwf)?)?;
                admin::save(&tx, &file)?;
                println!("{} approbation(s)", tx.approvals.len());
            }
            AdminCommand::Show { file } => {
                let tx = admin::load(&file)?;
                println!("chaîne      {}", tx.tx.chain_id);
                println!("nonce       {}", tx.tx.nonce);
                println!("action      {:?}", tx.tx.action);
                let idx: Vec<String> = tx
                    .approvals
                    .iter()
                    .map(|a| a.signer_index.to_string())
                    .collect();
                println!("approuvée par les signataires : {}", idx.join(", "));
            }
            AdminCommand::Submit { file } => {
                let tx = admin::load(&file)?;
                let id = client.submit_admin(&tx).map_err(|e| anyhow!("{e}"))?;
                println!("transaction admin {id}");
            }
        },
    }
    Ok(())
}
