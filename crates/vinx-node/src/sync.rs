//! Startup chain-sync: fetches blocks from a trusted peer and replays them.

use rayon::prelude::*;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};
use vinx_core::Block;
use vinx_state::WorldState;

use crate::chain::Chain;
use crate::consensus::validate_block;

#[derive(Deserialize)]
struct SyncResponse {
    count: usize,
    blocks: Vec<Block>,
}

/// Syncs the local chain and state from `peer_rpc_url` starting at the local
/// tip height + 1.  Returns the number of blocks applied.
///
/// The peer is only trusted as a *source* of blocks, never for their validity:
/// every block is fully validated before being applied, exactly like a block
/// received over P2P gossip —
/// - hash-chain linkage (`prev_hash` must match our tip),
/// - timestamp bounds (monotonic, not beyond local clock + max drift — ADR 0005),
/// - proposer membership and quorum co-signatures, cryptographically verified
///   against the validator set *as of that height* (the set evolves as
///   governance transactions are replayed),
/// - transaction signatures (verified in parallel, ADR 0015),
/// - post-apply `state_root` match and supply invariant (ADR 0004).
///
/// The state transition of each block is applied against a snapshot: any
/// failure rolls the world state back and aborts the sync, so a bad block can
/// never leave the node with a half-applied state.
///
/// The function stops on the first validation failure and returns the count
/// applied so far. Hitting a not-yet-finalized block at the peer's tip is a
/// normal stop condition (its co-signatures arrive later over P2P).
pub async fn sync_from_peer(
    peer_rpc_url: &str,
    state: &mut WorldState,
    chain: &mut Chain,
) -> usize {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    let batch = 200usize;
    let mut applied = 0usize;

    loop {
        let from = chain.tip_height() + 1;
        let url = format!(
            "{}/chain/sync?from={}&limit={}",
            peer_rpc_url.trim_end_matches('/'),
            from,
            batch
        );

        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "Sync request failed");
                break;
            }
        };

        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "Sync returned non-200");
            break;
        }

        let sync: SyncResponse = match resp.json().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse sync response");
                break;
            }
        };

        if sync.count == 0 {
            break; // fully caught up
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        for block in sync.blocks {
            // Verify block links to current tip
            let expected_prev = chain.tip_hash();
            if block.header.prev_hash != expected_prev {
                tracing::error!(
                    height = block.header.height,
                    "Sync block has wrong prev_hash — aborting sync"
                );
                return applied;
            }

            // ADR 0005: timestamp bounds — same rules as the P2P path. Monotonicity
            // protects the emission/unbonding clock; the drift cap rejects blocks
            // "from the future" that would let a producer over-forge emission.
            if block.header.timestamp <= chain.tip_timestamp() {
                tracing::error!(
                    height = block.header.height,
                    "Sync block timestamp not monotonic — aborting"
                );
                return applied;
            }
            if block.header.timestamp > now.saturating_add(vinx_core::amount::MAX_CLOCK_DRIFT_SECS)
            {
                tracing::error!(
                    height = block.header.height,
                    "Sync block timestamp too far in the future — aborting"
                );
                return applied;
            }

            // Proposer membership + quorum co-signatures, cryptographically verified
            // (`Block::is_finalized` checks each signature) against the validator set
            // as of this height — `state.validator_set` evolves as blocks are replayed.
            if let Err(e) = validate_block(&block, &state.validator_set) {
                tracing::warn!(
                    height = block.header.height,
                    error = %e,
                    "Sync block not finalized/valid — stopping here (P2P will catch up)"
                );
                return applied;
            }

            // ADR 0015: verify all transaction signatures in parallel (Ed25519 is the
            // dominant cost of replaying a synced block), then apply state sequentially
            // via the trusted path. Same security posture, off the sequential critical path.
            if !block
                .transactions
                .par_iter()
                .all(|tx| WorldState::verify_tx_signature_pure(tx).is_ok())
            {
                tracing::error!(
                    height = block.header.height,
                    "Sync block has invalid transaction signature(s) — aborting"
                );
                return applied;
            }

            // ADR 0002/0027 — quorum historique : capturer le quorum du set ACTIF à cette
            // hauteur (state.validator_set est le set pré-bloc, replay ordonné) avant que ce
            // bloc ne le modifie éventuellement.
            let pre_quorum = state.validator_set.quorum();
            // Apply the state transition against a snapshot so any failure below
            // rolls back instead of leaving a half-applied world state. Protocol
            // time = MTP including this block (ADR 0005), same as the producer.
            let protocol_ts = chain.median_time_past_with(block.header.timestamp);
            let snapshot = state.clone();
            state.set_block_context(protocol_ts);
            for tx in &block.transactions {
                if let Err(e) = state.apply_transaction_trusted(tx) {
                    tracing::error!(error = %e, height = block.header.height, "Sync tx failed — aborting");
                    *state = snapshot;
                    return applied;
                }
            }
            state.block_height = block.header.height;
            state.check_upgrade_activation();
            let _ = state.settle_block(&block.header.validator, block.header.height, protocol_ts);
            if !state.supply_invariant_holds() {
                tracing::error!(
                    height = block.header.height,
                    "Sync block breaks supply invariant — aborting"
                );
                *state = snapshot;
                return applied;
            }
            // The header's state_root commits to the exact post-block account state:
            // a mismatch means the peer served us a block from a different history.
            let root = state.compute_state_root();
            if root != block.header.state_root {
                tracing::error!(
                    height = block.header.height,
                    "Sync block state_root mismatch — aborting"
                );
                *state = snapshot;
                return applied;
            }

            let bh = block.header.height;
            chain.push(block);
            chain.note_quorum(bh, pre_quorum); // ADR 0002/0027 — quorum historique
            applied += 1;
        }

        // ADR 0002 — faire suivre le pointeur de finalité local : les blocs synchronisés
        // portent déjà les co-signatures du pair ; sans cet appel, `finalized_height` reste
        // figé après un rattrapage par sync (prefix-closed → seuls les blocs à quorum comptent).
        chain.advance_finality(&state.validator_set);

        tracing::info!(applied, tip = chain.tip_height(), "Sync batch applied");

        if sync.count < batch {
            break; // no more blocks
        }
    }

    applied
}
