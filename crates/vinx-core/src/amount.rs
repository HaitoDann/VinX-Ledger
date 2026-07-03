use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const DECIMALS: u32 = 18;
pub const DECIMAL_FACTOR: u128 = 1_000_000_000_000_000_000; // 10^18

/// Absolute supply cap: 100 billion VinX — immutable by protocol. "The total metal."
///
/// VinX has no burn: the supply is conserved forever. At any block,
/// `circulating_supply + foundry == MAX_SUPPLY_ATOMS`. Value cycles endlessly:
/// fees *melt* back into the Foundry, and staking rewards are *forged* out of it.
pub const MAX_SUPPLY_ATOMS: u128 = 100_000_000_000 * DECIMAL_FACTOR;

/// Genesis allocation forged to the founder account: 1 billion VinX (1% of supply),
/// to bootstrap circulation and seed the first economy.
pub const FOUNDER_ALLOCATION_ATOMS: u128 = 1_000_000_000 * DECIMAL_FACTOR;

/// Genesis reserve sealed in the Foundry: 99 billion VinX (99% of supply).
/// Forged into circulation over time as staking rewards; refilled by melted fees.
pub const FOUNDRY_GENESIS_ATOMS: u128 = 99_000_000_000 * DECIMAL_FACTOR;

/// Transaction fee: 0.05% = 5 / 10_000.
pub const FEE_NUMERATOR: u128 = 5;
pub const FEE_DENOMINATOR: u128 = 10_000;

/// Default fee floor: 0.0001 VinX.
pub const DEFAULT_FEE_FLOOR_ATOMS: u128 = DECIMAL_FACTOR / 10_000;

/// Staking rewards distributed every N blocks (~17 minutes at 10s/block).
pub const STAKING_DISTRIBUTION_INTERVAL: u64 = 100;

/// Warm-up: a stake must be held for at least this many blocks before it earns
/// any reward. Set to one full distribution epoch so a stake must weather an
/// entire epoch. This closes the "just-in-time" staking exploit — staking one
/// block before a distribution, collecting, and unstaking right after now earns
/// nothing. Combined with capital-weighted `stake_since` on top-ups, a large
/// late deposit cannot inherit a tiny early stake's seniority either.
pub const STAKE_WARMUP_BLOCKS: u64 = STAKING_DISTRIBUTION_INTERVAL;

/// Forge rate: fraction of the Foundry forged into staking rewards at each
/// distribution, in basis points (10 = 0.1%). Because forging takes a *fraction*
/// of the Foundry and fees continuously melt back in, the Foundry never empties —
/// the "infinite cycle". Tunable via a protocol upgrade.
pub const FORGE_RATE_BPS: u128 = 10;
pub const FORGE_RATE_DENOM: u128 = 10_000;

/// Minimum amount that can be staked: 1 VinX.
pub const MIN_STAKE_ATOMS: u128 = DECIMAL_FACTOR;

/// Number of recent blocks to retain with full data (header + transactions + signatures).
/// Older blocks are compacted: transactions and signatures are dropped, only the header
/// (height, hashes, validator, state_root) is kept for chain integrity verification.
/// At peak load (1 block/3s) this covers ~3.5 days; at low activity, much longer.
pub const BLOCK_RETENTION_COUNT: u64 = 100_000;

/// Pruning runs every N blocks to amortize the O(n) tx-index rebuild cost.
pub const PRUNE_INTERVAL: u64 = 1_000;

/// Heartbeat block interval when mempool is empty: 1 hour of real time.
/// Guarantees liveness and keeps height-based timers advancing.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 3_600;

/// Batch window after the first transaction arrives before sealing a block.
/// Allows concurrent submissions to be grouped into a single block.
pub const BATCH_WINDOW_MS: u64 = 200;

/// Announcement lead-time minimums by upgrade type.
pub const UPGRADE_NOTICE_PATCH_BLOCKS: u64 = 7 * 24 * 360; //  7 days
pub const UPGRADE_NOTICE_MINOR_BLOCKS: u64 = 30 * 24 * 360; // 30 days
pub const UPGRADE_NOTICE_MAJOR_BLOCKS: u64 = 90 * 24 * 360; // 90 days

/// Internal token amount stored as an integer in the smallest unit (10^-18 VinX).
/// All arithmetic uses checked operations to prevent overflow or underflow.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct Amount(pub(crate) u128);

impl Amount {
    pub const ZERO: Self = Amount(0);
    pub const MAX_SUPPLY: Self = Amount(MAX_SUPPLY_ATOMS);

    pub fn from_vinx(vinx: u64) -> Self {
        Amount((vinx as u128).saturating_mul(DECIMAL_FACTOR))
    }

    pub fn from_atoms(atoms: u128) -> Self {
        Amount(atoms)
    }

    pub fn atoms(self) -> u128 {
        self.0
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Amount)
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Amount)
    }

    pub fn saturating_add(self, other: Self) -> Self {
        Amount(self.0.saturating_add(other.0))
    }

    /// Computes the protocol fee for a transfer of this amount.
    /// Returns the higher of 0.05% of the amount or the floor.
    pub fn calculate_fee(self, floor: Self) -> Self {
        let pct = Amount(self.0 * FEE_NUMERATOR / FEE_DENOMINATOR);
        if pct >= floor {
            pct
        } else {
            floor
        }
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / DECIMAL_FACTOR;
        let sub = self.0 % DECIMAL_FACTOR;
        let cents = sub / (DECIMAL_FACTOR / 100);
        write!(f, "{}.{:02} VINX", whole, cents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_one_vinx_in_atoms() {
        let one = Amount::from_vinx(1);
        assert_eq!(one.atoms(), DECIMAL_FACTOR);
    }

    #[test]
    fn test_max_supply_is_100_billion() {
        let expected = Amount::from_vinx(100_000_000_000);
        assert_eq!(Amount::MAX_SUPPLY, expected);
    }

    #[test]
    fn test_genesis_allocations_sum_to_max_supply() {
        // Founder circulation + Foundry reserve == the full immutable supply.
        let founder = Amount::from_atoms(FOUNDER_ALLOCATION_ATOMS);
        let foundry = Amount::from_atoms(FOUNDRY_GENESIS_ATOMS);
        let total = founder.checked_add(foundry).unwrap();
        assert_eq!(total, Amount::MAX_SUPPLY);
    }

    #[test]
    fn test_checked_add() {
        let a = Amount::from_vinx(100);
        let b = Amount::from_vinx(50);
        assert_eq!(a.checked_add(b), Some(Amount::from_vinx(150)));
    }

    #[test]
    fn test_checked_sub() {
        let a = Amount::from_vinx(100);
        let b = Amount::from_vinx(40);
        assert_eq!(a.checked_sub(b), Some(Amount::from_vinx(60)));
    }

    #[test]
    fn test_checked_sub_underflow() {
        let a = Amount::from_vinx(10);
        let b = Amount::from_vinx(20);
        assert_eq!(a.checked_sub(b), None);
    }

    #[test]
    fn test_checked_add_overflow() {
        let near_max = Amount::from_atoms(u128::MAX - 1);
        assert_eq!(near_max.checked_add(Amount::from_atoms(2)), None);
    }

    #[test]
    fn test_fee_uses_percentage_for_large_amount() {
        // 0.05% of 1000 VinX = 0.5 VinX
        let amount = Amount::from_vinx(1_000);
        let floor = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS); // 0.0001 VinX
        let fee = amount.calculate_fee(floor);
        assert_eq!(fee, Amount::from_atoms(DECIMAL_FACTOR / 2)); // 0.5 VinX
    }

    #[test]
    fn test_fee_uses_floor_for_small_amount() {
        // 0.05% of 0.001 VinX = 0.0000005 VinX < floor of 0.0001 VinX
        let amount = Amount::from_atoms(DECIMAL_FACTOR / 1_000);
        let floor = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let fee = amount.calculate_fee(floor);
        assert_eq!(fee, floor);
    }

    #[test]
    fn test_display_whole_number() {
        assert_eq!(format!("{}", Amount::from_vinx(42)), "42.00 VINX");
    }

    #[test]
    fn test_display_with_cents() {
        let amount = Amount::from_atoms(DECIMAL_FACTOR + DECIMAL_FACTOR / 2);
        assert_eq!(format!("{}", amount), "1.50 VINX");
    }

    #[test]
    fn test_display_zero() {
        assert_eq!(format!("{}", Amount::ZERO), "0.00 VINX");
    }

    #[test]
    fn test_ordering() {
        assert!(Amount::from_vinx(10) > Amount::from_vinx(5));
        assert!(Amount::ZERO < Amount::from_vinx(1));
    }
}
