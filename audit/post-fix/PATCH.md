# PATCH — per-finding record

Format per finding: Vulnerability / Cause / Correction / Test / Result.

Audited commit `8774806`; fixes on `claude/vinx-ledger-audit-verify-ya6rl5`.

---

## VINX-02 / VX-RED-001 — forgeable BLS quorum (P0)

**Vulnerability.** A block reached finality quorum with BLS keys belonging to no
validator. Demonstrated against a 7-validator set (quorum 5) whose on-chain registry was
`vec![None; 7]` — no validator had registered a key at all —
`validate_block_with_registry` returned `Ok` and `is_finalized` returned `true`.

**Cause.** `Block::bls_signer_count_from_bitmap`, the primitive presented as the
registry-bound check, opened with:

```rust
if self.bls_bitmap.is_empty() {
    return self.bls_signer_count();   // block.rs:144-146
}
```

`bls_signer_count()` reconstructs signers from `self.bls_cosigner_pks` — a field the
block carries — and returns `pks.len()`. The registry is never consulted. The fallback
was documented as compatibility with pre-ADR-0029 blocks, which made a security-critical
downgrade selectable by untrusted input: omit a field, disable the check.

**Correction** (`b361343`). Fallback removed. An empty bitmap carrying an aggregate is
malformed (signatures attributed to no validator index) → `Err`; with no aggregate → 0.
`sign_block_bls` has always set the bitmap, so no legitimate producer is affected.
Duplicate G1 keys among reconstructed signers are also rejected.

**Test.** `audit_regression_node.rs::forged_quorum_via_empty_bitmap_is_rejected`;
`vinx-core block.rs::test_empty_bitmap_does_not_fall_back_to_cosigner_pks`,
`::test_empty_bitmap_without_aggregate_counts_zero`,
`::test_duplicate_registered_key_across_slots_rejected`.

**Result.** Attack succeeded before, rejected after. Full suite green.

---

## VINX-03 / VX-RED-005 — sponsor signature never verified (P0, fund theft)

**Vulnerability.** Any account could be drained by a stranger knowing only its address.
Demonstrated: a victim funded with 5 000 VINX went to zero.

**Cause.** A sponsored transaction debits `fee` from the sponsor. Only
`WorldState::verify_tx_signature_pure` verified the sponsor's signature, and none of the
three mempool entry points called it — `handlers::submit_tx`,
`handlers::submit_tx_batch` and `Mempool::verify_sig_static` each verified the *sender*
only. `admission_check` merely confirmed the sponsor existed and could cover the fee.
The transaction entered the mempool queues (whose documented invariant is that every
entry is validly signed) and the producer applied it with `apply_transaction_trusted`,
which skips crypto by contract. `fee` has no upper bound.

Attack: 0-VINX transfer to self, `sponsor = victim`, `fee = victim's whole balance`,
`sponsor_pub_key = None`, `sponsor_signature = None`, signed with the attacker's own key.

Second effect: honest peers *do* verify the sponsor, so they reject the resulting block.
Sending one such transaction to the leader at each height stalls the network.

**Correction** (`ec1127a`). All three paths call `WorldState::verify_tx_signature_pure`.
One definition of cryptographic validity; partially duplicating it created the gap.
`admission_check` additionally rejects a sponsored transaction that does not carry a
sponsor key and signature.

**Test.** `audit_regression.rs::sponsor_without_signature_cannot_drain_an_account`,
`::sponsor_with_forged_signature_is_rejected`,
`::genuinely_sponsored_transaction_still_applies` (the feature still works).

**Result.** Drain succeeded before, refused at admission after. Sponsorship still works.

---

## VINX-01 / VX-RED-002 — no proposer authentication on any P2P path (P0)

**Vulnerability.** A peer holding no validator key could author blocks accepted by every
node: full control of transaction ordering and censorship, with the issuance reward
directed at a validator of their choosing.

**Cause.** The `NewBlock` arm checked prev_hash, timestamps, `vs.contains(validator)`,
transaction signatures, state transition and `state_root` — and never read
`bls_aggregate`. Neither `validate_block` nor `validate_block_with_registry` was called
from `p2p/mod.rs`; the only callers were in `sync.rs`. `BlockHeader` carries no proposer
signature and `BlockSignature` is unused in production, so the aggregate was the only
evidence of authorship, and it went unread. `header.validator` is a plain public address.

`CompactBlock` was worse: its wire format carried only `header` and `tx_hashes`, and
reconstruction hardcoded `bls_aggregate: None, bls_cosigner_pks: vec![], bls_bitmap:
vec![]` before re-dispatching as `NewBlock` — a block with no signature at all was
applied. `SyncResponse` (up to 512 blocks, freely gossipable) checked membership only.

**Correction** (`fbc5e73`). New `consensus::verify_proposer_authenticated`: the
proposer's bit must be set in `bls_bitmap`, and the aggregate must verify against the
registered on-chain keys of exactly the set bits. Keys enter the registry only after a
verified PoP, so the aggregate cannot be forged without the proposer's secret key, and
tampering with the header changes `header.hash()`. Called from `NewBlock` and
`SyncResponse` before any state work. `CompactBlock` now carries `bls_aggregate` and
`bls_bitmap` on the wire and reassembly preserves them.

Deliberately does **not** require quorum on ingestion: a freshly gossiped block carries
only its producer's own co-signature and accumulates the rest afterwards, so requiring
quorum here would stall the chain. Quorum stays in `validate_block_with_registry` and
finality.

**Test.** `audit_regression_node.rs::proposer_impersonation_is_rejected` — covers an
unsigned block, a block padded with foreign keys, a block co-signed at the wrong index,
and a genuinely signed block that must still be accepted.

**Result.** Impersonation rejected; legitimate production unaffected (bench_n3 green).

---

## VINX-05 — fork-choice weight chosen by the block author (P1)

**Vulnerability.** A block padded with 50 self-generated BLS keys beat a genuinely
co-signed block at every contested height, then finalized.

**Cause.** `signer_weight` was `block.bls_signer_count().unwrap_or(0)` — the
`bls_cosigner_pks` path — and `Chain::advance_finality` used the same unverified count.

**Correction** (`b361343`). Both go through `bls_signer_count_from_bitmap` with the
registry. Weight is bounded by the validator set size by construction. `bls_signer_count`
was renamed `bls_signer_count_unverified` and documented as display-only, so no future
security decision reaches it by accident. `consensus::validate_block`, the unregistered
twin of `validate_block_with_registry`, was deleted.

**Test.** `audit_regression_node.rs::fork_choice_ignores_unregistered_cosigners`.

**Result.** Honest block now wins; forged block does not finalize.

---

## VINX-08 — `reorg_replace` ignores `height_base` (P1, remote crash)

**Vulnerability.** Any peer could kill any snapshot-synced node — i.e. every node that
joined a mature chain — by publishing a competing block at an unfinalized height.

**Cause.** `reorg_replace` used `let h = height as usize; self.blocks.drain(h..)` while
every other accessor subtracts `height_base`. On a snapshot-synced chain `height_base`
is the snapshot height and `blocks.len()` is small, so `drain` panicked with "start
index out of range". `height_base` is persisted and restored, so the condition survives
restarts. `median_time_past_ending_at` had the same defect — no panic, but a wrong MTP
window and therefore a divergent `state_root` on reorg replay.

**Correction** (`728736d`). Both convert height → index via `checked_sub(height_base)`
and return empty / clamp instead of indexing out of range.

**Test.** `audit_regression_node.rs::reorg_replace_is_height_base_aware` — asserts the
reorg now succeeds, installs the candidate, and preserves `height_base`.

**Result.** Panic before, correct reorg after.

---

## VINX-07 — `SyncResponse` missing the clock-drift bound (P1)

**Vulnerability.** Blocks dated arbitrarily far in the future move the Median Time Past.
Protocol time drives `emit_work_reward` and `mature_unbonds`, so this mints the entire
remaining supply in one block and matures every still-slashable bond.

**Cause.** The three other ingestion paths bound the timestamp by
`now + MAX_CLOCK_DRIFT_SECS`; the `SyncResponse` arm checked only monotonicity.

**Correction** (`fbc5e73`). The drift bound is applied there too.

**Test.** Not reproduced end-to-end — it needs the multi-node harness. Established by
direct comparison of the four ingestion paths; the fix makes them identical. Recorded as
requiring the harness in FINDINGS_STATUS.

**Result.** Path parity restored; full exploit unverified by design (see HUMAN REVIEW).

---

## VINX-09 — unbounded CompactBlock work (P2, remote CPU DoS)

**Vulnerability.** One `CompactBlock` message froze a node: `tx_hashes` was bounded only
by `MAX_DECODED_BYTES` (16 MiB, ~524 288 hashes) and each hash triggered a full
`pending_txs()` allocation (up to 100 000 entries) plus a SHA-256 per entry, all under
the mempool read lock — so block production and RPC froze together. The per-peer flood
guard counts messages, not work.

**Correction** (`74e3c69`, `fbc5e73`). The arm requires
`tx_hashes.len() == header.tx_count` and `<= MAX_TX_REQUEST_HASHES` before doing any
work. New `Mempool::resolve_hashes` resolves in a single scan; `get_by_hashes` hashes
the request set once instead of a linear `contains` per entry.

**Test.** No timing assertion added — a wall-clock threshold is a flaky test. The bound
is a structural precondition, verified by inspection and covered by existing CompactBlock
integration tests.

**Result.** Work is now O(mempool + hashes) with hashes capped at 512.

---

## VINX-11 — BLS PoP not bound to identity (P2, partial)

**Vulnerability.** Validator B copies A's published `bls_pub_key` / `bls_pop` out of
plain state and registers them as its own; the PoP verifies. Two bitmap indices then
resolve to the same G1 key, and aggregating A's single signature twice (an operation
requiring no secret) makes one decision count as two independent signers — corrupting
reliability scoring and epoch-pot distribution.

**Correction** (`8627ffc`, `b361343`). Registration rejects a G1 key already held by
another validator; `bls_signer_count_from_bitmap` rejects duplicate keys among
reconstructed signers, so the invariant holds even for keys registered earlier.

**Partial.** The PoP is still signed over `pk_bytes` alone. Binding it to
`bls_pub_key ‖ validator_address ‖ chain_id` is a protocol change and remains open.

**Test.** `audit_regression.rs::bls_key_cannot_be_registered_by_two_validators`;
`block.rs::test_duplicate_registered_key_across_slots_rejected`.

**Result.** Duplicate registration and double-counting both refused.
