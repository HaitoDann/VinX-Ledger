use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const DECIMALS: u32 = 18;
pub const DECIMAL_FACTOR: u128 = 1_000_000_000_000_000_000; // 10^18

/// Absolute supply cap: 100 billion VinX — immutable by protocol.
///
/// VinX has no burn and no pre-mine: the supply is conserved forever. At any block,
/// `circulating_supply + foundry == MAX_SUPPLY_ATOMS`. It all starts in the Foundry and
/// enters circulation only through work emission (block production).
pub const MAX_SUPPLY_ATOMS: u128 = 100_000_000_000 * DECIMAL_FACTOR;

/// Genesis reserve sealed in the Foundry: the **entire** supply (100 billion VinX).
///
/// VinX is a **fair launch**: there is no pre-mine and no founder allocation. At
/// genesis, circulation is zero and 100% of the supply sits in the Foundry — the
/// emission reserve. Tokens enter circulation *only* by rewarding the work of block
/// producers (see [`cumulative_emission_atoms`]).
pub const FOUNDRY_GENESIS_ATOMS: u128 = MAX_SUPPLY_ATOMS;

/// Flat base transaction fee: 0.0001 VinX. Charged as an absolute forfait (times the
/// tx-type weight and the congestion multiplier), **independent of the amount moved** —
/// processing a transaction costs the same whether it carries 1 or 1,000,000 VinX.
/// Governable via `UpdateFeeFloor`. 100% of every fee goes to the block producer.
pub const DEFAULT_FEE_FLOOR_ATOMS: u128 = DECIMAL_FACTOR / 10_000;

/// Minimum amount that can be staked in a single transaction: 1 VinX.
pub const MIN_STAKE_ATOMS: u128 = DECIMAL_FACTOR;

/// Maximum concurrent unbonding entries per account (ADR 0009). Caps the state a single
/// account can create by repeatedly unstaking tiny amounts — a hard bound is a stronger
/// anti-spam than a negligible flat fee would be. An account at the cap must wait for an
/// entry to mature before unstaking again.
pub const MAX_PENDING_UNBONDS_PER_ACCOUNT: usize = 16;

// ─── Emission by work — fair launch ────────────────────────────────────────────

/// Halving period: the emission rate is divided by two every 8 real-time years.
/// Measured in **seconds** (block timestamps), not block height, because the block
/// cadence is demand-adaptive and height is not a clock. 8 × 365.25 × 24 × 3600.
pub const HALVING_PERIOD_SECS: u64 = 252_460_800;

/// Total to emit over the first halving era (8 years): half of the supply.
/// Each subsequent era emits half the previous one, so the sum over all eras is
/// exactly `MAX_SUPPLY_ATOMS` — the whole supply is emitted, ever more slowly.
pub const ERA0_EMISSION_ATOMS: u128 = MAX_SUPPLY_ATOMS / 2;

// ─── Validator bond & slashing ─────────────────────────────────────────────────

/// Minimum bond required to be admitted to the validator set (100,000 VinX).
/// The genesis validator is grandfathered (it bootstraps with no balance). The bond
/// is a security deposit — skin in the game slashed on equivocation — it earns no
/// yield. Governable via governance.
pub const MIN_VALIDATOR_BOND_ATOMS: u128 = 100_000 * DECIMAL_FACTOR;

/// Unbonding delay: withdrawn bond returns to the balance only after this much
/// **real time** (3 days), measured on block timestamps. During this window the
/// funds remain slashable, so a validator cannot equivocate then exit before the
/// evidence lands.
pub const UNBONDING_SECS: u64 = 3 * 24 * 3_600;

/// Basis-point denominator (10_000 = 100%).
pub const BPS_DENOM: u128 = 10_000;

/// Fraction of the bond destroyed on a proven equivocation (100%).
pub const SLASH_EQUIVOCATION_BPS: u128 = 10_000;

/// Fraction of the slashed amount paid to the reporter as a bounty (10%).
/// The remainder melts back into the Foundry.
pub const SLASH_BOUNTY_BPS: u128 = 1_000;

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

/// Maximum a block timestamp may lead the receiving node's clock before the block is
/// rejected (ADR 0005). Accommodates honest clock skew while capping the emission-time
/// manipulation a producer could attempt with a bogus (future) clock.
pub const MAX_CLOCK_DRIFT_SECS: u64 = 120;

/// Window (number of recent blocks) for the Median Time Past (ADR 0005): a single
/// producer cannot make the network's time reference jump because it is a median.
pub const MEDIAN_TIME_BLOCKS: usize = 11;

/// Announcement lead-time minimums by upgrade type, in **real seconds** (ADR 0006).
/// Block height is not a clock (adaptive cadence), so the upgrade notice window is
/// measured against block timestamps — consistent with emission and unbonding.
pub const UPGRADE_NOTICE_PATCH_SECS: u64 = 7 * 24 * 3600; //  7 days
pub const UPGRADE_NOTICE_MINOR_SECS: u64 = 30 * 24 * 3600; // 30 days
pub const UPGRADE_NOTICE_MAJOR_SECS: u64 = 90 * 24 * 3600; // 90 days

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

    /// Protocol fee for a standard transaction: a **flat forfait**, independent of
    /// the amount moved. `base_fee` already folds in the congestion multiplier
    /// (see `WorldState::update_base_fee`); the tx-type weight for a transfer is 1.
    /// The receiver of `self` (the amount) is ignored on purpose — a payment rail
    /// prices by resource consumed, not by value transported.
    pub fn calculate_fee(self, base_fee: Self) -> Self {
        base_fee
    }
}

/// Cumulative VinX (in atoms) that should have been emitted `elapsed_secs` after the
/// emission epoch (the first block's timestamp).
///
/// Emission follows a discrete **halving** schedule: era `e` lasts
/// [`HALVING_PERIOD_SECS`] and emits `ERA0_EMISSION_ATOMS >> e` linearly across the
/// era; each era emits half the previous one, so the sum over all eras converges to
/// [`MAX_SUPPLY_ATOMS`]. The computation is **pure integer arithmetic** — fully
/// deterministic across platforms (no floating point), which consensus requires.
pub fn cumulative_emission_atoms(elapsed_secs: u64) -> u128 {
    let h = HALVING_PERIOD_SECS as u128;
    let full_eras = elapsed_secs / HALVING_PERIOD_SECS;
    let rem = (elapsed_secs % HALVING_PERIOD_SECS) as u128;
    let mut total: u128 = 0;
    let mut era_amount = ERA0_EMISSION_ATOMS;
    for _ in 0..full_eras {
        total = total.saturating_add(era_amount);
        era_amount /= 2;
        if era_amount == 0 {
            return total; // schedule exhausted (dust) — nothing more to emit, ever
        }
    }
    // Linear share of the current (partial) era. No overflow: era_amount ≤ 5e28,
    // rem < h ≈ 2.5e8, product ≤ 1.25e37 < u128::MAX.
    total.saturating_add(era_amount * rem / h)
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
    fn test_genesis_foundry_holds_entire_supply() {
        // Fair launch: no pre-mine — the whole supply starts in the Foundry.
        assert_eq!(FOUNDRY_GENESIS_ATOMS, MAX_SUPPLY_ATOMS);
    }

    #[test]
    fn test_emission_epoch_start_is_zero() {
        assert_eq!(cumulative_emission_atoms(0), 0);
    }

    #[test]
    fn test_emission_first_era_is_half_supply() {
        // After one full 8-year era, exactly half the supply has been emitted.
        assert_eq!(
            cumulative_emission_atoms(HALVING_PERIOD_SECS),
            ERA0_EMISSION_ATOMS
        );
        assert_eq!(
            cumulative_emission_atoms(HALVING_PERIOD_SECS),
            MAX_SUPPLY_ATOMS / 2
        );
    }

    #[test]
    fn test_emission_halves_each_era() {
        // Era 0 → 50 Md, era 1 → +25 Md (75 Md total), era 2 → +12.5 Md (87.5 Md).
        let one = cumulative_emission_atoms(HALVING_PERIOD_SECS);
        let two = cumulative_emission_atoms(2 * HALVING_PERIOD_SECS);
        let three = cumulative_emission_atoms(3 * HALVING_PERIOD_SECS);
        assert_eq!(two - one, ERA0_EMISSION_ATOMS / 2);
        assert_eq!(three - two, ERA0_EMISSION_ATOMS / 4);
    }

    #[test]
    fn test_emission_schedule_is_constitutional() {
        // ADR 0021: these constants ARE VinX's monetary policy and are immutable —
        // not governable by anyone. Changing any of them is a deliberate, breaking act,
        // and this test is the tripwire that forces it to be conscious.
        assert_eq!(
            HALVING_PERIOD_SECS, 252_460_800,
            "halving period is 8 years — immutable"
        );
        assert_eq!(
            ERA0_EMISSION_ATOMS,
            MAX_SUPPLY_ATOMS / 2,
            "era 0 emits half the supply"
        );
        assert_eq!(
            MAX_SUPPLY_ATOMS,
            100_000_000_000 * DECIMAL_FACTOR,
            "100 Md cap — immutable"
        );
        // The schedule asymptotically emits the entire supply (minus integer dust).
        let far_future = cumulative_emission_atoms(u64::MAX / 2);
        assert!(far_future <= MAX_SUPPLY_ATOMS);
        assert!(
            far_future > MAX_SUPPLY_ATOMS - MAX_SUPPLY_ATOMS / 1_000_000,
            "emission converges to the full supply"
        );
    }

    #[test]
    fn test_emission_monotonic_and_bounded() {
        let mut prev = 0u128;
        for years in 0..=120 {
            let t = years * (HALVING_PERIOD_SECS / 8); // one-year steps
            let e = cumulative_emission_atoms(t);
            assert!(e >= prev, "emission must be non-decreasing");
            assert!(
                e <= MAX_SUPPLY_ATOMS,
                "emission never exceeds the supply cap"
            );
            prev = e;
        }
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
    fn test_fee_is_flat_regardless_of_amount() {
        // The flat forfait is independent of the amount moved: sending 0.001 VinX
        // or 1000 VinX costs exactly the base fee.
        let base = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS); // 0.0001 VinX
        assert_eq!(Amount::from_vinx(1_000).calculate_fee(base), base);
        assert_eq!(
            Amount::from_atoms(DECIMAL_FACTOR / 1_000).calculate_fee(base),
            base
        );
        assert_eq!(Amount::ZERO.calculate_fee(base), base);
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
