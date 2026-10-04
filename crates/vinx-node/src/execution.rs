//! Block construction and validation — the **single** definition (ADR 0082).
//!
//! Every path that touches a block goes through here: the proposer building a proposal,
//! a validator checking one before voting, a node applying a committed block received by
//! gossip or sync, and the test harnesses. There is exactly one notion of "valid block",
//! so the paths can never disagree (the class of bug behind VINX-03).
//!
//! Execution never mutates the caller's state: it runs on a clone and returns the
//! post-state, which the caller installs only once the block is committed.

use vinx_core::{
    amount::{MAX_BLOCK_TXS, MAX_CLOCK_DRIFT_SECS},
    Block, BlockHeader, CommitCert, Transaction,
};
use vinx_crypto::{Address, Hash32};
use vinx_state::WorldState;

use crate::{chain::Tip, mempool::is_future_nonce};

/// Merkle root of the ordered transaction hashes of a block (ADR 0083): each payment
/// is provable with a short proof (`vinx_crypto::tx_proof`). Empty blocks: zero hash.
pub fn compute_receipts_root(txs: &[Transaction]) -> Hash32 {
    let hashes: Vec<Hash32> = txs.iter().map(|tx| tx.hash()).collect();
    vinx_crypto::tx_root(&hashes)
}

/// Verifies every transaction signature of `txs` in parallel (ADR 0015).
pub fn verify_tx_signatures(txs: &[Transaction]) -> bool {
    use rayon::prelude::*;
    txs.par_iter()
        .all(|tx| WorldState::verify_tx_signature_pure(tx).is_ok())
}

/// Opens `header` on a clone of `state`: protocol clock, then `begin_block`. Returns the
/// working state and the protocol timestamp.
fn open_block(
    state: &WorldState,
    tip: &Tip,
    header: &BlockHeader,
    last_commit: Option<&CommitCert>,
) -> Result<(WorldState, u64), String> {
    // ADR 0086: the chain activated a protocol this software does not implement —
    // refuse to execute (validate, vote, sync) rather than apply rules it does not know.
    let supported = vinx_core::protocol::NODE_PROTOCOL_VERSION;
    if state.current_version.as_u32() > supported.as_u32() {
        return Err(format!(
            "protocol upgrade required: the network runs {}, this node implements {} — \
             install the new version",
            state.current_version, supported
        ));
    }
    let mut s = state.clone();
    let protocol_ts = tip.median_time_past_with(header.timestamp);
    s.set_block_context(protocol_ts);
    s.begin_block(header, last_commit)
        .map_err(|e| e.to_string())?;
    Ok((s, protocol_ts))
}

/// Closes a block on the working state: settle rewards, next base fee, invariants.
fn close_block(
    s: &mut WorldState,
    height: u64,
    proposer: &Address,
    protocol_ts: u64,
    tx_count: usize,
) -> Result<Hash32, String> {
    s.block_height = height;
    s.check_upgrade_activation();
    s.settle_block(proposer, protocol_ts);
    // EIP-1559 style, deterministic: the next base fee follows how full this block was.
    s.update_base_fee(tx_count, MAX_BLOCK_TXS);
    if !s.supply_invariant_holds() {
        return Err(format!("block {height} breaks the supply invariant"));
    }
    Ok(s.compute_state_root())
}

/// Builds a proposal for the next height on top of `(state, tip)` from candidate
/// transactions (in priority order, typically `Mempool::select`).
///
/// Candidates that fail on the current state are skipped; those failing for a reason
/// other than a future nonce are returned as `invalid` so the caller can drop them from
/// the mempool. Returns the block, its post-state and the invalid transactions.
pub fn build_block(
    state: &WorldState,
    tip: &Tip,
    candidates: &[Transaction],
    proposer: Address,
    round: u32,
    now: u64,
) -> Result<(Block, WorldState, Vec<Transaction>), String> {
    let height = tip.height + 1;
    let last_commit = if height == 1 {
        None
    } else {
        Some(tip.commit.clone().ok_or_else(|| {
            format!("cannot build block {height}: the commit certificate of the tip is unknown")
        })?)
    };
    let timestamp = now.max(tip.timestamp.saturating_add(1));
    let mut header = BlockHeader {
        height,
        round,
        prev_hash: tip.hash,
        timestamp,
        validator: proposer,
        tx_count: 0,
        state_root: [0u8; 32],
        base_fee: 0,
        receipts_root: [0u8; 32],
        last_commit_hash: Block::last_commit_hash_of(last_commit.as_ref()),
        // ADR 0086: the proposer signals the protocol version its software runs.
        version: vinx_core::protocol::NODE_PROTOCOL_VERSION.as_u32(),
    };
    let (mut s, protocol_ts) = open_block(state, tip, &header, last_commit.as_ref())?;
    header.base_fee = s.base_fee.atoms() as u64;

    let mut included = Vec::new();
    let mut invalid = Vec::new();
    for tx in candidates.iter().take(MAX_BLOCK_TXS) {
        match s.apply_transaction_trusted(tx) {
            Ok(()) => included.push(tx.clone()),
            Err(e) if is_future_nonce(&e) => {}
            Err(e) => {
                tracing::debug!(error = %e, "transaction dropped while building a proposal");
                invalid.push(tx.clone());
            }
        }
    }

    header.tx_count = included.len() as u32;
    header.receipts_root = compute_receipts_root(&included);
    header.state_root = close_block(&mut s, height, &proposer, protocol_ts, included.len())?;
    let block = Block {
        header,
        transactions: included,
        last_commit,
    };
    Ok((block, s, invalid))
}

/// Fully validates `block` as the next block on top of `(state, tip)` and returns the
/// post-state. `now` bounds the header timestamp (ADR 0005, VINX-07).
///
/// Checks, in order: linkage, embedded certificate binding, timestamp bounds, size and
/// receipts root, transaction signatures, then the full state transition (proposer of the
/// round, previous commit certificate, every transaction, rewards, supply invariant) and
/// the resulting state root. Nothing is mutated on failure.
pub fn execute_block(
    state: &WorldState,
    tip: &Tip,
    block: &Block,
    now: u64,
) -> Result<WorldState, String> {
    let h = &block.header;
    if h.height != tip.height + 1 || h.prev_hash != tip.hash {
        return Err(format!("block {} does not extend the tip", h.height));
    }
    if !block.last_commit_matches() {
        return Err(format!(
            "block {} carries a certificate its header does not commit to",
            h.height
        ));
    }
    if h.timestamp <= tip.timestamp {
        return Err(format!("block {} timestamp is not monotonic", h.height));
    }
    if h.timestamp > now.saturating_add(MAX_CLOCK_DRIFT_SECS) {
        return Err(format!(
            "block {} timestamp is too far in the future",
            h.height
        ));
    }
    if block.transactions.len() != h.tx_count as usize || block.transactions.len() > MAX_BLOCK_TXS {
        return Err(format!(
            "block {} has an invalid transaction count",
            h.height
        ));
    }
    if compute_receipts_root(&block.transactions) != h.receipts_root {
        return Err(format!("block {} receipts root mismatch", h.height));
    }
    if !verify_tx_signatures(&block.transactions) {
        return Err(format!(
            "block {} has invalid transaction signature(s)",
            h.height
        ));
    }

    let (mut s, protocol_ts) = open_block(state, tip, h, block.last_commit.as_ref())?;
    if h.base_fee != s.base_fee.atoms() as u64 {
        return Err(format!("block {} declares a wrong base fee", h.height));
    }
    for tx in &block.transactions {
        s.apply_transaction_trusted(tx)
            .map_err(|e| format!("block {} transaction rejected: {e}", h.height))?;
    }
    let root = close_block(
        &mut s,
        h.height,
        &h.validator,
        protocol_ts,
        block.transactions.len(),
    )?;
    if root != h.state_root {
        return Err(format!("block {} state root mismatch", h.height));
    }
    Ok(s)
}

/// Verifies that `cert` commits `block` for the validator set of `state` (the state the
/// block applies to): quorum of power, registered keys, round not before the header's.
pub fn verify_commit(state: &WorldState, block: &Block, cert: &CommitCert) -> Result<(), String> {
    // A block is built at `header.round` and may be re-proposed and committed at a later
    // round (Tendermint "valid value"), never at an earlier one.
    if cert.round < block.header.round {
        return Err(format!(
            "certificate round {} precedes the block round {}",
            cert.round, block.header.round
        ));
    }
    let keys = state.indexed_bls_keys(&state.validator_set);
    cert.verify(
        state.chain_id,
        block.header.height,
        &block.hash(),
        &state.validator_set,
        &keys,
    )
    .map(|_| ())
    .map_err(|e| format!("block {} commit certificate: {e}", block.header.height))
}

/// Validates a **committed** block received from a peer (gossip or sync) and returns the
/// post-state: certificate first (cheap, rejects forgeries), then full execution.
pub fn execute_committed(
    state: &WorldState,
    tip: &Tip,
    block: &Block,
    cert: &CommitCert,
    now: u64,
) -> Result<WorldState, String> {
    verify_commit(state, block, cert)?;
    execute_block(state, tip, block, now)
}
