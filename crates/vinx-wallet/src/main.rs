//! Wallet VinX Ledger en ligne de commande.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use vinx_core::{AdminAction, Amount};
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
    /// Sans commande (double-clic) : ouvre le portefeuille.
    #[command(subcommand)]
    command: Option<Command>,
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
    /// Règles d'un compte (protocole 4) : multisig, plafond journalier, et
    /// propositions approuvées par ses signataires.
    #[command(subcommand)]
    Account(AccountCommand),
    /// Mandats de prélèvement (protocole 4).
    #[command(subcommand)]
    Mandate(MandateCommand),
    /// Vérifie que de futurs validateurs sont prêts, avant de les ajouter.
    ///
    /// `--node` désigne un nœud du réseau actuel (la référence) ; chaque
    /// `--new-node` est l'API d'un futur validateur, démarré avec sa clé.
    Preflight {
        #[arg(long = "new-node", required = true, num_args = 1)]
        new_nodes: Vec<String>,
    },
    /// Portefeuille : application locale pour envoyer et recevoir des VINX.
    ///
    /// C'est aussi ce que lance `vinx-wallet` sans argument (double-clic).
    App {
        /// Dossier du portefeuille (par défaut : celui de l'utilisateur, par exemple
        /// %APPDATA%\VinX sous Windows).
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long, default_value_t = 8700)]
        port: u16,
        /// N'ouvre pas le navigateur.
        #[arg(long)]
        no_browser: bool,
    },
    /// Cockpit : interface web locale pour administrer le réseau.
    ///
    /// Sert une page sur http://127.0.0.1:PORT/. Le dossier contient les
    /// fichiers de clé (signataires admin reconnus d'eux-mêmes, comptes),
    /// `cockpit.json` (nœuds suivis) et `propositions/`. Les clés ne quittent
    /// jamais ce poste.
    Cockpit {
        /// Dossier de travail.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value_t = 8600)]
        port: u16,
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
enum AccountCommand {
    /// Affiche les règles d'un compte.
    Policy { address: Address },
    /// Fixe les règles d'un compte qui n'en a pas encore, avec sa seule clé.
    /// Pour changer des règles existantes, passer par `propose set-policy`.
    SetPolicy {
        #[arg(long)]
        key: PathBuf,
        /// Compte émetteur, si la clé a été changée par rotation.
        #[arg(long)]
        from: Option<Address>,
        #[command(flatten)]
        policy: PolicyArgs,
    },
    /// Prépare une transaction du compte, à faire approuver par ses signataires.
    Propose {
        #[arg(long)]
        from: Address,
        #[arg(long)]
        out: PathBuf,
        #[command(subcommand)]
        action: AccountAction,
    },
    /// Ajoute l'approbation d'un signataire (indice trouvé d'après sa clé).
    Approve {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        key: PathBuf,
    },
    /// Affiche une proposition et ses approbations.
    Show {
        #[arg(long)]
        file: PathBuf,
    },
    /// Le parrain signe : son compte paiera les frais de la proposition.
    Sponsor {
        #[arg(long)]
        file: PathBuf,
        /// Clé du compte parrain.
        #[arg(long)]
        key: PathBuf,
    },
    /// Envoie une proposition (approuvée et parrainée s'il y a lieu), signée par la clé du compte.
    Submit {
        #[arg(long)]
        file: PathBuf,
        /// Clé du compte (celle qui signe ses transactions).
        #[arg(long)]
        key: PathBuf,
    },
}

#[derive(Subcommand)]
enum MandateCommand {
    /// Mandats accordés et reçus par une adresse.
    List { address: Address },
    /// Autorise un bénéficiaire à prélever sur ce compte.
    Grant {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        payee: Address,
        /// Montant maximal par période, en VINX.
        #[arg(long)]
        max: String,
        /// Durée d'une période, en jours (1 à 366).
        #[arg(long, default_value_t = 30)]
        period_days: u64,
        /// Fin du mandat, horodatage Unix en secondes.
        #[arg(long)]
        expires_at: Option<u64>,
    },
    /// Révoque le mandat accordé à un bénéficiaire.
    Revoke {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        payee: Address,
    },
    /// Prélève sur un payeur qui a accordé un mandat à ce compte.
    Collect {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        payer: Address,
        #[arg(long)]
        amount: String,
        #[arg(long)]
        memo: Option<String>,
    },
}

#[derive(Args)]
struct PolicyArgs {
    /// Clé publique d'un signataire, en hexadécimal (répétable, 10 au plus).
    #[arg(long = "signer", num_args = 1)]
    signers: Vec<String>,
    /// Approbations exigées.
    #[arg(long, default_value_t = 0)]
    threshold: u8,
    /// Plafond journalier sans approbation, en VINX.
    #[arg(long)]
    daily_limit: Option<String>,
}

impl PolicyArgs {
    fn config(&self) -> Result<vinx_core::PolicyConfig> {
        Ok(vinx_core::PolicyConfig {
            signers: self
                .signers
                .iter()
                .map(|k| key_from_hex(k))
                .collect::<Result<_>>()?,
            threshold: self.threshold,
            daily_limit: self.daily_limit.as_deref().map(parse_vinx).transpose()?,
        })
    }
}

#[derive(Subcommand)]
enum AccountAction {
    /// Paiement.
    Send {
        #[arg(long)]
        to: Address,
        #[arg(long)]
        amount: String,
        #[arg(long)]
        memo: Option<String>,
    },
    /// Nouvelles règles ; sans aucune option, retire les règles.
    SetPolicy(PolicyArgs),
    /// Nouvelle clé de compte (fichier de clé).
    RotateKey {
        #[arg(long)]
        new_key: PathBuf,
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

use vinx_wallet::params::param_id;

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

    let Some(command) = cli.command else {
        // Double-clic : la fenêtre se fermerait avant qu'on lise une erreur.
        let dir = vinx_wallet::app::default_dir();
        if let Err(e) = vinx_wallet::app::serve(&dir, 8700, true) {
            eprintln!("erreur : {e:#}");
            eprintln!("Appuie sur Entrée pour fermer.");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        return Ok(());
    };
    match command {
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
        Command::App {
            dir,
            port,
            no_browser,
        } => {
            let dir = dir.unwrap_or_else(vinx_wallet::app::default_dir);
            vinx_wallet::app::serve(&dir, port, !no_browser)?;
        }
        Command::Cockpit { dir, port } => {
            vinx_wallet::cockpit::serve(dir, port, &cli.node)?;
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
        Command::Account(cmd) => match cmd {
            AccountCommand::Policy { address } => {
                match vinx_wallet::multisig::policy(&client, &address)? {
                    None => println!("aucune règle : la clé du compte suffit"),
                    Some(p) => {
                        println!("seuil       {} approbation(s)", p.threshold);
                        for (i, k) in p.signers.iter().enumerate() {
                            println!(
                                "signataire  {i}  {}  {}",
                                key_to_hex(k),
                                Address::from_public_key(k)
                            );
                        }
                        match p.daily_limit {
                            Some(l) => println!(
                                "plafond     {} VINX par jour sans approbation",
                                format_vinx(l)
                            ),
                            None => println!(
                                "plafond     aucun : toute transaction exige les approbations"
                            ),
                        }
                    }
                }
            }
            AccountCommand::SetPolicy { key, from, policy } => {
                let keypair = load_key(&key, pwf)?;
                let body = vinx_core::TxBody::SetPolicy {
                    policy: policy.config()?,
                };
                println!(
                    "transaction {}",
                    ops::submit(&client, &keypair, from, body, None)?
                );
            }
            AccountCommand::Propose { from, out, action } => {
                let body = match action {
                    AccountAction::Send { to, amount, memo } => vinx_core::TxBody::Transfer {
                        to,
                        amount: parse_vinx(&amount)?,
                        memo: memo.map(String::into_bytes),
                    },
                    AccountAction::SetPolicy(p) => vinx_core::TxBody::SetPolicy {
                        policy: p.config()?,
                    },
                    AccountAction::RotateKey { new_key } => vinx_core::TxBody::RotateKey {
                        new_key: key_from_hex(&KeyFile::load(&new_key)?.public_key)?,
                    },
                };
                let p = vinx_wallet::multisig::propose(&client, from, body)?;
                vinx_wallet::multisig::save(&p, &out)?;
                println!("proposition : nonce {} → {}", p.tx.nonce, out.display());
            }
            AccountCommand::Approve { file, key } => {
                let mut p = vinx_wallet::multisig::load(&file)?;
                let policy = vinx_wallet::multisig::policy(&client, &p.tx.sender)?
                    .ok_or_else(|| anyhow!("ce compte n'a pas de règles : rien à approuver"))?;
                vinx_wallet::multisig::approve(&mut p, &policy.signers, &load_key(&key, pwf)?)?;
                vinx_wallet::multisig::save(&p, &file)?;
                println!(
                    "{} approbation(s) sur {} exigée(s)",
                    p.approvals.len(),
                    policy.threshold
                );
            }
            AccountCommand::Show { file } => {
                let p = vinx_wallet::multisig::load(&file)?;
                println!("compte      {}", p.tx.sender);
                println!("nonce       {}", p.tx.nonce);
                println!("frais       {} VINX", format_vinx(p.tx.fee));
                println!("transaction {:?}", p.tx.body);
                let idx: Vec<String> = p
                    .approvals
                    .iter()
                    .map(|a| a.signer_index.to_string())
                    .collect();
                println!(
                    "approuvée par les signataires : {}",
                    if idx.is_empty() {
                        "aucun".into()
                    } else {
                        idx.join(", ")
                    }
                );
            }
            AccountCommand::Sponsor { file, key } => {
                let mut p = vinx_wallet::multisig::load(&file)?;
                vinx_wallet::multisig::sponsor(&mut p, &load_key(&key, pwf)?);
                vinx_wallet::multisig::save(&p, &file)?;
                println!(
                    "parrainée : les frais ({} VINX) seront payés par ce compte",
                    format_vinx(p.tx.fee)
                );
            }
            AccountCommand::Submit { file, key } => {
                let p = vinx_wallet::multisig::load(&file)?;
                let has_key = client
                    .account(&p.tx.sender)?
                    .ok_or_else(|| anyhow!("compte inconnu"))?
                    .has_key;
                let id =
                    vinx_wallet::multisig::submit(&client, &p, &load_key(&key, pwf)?, has_key)?;
                println!("transaction {id}");
            }
        },
        Command::Mandate(cmd) => match cmd {
            MandateCommand::List { address } => {
                let v = client
                    .get_json(&format!("/accounts/{address}"))?
                    .ok_or_else(|| anyhow!("compte inconnu"))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        v.get("mandates").unwrap_or(&serde_json::Value::Null)
                    )?
                );
            }
            MandateCommand::Grant {
                key,
                payee,
                max,
                period_days,
                expires_at,
            } => {
                let body = vinx_core::TxBody::GrantMandate {
                    payee,
                    max_per_period: parse_vinx(&max)?,
                    period: period_days
                        .checked_mul(86_400)
                        .ok_or_else(|| anyhow!("période trop longue"))?,
                    expires_at,
                };
                println!(
                    "transaction {}",
                    ops::submit(&client, &load_key(&key, pwf)?, None, body, None)?
                );
            }
            MandateCommand::Revoke { key, payee } => {
                let body = vinx_core::TxBody::RevokeMandate { payee };
                println!(
                    "transaction {}",
                    ops::submit(&client, &load_key(&key, pwf)?, None, body, None)?
                );
            }
            MandateCommand::Collect {
                key,
                payer,
                amount,
                memo,
            } => {
                let body = vinx_core::TxBody::Collect {
                    payer,
                    amount: parse_vinx(&amount)?,
                    memo: memo.map(String::into_bytes),
                };
                println!(
                    "transaction {}",
                    ops::submit(&client, &load_key(&key, pwf)?, None, body, None)?
                );
            }
        },
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
