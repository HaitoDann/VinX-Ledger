use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::chain::{Chain, ChainRow};
use ahash::AHashMap;
use redb::{Database, ReadableTable, TableDefinition};
use vinx_core::{Account, SignedVote, Transaction, VoteKind};
use vinx_crypto::{Address, Hash32};
use vinx_state::WorldState;
use zstd;

/// Schema version stored in the meta table. Increment when the on-disk layout changes.
///
/// v23 (ADR 0082): block rows carry their commit certificate; the vote lock is keyed by
/// (height, round, kind). Pre-genesis, so older databases are refused like v22.
/// v22 (ADR 0081): pre-genesis reset. The protocol changed incompatibly (1 Md supply,
/// 9 decimals, Bech32m, module registry removed, fee split), so there is no migration
/// path from any older database: a pre-v22 database belongs to another protocol and is
/// refused, left untouched. Every earlier step (v2–v21) predates the public genesis and
/// its migration code has been retired. From v22 on, layout changes that must preserve
/// a live network add an explicit step to `Storage::open`.
const STORAGE_VERSION: u64 = 23;

/// zstd compression level — level 3 is the sweet spot: ~60-70% size reduction,
/// negligible latency compared to disk I/O.
const ZSTD_LEVEL: i32 = 3;

const STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("state");
/// Per-account rows: bech32 address → borsh(Account), stored uncompressed.
/// Accounts are tiny (~100 B); per-row zstd framing would cost more than it saves.
const ACCOUNTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("accounts");
/// Per-block rows: height → zstd(borsh(ChainRow)) — hash, block and commit. Written
/// incrementally — only heights dirtied since the last flush (v10).
const BLOCKS: TableDefinition<u64, &[u8]> = TableDefinition::new("blocks");
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
/// Double-sign guard (ADR 0082, VX-RED-003): `height(8 BE) ‖ round(4 BE) ‖ kind(1)` →
/// the value signed (`0` for nil, `1 ‖ hash`). Durable and committed *before* a signature
/// is released.
const VOTES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("votes");

pub struct Storage {
    db: Arc<Database>,
}

/// A batch of pre-serialized state ready to be compressed and written to disk.
/// Produced under the caller's state lock; compression + I/O happen later
/// (typically inside `tokio::task::spawn_blocking`).
pub struct StateWrite {
    /// Serialized `WorldState` meta — every field except the accounts map.
    pub meta: Vec<u8>,
    /// Changed account rows: (20-byte address, borsh(Account)). Empty on a no-op flush.
    pub account_rows: Vec<([u8; 20], Vec<u8>)>,
    /// Addresses of accounts reaped this flush (ADR 0026): their rows must be **deleted**
    /// from the accounts table, not merely absent from `account_rows`, or they would
    /// resurrect on the next load. Ignored when `replace_accounts` (the table is wiped).
    pub account_deletes: Vec<[u8; 20]>,
    /// When true, the accounts table is wiped before writing `account_rows`
    /// (used by full snapshot import to drop rows no longer present).
    pub replace_accounts: bool,
    /// Block rows below this height are deleted (retention pruning, ADR 0083).
    pub delete_blocks_below: Option<u64>,
    /// Tiny chain metadata blob (finalized height) — rewritten every flush.
    pub chain_meta: Vec<u8>,
    /// Changed block rows: (height, borsh((hash, Block))). Only the heights
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
    /// panicking so callers can fail gracefully. An older or newer on-disk schema is a
    /// clean, actionable error — never a silent wipe.
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
                // Older — pre-genesis protocol (ADR 0081): refuse, leave the data intact.
                Some(v) => {
                    return Err(Self::io_err(format!(
                        "cette base (schéma v{v}) a été écrite par une version antérieure au \
                         genesis ADR 0081 (supply, décimales, adresses Bech32m, format d'état) : \
                         elle appartient à un autre protocole et n'est pas migrable. Repartez \
                         d'un dossier de données vide (nouvelle genèse) ou resynchronisez \
                         depuis un pair à jour."
                    )));
                }
            }
            tx.commit().map_err(Self::io_err)?;
        }

        Ok(Self { db: Arc::new(db) })
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
        let chain_meta = borsh::to_vec(&chain.height_base)
            .map_err(|e| Self::io_err(format!("serialize chain_meta: {e}")))?;
        let mut block_rows = Vec::new();
        for height in chain.take_dirty_heights() {
            if let Some(row) = chain.row(height) {
                let bytes = borsh::to_vec(row)
                    .map_err(|e| Self::io_err(format!("serialize block {height}: {e}")))?;
                block_rows.push((height, bytes));
            }
        }
        let (tx_index, account_tx_index) = chain.export_tx_indexes();
        // AHashMap has no borsh impl: persist as a sorted entry list (deterministic bytes).
        let mut tx_entries: Vec<(&Hash32, &(u64, u32))> = tx_index.iter().collect();
        tx_entries.sort_unstable_by_key(|(k, _)| *k);
        let mut acc_entries: Vec<(&Address, &Vec<Hash32>)> = account_tx_index.iter().collect();
        acc_entries.sort_unstable_by_key(|(k, _)| *k);
        let tx_index_bytes = borsh::to_vec(&tx_entries)
            .map_err(|e| Self::io_err(format!("serialize tx_index: {e}")))?;
        let account_tx_index_bytes = borsh::to_vec(&acc_entries)
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
                let bytes = borsh::to_vec(acc)
                    .map_err(|e| Self::io_err(format!("serialize account: {e}")))?;
                account_rows.push((*addr.as_bytes(), bytes));
            } else {
                // Dirty but no longer in the map ⇒ reaped (ADR 0026): erase its row.
                account_deletes.push(*addr.as_bytes());
            }
        }
        let delete_blocks_below = chain.take_pruned_below();
        let (chain_meta, block_rows, tx_index, account_tx_index) = Self::serialize_chain(chain)?;
        Ok(StateWrite {
            delete_blocks_below,
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
            if let (Some(below), false) = (w.delete_blocks_below, w.replace_blocks) {
                let stale: Vec<u64> = btbl
                    .range(..below)
                    .map_err(Self::io_err)?
                    .flatten()
                    .map(|(k, _)| k.value())
                    .collect();
                for h in stale {
                    btbl.remove(h).map_err(Self::io_err)?;
                }
            }
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
        let mut state: WorldState = borsh::from_slice(&meta_bytes)
            .map_err(|e| tracing::warn!("Cannot deserialize state meta: {e}"))
            .ok()?;

        // Repopulate accounts from the per-key ACCOUNTS table.
        if let Ok(atbl) = tx.open_table(ACCOUNTS) {
            let mut count = 0usize;
            if let Ok(iter) = atbl.iter() {
                for entry in iter.flatten() {
                    match borsh::from_slice::<Account>(entry.1.value()) {
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

        // Chain: base height + per-height rows (block and commit certificate).
        let chain_meta_bytes = Self::decompress(tbl.get("chain_meta").ok()??.value())
            .map_err(|e| tracing::warn!("Cannot decompress chain_meta: {e}"))
            .ok()?;
        let stored_base: u64 = borsh::from_slice(&chain_meta_bytes)
            .map_err(|e| tracing::warn!("Cannot deserialize chain_meta: {e}"))
            .ok()?;

        let btbl = tx.open_table(BLOCKS).ok()?;
        let mut blocks: Vec<ChainRow> = Vec::new();
        let mut height_base: Option<u64> = None;
        let iter = btbl.iter().ok()?;
        for entry in iter.flatten() {
            let (height, row) = (entry.0.value(), entry.1.value());
            // The base is the height of the first block (0 for genesis chains,
            // non-zero for snapshot-synced chains). Enforce density from the base.
            let base = *height_base.get_or_insert(height);
            if height != base + blocks.len() as u64 {
                tracing::warn!(
                    expected = base + blocks.len() as u64,
                    found = height,
                    "Block table has a gap — refusing to load"
                );
                return None;
            }
            let bytes = Self::decompress(row)
                .map_err(|e| tracing::warn!("Cannot decompress block {height}: {e}"))
                .ok()?;
            let parsed: ChainRow = borsh::from_slice(&bytes)
                .map_err(|e| tracing::warn!("Cannot deserialize block {height}: {e}"))
                .ok()?;
            blocks.push(parsed);
        }
        if blocks.is_empty() {
            tracing::warn!("chain_meta present but no block rows — refusing to load");
            return None;
        }
        let base = height_base.unwrap_or(0);
        if base != stored_base {
            tracing::warn!(
                base,
                stored_base,
                "Chain base height mismatch — refusing to load"
            );
            return None;
        }
        let mut chain = Chain::from_rows(blocks, base);

        // Restore persisted indexes — O(1) vs O(blocks×txs) rebuild
        let indexes_restored = (|| -> Option<()> {
            let tx_index_bytes = Self::decompress(tbl.get("tx_index").ok()??.value()).ok()?;
            let account_tx_index_bytes =
                Self::decompress(tbl.get("account_tx_index").ok()??.value()).ok()?;
            let tx_index: AHashMap<Hash32, (u64, u32)> =
                borsh::from_slice::<Vec<(Hash32, (u64, u32))>>(&tx_index_bytes)
                    .ok()?
                    .into_iter()
                    .collect();
            let account_tx_index: AHashMap<Address, Vec<Hash32>> =
                borsh::from_slice::<Vec<(Address, Vec<Hash32>)>>(&account_tx_index_bytes)
                    .ok()?
                    .into_iter()
                    .collect();
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
        borsh::to_vec(txs).map_err(|e| Self::io_err(format!("serialize mempool: {e}")))
    }

    fn vote_key(height: u64, round: u32, kind: VoteKind) -> [u8; 13] {
        let mut k = [0u8; 13];
        k[..8].copy_from_slice(&height.to_be_bytes());
        k[8..12].copy_from_slice(&round.to_be_bytes());
        k[12] = match kind {
            VoteKind::Prevote => 1,
            VoteKind::Precommit => 2,
        };
        k
    }

    fn vote_value(value: &Option<Hash32>) -> Vec<u8> {
        match value {
            Some(h) => {
                let mut v = vec![1u8];
                v.extend_from_slice(h);
                v
            }
            None => vec![0u8],
        }
    }

    /// Claims the right to sign `vote` — the double-sign guard (ADR 0082, VX-RED-003).
    ///
    /// Returns `Ok(true)` when no vote of this `(height, round, kind)` was signed before,
    /// or the recorded one has the same value (re-signing is idempotent). Returns
    /// `Ok(false)` when a *different* value is recorded: signing would be equivocation.
    /// The record is committed before this returns, so it survives a crash between the
    /// claim and the signature — a lock written after the fact prevents nothing.
    pub fn claim_sign(&self, vote: &SignedVote) -> io::Result<bool> {
        let key = Self::vote_key(vote.height, vote.round, vote.kind);
        let value = Self::vote_value(&vote.value);
        let write = self.db.begin_write().map_err(Self::io_err)?;
        let allowed = {
            let mut t = write.open_table(VOTES).map_err(Self::io_err)?;
            let existing = t
                .get(key.as_slice())
                .map_err(Self::io_err)?
                .map(|v| v.value().to_vec());
            match existing {
                Some(prev) => prev == value,
                None => {
                    t.insert(key.as_slice(), value.as_slice())
                        .map_err(Self::io_err)?;
                    true
                }
            }
        };
        write.commit().map_err(Self::io_err)?;
        Ok(allowed)
    }

    /// The highest-round non-nil precommit we signed at `height` — our Tendermint lock,
    /// restored after a restart so the node keeps honouring it.
    pub fn last_precommit(&self, height: u64) -> Option<(u32, Hash32)> {
        let read = self.db.begin_read().ok()?;
        let t = read.open_table(VOTES).ok()?;
        let from = Self::vote_key(height, 0, VoteKind::Prevote);
        let to = Self::vote_key(height, u32::MAX, VoteKind::Precommit);
        let mut best: Option<(u32, Hash32)> = None;
        for entry in t.range(from.as_slice()..=to.as_slice()).ok()?.flatten() {
            let (k, v) = (entry.0.value(), entry.1.value());
            if k.len() == 13 && k[12] == 2 && v.len() == 33 && v[0] == 1 {
                let round = u32::from_be_bytes(k[8..12].try_into().ok()?);
                let hash: Hash32 = v[1..].try_into().ok()?;
                if best.is_none_or(|(r, _)| round > r) {
                    best = Some((round, hash));
                }
            }
        }
        best
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
        borsh::from_slice(&bytes)
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

    fn read_version(dir: &Path) -> Option<u64> {
        let db = Database::create(dir.join("vinx.redb")).unwrap();
        let tx = db.begin_read().unwrap();
        let m = tx.open_table(META).unwrap();
        let v = m.get("schema_version").unwrap().map(|v| v.value());
        v
    }

    /// ADR 0081 — toute base antérieure au schéma courant est refusée **et laissée intacte**.
    #[test]
    fn test_older_database_is_refused_and_left_intact() {
        for from in [3u64, 6, 19, 21] {
            let tmp = Tmp::new();
            stamp(&tmp.0, from, true);
            let err = Storage::open(&tmp.0)
                .err()
                .unwrap_or_else(|| panic!("une base v{from} doit être refusée"));
            let msg = err.to_string();
            assert!(
                msg.contains("dossier de données vide") && msg.contains("ADR 0081"),
                "le message doit dire quoi faire : {msg}"
            );
            assert_eq!(
                read_version(&tmp.0),
                Some(from),
                "la base v{from} doit rester intacte"
            );
        }
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
            validator_bls: vinx_state::GenesisBlsKey::from_secret(
                &vinx_crypto::BlsSecretKey::generate(),
                &validator,
                vinx_core::CHAIN_ID_DEVNET,
            ),
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

    fn make_test_block(height: u64, prev_hash: Hash32, validator: Address) -> vinx_core::Block {
        use vinx_core::BlockHeader;
        vinx_core::Block {
            header: BlockHeader {
                height,
                round: 0,
                prev_hash,
                timestamp: height,
                validator,
                tx_count: 0,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
                last_commit_hash: [0u8; 32],
                version: 0,
            },
            transactions: vec![],
            last_commit: None,
        }
    }

    fn cert_for(block: &vinx_core::Block) -> vinx_core::CommitCert {
        vinx_core::CommitCert {
            height: block.header.height,
            round: 0,
            block_hash: block.hash(),
            bitmap: vec![1],
            aggregate: vec![0u8; 96],
        }
    }

    /// ADR 0083 L3: pruned blocks are deleted from disk and the chain reloads from the
    /// new base.
    #[test]
    fn test_pruned_blocks_are_deleted_and_chain_reloads() {
        use vinx_crypto::KeyPair;
        use vinx_state::{create_genesis_state, GenesisConfig};

        let tmp = Tmp::new();
        let validator = Address::from_public_key(&KeyPair::generate().public_key());
        let mut state = create_genesis_state(&GenesisConfig {
            chain_id: vinx_core::CHAIN_ID_DEVNET,
            admin_address: validator,
            validator_address: validator,
            validator_bls: vinx_state::GenesisBlsKey::from_secret(
                &vinx_crypto::BlsSecretKey::generate(),
                &validator,
                vinx_core::CHAIN_ID_DEVNET,
            ),
        });
        let (mut chain, _) = Chain::new_with_genesis(validator, 0);
        {
            let storage = Storage::open(&tmp.0).unwrap();
            storage.save(&mut state, &mut chain).unwrap();
            for h in 1..=40 {
                let b = make_test_block(h, chain.tip_hash(), validator);
                let c = cert_for(&b);
                chain.push(b, Some(c));
            }
            storage
                .write_state(Storage::serialize_incremental(&mut state, &mut chain).unwrap())
                .unwrap();
            // Timestamps equal heights: keep heights >= 25.
            assert_eq!(chain.prune_before(40, 15), 25);
            storage
                .write_state(Storage::serialize_incremental(&mut state, &mut chain).unwrap())
                .unwrap();
        }
        let storage = Storage::open(&tmp.0).unwrap();
        let (_, loaded) = storage.load().expect("pruned chain reloads");
        assert_eq!(loaded.base_height(), 25);
        assert_eq!(loaded.tip_height(), 40);
        assert_eq!(loaded.tip_hash(), chain.tip_hash());
        assert!(loaded.get_block(24).is_none());
        assert_eq!(loaded.tip(), chain.tip());
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
            validator_bls: vinx_state::GenesisBlsKey::from_secret(
                &vinx_crypto::BlsSecretKey::generate(),
                &validator,
                vinx_core::CHAIN_ID_DEVNET,
            ),
        });
        let (mut chain, _) = Chain::new_with_genesis(validator, 0);

        {
            let storage = Storage::open(&tmp.0).unwrap();
            storage.save(&mut state, &mut chain).unwrap();

            // Two new blocks → exactly two dirty rows in the next flush.
            for h in 1..=2 {
                let b = make_test_block(h, chain.tip_hash(), validator);
                let c = cert_for(&b);
                chain.push(b, Some(c));
            }
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
        assert_eq!(
            loaded.get_commit(2),
            chain.get_commit(2),
            "commit survives reload"
        );
    }

    // ADR 0082 — the double-sign guard: one value per (height, round, kind), durable,
    // idempotent for the same value, and the lock is recoverable after a restart.
    #[test]
    fn test_sign_guard_and_lock_recovery() {
        use vinx_core::{SignedVote, VoteKind};
        let tmp = Tmp::new();
        let vote = |kind, round, value: Option<Hash32>| SignedVote {
            kind,
            height: 7,
            round,
            value,
            validator: Address::from_bytes([1; 20]),
            signature: vec![],
        };
        {
            let storage = Storage::open(&tmp.0).unwrap();
            assert!(storage
                .claim_sign(&vote(VoteKind::Prevote, 0, Some([1; 32])))
                .unwrap());
            assert!(
                storage
                    .claim_sign(&vote(VoteKind::Prevote, 0, Some([1; 32])))
                    .unwrap(),
                "same value again is idempotent"
            );
            assert!(
                !storage
                    .claim_sign(&vote(VoteKind::Prevote, 0, None))
                    .unwrap(),
                "another value at the same (height, round, kind) is refused"
            );
            assert!(storage
                .claim_sign(&vote(VoteKind::Precommit, 0, Some([1; 32])))
                .unwrap());
            assert!(storage
                .claim_sign(&vote(VoteKind::Precommit, 2, Some([2; 32])))
                .unwrap());
            assert!(storage
                .claim_sign(&vote(VoteKind::Precommit, 3, None))
                .unwrap());
        }
        // After a restart the refusal still holds and the lock is recovered.
        let storage = Storage::open(&tmp.0).unwrap();
        assert!(!storage
            .claim_sign(&vote(VoteKind::Prevote, 0, None))
            .unwrap());
        assert_eq!(storage.last_precommit(7), Some((2, [2; 32])));
        assert_eq!(storage.last_precommit(8), None);
    }
}
