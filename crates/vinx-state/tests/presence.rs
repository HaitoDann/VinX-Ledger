// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

//! Protocole 7, lot P7b : `last_commit`, présence, récompenses, mise à
//! l'écart, retour et départ (docs/PROTOCOLE-7.md §§5–6) ; lot P7c : sanction
//! de la double signature (§4).

mod common;

use common::*;
use vinx_core::{
    AdminAction, Block, BlockHeader, CommitCertificate, SignedTransaction, TxBody, Vote, VoteKind,
    block_tx_root,
};
use vinx_crypto::{Address, KeyPair, PublicKey};
use vinx_state::staking::{DEFAULT_VALIDATOR_BOND, StakingError, ValidatorStatus};
use vinx_state::{BlockError, TxError};

/// Réseau au protocole 7 (vraie mise à jour), validateurs financés avant.
fn net7() -> Net {
    let mut net = Net::current();
    let keys: Vec<PublicKey> = net.validators.iter().map(KeyPair::public_key).collect();
    for key in keys {
        let pay = net.admin(AdminAction::TreasuryTransfer {
            to: Address::from_public_key(&key),
            amount: 50_000 * VINX,
            memo: None,
        });
        net.block(T0, &[pay], &[]).unwrap();
    }
    let h = net.state.height;
    let up = net.admin(AdminAction::ScheduleUpgrade {
        version: 7,
        activation_height: h + 1 + 17_280,
    });
    net.block(T0, &[up], &[]).unwrap();
    net.state.height = h + 17_280;
    net.block(T0, &[], &[]).unwrap();
    assert_eq!(net.state.protocol_version, 7);
    net
}

/// Clé du validateur `i` de la genèse.
fn vkey(net: &Net, i: usize) -> PublicKey {
    net.validators[i].public_key()
}

/// Certificat du dernier bloc, signé par les validateurs (de la genèse) `who`.
fn cert(net: &Net, who: &[usize]) -> CommitCertificate {
    let h = net.state.height;
    let set = net.state.validators_at(h);
    let hash = [h as u8; 32];
    let vote = Vote {
        chain_id: CHAIN.into(),
        kind: VoteKind::Precommit,
        height: h,
        round: 0,
        block_hash: Some(hash),
    };
    let mut signatures: Vec<(u16, vinx_crypto::Signature)> = who
        .iter()
        .filter_map(|&g| {
            let kp = &net.validators[g];
            let i = set.iter().position(|k| *k == kp.public_key())?;
            Some((i as u16, vote.sign(kp)))
        })
        .collect();
    signatures.sort_by_key(|(i, _)| *i);
    CommitCertificate {
        height: h,
        round: 0,
        block_hash: hash,
        signatures,
    }
}

/// Bloc suivant, certificat signé par `who`, proposeur tournant.
fn step(
    net: &mut Net,
    at: u64,
    who: &[usize],
    txs: &[SignedTransaction],
) -> Result<(), BlockError> {
    let c = cert(net, who);
    let h = net.state.height + 1;
    let n = net.state.validators_at(h).len() as u64;
    let next = net
        .state
        .execute_block(h, at * 1000, Some((h % n) as u16), Some(&c), &[], txs)?;
    net.state = next;
    Ok(())
}

fn vtx(net: &Net, g: usize, body: TxBody) -> SignedTransaction {
    let kp = &net.validators[g];
    let a = addr(kp);
    let account = net.state.account(&a);
    let nonce = account.map_or(0, |a| a.nonce) + 1;
    let first = account.is_none_or(|a| a.key.is_none());
    user_tx(kp, nonce, body, first)
}

fn tx_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidTx { error, .. }) => error,
        other => panic!("attendu une transaction refusée, obtenu {other:?}"),
    }
}

fn status(net: &Net, g: usize) -> ValidatorStatus {
    net.state.staking.records.get(&vkey(net, g)).unwrap().status
}

#[test]
fn presence_is_counted_from_last_commit() {
    let mut net = net7();
    step(&mut net, T0, &[0, 1, 2, 3], &[]).unwrap();
    step(&mut net, T0, &[0, 1, 2], &[]).unwrap();
    let p = &net.state.staking.presence;
    assert_eq!(p[&vkey(&net, 0)].signed, 2);
    assert_eq!(p[&vkey(&net, 3)].signed, 1);
    assert_eq!(p[&vkey(&net, 3)].expected, 2);
    assert_eq!(p[&vkey(&net, 3)].missed_in_row, 1);
    assert!(net.state.check_invariant().is_ok());

    // Sous le quorum, ou pour une autre hauteur : refusé.
    assert_eq!(
        step(&mut net, T0, &[0, 1], &[]),
        Err(BlockError::BadLastCommit)
    );
    let mut c = cert(&net, &[0, 1, 2]);
    c.height -= 1;
    let h = net.state.height + 1;
    assert_eq!(
        net.state
            .execute_block(h, T0 * 1000, Some(0), Some(&c), &[], &[])
            .map(|_| ()),
        Err(BlockError::BadLastCommit)
    );
}

#[test]
fn an_absent_validator_is_jailed_then_comes_back() {
    let mut net = net7();
    let key3 = vkey(&net, 3);
    for _ in 0..999 {
        step(&mut net, T0, &[0, 1, 2], &[]).unwrap();
    }
    assert_eq!(status(&net, 3), ValidatorStatus::Active);
    step(&mut net, T0, &[0, 1, 2], &[]).unwrap();
    let jailed_at = net.state.height;
    assert_eq!(
        status(&net, 3),
        ValidatorStatus::Jailed { since: jailed_at }
    );
    // Retiré à H + 2 ; le quorum se recalcule sur les 3 restants.
    assert!(net.state.validators_at(jailed_at + 1).contains(&key3));
    assert!(!net.state.validators_at(jailed_at + 2).contains(&key3));
    step(&mut net, T0, &[0, 1, 2], &[]).unwrap();
    step(&mut net, T0, &[0, 1, 2], &[]).unwrap();
    assert_eq!(net.state.validators.active.len(), 3);

    // Trop tôt.
    let unjail = vtx(&net, 3, TxBody::Unjail);
    assert_eq!(
        tx_err(step(&mut net, T0, &[0, 1, 2], &[unjail])),
        TxError::Staking(StakingError::JailTooRecent)
    );
    while net.state.height + 1 < jailed_at + 1_000 {
        step(&mut net, T0, &[0, 1, 2], &[]).unwrap();
    }
    // Délai écoulé, mais sans caution (la grâce ne vaut pas pour le retour).
    let unjail = vtx(&net, 3, TxBody::Unjail);
    assert_eq!(
        tx_err(step(&mut net, T0, &[0, 1, 2], &[unjail])),
        TxError::Staking(StakingError::BondTooLow)
    );
    let bond = vtx(
        &net,
        3,
        TxBody::DepositBond {
            key: key3.clone(),
            amount: DEFAULT_VALIDATOR_BOND,
        },
    );
    step(&mut net, T0, &[0, 1, 2], &[bond]).unwrap();
    let unjail = vtx(&net, 3, TxBody::Unjail);
    step(&mut net, T0, &[0, 1, 2], &[unjail]).unwrap();
    let back = net.state.height;
    assert_eq!(status(&net, 3), ValidatorStatus::Active);
    assert_eq!(net.state.staking.presence[&key3].missed_in_row, 0);
    assert!(net.state.validators_at(back + 2).contains(&key3));
    assert!(net.state.check_invariant().is_ok());
}

#[test]
fn rewards_go_to_available_validators_only() {
    let mut net = net7();
    net.state.emission.epoch_blocks = 100;
    let start = net.state.height;
    let t = T0 + 86_400;
    let before: Vec<u64> = (0..4)
        .map(|g| net.state.balance(&addr(&net.validators[g])))
        .collect();
    // Le validateur 3 signe un bloc sur deux : 50 % < 97 %.
    let mut h = start;
    while (h + 1) % 100 != 0 {
        let who: &[usize] = if h % 2 == 0 {
            &[0, 1, 2, 3]
        } else {
            &[0, 1, 2]
        };
        step(&mut net, t, who, &[]).unwrap();
        h += 1;
    }
    step(&mut net, t, &[0, 1, 2], &[]).unwrap();
    let gain: Vec<u64> = (0..4)
        .map(|g| net.state.balance(&addr(&net.validators[g])) - before[g])
        .collect();
    assert_eq!(gain[3], 0, "non éligible : rien");
    assert!(gain[0] > 0 && gain[1] > 0 && gain[2] > 0, "{gain:?}");
    // Compteurs remis à zéro pour l'époque suivante.
    assert_eq!(net.state.staking.presence[&vkey(&net, 0)].expected, 0);
    assert!(net.state.check_invariant().is_ok());
}

#[test]
fn without_bond_after_grace_there_is_no_reward() {
    let mut net = net7();
    net.state.emission.epoch_blocks = 50;
    let after_grace = T0 + vinx_state::staking::BOND_GRACE + 1;
    let bond = vtx(
        &net,
        0,
        TxBody::DepositBond {
            key: vkey(&net, 0),
            amount: DEFAULT_VALIDATOR_BOND,
        },
    );
    step(&mut net, after_grace, &[0, 1, 2, 3], &[bond]).unwrap();
    let before: Vec<u64> = (0..4)
        .map(|g| net.state.balance(&addr(&net.validators[g])))
        .collect();
    loop {
        step(&mut net, after_grace, &[0, 1, 2, 3], &[]).unwrap();
        if net.state.height % 50 == 0 {
            break;
        }
    }
    let gain: Vec<u64> = (0..4)
        .map(|g| net.state.balance(&addr(&net.validators[g])) - before[g])
        .collect();
    assert!(
        gain[0] > 0,
        "seul le validateur cautionné est payé : {gain:?}"
    );
    assert_eq!(&gain[1..], &[0, 0, 0]);
    assert!(net.state.check_invariant().is_ok());
}

#[test]
fn leaving_then_withdrawing_the_bond() {
    let mut net = net7();
    let key1 = vkey(&net, 1);
    let bond = vtx(
        &net,
        1,
        TxBody::DepositBond {
            key: key1.clone(),
            amount: DEFAULT_VALIDATOR_BOND,
        },
    );
    step(&mut net, T0, &[0, 1, 2, 3], &[bond]).unwrap();
    let leave = vtx(&net, 1, TxBody::LeaveValidators);
    step(&mut net, T0, &[0, 1, 2, 3], &[leave]).unwrap();
    let h = net.state.height;
    let unlock_at = T0 + net.state.staking.params.bond_unlock_delay;
    assert_eq!(status(&net, 1), ValidatorStatus::Leaving { unlock_at });
    assert!(!net.state.validators_at(h + 2).contains(&key1));
    let again = vtx(&net, 1, TxBody::LeaveValidators);
    assert_eq!(
        tx_err(step(&mut net, T0, &[0, 1, 2, 3], &[again])),
        TxError::Staking(StakingError::AlreadyLeft)
    );
    let early = vtx(&net, 1, TxBody::WithdrawBond);
    assert_eq!(
        tx_err(step(&mut net, T0, &[0, 1, 2, 3], &[early])),
        TxError::Staking(StakingError::BondLocked)
    );
    let balance = net.state.balance(&addr(&net.validators[1]));
    let w = vtx(&net, 1, TxBody::WithdrawBond);
    step(&mut net, unlock_at, &[0, 2, 3], &[w]).unwrap();
    assert_eq!(status(&net, 1), ValidatorStatus::Left);
    assert_eq!(net.state.staking.bond(&key1), 0);
    assert_eq!(
        net.state.balance(&addr(&net.validators[1])),
        balance + DEFAULT_VALIDATOR_BOND - VINX / 1000
    );
    assert!(net.state.check_invariant().is_ok());
}

#[test]
fn removing_a_jailed_validator_only_marks_it_leaving() {
    let mut net = net7();
    for _ in 0..1_000 {
        step(&mut net, T0, &[0, 1, 2], &[]).unwrap();
    }
    assert!(matches!(status(&net, 3), ValidatorStatus::Jailed { .. }));
    step(&mut net, T0, &[0, 1, 2], &[]).unwrap();
    step(&mut net, T0, &[0, 1, 2], &[]).unwrap();
    let rm = net.admin(AdminAction::RemoveValidator { key: vkey(&net, 3) });
    net.block(T0, &[rm], &[]).unwrap();
    // Exécutée après le délai admin.
    net.block(T0 + 3 * 86_400, &[], &[]).unwrap();
    assert!(matches!(status(&net, 3), ValidatorStatus::Leaving { .. }));
    assert_eq!(net.state.validators.active.len(), 3);
}

#[test]
fn apply_block_requires_the_parent_certificate() {
    let mut net = net7();
    step(&mut net, T0, &[0, 1, 2, 3], &[]).unwrap();
    let c = cert(&net, &[0, 1, 2]);
    let h = net.state.height + 1;
    let next = net
        .state
        .execute_block(h, T0 * 1000, Some(0), Some(&c), &[], &[])
        .unwrap();
    let block = Block {
        header: BlockHeader {
            protocol_version: 7,
            chain_id: CHAIN.into(),
            height: h,
            timestamp: T0 * 1000,
            prev_hash: c.block_hash,
            tx_root: block_tx_root(7, &[], &[], Some(&c)),
            state_root: next.state_root(),
            validators_hash: next.next_validators_hash(),
            proposer: 0,
        },
        admin_txs: vec![],
        txs: vec![],
        last_commit: Some(c.clone()),
    };
    let mut s = net.state.clone();
    s.apply_block(&block).unwrap();
    assert_eq!(s, next);

    // Sans certificat.
    let mut none = block.clone();
    none.last_commit = None;
    none.header.tx_root = none.compute_tx_root();
    assert_eq!(
        net.state.clone().apply_block(&none),
        Err(BlockError::BadLastCommit)
    );
    // Certificat d'un autre bloc que le parent.
    let mut other = block.clone();
    other.header.prev_hash = [1; 32];
    assert_eq!(
        net.state.clone().apply_block(&other),
        Err(BlockError::BadLastCommit)
    );
    // Certificat modifié : tx_root ne correspond plus.
    let mut swapped = block.clone();
    swapped.last_commit = Some(cert(&net, &[0, 1, 2, 3]));
    assert_eq!(
        net.state.clone().apply_block(&swapped),
        Err(BlockError::TxRootMismatch)
    );
    // L'encodage relit le certificat.
    let bytes = vinx_core::encode(&block);
    assert_eq!(vinx_core::decode::<Block>(&bytes).unwrap(), block);
}

// ── P7c : sanction de la double signature (§4) ──────────────────────────────

/// Preuve : deux precommits contradictoires du validateur `g` à la hauteur `h`.
fn double_vote(net: &Net, g: usize, h: u64) -> Vec<u8> {
    use vinx_consensus::{Evidence, SignedVote};
    let vote = |hash: [u8; 32]| Vote {
        chain_id: CHAIN.into(),
        kind: VoteKind::Precommit,
        height: h,
        round: 0,
        block_hash: Some(hash),
    };
    let kp = &net.validators[g];
    let ev = Evidence::Votes(
        SignedVote::new(vote([1; 32]), g as u16, kp),
        SignedVote::new(vote([2; 32]), g as u16, kp),
    );
    vinx_core::encode(&ev)
}

fn report(net: &Net, reporter: usize, g: usize, evidence: Vec<u8>) -> SignedTransaction {
    vtx(
        net,
        reporter,
        TxBody::ReportEquivocation {
            key: vkey(net, g),
            evidence,
        },
    )
}

#[test]
fn equivocation_confiscates_the_bond_and_bans() {
    let mut net = net7();
    let key2 = vkey(&net, 2);
    let bond = vtx(
        &net,
        2,
        TxBody::DepositBond {
            key: key2.clone(),
            amount: DEFAULT_VALIDATOR_BOND + 1,
        },
    );
    step(&mut net, T0, &[0, 1, 2, 3], &[bond]).unwrap();
    let h = net.state.height;
    let before: Vec<u64> = (0..4)
        .map(|g| net.state.balance(&addr(&net.validators[g])))
        .collect();
    let pot = net.state.emission.pot;

    // Preuve d'un autre validateur, ou falsifiée : refusée.
    let wrong = vtx(
        &net,
        0,
        TxBody::ReportEquivocation {
            key: vkey(&net, 1),
            evidence: double_vote(&net, 2, h),
        },
    );
    assert_eq!(
        tx_err(step(&mut net, T0, &[0, 1, 2, 3], &[wrong])),
        TxError::Staking(StakingError::BadEvidence)
    );
    let garbage = report(&net, 0, 2, vec![1, 2, 3]);
    assert_eq!(
        tx_err(step(&mut net, T0, &[0, 1, 2, 3], &[garbage])),
        TxError::Staking(StakingError::BadEvidence)
    );

    let evidence = double_vote(&net, 2, h);
    let tx = report(&net, 0, 2, evidence.clone());
    step(&mut net, T0, &[0, 1, 2, 3], &[tx]).unwrap();
    let at = net.state.height;
    assert_eq!(status(&net, 2), ValidatorStatus::Banned);
    assert_eq!(net.state.staking.bond(&key2), 0);
    assert!(!net.state.validators_at(at + 2).contains(&key2));
    // 10 000 VINX + 1 unité, en trois parts égales ; le reliquat au pot.
    let share = (DEFAULT_VALIDATOR_BOND + 1) / 3;
    let fee = VINX / 1000;
    let after: Vec<u64> = (0..4)
        .map(|g| net.state.balance(&addr(&net.validators[g])))
        .collect();
    assert_eq!(
        after[0],
        before[0] + share - fee,
        "le signaleur paie les frais"
    );
    assert_eq!(after[1], before[1] + share);
    assert_eq!(after[3], before[3] + share);
    assert_eq!(after[2], before[2]);
    assert_eq!(
        net.state.emission.pot,
        pot + fee + (DEFAULT_VALIDATOR_BOND + 1 - 3 * share)
    );
    assert!(net.state.check_invariant().is_ok());

    // Une seule sanction par faute, et plus aucun retour possible.
    let again = report(&net, 1, 2, evidence);
    assert_eq!(
        tx_err(step(&mut net, T0, &[0, 1, 3], &[again])),
        TxError::Staking(StakingError::AlreadySlashed)
    );
    let bond = vtx(
        &net,
        2,
        TxBody::DepositBond {
            key: key2.clone(),
            amount: VINX,
        },
    );
    assert_eq!(
        tx_err(step(&mut net, T0, &[0, 1, 3], &[bond])),
        TxError::Staking(StakingError::Banned)
    );
}

#[test]
fn equivocation_is_punished_during_the_unlock_delay() {
    let mut net = net7();
    let key1 = vkey(&net, 1);
    let bond = vtx(
        &net,
        1,
        TxBody::DepositBond {
            key: key1.clone(),
            amount: DEFAULT_VALIDATOR_BOND,
        },
    );
    step(&mut net, T0, &[0, 1, 2, 3], &[bond]).unwrap();
    let h = net.state.height;
    let leave = vtx(&net, 1, TxBody::LeaveValidators);
    step(&mut net, T0, &[0, 1, 2, 3], &[leave]).unwrap();
    step(&mut net, T0, &[0, 2, 3], &[]).unwrap();
    step(&mut net, T0, &[0, 2, 3], &[]).unwrap();
    assert!(matches!(status(&net, 1), ValidatorStatus::Leaving { .. }));
    let tx = report(&net, 3, 1, double_vote(&net, 1, h));
    step(&mut net, T0, &[0, 2, 3], &[tx]).unwrap();
    assert_eq!(status(&net, 1), ValidatorStatus::Banned);
    assert_eq!(net.state.staking.bond(&key1), 0);
    assert_eq!(net.state.staking.slashed.len(), 1);
    // La caution n'est plus restituable.
    let w = vtx(&net, 1, TxBody::WithdrawBond);
    let unlock = T0 + net.state.staking.params.bond_unlock_delay;
    assert_eq!(
        tx_err(step(&mut net, unlock, &[0, 2, 3], &[w])),
        TxError::Staking(StakingError::NoBond)
    );
    assert!(net.state.check_invariant().is_ok());
}

#[test]
fn evidence_before_the_validator_joined_is_refused() {
    let mut net = net7();
    let joined = net
        .state
        .staking
        .records
        .get(&vkey(&net, 3))
        .unwrap()
        .joined_at;
    let tx = report(&net, 0, 3, double_vote(&net, 3, joined - 1));
    assert_eq!(
        tx_err(step(&mut net, T0, &[0, 1, 2, 3], &[tx])),
        TxError::Staking(StakingError::EvidenceOutOfRange)
    );
}
