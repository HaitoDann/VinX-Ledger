/// VinX chain identifiers — included in every transaction's signing bytes to
/// prevent cross-network replay attacks (analogous to Ethereum EIP-155).
///
/// A transaction signed for one chain ID is cryptographically invalid on any
/// other network, even if the sender's address exists on both.
pub const CHAIN_ID_MAINNET: u32 = 1;
pub const CHAIN_ID_TESTNET: u32 = 7;
pub const CHAIN_ID_DEVNET: u32 = 42;
