//! Startup chain-sync: fetches blocks from a trusted peer and replays them.

use futures::future::join_all;
use rayon::prelude::*;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};
use vinx_core::Block;
use vinx_state::WorldState;

use crate::chain::Chain;
use crate::consensus::validate_block_with_registry;
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
    checkpoints: &crate::checkpoints::Checkpoints,
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
            // against the on-chain BLS key registry. Uses the bitmap path
            // (validate_block_with_registry) so that only keys registered in the
            // validator pool can contribute to quorum — arbitrary BLS keys are rejected.
            let indexed_pks = state.indexed_bls_keys(&state.validator_set);
            if let Err(e) = validate_block_with_registry(&block, &state.validator_set, &indexed_pks)
            {
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

            // ADR 0074 §2.3 — un bloc à une hauteur de checkpoint doit porter le hash
            // attendu : c'est ce qui empêche un pair de servir une histoire fabriquée mais
            // internement cohérente. Contrôlé avant `push`, jamais après.
            if let Err(e) = checkpoints.accepts_block(block.header.height, block.hash()) {
                tracing::error!(error = %e, "Sync: bloc rejeté par les checkpoints");
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
        let indexed_pks = state.indexed_bls_keys(&state.validator_set);
        chain.advance_finality(&state.validator_set, &indexed_pks);

        tracing::info!(applied, tip = chain.tip_height(), "Sync batch applied");

        if sync.count < batch {
            break; // no more blocks
        }
    }

    applied
}

/// Number of blocks per HTTP fetch in parallel sync.
const PARALLEL_BATCH_SIZE: usize = 200;
/// Maximum concurrent HTTP fetches issued to the peer at once.
const MAX_PARALLEL_FETCHES: usize = 8;

/// Parallel catch-up sync (ADR 0038) — downloads block batches concurrently,
/// then applies them in strict sequential order.
///
/// Use this **after** `snapshot_sync_from_peer` when the remaining delta is
/// more than one batch.  It issues up to `MAX_PARALLEL_FETCHES` HTTP requests
/// to the peer at the same time, cutting wall-clock download time by ~4–8×
/// compared to the sequential `sync_from_peer`, while keeping state-transition
/// application strictly in order.
///
/// Falls back gracefully: any HTTP error or validation failure is logged and
/// sync stops at the last successfully applied height.
///
/// Returns the total number of blocks applied.
pub async fn parallel_sync_from_peer(
    peer_rpc_url: &str,
    state: &mut WorldState,
    chain: &mut Chain,
    checkpoints: &crate::checkpoints::Checkpoints,
) -> usize {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();
    let base_url = peer_rpc_url.trim_end_matches('/').to_owned();

    // 1. Ask the peer for its current tip so we can pre-plan the batches.
    let peer_height: u64 = {
        let url = format!("{}/chain/height", base_url);
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                Ok(v) => match v.get("height").and_then(|h| h.as_u64()) {
                    Some(h) => h,
                    None => {
                        tracing::warn!("Parallel sync: could not read peer height; falling back to sequential sync");
                        return 0;
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "Parallel sync: height parse failed");
                    return 0;
                }
            },
            Ok(r) => {
                tracing::warn!(status = %r.status(), "Parallel sync: height endpoint error");
                return 0;
            }
            Err(e) => {
                tracing::warn!(error = %e, "Parallel sync: height request failed");
                return 0;
            }
        }
    };

    let local_tip = chain.tip_height();
    if peer_height <= local_tip {
        return 0; // already caught up
    }

    // 2. Build the list of (from_height, limit) ranges for every needed batch.
    let mut ranges: Vec<(u64, usize)> = Vec::new();
    let mut h = local_tip + 1;
    while h <= peer_height {
        let remaining = (peer_height - h + 1) as usize;
        let limit = remaining.min(PARALLEL_BATCH_SIZE);
        ranges.push((h, limit));
        h += limit as u64;
    }

    tracing::info!(
        local_tip,
        peer_height,
        batches = ranges.len(),
        "Parallel sync: starting ({} concurrent fetches max)",
        MAX_PARALLEL_FETCHES
    );

    let mut total_applied = 0usize;
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 3. Process ranges in windows of MAX_PARALLEL_FETCHES — download the window
    //    concurrently, then apply in order, then move to the next window.
    for window in ranges.chunks(MAX_PARALLEL_FETCHES) {
        // Fire all fetches in this window concurrently.
        let fetches = window.iter().map(|(from, limit)| {
            let url = format!("{}/chain/sync?from={}&limit={}", base_url, from, limit);
            let c = client.clone();
            async move {
                let resp = match c.get(&url).send().await {
                    Ok(r) => r,
                    Err(e) => return Err(e.to_string()),
                };
                if !resp.status().is_success() {
                    return Err(format!("HTTP {}", resp.status()));
                }
                resp.json::<SyncResponse>().await.map_err(|e| e.to_string())
            }
        });
        let results: Vec<Result<SyncResponse, String>> = join_all(fetches).await;

        // Apply in strict order — stop the moment anything fails.
        let mut window_ok = true;
        'batch: for (i, result) in results.into_iter().enumerate() {
            let (from, _) = window[i];
            let sync = match result {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(from, error = %e, "Parallel sync: fetch failed — stopping");
                    window_ok = false;
                    break 'batch;
                }
            };

            for block in sync.blocks {
                let height = block.header.height;

                // Hash-chain linkage.
                if block.header.prev_hash != chain.tip_hash() {
                    tracing::error!(height, "Parallel sync: wrong prev_hash — aborting");
                    return total_applied;
                }
                // Timestamp monotonicity + drift cap.
                if block.header.timestamp <= chain.tip_timestamp() {
                    tracing::error!(height, "Parallel sync: timestamp not monotonic — aborting");
                    return total_applied;
                }
                if block.header.timestamp
                    > now_secs.saturating_add(vinx_core::amount::MAX_CLOCK_DRIFT_SECS)
                {
                    tracing::error!(
                        height,
                        "Parallel sync: timestamp too far in future — aborting"
                    );
                    return total_applied;
                }
                // Proposer + co-signatures (registry-bound: bitmap verified against
                // on-chain BLS keys, same security posture as the sequential path).
                let indexed_pks = state.indexed_bls_keys(&state.validator_set);
                if let Err(e) =
                    validate_block_with_registry(&block, &state.validator_set, &indexed_pks)
                {
                    tracing::warn!(height, error = %e, "Parallel sync: block not finalized — stopping");
                    return total_applied;
                }
                // Transaction signatures (parallel Ed25519).
                if !block
                    .transactions
                    .par_iter()
                    .all(|tx| WorldState::verify_tx_signature_pure(tx).is_ok())
                {
                    tracing::error!(height, "Parallel sync: invalid tx signature — aborting");
                    return total_applied;
                }
                // State transition with rollback on error.
                let pre_quorum = state.validator_set.quorum();
                let protocol_ts = chain.median_time_past_with(block.header.timestamp);
                let snapshot = state.clone();
                state.set_block_context(protocol_ts);
                for tx in &block.transactions {
                    if let Err(e) = state.apply_transaction_trusted(tx) {
                        tracing::error!(error = %e, height, "Parallel sync: tx failed — aborting");
                        *state = snapshot;
                        return total_applied;
                    }
                }
                state.block_height = height;
                state.check_upgrade_activation();
                let _ = state.settle_block(&block.header.validator, height, protocol_ts);
                if !state.supply_invariant_holds() {
                    tracing::error!(height, "Parallel sync: supply invariant broken — aborting");
                    *state = snapshot;
                    return total_applied;
                }
                let root = state.compute_state_root();
                if root != block.header.state_root {
                    tracing::error!(height, "Parallel sync: state_root mismatch — aborting");
                    *state = snapshot;
                    return total_applied;
                }
                // ADR 0074 §2.3 — même garde que la sync séquentielle : un bloc à une
                // hauteur de checkpoint doit porter le hash attendu.
                if let Err(e) = checkpoints.accepts_block(block.header.height, block.hash()) {
                    tracing::error!(error = %e, "Parallel sync: bloc rejeté par les checkpoints");
                    *state = snapshot;
                    return total_applied;
                }

                let bh = block.header.height;
                chain.push(block);
                chain.note_quorum(bh, pre_quorum);
                total_applied += 1;
            }
        }

        let indexed_pks = state.indexed_bls_keys(&state.validator_set);
        chain.advance_finality(&state.validator_set, &indexed_pks);
        tracing::info!(
            total_applied,
            tip = chain.tip_height(),
            "Parallel sync: window applied"
        );

        if !window_ok {
            break;
        }
    }

    total_applied
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
/// Returns true when `raw` is safe to fetch a state snapshot from: HTTPS, or a loopback
/// address where there is no network path for an attacker to sit on.
///
/// # Parse with the same parser the client uses
///
/// This deliberately delegates to the `url` crate — the one `reqwest` itself parses with —
/// rather than splitting the string by hand. A hand-rolled parser was the first version and
/// it was bypassable: for `http://evil.com\@127.0.0.1/` it took the text after the last `@`
/// and saw the loopback address `127.0.0.1`, while `url` (per the WHATWG spec, where a
/// backslash terminates the authority for special schemes) resolves the host to `evil.com`.
/// The guard said "loopback, allow" and the client then fetched an entire world state, in
/// cleartext, from the attacker's host. Any divergence between the checking parser and the
/// connecting parser is exploitable; sharing one removes the class.
fn is_transport_acceptable(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw.trim()) else {
        return false; // unparseable — refuse rather than guess
    };
    match parsed.scheme() {
        "https" => true,
        "http" => match parsed.host() {
            // `Host::Domain` covers "localhost"; anything else resolvable is not loopback.
            Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false, // unknown scheme
    }
}

/// that block-by-block replay is preferred (or if any step fails).
pub async fn snapshot_sync_from_peer(
    peer_rpc_url: &str,
    state: &mut WorldState,
    chain: &mut Chain,
    checkpoints: &crate::checkpoints::Checkpoints,
) -> bool {
    // VINX-10: a snapshot installs an entire world state — balances, validator set, admin
    // key. Fetching that over plain HTTP hands any on-path attacker full control of the
    // node. Refuse cleartext except against a loopback peer (local dev).
    if !is_transport_acceptable(peer_rpc_url) {
        tracing::error!(
            peer = %peer_rpc_url,
            "Snapshot sync: refusing to fetch a state snapshot over plaintext HTTP —              use https:// (or a loopback address for local development)"
        );
        return false;
    }
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

    // 3. Decode: hex → decompress with zstd → deserialize borsh → WorldState.
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
    let mut new_state: WorldState = match borsh::from_slice(&raw) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "Snapshot sync: borsh deserialize failed");
            return false;
        }
    };

    // 4. Verify the state root before trusting anything.
    //
    // VINX-10: this used to compare against `snap.state_root`, a field supplied by the
    // *same peer* that sent the state — it proved only that the peer could hash what it
    // had just sent, and any fabricated state passed trivially. The authoritative value
    // is the one committed in the snapshot block's header, which the rest of the network
    // also sees. `snap.state_root` is now only cross-checked for a mismatch (a peer
    // disagreeing with itself is a bug or an attack; either way, refuse).
    let computed_root = new_state.compute_state_root();
    if computed_root != snap.block.header.state_root {
        tracing::error!(
            height = snap.height,
            "Snapshot sync: state does not match the snapshot block header's state_root              — rejecting snapshot"
        );
        return false;
    }
    match hex::decode(&snap.state_root) {
        Ok(b) if b.as_slice() == computed_root.as_ref() => {}
        Ok(_) => {
            tracing::error!(
                height = snap.height,
                "Snapshot sync: peer's state_root field contradicts its own block header                  — rejecting snapshot"
            );
            return false;
        }
        Err(e) => {
            tracing::error!(error = %e, "Snapshot sync: state_root hex decode failed");
            return false;
        }
    }
    // The block must also be at the height the snapshot claims, or the state and the
    // chain would be installed at inconsistent heights.
    if snap.block.header.height != snap.height {
        tracing::error!(
            claimed = snap.height,
            header = snap.block.header.height,
            "Snapshot sync: snapshot height disagrees with its block header — rejecting"
        );
        return false;
    }

    // The snapshot block is installed as *finalized* by `Chain::new_from_snapshot`, so it
    // must carry a real quorum of co-signatures from validators registered in the state
    // being adopted — otherwise a peer can hand us a fabricated history and declare it
    // irreversible. This is checked against the snapshot's own validator set, which is
    // the best a syncing node can do without trusted checkpoints; shipping checkpoints
    // (weak subjectivity) remains open, see audit/post-fix/FINDINGS_STATUS.md.
    {
        let indexed_pks = new_state.indexed_bls_keys(&new_state.validator_set);
        if let Err(e) =
            validate_block_with_registry(&snap.block, &new_state.validator_set, &indexed_pks)
        {
            tracing::error!(
                height = snap.height, error = %e,
                "Snapshot sync: snapshot block is not quorum-signed by its own validator                  set — rejecting"
            );
            return false;
        }
    }

    // ADR 0074 §2.3 — subjectivité faible. Les contrôles ci-dessus prouvent que le
    // snapshot est *internement cohérent* ; ils ne peuvent pas prouver que c'est la bonne
    // histoire, puisqu'un attaquant qui fabrique une chaîne entière avec ses propres
    // validateurs les satisfait tous. Seul un point d'ancrage livré avec le binaire
    // tranche : il ne peut pas venir du pair.
    if let Err(e) = checkpoints.accepts_snapshot(snap.block.header.height, snap.block.hash()) {
        tracing::error!(height = snap.height, error = %e, "Snapshot sync: rejeté par les checkpoints");
        return false;
    }
    if checkpoints.is_empty() {
        tracing::warn!(
            height = snap.height,
            "Snapshot sync: aucun checkpoint configuré — le nœud fait confiance au set de \
             validateurs du snapshot lui-même. À ne pas faire sur un réseau portant de la valeur."
        );
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

#[cfg(test)]
mod transport_tests {
    use super::is_transport_acceptable;

    /// VINX-10: a snapshot installs an entire world state, so the transport must not be
    /// attacker-controllable. HTTPS is accepted; cleartext only against loopback.
    #[test]
    fn https_is_accepted_and_public_http_is_refused() {
        assert!(is_transport_acceptable("https://seed.example.com:8545"));
        assert!(is_transport_acceptable("HTTPS://Seed.Example.Com/"));
        assert!(!is_transport_acceptable("http://seed.example.com:8545"));
        assert!(!is_transport_acceptable("http://203.0.113.7:8545"));
    }

    #[test]
    fn loopback_http_is_allowed_for_local_development() {
        assert!(is_transport_acceptable("http://localhost:8545"));
        assert!(is_transport_acceptable("http://127.0.0.1:8545"));
        assert!(is_transport_acceptable("http://127.1.2.3:8545/"));
        assert!(is_transport_acceptable("http://[::1]:8545"));
    }

    /// A host that merely *contains* a loopback-looking substring must not pass.
    #[test]
    fn lookalike_hosts_are_refused() {
        assert!(!is_transport_acceptable("http://localhost.evil.com:8545"));
        assert!(!is_transport_acceptable("http://127.0.0.1.evil.com/"));
        assert!(!is_transport_acceptable("http://evil.com/?x=localhost"));
        assert!(!is_transport_acceptable("http://evil.com#localhost"));
        assert!(!is_transport_acceptable("http://user@evil.com/"));
        assert!(!is_transport_acceptable("ftp://seed.example.com"));
        assert!(!is_transport_acceptable("seed.example.com:8545"));
    }

    /// The guard must agree with the parser the HTTP client actually uses. A hand-written
    /// parser did not: for these inputs it read a loopback host where `url` — and therefore
    /// `reqwest` — resolves an attacker-controlled one, so the check passed and the snapshot
    /// was then fetched in cleartext from that host.
    #[test]
    fn parser_confusion_hosts_are_refused() {
        // Backslash terminates the authority for special schemes (WHATWG URL): the real
        // host is `evil.com`, not the `127.0.0.1` that follows the `@`.
        assert!(!is_transport_acceptable("http://evil.com\\@127.0.0.1/"));
        // Loopback in the userinfo, real host after the `@`.
        assert!(!is_transport_acceptable("http://127.0.0.1:8545@evil.com/"));
        // Alternate IPv4 spellings of 127.0.0.1 are still loopback once parsed — they must
        // be treated consistently, not by string comparison.
        assert!(is_transport_acceptable("http://2130706433/"));
        assert!(is_transport_acceptable("http://127.1/"));
        // IPv4-mapped IPv6 loopback is not the IPv6 loopback `::1`.
        assert!(!is_transport_acceptable("http://[::ffff:127.0.0.1]/"));
        // Whitespace and case must not change the verdict.
        assert!(is_transport_acceptable("  HTTP://LOCALHOST:8545  "));
        assert!(!is_transport_acceptable("  HTTP://EVIL.COM  "));
    }
}
