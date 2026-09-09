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
| VINX-12 / (ChatGPT serialization) | `signing_bytes` payload/sponsor ambiguity | Claude, ChatGPT (as UNPROVEN) | **CONFIRMED** | P2 | **Open** — consensus-breaking, see below |
| VINX-04 | `state_root` does not commit to consensus state | Claude | **CONFIRMED** | P1 | **Open** — consensus-breaking, see below |
| VINX-06 | One byzantine validator jails the honest set | Claude | LIKELY | P2 | **Open** |
| VINX-10 | Snapshot-sync root is self-consistent, unbound to header | Claude | **CONFIRMED** | P1 | **Open** |
| VX-RED-003 / VX-RED-007 | Equivocation detected but not prevented | ChatGPT | LIKELY | P1 | **Open** |
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

## Confirmed but deliberately NOT fixed here

These are real and demonstrated, but each changes consensus or state encoding and must
land as a coordinated protocol change with a golden-vector test and an activation
height — not folded into a security patch series.

### VINX-04 — `state_root` commits only to accounts (P1)

`hash_account` is `sha256(address ‖ balance ‖ nonce ‖ staked)`, so the root covers no
consensus state at all: `validator_set`, `validator_pool` (bonds, BLS keys, PoP, VRF
keys, jail status), `admin_address`, `admin_policy`, `pending_governance`,
`pending_upgrade`, `epoch_beacon`, `base_fee`, `emitted_atoms`, `circulating_supply`,
`chain_id` and more. Two nodes can disagree on the entire validator set and the admin
key while publishing an identical `state_root`.

Proven: two `WorldState`s with identical accounts but different `validator_set`,
`admin_address` and `epoch_beacon` produced equal roots.

Since `state_root` is the only state-integrity check on every block-validation path
(`p2p/mod.rs`, `sync.rs`, `reorg.rs`), any divergence in the uncovered region is
silent. It also makes snapshot-sync unverifiable (VINX-10) and prevents light-client
verification.

Recommended: `state_root = sha256(accounts_root ‖ consensus_root)` over a deterministic
encoding of the consensus fields, locked by a golden vector.

### VINX-12 — `signing_bytes` payload/sponsor ambiguity (P2)

`payload` is appended with no length prefix, immediately followed by the sponsor
marker (`0x00`, or `0x01 ‖ sponsor[20]`). The encoding is not injective. Proven: for any
sponsor address whose last byte is `0x00` (1 in 256), a sponsored transaction and an
unsponsored one with a re-cut payload produce **identical signing bytes and an identical
txid**, so one signature is valid for two economically different transactions — the fee
payer changes from the sponsor to the sender.

ChatGPT flagged the missing framing and correctly declined to call it proven without a
collision; the collision exists and is cheap to construct.

Recommended: length-prefix `payload` (and any other variable-length field) in
`signing_bytes`. This changes every transaction hash, so it is a hard fork.

### VINX-10 — snapshot-sync trusts a self-consistent root (P1)

`snapshot_sync_from_peer` compares the recomputed root against `snap.state_root`, a
field from the *same peer* that sent the state — it proves only that the peer can hash
what it just sent. It is never compared against `snap.block.header.state_root`, which
is present in the response and free to check, and `Chain::new_from_snapshot` then marks
the snapshot block **finalized** with no quorum verification. With VINX-04, even a
correct check would not cover the validator set or admin key.

Recommended: require `computed_root == snap.block.header.state_root`, validate the
snapshot block against the registry, ship trusted checkpoints with the binary
(weak subjectivity), and require HTTPS for `sync_peer_rpc`.

### VX-RED-003 / VX-RED-007 — equivocation is detected, not prevented (P1)

`record_signature()` returns `true` when a validator has already signed a different hash
at the same height, but the result is used as telemetry: nothing stops the second
signature. The invariant "one validator, one vote per height" is not enforced, and an
attacker sending two valid competing blocks can make honest validators co-sign both
branches.

Recommended: a persistent, atomic, per-height (ideally per `(height, round)`) vote lock,
consulted *before* signing and surviving restarts and local reorgs. This needs
durable storage design and is not a local patch.

### VINX-06 — one byzantine validator jails the honest set (LIKELY, P2)

`on_block_applied` charges a missed proposal to the scheduled leader whenever the actual
proposer differs, and the P2P path accepts any set member as proposer with no slot
timeout — so a validator that consistently pre-empts the leader jails the whole honest
set after three rounds. Marked LIKELY rather than CONFIRMED: not reproduced here.
Note the mitigation Claude's report itself records — finality quorum is computed over
the full set, so this breaks liveness and fairness, not safety.

Recommended: charge a miss only after a verifiable slot timeout derived from the header
timestamp, and reject a non-leader block proposed before that threshold.

## Not fixed, lower priority

VINX-13 (unbounded tx `payload`), VINX-14 (governance `ScheduleUpgrade` bypasses the
ADR 0006 notice delay), VINX-15 (node private keys written with default permissions),
VINX-16/VINX-22 (unbounded in-memory candidate and cooldown maps), VINX-17/VINX-18
(ECVRF `validate_key` absent, committee selection not wired), VINX-19 (web UI handles a
private key under a CDN script with no SRI/CSP), VINX-20 (admin authority fail-open when
no admin is configured), VINX-21 (monotonically decreasing peer reputation),
VINX-23 (`try_evict_for` O(n) per insertion), VINX-24 (non-strict Ed25519 verification).

VINX-15, VINX-19 and VINX-24 are cheap and worth doing next; the rest are hardening.

## DEPLOYMENT PREREQUISITE — read before deploying this branch

**On-chain BLS key registration is now required for a block to be accepted by peers.**

`verify_proposer_authenticated` (VINX-01) verifies the proposer's co-signature against
its key in the on-chain registry. A proposer whose key is not registered cannot be
authenticated — you cannot verify authorship against a key you do not have — so its
blocks are refused. That is the only secure behaviour available, but it has an
operational consequence that must not be discovered in production.

Current state of the repository:

* `create_genesis_state` registers **no** BLS keys. `validator_pool` entries start with
  `bls_pub_key: None`.
* There is **no way to submit a `RegisterBlsKey` transaction**: no node startup path, no
  wallet CLI subcommand. `grep -rn "RegisterBlsKey" crates/` returns only the type
  definition, the state handler and tests.

So on a fresh multi-node network the registry stays empty, and with this branch peers
reject every block. The producer still appends to its *own* chain (`producer.rs` does not
call the check), so a single-node devnet is unaffected — but a multi-node network cannot
make progress, and the transaction needed to fix that cannot be included in a block that
peers accept. That is a bootstrap deadlock.

This requirement is **partly pre-existing**: `sync.rs:129` and `sync.rs:362` already
called `validate_block_with_registry`, and since `producer.rs` always sets the proposer's
bitmap bit, that path already failed against an empty registry. The sync path was
therefore already broken in this configuration. What changed is that the requirement now
also applies to the main `NewBlock` gossip path, so the gap became load-bearing instead
of latent.

Pinned by `audit_regression_node.rs::unregistered_proposer_is_refused_and_registration_is_a_prerequisite`.

**Before deploying, one of these must land:**

1. Add the initial validators' BLS public keys to `GenesisConfig` and register them in
   `create_genesis_state`; and/or
2. Require a BLS key at bonding time (`Stake` / validator entry), so a validator cannot
   join the set without one; and
3. Ship the missing tooling — a wallet subcommand and/or node startup auto-registration —
   so an existing validator can register.

Option 1 or 2 is the real fix: a validator that cannot be authenticated should not be in
the active set. Both are protocol changes and are deliberately out of scope for this
security series, but this is a **launch blocker**, not hardening.

## Needs human review

1. **`apps/vinx-desktop/src-tauri`** — excluded from the workspace and not covered by
   `cargo test --workspace`. Gemini's `export_wallet` finding is fabricated, but the
   real IPC surface, CSP and key handling still need a first-hand review.
2. **The three open consensus items above** (VINX-04, VINX-10, VX-RED-003) require a
   protocol decision, not just a patch.
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
