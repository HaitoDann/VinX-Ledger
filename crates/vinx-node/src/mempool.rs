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

/// Capacité par défaut, pour borner la mémoire (`--mempool-size`).
pub const MAX_PENDING: usize = 20_000;

/// Capacité minimale : un bloc plein par défaut (5 000) doit pouvoir s'y former.
pub const MIN_CAPACITY: usize = 5_000;

/// Transactions en attente au plus par compte.
pub const MAX_PER_ACCOUNT: usize = 100;

#[derive(Debug, PartialEq, Eq)]
pub enum MempoolError {
    Duplicate,
    Full,
    AccountLimit,
    Invalid(TxError),
    InvalidAdmin(String),
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
            MempoolError::InvalidAdmin(reason) => write!(f, "transaction admin refusée : {reason}"),
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

    /// Frais conseillés pour être inclus dans le prochain bloc.
    ///
    /// Tant que les transactions en attente tiennent dans un bloc, le minimum
    /// suffit ; sinon, il faut payer plus que la dernière transaction qui y
    /// entrerait. Mempool plein, il faut aussi dépasser la moins chère pour y entrer.
    pub fn recommended_fee(&self, min_fee: u64, max_block_txs: u64) -> u64 {
        let block = usize::try_from(max_block_txs).unwrap_or(usize::MAX);
        let mut fee = min_fee;
        if self.txs.len() >= block {
            let ranked = self.by_priority();
            if let Some(cutoff) = block.checked_sub(1).and_then(|i| ranked.get(i)) {
                fee = fee.max(cutoff.tx.fee.saturating_add(1));
            }
        }
        if self.len() >= self.capacity {
            if let Some(cheapest) = self.txs.iter().map(|t| t.tx.fee).min() {
                fee = fee.max(cheapest.saturating_add(1));
            }
        }
        fee
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

    /// Change la capacité (au moins [`MIN_CAPACITY`]). Les transactions déjà
    /// en attente sont gardées, même au-delà.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(MIN_CAPACITY);
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Retire les transactions évinçables les moins chères qui paient moins que
    /// `fee` : 1 % de la capacité d'un coup, car chaque éviction réapplique tout
    /// le mempool ; une à la fois, un afflux de transactions mieux payées
    /// le reconstruirait à chaque arrivée.
    ///
    /// Seule la dernière transaction en attente d'un compte est évinçable :
    /// retirer une transaction intermédiaire rendrait les suivantes invalides.
    fn evict_cheaper_than(&mut self, fee: u64) -> Result<(), MempoolError> {
        let mut last_of_sender: BTreeMap<Address, usize> = BTreeMap::new();
        for (i, t) in self.txs.iter().enumerate() {
            last_of_sender.insert(t.tx.sender, i);
        }
        let mut victims: Vec<(usize, u64)> = last_of_sender
            .values()
            .copied()
            .filter_map(|i| Some((i, self.txs.get(i)?.tx.fee)))
            .filter(|(_, f)| *f < fee)
            .collect();
        if victims.is_empty() {
            return Err(MempoolError::Full);
        }
        victims.sort_by_key(|(_, f)| *f);
        victims.truncate((self.capacity / 100).max(1));
        let evicted: BTreeSet<usize> = victims.into_iter().map(|(i, _)| i).collect();
        let txs = std::mem::take(&mut self.txs)
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !evicted.contains(i))
            .map(|(_, t)| t)
            .collect();
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
        if tx.tx.chain_id != self.pending.chain_id {
            return Err(MempoolError::InvalidAdmin(
                "mauvais identifiant de chaîne".into(),
            ));
        }
        if tx.verify_approvals(&self.pending.admin.signers).is_err() {
            return Err(MempoolError::InvalidAdmin(
                "approbations insuffisantes ou invalides".into(),
            ));
        }
        // L'action entière est vérifiée tout de suite, à la suite des transactions
        // admin déjà en attente : une libération d'escrow non échue ou un nonce
        // faux est refusé dès la soumission, avec la raison.
        let mut queue = self.admin.clone();
        queue.push(tx.clone());
        let height = self.base.height.saturating_add(1);
        if let Err(e) = self
            .base
            .execute(height, crate::node::now_ms(), &queue, &[])
        {
            let reason = match e {
                vinx_state::BlockError::InvalidAdminTx { error, .. } => error.to_string(),
                other => other.to_string(),
            };
            return Err(MempoolError::InvalidAdmin(reason));
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
    fn admin_actions_are_checked_on_submission() {
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
        let mut pool = Mempool::new(&state);
        let signed = |nonce, action| {
            let tx = AdminTransaction {
                chain_id: "vinx-test".into(),
                nonce,
                action,
            };
            SignedAdminTransaction {
                approvals: vec![tx.approve(0, &admins[0]), tx.approve(1, &admins[1])],
                tx,
            }
        };
        // Tranche pas encore échue : refusée tout de suite, avec la raison.
        let early = pool.add_admin(signed(1, AdminAction::EscrowRelease { amount: 1 }));
        assert!(
            matches!(&early, Err(MempoolError::InvalidAdmin(r)) if r.contains("escrow")),
            "{early:?}"
        );
        // Deux actions valides à la suite : la seconde est vérifiée après la première.
        let pay = |nonce| {
            signed(
                nonce,
                AdminAction::TreasuryTransfer {
                    to: Address::from_bytes([3; 20]),
                    amount: VINX,
                    memo: None,
                },
            )
        };
        pool.add_admin(pay(1)).unwrap();
        pool.add_admin(pay(2)).unwrap();
        // Nonce sauté : refusé.
        assert!(pool.add_admin(pay(4)).is_err());
        assert_eq!(pool.admin_txs().len(), 2);
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
    fn a_full_pool_evicts_one_percent_at_once() {
        let users: Vec<KeyPair> = (0..201).map(|_| KeyPair::generate()).collect();
        let mut pool = Mempool::with_capacity(&funded(&users), 200);
        let min = VINX / 1000;
        for (i, u) in users.iter().take(200).enumerate() {
            pool.add(pay(u, 1, min + i as u64)).unwrap();
        }
        // Une seule reconstruction pour deux places : les deux moins chères sortent.
        pool.add(pay(&users[200], 1, min * 10)).unwrap();
        assert_eq!(pool.len(), 199);
        let fees: Vec<_> = pool.txs().iter().map(|t| t.tx.fee).collect();
        assert!(!fees.contains(&min) && !fees.contains(&(min + 1)));
    }

    #[test]
    fn capacity_has_a_floor() {
        let mut pool = Mempool::new(&funded(&[]));
        pool.set_capacity(10);
        assert_eq!(pool.capacity(), MIN_CAPACITY);
        pool.set_capacity(100_000);
        assert_eq!(pool.capacity(), 100_000);
    }

    #[test]
    fn recommended_fee_follows_congestion() {
        let users: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
        let mut pool = Mempool::with_capacity(&funded(&users), 4);
        let min = VINX / 1000;
        assert_eq!(pool.recommended_fee(min, 2), min);
        pool.add(pay(&users[0], 1, min * 5)).unwrap();
        pool.add(pay(&users[1], 1, min * 3)).unwrap();
        // Deux transactions, blocs de deux : il faut battre la 2e (3 × min).
        assert_eq!(pool.recommended_fee(min, 2), min * 3 + 1);
        assert_eq!(pool.recommended_fee(min, 10), min);
        pool.add(pay(&users[2], 1, min * 2)).unwrap();
        pool.add(pay(&users[3], 1, min * 4)).unwrap();
        // Plein (4/4) avec des blocs de 10 : il faut battre la moins chère.
        assert_eq!(pool.recommended_fee(min, 10), min * 2 + 1);
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
