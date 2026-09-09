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

### Second round (landed after the first dossier)

7. **VINX-04** — `WorldState::compute_consensus_root` / `compute_state_root`.
   `state_root = sha256(DST ‖ accounts_root ‖ consensus_root)`. Verify by enumerating
   every field of `WorldState` and checking it against the `ConsensusCommitment` struct:
   list any field that is neither committed nor `serde(skip)`. Then verify the encoding
   is order-stable — `banned_validator_keys` is a `HashSet` sorted before hashing;
   confirm nothing else, including anything nested, iterates non-deterministically.

8. **VINX-12** — `Transaction::signing_bytes` now length-prefixes `payload` with a `u32`
   BE, written unconditionally. Confirm the encoding is injective, and check the two
   hand-written JavaScript signing paths in `crates/vinx-node/src/rpc/ui.rs` byte for
   byte against the Rust — they were updated by hand, and a mismatch means every
   signature produced by the web UI is rejected.

9. **VINX-10** — `sync::snapshot_sync_from_peer`. Confirm the recomputed root is
   compared against `snap.block.header.state_root` (not the peer's `state_root` field),
   the height is cross-checked, and the block passes `validate_block_with_registry`.
   Confirm `is_transport_acceptable` cannot be bypassed.

10. **VX-RED-003/007** — `Storage::claim_vote` and its callers. Confirm no path releases
    a BLS signature at a height without first claiming the lock, and that the write is
    durable before the signature is published.

11. **VINX-06** — `reliability::on_block_applied`. Confirm `last_block_ts` is written on
    every block-application path (production, P2P, sync, reorg replay) and rewound
    correctly on a reorg; a stale value changes a jailing decision and therefore the
    state root.

12. **Bootstrap** — `GenesisConfig::validator_bls`, `BlsKeyFile`, and the startup
    auto-registration in `main.rs`. Confirm the BLS key is genuinely persisted across
    restarts and that a node cannot end up signing with a key that differs from the one
    registered on-chain.

13. **VINX-13 / 20 / 24** — payload bound, admin fail-closed, strict Ed25519.

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

See FINDINGS_STATUS.md, "Still open": the BLS PoP is not bound to the validator address
or `chain_id`; there are no weak-subjectivity checkpoints; a validator can bond without
a BLS key; the vote lock is per height, not per `(height, round)`; plus VINX-14, 16/22,
17/18, 19, 21, 23.

Do not report these as new. **Do** verify my claim that they are not made worse by these
patches, and check my reasoning that each requires a protocol change rather than a local
fix — if any of them can in fact be closed locally, say so and show how.

## Claims in the dossier I want checked specifically

These are assertions I made. Treat each as a hypothesis and confirm or refute it against
the code:

1. That `sign_block_bls` has always set the bitmap, so removing the empty-bitmap
   fallback costs no legitimate producer anything.
2. That requiring quorum on P2P block ingestion would stall the chain, which is why
   `verify_proposer_authenticated` deliberately does not require it.
3. That `producer.rs` always sets the proposer's own bitmap bit, so a produced block
   satisfies the authentication check it will face at its peers.
4. That the bootstrap deadlock is genuinely closed — that a fresh multi-node network can
   now reach a state where all validators are authenticable, without operator action
   beyond configuring genesis.
5. That `last_block_ts` being the last serialized field is what makes the v18→v19
   migration safe.

## Output

A table: item / was it real / is it fixed / evidence (`file:line`) / regressions found.
Then a detailed section for every regression or incomplete fix, and a final section
listing anything you could not verify and why.
