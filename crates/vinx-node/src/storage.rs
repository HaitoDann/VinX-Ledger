use std::io;
use std::path::{Path, PathBuf};

use crate::chain::Chain;
use vinx_state::WorldState;

pub struct Storage {
    dir: PathBuf,
}

impl Storage {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.bin")
    }

    fn chain_path(&self) -> PathBuf {
        self.dir.join("chain.bin")
    }

    /// Atomically saves state and chain to disk.
    /// Writes to temp files first, then renames — safe against mid-write crashes.
    pub fn save(&self, state: &WorldState, chain: &Chain) -> io::Result<()> {
        let state_bytes =
            bincode::serialize(state).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let chain_bytes =
            bincode::serialize(chain).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let state_tmp = self.dir.join("state.bin.tmp");
        let chain_tmp = self.dir.join("chain.bin.tmp");

        std::fs::write(&state_tmp, &state_bytes)?;
        std::fs::write(&chain_tmp, &chain_bytes)?;

        std::fs::rename(&state_tmp, self.state_path())?;
        std::fs::rename(&chain_tmp, self.chain_path())?;

        Ok(())
    }

    /// Returns `Some((state, chain))` if valid persisted data exists, `None` otherwise.
    pub fn load(&self) -> Option<(WorldState, Chain)> {
        let state_path = self.state_path();
        let chain_path = self.chain_path();

        if !state_path.exists() || !chain_path.exists() {
            return None;
        }

        let state_bytes = std::fs::read(&state_path)
            .map_err(|e| tracing::warn!("Cannot read state file: {e}"))
            .ok()?;
        let chain_bytes = std::fs::read(&chain_path)
            .map_err(|e| tracing::warn!("Cannot read chain file: {e}"))
            .ok()?;

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
        self.state_path().exists() && self.chain_path().exists()
    }
}

/// Removes persisted data — equivalent to `rm devnet/state.bin devnet/chain.bin`.
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("state.bin"));
    let _ = std::fs::remove_file(dir.join("chain.bin"));
}
