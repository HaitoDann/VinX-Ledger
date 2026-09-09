/// Open PoA validator pool types (ADR 0038 + ADR 0036).
///
/// Every bonded address lives in the pool. The *active set* is the top-N pool
/// entries by co-signature score, rotated at each epoch close (ADR 0028).
use serde::{Deserialize, Serialize};
use vinx_crypto::Address;

use crate::amount::VALIDATOR_WARMUP_EPOCHS;

/// An exit request queued when a validator's bond drops below the floor (ADR 0036).
///
/// Requests are processed FIFO — sorted by `(request_height, address)` — at each
/// epoch close, up to `MAX_VALIDATOR_EXITS_PER_EPOCH` per epoch.  The validator's
/// bond remains slashable while the request sits in the queue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidatorExitRequest {
    /// Address of the exiting validator.
    pub address: Address,
    /// Block height at which the exit was requested — primary FIFO sort key.
    pub request_height: u64,
    /// Unbonding unlock timestamp (`current_block_ts + UNBONDING_SECS`) computed at
    /// request time; used when the entry is promoted to `Unbonding`.
    pub unlock_ts: u64,
}

/// Status of a validator in the pool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PoolStatus {
    /// Completing the mandatory warm-up period before being eligible for the
    /// active set. `epochs_remaining` counts down from VALIDATOR_WARMUP_EPOCHS
    /// to 0 at each epoch close.
    Warmup { epochs_remaining: u32 },
    /// In the top-N by score — currently producing and co-signing blocks.
    Active,
    /// Score ranked but below the top-N cutoff. Continues co-signing to improve
    /// its score and may return to Active at the next epoch rotation.
    Benched,
    /// Bond is in the UNBONDING_SECS delay. Not eligible for the active set;
    /// bond remains slashable until the delay expires.
    Unbonding { unlock_ts: u64 },
}

/// A single entry in the validator pool.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidatorPoolEntry {
    /// Bond locked by this validator (atoms). Must stay ≥ MIN_VALIDATOR_BOND_ATOMS.
    pub bond_atoms: u128,
    /// Unix timestamp when the bond was first posted (block timestamp at admission).
    /// Used as the ancienneté tiebreaker when two validators have identical scores.
    pub bonded_since_ts: u64,
    /// Current lifecycle status.
    pub status: PoolStatus,
    /// Cumulative co-signature count within the current score window (7 days).
    /// Reset / pruned at each epoch close when old blocks fall outside the window.
    pub cosign_count_in_window: u64,
    /// Total finalized blocks within the score window where this validator was in
    /// the active set. Used as the denominator of the score.
    pub eligible_blocks_in_window: u64,
    /// BLS12-381 public key (G1 compressed, 48 bytes). None before ADR 0046 registration.
    #[serde(default)]
    pub bls_pub_key: Option<Vec<u8>>,
    /// Proof-of-Possession BLS signature (G2 compressed, 96 bytes). None before ADR 0046.
    #[serde(default)]
    pub bls_pop: Option<Vec<u8>>,
    /// ECVRF public key (compressed Edwards25519, 32 bytes). None before VRF key registration.
    /// Used by the committee-selection VRF (ADR 0029 Phase 2b).
    #[serde(default)]
    pub vrf_pub_key: Option<[u8; 32]>,
}

impl ValidatorPoolEntry {
    /// Create a new pool entry for a freshly bonded validator.
    pub fn new(bond_atoms: u128, now_ts: u64) -> Self {
        Self {
            bond_atoms,
            bonded_since_ts: now_ts,
            status: PoolStatus::Warmup {
                epochs_remaining: VALIDATOR_WARMUP_EPOCHS,
            },
            cosign_count_in_window: 0,
            eligible_blocks_in_window: 0,
            bls_pub_key: None,
            bls_pop: None,
            vrf_pub_key: None,
        }
    }

    /// Score in [0, 10_000] basis points (10_000 = 100 % co-signature rate).
    /// Returns 5_000 (50 %) during warm-up — a sentinel never used in ranking.
    pub fn score_bps(&self) -> u32 {
        match self.status {
            PoolStatus::Warmup { .. } => 5_000,
            _ => {
                if self.eligible_blocks_in_window == 0 {
                    0
                } else {
                    ((self.cosign_count_in_window as u64 * 10_000) / self.eligible_blocks_in_window)
                        as u32
                }
            }
        }
    }

    /// Whether this entry is eligible to be ranked for the active set.
    pub fn is_eligible(&self) -> bool {
        matches!(self.status, PoolStatus::Active | PoolStatus::Benched)
    }

    /// Decrement warm-up counter. Returns `true` if warm-up just completed.
    pub fn tick_warmup(&mut self) -> bool {
        if let PoolStatus::Warmup { epochs_remaining } = &mut self.status {
            if *epochs_remaining <= 1 {
                self.status = PoolStatus::Benched;
                return true;
            }
            *epochs_remaining -= 1;
        }
        false
    }

    /// Sliding-window decay: multiply both counters by `(window_epochs − 1) / window_epochs`.
    ///
    /// Applied at every epoch close so data older than `window_epochs` epochs is
    /// progressively forgotten. After `window_epochs` applications the original value is
    /// reduced to `((w-1)/w)^w ≈ 1/e ≈ 37%` — a natural decay, not a hard reset.
    /// Both numerator and denominator are decayed identically, so the ratio (score) is
    /// stable for a validator whose co-signature rate is constant.
    pub fn decay_window(&mut self, window_epochs: u64) {
        if window_epochs <= 1 {
            return;
        }
        let w = window_epochs;
        self.cosign_count_in_window = self.cosign_count_in_window * (w - 1) / w;
        self.eligible_blocks_in_window = self.eligible_blocks_in_window * (w - 1) / w;
    }

    /// Records a finalized block for this validator.
    ///
    /// `was_eligible`: the validator was in the active set for this block (increments
    /// the denominator). `did_cosign`: the validator's signature appeared in the block
    /// (increments the numerator). Calling with `was_eligible = false` is a no-op.
    pub fn record_block(&mut self, was_eligible: bool, did_cosign: bool) {
        if was_eligible {
            self.eligible_blocks_in_window += 1;
            if did_cosign {
                self.cosign_count_in_window += 1;
            }
        }
    }
}
