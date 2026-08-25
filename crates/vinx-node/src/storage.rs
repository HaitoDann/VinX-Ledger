use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::chain::Chain;
use ahash::AHashMap;
use redb::{Database, ReadableTable, TableDefinition};
use vinx_core::{Account, Block, Transaction};
use vinx_crypto::{Address, Hash32};
use vinx_state::WorldState;
use zstd;

/// Schema version stored in the meta table. Increment when the on-disk layout
/// changes; older data is migrated forward in place by `Storage::migrate_forward`
/// (add a step there for the new bump), so nodes upgrade without a data wipe.
/// v2: blobs are zstd-compressed (level 3) before insertion.
/// v3: tx_index and account_tx_index are persisted (no rebuild_tx_index on boot).
/// v4: accounts persisted per-key in a dedicated table; only changed rows are
///     written each block (O(dirty) instead of O(total accounts) per persist).
/// v5: addresses are stored as raw 20 bytes (Address is `[u8; 20]`); the accounts
///     table is keyed by those bytes and bincode encodes addresses as 20 bytes.
/// v6: "Fonderie" tokenomics — Account drops `frozen`/`frozen_since`; WorldState
///     replaces the pools (staking/melt/distribution/treasury/coffre) with `foundry`.
/// v7: tx indexes are keyed by raw bytes (`Hash32`, `Address`) instead of hex/bech32
///     Strings, and hashed with ahash. The persisted index blobs change layout;
///     they are derived data, so a fresh start simply rebuilds them from blocks.
/// v8: WorldState meta gains ADR 0011 governance fields (`admin_policy`,
///     `pending_governance`), appended last. A v7 meta blob is a strict prefix of a v8
///     one, so the migration simply appends those fields' default encodings.
/// v9: WorldState meta gains the ADR 0010 module registry (`modules`), appended last —
///     same prefix property, migrated by appending the empty-map encoding.
/// v10: WorldState meta gains ADR 0040 fields `epoch_dist_emission_pot` (Amount::ZERO)
///     and `destroyed_atoms` (0u128), appended after `modules`. The dormant `foundry`
///     field is retained in-place for bincode compatibility.
/// v11: (1) blocks are persisted per-height in BLOCKS table; the monolithic "chain"
///      blob is replaced by a tiny "chain_meta" entry (finalized height). (2) WorldState
///      meta gains ADR 0027 `reliability` map appended last — same prefix property.
/// v12: Open PoA (ADR 0038) — `validator_pool`, `banned_validator_keys`,
///      `active_set_size`, `last_active_set_size_change_ts`, `last_bond_change_ts`
///      appended after `reliability`. Migration appends their default encodings.
/// v13: Epoch close (ADR 0028/0038) — `last_epoch_close_ts` appended after
///      `last_bond_change_ts`. Migration appends `u64 = 0`.
/// v14: BLS co-signatures (ADR 0046 Phase 2) — each Block row gains two trailing fields:
///      `bls_aggregate: Option<[u8;96]>` (None) and `bls_cosigner_pks: Vec<[u8;48]>` ([]).
///      `ValidatorPoolEntry` also gains `bls_pub_key` and `bls_pop` (None each), but since
///      the pool is expected empty during this alpha migration, no entry-level patching is done.
/// v15: BLS bitmap (ADR 0029 Phase 1) — each Block row gains `bls_bitmap: Vec<u8>` ([]).
///      8 bytes appended per block row (bincode empty Vec<u8> = 0u64 LE).
/// v16: ADR 0038 governable bond floor — `min_validator_bond_atoms` (u128 =
///      MIN_VALIDATOR_BOND_ATOMS) appended to WorldState meta.
/// v17: ADR 0029 Phase 2 epoch beacon — `epoch_beacon` (Hash32 = [0u8; 32]) appended
///      to WorldState meta.
/// v18: ADR 0036 validator churn bounds — `exit_queue` (Vec<ValidatorExitRequest> = [])
///      appended to WorldState meta. Migration appends 8 zero bytes (bincode empty Vec).
const STORAGE_VERSION: u64 = 18;

/// zstd compression level — level 3 is the sweet spot: ~60-70% size reduction,
/// negligible latency compared to disk I/O.
const ZSTD_LEVEL: i32 = 3;

const STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("state");
/// Per-account rows: bech32 address → bincode(Account), stored uncompressed.
/// Accounts are tiny (~100 B); per-row zstd framing would cost more than it saves.
const ACCOUNTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("accounts");
/// Per-block rows: height → zstd(bincode((block_hash, Block))). Written
/// incrementally — only heights dirtied since the last flush (v10).
const BLOCKS: TableDefinition<u64, &[u8]> = TableDefinition::new("blocks");
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");

pub struct Storage {
    db: Arc<Database>,
}

/// A batch of pre-serialized state ready to be compressed and written to disk.
/// Produced under the caller's state lock; compression + I/O happen later
/// (typically inside `tokio::task::spawn_blocking`).
pub struct StateWrite {
    /// Serialized `WorldState` meta — every field except the accounts map.
    pub meta: Vec<u8>,
    /// Changed account rows: (20-byte address, bincode(Account)). Empty on a no-op flush.
    pub account_rows: Vec<([u8; 20], Vec<u8>)>,
    /// Addresses of accounts reaped this flush (ADR 0026): their rows must be **deleted**
    /// from the accounts table, not merely absent from `account_rows`, or they would
    /// resurrect on the next load. Ignored when `replace_accounts` (the table is wiped).
    pub account_deletes: Vec<[u8; 20]>,
    /// When true, the accounts table is wiped before writing `account_rows`
    /// (used by full snapshot import to drop rows no longer present).
    pub replace_accounts: bool,
    /// Tiny chain metadata blob (finalized height) — rewritten every flush.
    pub chain_meta: Vec<u8>,
    /// Changed block rows: (height, bincode((hash, Block))). Only the heights
    /// dirtied since the last flush — O(new blocks), not O(chain length).
    pub block_rows: Vec<(u64, Vec<u8>)>,
    /// When true, the blocks table is wiped before writing `block_rows`
    /// (full save: genesis bootstrap or snapshot import, where the new chain
    /// may be shorter than the one it replaces).
    pub replace_blocks: bool,
    pub tx_index: Vec<u8>,
    pub account_tx_index: Vec<u8>,
}

impl Storage {
    /// Opens (or creates) the storage at `dir`, returning an error instead of
    /// panicking so callers can fail gracefully. Older on-disk schemas are
    /// **migrated forward in place** (see [`Storage::migrate_forward`]); a newer
    /// on-disk schema, or one with no known migration path, is a clean, actionable
    /// error rather than a wipe.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir: PathBuf = dir.into();
        std::fs::create_dir_all(&dir)
            .map_err(|e| Self::io_err(format!("create data directory {}: {e}", dir.display())))?;
        let path = dir.join("vinx.redb");
        let db = Database::create(&path)
            .map_err(|e| Self::io_err(format!("open redb database {}: {e}", path.display())))?;

        {
            let tx = db.begin_write().map_err(Self::io_err)?;
            // Read the on-disk version (scoped so the table handle is released
            // before any migration reopens tables in the same transaction).
            let existing_version: Option<u64> = {
                let meta = tx.open_table(META).map_err(Self::io_err)?;
                let guard = meta.get("schema_version").map_err(Self::io_err)?;
                let version = guard.map(|v| v.value());
                version
            };
            match existing_version {
                // Fresh database — stamp the current version.
                None => {
                    let mut meta = tx.open_table(META).map_err(Self::io_err)?;
                    meta.insert("schema_version", STORAGE_VERSION)
                        .map_err(Self::io_err)?;
                }
                // Up to date — nothing to do.
                Some(v) if v == STORAGE_VERSION => {}
                // Newer on-disk than this binary — refuse (no downgrade).
                Some(v) if v > STORAGE_VERSION => {
                    return Err(Self::io_err(format!(
                        "on-disk schema is v{v}, newer than this binary (v{STORAGE_VERSION}). \
                         Use a matching or newer VinX build; downgrade is not supported."
                    )));
                }
                // Older — migrate forward step by step, then stamp the new version.
                Some(v) => {
                    Self::migrate_forward(&tx, v)?;
                    let mut meta = tx.open_table(META).map_err(Self::io_err)?;
                    meta.insert("schema_version", STORAGE_VERSION)
                        .map_err(Self::io_err)?;
                    tracing::info!(
                        from = v,
                        to = STORAGE_VERSION,
                        "Storage schema migrated in place — no wipe"
                    );
                }
            }
            tx.commit().map_err(Self::io_err)?;
        }

        Ok(Self { db: Arc::new(db) })
    }

    /// Applies forward migrations from on-disk `from` up to [`STORAGE_VERSION`],
    /// operating on an open write transaction. Each step transforms the data so
    /// this binary can read it. Returns a clear error if a step is unknown, so the
    /// operator can fall back to the snapshot export/import path instead of losing data.
    ///
    /// VinX's on-disk layout separates the *source of truth* (per-account rows,
    /// world-state meta, chain blocks) from *derived data* (the tx indexes, which
    /// are rebuildable from the chain). A version bump that changed only derived
    /// data therefore migrates by dropping the stale index blobs — `load()` then
    /// rebuilds them from the chain. Bumps that change the on-disk layout of
    /// accounts / meta / blocks need an explicit transform step added to the match.
    fn migrate_forward(tx: &redb::WriteTransaction, from: u64) -> io::Result<()> {
        let mut v = from;
        while v < STORAGE_VERSION {
            match v {
                // v6 → v7: tx indexes switched to raw-byte keys (Hash32 / Address).
                // Derived data — drop the stale blobs; load() rebuilds from the chain.
                6 => {
                    let mut state = tx.open_table(STATE).map_err(Self::io_err)?;
                    state.remove("tx_index").map_err(Self::io_err)?;
                    state.remove("account_tx_index").map_err(Self::io_err)?;
                }
                // v7 → v8 (ADR 0011): the WorldState meta gained two trailing serialized
                // fields. A v7 blob is a strict prefix of a v8 one, so append their default
                // encodings in place — no wipe, accounts and chain untouched.
                7 => Self::append_meta_suffix(tx, &vinx_state::v8_meta_suffix())?,
                // v8 → v9 (ADR 0010): the module registry was appended last — same prefix
                // property, migrated by appending the empty-map encoding.
                8 => Self::append_meta_suffix(tx, &vinx_state::v9_meta_suffix())?,
                // v9 → v10 (ADR 0040): epoch_dist_emission_pot and destroyed_atoms appended.
                9 => Self::append_meta_suffix(tx, &vinx_state::v10_meta_suffix())?,
                // v10 → v11: (1) split monolithic chain blob into per-height BLOCKS rows,
                // (2) append reliability map (ADR 0027) to WorldState meta.
                10 => {
                    Self::migrate_v9_chain_blob(tx)?;
                    Self::append_meta_suffix(tx, &vinx_state::v11_meta_suffix())?;
                }
                // v11 → v12 (ADR 0038 Open PoA): append validator_pool (empty BTreeMap),
                // banned_validator_keys (empty HashSet), active_set_size (u32 = 21),
                // last_active_set_size_change_ts (u64 = 0), last_bond_change_ts (u64 = 0).
                11 => Self::append_meta_suffix(tx, &vinx_state::v12_meta_suffix())?,
                // v12 → v13 (ADR 0028 epoch close): append last_epoch_close_ts (u64 = 0).
                12 => Self::append_meta_suffix(tx, &vinx_state::v13_meta_suffix())?,
                // v13 → v14 (ADR 0046 Phase 2 BLS): append BLS fields to every block row.
                13 => Self::migrate_v13_block_bls_fields(tx)?,
                // v14 → v15 (ADR 0029 Phase 1 bitmap): append bls_bitmap (empty Vec<u8>) to every block row.
                14 => Self::migrate_v14_block_bitmap_field(tx)?,
                // v15 → v16 (ADR 0038 governable bond floor): append min_validator_bond_atoms.
                15 => Self::append_meta_suffix(tx, &vinx_state::v16_meta_suffix())?,
                // v16 → v17 (ADR 0029 Phase 2 epoch beacon): append epoch_beacon ([0u8;32]).
                16 => Self::append_meta_suffix(tx, &vinx_state::v17_meta_suffix())?,
                // v17 → v18 (ADR 0036 churn bounds): append exit_queue (empty Vec = 8 zero bytes).
                17 => Self::append_meta_suffix(tx, &vinx_state::v18_meta_suffix())?,
                unknown => {
                    return Err(Self::io_err(format!(
                        "no automatic migration from schema v{unknown} to v{STORAGE_VERSION}. \
                         To migrate: run the previous VinX build, GET /snapshot to export the \
                         state, then POST /snapshot into a fresh data directory on this build."
                    )));
                }
            }
            v += 1;
        }
        Ok(())
    }

    /// v9 → v10: explodes the monolithic compressed chain blob into per-height rows
    /// in the BLOCKS table and a "chain_meta" entry, then removes the blob. A no-op
    /// if no chain has been written yet.
    fn migrate_v9_chain_blob(tx: &redb::WriteTransaction) -> io::Result<()> {
        let mut state = tx.open_table(STATE).map_err(Self::io_err)?;
        let compressed = state
            .get("chain")
            .map_err(Self::io_err)?
            .map(|g| g.value().to_vec());
        let Some(compressed) = compressed else {
            return Ok(());
        };
        let chain_bytes = Self::decompress(&compressed)?;
        let chain: Chain = bincode::deserialize(&chain_bytes)
            .map_err(|e| Self::io_err(format!("v10 migration: decode chain blob: {e}")))?;

        let mut blocks_tbl = tx.open_table(BLOCKS).map_err(Self::io_err)?;
        let mut height = 0u64;
        while let Some(row) = chain.block_row(height) {
            let bytes = bincode::serialize(row)
                .map_err(|e| Self::io_err(format!("v10 migration: encode block {height}: {e}")))?;
            let compressed_row = Self::compress(&bytes)?;
            blocks_tbl
                .insert(height, compressed_row.as_slice())
                .map_err(Self::io_err)?;
            height += 1;
        }

        let meta = bincode::serialize(&chain.finalized_height())
            .map_err(|e| Self::io_err(format!("v10 migration: encode chain_meta: {e}")))?;
        let meta_c = Self::compress(&meta)?;
        state
            .insert("chain_meta", meta_c.as_slice())
            .map_err(Self::io_err)?;
        state.remove("chain").map_err(Self::io_err)?;
        tracing::info!(
            blocks = height,
            "Chain blob migrated to per-height rows (v10)"
        );
        Ok(())
    }

    /// Appends `suffix` to the persisted (compressed) WorldState meta blob in place — the
    /// append-only bincode migration used by every meta-field addition (ADR 0010/0011).
    /// A no-op if no meta has been written yet.
    fn append_meta_suffix(tx: &redb::WriteTransaction, suffix: &[u8]) -> io::Result<()> {
        let mut state = tx.open_table(STATE).map_err(Self::io_err)?;
        let compressed = state
            .get("world_state_meta")
            .map_err(Self::io_err)?
            .map(|g| g.value().to_vec());
        if let Some(compressed) = compressed {
            let mut meta = Self::decompress(&compressed)?;
            meta.extend_from_slice(suffix);
            let recompressed = Self::compress(&meta)?;
            state
                .insert("world_state_meta", recompressed.as_slice())
                .map_err(Self::io_err)?;
        }
        Ok(())
    }

    /// v13 → v14 (ADR 0046 Phase 2 BLS): appends two new trailing fields to each Block's
    /// bincode data in the BLOCKS table. Each Block gains:
    ///   `bls_aggregate: Option<[u8; 96]>` = None   → 1 byte  (bincode discriminant 0)
    ///   `bls_cosigner_pks: Vec<[u8; 48]>` = []     → 8 bytes (bincode u64 length = 0)
    /// Total suffix per block: 9 bytes. Decompress → append → recompress in place.
    fn migrate_v13_block_bls_fields(tx: &redb::WriteTransaction) -> io::Result<()> {
        // bincode v1: None<Option<T>> = 0u8 (1 byte); empty Vec<T> = 0u64 LE (8 bytes).
        const SUFFIX: [u8; 9] = [0u8; 9];
        let rows: Vec<(u64, Vec<u8>)> = {
            let tbl = tx.open_table(BLOCKS).map_err(Self::io_err)?;
            tbl.iter()
                .map_err(Self::io_err)?
                .map(|r| {
                    r.map(|(k, v)| (k.value(), v.value().to_vec()))
                        .map_err(Self::io_err)
                })
                .collect::<io::Result<_>>()?
        };
        let count = rows.len() as u64;
        let mut tbl = tx.open_table(BLOCKS).map_err(Self::io_err)?;
        for (height, compressed) in rows {
            let mut data = Self::decompress(&compressed)?;
            data.extend_from_slice(&SUFFIX);
            let recompressed = Self::compress(&data)?;
            tbl.insert(height, recompressed.as_slice())
                .map_err(Self::io_err)?;
        }
        tracing::info!(blocks = count, "v14 migration: BLS fields appended to block rows");
        Ok(())
    }

    /// v14 → v15 (ADR 0029 Phase 1 bitmap): appends `bls_bitmap: Vec<u8>` (empty) to each
    /// Block's bincode data. bincode encodes an empty `Vec<u8>` as `0u64` LE (8 bytes).
    fn migrate_v14_block_bitmap_field(tx: &redb::WriteTransaction) -> io::Result<()> {
        const SUFFIX: [u8; 8] = [0u8; 8]; // bincode empty Vec<u8> = 0u64 LE
        let rows: Vec<(u64, Vec<u8>)> = {
            let tbl = tx.open_table(BLOCKS).map_err(Self::io_err)?;
            tbl.iter()
                .map_err(Self::io_err)?
                .map(|r| {
                    r.map(|(k, v)| (k.value(), v.value().to_vec()))
                        .map_err(Self::io_err)
                })
                .collect::<io::Result<_>>()?
        };
        let count = rows.len() as u64;
        let mut tbl = tx.open_table(BLOCKS).map_err(Self::io_err)?;
        for (height, compressed) in rows {
            let mut data = Self::decompress(&compressed)?;
            data.extend_from_slice(&SUFFIX);
            let recompressed = Self::compress(&data)?;
            tbl.insert(height, recompressed.as_slice())
                .map_err(Self::io_err)?;
        }
        tracing::info!(blocks = count, "v15 migration: bls_bitmap field appended to block rows");
        Ok(())
    }

    /// Convenience wrapper that panics on failure — kept for internal callers and
    /// tests. Prefer `open()` at startup so the error can be handled gracefully.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self::open(dir).expect("open storage")
    }

    fn io_err(msg: impl std::fmt::Display) -> io::Error {
        io::Error::other(msg.to_string())
    }

    fn compress(data: &[u8]) -> io::Result<Vec<u8>> {
        zstd::encode_all(data, ZSTD_LEVEL).map_err(|e| Self::io_err(format!("zstd compress: {e}")))
    }

    fn decompress(data: &[u8]) -> io::Result<Vec<u8>> {
        zstd::decode_all(data).map_err(|e| Self::io_err(format!("zstd decompress: {e}")))
    }

    /// Serializes the chain's dirty block rows, meta and tx indexes (no accounts).
    /// Drains the chain's dirty-height set — rows are written by `write_state`.
    #[allow(clippy::type_complexity)]
    fn serialize_chain(
        chain: &mut Chain,
    ) -> io::Result<(Vec<u8>, Vec<(u64, Vec<u8>)>, Vec<u8>, Vec<u8>)> {
        let chain_meta = bincode::serialize(&chain.finalized_height())
            .map_err(|e| Self::io_err(format!("serialize chain_meta: {e}")))?;
        let mut block_rows = Vec::new();
        for height in chain.take_dirty_heights() {
            if let Some(row) = chain.block_row(height) {
                let bytes = bincode::serialize(row)
                    .map_err(|e| Self::io_err(format!("serialize block {height}: {e}")))?;
                block_rows.push((height, bytes));
            }
        }
        let (tx_index, account_tx_index) = chain.export_tx_indexes();
        let tx_index_bytes = bincode::serialize(tx_index)
            .map_err(|e| Self::io_err(format!("serialize tx_index: {e}")))?;
        let account_tx_index_bytes = bincode::serialize(account_tx_index)
            .map_err(|e| Self::io_err(format!("serialize account_tx_index: {e}")))?;
        Ok((
            chain_meta,
            block_rows,
            tx_index_bytes,
            account_tx_index_bytes,
        ))
    }

    /// Builds an incremental `StateWrite`: the meta blob plus only the accounts
    /// and block rows changed since the last flush. Call while holding the state
    /// and chain write locks — `serialize_meta` moves the accounts map out and back,
    /// and the dirty sets are drained. Compression + I/O happen later in `write_state`.
    pub fn serialize_incremental(
        state: &mut WorldState,
        chain: &mut Chain,
    ) -> io::Result<StateWrite> {
        let meta = state
            .serialize_meta()
            .map_err(|e| Self::io_err(format!("serialize meta: {e}")))?;
        let dirty = state.take_persist_dirty();
        let mut account_rows = Vec::with_capacity(dirty.len());
        let mut account_deletes = Vec::new();
        for addr in dirty {
            if let Some(acc) = state.account_by_addr(&addr) {
                let bytes = bincode::serialize(acc)
                    .map_err(|e| Self::io_err(format!("serialize account: {e}")))?;
                account_rows.push((*addr.as_bytes(), bytes));
            } else {
                // Dirty but no longer in the map ⇒ reaped (ADR 0026): erase its row.
                account_deletes.push(*addr.as_bytes());
            }
        }
        let (chain_meta, block_rows, tx_index, account_tx_index) = Self::serialize_chain(chain)?;
        Ok(StateWrite {
            meta,
            account_rows,
            account_deletes,
            replace_accounts: false,
            chain_meta,
            block_rows,
            replace_blocks: false,
            tx_index,
            account_tx_index,
        })
    }

    /// Builds a full `StateWrite` containing every account and every block,
    /// flagged to wipe any stale rows first. Used for the initial genesis save
    /// and snapshot import.
    pub fn serialize_full(state: &mut WorldState, chain: &mut Chain) -> io::Result<StateWrite> {
        state.mark_all_persist_dirty();
        chain.mark_all_dirty();
        let mut w = Self::serialize_incremental(state, chain)?;
        w.replace_accounts = true;
        w.replace_blocks = true;
        Ok(w)
    }

    /// Compresses and writes a `StateWrite` in a single ACID transaction.
    /// Designed to run inside `tokio::task::spawn_blocking` — pure blocking I/O.
    pub fn write_state(&self, w: StateWrite) -> io::Result<()> {
        let meta_c = Self::compress(&w.meta)?;
        let chain_meta_c = Self::compress(&w.chain_meta)?;
        let tx_idx_c = Self::compress(&w.tx_index)?;
        let acc_idx_c = Self::compress(&w.account_tx_index)?;
        let mut block_rows_c = Vec::with_capacity(w.block_rows.len());
        for (height, bytes) in &w.block_rows {
            block_rows_c.push((*height, Self::compress(bytes)?));
        }

        tracing::debug!(
            meta_raw = w.meta.len(),
            meta_compressed = meta_c.len(),
            account_rows = w.account_rows.len(),
            block_rows = w.block_rows.len(),
            replace = w.replace_accounts,
            "Persisting state (incremental)"
        );

        let tx = self.db.begin_write().map_err(Self::io_err)?;
        if w.replace_accounts {
            // Drop and recreate the accounts table to clear rows no longer present.
            tx.delete_table(ACCOUNTS).map_err(Self::io_err)?;
        }
        if w.replace_blocks {
            // Same for blocks: the incoming chain may be shorter than the stored one.
            tx.delete_table(BLOCKS).map_err(Self::io_err)?;
        }
        {
            let mut tbl = tx.open_table(STATE).map_err(Self::io_err)?;
            tbl.insert("world_state_meta", meta_c.as_slice())
                .map_err(Self::io_err)?;
            tbl.insert("chain_meta", chain_meta_c.as_slice())
                .map_err(Self::io_err)?;
            tbl.insert("tx_index", tx_idx_c.as_slice())
                .map_err(Self::io_err)?;
            tbl.insert("account_tx_index", acc_idx_c.as_slice())
                .map_err(Self::io_err)?;
        }
        {
            let mut btbl = tx.open_table(BLOCKS).map_err(Self::io_err)?;
            for (height, bytes) in &block_rows_c {
                btbl.insert(*height, bytes.as_slice())
                    .map_err(Self::io_err)?;
            }
        }
        {
            let mut atbl = tx.open_table(ACCOUNTS).map_err(Self::io_err)?;
            for (addr, bytes) in &w.account_rows {
                atbl.insert(addr.as_slice(), bytes.as_slice())
                    .map_err(Self::io_err)?;
            }
            // ADR 0026: erase reaped account rows. Skipped when the whole table was just
            // wiped (`replace_accounts`), where there is nothing left to delete.
            if !w.replace_accounts {
                for addr in &w.account_deletes {
                    atbl.remove(addr.as_slice()).map_err(Self::io_err)?;
                }
            }
        }
        tx.commit().map_err(Self::io_err)?;
        Ok(())
    }

    /// Full synchronous save (initial genesis bootstrap, tests, snapshot import).
    /// Writes every account and block and clears any stale rows.
    pub fn save(&self, state: &mut WorldState, chain: &mut Chain) -> io::Result<()> {
        let w = Self::serialize_full(state, chain)?;
        self.write_state(w)
    }

    /// Load persisted state and chain.  Returns `None` if no data has been written yet.
    pub fn load(&self) -> Option<(WorldState, Chain)> {
        let tx = self.db.begin_read().ok()?;
        let tbl = tx.open_table(STATE).ok()?;

        let meta_bytes = Self::decompress(tbl.get("world_state_meta").ok()??.value())
            .map_err(|e| tracing::warn!("Cannot decompress state meta: {e}"))
            .ok()?;
        // Deserializes into a WorldState whose accounts map is empty — repopulated below.
        let mut state: WorldState = bincode::deserialize(&meta_bytes)
            .map_err(|e| tracing::warn!("Cannot deserialize state meta: {e}"))
            .ok()?;

        // Repopulate accounts from the per-key ACCOUNTS table.
        if let Ok(atbl) = tx.open_table(ACCOUNTS) {
            let mut count = 0usize;
            if let Ok(iter) = atbl.iter() {
                for entry in iter.flatten() {
                    match bincode::deserialize::<Account>(entry.1.value()) {
                        Ok(acc) => {
                            state.load_account(acc);
                            count += 1;
                        }
                        Err(e) => tracing::warn!("Cannot deserialize account row: {e}"),
                    }
                }
            }
            tracing::debug!(accounts = count, "Loaded accounts from per-key store");
        }

        // Chain: finalized-height watermark + per-height block rows (v10).
        let chain_meta_bytes = Self::decompress(tbl.get("chain_meta").ok()??.value())
            .map_err(|e| tracing::warn!("Cannot decompress chain_meta: {e}"))
            .ok()?;
        let finalized_height: u64 = bincode::deserialize(&chain_meta_bytes)
            .map_err(|e| tracing::warn!("Cannot deserialize chain_meta: {e}"))
            .ok()?;

        let btbl = tx.open_table(BLOCKS).ok()?;
        let mut blocks: Vec<(Hash32, Block)> = Vec::new();
        let iter = btbl.iter().ok()?;
        for entry in iter.flatten() {
            let (height, row) = (entry.0.value(), entry.1.value());
            // redb iterates u64 keys in ascending order; enforce density so a
            // corrupted table cannot silently produce a chain with holes.
            if height != blocks.len() as u64 {
                tracing::warn!(
                    expected = blocks.len(),
                    found = height,
                    "Block table has a gap — refusing to load"
                );
                return None;
            }
            let bytes = Self::decompress(row)
                .map_err(|e| tracing::warn!("Cannot decompress block {height}: {e}"))
                .ok()?;
            let parsed: (Hash32, Block) = bincode::deserialize(&bytes)
                .map_err(|e| tracing::warn!("Cannot deserialize block {height}: {e}"))
                .ok()?;
            blocks.push(parsed);
        }
        if blocks.is_empty() {
            tracing::warn!("chain_meta present but no block rows — refusing to load");
            return None;
        }
        let mut chain = Chain::from_parts(blocks, finalized_height);

        // Restore persisted indexes — O(1) vs O(blocks×txs) rebuild
        let indexes_restored = (|| -> Option<()> {
            let tx_index_bytes = Self::decompress(tbl.get("tx_index").ok()??.value()).ok()?;
            let account_tx_index_bytes =
                Self::decompress(tbl.get("account_tx_index").ok()??.value()).ok()?;
            let tx_index: AHashMap<Hash32, (u64, u32)> =
                bincode::deserialize(&tx_index_bytes).ok()?;
            let account_tx_index: AHashMap<Address, Vec<Hash32>> =
                bincode::deserialize(&account_tx_index_bytes).ok()?;
            chain.import_tx_indexes(tx_index, account_tx_index);
            Some(())
        })();

        if indexes_restored.is_none() {
            tracing::warn!("Persisted tx indexes missing — rebuilding (one-time cost)");
            chain.rebuild_tx_index();
        }

        Some((state, chain))
    }

    /// Serializes pending mempool transactions to raw bytes.
    pub fn serialize_mempool(txs: &[&Transaction]) -> io::Result<Vec<u8>> {
        bincode::serialize(txs).map_err(|e| Self::io_err(format!("serialize mempool: {e}")))
    }

    /// Compresses and writes a pre-serialized mempool blob to redb.
    /// Designed to run inside `tokio::task::spawn_blocking`.
    pub fn save_mempool_blob(&self, blob: Vec<u8>) -> io::Result<()> {
        let compressed = Self::compress(&blob)?;
        let tx = self.db.begin_write().map_err(Self::io_err)?;
        {
            let mut tbl = tx.open_table(STATE).map_err(Self::io_err)?;
            tbl.insert("mempool", compressed.as_slice())
                .map_err(Self::io_err)?;
        }
        tx.commit().map_err(Self::io_err)?;
        Ok(())
    }

    /// Loads and decompresses persisted mempool transactions.
    /// Returns `None` if no mempool snapshot exists yet.
    pub fn load_mempool(&self) -> Option<Vec<Transaction>> {
        let tx = self.db.begin_read().ok()?;
        let tbl = tx.open_table(STATE).ok()?;
        let compressed = tbl.get("mempool").ok()??.value().to_vec();
        let bytes = Self::decompress(&compressed)
            .map_err(|e| tracing::warn!("Cannot decompress mempool: {e}"))
            .ok()?;
        bincode::deserialize(&bytes)
            .map_err(|e| tracing::warn!("Cannot deserialize mempool: {e}"))
            .ok()
    }

    pub fn exists(&self) -> bool {
        let Ok(tx) = self.db.begin_read() else {
            return false;
        };
        let Ok(tbl) = tx.open_table(STATE) else {
            return false;
        };
        tbl.get("world_state_meta").ok().flatten().is_some()
    }

    /// Returns a clone of the inner `Arc<Database>` handle — used by Node to pass
    /// Storage into `spawn_blocking` without cloning the whole struct.
    pub fn arc_clone(&self) -> Arc<Database> {
        Arc::clone(&self.db)
    }
}

impl Clone for Storage {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
        }
    }
}

/// Removes the redb database file — full data wipe.
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("vinx.redb"));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Temp dir cleaned up on drop.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "vinx_mig_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Writes a schema-version marker (and optionally stale index blobs) into a
    /// fresh redb, then releases it so `Storage::open` can reopen the same file.
    fn stamp(dir: &Path, version: u64, with_stale_indexes: bool) {
        let db = Database::create(dir.join("vinx.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        {
            let mut m = tx.open_table(META).unwrap();
            m.insert("schema_version", version).unwrap();
        }
        if with_stale_indexes {
            let mut s = tx.open_table(STATE).unwrap();
            s.insert("tx_index", b"stale-v6".as_slice()).unwrap();
            s.insert("account_tx_index", b"stale-v6".as_slice())
                .unwrap();
        }
        tx.commit().unwrap();
    }

    /// Overwrites just the version marker on an existing db (leaves all data).
    fn set_version(dir: &Path, version: u64) {
        let db = Database::create(dir.join("vinx.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        {
            let mut m = tx.open_table(META).unwrap();
            m.insert("schema_version", version).unwrap();
        }
        tx.commit().unwrap();
    }

    fn read_version(dir: &Path) -> Option<u64> {
        let db = Database::create(dir.join("vinx.redb")).unwrap();
        let tx = db.begin_read().unwrap();
        let m = tx.open_table(META).unwrap();
        let v = m.get("schema_version").unwrap().map(|v| v.value());
        v
    }

    fn has_state_key(dir: &Path, key: &str) -> bool {
        let db = Database::create(dir.join("vinx.redb")).unwrap();
        let tx = db.begin_read().unwrap();
        let s = tx.open_table(STATE).unwrap();
        s.get(key).unwrap().is_some()
    }

    /// Strips the appended v8 (governance), v9 (module registry), v10 (ADR 0040),
    /// and v11 (reliability) suffixes from the persisted meta blob, turning a
    /// current-format meta back into its v7 prefix so the append migrations can be
    /// exercised on data that genuinely predates ADR 0010/0011/0040/0027.
    fn downgrade_meta_to_v7(dir: &Path) {
        let db = Database::create(dir.join("vinx.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        {
            let mut s = tx.open_table(STATE).unwrap();
            let compressed = s.get("world_state_meta").unwrap().unwrap().value().to_vec();
            let mut meta = zstd::decode_all(&compressed[..]).unwrap();
            let suffix_len = vinx_state::v8_meta_suffix().len()
                + vinx_state::v9_meta_suffix().len()
                + vinx_state::v10_meta_suffix().len()
                + vinx_state::v11_meta_suffix().len();
            meta.truncate(meta.len() - suffix_len);
            let recompressed = zstd::encode_all(&meta[..], ZSTD_LEVEL).unwrap();
            s.insert("world_state_meta", recompressed.as_slice())
                .unwrap();
        }
        tx.commit().unwrap();
    }

    #[test]
    fn test_migrate_v6_rebuilds_indexes_and_bumps() {
        let tmp = Tmp::new();
        stamp(&tmp.0, 6, true);
        assert!(has_state_key(&tmp.0, "tx_index"));

        // Open → migrates v6 → current in place (drops the stale derived indexes).
        let storage = Storage::open(&tmp.0).expect("v6 must auto-migrate, not wipe");
        drop(storage);

        assert_eq!(read_version(&tmp.0), Some(STORAGE_VERSION));
        assert!(
            !has_state_key(&tmp.0, "tx_index"),
            "stale tx_index should be dropped so load() rebuilds it"
        );
        assert!(!has_state_key(&tmp.0, "account_tx_index"));
    }

    #[test]
    fn test_unknown_old_version_errors_with_guidance() {
        let tmp = Tmp::new();
        stamp(&tmp.0, 3, false); // no migration path from v3 → snapshot fallback
        let err = Storage::open(&tmp.0).err().unwrap();
        assert!(err.to_string().contains("no automatic migration"));
    }

    #[test]
    fn test_newer_on_disk_version_refused() {
        let tmp = Tmp::new();
        stamp(&tmp.0, STORAGE_VERSION + 1, false);
        let err = Storage::open(&tmp.0).err().unwrap();
        assert!(err.to_string().contains("newer than this binary"));
    }

    #[test]
    fn test_current_version_opens_unchanged() {
        let tmp = Tmp::new();
        stamp(&tmp.0, STORAGE_VERSION, false);
        Storage::open(&tmp.0).expect("current version opens cleanly");
        assert_eq!(read_version(&tmp.0), Some(STORAGE_VERSION));
    }

    // End-to-end: real data (accounts + chain) survives a v6 → current migration,
    // and the derived tx index is rebuilt from the chain — no wipe, no data loss.
    #[test]
    fn test_migration_preserves_accounts_and_rebuilds_tx_index() {
        use vinx_core::amount::Amount;
        use vinx_core::{Block, BlockHeader, Transaction};
        use vinx_crypto::{Address, KeyPair};
        use vinx_state::{create_genesis_state, GenesisConfig};

        let tmp = Tmp::new();
        let kp = KeyPair::generate();
        let admin = Address::from_public_key(&kp.public_key());
        let validator = Address::from_public_key(&KeyPair::generate().public_key());
        let bob = Address::from_public_key(&KeyPair::generate().public_key());

        let mut state = create_genesis_state(&GenesisConfig {
            chain_id: vinx_core::CHAIN_ID_DEVNET,
            admin_address: admin,
            validator_address: validator,
        });
        // Fair launch: genesis grants nothing, so seed a balance to check it survives.
        state.credit_for_test(admin, Amount::from_vinx(500));
        let (mut chain, _) = Chain::new_with_genesis(validator, 0);
        let tx =
            Transaction::new_transfer(&kp, bob, Amount::from_vinx(10), Amount::from_vinx(1), 0);
        let tx_hash = tx.hash();
        let block = Block {
            header: BlockHeader {
                height: 1,
                prev_hash: chain.tip_hash(),
                timestamp: 1,
                validator,
                tx_count: 1,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
            },
            transactions: vec![tx],
            bls_aggregate: None,
            bls_cosigner_pks: vec![],
            bls_bitmap: vec![],
        };
        chain.push(block);

        // Save at the current version, then simulate genuinely old on-disk data: strip the
        // v8 governance suffix so the meta is v7-format, and stamp an older (v6) marker.
        {
            let storage = Storage::open(&tmp.0).unwrap();
            storage.save(&mut state, &mut chain).unwrap();
        }
        downgrade_meta_to_v7(&tmp.0);
        set_version(&tmp.0, 6);

        // Reopen → migrates in place (v6→v7 drops indexes; v7→v8 re-appends governance
        // defaults); load rebuilds the derived index.
        let storage = Storage::open(&tmp.0).expect("v6 must migrate, not wipe");
        let (loaded_state, loaded_chain) = storage.load().expect("data survives migration");

        assert_eq!(
            loaded_state.account_balance(&admin),
            Amount::from_vinx(500),
            "account balance must survive migration"
        );
        // ADR 0011/0010: the appended governance + module fields load with their defaults.
        assert!(loaded_state.admin_policy.is_none());
        assert!(loaded_state.pending_governance.is_empty());
        assert!(loaded_state.modules.is_empty());
        assert!(
            loaded_state.reliability.is_empty(),
            "ADR 0027 reliability loads empty (v11)"
        );
        assert_eq!(loaded_chain.tip_height(), 1, "chain must survive migration");
        assert!(
            loaded_chain.get_tx_by_hash(&tx_hash).is_some(),
            "tx index must be rebuilt after migration"
        );
    }

    /// Strips the last 8 bytes from every block row in the BLOCKS table, simulating the
    /// v14 on-disk format (no `bls_bitmap` field) so the v14→v15 migration can be tested.
    fn downgrade_blocks_to_v14(dir: &Path) {
        const BLS_BITMAP_SUFFIX_LEN: usize = 8; // bincode empty Vec<u8> = 0u64 LE
        let db = Database::create(dir.join("vinx.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        {
            let rows: Vec<(u64, Vec<u8>)> = {
                let tbl = tx.open_table(BLOCKS).unwrap();
                tbl.iter()
                    .unwrap()
                    .map(|r| r.map(|(k, v)| (k.value(), v.value().to_vec())).unwrap())
                    .collect()
            };
            let mut tbl = tx.open_table(BLOCKS).unwrap();
            for (height, compressed) in rows {
                let mut data = zstd::decode_all(&compressed[..]).unwrap();
                assert!(
                    data.len() >= BLS_BITMAP_SUFFIX_LEN,
                    "block row too short to strip"
                );
                data.truncate(data.len() - BLS_BITMAP_SUFFIX_LEN);
                let recompressed = zstd::encode_all(&data[..], ZSTD_LEVEL).unwrap();
                tbl.insert(height, recompressed.as_slice()).unwrap();
            }
        }
        tx.commit().unwrap();
    }

    // ADR 0029 Phase 1 — Storage migration v14→v15: bls_bitmap field appended to blocks.
    #[test]
    fn test_migration_v14_v15_bitmap_field() {
        use vinx_core::{Block, BlockHeader};
        use vinx_crypto::{Address, KeyPair};
        use vinx_state::{create_genesis_state, GenesisConfig};

        let tmp = Tmp::new();
        let validator = Address::from_public_key(&KeyPair::generate().public_key());
        let admin = Address::from_public_key(&KeyPair::generate().public_key());

        let mut state = create_genesis_state(&GenesisConfig {
            chain_id: vinx_core::CHAIN_ID_DEVNET,
            admin_address: admin,
            validator_address: validator,
        });
        let (mut chain, _) = Chain::new_with_genesis(validator, 0);

        // Push 3 blocks (v15 format, bls_bitmap = vec![]).
        for h in 1..=3u64 {
            let block = Block {
                header: BlockHeader {
                    height: h,
                    prev_hash: chain.tip_hash(),
                    timestamp: h,
                    validator,
                    tx_count: 0,
                    state_root: [0u8; 32],
                    base_fee: 0,
                    receipts_root: [0u8; 32],
                },
                transactions: vec![],
                bls_aggregate: None,
                bls_cosigner_pks: vec![],
                bls_bitmap: vec![],
            };
            chain.push(block);
        }

        // Save at STORAGE_VERSION (v15).
        {
            let storage = Storage::open(&tmp.0).unwrap();
            storage.save(&mut state, &mut chain).unwrap();
        }

        // Simulate v14 on-disk: strip bls_bitmap from each block row, downgrade version marker.
        downgrade_blocks_to_v14(&tmp.0);
        set_version(&tmp.0, 14);

        // Reopen → migration v14→v15 runs automatically.
        let loaded_chain = {
            let storage = Storage::open(&tmp.0).expect("v14 must auto-migrate to v15, not wipe");
            let (_, chain) = storage.load().expect("data must survive v14→v15 migration");
            chain
        }; // storage dropped here so read_version can reopen the file

        assert_eq!(
            read_version(&tmp.0),
            Some(STORAGE_VERSION),
            "version marker bumped to current after migration"
        );
        assert_eq!(loaded_chain.tip_height(), 3, "chain height preserved");

        for h in 0..=3u64 {
            let block = loaded_chain
                .get_block(h)
                .unwrap_or_else(|| panic!("block {h} must exist after migration"));
            assert_eq!(
                block.bls_bitmap,
                Vec::<u8>::new(),
                "h={h}: bls_bitmap defaults to [] after v14→v15 migration"
            );
        }
    }

    // ADR 0026: a reaped account must be *erased* from the store, not merely dropped from
    // the in-memory map — otherwise it resurrects on the next load.
    #[test]
    fn test_reaped_account_does_not_resurrect_on_reload() {
        use vinx_core::amount::Amount;
        use vinx_core::Transaction;
        use vinx_crypto::{Address, KeyPair};
        use vinx_state::{create_genesis_state, GenesisConfig};

        let tmp = Tmp::new();
        let admin = Address::from_public_key(&KeyPair::generate().public_key());
        let validator = Address::from_public_key(&KeyPair::generate().public_key());
        let bob_kp = KeyPair::generate();
        let bob = Address::from_public_key(&bob_kp.public_key());
        let (_, carol) = {
            let kp = KeyPair::generate();
            (kp.clone(), Address::from_public_key(&kp.public_key()))
        };

        let mut state = create_genesis_state(&GenesisConfig {
            chain_id: vinx_core::CHAIN_ID_DEVNET,
            admin_address: admin,
            validator_address: validator,
        });
        let (mut chain, _) = Chain::new_with_genesis(validator, 0);

        // Fund bob with exactly amount + fee so a single transfer drains him to zero.
        let amount = Amount::from_vinx(10);
        let fee = amount.calculate_fee(state.base_fee);
        let total = amount.checked_add(fee).unwrap();
        state.credit_for_test(bob, total);
        state.circulating_supply = total;

        // Scoped so the redb handle (and its file lock) is released before reopening.
        {
            let storage = Storage::open(&tmp.0).unwrap();
            storage.save(&mut state, &mut chain).unwrap(); // bob's row is on disk

            // Drain bob → he is reaped from the map.
            let tx = Transaction::new_transfer(&bob_kp, carol, amount, fee, 0);
            state.apply_transaction(&tx).unwrap();
            assert!(state.get_account(&bob).is_none(), "bob reaped in memory");

            // Persist incrementally.
            let w = Storage::serialize_incremental(&mut state, &mut chain).unwrap();
            assert!(
                w.account_deletes.iter().any(|a| a == bob.as_bytes()),
                "reaped account must be scheduled for deletion"
            );
            storage.write_state(w).unwrap();
        }

        let reopened = Storage::open(&tmp.0).unwrap();
        let (loaded, _) = reopened.load().expect("state reloads");
        assert!(
            loaded.get_account(&bob).is_none(),
            "reaped account must NOT resurrect after reload"
        );
        assert_eq!(
            loaded.account_balance(&carol),
            amount,
            "carol keeps the funds"
        );
    }

    fn make_test_block(height: u64, prev_hash: Hash32, validator: Address) -> Block {
        use vinx_core::BlockHeader;
        Block {
            header: BlockHeader {
                height,
                prev_hash,
                timestamp: height,
                validator,
                tx_count: 0,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
            },
            transactions: vec![],
            bls_aggregate: None,
            bls_cosigner_pks: vec![],
            bls_bitmap: vec![],
        }
    }

    // v10: a flush after N new blocks serializes exactly those N rows — never the
    // whole chain — and the chain reloads identically from the per-height table.
    #[test]
    fn test_incremental_block_rows_and_reload() {
        use vinx_crypto::KeyPair;
        use vinx_state::{create_genesis_state, GenesisConfig};

        let tmp = Tmp::new();
        let admin = Address::from_public_key(&KeyPair::generate().public_key());
        let validator = Address::from_public_key(&KeyPair::generate().public_key());
        let mut state = create_genesis_state(&GenesisConfig {
            chain_id: vinx_core::CHAIN_ID_DEVNET,
            admin_address: admin,
            validator_address: validator,
        });
        let (mut chain, _) = Chain::new_with_genesis(validator, 0);

        {
            let storage = Storage::open(&tmp.0).unwrap();
            storage.save(&mut state, &mut chain).unwrap();

            // Two new blocks → exactly two dirty rows in the next flush.
            chain.push(make_test_block(1, chain.tip_hash(), validator));
            chain.push(make_test_block(2, chain.tip_hash(), validator));
            let w = Storage::serialize_incremental(&mut state, &mut chain).unwrap();
            let mut heights: Vec<u64> = w.block_rows.iter().map(|(h, _)| *h).collect();
            heights.sort_unstable();
            assert_eq!(heights, vec![1, 2], "only the new blocks are written");
            storage.write_state(w).unwrap();

            // Nothing changed → the follow-up flush writes zero block rows.
            let w2 = Storage::serialize_incremental(&mut state, &mut chain).unwrap();
            assert!(w2.block_rows.is_empty(), "clean chain flushes no rows");
            storage.write_state(w2).unwrap();
        }

        let storage = Storage::open(&tmp.0).unwrap();
        let (_, loaded) = storage.load().expect("chain reloads from per-height rows");
        assert_eq!(loaded.tip_height(), 2);
        assert_eq!(loaded.tip_hash(), chain.tip_hash());
        assert_eq!(loaded.finalized_height(), chain.finalized_height());
    }

    // v9 → v10: a monolithic v9 "chain" blob is exploded into per-height rows on
    // open, the blob removed, and the chain loads identically afterwards.
    #[test]
    fn test_migrate_v9_chain_blob_to_block_rows() {
        use vinx_crypto::KeyPair;
        use vinx_state::{create_genesis_state, GenesisConfig};

        let tmp = Tmp::new();
        let admin = Address::from_public_key(&KeyPair::generate().public_key());
        let validator = Address::from_public_key(&KeyPair::generate().public_key());
        let mut state = create_genesis_state(&GenesisConfig {
            chain_id: vinx_core::CHAIN_ID_DEVNET,
            admin_address: admin,
            validator_address: validator,
        });
        let (mut chain, _) = Chain::new_with_genesis(validator, 0);
        chain.push(make_test_block(1, chain.tip_hash(), validator));
        chain.push(make_test_block(2, chain.tip_hash(), validator));
        let tip_hash = chain.tip_hash();

        // Save at the current version, then rewrite the chain the way a v9 binary
        // stored it: one compressed bincode blob under "chain", no per-height rows.
        {
            let storage = Storage::open(&tmp.0).unwrap();
            storage.save(&mut state, &mut chain).unwrap();
        }
        {
            let db = Database::create(tmp.0.join("vinx.redb")).unwrap();
            let tx = db.begin_write().unwrap();
            {
                let mut s = tx.open_table(STATE).unwrap();
                let blob = bincode::serialize(&chain).unwrap();
                let compressed = zstd::encode_all(&blob[..], ZSTD_LEVEL).unwrap();
                s.insert("chain", compressed.as_slice()).unwrap();
                s.remove("chain_meta").unwrap();
            }
            tx.delete_table(BLOCKS).unwrap();
            tx.commit().unwrap();
        }
        set_version(&tmp.0, 9);

        // Scoped so the redb handle is released before has_state_key reopens the file.
        let loaded = {
            let storage = Storage::open(&tmp.0).expect("v9 must migrate, not wipe");
            let (_, loaded) = storage.load().expect("chain survives v9→v10 migration");
            loaded
        };
        assert_eq!(loaded.tip_height(), 2);
        assert_eq!(loaded.tip_hash(), tip_hash);
        assert!(!has_state_key(&tmp.0, "chain"), "v9 blob removed");
        assert!(has_state_key(&tmp.0, "chain_meta"), "chain_meta written");
    }
}
