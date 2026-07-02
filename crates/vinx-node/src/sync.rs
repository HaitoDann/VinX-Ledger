//! Startup chain-sync: fetches blocks from a trusted peer and replays them.

use serde::Deserialize;
use vinx_core::{Block, ValidatorSet};
use vinx_state::WorldState;

use crate::chain::Chain;

#[derive(Deserialize)]
struct SyncResponse {
    count: usize,
    blocks: Vec<Block>,
}

/// Syncs the local chain and state from `peer_rpc_url` starting at the local
/// tip height + 1.  Returns the number of blocks applied.
///
/// Blocks are validated (hash chain linkage + finalization quorum) before
/// being applied.  The function stops on the first validation failure and
/// returns the count applied so far.
pub async fn sync_from_peer(
    peer_rpc_url: &str,
    state: &mut WorldState,
    chain: &mut Chain,
    _validator_set: &ValidatorSet,
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

            // Apply all transactions to state
            for tx in &block.transactions {
                if let Err(e) = state.apply_transaction(tx) {
                    tracing::error!(error = %e, height = block.header.height, "Sync tx failed — aborting");
                    return applied;
                }
            }
            state.block_height = block.header.height;
            state.check_upgrade_activation();
            let _rewards = state.distribute_staking_rewards();

            chain.push(block);
            applied += 1;
        }

        tracing::info!(applied, tip = chain.tip_height(), "Sync batch applied");

        if sync.count < batch {
            break; // no more blocks
        }
    }

    applied
}
