//! Tauri-agnostic wallet logic for the VinX desktop app.
//!
//! Everything here is pure and synchronous — key management, amount parsing, and
//! **local** transaction building/signing — so it is fully unit-testable and shared
//! by the Tauri backend. Networking lives in the Tauri crate; this crate never
//! touches the private key beyond signing in-process.

use serde::{Deserialize, Serialize};
use std::path::Path;
use vinx_core::amount::{Amount, DECIMAL_FACTOR};
use vinx_core::protocol::ProtocolVersion;
use vinx_core::{GovernanceAction, Transaction, TransactionType};
use vinx_crypto::{Address, KeyPair};

pub mod dto;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid amount '{0}'")]
    InvalidAmount(String),
    #[error("keystore: {0}")]
    Keystore(String),
    #[error("invalid address: {0}")]
    Address(String),
}

// ─── Keystore ────────────────────────────────────────────────────────────────

/// On-disk wallet, byte-for-byte compatible with the `vinx-wallet` CLI keystore
/// (`{ "address": "vinx1…", "secret_key_hex": "…" }`). The same file works in both.
#[derive(Serialize, Deserialize, Clone)]
pub struct Keystore {
    pub address: String,
    pub secret_key_hex: String,
}

impl Keystore {
    /// Generates a fresh key and its keystore.
    pub fn generate() -> (Self, KeyPair) {
        let kp = KeyPair::generate();
        let address = Address::from_public_key(&kp.public_key()).to_string();
        let secret_key_hex = hex::encode(kp.secret_bytes());
        (
            Self {
                address,
                secret_key_hex,
            },
            kp,
        )
    }

    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| CoreError::Keystore(format!("cannot read {}: {e}", path.display())))?;
        serde_json::from_str(&content)
            .map_err(|e| CoreError::Keystore(format!("invalid keystore file: {e}")))
    }

    pub fn save(&self, path: &Path) -> Result<(), CoreError> {
        let json =
            serde_json::to_string_pretty(self).map_err(|e| CoreError::Keystore(e.to_string()))?;
        std::fs::write(path, json)
            .map_err(|e| CoreError::Keystore(format!("cannot write {}: {e}", path.display())))
    }

    pub fn to_keypair(&self) -> Result<KeyPair, CoreError> {
        let bytes = hex::decode(&self.secret_key_hex)
            .map_err(|e| CoreError::Keystore(format!("invalid secret key hex: {e}")))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| CoreError::Keystore("secret key must be 32 bytes".to_string()))?;
        Ok(KeyPair::from_secret_bytes(&arr))
    }
}

// ─── Amount parsing / formatting ──────────────────────────────────────────────

/// Parses a human VINX string ("100", "99.50", "0.01") into atoms (10^-18 VINX).
/// Truncates beyond 18 decimals.
pub fn parse_amount(s: &str) -> Result<Amount, CoreError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(CoreError::InvalidAmount(s.to_string()));
    }
    let (whole_str, frac_str) = s.split_once('.').unwrap_or((s, ""));
    let whole: u128 = whole_str
        .parse()
        .map_err(|_| CoreError::InvalidAmount(s.to_string()))?;
    let frac_atoms: u128 = if frac_str.is_empty() {
        0
    } else {
        let truncated = &frac_str[..frac_str.len().min(18)];
        let padded = format!("{:0<18}", truncated);
        padded
            .parse()
            .map_err(|_| CoreError::InvalidAmount(s.to_string()))?
    };
    let atoms = whole
        .checked_mul(DECIMAL_FACTOR)
        .and_then(|w| w.checked_add(frac_atoms))
        .ok_or_else(|| CoreError::InvalidAmount(format!("{s} overflows")))?;
    Ok(Amount::from_atoms(atoms))
}

pub fn parse_address(s: &str) -> Result<Address, CoreError> {
    Address::from_bech32(s.trim()).map_err(|e| CoreError::Address(e.to_string()))
}

// ─── Transaction building (local signing) ─────────────────────────────────────

/// Builds and signs a transaction with an explicit `chain_id`. Built field-by-field
/// (rather than via the constructors, which default to devnet) so the signature
/// commits to the node's real chain id — critical for replay protection.
#[allow(clippy::too_many_arguments)]
fn build_signed(
    kp: &KeyPair,
    tx_type: TransactionType,
    to: Address,
    amount: Amount,
    fee: Amount,
    nonce: u64,
    chain_id: u32,
    payload: Vec<u8>,
) -> Transaction {
    let from = Address::from_public_key(&kp.public_key());
    let mut tx = Transaction {
        tx_type,
        from,
        to,
        amount,
        fee,
        nonce,
        chain_id,
        expires_at_height: None,
        payload,
        pub_key: Some(kp.public_key()),
        signature: None,
        sponsor: None,
        sponsor_pub_key: None,
        sponsor_signature: None,
    };
    tx.sign(kp);
    tx
}

pub fn build_transfer(
    kp: &KeyPair,
    to: &str,
    amount: Amount,
    fee: Amount,
    nonce: u64,
    chain_id: u32,
) -> Result<Transaction, CoreError> {
    let to = parse_address(to)?;
    Ok(build_signed(
        kp,
        TransactionType::Transfer,
        to,
        amount,
        fee,
        nonce,
        chain_id,
        vec![],
    ))
}

pub fn build_stake(
    kp: &KeyPair,
    amount: Amount,
    fee: Amount,
    nonce: u64,
    chain_id: u32,
) -> Transaction {
    let from = Address::from_public_key(&kp.public_key());
    build_signed(
        kp,
        TransactionType::Stake,
        from,
        amount,
        fee,
        nonce,
        chain_id,
        vec![],
    )
}

pub fn build_unstake(
    kp: &KeyPair,
    amount: Amount,
    fee: Amount,
    nonce: u64,
    chain_id: u32,
) -> Transaction {
    let from = Address::from_public_key(&kp.public_key());
    build_signed(
        kp,
        TransactionType::Unstake,
        from,
        amount,
        fee,
        nonce,
        chain_id,
        vec![],
    )
}

/// ADR 0007: validator-set changes go through the single governance path
/// (AdminAction carrying a bincode(GovernanceAction)), not a dedicated tx type.
/// `to` is the admin itself, matching `Transaction::new_admin_action`.
fn build_admin_action(
    kp: &KeyPair,
    action: &GovernanceAction,
    nonce: u64,
    chain_id: u32,
) -> Transaction {
    let from = Address::from_public_key(&kp.public_key());
    let payload = bincode::serialize(action).expect("GovernanceAction serialization is infallible");
    build_signed(
        kp,
        TransactionType::AdminAction,
        from,
        Amount::ZERO,
        Amount::ZERO,
        nonce,
        chain_id,
        payload,
    )
}

pub fn build_add_validator(
    kp: &KeyPair,
    validator: &str,
    nonce: u64,
    chain_id: u32,
) -> Result<Transaction, CoreError> {
    let to = parse_address(validator)?;
    Ok(build_admin_action(
        kp,
        &GovernanceAction::AddValidator(to),
        nonce,
        chain_id,
    ))
}

pub fn build_remove_validator(
    kp: &KeyPair,
    validator: &str,
    nonce: u64,
    chain_id: u32,
) -> Result<Transaction, CoreError> {
    let to = parse_address(validator)?;
    Ok(build_admin_action(
        kp,
        &GovernanceAction::RemoveValidator(to),
        nonce,
        chain_id,
    ))
}

pub fn build_announce_upgrade(
    kp: &KeyPair,
    version: ProtocolVersion,
    activation_ts: u64,
    nonce: u64,
    chain_id: u32,
) -> Transaction {
    let from = Address::from_public_key(&kp.public_key());
    // Payload: major(2) || minor(2) || patch(2) || activation_ts(8) = 14 bytes (ADR 0006).
    let mut payload = Vec::with_capacity(14);
    payload.extend_from_slice(&version.major.to_be_bytes());
    payload.extend_from_slice(&version.minor.to_be_bytes());
    payload.extend_from_slice(&version.patch.to_be_bytes());
    payload.extend_from_slice(&activation_ts.to_be_bytes());
    build_signed(
        kp,
        TransactionType::AnnounceUpgrade,
        from,
        Amount::ZERO,
        Amount::ZERO,
        nonce,
        chain_id,
        payload,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keystore_roundtrip() {
        let (ks, kp) = Keystore::generate();
        assert!(ks.address.starts_with("vinx1"));
        assert_eq!(ks.to_keypair().unwrap().public_key(), kp.public_key());
    }

    #[test]
    fn test_parse_amount() {
        assert_eq!(parse_amount("1").unwrap().atoms(), DECIMAL_FACTOR);
        assert_eq!(parse_amount("0.5").unwrap().atoms(), DECIMAL_FACTOR / 2);
        assert_eq!(
            parse_amount("100.25").unwrap().atoms(),
            100 * DECIMAL_FACTOR + DECIMAL_FACTOR / 4
        );
        assert!(parse_amount("abc").is_err());
        assert!(parse_amount("").is_err());
    }

    #[test]
    fn test_build_transfer_is_valid_and_chain_bound() {
        let (_, kp) = Keystore::generate();
        let (dest_ks, _) = Keystore::generate();
        let tx = build_transfer(
            &kp,
            &dest_ks.address,
            Amount::from_vinx(10),
            Amount::from_atoms(1),
            0,
            7,
        )
        .unwrap();
        assert_eq!(tx.chain_id, 7);
        assert_eq!(tx.tx_type, TransactionType::Transfer);
        // Signature verifies over the canonical signing bytes (incl. chain_id).
        let pk = tx.pub_key.as_ref().unwrap();
        let sig = tx.signature.as_ref().unwrap();
        assert!(pk.verify(&tx.signing_bytes(), sig).is_ok());
    }

    #[test]
    fn test_build_admin_txs() {
        let (_, kp) = Keystore::generate();
        let (v, _) = Keystore::generate();
        let add = build_add_validator(&kp, &v.address, 3, 42).unwrap();
        // ADR 0007: routed through the unified governance path.
        assert_eq!(add.tx_type, TransactionType::AdminAction);
        assert_eq!(add.fee, Amount::ZERO);
        let decoded: GovernanceAction = bincode::deserialize(&add.payload).unwrap();
        assert_eq!(
            decoded,
            GovernanceAction::AddValidator(parse_address(&v.address).unwrap())
        );
        let up = build_announce_upgrade(&kp, ProtocolVersion::new(1, 1, 0), 100_000, 0, 42);
        assert_eq!(up.decode_upgrade_payload().unwrap().1, 100_000);
    }

    #[test]
    fn test_bad_address_rejected() {
        let (_, kp) = Keystore::generate();
        assert!(build_transfer(
            &kp,
            "not-an-address",
            Amount::from_vinx(1),
            Amount::ZERO,
            0,
            42
        )
        .is_err());
    }
}
