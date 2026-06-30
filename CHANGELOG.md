# Changelog

All notable changes to VinX Ledger are documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)  
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

---

## [Unreleased]

### Performance optimisations — session 2026-06-30

A series of targeted optimisations were applied to the node, mempool, state,
and P2P layers. No breaking protocol change; schema version bumped to v3.

#### Storage & Persistence
- **Block pruning** (`BLOCK_RETENTION_COUNT = 100 000`, `PRUNE_INTERVAL = 1 000`) —
  blocks older than the retention window are compacted: transaction lists and
  validator signatures are dropped, only headers are kept. Slash-evidence
  entries older than the window are also removed. Triggered automatically
  every 1 000 blocks during block production.
  New helpers: `Chain::prune()`, `Chain::compact_old_txs()`.

- **Persistent transaction index** — `tx_index` and `account_tx_index` are
  now serialised alongside state and chain at every persist cycle (schema v3).
  On load, indexes are restored in O(1) via `import_tx_indexes()`; a full
  O(n) `rebuild_tx_index()` is kept as fallback for older snapshots.

- **Asynchronous persistence** — serialisation now occurs while holding the
  read locks; locks are released before any disk I/O. The compressed blobs
  are written to redb inside a `tokio::task::spawn_blocking` call, completely
  decoupling the critical path from storage latency.
  New API: `Storage::serialize()` (pure, no I/O) + `Storage::save_serialized()`
  (I/O only, callable from any thread). `Storage` is now `Clone` via `Arc<Database>`.

- **zstd compression** (level 3 storage / level 1 P2P wire) — all redb blobs
  (state, chain, tx-index) are compressed with zstd before write and
  decompressed on read. P2P messages above 512 bytes are also compressed at
  level 1 with a one-byte flag header (`0x00` = raw, `0x01` = zstd).
  Typical reduction: 60-75 % on serialised state, 40-60 % on P2P gossip.

#### State Layer
- **Lazy Merkle root cache** — `WorldState` now holds an `Option<Hash32>` root
  cache (`#[serde(skip)]`). `compute_state_root()` returns the cached value
  immediately on repeated calls; the cache is invalidated (`None`) only on
  actual mutations: `credit()`, `apply_transaction()`, `check_auto_unfreeze()`,
  `distribute_staking_rewards()`. Eliminates redundant full Merkle recomputes
  on every RPC read within the same epoch.

#### Mempool
- **Bloom-filter P2P deduplication** — a `bloomfilter::Bloom<Hash32>` (1 % FP
  rate at 2× capacity) pre-screens incoming gossiped transactions in `stage()`.
  Transactions whose hash hits the bloom filter skip expensive signature
  verification. Correctness is guaranteed by the exact `HashSet<Hash32>`
  (`seen`) that remains the authoritative dedup store.

- **Fee-based eviction** — when the mempool is at capacity (`max_size`), a
  newly admitted high-fee transaction can evict the pending transaction with
  the lowest fee across all queues (`try_evict_for()`). Transactions with equal
  or lower fees are rejected with `MempoolError::Full`.

- **Nonce validation at admission** — `Mempool` tracks `min_nonce: HashMap<String, u64>`,
  the minimum acceptable nonce per sender, updated after every produced block
  via `update_confirmed_nonces()`. Transactions with `nonce < min_nonce` are
  rejected immediately with `MempoolError::StaleNonce`, before any lock
  acquisition or signature check. Same-nonce replacement is allowed only when
  the incoming fee is strictly higher (fee-bump); otherwise the submission
  returns `MempoolError::NonceTaken`. `update_confirmed_nonces()` also
  atomically prunes stale pending entries and rebuilds the bloom filter.

- **Fee-bump** — a pending transaction can be replaced at the same nonce if
  the replacement carries a strictly higher fee. The old entry is evicted from
  both `seen` and the bloom filter; `pending_count` is unchanged.

#### P2P
- **Parallel signature verification** — `NewBlock` and `SyncResponse` handlers
  now verify all validator signatures concurrently using `rayon::par_iter()`.
  Each worker checks `Address::from_public_key(pub_key) == validator` then
  calls `pub_key.verify(block_hash, signature)`. Verification throughput scales
  linearly with available CPU cores.

#### RPC / Node
- **LRU address cache** — bech32 address parsing results are cached in a
  `lru::LruCache<String, Address>` (capacity 1 024, behind a `tokio::Mutex`).
  All five RPC handlers that previously decoded bech32 inline now go through
  `Node::parse_address()`.

- **LRU receipt cache** — transaction receipts are kept in an
  `lru::LruCache<String, TxReceipt>` (capacity 100 000, behind an `Arc<RwLock>`).
  The `GET /tx/:hash` handler writes to the cache on first access and returns
  the cached value on subsequent lookups.

### Changed
- `WorldState::compute_state_root` signature changed from `&self` to `&mut self`
  to support the lazy cache write.
- `Storage` constructor now wraps `redb::Database` in `Arc`; `Storage` derives
  `Clone`.
- Schema version bumped from v2 → v3 (adds tx-index blobs, zstd compression).

### Fixed
- Bloom filter is correctly rebuilt after `update_confirmed_nonces()` prunes
  stale entries, preventing false-negatives on reused tx hashes.
- `persist()` awaits the `spawn_blocking` handle, ensuring the `Arc<Database>`
  is fully released before the node is dropped (fixes `DatabaseAlreadyOpen`
  in crash-recovery integration test).

---

## [0.1.0-alpha.1] — 2026-06-13

First internal testnet release. All core protocol modules are in place and
passing 207 automated tests (unit, integration, proptest invariants).

### Added

#### Protocol
- **PoA Threshold consensus** — round-robin leader, quorum ⌈2n/3⌉, Ed25519 signatures
- **Dynamic base fee** — EIP-1559-style, adjusts ±12.5 % per block based on utilisation
- **Fee split** — 40 % staking pool / 30 % validator pool / 30 % melt pool
- **Supply model** — 21 M VinX admin + 99.979 B Coffre Maturité = 100 B max
- **SHA-256 Merkle tree** — account state root, inclusion proofs via `/account/:addr/proof`
- **Slashing** — equivocation detection with configurable slash ratio
- **Bech32 addresses** (`vinx1…`) derived from Ed25519 public keys

#### Node (`vinx-node`)
- Axum HTTP/RPC server — `GET /status`, `/block/:n`, `/tx/:hash`, `/account/:addr`,
  `/mempool`, `/health`, `/metrics`, `/ws` (WebSocket)
- Testnet faucet — `POST /faucet/request`, per-address 24 h cooldown, configurable amount
- Merkle proof endpoint — `GET /account/:addr/proof` → `{state_root, proof[]}`
- Rate limiting (100 req/min per IP) and `admin_token` authentication for sensitive routes
- Prometheus metrics — `vinx_chain_height`, `vinx_mempool_size`, `vinx_base_fee`,
  `vinx_staking_pool`, `vinx_melt_pool`, `vinx_distribution_pool`,
  `vinx_circulating_supply`, `vinx_validator_count`
- SSE endpoint for real-time block push to the explorer
- `--data-dir` persistence + crash-recovery (confirmed by integration test)
- Config file (`config.toml`) for faucet key, amounts, cooldowns, admin token

#### P2P (`libp2p`)
- Gossipsub block propagation
- mDNS peer discovery (LAN)
- `--sync-peer` HTTP bootstrap sync for new nodes joining a live testnet

#### Wallet (`vinx-wallet`)
- HD wallet derivation (BIP-32-compatible, Ed25519)
- Keystore with password-based encryption
- CLI: `keygen`, `address`, `send`, `balance`, `stake`, `unstake`

#### Infrastructure
- **3-validator Docker testnet** — `docker compose up --build`
- **Caddy TLS reverse proxy** — self-signed local cert (devnet), automatic Let's Encrypt (VPS)
  - CORS pre-flight handling, SSE/WebSocket keep-alive, HTTP → HTTPS redirect
- **Prometheus + Grafana stack** — auto-provisioned dashboard, 15 s scrape,
  panels: chain height, mempool size, base fee, all pool balances over time
- CI pipeline (GitHub Actions) — `cargo test`, `cargo clippy -D warnings`, `cargo fmt --check`,
  TypeScript SDK tests

#### SDK (`vinx-sdk` — TypeScript)
- `VinxClient` class — `getStatus()`, `getBlock()`, `getTx()`, `getAccount()`,
  `sendRawTx()`, `getMempool()`, `accountProof()`, `faucetRequest()`
- `verifyMerkleProof(leafHash, proof, stateRoot)` — browser-native SHA-256 via `crypto.subtle`
- Jest test suite (19 tests)

#### Block Explorer (built-in UI at `/`)
- Two-column layout: network stats + economy card
- Fee history canvas chart (last 30 blocks, green → orange gradient)
- Faucet form integrated in the economy card
- Account lookup with paginated TX history (10 TX/page)
- Block explorer with clickable TX rows
- TX lookup with clickable addresses and block height links
- SSE real-time updates + periodic refresh (15 s / 30 s)

#### Testing
- 207 tests — 0 failures
  - Unit tests across all crates
  - Integration tests: faucet endpoint, crash recovery, multi-validator, fee dynamics
  - **11 proptest invariants**: fee conservation, Merkle determinism, supply conservation,
    transfer nonce increment, wrong-nonce rejection

### Known Limitations (alpha)
- No slashing-evidence gossip between peers (evidence is local only)
- Validator set is static (defined at genesis); no on-chain rotation yet
- P2P sync is pull-based (new node pulls from `--sync-peer`); no push-based catchup
- `admin_token` must be set manually in `config.toml`; no automatic rotation

---

[Unreleased]: https://github.com/HaitoDann/vinx-ledger/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/HaitoDann/vinx-ledger/releases/tag/v0.1.0-alpha.1
