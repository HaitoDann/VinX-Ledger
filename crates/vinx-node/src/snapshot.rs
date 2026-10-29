//! Synchronisation par l'état (SPEC §8.3).
//!
//! Un nœud dont les blocs manquants ont été élagués chez ses pairs télécharge
//! directement l'état d'une hauteur récente, par morceaux, au lieu de rejouer
//! la chaîne. L'état n'est adopté qu'après vérification complète :
//!
//! 1. le bloc de l'instantané porte un certificat de commit valide pour
//!    l'ensemble de validateurs de la genèse ;
//! 2. la racine de l'état téléchargé est exactement le `state_root` de ce bloc,
//!    et l'ensemble de validateurs qu'il contient redonne son `validators_hash` ;
//! 3. l'invariant de supply est respecté.
//!
//! Le point 1 suppose que les validateurs n'ont pas changé depuis la genèse.
//! Sinon, la vérification échoue et le nœud doit se synchroniser bloc par bloc
//! auprès d'un nœud archive, ce qui vérifie chaque changement de validateurs.

use borsh::{BorshDeserialize, BorshSerialize};
use thiserror::Error;
use vinx_core::{Account, Block, CommitCertificate};
use vinx_crypto::{Address, PublicKey};
use vinx_state::{Accounts, State};

/// Comptes par morceau d'instantané.
pub const CHUNK_ACCOUNTS: usize = 10_000;

/// Un nouvel instantané est préparé toutes les `SNAPSHOT_INTERVAL` hauteurs.
pub const SNAPSHOT_INTERVAL: u64 = 100;

/// Instantané servi aux pairs : une copie figée de l'état (quasi gratuite).
#[derive(Clone)]
pub struct Snapshot {
    pub block: Block,
    pub cert: CommitCertificate,
    pub state: State,
}

/// Description d'un instantané : de quoi le vérifier avant d'en télécharger l'état.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotInfo {
    pub block: Block,
    pub cert: CommitCertificate,
    /// État sans les comptes (escrow, validateurs, paramètres…).
    pub globals: Vec<u8>,
    pub accounts: u64,
}

impl Snapshot {
    pub fn info(&self) -> SnapshotInfo {
        SnapshotInfo {
            block: self.block.clone(),
            cert: self.cert.clone(),
            globals: self.state.globals_bytes(),
            accounts: u64::try_from(self.state.accounts.len()).unwrap_or(u64::MAX),
        }
    }

    /// Morceau suivant `after`. Le second élément indique si c'est le dernier.
    pub fn chunk(&self, after: Option<&Address>) -> (Vec<(Address, Account)>, bool) {
        let chunk = self.state.accounts.chunk_after(after, CHUNK_ACCOUNTS);
        let done = chunk.len() < CHUNK_ACCOUNTS;
        (chunk, done)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("bloc et certificat incohérents")]
    Mismatch,
    #[error("certificat de commit invalide pour les validateurs de la genèse")]
    Certificate,
    #[error("état sans en-tête lisible")]
    Globals,
    #[error("comptes non triés ou dupliqués")]
    Order,
    #[error("la racine de l'état téléchargé ne correspond pas au bloc")]
    StateRoot,
    #[error("l'ensemble de validateurs ne correspond pas au bloc")]
    Validators,
    #[error("invariant de supply violé")]
    Invariant,
}

/// Vérifie l'en-tête d'un instantané avant d'en télécharger les comptes.
pub fn verify_info(
    info: &SnapshotInfo,
    chain_id: &str,
    genesis_validators: &[PublicKey],
) -> Result<(), SnapshotError> {
    let h = &info.block.header;
    if info.cert.height != h.height
        || info.cert.block_hash != info.block.hash()
        || h.chain_id != chain_id
    {
        return Err(SnapshotError::Mismatch);
    }
    info.cert
        .verify(chain_id, genesis_validators)
        .map_err(|_| SnapshotError::Certificate)
}

/// Assemble et vérifie l'état complet d'un instantané.
pub fn assemble(
    info: &SnapshotInfo,
    accounts: Vec<(Address, Account)>,
) -> Result<State, SnapshotError> {
    let ordered = accounts.windows(2).all(|w| match w {
        [a, b] => a.0 < b.0,
        _ => true,
    });
    if !ordered {
        return Err(SnapshotError::Order);
    }
    let accounts: Accounts = accounts.into_iter().collect();
    let state = State::from_parts(&info.globals, accounts).ok_or(SnapshotError::Globals)?;
    let h = &info.block.header;
    if state.height != h.height || state.chain_id != h.chain_id {
        return Err(SnapshotError::Mismatch);
    }
    if state.state_root() != h.state_root {
        return Err(SnapshotError::StateRoot);
    }
    if state.next_validators_hash() != h.validators_hash {
        return Err(SnapshotError::Validators);
    }
    state
        .check_invariant()
        .map_err(|_| SnapshotError::Invariant)?;
    Ok(state)
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)] // code de test
mod tests {
    use super::*;
    use vinx_core::{BlockHeader, Vote, VoteKind};
    use vinx_crypto::KeyPair;
    use vinx_state::Genesis;

    /// Instantané valide d'une chaîne à un validateur, avec quelques comptes.
    fn sample() -> (KeyPair, SnapshotInfo, Vec<(Address, Account)>) {
        let validator = KeyPair::generate();
        let k = || KeyPair::generate().public_key();
        let mut state = Genesis::legacy(
            "vinx-snap",
            1,
            [k(), k(), k()],
            vec![validator.public_key()],
        )
        .unwrap()
        .into_state()
        .unwrap();
        let mut treasury = state.accounts[&Address::treasury()].clone();
        for i in 1..=5u8 {
            treasury.balance -= 10;
            state.set_account_for_tests(
                Address::from_bytes([i; 20]),
                Account {
                    balance: 10,
                    ..Account::default()
                },
            );
        }
        state.set_account_for_tests(Address::treasury(), treasury);
        state.height = 7;
        let block = Block {
            header: BlockHeader {
                protocol_version: 1,
                chain_id: "vinx-snap".into(),
                height: 7,
                timestamp: 7,
                prev_hash: [0; 32],
                tx_root: [0; 32],
                state_root: state.state_root(),
                validators_hash: state.next_validators_hash(),
                proposer: 0,
            },
            admin_txs: vec![],
            txs: vec![],
        };
        let vote = Vote {
            chain_id: "vinx-snap".into(),
            kind: VoteKind::Precommit,
            height: 7,
            round: 0,
            block_hash: Some(block.hash()),
        };
        let cert = CommitCertificate {
            height: 7,
            round: 0,
            block_hash: block.hash(),
            signatures: vec![(0, vote.sign(&validator))],
        };
        let accounts = state.accounts.chunk_after(None, usize::MAX);
        let info = SnapshotInfo {
            block,
            cert,
            globals: state.globals_bytes(),
            accounts: accounts.len() as u64,
        };
        (validator, info, accounts)
    }

    #[test]
    fn valid_snapshot_is_accepted() {
        let (v, info, accounts) = sample();
        verify_info(&info, "vinx-snap", &[v.public_key()]).unwrap();
        let state = assemble(&info, accounts).unwrap();
        assert_eq!(state.state_root(), info.block.header.state_root);
    }

    #[test]
    fn certificate_from_other_validators_is_refused() {
        let (_, info, _) = sample();
        let impostor = KeyPair::generate().public_key();
        assert_eq!(
            verify_info(&info, "vinx-snap", std::slice::from_ref(&impostor)),
            Err(SnapshotError::Certificate)
        );
        assert_eq!(
            verify_info(&info, "autre-chaine", &[impostor]),
            Err(SnapshotError::Mismatch)
        );
    }

    #[test]
    fn tampered_accounts_are_refused() {
        let (_, info, accounts) = sample();
        // Un solde gonflé : la racine ne correspond plus (et l'invariant non plus).
        let mut inflated = accounts.clone();
        inflated[1].1.balance += 1_000;
        assert_eq!(assemble(&info, inflated), Err(SnapshotError::StateRoot));
        // Un compte retiré.
        let mut missing = accounts.clone();
        missing.remove(2);
        assert_eq!(assemble(&info, missing), Err(SnapshotError::StateRoot));
        // Des comptes dans le désordre.
        let mut shuffled = accounts;
        shuffled.swap(0, 1);
        assert_eq!(assemble(&info, shuffled), Err(SnapshotError::Order));
    }

    #[test]
    fn tampered_globals_are_refused() {
        let (_, mut info, accounts) = sample();
        // Escrow modifié : la racine d'état change.
        let mut state =
            vinx_state::State::from_parts(&info.globals, accounts.clone().into_iter().collect())
                .unwrap();
        state.escrow.tranches.pop_back();
        info.globals = state.globals_bytes();
        assert!(assemble(&info, accounts).is_err());
    }
}
