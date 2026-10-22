//! Stockage sur disque (redb) : blocs, certificats, état et garde de signature.
//!
//! Un bloc, son certificat et l'état qui en résulte sont écrits dans une seule
//! transaction : après un arrêt brutal, le nœud repart d'un état cohérent.

use anyhow::{Context, Result, bail};
use borsh::{BorshDeserialize, BorshSerialize};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::path::Path;
use vinx_consensus::SignGuard;
use vinx_core::{Block, CommitCertificate, encode};
use vinx_crypto::Hash;
use vinx_state::State;

const BLOCKS: TableDefinition<u64, &[u8]> = TableDefinition::new("blocks");
const CERTS: TableDefinition<u64, &[u8]> = TableDefinition::new("certificates");
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

const STATE: &str = "state";
const GUARD: &str = "guard";
const GENESIS_ROOT: &str = "genesis_root";

pub struct Store {
    db: Database,
}

fn decode<T: BorshDeserialize>(bytes: &[u8], what: &str) -> Result<T> {
    borsh::from_slice(bytes).with_context(|| format!("{what} corrompu sur le disque"))
}

impl Store {
    /// Ouvre (ou crée) la base. Refuse une base créée pour une autre genèse.
    pub fn open(path: &Path, genesis_root: Hash) -> Result<Self> {
        let db = Database::create(path)
            .with_context(|| format!("ouverture de la base {}", path.display()))?;
        let tx = db.begin_write()?;
        {
            tx.open_table(BLOCKS)?;
            tx.open_table(CERTS)?;
            let mut meta = tx.open_table(META)?;
            let existing = meta.get(GENESIS_ROOT)?.map(|v| v.value().to_vec());
            match existing {
                Some(root) if root != genesis_root => {
                    bail!("cette base de données appartient à une autre genèse")
                }
                Some(_) => {}
                None => {
                    meta.insert(GENESIS_ROOT, genesis_root.as_slice())?;
                }
            }
        }
        tx.commit()?;
        Ok(Self { db })
    }

    fn meta<T: BorshDeserialize>(&self, key: &str) -> Result<Option<T>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(META)?;
        let value = table.get(key)?;
        value.map(|v| decode(v.value(), key)).transpose()
    }

    fn put_meta<T: BorshSerialize>(&self, key: &str, value: &T) -> Result<()> {
        let tx = self.db.begin_write()?;
        tx.open_table(META)?.insert(key, encode(value).as_slice())?;
        tx.commit()?;
        Ok(())
    }

    /// Dernier état enregistré, ou `None` si la chaîne n'a jamais démarré.
    pub fn load_state(&self) -> Result<Option<State>> {
        self.meta(STATE)
    }

    /// Enregistre un bloc finalisé, son certificat et le nouvel état, atomiquement.
    pub fn commit_block(
        &self,
        block: &Block,
        cert: &CommitCertificate,
        state: &State,
    ) -> Result<()> {
        let height = block.header.height;
        let tx = self.db.begin_write()?;
        tx.open_table(BLOCKS)?
            .insert(height, encode(block).as_slice())?;
        tx.open_table(CERTS)?
            .insert(height, encode(cert).as_slice())?;
        tx.open_table(META)?
            .insert(STATE, encode(state).as_slice())?;
        tx.commit()?;
        Ok(())
    }

    pub fn block(&self, height: u64) -> Result<Option<Block>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(BLOCKS)?;
        let value = table.get(height)?;
        value.map(|v| decode(v.value(), "bloc")).transpose()
    }

    pub fn certificate(&self, height: u64) -> Result<Option<CommitCertificate>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(CERTS)?;
        let value = table.get(height)?;
        value.map(|v| decode(v.value(), "certificat")).transpose()
    }

    pub fn load_guard(&self) -> Result<SignGuard> {
        Ok(self.meta(GUARD)?.unwrap_or_default())
    }

    /// Persiste le garde. À appeler **avant** d'envoyer la signature autorisée.
    pub fn save_guard(&self, guard: &SignGuard) -> Result<()> {
        self.put_meta(GUARD, guard)
    }

    /// Nombre de blocs conservés (pour l'élagage et les tests).
    pub fn block_count(&self) -> Result<u64> {
        let tx = self.db.begin_read()?;
        Ok(tx.open_table(BLOCKS)?.len()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_database_from_another_genesis() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        drop(Store::open(&path, [1; 32]).unwrap());
        assert!(Store::open(&path, [2; 32]).is_err());
        assert!(Store::open(&path, [1; 32]).is_ok());
    }

    #[test]
    fn guard_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let store = Store::open(&path, [1; 32]).unwrap();
        let mut guard = store.load_guard().unwrap();
        assert!(guard.allow(1, 0, vinx_consensus::guard::Step::Prevote, Some([3; 32])));
        store.save_guard(&guard).unwrap();
        drop(store);
        let mut restored = Store::open(&path, [1; 32]).unwrap().load_guard().unwrap();
        assert!(!restored.allow(1, 0, vinx_consensus::guard::Step::Prevote, Some([4; 32])));
    }
}
