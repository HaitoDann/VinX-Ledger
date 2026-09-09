# Counter-audit prompt — ChatGPT (adversarial / break the patch)

You are a red-team engineer. A previous audit round found vulnerabilities in
VinX-Ledger; they have now been patched. **Your job is to break the patches**, not to
confirm them.

Do not answer "the fix looks correct". A reply that only validates the patch is a failed
review. If you genuinely cannot break something, say precisely what you tried, which code
path you followed, and what would have to be true for an attack to work.

## Repository

`HaitoDann/VinX-Ledger`, branch `claude/vinx-ledger-audit-verify-ya6rl5`.
Audited commit: `8774806`. Read `audit/post-fix/BEFORE.md`, `AFTER.md`, `PATCH.md`,
`TESTS.md` and `FINDINGS_STATUS.md` first, then go to the source. **Treat those documents
as claims, not as evidence.** Verify every assertion against the code.

## Ground rules

1. Every claim must cite `file:line` at the branch head. A finding I cannot locate in the
   code, I will discard.
2. If you assert an exploit, give the concrete sequence: attacker capability, message or
   transaction contents, the functions traversed in order, and the resulting state.
3. Distinguish what you proved from what you suspect. Label each finding CONFIRMED,
   LIKELY or UNPROVEN and say what evidence is missing for the weaker ones.
4. Inventing a filename or function is the worst possible outcome. One of the three
   original reports quoted `crates/vinx-core/src/crypto.rs`, `auth.rs`, `ledger.rs` and a
   `vinx-network` crate — none of which exist — and was worthless as a result. If you are
   not sure a symbol exists, grep for it and say so.

## Patches to attack

Each fix, and what it claims. Attack the claim.

1. **VINX-02 — BLS fallback removed** (`crates/vinx-core/src/block.rs`).
   `bls_signer_count_from_bitmap` no longer falls back to `bls_cosigner_pks` when
   `bls_bitmap` is empty; an empty bitmap with an aggregate is an error, without one it
   counts zero. Duplicate G1 keys among reconstructed signers are rejected.
   *Attack:* is there any other route to a signer count not bound to the registry? Can a
   crafted bitmap (over-long, bits beyond `indexed_bls_pks.len()`, all-zero bytes but
   non-empty vector) inflate or bypass the count? Does `bls_signer_count_unverified`
   still reach a security decision anywhere — check `rpc/types.rs` and every remaining
   caller. Is `EmptyAggregate` handled as a rejection everywhere it can now surface, or
   does some caller `unwrap_or(0)` it into silent acceptance?

2. **VINX-05 — fork-choice and finality bound to the registry**
   (`consensus::signer_weight`, `Chain::advance_finality`, `canonical_choice`,
   `would_reorg_at`).
   *Attack:* the registry is read from a `WorldState` at the call site. Which state, at
   which height? In `reorg::consider_candidate` it is read from the state *before* the
   replay. Can a validator-set or BLS-registry change between the candidate's height and
   the reading point make two nodes compute different weights for the same block — a
   fork-choice divergence, hence a fork? Is `more_canonical` still a total order under
   the new weight function?

3. **VINX-01 — proposer authentication**
   (`consensus::verify_proposer_authenticated`, wired into the `NewBlock` and
   `SyncResponse` arms of `p2p/mod.rs`).
   The check: the proposer's bit is set in `bls_bitmap`, and the aggregate verifies
   against the registered keys of the set bits. Quorum is deliberately *not* required.
   *Attack:* can a block be replayed or adapted — same aggregate, different header — and
   still authenticate? What if the proposer has no registered BLS key (`None` in the
   registry): is the block rejected, or does some path treat it as exempt and thereby
   allow unauthenticated blocks during the pre-registration window? Are there ingestion
   paths I missed — `sync.rs`, the `TxResponse` path, `reorg`, storage replay at startup?
   Does the new `CompactBlock` wire format (now carrying `bls_aggregate` / `bls_bitmap`
   with `#[serde(default)]`) admit a downgrade: an old-format message decodes with an
   empty bitmap — is it then rejected, or silently accepted somewhere?

4. **VINX-03 — sponsor signature** (`mempool.rs`, `rpc/handlers.rs`, `admission_check`).
   All three entry points now call `WorldState::verify_tx_signature_pure`.
   *Attack:* is there a fourth way into the mempool or into a produced block? Trace
   `apply_transaction_trusted` callers — `producer.rs`, `reorg.rs`, `sync.rs`,
   `p2p/mod.rs` — and check each has verified the sponsor first. `fee` still has no upper
   bound: with the signature now required, what can a *consenting* sponsor be tricked
   into? Consider the interaction with VINX-12 (still open): the payload/sponsor
   ambiguity means one signature can be valid for two transactions — can that turn a
   legitimately sponsored transaction into a different one?

5. **VINX-08 — `height_base` indexing** (`chain.rs`).
   *Attack:* audit every other index into `Chain::blocks`. Are there remaining sites
   using absolute height as an index? Does `reorg_replace` returning an empty `Vec` on
   an out-of-range height leave the caller (`reorg::consider_candidate`) in a state where
   it believes a reorg happened when it did not?

6. **VINX-09 — CompactBlock bounds** and **VINX-11 — BLS key uniqueness**.
   *Attack:* for VINX-09, is `header.tx_count` itself bounded before use? For VINX-11,
   the PoP is still not bound to the validator address or chain_id — what does that still
   allow now that duplicate registration is refused (consider re-registration after a
   validator exits, and cross-chain PoP replay)?

## Also look for

- Any **new** vulnerability introduced by these patches — a rejection path that turns
  into a liveness attack (can an attacker make honest nodes reject legitimate blocks?),
  a new panic, a new unbounded allocation, a lock ordering change in `p2p/mod.rs` where
  `state.read()` is now taken before `chain.write()`.
- **Deadlock**: the new `state.read().await` calls in the `NewBlock`, `SyncResponse` and
  BLS-aggregation arms. Is the lock order consistent with `admit_to_mempool` and the
  persist path?
- **Determinism**: anything that could make two honest nodes disagree.

## Known-open items — do not re-report as new

VINX-04 (`state_root` does not commit to consensus state), VINX-10 (snapshot-sync root),
VINX-12 (`signing_bytes` ambiguity), VX-RED-003/007 (equivocation detected not
prevented), VINX-06 (jailing). These are confirmed and deliberately deferred; see
FINDINGS_STATUS.md. **Do** report if a patch here made any of them worse or newly
reachable.

## Output

For each finding: ID, title, severity, confidence, `file:line`, attacker capability
required, the exploit sequence, impact, and a concrete test that would prove it. Then a
short section on what you tried and could not break, with the reasoning.
