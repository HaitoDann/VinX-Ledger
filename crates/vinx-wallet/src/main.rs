mod amount;
mod client;
mod error;
mod keystore;

use amount::parse_amount;
use client::RpcClient;
use error::WalletError;
use keystore::KeyStore;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
#[allow(unused_imports)]
use std::str::FromStr;
use vinx_core::amount::DEFAULT_FEE_FLOOR_ATOMS;
use vinx_core::{Amount, GovernanceAction, ProtocolVersion, Transaction};
use vinx_crypto::Address;

// ─── CLI definition ───────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "vinx-wallet",
    about = "VinX Ledger wallet — generate keys, check balances, send transactions",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a new keypair and save it to a wallet file
    Keygen {
        /// Output file path
        #[arg(short, long, default_value = "wallet.json")]
        output: PathBuf,
    },
    /// Show the address stored in a wallet file
    Address {
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
    },
    /// Check the balance of an address
    Balance {
        /// Bech32 address (vinx1...)
        address: String,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Transfer VINX to another address
    Transfer {
        /// Recipient address (vinx1...)
        #[arg(long)]
        to: String,
        /// Amount to send (e.g. 100 or 99.50)
        #[arg(long)]
        amount: String,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Lock VINX into the staking pool to earn fee rewards
    Stake {
        /// Amount to stake (e.g. 1000)
        #[arg(long)]
        amount: String,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Unlock staked VINX back to your balance
    Unstake {
        /// Amount to unstake
        #[arg(long)]
        amount: String,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Show details of a block
    Block {
        /// Block height
        height: u64,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Show node status (height, mempool)
    Status {
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Look up a transaction by its hash
    Tx {
        /// Hex-encoded transaction hash
        hash: String,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Freeze an account (admin only)
    Freeze {
        /// Target address to freeze
        #[arg(long)]
        target: String,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Unfreeze an account (admin only)
    Unfreeze {
        /// Target address to unfreeze
        #[arg(long)]
        target: String,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Announce a protocol upgrade (admin only)
    AnnounceUpgrade {
        /// New protocol version, e.g. 1.1.0
        #[arg(long)]
        version: String,
        /// Block height at which the upgrade activates
        #[arg(long)]
        activation_height: u64,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Show the active validator set
    Validators {
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Show the current protocol version and any pending upgrade
    Protocol {
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Add a new validator to the PoA set (admin only)
    AddValidator {
        /// Address of the new validator (vinx1...)
        #[arg(long)]
        validator: String,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Remove a validator from the PoA set (admin only)
    RemoveValidator {
        /// Address of the validator to remove (vinx1...)
        #[arg(long)]
        validator: String,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Show transaction history for an address
    History {
        /// Bech32 address (vinx1...)
        address: String,
        /// Maximum transactions to show
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Skip the first N transactions
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Generate a new wallet from a 12-word BIP-39 mnemonic
    NewWallet {
        /// Output file path
        #[arg(short, long, default_value = "wallet.json")]
        output: PathBuf,
    },
    /// Restore a wallet from a BIP-39 mnemonic phrase
    RestoreWallet {
        /// Output file path
        #[arg(short, long, default_value = "wallet.json")]
        output: PathBuf,
    },
    /// Execute a governance action immediately (admin only).
    AdminAction {
        /// JSON-encoded action, e.g. '{"AddValidator":"vinx1abc..."}'
        #[arg(long)]
        action: String,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Show network economic statistics
    NetworkStats {
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
}

// ─── Entry point ─────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli.command).await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

async fn run(cmd: Commands) -> Result<(), WalletError> {
    match cmd {
        Commands::Keygen { output } => cmd_keygen(&output),
        Commands::Address { wallet } => cmd_address(&wallet),
        Commands::Balance { address, node } => cmd_balance(&address, &node).await,
        Commands::Transfer {
            to,
            amount,
            wallet,
            node,
        } => cmd_transfer(&to, &amount, &wallet, &node).await,
        Commands::Stake {
            amount,
            wallet,
            node,
        } => cmd_stake(&amount, &wallet, &node).await,
        Commands::Unstake {
            amount,
            wallet,
            node,
        } => cmd_unstake(&amount, &wallet, &node).await,
        Commands::Block { height, node } => cmd_block(height, &node).await,
        Commands::Status { node } => cmd_status(&node).await,
        Commands::Tx { hash, node } => cmd_tx(&hash, &node).await,
        Commands::Freeze {
            target,
            wallet,
            node,
        } => cmd_freeze(&target, &wallet, &node).await,
        Commands::Unfreeze {
            target,
            wallet,
            node,
        } => cmd_unfreeze(&target, &wallet, &node).await,
        Commands::AnnounceUpgrade {
            version,
            activation_height,
            wallet,
            node,
        } => cmd_announce_upgrade(&version, activation_height, &wallet, &node).await,
        Commands::AddValidator {
            validator,
            wallet,
            node,
        } => cmd_add_validator(&validator, &wallet, &node).await,
        Commands::RemoveValidator {
            validator,
            wallet,
            node,
        } => cmd_remove_validator(&validator, &wallet, &node).await,
        Commands::Validators { node } => cmd_validators(&node).await,
        Commands::Protocol { node } => cmd_protocol(&node).await,
        Commands::History {
            address,
            limit,
            offset,
            node,
        } => cmd_history(&address, limit, offset, &node).await,
        Commands::NewWallet { output } => cmd_new_wallet(&output),
        Commands::RestoreWallet { output } => cmd_restore_wallet(&output),
        Commands::AdminAction {
            action,
            wallet,
            node,
        } => cmd_admin_action(&action, &wallet, &node).await,
        Commands::NetworkStats { node } => cmd_network_stats(&node).await,
    }
}

// ─── Commands ─────────────────────────────────────────────────────────────────

fn cmd_keygen(output: &Path) -> Result<(), WalletError> {
    let (ks, _) = KeyStore::generate();
    ks.save(output)?;
    println!("Address : {}", ks.address);
    println!("Saved   : {}", output.display());
    println!();
    println!("Keep your wallet file safe — it contains your secret key.");
    Ok(())
}

fn cmd_address(wallet: &Path) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    println!("{}", ks.address());
    Ok(())
}

async fn cmd_balance(address: &str, node: &str) -> Result<(), WalletError> {
    let client = RpcClient::new(node);
    let acc = client.get_account(address).await?;
    println!("Address : {}", acc.address);
    println!("Balance : {}", acc.balance);
    println!("Staked  : {}", acc.staked);
    println!("Nonce   : {}", acc.nonce);
    println!("Frozen  : {}", if acc.frozen { "YES" } else { "no" });
    Ok(())
}

async fn cmd_transfer(
    to: &str,
    amount_str: &str,
    wallet: &Path,
    node: &str,
) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let to_addr = Address::from_bech32(to)?;
    let amount = parse_amount(amount_str)?;
    let floor = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
    let fee = amount.calculate_fee(floor);

    // Fetch current nonce
    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let tx = Transaction::new_transfer(&kp, to_addr, amount, fee, nonce);

    println!("From    : {}", ks.address());
    println!("To      : {}", to);
    println!("Amount  : {}", amount);
    println!("Fee     : {}", fee);
    println!("Nonce   : {}", nonce);

    let resp = client.submit_tx(&tx).await?;
    if resp.accepted {
        println!("Status  : accepted");
        println!("Tx hash : {}", resp.tx_hash);
    } else {
        println!("Status  : rejected");
    }
    Ok(())
}

async fn cmd_stake(amount_str: &str, wallet: &Path, node: &str) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let amount = parse_amount(amount_str)?;

    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let tx = Transaction::new_stake(&kp, amount, Amount::ZERO, nonce);

    println!("Address : {}", ks.address());
    println!("Stake   : {}", amount);
    println!("Nonce   : {}", nonce);

    let resp = client.submit_tx(&tx).await?;
    if resp.accepted {
        println!("Status  : accepted");
        println!("Tx hash : {}", resp.tx_hash);
    } else {
        println!("Status  : rejected");
    }
    Ok(())
}

async fn cmd_unstake(amount_str: &str, wallet: &Path, node: &str) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let amount = parse_amount(amount_str)?;

    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let tx = Transaction::new_unstake(&kp, amount, Amount::ZERO, nonce);

    println!("Address : {}", ks.address());
    println!("Unstake : {}", amount);
    println!("Nonce   : {}", nonce);

    let resp = client.submit_tx(&tx).await?;
    if resp.accepted {
        println!("Status  : accepted");
        println!("Tx hash : {}", resp.tx_hash);
    } else {
        println!("Status  : rejected");
    }
    Ok(())
}

async fn cmd_block(height: u64, node: &str) -> Result<(), WalletError> {
    let client = RpcClient::new(node);
    let block = client.get_block(height).await?;
    println!("Height    : {}", block.height);
    println!("Hash      : {}", block.hash);
    println!("Prev hash : {}", block.prev_hash);
    println!("Timestamp : {}", block.timestamp);
    println!("Validator : {}", block.validator);
    println!("Tx count  : {}", block.tx_count);
    if !block.transactions.is_empty() {
        println!();
        println!("Transactions:");
        for tx in &block.transactions {
            println!(
                "  {:?}  {}  {}  →  {}  fee {}",
                tx.tx_type, tx.hash, tx.from, tx.to, tx.fee
            );
        }
    }
    Ok(())
}

async fn cmd_status(node: &str) -> Result<(), WalletError> {
    let client = RpcClient::new(node);
    let health = client.health().await?;
    println!("Node    : {}", node);
    println!("Status  : {}", health.status);
    println!("Height  : {}", health.height);
    println!("Mempool : {} pending", health.mempool_pending);
    Ok(())
}

async fn cmd_tx(hash: &str, node: &str) -> Result<(), WalletError> {
    let client = RpcClient::new(node);
    let tx = client.get_tx(hash).await?;
    println!("Hash      : {}", tx.hash);
    println!("Type      : {}", tx.tx_type);
    println!("Block     : {} ({})", tx.block_height, tx.block_hash);
    println!("From      : {}", tx.from);
    println!("To        : {}", tx.to);
    println!("Amount    : {}", tx.amount);
    println!("Fee       : {}", tx.fee);
    println!("Nonce     : {}", tx.nonce);
    Ok(())
}

async fn cmd_freeze(target: &str, wallet: &Path, node: &str) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let target_addr = Address::from_bech32(target)?;

    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let tx = Transaction::new_freeze(&kp, target_addr, nonce);

    println!("Admin   : {}", ks.address());
    println!("Target  : {}", target);
    println!("Action  : freeze");
    println!("Nonce   : {}", nonce);

    let resp = client.submit_tx(&tx).await?;
    if resp.accepted {
        println!("Status  : accepted");
        println!("Tx hash : {}", resp.tx_hash);
    } else {
        println!("Status  : rejected");
    }
    Ok(())
}

async fn cmd_unfreeze(target: &str, wallet: &Path, node: &str) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let target_addr = Address::from_bech32(target)?;

    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let tx = Transaction::new_unfreeze(&kp, target_addr, nonce);

    println!("Admin   : {}", ks.address());
    println!("Target  : {}", target);
    println!("Action  : unfreeze");
    println!("Nonce   : {}", nonce);

    let resp = client.submit_tx(&tx).await?;
    if resp.accepted {
        println!("Status  : accepted");
        println!("Tx hash : {}", resp.tx_hash);
    } else {
        println!("Status  : rejected");
    }
    Ok(())
}

async fn cmd_announce_upgrade(
    version_str: &str,
    activation_height: u64,
    wallet: &Path,
    node: &str,
) -> Result<(), WalletError> {
    let version = ProtocolVersion::from_str(version_str)
        .map_err(|e| WalletError::NodeError(format!("Invalid version '{}': {}", version_str, e)))?;

    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;

    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let tx = Transaction::new_announce_upgrade(&kp, version.clone(), activation_height, nonce);

    println!("Admin             : {}", ks.address());
    println!("New version       : {}", version);
    println!("Activation height : {}", activation_height);
    println!("Nonce             : {}", nonce);

    let resp = client.submit_tx(&tx).await?;
    if resp.accepted {
        println!("Status  : accepted");
        println!("Tx hash : {}", resp.tx_hash);
    } else {
        println!("Status  : rejected");
    }
    Ok(())
}

async fn cmd_add_validator(
    validator_str: &str,
    wallet: &Path,
    node: &str,
) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let validator_addr = Address::from_bech32(validator_str)?;

    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let tx = Transaction::new_add_validator(&kp, validator_addr, nonce);

    println!("Admin     : {}", ks.address());
    println!("Validator : {}", validator_str);
    println!("Action    : add-validator");
    println!("Nonce     : {}", nonce);

    let resp = client.submit_tx(&tx).await?;
    if resp.accepted {
        println!("Status  : accepted");
        println!("Tx hash : {}", resp.tx_hash);
    } else {
        println!("Status  : rejected");
    }
    Ok(())
}

async fn cmd_remove_validator(
    validator_str: &str,
    wallet: &Path,
    node: &str,
) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let validator_addr = Address::from_bech32(validator_str)?;

    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let tx = Transaction::new_remove_validator(&kp, validator_addr, nonce);

    println!("Admin     : {}", ks.address());
    println!("Validator : {}", validator_str);
    println!("Action    : remove-validator");
    println!("Nonce     : {}", nonce);

    let resp = client.submit_tx(&tx).await?;
    if resp.accepted {
        println!("Status  : accepted");
        println!("Tx hash : {}", resp.tx_hash);
    } else {
        println!("Status  : rejected");
    }
    Ok(())
}

async fn cmd_validators(node: &str) -> Result<(), WalletError> {
    let client = RpcClient::new(node);
    let vs = client.get_validators().await?;
    println!("Validators : {}", vs.count);
    println!("Quorum     : {}/{}", vs.quorum, vs.count);
    println!();
    for (i, v) in vs.validators.iter().enumerate() {
        println!("  [{:>2}] {}", i + 1, v);
    }
    Ok(())
}

async fn cmd_protocol(node: &str) -> Result<(), WalletError> {
    let client = RpcClient::new(node);
    let ps = client.get_protocol_status().await?;
    println!("Version  : {}", ps.current_version);
    match ps.pending_upgrade {
        None => println!("Upgrade  : none scheduled"),
        Some(u) => {
            println!("Upgrade  : {} at block {}", u.version, u.activation_height);
            println!("Announced: block {}", u.announced_at);
        }
    }
    Ok(())
}

async fn cmd_history(
    address: &str,
    limit: usize,
    offset: usize,
    node: &str,
) -> Result<(), WalletError> {
    let client = RpcClient::new(node);
    let info = client.get_account_txs(address, limit, offset).await?;
    println!("Address  : {}", info.address);
    println!("Total    : {} transactions", info.total);
    if info.txs.is_empty() {
        println!("(no transactions found)");
        return Ok(());
    }
    println!();
    println!(
        "{:<8} {:<12} {:<16} {:<20} Hash",
        "Block", "Type", "Amount", "Fee"
    );
    println!("{}", "-".repeat(80));
    for tx in &info.txs {
        println!(
            "{:<8} {:<12} {:<16} {:<20} {}",
            tx.block_height,
            tx.tx_type,
            tx.amount,
            tx.fee,
            &tx.hash[..16],
        );
    }
    if info.total > offset + info.txs.len() {
        println!();
        println!(
            "Showing {}-{} of {}. Use --offset {} to see more.",
            offset + 1,
            offset + info.txs.len(),
            info.total,
            offset + limit,
        );
    }
    Ok(())
}

fn cmd_new_wallet(output: &Path) -> Result<(), WalletError> {
    use bip39::Mnemonic;
    // Generate 16 bytes of OS entropy = 12-word mnemonic
    let entropy = {
        use rand::RngCore;
        let mut e = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut e);
        e
    };
    let mnemonic = Mnemonic::from_entropy(&entropy)
        .map_err(|e| WalletError::Keystore(format!("mnemonic generation failed: {}", e)))?;
    let seed = mnemonic.to_seed("");
    let key_bytes: [u8; 32] = seed[..32]
        .try_into()
        .map_err(|_| WalletError::Keystore("seed too short".to_string()))?;
    let kp = vinx_crypto::KeyPair::from_secret_bytes(&key_bytes);
    let address = Address::from_public_key(&kp.public_key()).to_string();
    let ks = keystore::KeyStore {
        address: address.clone(),
        secret_key_hex: hex::encode(kp.secret_bytes()),
    };
    ks.save(output)?;
    println!("Wallet generated");
    println!("  Address  : {}", address);
    println!("  Mnemonic : {}", mnemonic);
    println!("  File     : {}", output.display());
    println!();
    println!("Write down your mnemonic — it cannot be recovered!");
    Ok(())
}

fn cmd_restore_wallet(output: &Path) -> Result<(), WalletError> {
    use std::io::{self, BufRead};
    print!("Enter your 12-word mnemonic: ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let stdin = io::stdin();
    let line = stdin
        .lock()
        .lines()
        .next()
        .ok_or_else(|| WalletError::Keystore("no input".to_string()))?
        .map_err(|e| WalletError::Keystore(format!("read error: {}", e)))?;
    let mnemonic: bip39::Mnemonic = line
        .parse()
        .map_err(|e: bip39::Error| WalletError::Keystore(format!("invalid mnemonic: {}", e)))?;
    let seed = mnemonic.to_seed("");
    let key_bytes: [u8; 32] = seed[..32]
        .try_into()
        .map_err(|_| WalletError::Keystore("seed too short".to_string()))?;
    let kp = vinx_crypto::KeyPair::from_secret_bytes(&key_bytes);
    let address = Address::from_public_key(&kp.public_key()).to_string();
    let ks = keystore::KeyStore {
        address: address.clone(),
        secret_key_hex: hex::encode(kp.secret_bytes()),
    };
    ks.save(output)?;
    println!("Wallet restored");
    println!("  Address : {}", address);
    println!("  File    : {}", output.display());
    Ok(())
}

async fn cmd_admin_action(action_json: &str, wallet: &Path, node: &str) -> Result<(), WalletError> {
    // Parse the action from JSON
    let action: GovernanceAction = serde_json::from_str(action_json)
        .map_err(|e| WalletError::NodeError(format!("invalid action JSON: {}", e)))?;
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;

    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let tx = Transaction::new_admin_action(&kp, &action, nonce);

    println!("Admin  : {}", ks.address());
    println!("Nonce  : {}", nonce);

    let resp = client.submit_tx(&tx).await?;
    if resp.accepted {
        println!("Status : accepted");
        println!("Tx hash: {}", resp.tx_hash);
    } else {
        println!("Status : rejected");
    }
    Ok(())
}

async fn cmd_network_stats(node: &str) -> Result<(), WalletError> {
    let client = RpcClient::new(node);
    let stats = client.get_network_stats().await?;
    println!("Network Statistics");
    println!("  Base fee (atoms) : {}", stats.base_fee_atoms);
    println!("  Staking pool     : {}", stats.staking_pool);
    println!("  Melt pool        : {}", stats.melt_pool);
    println!("  Circulating      : {}", stats.circulating_supply);
    Ok(())
}
