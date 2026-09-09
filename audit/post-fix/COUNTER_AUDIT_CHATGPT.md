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

## Second round of patches — attack these too

These landed after the first dossier was written. Several change consensus rules, so
they are the highest-value targets in the repository right now.

7. **VINX-04 — `state_root` now commits to consensus state**
   (`WorldState::compute_consensus_root`, `compute_state_root`).
   `state_root = sha256(DST ‖ accounts_root ‖ consensus_root)`, where `consensus_root`
   is a bincode encoding of the validator set, pool, admin key and policy, pending
   governance and upgrades, epoch beacon, supply counters and more.
   *Attack:* find a consensus field that still is **not** committed — enumerate the
   `WorldState` fields and diff them against the `ConsensusCommitment` struct; anything
   missing that is not `serde(skip)` is a hole. Then attack determinism, which is the
   real risk here: is every collection in that encoding order-stable across nodes?
   `banned_validator_keys` is a `HashSet` and is sorted before hashing — is anything
   else reachable with a non-deterministic iteration order, directly or nested inside
   `ValidatorPoolEntry`, `GovernanceProposal`, `ModuleEntry`, `ReliabilityMap`? Can a
   `serde(skip)` field influence a committed one? And check the timing: `state_root` is
   computed at a specific point in block application — can a field included in the
   commitment be mutated between the producer's call and a validator's, so the two
   compute different roots for the same block? That is a fork.

8. **VINX-12 — `signing_bytes` length-prefixes the payload**
   (`Transaction::signing_bytes`).
   `payload` is now preceded by a `u32` big-endian length, written unconditionally.
   *Attack:* is the encoding actually injective now? Try to construct any two distinct
   transactions sharing signing bytes — vary `tx_type`, the expiry flag, payload length
   boundaries, the sponsor flag. Check the `u32` cast: `self.payload.len() as u32`
   truncates above 4 GiB — is a payload that large reachable, and is
   `MAX_TX_PAYLOAD_BYTES` enforced on every path that reaches `signing_bytes`
   (including `hash()`, which the mempool and the compact-block resolver call on
   unverified input)? Also check the JS in `rpc/ui.rs`: both signing paths were updated
   by hand — do they byte-for-byte match the Rust for a non-empty payload?

9. **VINX-10 — snapshot verified against the block header, HTTPS required**
   (`sync::snapshot_sync_from_peer`, `sync::is_transport_acceptable`).
   The recomputed root must equal `snap.block.header.state_root`, the block height must
   match, and the block must pass `validate_block_with_registry` against the snapshot's
   *own* validator set.
   *Attack:* the node still trusts the snapshot's own validator set — build a fully
   self-consistent fake history (attacker-controlled validators with registered BLS
   keys, quorum-signed) and show it is adopted and marked finalized. That is the known
   residual risk; confirm whether the checks make it *harder* or are pure theatre.
   Separately, attack `is_transport_acceptable`: it parses the host with string
   splitting before an `IpAddr` parse — try IPv6 zone ids, userinfo containing `/` or
   `#`, uppercase, trailing dots (`127.0.0.1.`), decimal/octal/hex IPv4 forms
   (`http://2130706433/`), and anything reqwest would resolve differently from this
   parser. A parser/resolver mismatch is the classic SSRF-style bypass.

10. **VX-RED-003/007 — durable vote lock**
    (`Storage::claim_vote`, its callers in `p2p/mod.rs` and `node.rs`).
    The lock is claimed before signing; refusal or error means no signature.
    *Attack:* find a path that still releases a signature at a height without claiming
    the lock. `p2p/mod.rs` guards the lock behind `if let Some(storage)` — what happens
    on a node with `data_dir: None` (storage absent)? Does it then sign unconditionally,
    and is that reachable in a real deployment? In `node.rs` the lock is claimed *after*
    `produce_block` (the hash is not known before) — can a validator produce a block and
    have a competing co-signature already in flight for the same height? Is the redb
    write actually durable at the point `claim_vote` returns, or does redb buffer it?
    Is there a TOCTOU between two concurrent tasks reaching `claim_vote`? And: the
    `votes` table grows without bound — is that a disk-exhaustion vector?

11. **VINX-06 — jailing requires an elapsed slot**
    (`reliability::on_block_applied`, `SLOT_TIMEOUT_SECS`, `WorldState::last_block_ts`).
    A missed proposal is charged only when `block_ts - prev_block_ts >=
    SLOT_TIMEOUT_SECS`.
    *Attack:* the condition is now attacker-influenceable in the other direction — a
    proposer chooses its own `block_ts` within the drift bound. Can a validator inflate
    the apparent gap to jail an honest leader anyway, or suppress it to make a genuinely
    absent leader never jailed (a liveness attack: keep a faulty leader in rotation
    forever)? `last_block_ts` is set inside `settle_block` — check it is set on every
    path that applies a block (production, P2P, sync, reorg replay) and that a reorg
    rewinds it correctly; a stale `last_block_ts` after a reorg changes the jailing
    decision and therefore the state root.

12. **Bootstrap: genesis BLS registration and startup auto-registration**
    (`GenesisConfig::validator_bls`, `BlsKeyFile`, the auto-registration block in
    `main.rs`).
    *Attack:* the node auto-submits a `RegisterBlsKey` transaction at startup using the
    account nonce it reads at that moment. What happens if a transaction for that nonce
    is already in the mempool, or if the node restarts repeatedly? Can the
    auto-registration be induced to overwrite a good registration with a stale one, or
    be used as a nonce-consumption grief? `create_genesis_state` **panics** on an invalid
    PoP — is that reachable from any input an attacker controls (a genesis spec file
    fetched or supplied remotely)? A remote panic is a DoS.

13. **VINX-13/20/24 — payload bound, admin fail-closed, strict Ed25519**
    *Attack:* for VINX-20, enumerate every governance path — does any *other* handler
    still authorize with the old permissive pattern? For VINX-24, `verify_strict`
    changes which signatures are valid: is there any persisted state, test vector, or
    already-signed artefact that was valid under `verify` and is now rejected (a
    consensus split between node versions)? For VINX-13, is `MAX_TX_PAYLOAD_BYTES`
    checked before or after the expensive work in every path that accepts untrusted
    transactions?

## Also look for

- **Cross-fix interactions.** These patches were written in sequence and interact:
  the consensus commitment (VINX-04) now includes `last_block_ts` (VINX-06), which is
  written by `settle_block`; the vote lock (VX-RED-003) and proposer authentication
  (VINX-01) both gate signing. Look for an ordering or reentrancy issue that only
  appears when two of them are combined.
- Any **new** vulnerability introduced by these patches — a rejection path that turns
  into a liveness attack (can an attacker make honest nodes reject legitimate blocks?),
  a new panic, a new unbounded allocation, a lock ordering change in `p2p/mod.rs` where
  `state.read()` is now taken before `chain.write()`.
- **Deadlock**: the new `state.read().await` calls in the `NewBlock`, `SyncResponse` and
  BLS-aggregation arms. Is the lock order consistent with `admit_to_mempool` and the
  persist path?
- **Determinism**: anything that could make two honest nodes disagree.

## Known-open items — do not re-report as new

The following are confirmed, deliberately deferred, and recorded in FINDINGS_STATUS.md:

- The BLS Proof-of-Possession is still signed over `pk_bytes` alone, not bound to the
  validator address or `chain_id` (VINX-11 partial) — so it is replayable across chains.
- No weak-subjectivity checkpoints: a syncing node still trusts the snapshot's own
  validator set (VINX-10 residual).
- A validator can enter the pool without a BLS key and register one afterwards.
- The vote lock is per height, not per `(height, round)`.
- VINX-07 (SyncResponse clock drift) is fixed but has no test — the exploit needs a
  multi-node harness.
- VINX-14, 16/22, 17/18, 19, 21, 23 remain open (see FINDINGS_STATUS.md).

**Do** report if a patch made any of these worse or newly reachable, and **do** report a
concrete exploit for any of them — "still open" is not "not worth proving".

## Output

For each finding: ID, title, severity, confidence, `file:line`, attacker capability
required, the exploit sequence, impact, and a concrete test that would prove it. Then a
short section on what you tried and could not break, with the reasoning.
