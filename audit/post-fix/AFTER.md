# AFTER — corrected code

## VINX-02 / VINX-05 — the registry is the only authority

`crates/vinx-core/src/block.rs`

```rust
pub fn bls_signer_count_from_bitmap(
    &self,
    indexed_bls_pks: &[Option<[u8; 48]>],
) -> Result<usize, BlsError> {
    if self.bls_bitmap.is_empty() {
        return match self.bls_aggregate {
            Some(_) => Err(BlsError::EmptyAggregate), // signatures attributed to nobody
            None => Ok(0),
        };
    }
    // ... reconstruct signers from indexed_bls_pks (the on-chain registry) ...
    for i in 1..pks.len() {
        if pks[..i].iter().any(|p| p.0 == pks[i].0) {
            return Err(BlsError::InvalidKey); // one bit = one independent decision
        }
    }
    bls_verify_aggregate(&pks, &agg_sig, &self.header.hash())?;
    Ok(pks.len())
}

pub fn is_finalized(
    &self,
    validator_set: &ValidatorSet,
    indexed_bls_pks: &[Option<[u8; 48]>],
) -> bool {
    if self.is_genesis() { return true; }
    self.bls_signer_count_from_bitmap(indexed_bls_pks)
        .map(|c| c >= validator_set.quorum())
        .unwrap_or(false)
}
```

`bls_signer_count` is now `bls_signer_count_unverified`, documented as display-only:

> `bls_cosigner_pks` is supplied **by the block itself**. This function proves only that
> the aggregate matches the keys the block chose to advertise — it does *not* prove those
> keys belong to any validator. […] Every security decision — finality, fork-choice
> weight, block validation — must use `bls_signer_count_from_bitmap` with the on-chain
> registry.

`crates/vinx-node/src/consensus.rs`

```rust
fn signer_weight(block: &Block, indexed_bls_pks: &[Option<[u8; 48]>]) -> usize {
    block.bls_signer_count_from_bitmap(indexed_bls_pks).unwrap_or(0)
}
```

`crates/vinx-node/src/chain.rs`

```rust
Some((_, b)) => b
    .bls_signer_count_from_bitmap(indexed_bls_pks)
    .map(|c| c >= threshold)
    .unwrap_or(false),
```

`consensus::validate_block` (the unregistered twin) is **deleted**. One definition of
block validity remains: `validate_block_with_registry`.

The registry now flows through `is_finalized`, `advance_finality`, `signer_weight`,
`more_canonical`, `canonical_head`, `canonical_choice`, `would_reorg_at` and
`BlockResponse::from_block`, sourced from `WorldState::indexed_bls_keys` at each call
site.

## VINX-01 — proposer authentication

`crates/vinx-node/src/consensus.rs`

```rust
pub fn verify_proposer_authenticated(
    block: &Block,
    validator_set: &ValidatorSet,
    indexed_bls_pks: &[Option<[u8; 48]>],
) -> Result<(), NodeError> {
    if block.is_genesis() { return Ok(()); }

    let Some(idx) = validator_set.index_of(&block.header.validator) else {
        return Err(/* non-validator proposer */);
    };
    if !block.bls_bitmap_has(idx) {
        return Err(/* no co-signature from its declared proposer */);
    }
    block.bls_signer_count_from_bitmap(indexed_bls_pks)?;
    Ok(())
}
```

Called in both `NewBlock` and `SyncResponse` before any state work:

```rust
let indexed_pks = state.read().await.indexed_bls_keys(&vs);
if let Err(e) = crate::consensus::verify_proposer_authenticated(&block, &vs, &indexed_pks) {
    warn!(height, error = %e, "P2P block failed proposer authentication");
    return;
}
```

The `CompactBlock` message now carries the proof, and reassembly preserves it:

```rust
CompactBlock {
    header: BlockHeader,
    tx_hashes: Vec<[u8; 32]>,
    #[serde(default)] bls_aggregate: Option<Vec<u8>>,
    #[serde(default)] bls_bitmap: Vec<u8>,
},
```

**Why not require quorum here.** A freshly gossiped block carries only its producer's own
co-signature and accumulates the rest via the pending-BLS path; requiring quorum on
ingestion would stall the chain. Quorum stays in `validate_block_with_registry` and in
finality. This check answers a different question — *who wrote this block* — which was
previously unanswered.

## VINX-03 — one definition of cryptographic validity

`crates/vinx-node/src/mempool.rs`

```rust
fn verify_sig_static(tx: &Transaction) -> bool {
    WorldState::verify_tx_signature_pure(tx).is_ok()
}
```

`crates/vinx-node/src/rpc/handlers.rs` — both `submit_tx` and `submit_tx_batch`:

```rust
WorldState::verify_tx_signature_pure(&tx).map_err(|e| ApiError::BadRequest(e.to_string()))?;
```

`crates/vinx-state/src/world_state.rs` — structural gate in `admission_check`:

```rust
if tx.sponsor_pub_key.is_none() || tx.sponsor_signature.is_none() {
    return Err(CoreError::InvalidTransaction(
        "sponsored transaction is missing the sponsor's key or signature".to_string(),
    ));
}
```

## VINX-08 — height → index conversion

`crates/vinx-node/src/chain.rs`

```rust
let Some(idx) = height.checked_sub(self.height_base) else { return Vec::new(); };
let idx = idx as usize;
if idx >= self.blocks.len() { return Vec::new(); }
let removed: Vec<Hash32> = self.blocks.drain(idx..).map(|(hash, _)| hash).collect();
```

and in `median_time_past_ending_at`:

```rust
let end = height
    .checked_sub(self.height_base)
    .map_or(0, |i| (i as usize).min(self.blocks.len()));
```

## VINX-07 — path parity on timestamp bounds

`crates/vinx-node/src/p2p/mod.rs`, `SyncResponse` arm:

```rust
if block.header.timestamp > now.saturating_add(vinx_core::amount::MAX_CLOCK_DRIFT_SECS) {
    warn!(height, "SyncResponse block timestamp too far in the future");
    break;
}
```

All four ingestion paths now apply the same bound.

## VINX-09 — bounded work

`crates/vinx-node/src/p2p/mod.rs`, before any resolution:

```rust
if tx_hashes.len() != header.tx_count as usize { /* reject */ }
if tx_hashes.len() > P2pMessage::MAX_TX_REQUEST_HASHES { /* reject */ }
let (resolved_txs, missing_hashes) = mempool.read().await.resolve_hashes(&tx_hashes);
```

`crates/vinx-node/src/mempool.rs` — single scan, hashed request set:

```rust
pub fn resolve_hashes(&self, hashes: &[[u8; 32]]) -> (Vec<Transaction>, Vec<[u8; 32]>) {
    let wanted: AHashSet<[u8; 32]> = hashes.iter().copied().collect();
    // one pass over the mempool, then reorder to the request order
}
```

## VINX-11 — key uniqueness

`crates/vinx-state/src/world_state.rs`, in `apply_register_bls_key`:

```rust
if self.validator_pool.iter().any(|(addr, e)| {
    addr != &tx.from && e.bls_pub_key.as_deref() == Some(&pk_arr[..])
}) {
    return Err(CoreError::InvalidTransaction(
        "BLS public key already registered by another validator".to_string(),
    ));
}
```
