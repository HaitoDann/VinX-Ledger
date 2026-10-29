//! Stockage sur disque (redb) : blocs, certificats, état et garde de signature.
//!
//! Un bloc, son certificat et l'état qui en résulte sont écrits dans une seule
//! transaction : après un arrêt brutal, le nœud repart d'un état cohérent.
//!
//! Seuls les comptes modifiés par le bloc sont écrits, avec le reste de l'état
//! (escrow, validateurs, paramètres), qui est petit.

use anyhow::{Context, Result, bail};
use borsh::{BorshDeserialize, BorshSerialize};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::path::Path;
use vinx_consensus::{Evidence, SignGuard};
use vinx_core::{
    Account, Block, CommitCertificate, SignedAdminTransaction, SignedTransaction, TxBody, encode,
};
use vinx_crypto::Address;
use vinx_crypto::Hash;
use vinx_state::State;

const BLOCKS: TableDefinition<u64, &[u8]> = TableDefinition::new("blocks");
const CERTS: TableDefinition<u64, &[u8]> = TableDefinition::new("certificates");
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const ACCOUNTS: TableDefinition<&[u8; 20], &[u8]> = TableDefinition::new("accounts");
const EVIDENCE: TableDefinition<&[u8; 32], &[u8]> = TableDefinition::new("evidence");
/// Identifiant de transaction → hauteur de son bloc, pour la recherche. Suit
/// l'élagage des blocs.
const TX_INDEX: TableDefinition<&[u8; 32], u64> = TableDefinition::new("tx_index");
/// Nonce admin → action admin finalisée, pour l'historique de gouvernance.
/// Petit et jamais élagué : il survit aux blocs d'où il vient.
const ADMIN_LOG: TableDefinition<u64, &[u8]> = TableDefinition::new("admin_log");
/// (adresse, hauteur, rang dans le bloc) des transactions qui concernent une
/// adresse, pour l'historique du portefeuille. Suit l'élagage des blocs. Les
/// paiements de la trésorerie (admin) ont un rang marqué par [`ADMIN_RANK`].
const ADDR_INDEX: TableDefinition<(&[u8; 20], u64, u32), ()> = TableDefinition::new("addr_index");
/// (empreinte, hauteur, rang) des horodatages (`Attest`), pour les vérifier
/// sans identifiant de transaction. Suit l'élagage des blocs.
const ATTEST_INDEX: TableDefinition<(&[u8; 32], u64, u32), ()> =
    TableDefinition::new("attest_index");

/// Preuves d'équivoque conservées au plus.
pub const MAX_EVIDENCE: u64 = 1_000;

/// Version du format de la base. Le format 1 stockait l'état d'un seul tenant.
const FORMAT: &str = "format";
const FORMAT_VERSION: u8 = 2;
const GLOBALS: &str = "globals";

const LEGACY_STATE: &str = "state";
const GUARD: &str = "guard";
const GENESIS_ROOT: &str = "genesis_root";

pub struct Store {
    db: Database,
}

fn decode<T: BorshDeserialize>(bytes: &[u8], what: &str) -> Result<T> {
    borsh::from_slice(bytes).with_context(|| format!("{what} corrompu sur le disque"))
}

/// Action admin finalisée, telle que gardée dans le journal.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct AdminLogEntry {
    pub height: u64,
    pub timestamp: u64,
    pub tx: SignedAdminTransaction,
}

/// Adresses concernées par une transaction : l'émetteur et, s'il y en a, le
/// destinataire, le vendeur ou l'arbitre.
pub fn parties(t: &SignedTransaction) -> Vec<Address> {
    let mut out = vec![t.tx.sender];
    match t.tx.body.inner() {
        TxBody::Transfer { to, .. } | TxBody::TransferName { to, .. } => out.push(*to),
        TxBody::BatchTransfer { payments } => out.extend(payments.iter().map(|p| p.to)),
        TxBody::GrantMandate { payee, .. } | TxBody::RevokeMandate { payee } => out.push(*payee),
        TxBody::Collect { payer, .. } => out.push(*payer),
        TxBody::OpenDeal {
            seller, arbiter, ..
        } => {
            out.push(*seller);
            out.extend(*arbiter);
        }
        _ => {}
    }
    if let TxBody::Sponsored { sponsor, .. } = &t.tx.body {
        out.push(*sponsor);
    }
    out.sort();
    out.dedup();
    out
}

/// Bit de rang des transactions admin dans [`ADDR_INDEX`].
const ADMIN_RANK: u32 = 1 << 31;

/// Transaction trouvée dans l'historique d'une adresse.
pub enum HistoryEntry {
    User(SignedTransaction),
    /// Paiement de la trésorerie.
    Admin(SignedAdminTransaction),
}

/// Bénéficiaire d'un paiement de la trésorerie.
fn treasury_payee(t: &SignedAdminTransaction) -> Option<Address> {
    match &t.tx.action {
        vinx_core::AdminAction::TreasuryTransfer { to, .. } => Some(*to),
        _ => None,
    }
}

/// Clés de [`ADDR_INDEX`] d'un bloc.
fn address_keys(block: &Block) -> Result<Vec<(Address, u32)>> {
    let mut keys = Vec::new();
    for (rank, t) in block.txs.iter().enumerate() {
        let rank = u32::try_from(rank)?;
        keys.extend(parties(t).into_iter().map(|a| (a, rank)));
    }
    for (i, t) in block.admin_txs.iter().enumerate() {
        if let Some(to) = treasury_payee(t) {
            let rank = u32::try_from(i)? | ADMIN_RANK;
            keys.push((to, rank));
            keys.push((Address::treasury(), rank));
        }
    }
    Ok(keys)
}

/// Clés de [`ATTEST_INDEX`] d'un bloc.
fn attest_keys(block: &Block) -> Result<Vec<(Hash, u32)>> {
    let mut keys = Vec::new();
    for (rank, t) in block.txs.iter().enumerate() {
        if let TxBody::Attest { digest } = t.tx.body.inner() {
            keys.push((*digest, u32::try_from(rank)?));
        }
    }
    Ok(keys)
}

/// Indexe les transactions d'un bloc et journalise ses actions admin.
fn index_block(tx: &redb::WriteTransaction, block: &Block) -> Result<()> {
    let height = block.header.height;
    let mut index = tx.open_table(TX_INDEX)?;
    for t in &block.txs {
        index.insert(&t.id(), height)?;
    }
    let mut by_addr = tx.open_table(ADDR_INDEX)?;
    for (a, rank) in address_keys(block)? {
        by_addr.insert((a.as_bytes(), height, rank), ())?;
    }
    let mut attests = tx.open_table(ATTEST_INDEX)?;
    for (digest, rank) in attest_keys(block)? {
        attests.insert((&digest, height, rank), ())?;
    }
    let mut log = tx.open_table(ADMIN_LOG)?;
    for t in &block.admin_txs {
        index.insert(&t.id(), height)?;
        let entry = AdminLogEntry {
            height,
            timestamp: block.header.timestamp,
            tx: t.clone(),
        };
        log.insert(t.tx.nonce, encode(&entry).as_slice())?;
    }
    Ok(())
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
            tx.open_table(ACCOUNTS)?;
            tx.open_table(EVIDENCE)?;
            tx.open_table(TX_INDEX)?;
            tx.open_table(ADMIN_LOG)?;
            tx.open_table(ADDR_INDEX)?;
            tx.open_table(ATTEST_INDEX)?;
            let mut meta = tx.open_table(META)?;
            if meta.get(LEGACY_STATE)?.is_some() {
                bail!(
                    "base de données à l'ancien format : supprimez le dossier de données \
                     pour resynchroniser le nœud"
                );
            }
            let format = meta.get(FORMAT)?.and_then(|v| v.value().first().copied());
            if format.is_some_and(|f| f > FORMAT_VERSION) {
                bail!(
                    "base de données écrite par une version plus récente de vinx-node \
                     (format {}, cette version lit le format {FORMAT_VERSION}) : \
                     restaurez la sauvegarde faite avant la mise à jour",
                    format.unwrap_or_default()
                );
            }
            meta.insert(FORMAT, [FORMAT_VERSION].as_slice())?;
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
        let tx = self.db.begin_read()?;
        let meta = tx.open_table(META)?;
        let Some(globals) = meta.get(GLOBALS)? else {
            return Ok(None);
        };
        let mut accounts = Vec::new();
        for entry in tx.open_table(ACCOUNTS)?.iter()? {
            let (address, account) = entry?;
            accounts.push((
                Address::from_bytes(*address.value()),
                decode::<Account>(account.value(), "compte")?,
            ));
        }
        let state = State::from_parts(globals.value(), accounts.into_iter().collect())
            .context("état corrompu sur le disque")?;
        Ok(Some(state))
    }

    /// Enregistre un bloc finalisé, son certificat et le nouvel état, atomiquement.
    /// Seuls les comptes modifiés depuis la dernière écriture sont écrits.
    pub fn commit_block(
        &self,
        block: &Block,
        cert: &CommitCertificate,
        state: &mut State,
    ) -> Result<()> {
        let height = block.header.height;
        let tx = self.db.begin_write()?;
        tx.open_table(BLOCKS)?
            .insert(height, encode(block).as_slice())?;
        tx.open_table(CERTS)?
            .insert(height, encode(cert).as_slice())?;
        index_block(&tx, block)?;
        {
            let mut table = tx.open_table(ACCOUNTS)?;
            for (address, account) in state.accounts.take_dirty() {
                table.insert(address.as_bytes(), encode(&account).as_slice())?;
            }
        }
        tx.open_table(META)?
            .insert(GLOBALS, state.globals_bytes().as_slice())?;
        tx.commit()?;
        Ok(())
    }

    /// Remplace tout le contenu par un instantané vérifié : l'état complet, son
    /// bloc et son certificat. Les blocs antérieurs éventuels sont effacés.
    pub fn install_snapshot(
        &self,
        block: &Block,
        cert: &CommitCertificate,
        state: &mut State,
    ) -> Result<()> {
        let height = block.header.height;
        state.accounts.mark_all_dirty();
        let tx = self.db.begin_write()?;
        for table in [BLOCKS, CERTS] {
            tx.delete_table(table)?;
        }
        tx.delete_table(ACCOUNTS)?;
        // Les blocs effacés ne sont plus consultables : leur index non plus.
        tx.delete_table(TX_INDEX)?;
        tx.delete_table(ADDR_INDEX)?;
        tx.delete_table(ATTEST_INDEX)?;
        tx.open_table(BLOCKS)?
            .insert(height, encode(block).as_slice())?;
        tx.open_table(CERTS)?
            .insert(height, encode(cert).as_slice())?;
        index_block(&tx, block)?;
        {
            let mut table = tx.open_table(ACCOUNTS)?;
            for (address, account) in state.accounts.take_dirty() {
                table.insert(address.as_bytes(), encode(&account).as_slice())?;
            }
        }
        tx.open_table(META)?
            .insert(GLOBALS, state.globals_bytes().as_slice())?;
        tx.commit()?;
        Ok(())
    }

    /// Plus petite hauteur dont le bloc est encore conservé.
    pub fn lowest_height(&self) -> Result<Option<u64>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(BLOCKS)?;
        let first = table.first()?;
        Ok(first.map(|(h, _)| h.value()))
    }

    /// Supprime les blocs (et certificats) horodatés avant `cutoff_ms`, en gardant
    /// toujours le plus récent. Renvoie le nombre de blocs supprimés.
    pub fn prune(&self, cutoff_ms: u64) -> Result<u64> {
        let tx = self.db.begin_write()?;
        let mut removed = 0u64;
        {
            let mut blocks = tx.open_table(BLOCKS)?;
            let mut certs = tx.open_table(CERTS)?;
            let newest = blocks.last()?.map(|(h, _)| h.value());
            loop {
                let first = blocks
                    .first()?
                    .map(|(h, b)| (h.value(), b.value().to_vec()));
                let Some((height, bytes)) = first else { break };
                if Some(height) == newest {
                    break;
                }
                let block: Block = decode(&bytes, "bloc")?;
                if block.header.timestamp >= cutoff_ms {
                    break;
                }
                let mut index = tx.open_table(TX_INDEX)?;
                for t in &block.txs {
                    index.remove(&t.id())?;
                }
                let mut by_addr = tx.open_table(ADDR_INDEX)?;
                for (a, rank) in address_keys(&block)? {
                    by_addr.remove((a.as_bytes(), height, rank))?;
                }
                let mut attests = tx.open_table(ATTEST_INDEX)?;
                for (digest, rank) in attest_keys(&block)? {
                    attests.remove((&digest, height, rank))?;
                }
                for t in &block.admin_txs {
                    index.remove(&t.id())?;
                }
                blocks.remove(height)?;
                certs.remove(height)?;
                removed = removed.saturating_add(1);
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Enregistre une preuve d'équivoque. Renvoie `false` si elle était déjà
    /// connue ou si la limite de conservation est atteinte.
    pub fn record_evidence(&self, evidence: &Evidence) -> Result<bool> {
        let id = evidence.id();
        let tx = self.db.begin_write()?;
        let added = {
            let mut table = tx.open_table(EVIDENCE)?;
            if table.get(&id)?.is_some() || table.len()? >= MAX_EVIDENCE {
                false
            } else {
                table.insert(&id, encode(evidence).as_slice())?;
                true
            }
        };
        tx.commit()?;
        Ok(added)
    }

    pub fn evidence(&self) -> Result<Vec<Evidence>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(EVIDENCE)?;
        let mut out = Vec::new();
        for entry in table.iter()? {
            let (_, bytes) = entry?;
            out.push(decode(bytes.value(), "preuve")?);
        }
        Ok(out)
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

    /// Hauteur du bloc qui contient la transaction (utilisateur ou admin), s'il
    /// est encore conservé.
    pub fn tx_height(&self, id: &Hash) -> Result<Option<u64>> {
        let tx = self.db.begin_read()?;
        Ok(tx.open_table(TX_INDEX)?.get(id)?.map(|v| v.value()))
    }

    /// Les `limit` dernières transactions conservées qui concernent `address`,
    /// de la plus récente à la plus ancienne, avec la hauteur et l'horodatage
    /// de leur bloc.
    pub fn address_history(
        &self,
        address: &Address,
        limit: usize,
    ) -> Result<Vec<(u64, u64, HistoryEntry)>> {
        let tx = self.db.begin_read()?;
        let index = tx.open_table(ADDR_INDEX)?;
        let blocks = tx.open_table(BLOCKS)?;
        let key = address.as_bytes();
        let mut out = Vec::new();
        for entry in index
            .range((key, 0, 0)..=(key, u64::MAX, u32::MAX))?
            .rev()
            .take(limit)
        {
            let (k, _) = entry?;
            let (_, height, rank) = k.value();
            let Some(bytes) = blocks.get(height)? else {
                continue;
            };
            let block: Block = decode(bytes.value(), "bloc")?;
            let i = usize::try_from(rank & !ADMIN_RANK)?;
            let entry = if rank & ADMIN_RANK == 0 {
                block.txs.get(i).cloned().map(HistoryEntry::User)
            } else {
                block.admin_txs.get(i).cloned().map(HistoryEntry::Admin)
            };
            if let Some(e) = entry {
                out.push((height, block.header.timestamp, e));
            }
        }
        Ok(out)
    }

    /// Horodatages de `digest` encore dans les blocs conservés : hauteur,
    /// horodatage du bloc et transaction.
    pub fn attestations(&self, digest: &Hash) -> Result<Vec<(u64, u64, SignedTransaction)>> {
        let tx = self.db.begin_read()?;
        let index = tx.open_table(ATTEST_INDEX)?;
        let blocks = tx.open_table(BLOCKS)?;
        let mut out = Vec::new();
        for entry in index.range((digest, 0, 0)..=(digest, u64::MAX, u32::MAX))? {
            let (k, _) = entry?;
            let (_, height, rank) = k.value();
            let Some(bytes) = blocks.get(height)? else {
                continue;
            };
            let block: Block = decode(bytes.value(), "bloc")?;
            if let Some(t) = block.txs.get(usize::try_from(rank)?) {
                out.push((height, block.header.timestamp, t.clone()));
            }
        }
        Ok(out)
    }

    /// Les `limit` dernières actions admin finalisées, de la plus récente à la
    /// plus ancienne.
    pub fn admin_log(&self, limit: usize) -> Result<Vec<AdminLogEntry>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(ADMIN_LOG)?;
        let mut out = Vec::new();
        for entry in table.iter()?.rev().take(limit) {
            let (_, bytes) = entry?;
            out.push(decode(bytes.value(), "journal admin")?);
        }
        Ok(out)
    }

    /// Construit les index et le journal admin à partir des blocs conservés,
    /// pour une base créée avant leur existence. Sans effet si les deux index
    /// contiennent déjà des entrées.
    pub fn backfill_indexes(&self) -> Result<u64> {
        let tx = self.db.begin_write()?;
        let mut indexed = 0u64;
        {
            let filled =
                !tx.open_table(TX_INDEX)?.is_empty()? && !tx.open_table(ADDR_INDEX)?.is_empty()?;
            if filled {
                return Ok(0);
            }
            let blocks = tx.open_table(BLOCKS)?;
            for entry in blocks.iter()? {
                let (_, bytes) = entry?;
                let block: Block = decode(bytes.value(), "bloc")?;
                index_block(&tx, &block)?;
                indexed = indexed.saturating_add(1);
            }
        }
        tx.commit()?;
        Ok(indexed)
    }

    /// Nombre de blocs conservés (pour l'élagage et les tests).
    pub fn block_count(&self) -> Result<u64> {
        let tx = self.db.begin_read()?;
        Ok(tx.open_table(BLOCKS)?.len()?)
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)] // code de test
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
    fn refuses_a_database_from_a_newer_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let store = Store::open(&path, [1; 32]).unwrap();
        let tx = store.db.begin_write().unwrap();
        tx.open_table(META)
            .unwrap()
            .insert(FORMAT, [FORMAT_VERSION + 1].as_slice())
            .unwrap();
        tx.commit().unwrap();
        drop(store);
        let err = Store::open(&path, [1; 32]).err().unwrap();
        assert!(err.to_string().contains("plus récente"));
    }

    #[test]
    fn incremental_state_reloads_identically() {
        use vinx_core::{Account, BlockHeader};
        use vinx_crypto::KeyPair;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let k = || KeyPair::generate().public_key();
        let mut state = vinx_state::Genesis::legacy("vinx-test", 1, [k(), k(), k()], vec![k()])
            .unwrap()
            .into_state()
            .unwrap();
        let store = Store::open(&path, [1; 32]).unwrap();
        assert!(store.load_state().unwrap().is_none());
        let block = |height| Block {
            header: BlockHeader {
                protocol_version: 1,
                chain_id: "vinx-test".into(),
                height,
                timestamp: height,
                prev_hash: [0; 32],
                tx_root: [0; 32],
                state_root: [0; 32],
                validators_hash: [0; 32],
                proposer: 0,
            },
            admin_txs: vec![],
            txs: vec![],
        };
        let cert = CommitCertificate {
            height: 1,
            round: 0,
            block_hash: [0; 32],
            signatures: vec![],
        };
        // Bloc 1 : la trésorerie (compte de genèse) est écrite.
        state.height = 1;
        store.commit_block(&block(1), &cert, &mut state).unwrap();
        // Bloc 2 : un seul compte modifié.
        let alice = vinx_crypto::Address::from_bytes([9; 20]);
        let mut treasury = state.accounts[&vinx_crypto::Address::treasury()].clone();
        treasury.balance -= 5;
        state.set_account_for_tests(vinx_crypto::Address::treasury(), treasury);
        state.set_account_for_tests(
            alice,
            Account {
                balance: 5,
                ..Account::default()
            },
        );
        state.height = 2;
        store.commit_block(&block(2), &cert, &mut state).unwrap();
        drop(store);

        let reloaded = Store::open(&path, [1; 32])
            .unwrap()
            .load_state()
            .unwrap()
            .unwrap();
        assert_eq!(reloaded, state);
        assert_eq!(reloaded.state_root(), state.state_root());
        assert!(reloaded.check_invariant().is_ok());
    }

    #[test]
    fn evidence_is_recorded_once() {
        use vinx_consensus::SignedVote;
        use vinx_core::{Vote, VoteKind};
        use vinx_crypto::KeyPair;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db"), [1; 32]).unwrap();
        let kp = KeyPair::generate();
        let vote = |target| Vote {
            chain_id: "vinx-test".into(),
            kind: VoteKind::Prevote,
            height: 4,
            round: 0,
            block_hash: target,
        };
        let a = SignedVote::new(vote(Some([1; 32])), 0, &kp);
        let b = SignedVote::new(vote(None), 0, &kp);
        assert!(
            store
                .record_evidence(&Evidence::Votes(a.clone(), b.clone()))
                .unwrap()
        );
        // Même preuve, messages dans l'autre ordre : déjà connue.
        assert!(!store.record_evidence(&Evidence::Votes(b, a)).unwrap());
        assert_eq!(store.evidence().unwrap().len(), 1);
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
