//! Startup chain-sync over HTTP: fetches committed blocks (with their certificates) from
//! a peer and applies them through the single block-validation path (ADR 0082).

use futures::future::join_all;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};
use vinx_core::{Block, CommitCert};
use vinx_state::WorldState;

use crate::chain::Chain;
use crate::execution;
use crate::rpc::types::ChainSnapshotResponse;

/// One committed block as served by `GET /chain/commits`.
#[derive(Deserialize, serde::Serialize, Clone, Debug)]
pub struct CommittedRow {
    pub block: Block,
    pub commit: CommitCert,
}

#[derive(Deserialize)]
struct CommitsResponse {
    rows: Vec<CommittedRow>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validates `row` as the next block and installs it. The peer is only a *source*: the
/// certificate is checked against the local voting set and the block is fully
/// re-executed (`execution::execute_committed`), exactly like a block from gossip.
fn apply_row(
    row: CommittedRow,
    state: &mut WorldState,
    chain: &mut Chain,
    checkpoints: &crate::checkpoints::Checkpoints,
) -> Result<(), String> {
    let height = row.block.header.height;
    let post =
        execution::execute_committed(state, &chain.tip(), &row.block, &row.commit, now_secs())?;
    // ADR 0074 §2.3 — a block at a checkpoint height must carry the expected hash.
    checkpoints
        .accepts_block(height, row.block.hash())
        .map_err(|e| format!("block {height} refused by checkpoints: {e}"))?;
    *state = post;
    chain.push(row.block, Some(row.commit));
    Ok(())
}

async fn fetch_rows(
    client: &reqwest::Client,
    base_url: &str,
    from: u64,
    limit: usize,
) -> Result<Vec<CommittedRow>, String> {
    let url = format!("{base_url}/chain/commits?from={from}&limit={limit}");
    let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json::<CommitsResponse>()
        .await
        .map(|r| r.rows)
        .map_err(|e| e.to_string())
}

/// Syncs the local chain and state from `peer_rpc_url`, starting at the local tip + 1.
/// Stops at the first invalid block. Returns the number of blocks applied.
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
    let base = peer_rpc_url.trim_end_matches('/');
    let batch = 200usize;
    let mut applied = 0usize;
    loop {
        let rows = match fetch_rows(&client, base, chain.tip_height() + 1, batch).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "Sync request failed");
                break;
            }
        };
        let n = rows.len();
        for row in rows {
            if let Err(e) = apply_row(row, state, chain, checkpoints) {
                tracing::error!(error = %e, "Sync block rejected — aborting");
                return applied;
            }
            applied += 1;
        }
        tracing::info!(applied, tip = chain.tip_height(), "Sync batch applied");
        if n < batch {
            break;
        }
    }
    applied
}

/// Number of blocks per HTTP fetch in parallel sync.
const PARALLEL_BATCH_SIZE: usize = 200;
/// Maximum concurrent HTTP fetches issued to the peer at once.
const MAX_PARALLEL_FETCHES: usize = 8;

/// Parallel catch-up sync: downloads batches concurrently, applies them strictly in order.
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
    let base = peer_rpc_url.trim_end_matches('/').to_owned();

    let peer_height: u64 = {
        let url = format!("{base}/chain/height");
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                Ok(v) => match v.get("height").and_then(|h| h.as_u64()) {
                    Some(h) => h,
                    None => return 0,
                },
                Err(_) => return 0,
            },
            _ => return 0,
        }
    };
    let local_tip = chain.tip_height();
    if peer_height <= local_tip {
        return 0;
    }
    let mut ranges: Vec<(u64, usize)> = Vec::new();
    let mut h = local_tip + 1;
    while h <= peer_height {
        let limit = ((peer_height - h + 1) as usize).min(PARALLEL_BATCH_SIZE);
        ranges.push((h, limit));
        h += limit as u64;
    }
    tracing::info!(
        local_tip,
        peer_height,
        batches = ranges.len(),
        "Parallel sync: starting"
    );

    let mut total = 0usize;
    for window in ranges.chunks(MAX_PARALLEL_FETCHES) {
        let fetches = window
            .iter()
            .map(|(from, limit)| fetch_rows(&client, &base, *from, *limit));
        let results = join_all(fetches).await;
        for result in results {
            let rows = match result {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "Parallel sync: fetch failed — stopping");
                    return total;
                }
            };
            for row in rows {
                if let Err(e) = apply_row(row, state, chain, checkpoints) {
                    tracing::error!(error = %e, "Parallel sync: block rejected — aborting");
                    return total;
                }
                total += 1;
            }
        }
        tracing::info!(
            total,
            tip = chain.tip_height(),
            "Parallel sync: window applied"
        );
    }
    total
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

    // The snapshot block is installed as committed, so its certificate must hold: signed
    // by more than 2/3 of the power of the set that voted on it — the adopted state's
    // `last_voting_set` — with keys registered in that state. Otherwise a peer could hand
    // us a fabricated history. (This is the best a syncing node can do without trusted
    // checkpoints — see ADR 0074.)
    {
        let voters = match new_state.last_voting_set.as_ref() {
            Some(v) => v.clone(),
            None => {
                tracing::error!("Snapshot sync: snapshot has no voting set — rejecting");
                return false;
            }
        };
        // The keys the voters signed with, frozen with the voting set (ADR 0084).
        let keys: Vec<Option<[u8; 48]>> = new_state
            .last_voting_keys
            .iter()
            .map(|k| k.as_deref().and_then(|b| b.try_into().ok()))
            .collect();
        if let Err(e) = snap.commit.verify(
            new_state.chain_id,
            snap.height,
            &snap.block.hash(),
            &voters,
            &keys,
        ) {
            tracing::error!(
                height = snap.height, error = %e,
                "Snapshot sync: snapshot block is not committed by its voting set — rejecting"
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

    // The ancestors only feed the protocol clock, but they must be the real ones: they
    // are authenticated by their hash chain up to the committed snapshot block.
    let expected = vinx_core::amount::MEDIAN_TIME_BLOCKS as u64 - 1;
    if snap.ancestors.len() as u64 != expected.min(snap.height)
        || !Chain::ancestors_link(&snap.ancestors, &snap.block)
    {
        tracing::error!(
            height = snap.height,
            "Snapshot sync: missing or unlinked ancestor blocks — rejecting"
        );
        return false;
    }

    // 5. Initialize the chain from the snapshot block and replace state.
    let new_chain = Chain::new_from_snapshot(snap.ancestors, snap.block, Some(snap.commit));
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
