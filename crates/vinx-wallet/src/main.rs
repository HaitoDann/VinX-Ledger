mod amount;
mod client;
mod error;
mod keystore;
mod names;
mod receipt;

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
        /// Recipient: a vinx1… address, or a payment address name@domain (resolved
        /// off-chain, the resolved vinx1… address is shown)
        #[arg(long)]
        to: String,
        /// Amount to send (e.g. 100 or 99.50)
        #[arg(long)]
        amount: String,
        /// Public memo, at most 32 bytes (invoice number, deposit reference)
        #[arg(long)]
        memo: Option<String>,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Lock VINX as stake to earn work-emission rewards (minting progressif)
    Stake {
        /// Amount to stake (e.g. 1000)
        #[arg(long)]
        amount: String,
        /// Validator keys file, as printed by
        /// `vinx-node --genesis-entry --validator-owner <this wallet>` — required for the
        /// bond that makes this wallet a validator (ADR 0075/0084).
        #[arg(long)]
        validator_keys: Option<PathBuf>,
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
    /// Announce a protocol upgrade (admin only)
    AnnounceUpgrade {
        /// New protocol version, e.g. 1.1.0
        #[arg(long)]
        version: String,
        /// Unix timestamp (seconds) at which the upgrade activates (ADR 0006)
        #[arg(long)]
        activation_ts: u64,
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
    /// Fetch, verify and save the payment receipt of a transaction (ADR 0083). Keep it:
    /// it proves the payment even after nodes prune the block.
    Receipt {
        /// Hex-encoded transaction hash
        hash: String,
        /// Directory where the receipt is saved as <hash>.json
        #[arg(long, default_value = "receipts")]
        out: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Change the validator's BLS voting key and operator (owner only, ADR 0084)
    SetValidatorKeys {
        /// Keys file printed by `vinx-node --genesis-entry --validator-owner <owner>`
        #[arg(long)]
        keys: PathBuf,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Put a jailed validator back in rotation (signed by its owner or operator)
    Unjail {
        /// Validator (owner) address
        #[arg(long)]
        validator: String,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Require a memo on every transfer to this account (exchange deposit addresses)
    MemoRequired {
        /// on | off
        state: String,
        #[arg(short, long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        node: String,
    },
    /// Verify a saved payment receipt offline
    VerifyReceipt {
        /// Receipt file (<hash>.json)
        file: PathBuf,
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
            memo,
            wallet,
            node,
        } => cmd_transfer(&to, &amount, memo.as_deref(), &wallet, &node).await,
        Commands::MemoRequired {
            state,
            wallet,
            node,
        } => cmd_memo_required(&state, &wallet, &node).await,
        Commands::Stake {
            amount,
            validator_keys,
            wallet,
            node,
        } => cmd_stake(&amount, validator_keys.as_deref(), &wallet, &node).await,
        Commands::SetValidatorKeys { keys, wallet, node } => {
            cmd_set_validator_keys(&keys, &wallet, &node).await
        }
        Commands::Unjail {
            validator,
            wallet,
            node,
        } => cmd_unjail(&validator, &wallet, &node).await,
        Commands::Unstake {
            amount,
            wallet,
            node,
        } => cmd_unstake(&amount, &wallet, &node).await,
        Commands::Block { height, node } => cmd_block(height, &node).await,
        Commands::Status { node } => cmd_status(&node).await,
        Commands::Tx { hash, node } => cmd_tx(&hash, &node).await,
        Commands::Receipt { hash, out, node } => cmd_receipt(&hash, &out, &node).await,
        Commands::VerifyReceipt { file } => cmd_verify_receipt(&file),
        Commands::AnnounceUpgrade {
            version,
            activation_ts,
            wallet,
            node,
        } => cmd_announce_upgrade(&version, activation_ts, &wallet, &node).await,
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
    let pass = keystore::prompt_new_passphrase()?;
    ks.save(output, pass.as_deref())?;
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
    Ok(())
}

async fn cmd_transfer(
    to: &str,
    amount_str: &str,
    memo: Option<&str>,
    wallet: &Path,
    node: &str,
) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let memo = memo.unwrap_or_default().as_bytes();
    if memo.len() > vinx_core::amount::MAX_MEMO_BYTES {
        return Err(WalletError::InvalidAmount(format!(
            "memo of {} bytes exceeds {} bytes",
            memo.len(),
            vinx_core::amount::MAX_MEMO_BYTES
        )));
    }
    let to_addr = if names::parse_handle(to).is_some() {
        let a = names::resolve(to)
            .await
            .map_err(WalletError::InvalidAddress)?;
        println!("Resolved: {to} → {a}");
        a
    } else {
        Address::from_bech32(to)?
    };
    let amount = parse_amount(amount_str)?;
    let floor = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
    let fee = amount.calculate_fee(floor);

    // Fetch current nonce
    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let tx = Transaction::new_transfer_with_memo(&kp, to_addr, amount, fee, nonce, memo);

    println!("From    : {}", ks.address());
    println!("To      : {}", to_addr);
    if !memo.is_empty() {
        println!("Memo    : {} (public)", String::from_utf8_lossy(memo));
    }
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

/// Reads a validator keys file (`vinx-node --genesis-entry`) and checks it was made for
/// `owner` — the Proof-of-Possession is bound to the owner address.
fn load_validator_keys(
    path: &Path,
    owner: &str,
) -> Result<vinx_core::RegisterBlsKeyPayload, WalletError> {
    let bad = |m: &str| WalletError::Keystore(format!("validator keys file: {m}"));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    if v["address"].as_str() != Some(owner) {
        return Err(bad(&format!(
            "made for {}, not for this wallet ({owner}) — regenerate it with              `vinx-node --genesis-entry --validator-owner {owner}`",
            v["address"].as_str().unwrap_or("?")
        )));
    }
    let hexf = |k: &str| {
        v[k].as_str()
            .and_then(|s| hex::decode(s).ok())
            .ok_or_else(|| bad(&format!("missing or invalid {k}")))
    };
    let operator = match v["operator"].as_str() {
        Some(s) => Some(s.parse().map_err(|_| bad("invalid operator address"))?),
        None => None,
    };
    Ok(vinx_core::RegisterBlsKeyPayload {
        bls_pub_key: hexf("bls_pub_key")?,
        bls_pop: hexf("bls_pop")?,
        operator,
    })
}

async fn cmd_stake(
    amount_str: &str,
    validator_keys: Option<&Path>,
    wallet: &Path,
    node: &str,
) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let amount = parse_amount(amount_str)?;

    let client = RpcClient::new(node);
    let acc = client.get_account(ks.address()).await?;
    let nonce = acc.nonce;

    let mut tx = Transaction::new_stake(&kp, amount, Amount::ZERO, nonce);
    if let Some(path) = validator_keys {
        let payload = load_validator_keys(path, ks.address())?;
        tx.payload = borsh::to_vec(&payload).expect("payload serialization");
        tx.sign(&kp);
    }

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

async fn cmd_memo_required(state: &str, wallet: &Path, node: &str) -> Result<(), WalletError> {
    let required = match state {
        "on" => true,
        "off" => false,
        _ => return Err(WalletError::InvalidAmount("expected on or off".into())),
    };
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let client = RpcClient::new(node);
    let nonce = client.get_account(ks.address()).await?.nonce;
    let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
    let tx = Transaction::new_set_memo_required(&kp, required, fee, nonce);
    let resp = client.submit_tx(&tx).await?;
    println!(
        "Memo required = {state} for {}: {}",
        ks.address(),
        if resp.accepted {
            "accepted"
        } else {
            "rejected"
        }
    );
    Ok(())
}

async fn cmd_set_validator_keys(keys: &Path, wallet: &Path, node: &str) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let payload = load_validator_keys(keys, ks.address())?;
    let client = RpcClient::new(node);
    let nonce = client.get_account(ks.address()).await?.nonce;
    let tx = Transaction::new_register_bls_key(&kp, &payload, nonce);
    let resp = client.submit_tx(&tx).await?;
    println!("Validator : {}", ks.address());
    if let Some(op) = payload.operator {
        println!("Operator  : {op}");
    }
    println!(
        "Status    : {}",
        if resp.accepted {
            "accepted"
        } else {
            "rejected"
        }
    );
    println!("Tx hash   : {}", resp.tx_hash);
    println!("Restart the validator node with the matching BLS key once included.");
    Ok(())
}

async fn cmd_unjail(validator: &str, wallet: &Path, node: &str) -> Result<(), WalletError> {
    let ks = KeyStore::load(wallet)?;
    let kp = ks.to_keypair()?;
    let target: vinx_crypto::Address = validator.parse()?;
    let client = RpcClient::new(node);
    let nonce = client.get_account(ks.address()).await?.nonce;
    let tx = Transaction::new_unjail_for(&kp, target, nonce);
    let resp = client.submit_tx(&tx).await?;
    println!(
        "Unjail {target}: {}",
        if resp.accepted {
            "accepted"
        } else {
            "rejected"
        }
    );
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

async fn cmd_receipt(hash: &str, out: &Path, node: &str) -> Result<(), WalletError> {
    let client = RpcClient::new(node);
    let receipt: serde_json::Value = client.get_json(&format!("/tx/{hash}/proof")).await?;
    let height = receipt::verify(&receipt, hash).map_err(WalletError::Receipt)?;
    std::fs::create_dir_all(out)?;
    let path = out.join(format!("{}.json", hash.to_lowercase()));
    let json = serde_json::to_string_pretty(&receipt).expect("json");
    std::fs::write(&path, json)?;
    println!("✓ Payment included in block {height} — receipt verified");
    println!("  Saved to {}", path.display());
    Ok(())
}

fn cmd_verify_receipt(file: &Path) -> Result<(), WalletError> {
    let raw = std::fs::read_to_string(file)?;
    let receipt: serde_json::Value = serde_json::from_str(&raw)?;
    let hash = receipt["tx_hash"].as_str().unwrap_or_default().to_string();
    let height = receipt::verify(&receipt, &hash).map_err(WalletError::Receipt)?;
    println!("✓ Valid receipt: transaction {hash} is in block {height}");
    println!(
        "  Block hash: {}",
        receipt["block_hash"].as_str().unwrap_or_default()
    );
    Ok(())
}

async fn cmd_announce_upgrade(
    version_str: &str,
    activation_ts: u64,
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

    let tx = Transaction::new_announce_upgrade(&kp, version.clone(), activation_ts, nonce);

    println!("Admin             : {}", ks.address());
    println!("New version       : {}", version);
    println!("Activation (unix) : {}", activation_ts);
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

    // ADR 0007: goes through the unified governance path (AdminAction), not a
    // dedicated tx type.
    let tx =
        Transaction::new_admin_action(&kp, &GovernanceAction::AddValidator(validator_addr), nonce);

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

    // ADR 0007: unified governance path (AdminAction).
    let tx = Transaction::new_admin_action(
        &kp,
        &GovernanceAction::RemoveValidator(validator_addr),
        nonce,
    );

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
            println!("Upgrade  : {} at unix ts {}", u.version, u.activation_ts);
            println!("Announced: unix ts {}", u.announced_at);
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
    let ks = keystore::KeyStore::from_keypair(&kp);
    let pass = keystore::prompt_new_passphrase()?;
    ks.save(output, pass.as_deref())?;
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
    let ks = keystore::KeyStore::from_keypair(&kp);
    let pass = keystore::prompt_new_passphrase()?;
    ks.save(output, pass.as_deref())?;
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
