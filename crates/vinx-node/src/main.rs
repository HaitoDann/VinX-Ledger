//! Ligne de commande du nœud VinX Ledger.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use libp2p::{Multiaddr, PeerId};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tracing::info;
use vinx_crypto::{Address, KeyPair};
use vinx_node::encoding::key_to_hex;
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
    },
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
            GenesisFile::save(&genesis, &dir.join("genesis.json"))?;
            for (i, a) in admins.iter().enumerate() {
                describe(&format!("admin-{}", i.saturating_add(1)), a);
            }
            for (i, v) in validators.iter().enumerate() {
                describe(&format!("validator-{}", i.saturating_add(1)), v);
            }
            println!("trésorerie   adresse {}", Address::treasury());
            println!(
                "\nDémarrer : vinx-node run --genesis {0}/genesis.json --key {0}/validator-1.json --data-dir {0}/data-1",
                dir.display()
            );
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
            let server = axum::serve(listener, api::router(shared.clone())).with_graceful_shutdown(
                async move {
                    let _ = api_stop.changed().await;
                },
            );
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
