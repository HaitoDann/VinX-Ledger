use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::chain::Chain;
use ahash::AHashMap;
use redb::{Database, ReadableTable, TableDefinition};
use vinx_core::{Account, Transaction};
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
const STORAGE_VERSION: u64 = 7;

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
    /// Addresses of accounts reaped this flush (ADR 0026): their rows must be **deleted**
    /// from the accounts table, not merely absent from `account_rows`, or they would
    /// resurrect on the next load. Ignored when `replace_accounts` (the table is wiped).
    pub account_deletes: Vec<[u8; 20]>,
    /// When true, the accounts table is wiped before writing `account_rows`
    /// (used by full snapshot import to drop rows no longer present).
    pub replace_accounts: bool,
    pub chain: Vec<u8>,
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
        let (chain, tx_index, account_tx_index) = Self::serialize_chain(chain)?;
        Ok(StateWrite {
            meta,
            account_rows,
            account_deletes,
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
            signatures: vec![],
        };
        chain.push(block);

        // Save at the current version, then simulate an older (v6) on-disk marker.
        {
            let storage = Storage::open(&tmp.0).unwrap();
            storage.save(&mut state, &chain).unwrap();
        }
        set_version(&tmp.0, 6);

        // Reopen → migrates in place (no wipe); load rebuilds the derived index.
        let storage = Storage::open(&tmp.0).expect("v6 must migrate, not wipe");
        let (loaded_state, loaded_chain) = storage.load().expect("data survives migration");

        assert_eq!(
            loaded_state.account_balance(&admin),
            Amount::from_vinx(500),
            "account balance must survive migration"
        );
        assert_eq!(loaded_chain.tip_height(), 1, "chain must survive migration");
        assert!(
            loaded_chain.get_tx_by_hash(&tx_hash).is_some(),
            "tx index must be rebuilt after migration"
        );
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
        let (chain, _) = Chain::new_with_genesis(validator, 0);

        // Fund bob with exactly amount + fee so a single transfer drains him to zero.
        let amount = Amount::from_vinx(10);
        let fee = amount.calculate_fee(state.base_fee);
        let total = amount.checked_add(fee).unwrap();
        state.credit_for_test(bob, total);
        state.circulating_supply = total;

        // Scoped so the redb handle (and its file lock) is released before reopening.
        {
            let storage = Storage::open(&tmp.0).unwrap();
            storage.save(&mut state, &chain).unwrap(); // bob's row is on disk

            // Drain bob → he is reaped from the map.
            let tx = Transaction::new_transfer(&bob_kp, carol, amount, fee, 0);
            state.apply_transaction(&tx).unwrap();
            assert!(state.get_account(&bob).is_none(), "bob reaped in memory");

            // Persist incrementally.
            let w = Storage::serialize_incremental(&mut state, &chain).unwrap();
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
}
