//! État global et application des transactions et des blocs (SPEC §4, §5, §6, §9).

use crate::escrow::{Escrow, EscrowError};
use crate::smt;
use crate::validators::{ValidatorChange, ValidatorError, ValidatorSet, validators_hash};
use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;
use thiserror::Error;
use vinx_core::params::MIN_UPGRADE_DELAY;
use vinx_core::{
    ADMIN_SIGNERS, Account, AdminAction, Amount, Block, ParamId, Params, SignedAdminTransaction,
    SignedTransaction, TOTAL_SUPPLY, TxBody, encode,
};
use vinx_crypto::{Address, Domain, Hash, PublicKey, hash_tagged_parts, merkle_root};

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AdminSet {
    pub signers: [PublicKey; ADMIN_SIGNERS],
    /// Nonce de la dernière transaction admin appliquée.
    pub nonce: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PendingUpgrade {
    pub version: u32,
    pub activation_height: u64,
}

/// État complet après le bloc `height`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub chain_id: String,
    pub protocol_version: u32,
    /// Hauteur du dernier bloc appliqué (0 à la genèse).
    pub height: u64,
    pub accounts: BTreeMap<Address, Account>,
    pub escrow: Escrow,
    pub admin: AdminSet,
    pub validators: ValidatorSet,
    pub params: Params,
    pub upgrade: Option<PendingUpgrade>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TxError {
    #[error("mauvais identifiant de chaîne")]
    WrongChain,
    #[error("compte émetteur inconnu")]
    UnknownSender,
    #[error("clé publique absente, en trop ou sans rapport avec l'adresse")]
    KeyMismatch,
    #[error("signature invalide")]
    BadSignature,
    #[error("approbations admin insuffisantes ou invalides")]
    Unauthorized,
    #[error("nonce incorrect")]
    BadNonce,
    #[error("frais inférieurs au minimum")]
    FeeTooLow,
    #[error("montant nul")]
    ZeroAmount,
    #[error("mémo vide ou trop long")]
    InvalidMemo,
    #[error("le destinataire exige un mémo")]
    MemoRequired,
    #[error("transfert vers soi-même")]
    SelfTransfer,
    #[error("solde insuffisant")]
    InsufficientBalance,
    #[error("solde sous la réserve minimale")]
    BelowReserve,
    #[error("escrow : {0:?}")]
    Escrow(EscrowError),
    #[error("validateurs : {0:?}")]
    Validator(ValidatorError),
    #[error("valeur de paramètre invalide")]
    InvalidParam,
    #[error("remplacement de signataire invalide")]
    InvalidSigner,
    #[error("mise à jour du protocole invalide")]
    InvalidUpgrade,
    #[error("dépassement arithmétique")]
    Overflow,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BlockError {
    #[error("hauteur inattendue")]
    WrongHeight,
    #[error("mauvais identifiant de chaîne")]
    WrongChain,
    #[error("version de protocole incorrecte")]
    WrongVersion,
    #[error("trop de transactions")]
    TooManyTxs,
    #[error("bloc trop volumineux")]
    TooLarge,
    #[error("tx_root incorrect")]
    TxRootMismatch,
    #[error("state_root incorrect")]
    StateRootMismatch,
    #[error("validators_hash incorrect")]
    ValidatorsHashMismatch,
    #[error("transaction admin {index} invalide : {error}")]
    InvalidAdminTx { index: usize, error: TxError },
    #[error("transaction {index} invalide : {error}")]
    InvalidTx { index: usize, error: TxError },
    #[error("invariant de supply violé")]
    InvariantViolated,
}

/// Contexte du bloc en cours d'application.
#[derive(Clone, Copy, Debug)]
struct Ctx {
    height: u64,
    /// Horodatage du bloc, en secondes.
    now: u64,
}

/// Comptes touchés par une transaction, modifiés à part puis écrits d'un coup :
/// une transaction refusée ne laisse aucune trace.
#[derive(Default)]
struct Changes {
    touched: BTreeMap<Address, Account>,
}

impl Changes {
    fn get(&mut self, base: &BTreeMap<Address, Account>, address: Address) -> &mut Account {
        self.touched
            .entry(address)
            .or_insert_with(|| base.get(&address).cloned().unwrap_or_default())
    }

    fn debit(
        &mut self,
        base: &BTreeMap<Address, Account>,
        address: Address,
        amount: Amount,
    ) -> Result<(), TxError> {
        let account = self.get(base, address);
        account.balance = account
            .balance
            .checked_sub(amount)
            .ok_or(TxError::InsufficientBalance)?;
        Ok(())
    }

    fn credit(
        &mut self,
        base: &BTreeMap<Address, Account>,
        address: Address,
        amount: Amount,
    ) -> Result<(), TxError> {
        let account = self.get(base, address);
        account.balance = account
            .balance
            .checked_add(amount)
            .ok_or(TxError::Overflow)?;
        Ok(())
    }

    fn commit(self, accounts: &mut BTreeMap<Address, Account>) {
        accounts.extend(self.touched);
    }
}

impl State {
    /// `Σ soldes + Σ escrow = 10¹⁸ unités`.
    pub fn check_invariant(&self) -> Result<(), BlockError> {
        let balances: u128 = self.accounts.values().map(|a| u128::from(a.balance)).sum();
        if balances.checked_add(self.escrow.total()) == Some(u128::from(TOTAL_SUPPLY)) {
            Ok(())
        } else {
            Err(BlockError::InvariantViolated)
        }
    }

    /// `H("vinx/state" ‖ racine_SMT(comptes) ‖ H(borsh(reste de l'état)))`.
    pub fn state_root(&self) -> Hash {
        let globals = encode(&(
            &self.escrow,
            &self.admin,
            &self.validators,
            &self.params,
            &self.upgrade,
            self.protocol_version,
        ));
        hash_tagged_parts(
            Domain::State,
            &[
                &smt::root(&self.accounts),
                blake3::hash(&globals).as_bytes(),
            ],
        )
    }

    /// Validateurs de la hauteur `height`.
    pub fn validators_at(&self, height: u64) -> Vec<PublicKey> {
        self.validators.at_height(height)
    }

    /// `validators_hash` attendu dans l'en-tête du bloc `self.height`.
    pub fn next_validators_hash(&self) -> Hash {
        validators_hash(&self.validators_at(self.height.saturating_add(1)))
    }

    pub fn balance(&self, address: &Address) -> Amount {
        self.accounts.get(address).map_or(0, |a| a.balance)
    }

    // ── Transactions utilisateur ────────────────────────────────────────────

    /// Règles communes aux transferts utilisateur et aux transferts de trésorerie.
    fn check_payment(
        &self,
        from: Address,
        to: Address,
        amount: Amount,
        memo: &Option<Vec<u8>>,
    ) -> Result<(), TxError> {
        if amount == 0 {
            return Err(TxError::ZeroAmount);
        }
        if to == from {
            return Err(TxError::SelfTransfer);
        }
        if let Some(m) = memo {
            let too_long = u64::try_from(m.len()).map_or(true, |l| l > self.params.max_memo_bytes);
            if m.is_empty() || too_long {
                return Err(TxError::InvalidMemo);
            }
        }
        match self.accounts.get(&to) {
            Some(recipient) if recipient.memo_required && memo.is_none() => {
                Err(TxError::MemoRequired)
            }
            Some(_) => Ok(()),
            None if amount < self.params.min_reserve => Err(TxError::BelowReserve),
            None => Ok(()),
        }
    }

    /// Applique une transaction utilisateur. En cas d'erreur, l'état est inchangé.
    pub fn apply_tx(&mut self, stx: &SignedTransaction) -> Result<(), TxError> {
        let tx = &stx.tx;
        if tx.chain_id != self.chain_id {
            return Err(TxError::WrongChain);
        }
        let sender = self
            .accounts
            .get(&tx.sender)
            .ok_or(TxError::UnknownSender)?;
        let key = match (&sender.key, &stx.public_key) {
            (Some(known), None) => known.clone(),
            (None, Some(given)) if Address::from_public_key(given) == tx.sender => given.clone(),
            _ => return Err(TxError::KeyMismatch),
        };
        stx.verify_signature(&key)
            .map_err(|_| TxError::BadSignature)?;
        if sender.nonce.checked_add(1) != Some(tx.nonce) {
            return Err(TxError::BadNonce);
        }
        if tx.fee < self.params.min_fee {
            return Err(TxError::FeeTooLow);
        }

        let mut changes = Changes::default();
        changes.debit(&self.accounts, tx.sender, tx.fee)?;
        changes.credit(&self.accounts, Address::treasury(), tx.fee)?;
        let account = changes.get(&self.accounts, tx.sender);
        account.nonce = tx.nonce;
        account.key = Some(key);
        match &tx.body {
            TxBody::Transfer { to, amount, memo } => {
                self.check_payment(tx.sender, *to, *amount, memo)?;
                changes.debit(&self.accounts, tx.sender, *amount)?;
                changes.credit(&self.accounts, *to, *amount)?;
            }
            TxBody::SetMemoRequired { required } => {
                changes.get(&self.accounts, tx.sender).memo_required = *required;
            }
            TxBody::RotateKey { new_key } => {
                changes.get(&self.accounts, tx.sender).key = Some(new_key.clone());
            }
        }
        if changes.get(&self.accounts, tx.sender).balance < self.params.min_reserve {
            return Err(TxError::BelowReserve);
        }
        changes.commit(&mut self.accounts);
        Ok(())
    }

    // ── Transactions d'administration ───────────────────────────────────────

    fn apply_admin(&mut self, satx: &SignedAdminTransaction, ctx: Ctx) -> Result<(), TxError> {
        let tx = &satx.tx;
        if tx.chain_id != self.chain_id {
            return Err(TxError::WrongChain);
        }
        if self.admin.nonce.checked_add(1) != Some(tx.nonce) {
            return Err(TxError::BadNonce);
        }
        satx.verify_approvals(&self.admin.signers)
            .map_err(|_| TxError::Unauthorized)?;

        let treasury = Address::treasury();
        match &tx.action {
            AdminAction::EscrowRelease { amount } => {
                self.escrow
                    .check_release(*amount, ctx.now)
                    .map_err(TxError::Escrow)?;
                let mut changes = Changes::default();
                changes.credit(&self.accounts, treasury, *amount)?;
                self.escrow
                    .release(*amount, ctx.now)
                    .map_err(TxError::Escrow)?;
                changes.commit(&mut self.accounts);
            }
            AdminAction::EscrowReturn { amount } => {
                if *amount == 0 {
                    return Err(TxError::ZeroAmount);
                }
                let mut changes = Changes::default();
                changes.debit(&self.accounts, treasury, *amount)?;
                self.escrow
                    .push_back(*amount, ctx.now)
                    .map_err(TxError::Escrow)?;
                changes.commit(&mut self.accounts);
            }
            AdminAction::TreasuryTransfer { to, amount, memo } => {
                self.check_payment(treasury, *to, *amount, memo)?;
                let mut changes = Changes::default();
                changes.debit(&self.accounts, treasury, *amount)?;
                changes.credit(&self.accounts, *to, *amount)?;
                changes.commit(&mut self.accounts);
            }
            AdminAction::AddValidator { key } => {
                self.schedule_validator(ValidatorChange::Add(key.clone()), ctx)?;
            }
            AdminAction::RemoveValidator { key } => {
                self.schedule_validator(ValidatorChange::Remove(key.clone()), ctx)?;
            }
            AdminAction::SetParam { param, value } => {
                let may_be_zero = matches!(param, ParamId::MinFee | ParamId::MinReserve);
                if *value == 0 && !may_be_zero {
                    return Err(TxError::InvalidParam);
                }
                self.params.set(*param, *value);
            }
            AdminAction::ReplaceSigner { index, new_key } => {
                if self.admin.signers.contains(new_key) {
                    return Err(TxError::InvalidSigner);
                }
                let slot = self
                    .admin
                    .signers
                    .get_mut(usize::from(*index))
                    .ok_or(TxError::InvalidSigner)?;
                *slot = new_key.clone();
            }
            AdminAction::ScheduleUpgrade {
                version,
                activation_height,
            } => {
                let next_version = self.protocol_version.checked_add(1);
                let earliest = ctx.height.checked_add(MIN_UPGRADE_DELAY);
                let too_early = earliest.is_none_or(|e| *activation_height < e);
                if next_version != Some(*version) || too_early {
                    return Err(TxError::InvalidUpgrade);
                }
                self.upgrade = Some(PendingUpgrade {
                    version: *version,
                    activation_height: *activation_height,
                });
            }
        }
        self.admin.nonce = tx.nonce;
        Ok(())
    }

    fn schedule_validator(&mut self, change: ValidatorChange, ctx: Ctx) -> Result<(), TxError> {
        self.validators.check(&change).map_err(TxError::Validator)?;
        let effective = ctx.height.checked_add(2).ok_or(TxError::Overflow)?;
        self.validators.schedule(change, effective);
        Ok(())
    }

    // ── Blocs ───────────────────────────────────────────────────────────────

    /// Changements qui prennent effet au début du bloc `height`.
    fn begin_block(&mut self, height: u64) {
        self.validators.advance_to(height);
        if let Some(up) = &self.upgrade {
            if up.activation_height <= height {
                self.protocol_version = up.version;
                self.upgrade = None;
            }
        }
    }

    /// Exécute un bloc sur une copie de l'état et renvoie le nouvel état.
    ///
    /// Sert au proposeur pour calculer `state_root` et `validators_hash`, et à
    /// [`Self::apply_block`] pour vérifier un bloc reçu.
    pub fn execute(
        &self,
        height: u64,
        timestamp_ms: u64,
        admin_txs: &[SignedAdminTransaction],
        txs: &[SignedTransaction],
    ) -> Result<State, BlockError> {
        if self.height.checked_add(1) != Some(height) {
            return Err(BlockError::WrongHeight);
        }
        let count = admin_txs.len().saturating_add(txs.len());
        if u64::try_from(count).map_or(true, |c| c > self.params.max_block_txs) {
            return Err(BlockError::TooManyTxs);
        }
        let ctx = Ctx {
            height,
            now: timestamp_ms / 1000,
        };
        let mut next = self.clone();
        next.begin_block(height);
        for (index, satx) in admin_txs.iter().enumerate() {
            next.apply_admin(satx, ctx)
                .map_err(|error| BlockError::InvalidAdminTx { index, error })?;
        }
        for (index, stx) in txs.iter().enumerate() {
            next.apply_tx(stx)
                .map_err(|error| BlockError::InvalidTx { index, error })?;
        }
        next.height = height;
        next.check_invariant()?;
        Ok(next)
    }

    /// Vérifie et applique un bloc reçu. En cas d'erreur, l'état est inchangé.
    ///
    /// Ne vérifie ni l'horodatage par rapport au parent, ni le proposeur, ni le
    /// certificat de commit : c'est le rôle du consensus.
    pub fn apply_block(&mut self, block: &Block) -> Result<(), BlockError> {
        let h = &block.header;
        if h.chain_id != self.chain_id {
            return Err(BlockError::WrongChain);
        }
        if u64::try_from(block.encoded_len()).map_or(true, |l| l > self.params.max_block_bytes) {
            return Err(BlockError::TooLarge);
        }
        if block.compute_tx_root() != h.tx_root {
            return Err(BlockError::TxRootMismatch);
        }
        let next = self.execute(h.height, h.timestamp, &block.admin_txs, &block.txs)?;
        if h.protocol_version != next.protocol_version {
            return Err(BlockError::WrongVersion);
        }
        if h.state_root != next.state_root() {
            return Err(BlockError::StateRootMismatch);
        }
        if h.validators_hash != next.next_validators_hash() {
            return Err(BlockError::ValidatorsHashMismatch);
        }
        *self = next;
        Ok(())
    }
}

/// Racine des transactions d'un bloc, pour le proposeur.
pub fn tx_root(admin_txs: &[SignedAdminTransaction], txs: &[SignedTransaction]) -> Hash {
    let ids: Vec<Hash> = admin_txs
        .iter()
        .map(SignedAdminTransaction::id)
        .chain(txs.iter().map(SignedTransaction::id))
        .collect();
    merkle_root(&ids)
}
