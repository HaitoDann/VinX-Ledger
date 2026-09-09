# BEFORE — vulnerable code at `8774806`

The exact code each fix replaces, with the PoC result recorded against it.
All six proofs **passed on this commit**, i.e. every attack worked.

## VINX-02 — the fallback that disabled the registry

`crates/vinx-core/src/block.rs:140-146`

```rust
/// Falls back to `bls_signer_count()` (cosigner_pks path) when `bls_bitmap` is empty
/// (pre-ADR-0029 blocks produced before Phase 1 was deployed).
pub fn bls_signer_count_from_bitmap(
    &self,
    indexed_bls_pks: &[Option<[u8; 48]>],
) -> Result<usize, BlsError> {
    if self.bls_bitmap.is_empty() {
        return self.bls_signer_count();
    }
```

and the function it falls back to (`block.rs:178-204`), which trusts the block:

```rust
pub fn bls_signer_count(&self) -> Result<usize, BlsError> {
    // ...
    let pks: Vec<BlsPubKey> = self
        .bls_cosigner_pks          // ← supplied by the block itself
        .iter()
        // ...
    bls_verify_aggregate(&pks, &agg_sig, &self.header.hash())?;
    Ok(pks.len())                  // ← no registry, no membership, no bound
}
```

`is_finalized` used it directly (`block.rs:207-214`):

```rust
self.bls_signer_count()
    .map(|c| c >= validator_set.quorum())
    .unwrap_or(false)
```

**PoC result:** 7 validators, quorum 5, registry `vec![None; 7]`, block signed by 5
attacker-generated keys with `bls_bitmap = []` →
`validate_block_with_registry(..) == Ok`, `is_finalized(..) == true`.

## VINX-05 — fork-choice and finality on the same unverified count

`crates/vinx-node/src/consensus.rs:180-182`

```rust
fn signer_weight(block: &Block, _vs: &ValidatorSet) -> usize {
    block.bls_signer_count().unwrap_or(0)
}
```

`crates/vinx-node/src/chain.rs:172`

```rust
Some((_, b)) => b.bls_signer_count().map(|c| c >= threshold).unwrap_or(false),
```

**PoC result:** a block with 50 foreign co-signers beat a 2-signer block at
`canonical_head`, and `advance_finality` moved `finalized_height` to 1.

## VINX-03 — three entry points, none verifying the sponsor

`crates/vinx-node/src/mempool.rs:368-381`

```rust
fn verify_sig_static(tx: &Transaction) -> bool {
    let Some(pk) = &tx.pub_key else { return false };
    if Address::from_public_key(pk) != tx.from { return false; }
    let Some(sig) = &tx.signature else { return false; };
    pk.verify(&tx.signing_bytes(), sig).is_ok()
    // ← sponsor_pub_key / sponsor_signature never examined
}
```

`crates/vinx-node/src/rpc/handlers.rs:118-133` (and `800-818` for the batch path) did the
same thing inline. `admission_check` (`world_state.rs:1237-1246`) checked only existence
and balance:

```rust
if let Some(ref sponsor) = tx.sponsor {
    let Some(sponsor_acc) = self.accounts.get(sponsor) else { /* err */ };
    if sponsor_acc.balance < tx.fee { return Err(CoreError::InsufficientBalance); }
}
```

while the canonical predicate that *did* check it was never called on these paths:

```rust
// world_state.rs:1340-1355 — correct, and unreachable from the mempool
if let Some(ref sponsor_addr) = tx.sponsor {
    let spk = tx.sponsor_pub_key.as_ref().ok_or(CoreError::InvalidSignature)?;
    if &Address::from_public_key(spk) != sponsor_addr { return Err(CoreError::PubKeyMismatch); }
    let ssig = tx.sponsor_signature.as_ref().ok_or(CoreError::InvalidSignature)?;
    spk.verify(&tx.signing_bytes(), ssig)?;
}
```

**PoC result:** victim balance 5 000 VINX → 0, with `sponsor_signature: None`.
`admission_check` returned `Ok`; `apply_transaction_trusted` applied it;
`verify_tx_signature_pure` would have rejected it but was never invoked.

## VINX-01 — the only authority check on the P2P path

`crates/vinx-node/src/p2p/mod.rs:666`

```rust
if !vs.contains(&block.header.validator) {
    warn!(height, "P2P block from non-validator proposer");
    return;
}
```

then straight to transactions (`:731`) and commit (`:784`). `bls_aggregate` is never read
on this path. The compact path (`:1178-1184`, `:1272-1278`) rebuilt the block as:

```rust
let block = Block {
    header,
    transactions: ordered_txs,
    bls_aggregate: None,
    bls_cosigner_pks: vec![],
    bls_bitmap: vec![],
};
```

and re-dispatched it as `NewBlock`.

## VINX-08 — absolute height used as a slice index

`crates/vinx-node/src/chain.rs:386-389`

```rust
let h = height as usize;
let removed: Vec<Hash32> = self.blocks.drain(h..).map(|(hash, _)| hash).collect();
```

against the convention used everywhere else (`chain.rs:141`):

```rust
let idx = height.checked_sub(self.height_base)? as usize;
```

**PoC result:** chain with `height_base = 1000` and 2 blocks, `reorg_replace(1001, ..)`
panicked with "start index out of range".

## VINX-12 — non-injective signing bytes (still open)

`crates/vinx-core/src/transaction.rs:158-168`

```rust
if !self.payload.is_empty() {
    bytes.extend_from_slice(&self.payload);   // ← no length prefix
}
if let Some(ref sponsor) = self.sponsor {
    bytes.push(1u8);
    bytes.extend_from_slice(sponsor.as_bytes());
} else {
    bytes.push(0u8);
}
```

**PoC result:** for a sponsor address ending in `0x00`, `tx_a.signing_bytes() ==
tx_b.signing_bytes()` and `tx_a.hash() == tx_b.hash()` for two transactions with
different fee payers.

## VINX-04 — the Merkle leaf (still open)

`crates/vinx-state/src/world_state.rs:2426-2434` — `sha256(address ‖ balance ‖ nonce ‖
staked)`. Nothing else is committed.

**PoC result:** two states differing in `validator_set`, `admin_address` and
`epoch_beacon` produced **equal** `state_root`s.
