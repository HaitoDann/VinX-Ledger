# Changelog

All notable changes to VinX Ledger are documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)  
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

---

## [Unreleased]

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
