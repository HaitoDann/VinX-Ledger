use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::chain::Chain;
use redb::{Database, ReadableTable, TableDefinition};
use vinx_core::{Account, Transaction};
use vinx_state::WorldState;
use zstd;

/// Schema version stored in the meta table.  Increment when the storage layout changes
/// in a backwards-incompatible way so nodes refuse to start on stale data.
/// v2: blobs are zstd-compressed (level 3) before insertion.
/// v3: tx_index and account_tx_index are persisted (no rebuild_tx_index on boot).
/// v4: accounts persisted per-key in a dedicated table; only changed rows are
///     written each block (O(dirty) instead of O(total accounts) per persist).
/// v5: addresses are stored as raw 20 bytes (Address is `[u8; 20]`); the accounts
///     table is keyed by those bytes and bincode encodes addresses as 20 bytes.
const STORAGE_VERSION: u64 = 5;

/// zstd compression level — level 3 is the sweet spot: ~60-70% size reduction,
/// negligible latency compared to disk I/O.
const ZSTD_LEVEL: i32 = 3;

const STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("state");
/// Per-account rows: bech32 address → bincode(Account), stored uncompressed.
/// Accounts are tiny (~100 B); per-row zstd framing would cost more than it saves.
const ACCOUNTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("accounts");
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
    /// When true, the accounts table is wiped before writing `account_rows`
    /// (used by full snapshot import to drop rows no longer present).
    pub replace_accounts: bool,
    pub chain: Vec<u8>,
    pub tx_index: Vec<u8>,
    pub account_tx_index: Vec<u8>,
}

impl Storage {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir: PathBuf = dir.into();
        std::fs::create_dir_all(&dir).expect("create data directory");
        let path = dir.join("vinx.redb");
        let db = Database::create(&path).expect("open redb database");

        {
            let tx = db.begin_write().expect("begin write");
            {
                let mut meta = tx.open_table(META).expect("open meta table");
                let existing_version: Option<u64> = meta
                    .get("schema_version")
                    .expect("read schema version")
                    .map(|v| v.value());
                match existing_version {
                    None => {
                        meta.insert("schema_version", STORAGE_VERSION)
                            .expect("write schema version");
                    }
                    Some(existing) => {
                        assert!(
                            existing == STORAGE_VERSION,
                            "storage schema version mismatch: found {existing}, expected {STORAGE_VERSION}. \
                             Delete the data directory to start fresh."
                        );
                    }
                }
            }
            tx.commit().expect("commit schema version");
        }

        Self { db: Arc::new(db) }
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

    /// Serializes the chain and its tx indexes (no accounts, no meta).
    fn serialize_chain(chain: &Chain) -> io::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let chain_bytes =
            bincode::serialize(chain).map_err(|e| Self::io_err(format!("serialize chain: {e}")))?;
        let (tx_index, account_tx_index) = chain.export_tx_indexes();
        let tx_index_bytes = bincode::serialize(tx_index)
            .map_err(|e| Self::io_err(format!("serialize tx_index: {e}")))?;
        let account_tx_index_bytes = bincode::serialize(account_tx_index)
            .map_err(|e| Self::io_err(format!("serialize account_tx_index: {e}")))?;
        Ok((chain_bytes, tx_index_bytes, account_tx_index_bytes))
    }

    /// Builds an incremental `StateWrite`: the meta blob plus only the accounts
    /// changed since the last flush. Call while holding the state write lock —
    /// `serialize_meta` moves the accounts map out and back, and `take_persist_dirty`
    /// drains the change set. Compression + I/O happen later in `write_state`.
    pub fn serialize_incremental(state: &mut WorldState, chain: &Chain) -> io::Result<StateWrite> {
        let meta = state
            .serialize_meta()
            .map_err(|e| Self::io_err(format!("serialize meta: {e}")))?;
        let dirty = state.take_persist_dirty();
        let mut account_rows = Vec::with_capacity(dirty.len());
        for addr in dirty {
            if let Some(acc) = state.account_by_addr(&addr) {
                let bytes = bincode::serialize(acc)
                    .map_err(|e| Self::io_err(format!("serialize account: {e}")))?;
                account_rows.push((*addr.as_bytes(), bytes));
            }
        }
        let (chain, tx_index, account_tx_index) = Self::serialize_chain(chain)?;
        Ok(StateWrite {
            meta,
            account_rows,
            replace_accounts: false,
            chain,
            tx_index,
            account_tx_index,
        })
    }

    /// Builds a full `StateWrite` containing every account, flagged to wipe any
    /// stale rows first. Used for the initial genesis save and snapshot import.
    pub fn serialize_full(state: &mut WorldState, chain: &Chain) -> io::Result<StateWrite> {
        state.mark_all_persist_dirty();
        let mut w = Self::serialize_incremental(state, chain)?;
        w.replace_accounts = true;
        Ok(w)
    }

    /// Compresses and writes a `StateWrite` in a single ACID transaction.
    /// Designed to run inside `tokio::task::spawn_blocking` — pure blocking I/O.
    pub fn write_state(&self, w: StateWrite) -> io::Result<()> {
        let meta_c = Self::compress(&w.meta)?;
        let chain_c = Self::compress(&w.chain)?;
        let tx_idx_c = Self::compress(&w.tx_index)?;
        let acc_idx_c = Self::compress(&w.account_tx_index)?;

        tracing::debug!(
            meta_raw = w.meta.len(),
            meta_compressed = meta_c.len(),
            account_rows = w.account_rows.len(),
            replace = w.replace_accounts,
            "Persisting state (incremental)"
        );

        let tx = self.db.begin_write().map_err(Self::io_err)?;
        if w.replace_accounts {
            // Drop and recreate the accounts table to clear rows no longer present.
            tx.delete_table(ACCOUNTS).map_err(Self::io_err)?;
        }
        {
            let mut tbl = tx.open_table(STATE).map_err(Self::io_err)?;
            tbl.insert("world_state_meta", meta_c.as_slice())
                .map_err(Self::io_err)?;
            tbl.insert("chain", chain_c.as_slice())
                .map_err(Self::io_err)?;
            tbl.insert("tx_index", tx_idx_c.as_slice())
                .map_err(Self::io_err)?;
            tbl.insert("account_tx_index", acc_idx_c.as_slice())
                .map_err(Self::io_err)?;
        }
        {
            let mut atbl = tx.open_table(ACCOUNTS).map_err(Self::io_err)?;
            for (addr, bytes) in &w.account_rows {
                atbl.insert(addr.as_slice(), bytes.as_slice())
                    .map_err(Self::io_err)?;
            }
        }
        tx.commit().map_err(Self::io_err)?;
        Ok(())
    }

    /// Full synchronous save (initial genesis bootstrap, tests, snapshot import).
    /// Writes every account and clears any stale rows.
    pub fn save(&self, state: &mut WorldState, chain: &Chain) -> io::Result<()> {
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

        let chain_bytes = Self::decompress(tbl.get("chain").ok()??.value())
            .map_err(|e| tracing::warn!("Cannot decompress chain: {e}"))
            .ok()?;
        let mut chain: Chain = bincode::deserialize(&chain_bytes)
            .map_err(|e| tracing::warn!("Cannot deserialize chain: {e}"))
            .ok()?;

        // Restore persisted indexes — O(1) vs O(blocks×txs) rebuild
        let indexes_restored = (|| -> Option<()> {
            let tx_index_bytes = Self::decompress(tbl.get("tx_index").ok()??.value()).ok()?;
            let account_tx_index_bytes =
                Self::decompress(tbl.get("account_tx_index").ok()??.value()).ok()?;
            let tx_index: HashMap<String, (u64, u32)> =
                bincode::deserialize(&tx_index_bytes).ok()?;
            let account_tx_index: HashMap<String, Vec<String>> =
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
