# Counter-audit prompt — Gemini (independent verification / regression hunt)

Verify independently whether the vulnerabilities listed below are **actually** fixed.
Do not assume the patch is correct. Compare the code before and after, and look for
regressions the fix may have introduced.

Do not reply "verified, correct". A reply that merely agrees is a failed review. For each
item, state what you checked, at which `file:line`, and what would falsify your
conclusion.

## Before you start — a required calibration step

In the previous round, your report asserted CONFIRMED findings against files and
functions that **do not exist in this repository**:

```
crates/vinx-core/src/ledger.rs        crates/vinx-core/src/crypto.rs
crates/vinx-core/src/auth.rs          crates/vinx-core/src/state.rs
crates/vinx-network/src/codec.rs      crates/vinx-network/src/mempool.rs
verify_auth_token()   transfer_credits()   export_wallet()   Keypair::generate()
```

FINDING-07 claimed key generation used `StdRng::seed_from_u64(SystemTime::now())` and
rated it Critique. The real code is `SigningKey::generate(&mut OsRng)` at
`crates/vinx-crypto/src/keys.rs:39`; `seed_from_u64` appears nowhere in the workspace.

So, first: **list the actual crates and source files** in the workspace, and confirm each
file you intend to cite exists. If you cannot locate a symbol, grep for it and report the
absence rather than describing what it would contain. Every claim in your report must
quote code that is present at branch `claude/vinx-ledger-audit-verify-ya6rl5`.

Your one correct contribution last round was refuting FINDING-03 (integer underflow) by
reading `checked_sub` and the `overflow-checks` profile. That is the method to apply
throughout — including to the fixes below.

## What to verify

For each: (a) was the vulnerability real, (b) is it actually gone, (c) did the fix break
anything.

1. **VINX-02** — `crates/vinx-core/src/block.rs`, `bls_signer_count_from_bitmap`.
   The empty-bitmap fallback to `bls_cosigner_pks` was removed. Confirm no path still
   reaches a signer count derived from block-supplied keys. Check every caller of
   `bls_signer_count_unverified`.

2. **VINX-05** — `consensus::signer_weight`, `Chain::advance_finality`.
   Both now take `indexed_bls_pks`. Confirm every call site passes a registry derived
   from the correct `WorldState`, and that none passes an empty slice as a placeholder
   (an empty registry silently yields weight 0 — where would that change behaviour?).

3. **VINX-01** — `consensus::verify_proposer_authenticated` and its wiring in
   `p2p/mod.rs`. Confirm it is called on **every** path that appends a block: `NewBlock`,
   `SyncResponse`, both `CompactBlock` reconstruction sites, `sync.rs`, and startup
   replay from storage. List any path that appends without it.

4. **VINX-03** — `mempool.rs`, `rpc/handlers.rs`, `world_state.rs`. Confirm the three
   entry points call `verify_tx_signature_pure`, and that no other route reaches
   `apply_transaction_trusted` without prior verification.

5. **VINX-08** — `chain.rs`. Confirm `reorg_replace` and `median_time_past_ending_at`
   handle `height_base` correctly, and search for any other absolute-height indexing into
   `Chain::blocks`.

6. **VINX-07, VINX-09, VINX-11** — see PATCH.md.

## Regression hunt — the part that matters most

These patches changed public APIs and consensus-relevant behaviour. Look specifically for:

- **Liveness.** Blocks that were legitimately accepted before and are now rejected. In
  particular: does requiring the proposer's bitmap bit break block production for a
  validator that has not yet registered a BLS key? What happens at genesis, at the first
  block after a validator set change, and for a backup proposer on slot-skip? Trace
  `producer.rs` and confirm a produced block carries what ingestion now demands.
- **Determinism.** `signer_weight` now depends on a registry read at the call site. Can
  two honest nodes read different registries for the same block and elect different heads?
- **Compatibility.** `CompactBlock` gained two `#[serde(default)]` fields. What happens
  when an old node and a new node talk to each other, in both directions? Does the
  storage layer persist anything whose encoding changed?
- **The test migration.** `consensus::validate_block` was deleted and tests were moved to
  `validate_block_with_registry` with deterministic per-index BLS keys. Read the diff in
  `crates/vinx-node/tests/bench_n3.rs`, `consensus.rs` and `chain.rs` tests and judge
  whether any assertion was **weakened** to make it pass — a test that no longer proves
  what it claims is a regression disguised as a fix.
- **Error handling.** Check that no fix introduced `unwrap`, silent `unwrap_or(0)` on a
  security-relevant `Result`, or a swallowed error.

## Known-open, deliberately not fixed

VINX-04, VINX-10, VINX-12, VX-RED-003/007, VINX-06 — see FINDINGS_STATUS.md. Do not
report them as new. **Do** verify my claim that they are *not* made worse by these
patches, and check my reasoning that they require protocol changes rather than local
fixes.

## Output

A table: item / was it real / is it fixed / evidence (`file:line`) / regressions found.
Then a detailed section for every regression or incomplete fix, and a final section
listing anything you could not verify and why.
