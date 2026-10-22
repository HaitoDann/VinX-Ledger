//! Transactions en attente d'inclusion.
//!
//! Une transaction n'est acceptée que si elle s'applique à l'état courant après
//! les transactions déjà en attente : le mempool ne contient que des
//! transactions incluables, dans un ordre valide.

use std::collections::BTreeSet;
use vinx_core::{SignedAdminTransaction, SignedTransaction};
use vinx_crypto::Hash;
use vinx_state::{State, TxError};

/// Capacité maximale, pour borner la mémoire.
pub const MAX_PENDING: usize = 20_000;

#[derive(Debug, PartialEq, Eq)]
pub enum MempoolError {
    Duplicate,
    Full,
    Invalid(TxError),
    InvalidAdmin,
}

impl std::fmt::Display for MempoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MempoolError::Duplicate => write!(f, "transaction déjà en attente"),
            MempoolError::Full => write!(f, "mempool plein"),
            MempoolError::Invalid(e) => write!(f, "transaction refusée : {e}"),
            MempoolError::InvalidAdmin => write!(f, "transaction admin refusée"),
        }
    }
}

pub struct Mempool {
    txs: Vec<SignedTransaction>,
    admin: Vec<SignedAdminTransaction>,
    ids: BTreeSet<Hash>,
    /// État courant avec les transactions utilisateur en attente appliquées.
    pending: State,
}

impl Mempool {
    pub fn new(state: &State) -> Self {
        Self {
            txs: Vec::new(),
            admin: Vec::new(),
            ids: BTreeSet::new(),
            pending: state.clone(),
        }
    }

    pub fn len(&self) -> usize {
        self.txs.len().saturating_add(self.admin.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn txs(&self) -> &[SignedTransaction] {
        &self.txs
    }

    pub fn admin_txs(&self) -> &[SignedAdminTransaction] {
        &self.admin
    }

    pub fn add(&mut self, tx: SignedTransaction) -> Result<Hash, MempoolError> {
        let id = tx.id();
        if self.ids.contains(&id) {
            return Err(MempoolError::Duplicate);
        }
        if self.len() >= MAX_PENDING {
            return Err(MempoolError::Full);
        }
        self.pending.apply_tx(&tx).map_err(MempoolError::Invalid)?;
        self.ids.insert(id);
        self.txs.push(tx);
        Ok(id)
    }

    /// Contrôle sans état des approbations ; le reste est vérifié à l'inclusion.
    pub fn add_admin(&mut self, tx: SignedAdminTransaction) -> Result<Hash, MempoolError> {
        let id = tx.id();
        if self.ids.contains(&id) {
            return Err(MempoolError::Duplicate);
        }
        if self.len() >= MAX_PENDING {
            return Err(MempoolError::Full);
        }
        if tx.tx.chain_id != self.pending.chain_id
            || tx.tx.nonce <= self.pending.admin.nonce
            || tx.verify_approvals(&self.pending.admin.signers).is_err()
        {
            return Err(MempoolError::InvalidAdmin);
        }
        self.ids.insert(id);
        self.admin.push(tx);
        Ok(id)
    }

    /// Après un bloc : retire les transactions incluses ou devenues invalides.
    pub fn reset(&mut self, state: &State, included: &BTreeSet<Hash>) {
        let txs = std::mem::take(&mut self.txs);
        let admin = std::mem::take(&mut self.admin);
        self.ids.clear();
        self.pending = state.clone();
        for tx in txs.into_iter().filter(|t| !included.contains(&t.id())) {
            let _ = self.add(tx);
        }
        for tx in admin.into_iter().filter(|t| !included.contains(&t.id())) {
            let _ = self.add_admin(tx);
        }
    }
}
