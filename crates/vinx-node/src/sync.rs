//! Startup chain-sync: fetches blocks from a trusted peer and replays them.

use rayon::prelude::*;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};
use vinx_core::Block;
use vinx_state::WorldState;

use crate::chain::Chain;
use crate::consensus::validate_block;
use crate::rpc::types::ChainSnapshotResponse;

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

            // ADR 0002/0027 — SÛRETÉ : quorum de finalité sur le set COMPLET bondé à cette
            // hauteur (jamais le set actif) — state.validator_set est le set pré-bloc (replay
            // ordonné), capturé avant les changements de set par gouvernance de ce bloc.
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

/// Minimum block gap before snapshot sync is preferred over block-by-block replay.
const SNAPSHOT_SYNC_THRESHOLD: u64 = 500;

/// Bootstrap a new node from a peer's state snapshot (ADR snapshot-sync).
///
/// Fetches the peer's current tip height first. If the local tip is more than
/// `SNAPSHOT_SYNC_THRESHOLD` blocks behind, downloads the full WorldState snapshot
/// from `GET /chain/snapshot`, verifies the `state_root`, and replaces the local
/// state and chain with the snapshot. The caller should then call `sync_from_peer`
/// for the remaining delta between the snapshot height and the current peer tip.
///
/// Returns `true` if a snapshot was applied, `false` if the gap is small enough
/// that block-by-block replay is preferred (or if any step fails).
pub async fn snapshot_sync_from_peer(
    peer_rpc_url: &str,
    state: &mut WorldState,
    chain: &mut Chain,
) -> bool {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_default();

    // 1. Check the peer's current tip height.
    let height_url = format!("{}/chain/height", peer_rpc_url.trim_end_matches('/'));
    let peer_height: u64 = match client.get(&height_url).send().await {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => match v.get("height").and_then(|h| h.as_u64()) {
                Some(h) => h,
                None => {
                    tracing::warn!("Snapshot sync: could not read peer height");
                    return false;
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "Snapshot sync: failed to parse height response");
                return false;
            }
        },
        Ok(r) => {
            tracing::warn!(status = %r.status(), "Snapshot sync: height endpoint returned error");
            return false;
        }
        Err(e) => {
            tracing::warn!(error = %e, "Snapshot sync: height request failed");
            return false;
        }
    };

    let local_tip = chain.tip_height();
    if peer_height <= local_tip + SNAPSHOT_SYNC_THRESHOLD {
        tracing::info!(
            local_tip,
            peer_height,
            "Snapshot sync: gap is small, using block-by-block replay"
        );
        return false;
    }

    tracing::info!(
        local_tip,
        peer_height,
        gap = peer_height - local_tip,
        "Snapshot sync: downloading state snapshot from peer"
    );

    // 2. Fetch the snapshot.
    let snap_url = format!("{}/chain/snapshot", peer_rpc_url.trim_end_matches('/'));
    let snap: ChainSnapshotResponse = match client.get(&snap_url).send().await {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "Snapshot sync: failed to parse snapshot response");
                return false;
            }
        },
        Ok(r) => {
            tracing::warn!(status = %r.status(), "Snapshot sync: snapshot endpoint returned error");
            return false;
        }
        Err(e) => {
            tracing::warn!(error = %e, "Snapshot sync: snapshot request failed");
            return false;
        }
    };

    // 3. Decode: hex → decompress with zstd → deserialize bincode → WorldState.
    let compressed = match hex::decode(&snap.state_hex) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "Snapshot sync: hex decode failed");
            return false;
        }
    };
    let raw = match zstd::decode_all(compressed.as_slice()) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "Snapshot sync: zstd decompress failed");
            return false;
        }
    };
    let mut new_state: WorldState = match bincode::deserialize(&raw) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "Snapshot sync: bincode deserialize failed");
            return false;
        }
    };

    // 4. Verify the state root before trusting anything.
    let computed_root = new_state.compute_state_root();
    let expected_root = match hex::decode(&snap.state_root) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "Snapshot sync: state_root hex decode failed");
            return false;
        }
    };
    if computed_root.as_ref() != expected_root.as_slice() {
        tracing::error!(
            height = snap.height,
            "Snapshot sync: state_root mismatch — rejecting snapshot"
        );
        return false;
    }

    // 5. Initialize the chain from the snapshot block and replace state.
    let new_chain = Chain::new_from_snapshot(snap.block);
    *state = new_state;
    *chain = new_chain;

    tracing::info!(
        height = snap.height,
        "Snapshot sync: applied — continuing with block-by-block sync for delta"
    );
    true
}
