// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Protocole 8 : réserve du protocole et gouvernance votée
//! (docs/PROTOCOLE-8.md).

mod common;

use common::*;
use vinx_core::{
    AdminAction, AdminTransaction, ParamId, SignedAdminTransaction, SignedTransaction, TxBody,
};
use vinx_crypto::{Address, Hash, KeyPair};
use vinx_state::governance::{
    GENESIS_RESERVE, GovernanceError, HOLDER_VOTE_PERIOD, MONTH, MONTHLY_CAP, Phase,
    VALIDATOR_VOTE_PERIOD,
};
use vinx_state::{BlockError, State, TxError};

const DAY: u64 = 86_400;
const DEPOSIT: u64 = 1_000 * VINX;

fn tx_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidTx { error, .. }) => error,
        other => panic!("attendu une transaction refusée, obtenu {other:?}"),
    }
}

fn admin_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidAdminTx { error, .. }) => error,
        other => panic!("attendu une action admin refusée, obtenu {other:?}"),
    }
}

fn gov(e: GovernanceError) -> TxError {
    TxError::Governance(e)
}

fn signed(net: &Net, kp: &KeyPair, body: TxBody) -> SignedTransaction {
    let a = net.state.account(&addr(kp));
    let nonce = a.map_or(0, |a| a.nonce) + 1;
    let first = a.is_none_or(|a| a.key.is_none());
    user_tx(kp, nonce, body, first)
}

fn send(net: &mut Net, kp: &KeyPair, body: TxBody, at: u64) -> Result<(), BlockError> {
    let tx = signed(net, kp, body);
    net.block(at, &[], &[tx])
}

/// Réseau réserve, validateurs et `extra` comptes financés.
fn setup(extra: &[(&KeyPair, u64)]) -> Net {
    let mut net = Net::reserve();
    for i in 0..4 {
        let a = addr(&net.validators[i]);
        net.give(a, 10 * VINX);
    }
    for (kp, amount) in extra {
        net.give(addr(kp), *amount);
    }
    net
}

fn request(net: &mut Net, kp: &KeyPair, to: Address, amount: u64, at: u64) -> Hash {
    let tx = signed(
        net,
        kp,
        TxBody::SubmitSpend {
            beneficiary: to,
            amount,
            title: "Audit de sécurité".into(),
            url: "https://vinxlabs.fr/audit".into(),
        },
    );
    let id = tx.id();
    net.block(at, &[], &[tx]).unwrap();
    id
}

fn validators_vote(net: &mut Net, id: Hash, who: &[usize], approve: bool, at: u64) {
    let txs: Vec<_> = who
        .iter()
        .map(|&i| {
            let kp = &net.validators[i];
            signed(net, kp, TxBody::VoteValidator { id, approve })
        })
        .collect();
    net.block(at, &[], &txs).unwrap();
}

fn phase(net: &Net, id: &Hash) -> Phase {
    net.state.governance.proposals.get(id).unwrap().phase
}

#[test]
fn a_spend_goes_through_both_votes_and_is_paid() {
    let alice = KeyPair::generate();
    let bob = KeyPair::generate();
    let holder = KeyPair::generate();
    let mut net = setup(&[(&alice, 2_000 * VINX), (&holder, 300_000 * VINX)]);
    assert_eq!(net.state.protocol_version, 8);
    let t = T0 + 10;
    let id = request(&mut net, &alice, addr(&bob), 50_000 * VINX, t);
    let fee = VINX / 1000;
    assert_eq!(
        net.state.balance(&addr(&alice)),
        2_000 * VINX - DEPOSIT - fee
    );
    assert!(
        net.state.check_invariant().is_ok(),
        "caution dans l'invariant"
    );

    // 2 validateurs sur 4 : pas assez.
    validators_vote(&mut net, id, &[0, 1], true, t);
    net.block(t, &[], &[]).unwrap();
    assert!(matches!(phase(&net, &id), Phase::Validators { .. }));
    // Le troisième : plus des 2/3, la caution est rendue, vote des détenteurs.
    validators_vote(&mut net, id, &[2], true, t);
    net.block(t, &[], &[]).unwrap();
    assert!(matches!(phase(&net, &id), Phase::Holders { .. }));
    assert_eq!(net.state.balance(&addr(&alice)), 2_000 * VINX - fee);
    // Un validateur ne vote plus à cette étape.
    let late = signed(
        &net,
        &net.validators[3],
        TxBody::VoteValidator { id, approve: true },
    );
    assert_eq!(
        tx_err(net.block(t, &[], &[late])),
        gov(GovernanceError::WrongPhase)
    );

    // Vote du détenteur : 200 000 VINX bloqués, au-dessus du quorum minimal.
    let vote = |approve, amount| TxBody::VoteHolder {
        id,
        approve,
        amount,
    };
    send(&mut net, &holder, vote(true, 150_000 * VINX), t).unwrap();
    send(&mut net, &holder, vote(true, 50_000 * VINX), t).unwrap();
    assert_eq!(
        tx_err(send(&mut net, &holder, vote(false, VINX), t)),
        gov(GovernanceError::ChangedSide)
    );
    let held = net.state.balance(&addr(&holder));
    assert_eq!(held, 300_000 * VINX - 200_000 * VINX - 2 * fee);
    assert!(
        net.state.check_invariant().is_ok(),
        "votes dans l'invariant"
    );

    // Clôture : VINX rendus, demande acceptée et versée dans le même bloc.
    let end = t + HOLDER_VOTE_PERIOD;
    net.block(end, &[], &[]).unwrap();
    assert_eq!(net.state.balance(&addr(&holder)), held + 200_000 * VINX);
    assert!(matches!(phase(&net, &id), Phase::Paid { .. }));
    assert_eq!(net.state.balance(&addr(&bob)), 50_000 * VINX);
    let g = &net.state.governance;
    assert_eq!(g.spent_this_month, 50_000 * VINX);
    assert!(g.locks.is_empty() && g.open.is_empty() && g.queue.is_empty());
    assert!(net.state.check_invariant().is_ok());
}

#[test]
fn rejected_by_validators_the_deposit_goes_to_the_reserve() {
    let alice = KeyPair::generate();
    let mut net = setup(&[(&alice, 2_000 * VINX)]);
    let reserve = net.state.governance.reserve;
    let id = request(&mut net, &alice, addr(&alice), VINX, T0);
    validators_vote(&mut net, id, &[0, 1], true, T0);
    validators_vote(&mut net, id, &[2, 3], false, T0);
    net.block(T0 + VALIDATOR_VOTE_PERIOD - 1, &[], &[]).unwrap();
    assert!(matches!(phase(&net, &id), Phase::Validators { .. }));
    net.block(T0 + VALIDATOR_VOTE_PERIOD, &[], &[]).unwrap();
    assert!(matches!(phase(&net, &id), Phase::Rejected { .. }));
    assert_eq!(net.state.governance.reserve, reserve + DEPOSIT);
    assert!(net.state.check_invariant().is_ok());
}

#[test]
fn without_quorum_the_holders_vote_fails() {
    let alice = KeyPair::generate();
    let mut net = setup(&[(&alice, 60_000 * VINX)]);
    let id = request(&mut net, &alice, addr(&alice), VINX, T0);
    validators_vote(&mut net, id, &[0, 1, 2, 3], true, T0);
    net.block(T0, &[], &[]).unwrap();
    // 50 000 VINX < quorum minimal de 100 000.
    let v = TxBody::VoteHolder {
        id,
        approve: true,
        amount: 50_000 * VINX,
    };
    send(&mut net, &alice, v, T0).unwrap();
    net.block(T0 + HOLDER_VOTE_PERIOD, &[], &[]).unwrap();
    assert!(matches!(phase(&net, &id), Phase::Rejected { .. }));
    assert!(net.state.governance.locks.is_empty(), "VINX rendus");
    assert!(net.state.check_invariant().is_ok());
}

#[test]
fn the_monthly_cap_delays_payments() {
    let alice = KeyPair::generate();
    let holder = KeyPair::generate();
    let mut net = setup(&[(&alice, 2_000 * VINX), (&holder, 200_000 * VINX)]);
    // Le mois est presque épuisé.
    net.state.governance.spent_this_month = MONTHLY_CAP - 10_000 * VINX;
    let id = request(&mut net, &alice, addr(&alice), 50_000 * VINX, T0);
    validators_vote(&mut net, id, &[0, 1, 2], true, T0);
    net.block(T0, &[], &[]).unwrap();
    let v = TxBody::VoteHolder {
        id,
        approve: true,
        amount: 150_000 * VINX,
    };
    send(&mut net, &holder, v, T0).unwrap();
    net.block(T0 + HOLDER_VOTE_PERIOD, &[], &[]).unwrap();
    assert_eq!(phase(&net, &id), Phase::Queued);
    assert_eq!(net.state.governance.queue, vec![id]);
    // Mois suivant : versée.
    net.block(T0 + MONTH, &[], &[]).unwrap();
    assert!(matches!(phase(&net, &id), Phase::Paid { .. }));
    assert_eq!(net.state.governance.spent_this_month, 50_000 * VINX);
    assert!(net.state.check_invariant().is_ok());
}

#[test]
fn requests_and_votes_are_checked() {
    let alice = KeyPair::generate();
    let mut net = setup(&[(&alice, 2_000 * VINX)]);
    let body = |amount, title: &str| TxBody::SubmitSpend {
        beneficiary: addr(&alice),
        amount,
        title: title.into(),
        url: String::new(),
    };
    assert_eq!(
        tx_err(send(&mut net, &alice, body(100_001 * VINX, "x"), T0)),
        gov(GovernanceError::BadAmount)
    );
    assert_eq!(
        tx_err(send(&mut net, &alice, body(VINX, ""), T0)),
        gov(GovernanceError::BadText)
    );
    // Un validateur bénéficiaire ne vote pas sur sa demande.
    let v0 = addr(&net.validators[0]);
    let id = request(&mut net, &alice, v0, VINX, T0);
    let own = signed(
        &net,
        &net.validators[0],
        TxBody::VoteValidator { id, approve: true },
    );
    assert_eq!(
        tx_err(net.block(T0, &[], &[own])),
        gov(GovernanceError::Beneficiary)
    );
    // Un compte qui n'est pas validateur non plus.
    assert_eq!(
        tx_err(send(
            &mut net,
            &alice,
            TxBody::VoteValidator { id, approve: true },
            T0
        )),
        gov(GovernanceError::NotValidator)
    );
    // Les 3 autres suffisent : l'électorat est de 3.
    validators_vote(&mut net, id, &[1, 2, 3], true, T0);
    net.block(T0, &[], &[]).unwrap();
    assert!(matches!(phase(&net, &id), Phase::Holders { .. }));
}

#[test]
fn the_admin_proposes_parameters_but_cannot_impose_them() {
    let mut net = setup(&[]);
    // Dépenser ou fixer un paramètre : refusé.
    for action in [
        AdminAction::TreasuryTransfer {
            to: Address::treasury(),
            amount: 1,
            memo: None,
        },
        AdminAction::SetParam {
            param: ParamId::MinFee,
            value: 7,
        },
        AdminAction::EscrowRelease { amount: 1 },
    ] {
        let a = net.admin(action);
        assert_eq!(
            admin_err(net.block(T0, &[a], &[])),
            gov(GovernanceError::EscrowOnly)
        );
        net.admin_nonce -= 1;
    }
    // Paramètre technique : validateurs seuls, appliqué dès leur accord.
    let a = net.admin(AdminAction::ProposeParam {
        param: ParamId::AbsenceBlocks,
        value: 2_000,
    });
    let tech = a.id();
    net.block(T0, &[a], &[]).unwrap();
    validators_vote(&mut net, tech, &[0, 1, 2], true, T0);
    net.block(T0, &[], &[]).unwrap();
    assert!(matches!(phase(&net, &tech), Phase::Applied { .. }));
    assert_eq!(net.state.staking.params.absence_blocks, 2_000);
    // Paramètre économique : il faut aussi les détenteurs.
    let a = net.admin(AdminAction::ProposeParam {
        param: ParamId::MinFee,
        value: 2_000_000,
    });
    let eco = a.id();
    net.block(T0, &[a], &[]).unwrap();
    validators_vote(&mut net, eco, &[0, 1, 2, 3], true, T0);
    net.block(T0, &[], &[]).unwrap();
    assert!(matches!(phase(&net, &eco), Phase::Holders { .. }));
    net.block(T0 + HOLDER_VOTE_PERIOD, &[], &[]).unwrap();
    assert!(
        matches!(phase(&net, &eco), Phase::Rejected { .. }),
        "sans quorum"
    );
    assert_eq!(
        net.state.params.min_fee,
        vinx_core::Params::default().min_fee
    );
    // Valeur invalide : refusée dès la proposition.
    let bad = net.admin(AdminAction::ProposeParam {
        param: ParamId::MinAvailability,
        value: 20_000,
    });
    assert_eq!(admin_err(net.block(T0, &[bad], &[])), TxError::InvalidParam);
}

#[test]
fn an_emergency_upgrade_needs_all_three_signatures() {
    let mut net = setup(&[]);
    let h = net.state.height;
    let upgrade = |net: &mut Net, signers: &[usize]| {
        net.admin_nonce += 1;
        let tx = AdminTransaction {
            chain_id: CHAIN.into(),
            nonce: net.admin_nonce,
            action: AdminAction::ScheduleUpgrade {
                version: 9,
                activation_height: h + 1 + vinx_state::state::EMERGENCY_UPGRADE_DELAY,
            },
        };
        SignedAdminTransaction {
            approvals: signers
                .iter()
                .map(|&i| tx.approve(i as u8, &net.admins[i]))
                .collect(),
            tx,
        }
    };
    let two = upgrade(&mut net, &[0, 1]);
    assert_eq!(
        admin_err(net.block(T0, &[two], &[])),
        TxError::InvalidUpgrade
    );
    net.admin_nonce -= 1;
    let three = upgrade(&mut net, &[0, 1, 2]);
    net.block(T0, &[three], &[]).unwrap();
    assert_eq!(net.state.upgrade.as_ref().unwrap().version, 9);
}

#[test]
fn an_escrow_chain_has_no_reserve() {
    let alice = KeyPair::generate();
    let mut net = Net::new();
    let pay = net.admin(AdminAction::TreasuryTransfer {
        to: addr(&alice),
        amount: 10_000 * VINX,
        memo: None,
    });
    net.block(T0, &[pay], &[]).unwrap();
    net.state.protocol_version = 8;
    let r = send(
        &mut net,
        &alice,
        TxBody::SubmitSpend {
            beneficiary: addr(&alice),
            amount: VINX,
            title: "x".into(),
            url: String::new(),
        },
        T0,
    );
    assert_eq!(tx_err(r), gov(GovernanceError::NoReserve));
}

#[test]
fn reserve_genesis_and_rows_reload() {
    let alice = KeyPair::generate();
    let holder = KeyPair::generate();
    let mut net = setup(&[(&alice, 2_000 * VINX), (&holder, 200_000 * VINX)]);
    let g = &net.state.governance;
    assert!(g.enabled);
    assert!(net.state.escrow.tranches.is_empty());
    assert!(g.reserve < GENESIS_RESERVE);
    let id = request(&mut net, &alice, addr(&alice), VINX, T0);
    validators_vote(&mut net, id, &[0, 1, 2], true, T0);
    net.block(T0, &[], &[]).unwrap();
    let v = TxBody::VoteHolder {
        id,
        approve: false,
        amount: VINX,
    };
    send(&mut net, &holder, v, T0 + DAY).unwrap();
    let root = net.state.state_root();
    let mut state = net.state.clone();
    state.mark_all_rows_dirty();
    let rows = state.take_dirty_rows();
    let mut reloaded =
        State::from_parts(&state.globals_bytes_light(), state.accounts.clone()).unwrap();
    for (k, v) in rows {
        if let Some(v) = v {
            reloaded.load_row(&k, &v).unwrap();
        }
    }
    assert_eq!(reloaded.state_root(), root);
    assert_eq!(reloaded.governance.locks.len(), 1);
    assert!(reloaded.check_invariant().is_ok());
}

/// Déroulé de la pré-production (docs/PROTOCOLE-8.md §8), en conditions
/// réelles : un seul validateur, pas de robinet ni de premine, caution exigée
/// de 1 000 VINX ; il finance les trois suivants avec son émission.
#[test]
fn preprod_bootstrap_from_one_validator() {
    use vinx_core::ValidatorIdentity;
    use vinx_state::Genesis;
    let admins = [
        KeyPair::generate(),
        KeyPair::generate(),
        KeyPair::generate(),
    ];
    let vals: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
    let mut genesis = Genesis::standard(
        CHAIN,
        T0,
        [
            admins[0].public_key(),
            admins[1].public_key(),
            admins[2].public_key(),
        ],
        vec![vals[0].public_key()],
    )
    .unwrap()
    .with_reserve()
    .unwrap();
    genesis.required_bond = Some(1_000 * VINX);
    genesis.identities = vec![ValidatorIdentity {
        name: "Cinnamon".into(),
        operator: "VinX Labs".into(),
        country: *b"FR",
    }];
    let mut net = Net {
        admins,
        validators: vals,
        state: genesis.into_state().unwrap(),
        admin_nonce: 0,
    };
    let cinnamon = addr(&net.validators[0]);
    assert_eq!(net.state.balance(&cinnamon), 0, "aucune part réservée");

    // Un jour de blocs (époques de 17 280) : toute l'émission va à Cinnamon.
    let day_end = T0 + DAY;
    net.state.height = 17_279;
    let vote = vinx_core::Vote {
        chain_id: CHAIN.into(),
        kind: vinx_core::VoteKind::Precommit,
        height: 17_279,
        round: 0,
        block_hash: Some([7; 32]),
    };
    let cert = vinx_core::CommitCertificate {
        height: 17_279,
        round: 0,
        block_hash: [7; 32],
        signatures: vec![(0, vote.sign(&net.validators[0]))],
    };
    net.state = net
        .state
        .execute_block(17_280, day_end * 1000, Some(0), Some(&cert), &[], &[], &[])
        .unwrap();
    let earned = net.state.balance(&cinnamon);
    assert!(
        earned > 15_000 * VINX,
        "émission du premier jour : {earned}"
    );

    // Caution de Cinnamon, puis financement des trois suivants.
    let mut txs = vec![signed(
        &net,
        &net.validators[0],
        TxBody::DepositBond {
            key: net.validators[0].public_key(),
            amount: 1_000 * VINX,
        },
    )];
    for i in 1..4 {
        let mut t = signed(
            &net,
            &net.validators[0],
            TxBody::Transfer {
                to: addr(&net.validators[i]),
                amount: 1_100 * VINX,
                memo: None,
            },
        );
        t.tx.nonce += i as u64;
        txs.push(t.tx.sign(&net.validators[0], false));
    }
    net.block(day_end, &[], &txs).unwrap();
    let names = ["Amber", "Obsidian", "Pearl"];
    let mut bonds = Vec::new();
    for i in 1..4 {
        bonds.push(signed(
            &net,
            &net.validators[i],
            TxBody::DepositBond {
                key: net.validators[i].public_key(),
                amount: 1_000 * VINX,
            },
        ));
    }
    net.block(day_end, &[], &bonds).unwrap();
    let adds: Vec<_> = (1..4)
        .map(|i| {
            let key = net.validators[i].public_key();
            net.admin(AdminAction::AddNamedValidator {
                key,
                identity: ValidatorIdentity {
                    name: names[i - 1].into(),
                    operator: "VinX Labs".into(),
                    country: *b"FR",
                },
            })
        })
        .collect();
    net.block(day_end, &adds, &[]).unwrap();
    assert_eq!(net.state.staking.pending.len(), 3, "délai admin de 48 h");
    net.block(day_end + 2 * DAY, &[], &[]).unwrap();
    net.block(day_end + 2 * DAY, &[], &[]).unwrap();
    net.block(day_end + 2 * DAY, &[], &[]).unwrap();
    assert_eq!(
        net.state.validators.active.len(),
        4,
        "les trois entrent ensemble"
    );
    assert_eq!(
        net.state.governance.reserve, GENESIS_RESERVE,
        "réserve intacte"
    );
    assert!(net.state.check_invariant().is_ok());
}
