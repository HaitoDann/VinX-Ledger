use serde::{Deserialize, Serialize};
use std::fmt;

pub const DECIMALS: u32 = 18;
pub const DECIMAL_FACTOR: u128 = 1_000_000_000_000_000_000; // 10^18

/// Absolute supply cap: 100 billion VinX — immutable by protocol.
pub const MAX_SUPPLY_ATOMS: u128 = 100_000_000_000 * DECIMAL_FACTOR;
/// Genesis allocation to VinX Labs admin account: 21 billion VinX (Sandbox Phase 1).
pub const ADMIN_ALLOCATION_ATOMS: u128 = 21_000_000_000 * DECIMAL_FACTOR;
/// Coffre Maturité: 79 billion VinX locked until 3 conditions are met (MiCA CASP, audit, public policy).
pub const COFFRE_MATURITY_ATOMS: u128 = 79_000_000_000 * DECIMAL_FACTOR;

/// Transaction fee: 0.05% = 5 / 10_000.
pub const FEE_NUMERATOR: u128 = 5;
pub const FEE_DENOMINATOR: u128 = 10_000;

/// Default fee floor: 0.0001 VinX.
pub const DEFAULT_FEE_FLOOR_ATOMS: u128 = DECIMAL_FACTOR / 10_000;

/// Fee split (basis points): 40% staking / 30% validators / 30% melt (redistribution pool).
pub const STAKING_FEE_BPS: u128 = 4_000;
pub const VALIDATOR_FEE_BPS: u128 = 3_000;
// melt share = remainder (ensures no rounding loss)
const FEE_BPS_DENOM: u128 = 10_000;

/// Staking rewards distributed every N blocks (~17 minutes at 10s/block).
pub const STAKING_DISTRIBUTION_INTERVAL: u64 = 100;

/// Minimum amount that can be staked: 1 VinX.
pub const MIN_STAKE_ATOMS: u128 = DECIMAL_FACTOR;

/// Judicial freeze duration before mandatory auto-unfreeze: 365 days at 10 s/block.
pub const FREEZE_DURATION_BLOCKS: u64 = 365 * 24 * 360; // 3_153_600 blocks ≈ 1 year

/// Announcement lead-time minimums by upgrade type.
pub const UPGRADE_NOTICE_PATCH_BLOCKS: u64 = 7 * 24 * 360;  //  7 days
pub const UPGRADE_NOTICE_MINOR_BLOCKS: u64 = 30 * 24 * 360; // 30 days
pub const UPGRADE_NOTICE_MAJOR_BLOCKS: u64 = 90 * 24 * 360; // 90 days

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

    /// 40% of a fee amount — goes to the staking pool.
    pub fn staking_share(fee: Self) -> Self {
        Amount(fee.0 * STAKING_FEE_BPS / FEE_BPS_DENOM)
    }

    /// 30% of a fee amount — goes to the block's validator as reward.
    pub fn validator_share(fee: Self) -> Self {
        Amount(fee.0 * VALIDATOR_FEE_BPS / FEE_BPS_DENOM)
    }

    /// 30% of a fee amount — goes to the melt pool (redistribution reserve, not a burn).
    pub fn melt_share(fee: Self) -> Self {
        let s = Self::staking_share(fee);
        let v = Self::validator_share(fee);
        fee.checked_sub(s).unwrap_or(Self::ZERO)
           .checked_sub(v).unwrap_or(Self::ZERO)
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
    fn test_allocations_sum_to_max_supply() {
        let admin = Amount::from_atoms(ADMIN_ALLOCATION_ATOMS);
        let coffre = Amount::from_atoms(COFFRE_MATURITY_ATOMS);
        let total = admin.checked_add(coffre).unwrap();
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
    fn test_fee_split_40_30_30() {
        let fee = Amount::from_vinx(100);
        let staking = Amount::staking_share(fee);
        let validator = Amount::validator_share(fee);
        let melt = Amount::melt_share(fee);
        assert_eq!(staking, Amount::from_vinx(40));
        assert_eq!(validator, Amount::from_vinx(30));
        assert_eq!(melt, Amount::from_vinx(30));
        // All three parts sum to the whole fee
        assert_eq!(staking.checked_add(validator).unwrap().checked_add(melt).unwrap(), fee);
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
