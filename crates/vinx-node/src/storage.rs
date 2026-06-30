use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::chain::Chain;
use redb::{Database, ReadableTable, TableDefinition};
use vinx_state::WorldState;
use zstd;

/// Schema version stored in the meta table.  Increment when the storage layout changes
/// in a backwards-incompatible way so nodes refuse to start on stale data.
/// v2: blobs are zstd-compressed (level 3) before insertion.
/// v3: tx_index and account_tx_index are persisted (no rebuild_tx_index on boot).
const STORAGE_VERSION: u64 = 3;

/// zstd compression level — level 3 is the sweet spot: ~60-70% size reduction,
/// negligible latency compared to disk I/O.
const ZSTD_LEVEL: i32 = 3;

const STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("state");
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");

pub struct Storage {
    db: Arc<Database>,
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
        io::Error::new(io::ErrorKind::Other, msg.to_string())
    }

    fn compress(data: &[u8]) -> io::Result<Vec<u8>> {
        zstd::encode_all(data, ZSTD_LEVEL)
            .map_err(|e| Self::io_err(format!("zstd compress: {e}")))
    }

    fn decompress(data: &[u8]) -> io::Result<Vec<u8>> {
        zstd::decode_all(data).map_err(|e| Self::io_err(format!("zstd decompress: {e}")))
    }

    /// Serializes state and chain to raw bytes while the caller holds the read locks.
    /// Returns (state_bytes, chain_bytes, tx_index_bytes, account_tx_index_bytes).
    /// Compression and disk I/O happen later in `save_serialized` off the async executor.
    pub fn serialize(
        state: &WorldState,
        chain: &Chain,
    ) -> io::Result<(Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> {
        let state_bytes = bincode::serialize(state)
            .map_err(|e| Self::io_err(format!("serialize state: {e}")))?;
        let chain_bytes = bincode::serialize(chain)
            .map_err(|e| Self::io_err(format!("serialize chain: {e}")))?;
        let (tx_index, account_tx_index) = chain.export_tx_indexes();
        let tx_index_bytes = bincode::serialize(tx_index)
            .map_err(|e| Self::io_err(format!("serialize tx_index: {e}")))?;
        let account_tx_index_bytes = bincode::serialize(account_tx_index)
            .map_err(|e| Self::io_err(format!("serialize account_tx_index: {e}")))?;
        Ok((state_bytes, chain_bytes, tx_index_bytes, account_tx_index_bytes))
    }

    /// Compresses and writes pre-serialized blobs to redb in a single ACID transaction.
    /// Designed to run inside `tokio::task::spawn_blocking` — no async, pure blocking I/O.
    pub fn save_serialized(
        &self,
        state_bytes: Vec<u8>,
        chain_bytes: Vec<u8>,
        tx_index_bytes: Vec<u8>,
        account_tx_index_bytes: Vec<u8>,
    ) -> io::Result<()> {
        let state_c = Self::compress(&state_bytes)?;
        let chain_c = Self::compress(&chain_bytes)?;
        let tx_idx_c = Self::compress(&tx_index_bytes)?;
        let acc_idx_c = Self::compress(&account_tx_index_bytes)?;

        tracing::debug!(
            state_raw = state_bytes.len(),
            state_compressed = state_c.len(),
            chain_raw = chain_bytes.len(),
            chain_compressed = chain_c.len(),
            "Persisting compressed state"
        );

        let tx = self.db.begin_write().map_err(|e| Self::io_err(e))?;
        {
            let mut tbl = tx.open_table(STATE).map_err(|e| Self::io_err(e))?;
            tbl.insert("world_state", state_c.as_slice())
                .map_err(|e| Self::io_err(e))?;
            tbl.insert("chain", chain_c.as_slice())
                .map_err(|e| Self::io_err(e))?;
            tbl.insert("tx_index", tx_idx_c.as_slice())
                .map_err(|e| Self::io_err(e))?;
            tbl.insert("account_tx_index", acc_idx_c.as_slice())
                .map_err(|e| Self::io_err(e))?;
        }
        tx.commit().map_err(|e| Self::io_err(e))?;
        Ok(())
    }

    /// Persist current state and chain in a single ACID transaction (synchronous path,
    /// used from blocking contexts such as tests and main.rs initial save).
    pub fn save(&self, state: &WorldState, chain: &Chain) -> io::Result<()> {
        let (sb, cb, tib, atib) = Self::serialize(state, chain)?;
        self.save_serialized(sb, cb, tib, atib)
    }

    /// Load persisted state and chain.  Returns `None` if no data has been written yet.
    pub fn load(&self) -> Option<(WorldState, Chain)> {
        let tx = self.db.begin_read().ok()?;
        let tbl = tx.open_table(STATE).ok()?;

        let state_bytes = Self::decompress(tbl.get("world_state").ok()??.value())
            .map_err(|e| tracing::warn!("Cannot decompress state: {e}"))
            .ok()?;
        let chain_bytes = Self::decompress(tbl.get("chain").ok()??.value())
            .map_err(|e| tracing::warn!("Cannot decompress chain: {e}"))
            .ok()?;

        let state: WorldState = bincode::deserialize(&state_bytes)
            .map_err(|e| tracing::warn!("Cannot deserialize state: {e}"))
            .ok()?;
        let mut chain: Chain = bincode::deserialize(&chain_bytes)
            .map_err(|e| tracing::warn!("Cannot deserialize chain: {e}"))
            .ok()?;

        // Restore persisted indexes — O(1) vs O(blocks×txs) rebuild
        let indexes_restored = (|| -> Option<()> {
            let tx_index_bytes =
                Self::decompress(tbl.get("tx_index").ok()??.value()).ok()?;
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

    pub fn exists(&self) -> bool {
        let Ok(tx) = self.db.begin_read() else {
            return false;
        };
        let Ok(tbl) = tx.open_table(STATE) else {
            return false;
        };
        tbl.get("world_state").ok().flatten().is_some()
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
