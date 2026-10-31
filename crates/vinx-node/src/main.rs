//! Ligne de commande du nœud VinX Ledger.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use libp2p::{Multiaddr, PeerId};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tracing::info;
use vinx_crypto::{Address, KeyPair};
use vinx_node::encoding::{key_from_hex, key_to_hex};
use vinx_node::genesis_file::GenesisFile;
use vinx_node::keyfile::{DEFAULT_KDF, KeyFile, password};
use vinx_node::network::{self, NetConfig};
use vinx_node::{api, node};
use vinx_state::Genesis;

#[derive(Parser)]
#[command(name = "vinx-node", version, about = "Nœud VinX Ledger")]
struct Cli {
    /// Fichier contenant le mot de passe des clés (sinon : variable VINX_KEY_PASSWORD).
    #[arg(long, global = true)]
    password_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Génère une paire de clés chiffrée.
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Prépare un réseau de développement : 3 clés admin, 1 clé de validateur, la genèse.
    Devnet {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value = "vinx-devnet-1")]
        chain_id: String,
        /// Nombre de validateurs.
        #[arg(long, default_value_t = 1)]
        validators: usize,
        /// Période de l'escrow en secondes, au lieu d'un mois. Réservé aux réseaux
        /// de test (`chain_id` contenant « testnet » ou « devnet »).
        #[arg(long)]
        escrow_period: Option<u64>,
        /// VINX versés à la trésorerie pour financer les testeurs, pris sur la
        /// dernière tranche d'escrow.
        #[arg(long, default_value = "1000000")]
        faucet: String,
        /// Durée d'une époque d'émission, en blocs (17 280 ≈ 24 h).
        #[arg(long)]
        epoch_blocks: Option<u64>,
    },
    /// Écrit la genèse d'un réseau à partir des clés publiques de ses participants.
    ///
    /// Chaque clé est créée sur la machine qui la détient (`keygen`) ; seules
    /// les clés publiques sont réunies ici.
    Genesis {
        #[arg(long)]
        chain_id: String,
        /// Clé publique d'un signataire admin (exactement 3 fois).
        #[arg(long = "admin", required = true, num_args = 1)]
        admins: Vec<String>,
        /// Clé publique d'un validateur (répétable).
        #[arg(long = "validator", required = true, num_args = 1)]
        validators: Vec<String>,
        /// Horodatage Unix de la genèse (par défaut : maintenant).
        #[arg(long)]
        time: Option<u64>,
        /// Période de l'escrow en secondes, au lieu d'un mois. Réservé aux réseaux
        /// de test (`chain_id` contenant « testnet » ou « devnet »).
        #[arg(long)]
        escrow_period: Option<u64>,
        /// Réseau de test : VINX versés à la trésorerie pour financer les
        /// testeurs (le robinet), pris sur la dernière tranche d'escrow.
        #[arg(long)]
        faucet: Option<String>,
        /// Durée d'une époque d'émission, en blocs (17 280 ≈ 24 h).
        #[arg(long)]
        epoch_blocks: Option<u64>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Affiche l'identifiant réseau du nœud (crée sa clé réseau si besoin).
    PeerId {
        #[arg(long, default_value = "data")]
        data_dir: PathBuf,
    },
    /// Démarre le nœud.
    Run {
        #[arg(long)]
        genesis: PathBuf,
        /// Clé du validateur. Sans clé, le nœud suit la chaîne en observateur.
        #[arg(long)]
        key: Option<PathBuf>,
        #[arg(long, default_value = "data")]
        data_dir: PathBuf,
        /// Adresse d'écoute de l'API (127.0.0.1 par défaut : non exposée au réseau).
        #[arg(long, default_value = "127.0.0.1:8545")]
        api: SocketAddr,
        /// Adresse d'écoute P2P.
        #[arg(long, default_value = "/ip4/0.0.0.0/tcp/26656")]
        listen: Multiaddr,
        /// Pair à joindre, au format /ip4/…/tcp/…/p2p/<id> (répétable). Autorisé d'office.
        #[arg(long = "peer")]
        peers: Vec<Multiaddr>,
        /// Identifiant de pair supplémentaire autorisé à se connecter (répétable).
        #[arg(long = "allow")]
        allowed: Vec<PeerId>,
        /// Conserver tous les blocs (nœud archive) au lieu de les élaguer.
        #[arg(long)]
        archive: bool,
        /// Durée de conservation des blocs, en heures (ignorée avec --archive).
        #[arg(long, default_value_t = 24)]
        block_retention_hours: u64,
        /// Transactions en attente au plus (au moins 5 000). Environ 500 octets
        /// de mémoire par transaction.
        #[arg(long, default_value_t = vinx_node::mempool::MAX_PENDING)]
        mempool_size: usize,
        /// Requêtes par minute et par adresse IP sur l'API (0 : sans limite).
        /// Derrière un proxy local (Caddy), l'adresse du visiteur est lue dans
        /// `X-Forwarded-For`.
        #[arg(long, default_value_t = 600)]
        rate_limit: u32,
        /// Fichier JSON des noms de validateurs pour le tableau de bord :
        /// `{ "01…clé…": "Validateur-3", … }`.
        #[arg(long)]
        validator_names: Option<PathBuf>,
    },
}

/// Lit le fichier des noms de validateurs : un objet JSON clé → nom.
fn load_names(path: &Path) -> Result<std::collections::BTreeMap<vinx_crypto::PublicKey, String>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("lecture de {}", path.display()))?;
    let raw: std::collections::BTreeMap<String, String> = serde_json::from_str(&text)
        .with_context(|| {
            format!(
                "{} : objet JSON attendu, {{ \"01…\": \"nom\" }}",
                path.display()
            )
        })?;
    raw.into_iter()
        .map(|(k, name)| {
            let key = key_from_hex(k.trim())
                .with_context(|| format!("{} : clé invalide « {k} »", path.display()))?;
            Ok((key, name))
        })
        .collect()
}

/// Options de genèse réservées aux réseaux de test, et durée d'époque.
fn with_test_options(
    genesis: Genesis,
    escrow_period: Option<u64>,
    faucet: Option<&str>,
    epoch_blocks: Option<u64>,
) -> Result<Genesis> {
    let mut genesis = with_escrow_period(genesis, escrow_period)?;
    if let Some(faucet) = faucet {
        let amount = vinx_wallet_amount(faucet)?;
        if amount > 0 {
            if !is_test_chain(&genesis.chain_id) {
                bail!(
                    "--faucet est réservé aux réseaux de test : l'identifiant de chaîne \
                     doit contenir « testnet » ou « devnet »"
                );
            }
            genesis = genesis
                .with_faucet(amount)
                .map_err(|e| anyhow::anyhow!("robinet : {e}"))?;
        }
    }
    if let Some(n) = epoch_blocks {
        if n == 0 {
            bail!("--epoch-blocks doit valoir au moins 1");
        }
        genesis.epoch_blocks = n;
    }
    Ok(genesis)
}

fn is_test_chain(chain_id: &str) -> bool {
    ["testnet", "devnet"].iter().any(|t| chain_id.contains(t))
}

/// Montant en VINX entiers (« 1000000 ») en unités.
fn vinx_wallet_amount(s: &str) -> Result<vinx_core::Amount> {
    let whole: vinx_core::Amount = s
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("montant en VINX entiers attendu : « {s} »"))?;
    whole
        .checked_mul(vinx_core::UNITS_PER_VINX)
        .ok_or_else(|| anyhow::anyhow!("montant trop grand : {s}"))
}

/// Remplace la période de l'escrow, sur un réseau de test seulement.
fn with_escrow_period(mut genesis: Genesis, period: Option<u64>) -> Result<Genesis> {
    let Some(period) = period else {
        return Ok(genesis);
    };
    let is_test = ["testnet", "devnet"]
        .iter()
        .any(|t| genesis.chain_id.contains(t));
    if !is_test {
        bail!(
            "--escrow-period est réservé aux réseaux de test : l'identifiant de chaîne \
             doit contenir « testnet » ou « devnet »"
        );
    }
    genesis.escrow = vinx_state::Escrow::genesis_with_period(genesis.genesis_time, period)
        .context("période d'escrow invalide")?;
    Ok(genesis)
}

fn new_key(path: &Path, password: &str) -> Result<KeyPair> {
    let keypair = KeyPair::generate();
    KeyFile::encrypt(&keypair, password, DEFAULT_KDF)?.save(path)?;
    Ok(keypair)
}

fn describe(name: &str, keypair: &KeyPair) {
    println!(
        "{name:<12} clé {}  adresse {}",
        key_to_hex(&keypair.public_key()),
        Address::from_public_key(&keypair.public_key())
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    let pw = || password(cli.password_file.as_deref());

    match &cli.command {
        Command::Keygen { out } => {
            let keypair = new_key(out, &pw()?)?;
            describe("clé", &keypair);
        }
        Command::Devnet {
            dir,
            chain_id,
            validators: count,
            escrow_period,
            faucet,
            epoch_blocks,
        } => {
            let pw = pw()?;
            if *count == 0 {
                bail!("il faut au moins un validateur");
            }
            if dir.exists() && std::fs::read_dir(dir)?.next().is_some() {
                bail!("{} n'est pas vide", dir.display());
            }
            std::fs::create_dir_all(dir)?;
            let admins = [
                new_key(&dir.join("admin-1.json"), &pw)?,
                new_key(&dir.join("admin-2.json"), &pw)?,
                new_key(&dir.join("admin-3.json"), &pw)?,
            ];
            let validators = (1..=*count)
                .map(|i| new_key(&dir.join(format!("validator-{i}.json")), &pw))
                .collect::<Result<Vec<_>>>()?;
            let genesis_time = node::now_ms() / 1000;
            let genesis = Genesis::standard(
                chain_id.clone(),
                genesis_time,
                [
                    admins[0].public_key(),
                    admins[1].public_key(),
                    admins[2].public_key(),
                ],
                validators.iter().map(KeyPair::public_key).collect(),
            )
            .context("horodatage de genèse invalide")?;
            let genesis = with_test_options(genesis, *escrow_period, Some(faucet), *epoch_blocks)?;
            GenesisFile::save(&genesis, &dir.join("genesis.json"))?;
            for (i, a) in admins.iter().enumerate() {
                describe(&format!("admin-{}", i.saturating_add(1)), a);
            }
            for (i, v) in validators.iter().enumerate() {
                describe(&format!("validator-{}", i.saturating_add(1)), v);
            }
            println!(
                "trésorerie   adresse {}  (robinet : {} VINX)",
                Address::treasury(),
                genesis.treasury_balance / vinx_core::UNITS_PER_VINX
            );
            println!(
                "\nDémarrer : vinx-node run --genesis {0}/genesis.json --key {0}/validator-1.json --data-dir {0}/data-1",
                dir.display()
            );
        }
        Command::Genesis {
            chain_id,
            admins,
            validators,
            time,
            escrow_period,
            faucet,
            epoch_blocks,
            out,
        } => {
            let admins = admins
                .iter()
                .map(|k| key_from_hex(k))
                .collect::<Result<Vec<_>>>()?;
            let admins: [_; 3] = admins
                .try_into()
                .map_err(|_| anyhow::anyhow!("il faut exactement 3 clés --admin"))?;
            let validators = validators
                .iter()
                .map(|k| key_from_hex(k))
                .collect::<Result<Vec<_>>>()?;
            let time = time.unwrap_or_else(|| node::now_ms() / 1000);
            let genesis = Genesis::standard(chain_id.clone(), time, admins, validators)
                .context("horodatage de genèse invalide")?;
            let genesis =
                with_test_options(genesis, *escrow_period, faucet.as_deref(), *epoch_blocks)?;
            // Vérifie toutes les règles (doublons, supply) avant d'écrire.
            genesis
                .clone()
                .into_state()
                .map_err(|e| anyhow::anyhow!("genèse invalide : {e}"))?;
            if out.exists() {
                bail!("{} existe déjà", out.display());
            }
            GenesisFile::save(&genesis, out)?;
            println!("genèse écrite dans {}", out.display());
        }
        Command::PeerId { data_dir } => {
            let id = network::load_or_create_identity(data_dir)?;
            println!("{}", id.public().to_peer_id());
        }
        Command::Run {
            genesis,
            key,
            data_dir,
            api: addr,
            listen,
            peers,
            allowed,
            archive,
            block_retention_hours,
            mempool_size,
            rate_limit,
            validator_names,
        } => {
            let genesis = GenesisFile::load(genesis)?;
            let keypair = match key {
                Some(path) => Some(KeyFile::load(path)?.decrypt(&pw()?)?),
                None => None,
            };
            let shared = node::open(genesis, data_dir)?;
            let identity = network::load_or_create_identity(data_dir)?;
            let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
            let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
            let (req_tx, req_rx) = tokio::sync::mpsc::unbounded_channel();
            {
                let mut s = node::lock(&shared);
                s.outbound = Some(out_tx);
                s.requests = Some(req_tx);
                s.retention_ms =
                    (!*archive).then(|| block_retention_hours.saturating_mul(3_600_000));
                s.mempool.set_capacity(*mempool_size);
                if let Some(path) = validator_names {
                    s.names = load_names(path)?;
                    info!(noms = s.names.len(), "noms de validateurs chargés");
                }
                info!(
                    archive = *archive,
                    "conservation des blocs : {}",
                    if *archive {
                        "tous".to_owned()
                    } else {
                        format!("{block_retention_hours} h")
                    }
                );
            }
            {
                let s = node::lock(&shared);
                info!(
                    chain_id = %s.chain.state.chain_id,
                    height = s.chain.state.height,
                    "chaîne chargée"
                );
            }
            let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .with_context(|| format!("écoute sur {addr}"))?;
            info!(%addr, "API démarrée");
            let mut api_stop = stop_rx.clone();
            let app = api::router_with_limit(shared.clone(), *rate_limit)
                .into_make_service_with_connect_info::<std::net::SocketAddr>();
            let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                let _ = api_stop.changed().await;
            });
            let stop_tx = std::sync::Arc::new(stop_tx);
            let on_failure = stop_tx.clone();
            // Si le consensus s'arrête (erreur), l'API s'arrête aussi.
            let net = tokio::spawn(network::run(
                identity,
                NetConfig {
                    listen: listen.clone(),
                    peers: peers.clone(),
                    allowed: allowed.clone(),
                },
                shared.clone(),
                out_rx,
                req_rx,
                in_tx,
                stop_rx.clone(),
            ));
            let consensus = tokio::spawn(async move {
                let result = node::run(shared, keypair, in_rx, stop_rx).await;
                let _ = on_failure.send(true);
                result
            });
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                info!("arrêt demandé");
                let _ = stop_tx.send(true);
            });
            let (served, ran, networked) = tokio::join!(server, consensus, net);
            served?;
            ran??;
            networked??;
        }
    }
    Ok(())
}
