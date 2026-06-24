use std::io;
use std::path::{Path, PathBuf};

use crate::chain::Chain;
use redb::{Database, ReadableTable, TableDefinition};
use vinx_state::WorldState;

/// Schema version stored in the meta table.  Increment when the storage layout changes
/// in a backwards-incompatible way so nodes refuse to start on stale data.
const STORAGE_VERSION: u64 = 1;

const STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("state");
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");

pub struct Storage {
    db: Database,
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
                // Read existing version, copy the u64 out so the borrow ends before we write.
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

        Self { db }
    }

    fn io_err(msg: impl std::fmt::Display) -> io::Error {
        io::Error::new(io::ErrorKind::Other, msg.to_string())
    }

    /// Persist current state and chain in a single ACID transaction.
    pub fn save(&self, state: &WorldState, chain: &Chain) -> io::Result<()> {
        let state_bytes =
            bincode::serialize(state).map_err(|e| Self::io_err(format!("serialize state: {e}")))?;
        let chain_bytes =
            bincode::serialize(chain).map_err(|e| Self::io_err(format!("serialize chain: {e}")))?;

        let tx = self.db.begin_write().map_err(|e| Self::io_err(e))?;
        {
            let mut tbl = tx.open_table(STATE).map_err(|e| Self::io_err(e))?;
            tbl.insert("world_state", state_bytes.as_slice())
                .map_err(|e| Self::io_err(e))?;
            tbl.insert("chain", chain_bytes.as_slice())
                .map_err(|e| Self::io_err(e))?;
        }
        tx.commit().map_err(|e| Self::io_err(e))?;
        Ok(())
    }

    /// Load persisted state and chain.  Returns `None` if no data has been written yet.
    pub fn load(&self) -> Option<(WorldState, Chain)> {
        let tx = self.db.begin_read().ok()?;
        let tbl = tx.open_table(STATE).ok()?;

        let state_bytes = tbl.get("world_state").ok()??.value().to_vec();
        let chain_bytes = tbl.get("chain").ok()??.value().to_vec();

        let state: WorldState = bincode::deserialize(&state_bytes)
            .map_err(|e| tracing::warn!("Cannot deserialize state: {e}"))
            .ok()?;
        let mut chain: Chain = bincode::deserialize(&chain_bytes)
            .map_err(|e| tracing::warn!("Cannot deserialize chain: {e}"))
            .ok()?;
        chain.rebuild_tx_index();
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
}

/// Removes the redb database file — full data wipe.
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("vinx.redb"));
}
