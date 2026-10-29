//! Fuzzing des décodeurs : toute entrée venue du réseau ou de l'API est décodée
//! sans jamais faire planter le nœud, et seul l'encodage canonique est accepté.

// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use borsh::{BorshDeserialize, BorshSerialize};
use proptest::prelude::*;
use vinx_consensus::{Proposal, SignedVote};
use vinx_core::Account;
use vinx_core::{
    AdminAction, AdminTransaction, Block, BlockHeader, CommitCertificate, SignedAdminTransaction,
    SignedTransaction, Transaction, TxBody, Vote, VoteKind,
};
use vinx_crypto::{Address, KeyPair};
use vinx_node::network::{NetMessage, SyncRequest, SyncResponse};
use vinx_node::snapshot::SnapshotInfo;
use vinx_state::Genesis;

/// Décode `bytes` ; en cas de succès, l'encodage doit redonner exactement `bytes`.
fn check<T: BorshDeserialize + BorshSerialize>(bytes: &[u8]) -> Result<(), TestCaseError> {
    if let Ok(value) = borsh::from_slice::<T>(bytes) {
        prop_assert_eq!(
            borsh::to_vec(&value).unwrap(),
            bytes,
            "encodage non canonique accepté"
        );
    }
    Ok(())
}

fn check_all(bytes: &[u8]) -> Result<(), TestCaseError> {
    check::<SignedTransaction>(bytes)?;
    check::<SignedAdminTransaction>(bytes)?;
    check::<Block>(bytes)?;
    check::<CommitCertificate>(bytes)?;
    check::<NetMessage>(bytes)?;
    check::<SyncRequest>(bytes)?;
    check::<SyncResponse>(bytes)?;
    check::<Vote>(bytes)
}

/// Encodages valides de chaque type, à muter.
fn samples() -> Vec<Vec<u8>> {
    let kp = KeyPair::from_secret_bytes(&[7; 32]);
    let tx = Transaction {
        chain_id: "vinx-fuzz".into(),
        sender: Address::from_public_key(&kp.public_key()),
        nonce: 1,
        fee: 1_000_000,
        body: TxBody::Transfer {
            to: Address::treasury(),
            amount: 5,
            memo: Some(b"m".to_vec()),
        },
    }
    .sign(&kp, true);
    let admin = AdminTransaction {
        chain_id: "vinx-fuzz".into(),
        nonce: 1,
        action: AdminAction::ReplaceSigner {
            index: 1,
            new_key: kp.public_key(),
        },
    };
    let admin = SignedAdminTransaction {
        approvals: vec![admin.approve(0, &kp)],
        tx: admin,
    };
    let block = Block {
        header: BlockHeader {
            protocol_version: 1,
            chain_id: "vinx-fuzz".into(),
            height: 3,
            timestamp: 9,
            prev_hash: [1; 32],
            tx_root: [2; 32],
            state_root: [3; 32],
            validators_hash: [4; 32],
            proposer: 0,
        },
        admin_txs: vec![admin.clone()],
        txs: vec![tx.clone()],
    };
    let vote = Vote {
        chain_id: "vinx-fuzz".into(),
        kind: VoteKind::Precommit,
        height: 3,
        round: 1,
        block_hash: Some(block.hash()),
    };
    let cert = CommitCertificate {
        height: 3,
        round: 1,
        block_hash: block.hash(),
        signatures: vec![(0, vote.sign(&kp))],
    };
    let proposal = Proposal {
        chain_id: "vinx-fuzz".into(),
        height: 3,
        round: 1,
        pol_round: Some(0),
        block: block.clone(),
    }
    .sign(&kp);
    vec![
        borsh::to_vec(&tx).unwrap(),
        borsh::to_vec(&admin).unwrap(),
        borsh::to_vec(&block).unwrap(),
        borsh::to_vec(&cert).unwrap(),
        borsh::to_vec(&NetMessage::Proposal(proposal)).unwrap(),
        borsh::to_vec(&NetMessage::Vote(SignedVote::new(vote, 0, &kp))).unwrap(),
        borsh::to_vec(&NetMessage::Tx(tx.clone())).unwrap(),
        borsh::to_vec(&NetMessage::Txs(vec![tx.clone(), tx])).unwrap(),
        borsh::to_vec(&SyncResponse::Blocks {
            lowest: 1,
            blocks: vec![(block.clone(), cert.clone())],
        })
        .unwrap(),
        borsh::to_vec(&SyncResponse::SnapshotInfo(Some(Box::new(SnapshotInfo {
            block,
            cert,
            globals: vec![1, 2, 3],
            accounts: 2,
        }))))
        .unwrap(),
        borsh::to_vec(&SyncResponse::SnapshotChunk {
            height: 3,
            chunk: Some(vec![(Address::treasury(), Account::default())]),
            done: true,
        })
        .unwrap(),
    ]
}

#[derive(Debug, Clone)]
enum Mutation {
    Flip(usize, u8),
    Truncate(usize),
    Insert(usize, u8),
    Overwrite(usize, [u8; 4]),
}

fn mutate(mut bytes: Vec<u8>, mutations: &[Mutation]) -> Vec<u8> {
    for m in mutations {
        let n = bytes.len().max(1);
        match m {
            Mutation::Flip(i, mask) => {
                let i = i % n;
                if let Some(b) = bytes.get_mut(i) {
                    *b ^= mask | 1;
                }
            }
            Mutation::Truncate(i) => bytes.truncate(i % n),
            Mutation::Insert(i, b) => bytes.insert(i % (bytes.len() + 1), *b),
            // Écrase 4 octets : fabrique notamment des longueurs de vecteur géantes.
            Mutation::Overwrite(i, w) => {
                for (k, v) in w.iter().enumerate() {
                    if let Some(b) = bytes.get_mut((i + k) % n) {
                        *b = *v;
                    }
                }
            }
        }
    }
    bytes
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        (any::<usize>(), any::<u8>()).prop_map(|(i, m)| Mutation::Flip(i, m)),
        any::<usize>().prop_map(Mutation::Truncate),
        (any::<usize>(), any::<u8>()).prop_map(|(i, b)| Mutation::Insert(i, b)),
        (any::<usize>(), any::<[u8; 4]>()).prop_map(|(i, w)| Mutation::Overwrite(i, w)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    #[test]
    fn random_bytes_never_crash(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        check_all(&bytes)?;
    }

    #[test]
    fn mutated_messages_never_crash(
        which in 0usize..10,
        mutations in prop::collection::vec(mutation(), 1..6),
    ) {
        let bytes = mutate(samples()[which].clone(), &mutations);
        check_all(&bytes)?;
    }

    /// Une transaction décodée de n'importe quels octets s'applique ou est
    /// refusée proprement, sans jamais violer l'invariant de supply.
    #[test]
    fn decoded_transactions_never_break_the_state(
        which in 0usize..2,
        mutations in prop::collection::vec(mutation(), 0..4),
    ) {
        let bytes = mutate(samples()[which].clone(), &mutations);
        let k = || KeyPair::generate().public_key();
        let mut state = Genesis::legacy("vinx-fuzz", 1_800_000_000, [k(), k(), k()], vec![k()])
            .unwrap()
            .into_state()
            .unwrap();
        if let Ok(tx) = borsh::from_slice::<SignedTransaction>(&bytes) {
            let _ = state.apply_tx(&tx);
        }
        if let Ok(tx) = borsh::from_slice::<SignedAdminTransaction>(&bytes) {
            let _ = state.execute(1, 1_800_000_000_000, &[tx], &[]);
        }
        prop_assert!(state.check_invariant().is_ok());
    }
}
