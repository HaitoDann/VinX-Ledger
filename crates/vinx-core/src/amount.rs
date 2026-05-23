use serde::{Deserialize, Serialize};
use std::fmt;

pub const DECIMALS: u32 = 18;
pub const DECIMAL_FACTOR: u128 = 1_000_000_000_000_000_000; // 10^18

/// Absolute supply cap: 100 billion VinX — immutable by protocol.
pub const MAX_SUPPLY_ATOMS: u128 = 100_000_000_000 * DECIMAL_FACTOR;
/// Genesis allocation to VinX Labs admin account: 500 million VinX.
pub const ADMIN_ALLOCATION_ATOMS: u128 = 500_000_000 * DECIMAL_FACTOR;
/// Protocol reserve for linear emission over 10 years: 99.5 billion VinX.
pub const RESERVE_ALLOCATION_ATOMS: u128 = 99_500_000_000 * DECIMAL_FACTOR;

/// 10 years × 365 days × 24h × 60min × 6 blocks/min (10s block time).
pub const EMISSION_TOTAL_BLOCKS: u64 = 31_536_000;
/// Atoms emitted per block (integer division; the last block emits the remainder).
pub const EMISSION_PER_BLOCK_ATOMS: u128 =
    RESERVE_ALLOCATION_ATOMS / EMISSION_TOTAL_BLOCKS as u128;

/// Transaction fee: 0.05% = 5 / 10_000.
pub const FEE_NUMERATOR: u128 = 5;
pub const FEE_DENOMINATOR: u128 = 10_000;

/// Default fee floor: 0.01 VinX.
pub const DEFAULT_FEE_FLOOR_ATOMS: u128 = DECIMAL_FACTOR / 100;

/// Fee split: 80% to staking pool, 20% to VinX Labs treasury.
pub const STAKING_SHARE_NUMERATOR: u128 = 80;
pub const STAKING_SHARE_DENOMINATOR: u128 = 100;

/// Internal token amount stored as an integer in the smallest unit (10^-18 VinX).
/// All arithmetic uses checked operations to prevent overflow or underflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
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

    /// 80% of a fee amount — goes to the staking pool.
    pub fn staking_share(fee: Self) -> Self {
        Amount(fee.0 * STAKING_SHARE_NUMERATOR / STAKING_SHARE_DENOMINATOR)
    }

    /// 20% of a fee amount — goes to VinX Labs treasury.
    pub fn treasury_share(fee: Self) -> Self {
        let staking = Self::staking_share(fee);
        fee.checked_sub(staking).unwrap_or(Self::ZERO)
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / DECIMAL_FACTOR;
        // Extract the first 2 displayed decimal digits
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
    fn test_allocations_sum_to_max_supply() {
        let admin = Amount::from_atoms(ADMIN_ALLOCATION_ATOMS);
        let reserve = Amount::from_atoms(RESERVE_ALLOCATION_ATOMS);
        let total = admin.checked_add(reserve).unwrap();
        assert_eq!(total, Amount::MAX_SUPPLY);
    }

    #[test]
    fn test_emission_per_block_atoms() {
        // ~3155 VinX per block
        let per_block = Amount::from_atoms(EMISSION_PER_BLOCK_ATOMS);
        assert!(per_block > Amount::from_vinx(3_100));
        assert!(per_block < Amount::from_vinx(3_200));
    }

    #[test]
    fn test_emission_total_within_reserve() {
        let total_emitted = EMISSION_PER_BLOCK_ATOMS * EMISSION_TOTAL_BLOCKS as u128;
        // Must not exceed reserve (integer division floors it)
        assert!(total_emitted <= RESERVE_ALLOCATION_ATOMS);
        // Must be very close (within one block emission of the reserve)
        let remainder = RESERVE_ALLOCATION_ATOMS - total_emitted;
        assert!(remainder < EMISSION_PER_BLOCK_ATOMS);
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
        let floor = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS); // 0.01 VinX
        let fee = amount.calculate_fee(floor);
        assert_eq!(fee, Amount::from_atoms(DECIMAL_FACTOR / 2)); // 0.5 VinX
    }

    #[test]
    fn test_fee_uses_floor_for_small_amount() {
        // 0.05% of 0.001 VinX = 0.0000005 VinX < floor of 0.01 VinX
        let amount = Amount::from_atoms(DECIMAL_FACTOR / 1_000);
        let floor = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let fee = amount.calculate_fee(floor);
        assert_eq!(fee, floor);
    }

    #[test]
    fn test_fee_split_80_20() {
        let fee = Amount::from_vinx(100);
        let staking = Amount::staking_share(fee);
        let treasury = Amount::treasury_share(fee);
        assert_eq!(staking, Amount::from_vinx(80));
        assert_eq!(treasury, Amount::from_vinx(20));
        // No atoms lost in the split
        assert_eq!(staking.checked_add(treasury).unwrap(), fee);
    }

    #[test]
    fn test_display_whole_number() {
        assert_eq!(format!("{}", Amount::from_vinx(42)), "42.00 VINX");
    }

    #[test]
    fn test_display_with_cents() {
        // 1.5 VinX = 1.5 * 10^18 atoms
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
