# FINDINGS_STATUS — consolidated matrix

**Audited commit:** `8774806` (branch `main`)
**Fix branch:** `claude/vinx-ledger-audit-verify-ya6rl5`
**Toolchain:** rustc 1.94.1 / cargo 1.94.1
**Workspace:** `vinx-crypto`, `vinx-core`, `vinx-state`, `vinx-node`, `vinx-wallet`,
`vinx-desktop-core` (the Tauri shell `apps/vinx-desktop/src-tauri` is a separate,
excluded workspace).

Every verdict below was reached by reading the code at `8774806`, not by counting how
many reports mentioned it. Six findings were turned into executable proofs before any
fix was written; all six passed on the audited commit, i.e. each attack worked.

## Status legend

| Status | Meaning |
|---|---|
| CONFIRMED | Reproduced from the code, exploit path demonstrated |
| LIKELY | Code defect established; full exploit needs a harness not built here |
| UNPROVEN | Cannot be established or refuted from the code alone |
| FALSE POSITIVE | The described code does not exist, or the behaviour is already safe |

## Matrix

| ID | Finding | Sources | Status | Prio | Action |
|---|---|---|---|---|---|
| VINX-02 / VX-RED-001 / — | BLS quorum forgeable via empty `bls_bitmap` fallback | Claude, ChatGPT | **CONFIRMED** | P0 | Fixed `b361343` |
| VINX-03 / VX-RED-005 / FINDING-01 | Sponsor signature never verified → any account drained | Claude, ChatGPT, (Gemini, wrong file) | **CONFIRMED** | P0 | Fixed `ec1127a` |
| VINX-01 / VX-RED-002 | No block/proposer authentication on any P2P path | Claude, ChatGPT | **CONFIRMED** | P0 | Fixed `fbc5e73` |
| VINX-05 / VX-RED-004 | Fork-choice weight & finality from unverified co-signer count | Claude, ChatGPT | **CONFIRMED** | P1 | Fixed `b361343` |
| VINX-08 | `reorg_replace` ignores `height_base` → remote node panic | Claude | **CONFIRMED** | P1 | Fixed `728736d` |
| VINX-07 | `SyncResponse` lacks the clock-drift bound → supply inflation | Claude | **CONFIRMED** | P1 | Fixed `fbc5e73` |
| VINX-09 | CompactBlock resolution unbounded → remote CPU DoS | Claude | **CONFIRMED** | P2 | Fixed `74e3c69` + `fbc5e73` |
| VINX-11 | BLS PoP not bound to identity; key re-registrable | Claude | **CONFIRMED** | P2 | Partially fixed `8627ffc` + `b361343` |
| VINX-12 / (ChatGPT serialization) | `signing_bytes` payload/sponsor ambiguity | Claude, ChatGPT (as UNPROVEN) | **CONFIRMED** | P2 | Fixed `0824b7b` |
| VINX-04 | `state_root` does not commit to consensus state | Claude | **CONFIRMED** | P1 | Fixed `1c75312` |
| VINX-06 | One byzantine validator jails the honest set | Claude | **CONFIRMED** | P2 | Fixed `86f0b05` |
| VINX-10 | Snapshot-sync root is self-consistent, unbound to header | Claude | **CONFIRMED** | P1 | Fixed `26df4a7` |
| VX-RED-003 / VX-RED-007 | Equivocation detected but not prevented | ChatGPT | **CONFIRMED** | P1 | Fixed `bc7fb8e` |
| VX-RED-008 | Reorg relies on a `trusted` precondition | ChatGPT (self-marked UNPROVEN) | UNPROVEN | P3 | No action |
| VX-RED-009 | Deferred crypto lets txs accumulate before verification | ChatGPT (self-marked partial) | LIKELY | P3 | Mitigated by `MAX_NONCE_AHEAD` |
| VX-RED-010 / FINDING-04 | P2P Sybil / rate-limit bypass; mempool deadlock | ChatGPT (UNPROVEN), Gemini (UNPROVEN) | UNPROVEN | P3 | No action |
| FINDING-01 (Gemini) | `apply_transaction` omits signature check | Gemini | **FALSE POSITIVE** | — | See below |
| FINDING-02 (Gemini) | `chain_id` missing from `Transaction::hash()` | Gemini | **FALSE POSITIVE** | — | See below |
| FINDING-03 (Gemini) | Integer underflow on balances | Gemini (self-marked FP) | **FALSE POSITIVE** | — | Correctly refuted by Gemini |
| FINDING-05 (Gemini) | `export_wallet` Tauri IPC leaks private key | Gemini, Claude (as VINX-19) | **FALSE POSITIVE** (as described) | — | See below |
| FINDING-06 (Gemini) | `bincode` unbounded allocation in `codec.rs` | Gemini | **FALSE POSITIVE** (as described) | — | See below |
| FINDING-07 (Gemini) | `StdRng::seed_from_u64(now.as_secs())` key generation | Gemini | **FALSE POSITIVE** | — | See below |
| FINDING-08 (Gemini) | Timing attack in `verify_auth_token` | Gemini | **FALSE POSITIVE** | — | See below |

## On the Gemini report

Gemini's report is presented with high confidence ("CONFIRMED", "Critique") and quotes
Rust source verbatim. **None of the code it quotes exists in this repository.** The
files it names are absent:

```
crates/vinx-core/src/ledger.rs      ABSENT
crates/vinx-core/src/crypto.rs      ABSENT
crates/vinx-core/src/auth.rs        ABSENT
crates/vinx-core/src/state.rs       ABSENT
crates/vinx-network/src/codec.rs    ABSENT  (no vinx-network crate exists)
crates/vinx-network/src/mempool.rs  ABSENT
```

So are the functions: `verify_auth_token`, `transfer_credits`, `export_wallet`,
`decode_message`, `add_tx_and_sync`, `Keypair::generate` — zero matches across the
workspace.

The two "new" findings Gemini claims the other auditors missed are both refuted
directly:

* **FINDING-07 (weak PRNG, "Critique").** Key generation uses `OsRng`, not a
  time-seeded `StdRng`:
  `crates/vinx-crypto/src/keys.rs:39` — `SigningKey::generate(&mut OsRng)`.
  `seed_from_u64` appears nowhere in the workspace. The quoted `Keypair::generate`
  body is fabricated.
* **FINDING-08 (timing attack).** `verify_auth_token` does not exist. There is no
  auth-token comparison to attack.

FINDING-01 and FINDING-02 are wrong *as written* (`apply_transaction` does verify
signatures; `signing_bytes` does include `chain_id`, at
`crates/vinx-core/src/transaction.rs:153`) — but FINDING-01 gestures at a real problem
that Claude and ChatGPT located precisely: the *sponsor* signature was unverified, in
`world_state.rs` / `mempool.rs`, not in a `ledger.rs` that does not exist. Credit for
that finding belongs to the reports that identified the actual code.

FINDING-05 and FINDING-06 describe plausible classes of bug against invented code.
The real Tauri shell should still be reviewed on its own terms (see HUMAN REVIEW), and
the real decoder does bound decompression to 16 MiB
(`crates/vinx-node/src/p2p/messages.rs`), which ChatGPT correctly noted as a positive.

**Conclusion:** Gemini's verdicts were not usable as evidence. Its one methodological
contribution was correctly refuting FINDING-03, which Claude and ChatGPT had both
reported — a reminder that the refutations in these reports deserve the same
independent check as the accusations.

## Still open

### VINX-11 (partial) — the PoP is not bound to an identity

Duplicate registration is now refused and duplicate keys are rejected among a block's
signers, so one signature can no longer count twice. But the Proof-of-Possession is
still signed over `pk_bytes` alone, not over `bls_pub_key ‖ validator_address ‖
chain_id`. It therefore remains replayable across chains and carries no binding to the
registering validator. Closing this changes the PoP format — a protocol change, and the
right moment is alongside a BLS-at-bonding requirement.

### Weak subjectivity for snapshot-sync

VINX-10 now verifies the snapshot against its block header and requires a real quorum
from the snapshot's own validator set. A syncing node still trusts that validator set.
The real answer is trusted checkpoints (header hash + height) shipped with the binary,
plus cross-checking the snapshot against several independent peers. Neither is
implemented.

### BLS key required at bonding time

A validator can enter the pool without a BLS key and only then register one. Requiring
the key as part of bonding would make "every active validator is authenticable" a
structural invariant rather than a convergence process.

### Per-round vote locking

The vote lock is per height. If the protocol adopts explicit rounds, it must become
per `(height, round)`.

## Not fixed, lower priority

Fixed since: VINX-13 (unbounded tx `payload`, `07191f3`), VINX-15 (key files written
world-readable, `48f5cb6`), VINX-20 (admin authority fail-open, `07191f3`),
VINX-24 (non-strict Ed25519 verification, `07191f3`).

Still open: VINX-14 (governance `ScheduleUpgrade` bypasses the ADR 0006 notice delay),
VINX-16/VINX-22 (unbounded in-memory candidate and cooldown maps), VINX-17/VINX-18
(ECVRF `validate_key` absent, committee selection not wired), VINX-19 (web UI handles a
private key under a CDN script with no SRI/CSP), VINX-21 (monotonically decreasing peer
reputation), VINX-23 (`try_evict_for` O(n) per insertion).

VINX-19 and VINX-14 are the two worth doing next; the rest are performance hardening.

## Bootstrap prerequisite — RESOLVED (`48f5cb6`)

The proposer-authentication fix (VINX-01) made on-chain BLS registration a precondition
for a block to be accepted. At the time it was introduced, `create_genesis_state`
registered no BLS keys and nothing in the codebase could submit a `RegisterBlsKey`
transaction, so on a fresh multi-node network peers would have refused every block —
and the transaction that fixes that can only travel inside a block peers accept. A
bootstrap deadlock, introduced by this series and recorded here as a launch blocker.

It is now closed:

* `GenesisConfig::validator_bls` registers the genesis validator's key (PoP verified) so
  it is authenticable from height 1; `GenesisSpec` carries it for multi-node testnets.
* The node auto-submits its own `RegisterBlsKey` at startup when it is bonded and its
  registered key is missing or stale, so later validators converge without operator
  action. The genesis validator produces the blocks that carry those registrations.
* The validator BLS key is now persisted (`<data_dir>/validator_bls.json`). It was
  previously regenerated on every `NodeConfig::new`, which — once blocks are
  authenticated against a registered key — would have had every node's blocks refused
  after its first restart.

Pinned by `audit_regression.rs::genesis_registers_the_validator_bls_key` and
`::genesis_rejects_an_invalid_bls_pop`.

Still an operational requirement, just no longer a deadlock: **a validator must have a
registered BLS key before its blocks are accepted.** Requiring one at bonding time, so a
validator cannot enter the active set without it, remains the stronger design and is
listed below.

## Needs human review

1. **`apps/vinx-desktop/src-tauri`** — excluded from the workspace and not covered by
   `cargo test --workspace`. Gemini's `export_wallet` finding is fabricated, but the
   real IPC surface, CSP and key handling still need a first-hand review.
2. **The consensus changes in this series are hard forks** — `state_root` now commits
   to consensus state (VINX-04) and `signing_bytes` length-prefixes the payload
   (VINX-12), so every state root and every transaction hash changes. This is
   deliberate and safe only because the chain is pre-launch at `0.1.0-alpha.1` with no
   live network to migrate. Confirm that assumption before merging anywhere that has
   real state.
3. **The deployment prerequisite above** — decide between genesis registration and
   bonding-time enforcement, and ship the registration tooling. Nothing else in this
   series changes network behaviour as much as this does.
4. **Multi-node adversarial testing.** Everything here is unit- and integration-level.
   ChatGPT's recommendation stands: a 4–7 validator harness with controlled partitions,
   asserting the invariants it lists, is the only way to validate consensus safety.
5. **`with_sponsor` ordering.** Attaching a sponsor changes `signing_bytes`, so a
   transaction built with `new_transfer(..).with_sponsor(..)` alone has an invalid
   *sender* signature. Documented in this series, but any existing client SDK that
   builds sponsored transactions this way is producing invalid transactions.
