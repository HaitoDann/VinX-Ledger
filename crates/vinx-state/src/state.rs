//! État global et application des transactions et des blocs (SPEC §4, §5, §6, §9).

use crate::accounts::Accounts;
use crate::emission::Emission;
use crate::escrow::{Escrow, EscrowError};
use crate::policy::{Policies, Policy, PolicyError};
use crate::registry::{
    Deal, MAX_DEAL_DURATION, NAME_GRACE, NAME_PERIOD, NameRecord, Registry, valid_name,
};
use crate::validators::{ValidatorChange, ValidatorError, ValidatorSet, validators_hash};
use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use thiserror::Error;
use vinx_core::params::MIN_UPGRADE_DELAY;
use vinx_core::{
    ADMIN_SIGNERS, Account, AdminAction, Amount, Block, ParamId, Params, SignedAdminTransaction,
    SignedTransaction, TOTAL_SUPPLY, TxBody, deal_id, encode,
};
use vinx_crypto::{Address, Domain, Hash, PublicKey, hash_tagged_parts, merkle_root};

/// Dernière version du protocole que ce code sait exécuter. Un nœud plus
/// ancien qu'une mise à jour activée s'arrête au lieu de diverger.
pub const SUPPORTED_PROTOCOL: u32 = 4;

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
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct State {
    pub chain_id: String,
    pub protocol_version: u32,
    /// Hauteur du dernier bloc appliqué (0 à la genèse).
    pub height: u64,
    pub accounts: Accounts,
    pub escrow: Escrow,
    pub admin: AdminSet,
    pub validators: ValidatorSet,
    pub params: Params,
    pub upgrade: Option<PendingUpgrade>,
    /// Noms et séquestres (protocole 2). Vide et hors de `state_root` avant.
    pub registry: Registry,
    /// Émission vers les validateurs et cagnotte des frais (protocole 3).
    pub emission: Emission,
    /// Règles des comptes : multisig, plafonds (protocole 4).
    pub policies: Policies,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TxError {
    #[error(
        "type de transaction pas encore actif sur ce réseau (mise à jour du protocole requise)"
    )]
    NotYetActive,
    #[error("nom invalide : 3 à 32 caractères a-z, 0-9 et tiret")]
    InvalidName,
    #[error("nom déjà pris")]
    NameTaken,
    #[error("ce nom n'appartient pas à l'émetteur, ou a expiré")]
    NotNameOwner,
    #[error("{0}")]
    Policy(PolicyError),
    #[error("paiement groupé vide ou de plus de 100 paiements")]
    InvalidBatch,
    #[error("séquestre inconnu")]
    UnknownDeal,
    #[error("séquestre invalide (participants ou échéance)")]
    InvalidDeal,
    #[error("action non autorisée sur ce séquestre")]
    DealForbidden,
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
    #[error("escrow : {0}")]
    Escrow(EscrowError),
    #[error("validateurs : {0}")]
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
    #[error("proposeur hors de l'ensemble de validateurs")]
    UnknownProposer,
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
    fn get(&mut self, base: &Accounts, address: Address) -> &mut Account {
        self.touched
            .entry(address)
            .or_insert_with(|| base.get(&address).cloned().unwrap_or_default())
    }

    fn debit(&mut self, base: &Accounts, address: Address, amount: Amount) -> Result<(), TxError> {
        let account = self.get(base, address);
        account.balance = account
            .balance
            .checked_sub(amount)
            .ok_or(TxError::InsufficientBalance)?;
        Ok(())
    }

    fn credit(&mut self, base: &Accounts, address: Address, amount: Amount) -> Result<(), TxError> {
        let account = self.get(base, address);
        account.balance = account
            .balance
            .checked_add(amount)
            .ok_or(TxError::Overflow)?;
        Ok(())
    }

    fn commit(self, accounts: &mut Accounts) {
        for (address, account) in self.touched {
            accounts.insert(address, account);
        }
    }
}

/// Tout l'état sauf les comptes, pour l'écrire sur disque à part.
#[derive(BorshSerialize, BorshDeserialize)]
struct Globals {
    chain_id: String,
    protocol_version: u32,
    height: u64,
    escrow: Escrow,
    admin: AdminSet,
    validators: ValidatorSet,
    params: Params,
    upgrade: Option<PendingUpgrade>,
    registry: Registry,
    emission: Emission,
    policies: Policies,
}

/// [`Globals`] des versions 0.10 et 0.11, sans politiques de compte.
#[derive(BorshDeserialize)]
struct GlobalsV3 {
    chain_id: String,
    protocol_version: u32,
    height: u64,
    escrow: Escrow,
    admin: AdminSet,
    validators: ValidatorSet,
    params: Params,
    upgrade: Option<PendingUpgrade>,
    registry: Registry,
    emission: Emission,
}

impl From<GlobalsV3> for Globals {
    fn from(g: GlobalsV3) -> Self {
        Self {
            chain_id: g.chain_id,
            protocol_version: g.protocol_version,
            height: g.height,
            escrow: g.escrow,
            admin: g.admin,
            validators: g.validators,
            params: g.params,
            upgrade: g.upgrade,
            registry: g.registry,
            emission: g.emission,
            policies: Policies::default(),
        }
    }
}

/// [`Globals`] des versions 0.8 et 0.9, sans émission.
#[derive(BorshDeserialize)]
struct GlobalsV2 {
    chain_id: String,
    protocol_version: u32,
    height: u64,
    escrow: Escrow,
    admin: AdminSet,
    validators: ValidatorSet,
    params: Params,
    upgrade: Option<PendingUpgrade>,
    registry: Registry,
}

impl From<GlobalsV2> for Globals {
    fn from(g: GlobalsV2) -> Self {
        Self {
            chain_id: g.chain_id,
            protocol_version: g.protocol_version,
            height: g.height,
            escrow: g.escrow,
            admin: g.admin,
            validators: g.validators,
            params: g.params,
            upgrade: g.upgrade,
            registry: g.registry,
            emission: Emission::default(),
            policies: Policies::default(),
        }
    }
}

/// [`Globals`] des versions 0.7 et antérieures, sans registre.
#[derive(BorshDeserialize)]
struct LegacyGlobals {
    chain_id: String,
    protocol_version: u32,
    height: u64,
    escrow: Escrow,
    admin: AdminSet,
    validators: ValidatorSet,
    params: Params,
    upgrade: Option<PendingUpgrade>,
}

impl From<LegacyGlobals> for Globals {
    fn from(g: LegacyGlobals) -> Self {
        Self {
            chain_id: g.chain_id,
            protocol_version: g.protocol_version,
            height: g.height,
            escrow: g.escrow,
            admin: g.admin,
            validators: g.validators,
            params: g.params,
            upgrade: g.upgrade,
            registry: Registry::default(),
            emission: Emission::default(),
            policies: Policies::default(),
        }
    }
}

impl State {
    /// État sans les comptes, encodé : le stockage écrit les comptes séparément.
    pub fn globals_bytes(&self) -> Vec<u8> {
        encode(&Globals {
            chain_id: self.chain_id.clone(),
            protocol_version: self.protocol_version,
            height: self.height,
            escrow: self.escrow.clone(),
            admin: self.admin.clone(),
            validators: self.validators.clone(),
            params: self.params.clone(),
            upgrade: self.upgrade.clone(),
            registry: self.registry.clone(),
            emission: self.emission.clone(),
            policies: self.policies.clone(),
        })
    }

    /// Reconstitue un état à partir de [`Self::globals_bytes`] et des comptes.
    pub fn from_parts(globals: &[u8], accounts: Accounts) -> Option<Self> {
        let g: Globals = vinx_core::decode(globals)
            .or_else(|_| vinx_core::decode::<GlobalsV3>(globals).map(Globals::from))
            .or_else(|_| vinx_core::decode::<GlobalsV2>(globals).map(Globals::from))
            .or_else(|_| vinx_core::decode::<LegacyGlobals>(globals).map(Globals::from))
            .ok()?;
        let mut accounts = accounts;
        accounts.take_dirty();
        Some(Self {
            chain_id: g.chain_id,
            protocol_version: g.protocol_version,
            height: g.height,
            accounts,
            escrow: g.escrow,
            admin: g.admin,
            validators: g.validators,
            params: g.params,
            upgrade: g.upgrade,
            registry: g.registry,
            emission: g.emission,
            policies: g.policies,
        })
    }

    /// `Σ soldes + Σ escrow + Σ séquestres + cagnotte + émission restante = 10¹⁸ unités`.
    pub fn check_invariant(&self) -> Result<(), BlockError> {
        let total = self
            .accounts
            .total_balance()
            .checked_add(self.escrow.total())
            .and_then(|t| t.checked_add(self.registry.locked()))
            .and_then(|t| t.checked_add(u128::from(self.emission.pot)))
            .and_then(|t| t.checked_add(u128::from(self.emission.remaining())));
        if total == Some(u128::from(TOTAL_SUPPLY)) {
            Ok(())
        } else {
            Err(BlockError::InvariantViolated)
        }
    }

    /// `H("vinx/state" ‖ racine_SMT(comptes) ‖ H(borsh(reste de l'état)))`.
    /// Le registre en fait partie à partir du protocole 2.
    pub fn state_root(&self) -> Hash {
        let mut globals = encode(&(
            &self.escrow,
            &self.admin,
            &self.validators,
            &self.params,
            &self.upgrade,
            self.protocol_version,
        ));
        if self.protocol_version >= 2 {
            globals.extend_from_slice(&encode(&self.registry));
        }
        if self.protocol_version >= 3 {
            globals.extend_from_slice(&encode(&self.emission));
        }
        if self.protocol_version >= 4 {
            globals.extend_from_slice(&encode(&self.policies));
        }
        hash_tagged_parts(
            Domain::State,
            &[&self.accounts.root(), blake3::hash(&globals).as_bytes()],
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

    pub fn account(&self, address: &Address) -> Option<&Account> {
        self.accounts.get(address)
    }

    /// Écrit un compte directement, sans règle : réservé aux tests et bancs d'essai.
    #[doc(hidden)]
    pub fn set_account_for_tests(&mut self, address: Address, account: Account) {
        self.accounts.insert(address, account);
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
        crate::sigcache::verify(stx, &key).map_err(|_| TxError::BadSignature)?;
        if sender.nonce.checked_add(1) != Some(tx.nonce) {
            return Err(TxError::BadNonce);
        }
        if tx.fee < tx.body.min_fee(self.params.min_fee) {
            return Err(TxError::FeeTooLow);
        }
        if tx.body.min_protocol() > self.protocol_version {
            return Err(TxError::NotYetActive);
        }
        let body = tx.body.inner();
        if body.min_protocol() > self.protocol_version {
            return Err(TxError::NotYetActive);
        }
        let policy_write = self.check_policy(tx, body)?;

        let mut changes = Changes::default();
        let mut registry = None;
        changes.debit(&self.accounts, tx.sender, tx.fee)?;
        // Protocole 3 : les frais vont aux validateurs, via la cagnotte d'époque.
        let fee_to_pot = self.protocol_version >= 3;
        if !fee_to_pot {
            changes.credit(&self.accounts, Address::treasury(), tx.fee)?;
        }
        let account = changes.get(&self.accounts, tx.sender);
        account.nonce = tx.nonce;
        account.key = Some(key);
        let mut set_policy = None;
        match body {
            TxBody::Transfer { to, amount, memo } => {
                self.check_payment(tx.sender, *to, *amount, memo)?;
                changes.debit(&self.accounts, tx.sender, *amount)?;
                changes.credit(&self.accounts, *to, *amount)?;
            }
            TxBody::BatchTransfer { payments } => {
                if payments.is_empty() || payments.len() > vinx_core::MAX_BATCH_PAYMENTS {
                    return Err(TxError::InvalidBatch);
                }
                for p in payments {
                    self.check_payment(tx.sender, p.to, p.amount, &p.memo)?;
                    changes.debit(&self.accounts, tx.sender, p.amount)?;
                    changes.credit(&self.accounts, p.to, p.amount)?;
                }
            }
            // L'empreinte n'est que dans le bloc : rien à changer dans l'état.
            TxBody::Attest { .. } => {}
            TxBody::SetPolicy { policy } => {
                let keep = crate::policy::validate(policy).map_err(TxError::Policy)?;
                set_policy = Some(keep.map(|()| Policy {
                    config: policy.clone(),
                    day: 0,
                    spent: 0,
                }));
            }
            // Un seul niveau d'enveloppe.
            TxBody::Approved { .. } => {
                return Err(TxError::Policy(PolicyError::InvalidApprovals));
            }
            TxBody::SetMemoRequired { required } => {
                changes.get(&self.accounts, tx.sender).memo_required = *required;
            }
            TxBody::RotateKey { new_key } => {
                changes.get(&self.accounts, tx.sender).key = Some(new_key.clone());
            }
            TxBody::RegisterName { name } => {
                let record = self.register_name(tx.sender, name)?;
                let fee = self.registry.name_fee;
                if fee > 0 {
                    changes.debit(&self.accounts, tx.sender, fee)?;
                    changes.credit(&self.accounts, Address::treasury(), fee)?;
                }
                registry = Some(Write::Name(name.clone(), record));
            }
            TxBody::TransferName { name, to } => {
                let now = self.registry.now;
                if self.registry.resolve(name, now) != Some(tx.sender) {
                    return Err(TxError::NotNameOwner);
                }
                if *to == tx.sender {
                    return Err(TxError::SelfTransfer);
                }
                let mut record = self
                    .registry
                    .names
                    .get(name)
                    .cloned()
                    .ok_or(TxError::NotNameOwner)?;
                record.owner = *to;
                registry = Some(Write::Name(name.clone(), record));
            }
            TxBody::OpenDeal {
                seller,
                arbiter,
                amount,
                deadline,
                memo,
            } => {
                let now = self.registry.now;
                let latest = now
                    .checked_add(MAX_DEAL_DURATION)
                    .ok_or(TxError::Overflow)?;
                let bad_arbiter = arbiter.is_some_and(|a| a == tx.sender || a == *seller);
                if bad_arbiter || *deadline <= now || *deadline > latest {
                    return Err(TxError::InvalidDeal);
                }
                self.check_payment(tx.sender, *seller, *amount, memo)?;
                changes.debit(&self.accounts, tx.sender, *amount)?;
                let deal = Deal {
                    buyer: tx.sender,
                    seller: *seller,
                    arbiter: *arbiter,
                    amount: *amount,
                    deadline: *deadline,
                    memo: memo.clone(),
                };
                registry = Some(Write::OpenDeal(deal_id(&tx.sender, tx.nonce), deal));
            }
            TxBody::ReleaseDeal { id } => {
                let deal = self.registry.deals.get(id).ok_or(TxError::UnknownDeal)?;
                if tx.sender != deal.buyer && deal.arbiter != Some(tx.sender) {
                    return Err(TxError::DealForbidden);
                }
                changes.credit(&self.accounts, deal.seller, deal.amount)?;
                registry = Some(Write::CloseDeal(*id));
            }
            TxBody::RefundDeal { id } => {
                let deal = self.registry.deals.get(id).ok_or(TxError::UnknownDeal)?;
                let expired = self.registry.now >= deal.deadline;
                let allowed = tx.sender == deal.seller
                    || deal.arbiter == Some(tx.sender)
                    || (tx.sender == deal.buyer && expired);
                if !allowed {
                    return Err(TxError::DealForbidden);
                }
                changes.credit(&self.accounts, deal.buyer, deal.amount)?;
                registry = Some(Write::CloseDeal(*id));
            }
        }
        // Un compte peut être vidé entièrement. Il est conservé avec son nonce :
        // le supprimer permettrait de rejouer ses anciennes transactions s'il
        // était recréé.
        let remaining = changes.get(&self.accounts, tx.sender).balance;
        if remaining != 0 && remaining < self.params.min_reserve {
            return Err(TxError::BelowReserve);
        }
        let pot = if fee_to_pot {
            self.emission
                .pot
                .checked_add(tx.fee)
                .ok_or(TxError::Overflow)?
        } else {
            self.emission.pot
        };
        changes.commit(&mut self.accounts);
        self.emission.pot = pot;
        if let Some(p) = policy_write {
            Arc::make_mut(&mut self.policies).insert(tx.sender, p);
        }
        match set_policy {
            Some(Some(p)) => {
                Arc::make_mut(&mut self.policies).insert(tx.sender, p);
            }
            Some(None) => {
                Arc::make_mut(&mut self.policies).remove(&tx.sender);
            }
            None => {}
        }
        match registry {
            Some(Write::Name(name, record)) => {
                Arc::make_mut(&mut self.registry.names).insert(name, record);
            }
            Some(Write::OpenDeal(id, deal)) => {
                Arc::make_mut(&mut self.registry.deals).insert(id, deal);
            }
            Some(Write::CloseDeal(id)) => {
                Arc::make_mut(&mut self.registry.deals).remove(&id);
            }
            None => {}
        }
        Ok(())
    }

    /// Applique la politique du compte émetteur à `tx`, dont `body` est le corps
    /// effectif. Renvoie la politique mise à jour (dépense du jour) à écrire.
    fn check_policy(
        &self,
        tx: &vinx_core::Transaction,
        body: &TxBody,
    ) -> Result<Option<Policy>, TxError> {
        let approvals = match &tx.body {
            TxBody::Approved { approvals, .. } => Some(approvals),
            _ => None,
        };
        let Some(policy) = self.policies.get(&tx.sender) else {
            return match approvals {
                Some(_) => Err(TxError::Policy(PolicyError::InvalidApprovals)),
                None => Ok(None),
            };
        };
        if let Some(approvals) = approvals {
            let count =
                crate::policy::count_approvals(&policy.config, approvals, &tx.approval_message())
                    .map_err(TxError::Policy)?;
            return if count >= policy.config.threshold {
                Ok(None)
            } else {
                Err(TxError::Policy(PolicyError::ApprovalsMissing))
            };
        }
        // Sans approbation : seulement sous le plafond, et jamais pour changer
        // la politique ou la clé.
        if matches!(body, TxBody::SetPolicy { .. } | TxBody::RotateKey { .. }) {
            return Err(TxError::Policy(PolicyError::ApprovalsMissing));
        }
        let spend = match body {
            TxBody::Transfer { amount, .. } | TxBody::OpenDeal { amount, .. } => *amount,
            TxBody::BatchTransfer { payments } => payments
                .iter()
                .try_fold(0 as Amount, |s, p| s.checked_add(p.amount))
                .ok_or(TxError::Overflow)?,
            TxBody::RegisterName { .. } => self.registry.name_fee,
            _ => 0,
        };
        let mut updated = policy.clone();
        updated
            .spend_unapproved(self.registry.now / 86_400, spend)
            .map_err(TxError::Policy)?;
        Ok(Some(updated))
    }

    /// Enregistrement ou renouvellement de `name` par `sender` : renvoie la fiche à écrire.
    fn register_name(&self, sender: Address, name: &str) -> Result<NameRecord, TxError> {
        if !valid_name(name) {
            return Err(TxError::InvalidName);
        }
        let now = self.registry.now;
        let start = match self.registry.names.get(name) {
            // Renouvellement : l'année s'ajoute à l'échéance en cours.
            Some(r) if r.owner == sender => r.expires_at.max(now),
            Some(r) if now < r.expires_at.saturating_add(NAME_GRACE) => {
                return Err(TxError::NameTaken);
            }
            _ => now,
        };
        Ok(NameRecord {
            owner: sender,
            expires_at: start.checked_add(NAME_PERIOD).ok_or(TxError::Overflow)?,
        })
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
            AdminAction::SetParam {
                param: ParamId::NameFee,
                value,
            } => {
                if self.protocol_version < 2 {
                    return Err(TxError::NotYetActive);
                }
                self.registry.name_fee = *value;
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

    /// Protocole 3 : compte le bloc pour son proposeur et, en fin d'époque,
    /// verse l'émission due et la cagnotte aux validateurs.
    fn end_block(&mut self, height: u64, proposer: u16, now: u64) -> Result<(), BlockError> {
        if self.protocol_version < 3 {
            return Ok(());
        }
        let key = self
            .validators_at(height)
            .get(usize::from(proposer))
            .cloned()
            .ok_or(BlockError::UnknownProposer)?;
        self.emission.record(&key);
        let epoch = self.emission.epoch_blocks.max(1);
        if height.checked_rem(epoch) != Some(0) {
            return Ok(());
        }
        let mut changes = Changes::default();
        for (key, share) in self.emission.close_epoch(now) {
            changes
                .credit(&self.accounts, Address::from_public_key(&key), share)
                .map_err(|_| BlockError::InvariantViolated)?;
        }
        changes.commit(&mut self.accounts);
        Ok(())
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
        self.execute_by(height, timestamp_ms, None, admin_txs, txs)
    }

    /// Comme [`Self::execute`], pour un bloc dont `proposer` est l'indice du
    /// proposeur dans l'ensemble de la hauteur : c'est ce qui compte le bloc
    /// pour l'émission (protocole 3) et clôt l'époque. Sans proposeur (essais
    /// du mempool, sélection des transactions), rien de tout cela n'a lieu.
    pub fn execute_by(
        &self,
        height: u64,
        timestamp_ms: u64,
        proposer: Option<u16>,
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
        // Signatures vérifiées sur tous les cœurs ; l'application, séquentielle,
        // ne fait plus que les retrouver.
        crate::sigcache::preverify(self, txs);
        let mut next = self.clone();
        next.begin_block(height);
        if next.protocol_version > SUPPORTED_PROTOCOL {
            return Err(BlockError::WrongVersion);
        }
        next.registry.now = ctx.now;
        for (index, satx) in admin_txs.iter().enumerate() {
            next.apply_admin(satx, ctx)
                .map_err(|error| BlockError::InvalidAdminTx { index, error })?;
        }
        for (index, stx) in txs.iter().enumerate() {
            next.apply_tx(stx)
                .map_err(|error| BlockError::InvalidTx { index, error })?;
        }
        next.height = height;
        if let Some(index) = proposer {
            next.end_block(height, index, ctx.now)?;
        }
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
        let next = self.execute_by(
            h.height,
            h.timestamp,
            Some(h.proposer),
            &block.admin_txs,
            &block.txs,
        )?;
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

/// Écriture dans le registre, différée comme celles des comptes.
enum Write {
    Name(String, NameRecord),
    OpenDeal(Hash, Deal),
    CloseDeal(Hash),
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
