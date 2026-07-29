/// VinX chain identifiers — included in every transaction's signing bytes to
/// prevent cross-network replay attacks (analogous to Ethereum EIP-155).
///
/// A transaction signed for one chain ID is cryptographically invalid on any
/// other network, even if the sender's address exists on both.
/// Reserved "no chain" sentinel (ADR 0008). A transaction that arrives without an
/// explicit `chain_id` deserializes to this value, which never matches any real chain
/// ID and is therefore always rejected — no more silent fallback to devnet.
pub const CHAIN_ID_INVALID: u32 = 0;

pub const CHAIN_ID_MAINNET: u32 = 1;
pub const CHAIN_ID_TESTNET: u32 = 7;
pub const CHAIN_ID_DEVNET: u32 = 42;

/// True for the reserved chain IDs the protocol knows about (not the invalid sentinel).
pub fn is_known_chain_id(id: u32) -> bool {
    matches!(id, CHAIN_ID_MAINNET | CHAIN_ID_TESTNET | CHAIN_ID_DEVNET)
}
