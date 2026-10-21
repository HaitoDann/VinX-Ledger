//! Comptes (SPEC §3).

use crate::Amount;
use borsh::{BorshDeserialize, BorshSerialize};
use vinx_crypto::PublicKey;

#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Account {
    pub balance: Amount,
    pub nonce: u64,
    /// Connue après la première transaction signée du compte.
    pub key: Option<PublicKey>,
    pub memo_required: bool,
}
