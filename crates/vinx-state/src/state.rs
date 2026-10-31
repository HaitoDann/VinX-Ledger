//! État global et application des transactions et des blocs (SPEC §4, §5, §6, §9).

use crate::accounts::Accounts;
use crate::assets::{
    Asset, AssetChanges, AssetDeal, AssetError, AssetMandate, Assets, Holding, MAX_ASSET_DECIMALS,
    MAX_ASSET_NAME,
};
use crate::emission::Emission;
use crate::escrow::{Escrow, EscrowError};
use crate::mandate::{Mandate, MandateError, Mandates};
use crate::policy::{Policies, Policy, PolicyError};
use crate::registry::{
    Deal, MAX_DEAL_DURATION, NAME_GRACE, NAME_PERIOD, NameRecord, Registry, valid_name,
};
use crate::reserve::{Owned, Reserves};
use crate::table::{Row, Table};
use crate::validators::{ValidatorChange, ValidatorError, ValidatorSet, validators_hash};
use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;
use thiserror::Error;
use vinx_core::params::MIN_UPGRADE_DELAY;
use vinx_core::{
    ADMIN_SIGNERS, Account, AdminAction, Amount, Block, ParamId, Params, SignedAdminTransaction,
    SignedTransaction, TOTAL_SUPPLY, TxBody, deal_id, encode,
};
use vinx_crypto::{Address, Domain, Hash, PublicKey, hash_tagged_parts, merkle_root};

/// Dernière version du protocole que ce code sait exécuter. Un nœud plus
/// ancien qu'une mise à jour activée s'arrête au lieu de diverger.
pub const SUPPORTED_PROTOCOL: u32 = 6;

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
    /// Mandats de prélèvement (protocole 4).
    pub mandates: Mandates,
    /// Actifs natifs et soldes (protocole 5).
    pub assets: Assets,
    /// Réserve par objet (protocole 6).
    pub reserves: Reserves,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TxError {
    #[error("{0}")]
    Asset(AssetError),
    #[error(
        "solde insuffisant pour la réserve des objets du compte (jetons acceptés, mandats, règles)"
    )]
    ObjectReserve,
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
    #[error("{0}")]
    Mandate(MandateError),
    #[error("enveloppes de transaction dans un ordre invalide")]
    BadEnvelope,
    #[error("parrain des frais invalide (compte, clé ou signature)")]
    BadSponsor,
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
    mandates: Mandates,
    assets: Assets,
    reserves: Reserves,
}

/// [`Globals`] des versions 0.15 à 0.20, sans réserve par objet.
#[derive(BorshDeserialize)]
struct GlobalsV5 {
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
    mandates: Mandates,
    assets: Assets,
}

impl From<GlobalsV5> for Globals {
    fn from(g: GlobalsV5) -> Self {
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
            policies: g.policies,
            mandates: g.mandates,
            assets: g.assets,
            reserves: Reserves::default(),
        }
    }
}

/// [`Globals`] des versions 0.13 et 0.14, sans actifs.
#[derive(BorshDeserialize)]
struct GlobalsV4M {
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
    mandates: Mandates,
}

impl From<GlobalsV4M> for Globals {
    fn from(g: GlobalsV4M) -> Self {
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
            policies: g.policies,
            mandates: g.mandates,
            assets: Assets::default(),
            reserves: Reserves::default(),
        }
    }
}

/// [`Globals`] de la version 0.12, sans mandats.
#[derive(BorshDeserialize)]
struct GlobalsV4 {
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

impl From<GlobalsV4> for Globals {
    fn from(g: GlobalsV4) -> Self {
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
            policies: g.policies,
            mandates: Mandates::default(),
            assets: Assets::default(),
            reserves: Reserves::default(),
        }
    }
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
            mandates: Mandates::default(),
            assets: Assets::default(),
            reserves: Reserves::default(),
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
            mandates: Mandates::default(),
            assets: Assets::default(),
            reserves: Reserves::default(),
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
            mandates: Mandates::default(),
            assets: Assets::default(),
            reserves: Reserves::default(),
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
            mandates: self.mandates.clone(),
            assets: self.assets.clone(),
            reserves: self.reserves.clone(),
        })
    }

    /// Comme [`Self::globals_bytes`], sans les lignes des tables : le stockage
    /// les écrit à part, ligne par ligne ([`Self::take_dirty_rows`]).
    pub fn globals_bytes_light(&self) -> Vec<u8> {
        let mut registry = self.registry.clone();
        registry.names = Table::new();
        registry.deals = Table::new();
        encode(&Globals {
            chain_id: self.chain_id.clone(),
            protocol_version: self.protocol_version,
            height: self.height,
            escrow: self.escrow.clone(),
            admin: self.admin.clone(),
            validators: self.validators.clone(),
            params: self.params.clone(),
            upgrade: self.upgrade.clone(),
            registry,
            emission: self.emission.clone(),
            policies: Table::new(),
            mandates: Table::new(),
            assets: Assets {
                fee: self.assets.fee,
                ..Assets::default()
            },
            reserves: Reserves {
                per_object: self.reserves.per_object,
                ..Reserves::default()
            },
        })
    }

    /// Lignes des tables modifiées depuis le dernier appel : clé
    /// `étiquette ‖ borsh(clé)`, valeur `borsh(valeur)` ou `None` si supprimée.
    pub fn take_dirty_rows(&mut self) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
        fn rows<K: crate::table::Key, V: Row>(
            t: &mut Table<K, V>,
        ) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
            t.take_dirty()
                .into_iter()
                .map(|(k, v)| {
                    let mut key = vec![V::TAG];
                    key.extend_from_slice(&encode(&k));
                    (key, v.map(|v| encode(&v)))
                })
                .collect()
        }
        let mut out = rows(&mut self.registry.names);
        out.extend(rows(&mut self.registry.deals));
        out.extend(rows(&mut self.policies));
        out.extend(rows(&mut self.mandates));
        out.extend(rows(&mut self.assets.assets));
        out.extend(rows(&mut self.assets.holdings));
        out.extend(rows(&mut self.assets.deals));
        out.extend(rows(&mut self.assets.mandates));
        out.extend(rows(&mut self.reserves.owned));
        out
    }

    /// Recharge une ligne écrite par [`Self::take_dirty_rows`].
    pub fn load_row(&mut self, key: &[u8], value: &[u8]) -> Option<()> {
        fn load<K: crate::table::Key, V: Row>(
            t: &mut Table<K, V>,
            key: &[u8],
            value: &[u8],
        ) -> Option<()> {
            let k = vinx_core::decode(key).ok()?;
            let v = vinx_core::decode(value).ok()?;
            t.load(k, v);
            Some(())
        }
        let (tag, key) = key.split_first()?;
        match *tag {
            t if t == <NameRecord as Row>::TAG => load(&mut self.registry.names, key, value),
            t if t == <Deal as Row>::TAG => load(&mut self.registry.deals, key, value),
            t if t == <Policy as Row>::TAG => load(&mut self.policies, key, value),
            t if t == <Mandate as Row>::TAG => load(&mut self.mandates, key, value),
            t if t == <Asset as Row>::TAG => load(&mut self.assets.assets, key, value),
            t if t == <Holding as Row>::TAG => load(&mut self.assets.holdings, key, value),
            t if t == <AssetDeal as Row>::TAG => load(&mut self.assets.deals, key, value),
            t if t == <AssetMandate as Row>::TAG => load(&mut self.assets.mandates, key, value),
            t if t == <Owned as Row>::TAG => load(&mut self.reserves.owned, key, value),
            _ => None,
        }
    }

    /// Note toutes les lignes des tables comme modifiées (installation d'un instantané).
    pub fn mark_all_rows_dirty(&mut self) {
        self.registry.names.mark_all_dirty();
        self.registry.deals.mark_all_dirty();
        self.policies.mark_all_dirty();
        self.mandates.mark_all_dirty();
        self.assets.assets.mark_all_dirty();
        self.assets.holdings.mark_all_dirty();
        self.assets.deals.mark_all_dirty();
        self.assets.mandates.mark_all_dirty();
        self.reserves.owned.mark_all_dirty();
    }

    /// Reconstitue un état à partir de [`Self::globals_bytes`] et des comptes.
    pub fn from_parts(globals: &[u8], accounts: Accounts) -> Option<Self> {
        let g: Globals = vinx_core::decode(globals)
            .or_else(|_| vinx_core::decode::<GlobalsV5>(globals).map(Globals::from))
            .or_else(|_| vinx_core::decode::<GlobalsV4M>(globals).map(Globals::from))
            .or_else(|_| vinx_core::decode::<GlobalsV4>(globals).map(Globals::from))
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
            mandates: g.mandates,
            assets: g.assets,
            reserves: g.reserves,
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
        if self.protocol_version >= 4 {
            // Les tables par leur racine (§4.5) : coût proportionnel aux lignes modifiées.
            let r = &self.registry;
            globals.extend_from_slice(&encode(&(
                r.now,
                r.name_fee,
                r.names.root(),
                r.deals.root(),
            )));
        } else if self.protocol_version >= 2 {
            globals.extend_from_slice(&encode(&self.registry));
        }
        if self.protocol_version >= 3 {
            globals.extend_from_slice(&encode(&self.emission));
        }
        if self.protocol_version >= 4 {
            globals.extend_from_slice(&encode(&(self.policies.root(), self.mandates.root())));
        }
        if self.protocol_version >= 5 {
            let a = &self.assets;
            globals.extend_from_slice(&encode(&(
                a.fee,
                a.assets.root(),
                a.holdings.root(),
                a.deals.root(),
                a.mandates.root(),
            )));
        }
        if self.protocol_version >= 6 {
            let r = &self.reserves;
            globals.extend_from_slice(&encode(&(r.per_object, r.owned.root())));
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
        if !tx.body.well_wrapped() {
            return Err(TxError::BadEnvelope);
        }
        let body = tx.body.inner();
        if body.min_protocol() > self.protocol_version {
            return Err(TxError::NotYetActive);
        }
        let policy_write = self.check_policy(tx, body)?;
        let fee_payer = match &tx.body {
            TxBody::Sponsored {
                sponsor, signature, ..
            } => {
                let key = self
                    .accounts
                    .get(sponsor)
                    .and_then(|a| a.key.clone())
                    .ok_or(TxError::BadSponsor)?;
                if *sponsor == tx.sender
                    || key.verify(&tx.sponsor_message(sponsor), signature).is_err()
                {
                    return Err(TxError::BadSponsor);
                }
                *sponsor
            }
            _ => tx.sender,
        };
        let now = self.registry.now;
        let mut mandate_write: Option<((Address, Address), Option<Mandate>)> = None;

        let mut changes = Changes::default();
        let mut registry = None;
        changes.debit(&self.accounts, fee_payer, tx.fee)?;
        // Protocole 3 : les frais vont aux validateurs, via la cagnotte d'époque.
        let fee_to_pot = self.protocol_version >= 3;
        if !fee_to_pot {
            changes.credit(&self.accounts, Address::treasury(), tx.fee)?;
        }
        let account = changes.get(&self.accounts, tx.sender);
        account.nonce = tx.nonce;
        account.key = Some(key);
        let mut set_policy = None;
        let mut assets = AssetChanges::default();
        let mut asset_deal_write: Option<(Hash, Option<AssetDeal>)> = None;
        let mut asset_mandate_write: Option<((String, Address, Address), Option<AssetMandate>)> =
            None;
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
            // `inner()` ne renvoie jamais une enveloppe.
            TxBody::Approved { .. } | TxBody::Sponsored { .. } => {
                return Err(TxError::BadEnvelope);
            }
            TxBody::GrantMandate {
                payee,
                max_per_period,
                period,
                expires_at,
            } => {
                let valid = *payee != tx.sender
                    && *max_per_period > 0
                    && (vinx_core::MIN_MANDATE_PERIOD..=vinx_core::MAX_MANDATE_PERIOD)
                        .contains(period)
                    && expires_at.is_none_or(|e| e > now);
                if !valid {
                    return Err(TxError::Mandate(MandateError::Invalid));
                }
                mandate_write = Some((
                    (tx.sender, *payee),
                    Some(Mandate {
                        max_per_period: *max_per_period,
                        period: *period,
                        start: now,
                        expires_at: *expires_at,
                        period_index: 0,
                        used: 0,
                    }),
                ));
            }
            TxBody::RevokeMandate { payee } => {
                if !self.mandates.contains_key(&(tx.sender, *payee)) {
                    return Err(TxError::Mandate(MandateError::Unknown));
                }
                mandate_write = Some(((tx.sender, *payee), None));
            }
            TxBody::Collect {
                payer,
                amount,
                memo,
            } => {
                if *amount == 0 {
                    return Err(TxError::ZeroAmount);
                }
                let mut mandate = self
                    .mandates
                    .get(&(*payer, tx.sender))
                    .cloned()
                    .ok_or(TxError::Mandate(MandateError::Unknown))?;
                mandate.collect(now, *amount).map_err(TxError::Mandate)?;
                self.check_payment(*payer, tx.sender, *amount, memo)?;
                changes.debit(&self.accounts, *payer, *amount)?;
                changes.credit(&self.accounts, tx.sender, *amount)?;
                let left = changes.get(&self.accounts, *payer).balance;
                if left != 0 && left < self.params.min_reserve {
                    return Err(TxError::BelowReserve);
                }
                self.check_object_reserve(*payer, left)?;
                mandate_write = Some(((*payer, tx.sender), Some(mandate)));
            }
            TxBody::CreateAsset {
                symbol,
                name,
                decimals,
                initial_supply,
                max_supply,
                powers,
            } => {
                let owner = self.registry.resolve(&symbol.to_ascii_lowercase(), now);
                if *symbol != crate::assets::symbol_of(symbol) || owner != Some(tx.sender) {
                    return Err(TxError::Asset(AssetError::NotSymbolOwner));
                }
                if self.assets.assets.contains_key(symbol.as_str()) {
                    return Err(TxError::Asset(AssetError::SymbolTaken));
                }
                let valid = !name.trim().is_empty()
                    && name.len() <= MAX_ASSET_NAME
                    && *decimals <= MAX_ASSET_DECIMALS
                    && max_supply.is_none_or(|m| m > 0 && *initial_supply <= m)
                    && (*initial_supply > 0 || powers.mint);
                if !valid {
                    return Err(TxError::Asset(AssetError::Invalid));
                }
                let fee = self.assets.fee;
                if fee > 0 {
                    changes.debit(&self.accounts, tx.sender, fee)?;
                    changes.credit(&self.accounts, Address::treasury(), fee)?;
                }
                assets.accept(&self.assets, symbol, tx.sender);
                if *initial_supply > 0 {
                    assets
                        .credit(&self.assets, symbol, tx.sender, *initial_supply)
                        .map_err(TxError::Asset)?;
                }
                assets.assets.push((
                    symbol.clone(),
                    Asset {
                        issuer: tx.sender,
                        name: name.clone(),
                        decimals: *decimals,
                        supply: *initial_supply,
                        max_supply: *max_supply,
                        powers: *powers,
                        frozen: false,
                    },
                ));
            }
            TxBody::MintAsset { symbol, to, amount } => {
                let mut asset = self.issued_asset(symbol, tx.sender)?;
                if !asset.powers.mint {
                    return Err(TxError::Asset(AssetError::PowerMissing));
                }
                if *amount == 0 {
                    return Err(TxError::ZeroAmount);
                }
                asset.supply = asset
                    .supply
                    .checked_add(*amount)
                    .filter(|s| asset.max_supply.is_none_or(|m| *s <= m))
                    .ok_or(TxError::Asset(AssetError::MaxSupply))?;
                assets
                    .credit(&self.assets, symbol, *to, *amount)
                    .map_err(TxError::Asset)?;
                assets.assets.push((symbol.clone(), asset));
            }
            TxBody::BurnAsset { symbol, amount } => {
                let mut asset = self.asset(symbol)?;
                if *amount == 0 {
                    return Err(TxError::ZeroAmount);
                }
                if asset.frozen || assets.is_frozen(&self.assets, symbol, tx.sender) {
                    return Err(TxError::Asset(AssetError::Frozen));
                }
                assets
                    .debit(&self.assets, symbol, tx.sender, *amount)
                    .map_err(TxError::Asset)?;
                asset.supply = asset.supply.checked_sub(*amount).ok_or(TxError::Overflow)?;
                assets.assets.push((symbol.clone(), asset));
            }
            TxBody::TransferAsset {
                symbol,
                to,
                amount,
                memo,
            } => {
                let asset = self.asset(symbol)?;
                self.check_asset_payment(tx.sender, *to, *amount, memo)?;
                let frozen = asset.frozen
                    || assets.is_frozen(&self.assets, symbol, tx.sender)
                    || assets.is_frozen(&self.assets, symbol, *to);
                if frozen {
                    return Err(TxError::Asset(AssetError::Frozen));
                }
                assets
                    .debit(&self.assets, symbol, tx.sender, *amount)
                    .map_err(TxError::Asset)?;
                assets
                    .credit(&self.assets, symbol, *to, *amount)
                    .map_err(TxError::Asset)?;
            }
            TxBody::FreezeAsset {
                symbol,
                account,
                frozen,
            } => {
                let mut asset = self.issued_asset(symbol, tx.sender)?;
                // Dégeler reste possible après avoir abandonné le pouvoir de gel.
                if *frozen && !asset.powers.freeze {
                    return Err(TxError::Asset(AssetError::PowerMissing));
                }
                match account {
                    Some(who) => assets
                        .set_frozen(&self.assets, symbol, *who, *frozen)
                        .map_err(TxError::Asset)?,
                    None => {
                        asset.frozen = *frozen;
                        assets.assets.push((symbol.clone(), asset));
                    }
                }
            }
            TxBody::ClawbackAsset {
                symbol,
                from,
                amount,
            } => {
                let asset = self.issued_asset(symbol, tx.sender)?;
                if !asset.powers.clawback {
                    return Err(TxError::Asset(AssetError::PowerMissing));
                }
                if *amount == 0 {
                    return Err(TxError::ZeroAmount);
                }
                if *from == tx.sender {
                    return Err(TxError::SelfTransfer);
                }
                assets
                    .debit(&self.assets, symbol, *from, *amount)
                    .map_err(TxError::Asset)?;
                assets.accept(&self.assets, symbol, tx.sender);
                assets
                    .credit(&self.assets, symbol, tx.sender, *amount)
                    .map_err(TxError::Asset)?;
            }
            TxBody::AcceptAsset { symbol, accept } => {
                self.asset(symbol)?;
                if *accept {
                    assets.accept(&self.assets, symbol, tx.sender);
                } else {
                    assets
                        .leave(&self.assets, symbol, tx.sender)
                        .map_err(TxError::Asset)?;
                }
            }
            TxBody::RenounceAssetPowers { symbol, powers } => {
                let mut asset = self.issued_asset(symbol, tx.sender)?;
                let p = &mut asset.powers;
                p.mint &= !powers.mint;
                p.freeze &= !powers.freeze;
                p.clawback &= !powers.clawback;
                assets.assets.push((symbol.clone(), asset));
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
            TxBody::ReleaseDeal { id } if !self.registry.deals.contains_key(id) => {
                let ad = self.assets.deals.get(id).ok_or(TxError::UnknownDeal)?;
                let deal = &ad.deal;
                if tx.sender != deal.buyer && deal.arbiter != Some(tx.sender) {
                    return Err(TxError::DealForbidden);
                }
                self.pay_from_deal(&mut assets, &ad.symbol, deal.seller, deal.amount)?;
                asset_deal_write = Some((*id, None));
            }
            TxBody::RefundDeal { id } if !self.registry.deals.contains_key(id) => {
                let ad = self.assets.deals.get(id).ok_or(TxError::UnknownDeal)?;
                let deal = &ad.deal;
                let expired = now >= deal.deadline;
                let allowed = tx.sender == deal.seller
                    || deal.arbiter == Some(tx.sender)
                    || (tx.sender == deal.buyer && expired);
                if !allowed {
                    return Err(TxError::DealForbidden);
                }
                self.pay_from_deal(&mut assets, &ad.symbol, deal.buyer, deal.amount)?;
                asset_deal_write = Some((*id, None));
            }
            TxBody::OpenAssetDeal {
                symbol,
                seller,
                arbiter,
                amount,
                deadline,
                memo,
            } => {
                let asset = self.asset(symbol)?;
                let latest = now
                    .checked_add(MAX_DEAL_DURATION)
                    .ok_or(TxError::Overflow)?;
                let bad_arbiter = arbiter.is_some_and(|a| a == tx.sender || a == *seller);
                if bad_arbiter || *deadline <= now || *deadline > latest {
                    return Err(TxError::InvalidDeal);
                }
                self.check_asset_payment(tx.sender, *seller, *amount, memo)?;
                // Le vendeur doit accepter l'actif : il sera payé dans cet actif.
                if !assets.accepted(&self.assets, symbol, *seller) {
                    return Err(TxError::Asset(AssetError::NotAccepted));
                }
                if asset.frozen || assets.is_frozen(&self.assets, symbol, tx.sender) {
                    return Err(TxError::Asset(AssetError::Frozen));
                }
                assets
                    .debit(&self.assets, symbol, tx.sender, *amount)
                    .map_err(TxError::Asset)?;
                let deal = Deal {
                    buyer: tx.sender,
                    seller: *seller,
                    arbiter: *arbiter,
                    amount: *amount,
                    deadline: *deadline,
                    memo: memo.clone(),
                };
                asset_deal_write = Some((
                    deal_id(&tx.sender, tx.nonce),
                    Some(AssetDeal {
                        symbol: symbol.clone(),
                        deal,
                    }),
                ));
            }
            TxBody::GrantAssetMandate {
                symbol,
                payee,
                max_per_period,
                period,
                expires_at,
            } => {
                self.asset(symbol)?;
                let valid = *payee != tx.sender
                    && *max_per_period > 0
                    && (vinx_core::MIN_MANDATE_PERIOD..=vinx_core::MAX_MANDATE_PERIOD)
                        .contains(period)
                    && expires_at.is_none_or(|e| e > now);
                if !valid {
                    return Err(TxError::Mandate(MandateError::Invalid));
                }
                asset_mandate_write = Some((
                    (symbol.clone(), tx.sender, *payee),
                    Some(AssetMandate {
                        mandate: Mandate {
                            max_per_period: *max_per_period,
                            period: *period,
                            start: now,
                            expires_at: *expires_at,
                            period_index: 0,
                            used: 0,
                        },
                    }),
                ));
            }
            TxBody::RevokeAssetMandate { symbol, payee } => {
                let key = (symbol.clone(), tx.sender, *payee);
                if !self.assets.mandates.contains_key(&key) {
                    return Err(TxError::Mandate(MandateError::Unknown));
                }
                asset_mandate_write = Some((key, None));
            }
            TxBody::CollectAsset {
                symbol,
                payer,
                amount,
                memo,
            } => {
                let asset = self.asset(symbol)?;
                let key = (symbol.clone(), *payer, tx.sender);
                let mut m = self
                    .assets
                    .mandates
                    .get(&key)
                    .cloned()
                    .ok_or(TxError::Mandate(MandateError::Unknown))?;
                m.mandate.collect(now, *amount).map_err(TxError::Mandate)?;
                self.check_asset_payment(*payer, tx.sender, *amount, memo)?;
                let frozen = asset.frozen
                    || assets.is_frozen(&self.assets, symbol, *payer)
                    || assets.is_frozen(&self.assets, symbol, tx.sender);
                if frozen {
                    return Err(TxError::Asset(AssetError::Frozen));
                }
                assets
                    .debit(&self.assets, symbol, *payer, *amount)
                    .map_err(TxError::Asset)?;
                assets
                    .credit(&self.assets, symbol, tx.sender, *amount)
                    .map_err(TxError::Asset)?;
                asset_mandate_write = Some((key, Some(m)));
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
        for who in [tx.sender, fee_payer] {
            let remaining = changes.get(&self.accounts, who).balance;
            if remaining != 0 && remaining < self.params.min_reserve {
                return Err(TxError::BelowReserve);
            }
        }
        // Protocole 6 : objets créés ou supprimés, et réserve de l'émetteur.
        let mut deltas: std::collections::BTreeMap<Address, i64> = Default::default();
        if self.protocol_version >= 6 {
            assets.owner_deltas(&self.assets, &mut deltas);
            let mut bump = |who: Address, before: bool, after: bool| {
                let d = i64::from(after).saturating_sub(i64::from(before));
                if d != 0 {
                    let e = deltas.entry(who).or_insert(0);
                    *e = e.saturating_add(d);
                }
            };
            if let Some((key, m)) = &mandate_write {
                bump(key.0, self.mandates.contains_key(key), m.is_some());
            }
            if let Some((key, m)) = &asset_mandate_write {
                bump(key.1, self.assets.mandates.contains_key(key), m.is_some());
            }
            if let Some(p) = &set_policy {
                bump(
                    tx.sender,
                    self.policies.contains_key(&tx.sender),
                    p.is_some(),
                );
            }
            // L'émetteur ne peut ni créer d'objet, ni dépenser au-delà des
            // frais, en passant sous sa réserve ; payer des frais reste
            // toujours permis, ne serait-ce que pour supprimer des objets.
            let before_count = self.reserves.count(&tx.sender);
            let delta = deltas.get(&tx.sender).copied().unwrap_or(0);
            let after_count = u32::try_from(i64::from(before_count).saturating_add(delta).max(0))
                .unwrap_or(u32::MAX);
            let before = self.balance(&tx.sender);
            let after = changes.get(&self.accounts, tx.sender).balance;
            let own_fee = if fee_payer == tx.sender { tx.fee } else { 0 };
            let spent = before.saturating_sub(after) > own_fee;
            let required = self.reserves.required(after_count, self.params.min_reserve);
            if after < required && (after_count > before_count || spent) {
                return Err(TxError::ObjectReserve);
            }
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
        assets.commit(&mut self.assets);
        self.reserves.apply(&deltas);
        match asset_deal_write {
            Some((id, Some(d))) => self.assets.deals.insert(id, d),
            Some((id, None)) => {
                self.assets.deals.remove(&id);
            }
            None => {}
        }
        match asset_mandate_write {
            Some((key, Some(m))) => self.assets.mandates.insert(key, m),
            Some((key, None)) => {
                self.assets.mandates.remove(&key);
            }
            None => {}
        }
        self.emission.pot = pot;
        match mandate_write {
            Some((key, Some(m))) => {
                self.mandates.insert(key, m);
            }
            Some((key, None)) => {
                self.mandates.remove(&key);
            }
            None => {}
        }
        if let Some(p) = policy_write {
            self.policies.insert(tx.sender, p);
        }
        match set_policy {
            Some(Some(p)) => {
                self.policies.insert(tx.sender, p);
            }
            Some(None) => {
                self.policies.remove(&tx.sender);
            }
            None => {}
        }
        match registry {
            Some(Write::Name(name, record)) => {
                self.registry.names.insert(name, record);
            }
            Some(Write::OpenDeal(id, deal)) => {
                self.registry.deals.insert(id, deal);
            }
            Some(Write::CloseDeal(id)) => {
                self.registry.deals.remove(&id);
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
        let approvals = tx.body.approvals();
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
        if matches!(
            body,
            TxBody::SetPolicy { .. }
                | TxBody::RotateKey { .. }
                | TxBody::GrantMandate { .. }
                | TxBody::GrantAssetMandate { .. }
                | TxBody::MintAsset { .. }
                | TxBody::FreezeAsset { .. }
                | TxBody::ClawbackAsset { .. }
                | TxBody::RenounceAssetPowers { .. }
        ) {
            return Err(TxError::Policy(PolicyError::ApprovalsMissing));
        }
        let spend = match body {
            TxBody::Transfer { amount, .. } | TxBody::OpenDeal { amount, .. } => *amount,
            TxBody::BatchTransfer { payments } => payments
                .iter()
                .try_fold(0 as Amount, |s, p| s.checked_add(p.amount))
                .ok_or(TxError::Overflow)?,
            TxBody::RegisterName { .. } => self.registry.name_fee,
            TxBody::CreateAsset { .. } => self.assets.fee,
            _ => 0,
        };
        let mut updated = policy.clone();
        updated
            .spend_unapproved(self.registry.now / 86_400, spend)
            .map_err(TxError::Policy)?;
        Ok(Some(updated))
    }

    /// Protocole 6 : `who`, qui ne garde que `left`, couvre-t-il sa réserve ?
    fn check_object_reserve(&self, who: Address, left: Amount) -> Result<(), TxError> {
        if self.protocol_version < 6 {
            return Ok(());
        }
        let required = self
            .reserves
            .required(self.reserves.count(&who), self.params.min_reserve);
        if left < required {
            return Err(TxError::ObjectReserve);
        }
        Ok(())
    }

    /// Recompte les objets de chaque compte à partir des tables : à
    /// l'activation du protocole 6, pour les objets créés avant.
    fn recount_objects(&mut self) {
        let mut deltas: std::collections::BTreeMap<Address, i64> = Default::default();
        let mut add = |who: Address| {
            let e = deltas.entry(who).or_insert(0);
            *e = e.saturating_add(1);
        };
        self.assets
            .holdings
            .iter()
            .for_each(|((_, who), _)| add(*who));
        self.mandates.iter().for_each(|((payer, _), _)| add(*payer));
        self.assets
            .mandates
            .iter()
            .for_each(|((_, payer, _), _)| add(*payer));
        self.policies.iter().for_each(|(who, _)| add(*who));
        self.reserves.owned = Table::new();
        self.reserves.apply(&deltas);
    }

    fn asset(&self, symbol: &str) -> Result<Asset, TxError> {
        self.assets
            .assets
            .get(symbol)
            .cloned()
            .ok_or(TxError::Asset(AssetError::Unknown))
    }

    /// Verse `amount` d'un séquestre en actif à `to`, qui l'accepte d'office :
    /// il y a consenti en ouvrant le séquestre ou en l'acceptant comme vendeur.
    /// Refusé si l'actif ou le compte est gelé.
    fn pay_from_deal(
        &self,
        assets: &mut AssetChanges,
        symbol: &str,
        to: Address,
        amount: Amount,
    ) -> Result<(), TxError> {
        let asset = self.asset(symbol)?;
        if asset.frozen || assets.is_frozen(&self.assets, symbol, to) {
            return Err(TxError::Asset(AssetError::Frozen));
        }
        assets.accept(&self.assets, symbol, to);
        assets
            .credit(&self.assets, symbol, to, amount)
            .map_err(TxError::Asset)
    }

    /// L'actif `symbol`, si `sender` en est l'émetteur.
    fn issued_asset(&self, symbol: &str, sender: Address) -> Result<Asset, TxError> {
        let asset = self.asset(symbol)?;
        if asset.issuer != sender {
            return Err(TxError::Asset(AssetError::NotIssuer));
        }
        Ok(asset)
    }

    /// Règles d'un transfert d'actif : montant, mémo et exigence de mémo du
    /// destinataire. Pas de réserve : le destinataire n'a pas besoin de compte.
    fn check_asset_payment(
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
        if memo.is_none() && self.accounts.get(&to).is_some_and(|a| a.memo_required) {
            return Err(TxError::MemoRequired);
        }
        Ok(())
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
            AdminAction::SetParam {
                param: ParamId::ObjectReserve,
                value,
            } => {
                if self.protocol_version < 6 {
                    return Err(TxError::NotYetActive);
                }
                self.reserves.per_object = *value;
            }
            AdminAction::SetParam {
                param: ParamId::AssetFee,
                value,
            } => {
                if self.protocol_version < 5 {
                    return Err(TxError::NotYetActive);
                }
                self.assets.fee = *value;
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
                let before = self.protocol_version;
                self.protocol_version = up.version;
                self.upgrade = None;
                if before < 6 && self.protocol_version >= 6 {
                    self.recount_objects();
                }
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
