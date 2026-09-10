use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const DECIMALS: u32 = 18;
pub const DECIMAL_FACTOR: u128 = 1_000_000_000_000_000_000; // 10^18

/// Absolute supply cap: 100 billion VinX — immutable by protocol.
///
/// VinX has no burn and no pre-mine. At genesis `emitted_atoms = 0`; tokens enter
/// circulation only through progressive minting by block producers (work emission).
/// The invariant `circulating + epoch_pot + destroyed == emitted_atoms ≤ MAX_SUPPLY_ATOMS`
/// holds at every block (ADR 0040).
pub const MAX_SUPPLY_ATOMS: u128 = 100_000_000_000 * DECIMAL_FACTOR;

/// Flat base transaction fee: 0.0001 VinX. Charged as an absolute forfait (times the
/// tx-type weight and the congestion multiplier), **independent of the amount moved** —
/// processing a transaction costs the same whether it carries 1 or 1,000,000 VinX.
/// Governable via `UpdateFeeFloor`. 100% of every fee goes to the block producer.
pub const DEFAULT_FEE_FLOOR_ATOMS: u128 = DECIMAL_FACTOR / 10_000;

/// Minimum amount that can be staked in a single transaction: 1 VinX.
pub const MIN_STAKE_ATOMS: u128 = DECIMAL_FACTOR;

/// Existential deposit: minimum non-zero balance a plain account may hold (ADR 0026).
/// 0.001 VinX. An account cannot exist with a balance in `]0, ED[`: a transfer that would
/// leave either party with such a balance is rejected, and an account drained to exactly
/// `0` (with no stake and no pending unbond) is *reaped* — removed from the state, freeing
/// its 60 bytes. This closes the only unbounded term in VinX's storage model (dust-account
/// spam) by making state inflation cost real, immobilized capital.
///
/// Consensus-critical: it enters the state-transition function, so it is a **graved
/// constant**, never governable — pinned by `test_existential_deposit_is_constitutional`.
pub const EXISTENTIAL_DEPOSIT_ATOMS: u128 = DECIMAL_FACTOR / 1_000;

/// Maximum concurrent unbonding entries per account (ADR 0009). Caps the state a single
/// account can create by repeatedly unstaking tiny amounts — a hard bound is a stronger
/// anti-spam than a negligible flat fee would be. An account at the cap must wait for an
/// entry to mature before unstaking again.
pub const MAX_PENDING_UNBONDS_PER_ACCOUNT: usize = 16;

// ─── Emission by work — fair launch ────────────────────────────────────────────

/// Emission half-life (ADR 0040): the cumulative emission reaches half the supply after
/// this many real-time **seconds**. ~20 years (20 × 365 × 24 × 3600 = 630 720 000 s).
/// Measured in seconds, not block height — the block cadence is fixed at 12 s (ADR 0043).
/// Immutable after genesis (ADR 0021). Replaces the former `HALVING_PERIOD_SECS` (8 years).
pub const EMISSION_T_HALF_SECS: u64 = 630_720_000;

/// Tokens emitted over the first half-life (~20 years): half the supply.
/// Each subsequent half-life emits half the previous quota; the geometric series
/// converges to `MAX_SUPPLY_ATOMS` — the entire supply, ever more slowly.
pub const ERA0_EMISSION_ATOMS: u128 = MAX_SUPPLY_ATOMS / 2;

// ─── Module registry (ADR 0010) ─────────────────────────────────────────────────

/// Minimum bond to register a module (1,000 VinX). Skin in the game for a module operator —
/// far below the validator bond (a module secures itself, not the L1) but high enough that
/// registering millions of dust modules is economically absurd (anti-bloat, cf. ADR 0026).
pub const MIN_MODULE_BOND_ATOMS: u128 = 1_000 * DECIMAL_FACTOR;

/// Hard cap on the number of registered modules — bounds the module-registry state a
/// coordinated actor could accumulate. Generous for realistic ecosystems.
pub const MAX_MODULES: usize = 100_000;

// ─── Validator bond & slashing ─────────────────────────────────────────────────

/// Minimum bond required to enter the validator pool (100,000 VinX, governable).
/// The genesis validator is grandfathered. The bond is a security deposit slashed
/// on equivocation — it earns no yield. Governable within [MIN_BOND_HARD_FLOOR,
/// MAX_BOND_HARD_CAP]. Changes limited to ±BOND_STEP_BPS per modification with
/// BOND_COOLDOWN_SECS between modifications (ADR 0038).
pub const MIN_VALIDATOR_BOND_ATOMS: u128 = 100_000 * DECIMAL_FACTOR;

/// Hard floor on the validator bond (ADR 0038). Immutable — governance cannot drop
/// the bond below this even if the governable minimum is set lower.
pub const MIN_BOND_HARD_FLOOR: u128 = 10_000 * DECIMAL_FACTOR;

/// Hard cap on the validator bond (ADR 0038). Immutable — prevents governance from
/// pricing out new validators by inflating the bond requirement.
pub const MAX_BOND_HARD_CAP: u128 = 100_000_000 * DECIMAL_FACTOR;

/// Maximum bond change per governance action, in basis points of the current value
/// (2 500 bps = 25%). Immutable. Limits how fast the bond can be moved in either
/// direction — an attacker controlling governance needs ~37 steps × 7-day cooldown
/// to go from 100 000 VinX to 1 VinX, giving the community time to react.
pub const BOND_STEP_BPS: u128 = 2_500;

/// Minimum real-time gap between two bond governance modifications (7 days). Immutable.
pub const BOND_COOLDOWN_SECS: u64 = 7 * 24 * 3_600;

/// Unbonding delay: withdrawn bond returns to the balance only after this much
/// **real time** (3 days), measured on block timestamps. During this window the
/// funds remain slashable, so a validator cannot equivocate then exit before the
/// evidence lands.
pub const UNBONDING_SECS: u64 = 3 * 24 * 3_600;

// ─── Open PoA — active set (ADR 0038) ─────────────────────────────────────────

/// Default number of validators in the active signing set (ADR 0038). Governable
/// in steps of ACTIVE_SET_STEP with ACTIVE_SET_COOLDOWN_SECS between modifications.
/// No hard upper cap — governance controls the ceiling. Note: above ~100 validators,
/// Ed25519 individual co-signatures stress the gossip layer; BLS aggregation (ADR 0029)
/// is recommended for large committees.
pub const DEFAULT_ACTIVE_SET_SIZE: u32 = 21;

/// Hard floor on the active set size (ADR 0038). Immutable — below 5 the BFT
/// safety threshold (⌈2n/3⌉ = 4) has no tolerance for faults.
/// Activated once the validator pool reaches 5 entries; below that the set equals
/// the pool size.
pub const MIN_ACTIVE_SET_SIZE: u32 = 5;

/// Active-set size changes are limited to this step per governance action (ADR 0038).
/// Prevents an attacker from jumping from 21 to the floor of 5 in a single transaction.
pub const ACTIVE_SET_STEP: u32 = 2;

/// Minimum real-time gap between two active-set-size governance modifications (7 days).
/// Immutable. Combined with ACTIVE_SET_STEP, going from 21 to the floor of 5 takes
/// 8 steps × 7 days = 56 days of sustained governance control (ADR 0038).
pub const ACTIVE_SET_COOLDOWN_SECS: u64 = 7 * 24 * 3_600;

/// Duration of one epoch in real-time seconds (ADR 0028). With a fixed 12s block cadence
/// (ADR 0043/0045), each epoch contains exactly EPOCH_DURATION_SECS / block_time_secs = 300
/// blocks. The epoch close triggers: score rotation, warmup tick, epoch pot distribution.
pub const EPOCH_DURATION_SECS: u64 = 3_600;

/// Sliding window over which the validator reliability score is computed (7 days).
/// score(v) = co_signatures(v) / finalized_blocks_where_v_was_in_active_set
/// over the last VALIDATOR_SCORE_WINDOW_SECS of real time (ADR 0038).
pub const VALIDATOR_SCORE_WINDOW_SECS: u64 = 7 * 24 * 3_600;

/// Number of complete epochs a newly bonded validator must observe and co-sign before
/// being eligible for the active set (ADR 0038). Prevents an unoperational node from
/// displacing a veteran by entering at the 50%-start score sentinel.
pub const VALIDATOR_WARMUP_EPOCHS: u32 = 3;

/// Maximum number of validators promoted from the exit queue to `Unbonding` status
/// at each epoch close (ADR 0036). Rate-limits churn so a coordinated or governance-
/// driven mass-exit cannot drain the active set in a single epoch — each wave costs
/// at least one epoch of delay per `MAX_VALIDATOR_EXITS_PER_EPOCH` batch.
pub const MAX_VALIDATOR_EXITS_PER_EPOCH: usize = 2;

/// Basis-point denominator (10_000 = 100%).
pub const BPS_DENOM: u128 = 10_000;

/// Fraction of the bond destroyed on a proven equivocation (100%).
pub const SLASH_EQUIVOCATION_BPS: u128 = 10_000;

/// Fraction of the slashed amount paid to the reporter as a bounty (10%).
/// The remainder (90%) is redistributed to honest validators via the epoch
/// distribution pot — no tokens are destroyed (ADR 0040, no-burn principle).
pub const SLASH_BOUNTY_BPS: u128 = 1_000;

/// Share of the block emission credited directly to the block proposer (ADR 0028).
/// 20% goes to the producer immediately; 80% goes to the epoch distribution pot
/// and is shared proportionally among co-signers at epoch close.
pub const PROPOSER_SHARE_BPS: u128 = 2_000;

/// How long (in real-time seconds) to keep full block data (header + transactions +
/// signatures). After this window, transactions and signatures are dropped — only the
/// header (height, hashes, validator, state_root) is kept for chain integrity.
/// 90 days ≈ 648 000 blocks at 12 s cadence. Keeps the node light while allowing
/// 3-month transaction history lookups.
pub const TX_RETENTION_SECS: u64 = 90 * 24 * 3_600;

/// Pruning runs every N blocks to amortize the O(n) tx-index rebuild cost.
pub const PRUNE_INTERVAL: u64 = 1_000;

/// Maximum distance a transaction's nonce may run ahead of the sender's account
/// nonce to be admitted into the mempool. Bounds nonce-gap parking (filling the
/// mempool with far-future nonces that can never apply). Larger than the
/// per-address mempool cap, so it never rejects a legitimately queued burst.
pub const MAX_NONCE_AHEAD: u64 = 64;

/// ADR 0002 — profondeur maximale de blocs **non finalisés** qu'un producteur empile
/// au-dessus de `finalized_height`. Au-delà, la production **s'arrête** (« refus de bâtir
/// dans le vide ») : borne la longueur des forks concurrents et la fenêtre où opère le
/// fork-choice (ADR 0031). À n=1 la finalité est immédiate (chaque bloc finalise) → jamais
/// atteint ; ne se déclenche que si la finalité est **réellement bloquée** (quorum
/// inatteignable, p. ex. trop de validateurs hors-ligne). Généreux pour absorber des
/// retards transitoires de co-signatures sans stopper une chaîne saine.
pub const MAX_UNFINALIZED_DEPTH: u64 = 64;

/// Batch window after the first transaction arrives before sealing a block.
/// Allows concurrent submissions to be grouped into a single block.
pub const BATCH_WINDOW_MS: u64 = 200;

/// Maximum a block timestamp may lead the receiving node's clock before the block is
/// rejected (ADR 0005). Accommodates honest clock skew while capping the emission-time
/// manipulation a producer could attempt with a bogus (future) clock.
/// Hard ceiling on a transaction's `payload`, in bytes (VINX-13).
///
/// `payload` was unbounded while the fee is computed from `amount` alone, so a
/// zero-value transaction carrying megabytes of payload cost the fee floor and nothing
/// more. Filling blocks and every peer's mempool with such transactions is cheap block
/// bloat and a cheap way to crowd out real traffic.
///
/// 16 KiB comfortably covers every payload the protocol defines — the largest are
/// governance actions and BLS registrations, all well under 1 KiB.
pub const MAX_TX_PAYLOAD_BYTES: usize = 16 * 1024;

pub const MAX_CLOCK_DRIFT_SECS: u64 = 120;

/// Grace period a scheduled leader gets before a backup proposal counts as a missed
/// proposal against it (ADR 0027, VINX-06).
///
/// Jailing used to be charged whenever the actual proposer differed from the scheduled
/// leader, with no timing condition at all — and the P2P path accepts a block from any
/// set member, with no slot deadline. A single validator that simply proposed *first* at
/// every height therefore charged a miss to every honest leader in turn and jailed the
/// entire honest set after three rounds.
///
/// A miss is only real if the leader actually had its turn and did not take it, so the
/// charge now requires the block to arrive at least this long after the previous one.
/// Derived from the 12 s target cadence plus the clock-drift allowance, so an honest
/// backup stepping in for a genuinely absent leader still charges the miss, while a
/// pre-emptive proposal at normal cadence does not.
pub const SLOT_TIMEOUT_SECS: u64 = 3 * 12 + MAX_CLOCK_DRIFT_SECS;

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

/// Cumulative VinX (in atoms) emitted `elapsed_secs` after the emission epoch
/// (the first block's timestamp).
///
/// Approximates the continuous exponential decay `R(t) = R₀·e^(−λt)` using a
/// geometric series with linear interpolation within each half-life period.
/// Over each [`EMISSION_T_HALF_SECS`] window, half the remaining quota is emitted
/// linearly; the series converges to [`MAX_SUPPLY_ATOMS`]. Pure integer arithmetic —
/// deterministic across all platforms (no floating point), as consensus requires.
pub fn cumulative_emission_atoms(elapsed_secs: u64) -> u128 {
    let h = EMISSION_T_HALF_SECS as u128;
    let full_eras = elapsed_secs / EMISSION_T_HALF_SECS;
    let rem = (elapsed_secs % EMISSION_T_HALF_SECS) as u128;
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
    fn test_emission_epoch_start_is_zero() {
        assert_eq!(cumulative_emission_atoms(0), 0);
    }

    #[test]
    fn test_emission_first_half_life_is_half_supply() {
        // After one full ~20-year half-life, exactly half the supply has been emitted.
        assert_eq!(
            cumulative_emission_atoms(EMISSION_T_HALF_SECS),
            ERA0_EMISSION_ATOMS
        );
        assert_eq!(
            cumulative_emission_atoms(EMISSION_T_HALF_SECS),
            MAX_SUPPLY_ATOMS / 2
        );
    }

    #[test]
    fn test_emission_halves_each_period() {
        // T₁ → 50 Md, T₂ → +25 Md (75 Md total), T₃ → +12.5 Md (87.5 Md).
        let one = cumulative_emission_atoms(EMISSION_T_HALF_SECS);
        let two = cumulative_emission_atoms(2 * EMISSION_T_HALF_SECS);
        let three = cumulative_emission_atoms(3 * EMISSION_T_HALF_SECS);
        assert_eq!(two - one, ERA0_EMISSION_ATOMS / 2);
        assert_eq!(three - two, ERA0_EMISSION_ATOMS / 4);
    }

    #[test]
    fn test_emission_schedule_is_constitutional() {
        // ADR 0021 + ADR 0040: these constants ARE VinX's monetary policy — immutable
        // after genesis, not governable by anyone. Changing any of them is a deliberate,
        // breaking act; this tripwire forces it to be conscious.
        assert_eq!(
            EMISSION_T_HALF_SECS, 630_720_000,
            "emission half-life is ~20 years — immutable (ADR 0040)"
        );
        assert_eq!(
            ERA0_EMISSION_ATOMS,
            MAX_SUPPLY_ATOMS / 2,
            "first half-life emits half the supply"
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
        for years in 0..=200 {
            // one-year steps using the ~20-year half-life
            let t = years * (EMISSION_T_HALF_SECS / 20);
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
    fn test_existential_deposit_is_constitutional() {
        // ADR 0026: ED enters the state-transition function — two nodes with different
        // values diverge — so it is a graved constant, never governable. This tripwire
        // forces any change to be a conscious, breaking act.
        assert_eq!(
            EXISTENTIAL_DEPOSIT_ATOMS,
            DECIMAL_FACTOR / 1_000,
            "existential deposit is 0.001 VinX — graved, non-governable"
        );
        // Sanity: ED must be negligible for a user yet far below the validator bond, so a
        // staked account is never dust (the `staked > 0` exemption stays coherent).
        // Compile-time assertions — these relationships between graved constants must hold.
        const _: () = assert!(EXISTENTIAL_DEPOSIT_ATOMS < MIN_STAKE_ATOMS);
        const _: () = assert!(EXISTENTIAL_DEPOSIT_ATOMS < MIN_VALIDATOR_BOND_ATOMS);
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
