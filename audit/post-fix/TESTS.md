# TESTS

## Commands

```
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets
cargo test --workspace
```

## Results at HEAD

| Command | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo check --workspace --all-targets` | 0 errors |
| `cargo clippy --workspace --all-targets` | 0 errors (warnings pre-existing, chiefly `clone` on `Copy` `Address`) |
| `cargo test --workspace` | **397 passed, 0 failed** |

Every commit in the series was also checked out individually and verified to build
(`cargo check --workspace --all-targets`) and pass `cargo test --workspace` on its own,
so the history is bisectable and each fix is independently revertible.

## Proof-of-concept phase (before any fix)

Six PoCs were written against `8774806` and **all six passed**, i.e. each attack worked:

| PoC | Finding | Result on `8774806` |
|---|---|---|
| `poc_sponsor_signature_never_checked` | VINX-03 | PASS — victim drained 5 000 → 0 VINX |
| `poc_signing_bytes_payload_sponsor_ambiguity` | VINX-12 | PASS — identical signing bytes and txid |
| `poc_state_root_does_not_commit_to_consensus_state` | VINX-04 | PASS — equal roots, different validator sets |
| `poc_forged_quorum_via_empty_bitmap` | VINX-02 | PASS — quorum with an empty registry |
| `poc_fork_choice_weight_is_attacker_controlled` | VINX-05 | PASS — 50 foreign keys won and finalized |
| `poc_reorg_replace_panics_on_snapshot_synced_chain` | VINX-08 | PASS — panic "start index out of range" |

## Regression tests added

Each carries the same attack construction as its PoC and asserts it is now refused.
They run in the normal suite (not `#[ignore]`d), so a revert fails CI.

`crates/vinx-state/tests/audit_regression.rs`

| Test | Covers |
|---|---|
| `sponsor_without_signature_cannot_drain_an_account` | VINX-03 — the drain, refused at admission |
| `sponsor_with_forged_signature_is_rejected` | VINX-03 — wrong-key sponsor signature |
| `genuinely_sponsored_transaction_still_applies` | VINX-03 — the feature still works |
| `bls_key_cannot_be_registered_by_two_validators` | VINX-11 — key uniqueness |

`crates/vinx-node/tests/audit_regression_node.rs`

| Test | Covers |
|---|---|
| `forged_quorum_via_empty_bitmap_is_rejected` | VINX-02 — empty bitmap never reaches quorum |
| `fork_choice_ignores_unregistered_cosigners` | VINX-05 — honest block wins; forged does not finalize |
| `proposer_impersonation_is_rejected` | VINX-01 — unsigned / foreign-key / wrong-index blocks refused, genuine block accepted |
| `reorg_replace_is_height_base_aware` | VINX-08 — reorg succeeds, `height_base` preserved |
| `unregistered_proposer_is_refused_and_registration_is_a_prerequisite` | Pins the deployment prerequisite the VINX-01 fix creates |

`crates/vinx-core/src/block.rs` (unit)

| Test | Covers |
|---|---|
| `test_empty_bitmap_does_not_fall_back_to_cosigner_pks` | VINX-02 — the fallback is gone |
| `test_empty_bitmap_without_aggregate_counts_zero` | VINX-02 — no aggregate ⇒ 0 signers |
| `test_duplicate_registered_key_across_slots_rejected` | VINX-11 — one signature ≠ two signers |

## Existing tests updated (not weakened)

`bench_n3.rs`, `consensus.rs` and `chain.rs` tests were migrated from the deleted
`validate_block` and from `bls_signer_count()` to the registry-bound path. Test helpers
now generate deterministic per-index BLS keys and build a matching registry — mirroring
what a real validator does (register on-chain, then sign). The quorum, round-robin,
prefix-closed finality, safety-below-quorum and fork-choice-convergence assertions are
unchanged in substance; they now run against the registry, which is strictly stronger.

## Coverage gaps, stated plainly

* **VINX-07** has no test. Moving the Median Time Past requires six consecutive accepted
  blocks and a full node harness. The fix was established by comparing the four
  ingestion paths, which are now identical; the exploit itself is unverified here.
* **VINX-09** has no timing assertion — a wall-clock threshold is a flaky test. The
  bound is a structural precondition verified by inspection.
* **VINX-04, VINX-10, VINX-12, VX-RED-003, VINX-06** remain open and therefore have no
  passing regression test. Their PoCs demonstrated the flaw and are reproduced in
  BEFORE.md; they were not committed as `#[ignore]`d tests because a permanently failing
  test in the tree is noise. FINDINGS_STATUS.md records what each still needs.
* **No multi-node adversarial testing** was performed. Everything here is unit and
  integration level within one process.
