//! Per-peer anti-DoS guard (ADR 0022): inbound message rate limiting + reputation/ban.
//!
//! The event loop funnels every inbound gossip message through a [`PeerGuard`], which
//! combines two defenses:
//!
//! - a **token bucket per peer** that caps the sustained inbound message rate (absorbing
//!   legitimate bursts), and
//! - a **reputation score per peer** that is docked for misbehavior (undecipherable or
//!   oversized messages, sustained flooding) and, once it crosses [`BAN_THRESHOLD`],
//!   signals the caller to blacklist the peer at the gossipsub layer.
//!
//! The time source is passed in explicitly (`now: Instant`) so the rate logic is pure and
//! unit-testable without sleeping.

use std::collections::HashMap;
use std::time::Instant;

use libp2p::PeerId;

/// Reputation at or below which a peer is banned (blacklisted at the gossip layer).
pub const BAN_THRESHOLD: i32 = -5;

/// How long a ban lasts. Never permanent: on a small validator set, cutting a validator
/// off for good halts the chain, and an honest node can burst (restart, catch-up).
pub const BAN_DURATION: std::time::Duration = std::time::Duration::from_secs(300);

/// Sustained inbound message budget per peer, in messages per second. Comfortably above
/// any honest steady-state (blocks, co-signatures, the occasional transaction) but far
/// below what a flooding peer would sustain.
pub const PEER_MSG_RATE_PER_SEC: f64 = 50.0;

/// Burst capacity per peer, in messages. Absorbs legitimate bursts — e.g. a sync response
/// followed immediately by a wave of co-signatures — without penalizing.
pub const PEER_MSG_BURST: f64 = 200.0;

/// Reputation dock applied each time a peer overruns its rate budget.
pub const RATE_FLOOD_PENALTY: i32 = 1;

/// Reputation dock for a single undecipherable or oversized message.
pub const BAD_MESSAGE_PENALTY: i32 = 1;

/// A classic token bucket: `rate` tokens accrue per second, capped at `capacity`.
#[derive(Clone, Debug)]
struct TokenBucket {
    tokens: f64,
    capacity: f64,
    rate: f64,
    last: Instant,
}

impl TokenBucket {
    fn new(capacity: f64, rate: f64, now: Instant) -> Self {
        Self {
            tokens: capacity,
            capacity,
            rate,
            last: now,
        }
    }

    /// Refills for the time elapsed since the last call, then tries to spend one token.
    /// Returns `true` if a token was available (message admitted).
    fn try_take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.rate).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Outcome of admitting an inbound message from a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admit {
    /// Process the message normally.
    Allow,
    /// The peer exceeded its rate budget; the message was dropped and the peer penalized.
    /// `ban` is true when the penalty pushed the peer past [`BAN_THRESHOLD`].
    RateLimited { ban: bool },
}

/// Combined per-peer rate limiter and reputation ledger.
#[derive(Default)]
pub struct PeerGuard {
    reputation: HashMap<PeerId, i32>,
    buckets: HashMap<PeerId, TokenBucket>,
    banned: HashMap<PeerId, Instant>,
}

impl PeerGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rate-checks an inbound message from `peer`. On overrun the message is dropped and a
    /// [`RATE_FLOOD_PENALTY`] is applied; the returned `ban` flag tells the caller whether
    /// to blacklist the peer now.
    pub fn admit(&mut self, peer: PeerId, now: Instant) -> Admit {
        let bucket = self
            .buckets
            .entry(peer)
            .or_insert_with(|| TokenBucket::new(PEER_MSG_BURST, PEER_MSG_RATE_PER_SEC, now));
        if bucket.try_take(now) {
            Admit::Allow
        } else {
            let ban = self.penalize(peer, RATE_FLOOD_PENALTY);
            Admit::RateLimited { ban }
        }
    }

    /// Docks `points` from `peer`'s reputation. Returns `true` if the peer is now at or
    /// below [`BAN_THRESHOLD`] and should be blacklisted.
    pub fn penalize(&mut self, peer: PeerId, points: i32) -> bool {
        let score = self.reputation.entry(peer).or_insert(0);
        *score -= points;
        *score <= BAN_THRESHOLD
    }

    /// Records a ban of `peer` from `now`; [`PeerGuard::expired_bans`] lifts it later.
    pub fn ban(&mut self, peer: PeerId, now: Instant) {
        self.banned.insert(peer, now + BAN_DURATION);
    }

    /// Peers whose ban is over: their reputation and rate budget start afresh.
    pub fn expired_bans(&mut self, now: Instant) -> Vec<PeerId> {
        let done: Vec<PeerId> = self
            .banned
            .iter()
            .filter(|(_, until)| **until <= now)
            .map(|(p, _)| *p)
            .collect();
        for p in &done {
            self.banned.remove(p);
            self.reputation.remove(p);
            self.buckets.remove(p);
        }
        done
    }

    /// Drops all per-peer state for a disconnected peer, bounding memory against churn.
    pub fn forget(&mut self, peer: &PeerId) {
        self.reputation.remove(peer);
        self.buckets.remove(peer);
    }

    #[cfg(test)]
    fn score(&self, peer: &PeerId) -> i32 {
        self.reputation.get(peer).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn peer() -> PeerId {
        PeerId::random()
    }

    #[test]
    fn token_bucket_allows_burst_then_throttles() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(3.0, 1.0, t0);
        // Full bucket → first 3 immediate messages pass.
        assert!(b.try_take(t0));
        assert!(b.try_take(t0));
        assert!(b.try_take(t0));
        // Bucket empty → next is throttled.
        assert!(!b.try_take(t0));
    }

    #[test]
    fn token_bucket_refills_over_time() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(2.0, 10.0, t0); // 10 tokens/sec
        assert!(b.try_take(t0));
        assert!(b.try_take(t0));
        assert!(!b.try_take(t0)); // drained
                                  // After 200ms → 2 tokens refilled (10/s * 0.2s).
        let t1 = t0 + Duration::from_millis(200);
        assert!(b.try_take(t1));
        assert!(b.try_take(t1));
        assert!(!b.try_take(t1));
    }

    #[test]
    fn token_bucket_refill_is_capped_at_capacity() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(2.0, 100.0, t0);
        // Idle a long time — tokens must not exceed capacity.
        let t1 = t0 + Duration::from_secs(10);
        assert!(b.try_take(t1));
        assert!(b.try_take(t1));
        assert!(!b.try_take(t1)); // only `capacity` (2) available, not 1000
    }

    #[test]
    fn flooding_peer_is_eventually_banned() {
        let mut g = PeerGuard::new();
        let p = peer();
        let t0 = Instant::now();
        // Burst of 200 passes; everything beyond drains reputation by 1 each.
        let mut banned = false;
        for _ in 0..(PEER_MSG_BURST as usize + BAN_THRESHOLD.unsigned_abs() as usize) {
            if let Admit::RateLimited { ban } = g.admit(p, t0) {
                banned |= ban;
            }
        }
        assert!(banned, "a sustained flood must trip the ban threshold");
        assert!(g.score(&p) <= BAN_THRESHOLD);
    }

    #[test]
    fn honest_peer_within_budget_is_never_penalized() {
        let mut g = PeerGuard::new();
        let p = peer();
        let mut t = Instant::now();
        // One message every 100ms for 100 iterations — well under 50 msg/s.
        for _ in 0..100 {
            assert_eq!(g.admit(p, t), Admit::Allow);
            t += Duration::from_millis(100);
        }
        assert_eq!(g.score(&p), 0);
    }

    #[test]
    fn penalize_crosses_threshold() {
        let mut g = PeerGuard::new();
        let p = peer();
        for _ in 0..(BAN_THRESHOLD.unsigned_abs() - 1) {
            assert!(!g.penalize(p, 1));
        }
        assert!(g.penalize(p, 1)); // fifth strike → banned
    }

    #[test]
    fn bans_are_lifted_after_their_duration() {
        let mut g = PeerGuard::new();
        let p = peer();
        let t0 = Instant::now();
        g.penalize(p, 10);
        g.ban(p, t0);
        assert!(g.expired_bans(t0 + BAN_DURATION / 2).is_empty());
        assert_eq!(g.expired_bans(t0 + BAN_DURATION), vec![p]);
        assert_eq!(
            g.score(&p),
            0,
            "a lifted ban starts from a clean reputation"
        );
        assert!(g.expired_bans(t0 + BAN_DURATION * 2).is_empty());
    }

    #[test]
    fn forget_clears_peer_state() {
        let mut g = PeerGuard::new();
        let p = peer();
        g.penalize(p, 3);
        g.forget(&p);
        assert_eq!(g.score(&p), 0);
    }
}
