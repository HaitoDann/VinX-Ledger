//! Transactions en attente d'inclusion.
//!
//! Une transaction n'est acceptée que si elle s'applique à l'état courant après
//! les transactions déjà en attente : le mempool ne contient que des
//! transactions incluables, dans un ordre valide.
//!
//! Protections contre l'inondation : au plus [`MAX_PER_ACCOUNT`] transactions
//! en attente par compte et [`MAX_PENDING`] au total. Mempool plein, une
//! transaction n'entre qu'en payant plus de frais que la moins chère des
//! transactions évinçables, qui est alors retirée.

use std::collections::{BTreeMap, BTreeSet};
use vinx_core::{SignedAdminTransaction, SignedTransaction};
use vinx_crypto::Address;
use vinx_crypto::Hash;
use vinx_state::{State, TxError};

/// Capacité maximale, pour borner la mémoire.
pub const MAX_PENDING: usize = 20_000;

/// Transactions en attente au plus par compte.
pub const MAX_PER_ACCOUNT: usize = 100;

#[derive(Debug, PartialEq, Eq)]
pub enum MempoolError {
    Duplicate,
    Full,
    AccountLimit,
    Invalid(TxError),
    InvalidAdmin,
}

impl std::fmt::Display for MempoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MempoolError::Duplicate => write!(f, "transaction déjà en attente"),
            MempoolError::Full => write!(f, "mempool plein : frais insuffisants pour y entrer"),
            MempoolError::AccountLimit => write!(
                f,
                "trop de transactions en attente pour ce compte (maximum {MAX_PER_ACCOUNT})"
            ),
            MempoolError::Invalid(e) => write!(f, "transaction refusée : {e}"),
            MempoolError::InvalidAdmin => write!(f, "transaction admin refusée"),
        }
    }
}

pub struct Mempool {
    txs: Vec<SignedTransaction>,
    admin: Vec<SignedAdminTransaction>,
    ids: BTreeSet<Hash>,
    per_account: BTreeMap<Address, usize>,
    capacity: usize,
    /// Dernier état finalisé.
    base: State,
    /// `base` avec les transactions utilisateur en attente appliquées.
    pending: State,
}

impl Mempool {
    pub fn new(state: &State) -> Self {
        Self::with_capacity(state, MAX_PENDING)
    }

    pub fn with_capacity(state: &State, capacity: usize) -> Self {
        Self {
            capacity,
            txs: Vec::new(),
            admin: Vec::new(),
            ids: BTreeSet::new(),
            per_account: BTreeMap::new(),
            base: state.clone(),
            pending: state.clone(),
        }
    }

    pub fn len(&self) -> usize {
        self.txs.len().saturating_add(self.admin.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Dans l'ordre d'arrivée, qui respecte l'ordre des nonces de chaque compte.
    pub fn txs(&self) -> &[SignedTransaction] {
        &self.txs
    }

    /// Candidates pour un bloc : frais décroissants, ordre d'arrivée à frais égaux.
    pub fn by_priority(&self) -> Vec<SignedTransaction> {
        let mut out = self.txs.clone();
        out.sort_by_key(|t| std::cmp::Reverse(t.tx.fee));
        out
    }

    pub fn admin_txs(&self) -> &[SignedAdminTransaction] {
        &self.admin
    }

    pub fn add(&mut self, tx: SignedTransaction) -> Result<Hash, MempoolError> {
        let id = tx.id();
        if self.ids.contains(&id) {
            return Err(MempoolError::Duplicate);
        }
        let sender = tx.tx.sender;
        if self.per_account.get(&sender).copied().unwrap_or(0) >= MAX_PER_ACCOUNT {
            return Err(MempoolError::AccountLimit);
        }
        if self.len() >= self.capacity {
            self.evict_cheaper_than(tx.tx.fee)?;
        }
        self.pending.apply_tx(&tx).map_err(MempoolError::Invalid)?;
        self.ids.insert(id);
        let count = self.per_account.entry(sender).or_insert(0);
        *count = count.saturating_add(1);
        self.txs.push(tx);
        Ok(id)
    }

    /// Retire la transaction évinçable la moins chère si elle paie moins que `fee`.
    ///
    /// Seule la dernière transaction en attente d'un compte est évinçable :
    /// retirer une transaction intermédiaire rendrait les suivantes invalides.
    fn evict_cheaper_than(&mut self, fee: u64) -> Result<(), MempoolError> {
        let mut last_of_sender: BTreeMap<Address, usize> = BTreeMap::new();
        for (i, t) in self.txs.iter().enumerate() {
            last_of_sender.insert(t.tx.sender, i);
        }
        let victim = last_of_sender
            .values()
            .copied()
            .filter_map(|i| Some((i, self.txs.get(i)?.tx.fee)))
            .filter(|(_, f)| *f < fee)
            .min_by_key(|(_, f)| *f)
            .map(|(i, _)| i)
            .ok_or(MempoolError::Full)?;
        let mut txs = std::mem::take(&mut self.txs);
        txs.remove(victim);
        let admin = std::mem::take(&mut self.admin);
        self.rebuild(txs, admin);
        Ok(())
    }

    /// Réapplique les transactions données à partir de l'état finalisé.
    fn rebuild(&mut self, txs: Vec<SignedTransaction>, admin: Vec<SignedAdminTransaction>) {
        self.ids.clear();
        self.per_account.clear();
        self.pending = self.base.clone();
        for tx in txs {
            let _ = self.add(tx);
        }
        for tx in admin {
            let _ = self.add_admin(tx);
        }
    }

    /// Contrôle sans état des approbations ; le reste est vérifié à l'inclusion.
    pub fn add_admin(&mut self, tx: SignedAdminTransaction) -> Result<Hash, MempoolError> {
        let id = tx.id();
        if self.ids.contains(&id) {
            return Err(MempoolError::Duplicate);
        }
        if self.len() >= self.capacity {
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
        let txs = std::mem::take(&mut self.txs)
            .into_iter()
            .filter(|t| !included.contains(&t.id()))
            .collect();
        let admin = std::mem::take(&mut self.admin)
            .into_iter()
            .filter(|t| !included.contains(&t.id()))
            .collect();
        self.base = state.clone();
        self.rebuild(txs, admin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::{AdminAction, AdminTransaction, Transaction, TxBody, UNITS_PER_VINX};
    use vinx_crypto::KeyPair;
    use vinx_state::Genesis;

    const VINX: u64 = UNITS_PER_VINX;

    /// État où chaque compte de `users` a reçu 1 000 VINX.
    fn funded(users: &[KeyPair]) -> State {
        let admins = [
            KeyPair::generate(),
            KeyPair::generate(),
            KeyPair::generate(),
        ];
        let state = Genesis::standard(
            "vinx-test",
            1_800_000_000,
            [
                admins[0].public_key(),
                admins[1].public_key(),
                admins[2].public_key(),
            ],
            vec![KeyPair::generate().public_key()],
        )
        .unwrap()
        .into_state()
        .unwrap();
        let admin_txs: Vec<_> = users
            .iter()
            .zip(1u64..)
            .map(|(u, nonce)| {
                let tx = AdminTransaction {
                    chain_id: "vinx-test".into(),
                    nonce,
                    action: AdminAction::TreasuryTransfer {
                        to: Address::from_public_key(&u.public_key()),
                        amount: 1_000 * VINX,
                        memo: None,
                    },
                };
                SignedAdminTransaction {
                    approvals: vec![tx.approve(0, &admins[0]), tx.approve(1, &admins[1])],
                    tx,
                }
            })
            .collect();
        state
            .execute(1, 1_800_000_000_000, &admin_txs, &[])
            .unwrap()
    }

    fn pay(from: &KeyPair, nonce: u64, fee: u64) -> SignedTransaction {
        Transaction {
            chain_id: "vinx-test".into(),
            sender: Address::from_public_key(&from.public_key()),
            nonce,
            fee,
            body: TxBody::Transfer {
                to: Address::treasury(),
                amount: VINX,
                memo: None,
            },
        }
        .sign(from, nonce == 1)
    }

    #[test]
    fn per_account_limit() {
        let alice = KeyPair::generate();
        let mut pool = Mempool::new(&funded(std::slice::from_ref(&alice)));
        for n in 1..=MAX_PER_ACCOUNT as u64 {
            pool.add(pay(&alice, n, VINX / 1000)).unwrap();
        }
        let next = pay(&alice, MAX_PER_ACCOUNT as u64 + 1, VINX / 1000);
        assert_eq!(pool.add(next), Err(MempoolError::AccountLimit));
    }

    #[test]
    fn full_pool_admits_only_higher_fees_and_evicts_the_cheapest() {
        let users: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
        let mut pool = Mempool::with_capacity(&funded(&users), 3);
        let min = VINX / 1000;
        pool.add(pay(&users[0], 1, min * 5)).unwrap();
        pool.add(pay(&users[1], 1, min)).unwrap();
        pool.add(pay(&users[2], 1, min * 3)).unwrap();
        // Pas plus cher que la moins chère : refusée.
        assert_eq!(pool.add(pay(&users[3], 1, min)), Err(MempoolError::Full));
        // Plus chère : entre, et la transaction de users[1] (la moins chère) sort.
        pool.add(pay(&users[3], 1, min * 2)).unwrap();
        assert_eq!(pool.len(), 3);
        let senders: Vec<_> = pool.txs().iter().map(|t| t.tx.sender).collect();
        assert!(!senders.contains(&Address::from_public_key(&users[1].public_key())));
    }

    #[test]
    fn only_the_last_transaction_of_an_account_is_evictable() {
        let users: Vec<KeyPair> = (0..2).map(|_| KeyPair::generate()).collect();
        let mut pool = Mempool::with_capacity(&funded(&users), 2);
        let min = VINX / 1000;
        // users[0] : nonce 1 bon marché, nonce 2 cher. Évincer le nonce 1 rendrait le 2 invalide.
        pool.add(pay(&users[0], 1, min)).unwrap();
        pool.add(pay(&users[0], 2, min * 10)).unwrap();
        assert_eq!(
            pool.add(pay(&users[1], 1, min * 2)),
            Err(MempoolError::Full)
        );
        pool.add(pay(&users[1], 1, min * 20)).unwrap();
        let nonces: Vec<_> = pool.txs().iter().map(|t| t.tx.nonce).collect();
        assert_eq!(nonces, [1, 1]);
    }

    #[test]
    fn priority_order_puts_higher_fees_first() {
        let users: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
        let mut pool = Mempool::new(&funded(&users));
        let min = VINX / 1000;
        for (u, fee) in users.iter().zip([min, min * 7, min * 3]) {
            pool.add(pay(u, 1, fee)).unwrap();
        }
        let fees: Vec<_> = pool.by_priority().iter().map(|t| t.tx.fee).collect();
        assert_eq!(fees, [min * 7, min * 3, min]);
    }
}
