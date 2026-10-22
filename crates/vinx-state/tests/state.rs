// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

mod common;

use common::*;
use vinx_core::{AdminAction, AdminTransaction, ParamId, SignedAdminTransaction, TxBody};
use vinx_crypto::{Address, KeyPair};
use vinx_state::{BlockError, TxError, escrow::GENESIS_TRANCHE_AMOUNT};

const FEE: u64 = VINX / 1000;

fn tx_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidTx { error, .. } | BlockError::InvalidAdminTx { error, .. }) => {
            error
        }
        other => panic!("attendu une transaction invalide, obtenu {other:?}"),
    }
}

// ── Transactions utilisateur ────────────────────────────────────────────────

#[test]
fn transfer_moves_funds_and_pays_fee_to_treasury() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let bob = KeyPair::generate();
    let treasury_before = net.state.balance(&Address::treasury());
    let tx = transfer(&alice, 1, addr(&bob), 2 * VINX, true);
    net.block(T0, &[], &[tx]).unwrap();
    assert_eq!(net.state.balance(&addr(&alice)), 8 * VINX - FEE);
    assert_eq!(net.state.balance(&addr(&bob)), 2 * VINX);
    assert_eq!(
        net.state.balance(&Address::treasury()),
        treasury_before + FEE
    );
}

#[test]
fn public_key_required_first_then_forbidden() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let bob = addr(&KeyPair::generate());
    let mut s = net.state.clone();
    assert_eq!(
        s.apply_tx(&transfer(&alice, 1, bob, VINX, false)),
        Err(TxError::KeyMismatch)
    );
    s.apply_tx(&transfer(&alice, 1, bob, VINX, true)).unwrap();
    assert_eq!(
        s.apply_tx(&transfer(&alice, 2, bob, VINX, true)),
        Err(TxError::KeyMismatch)
    );
    s.apply_tx(&transfer(&alice, 2, bob, VINX, false)).unwrap();
}

#[test]
fn key_of_another_address_is_rejected() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let mallory = KeyPair::generate();
    let mut tx = transfer(&mallory, 1, addr(&mallory), VINX, true);
    tx.tx.sender = addr(&alice);
    let tx = tx.tx.sign(&mallory, true);
    assert_eq!(net.state.clone().apply_tx(&tx), Err(TxError::KeyMismatch));
}

#[test]
fn replay_and_nonce_gaps_are_rejected() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let bob = addr(&KeyPair::generate());
    let first = transfer(&alice, 1, bob, VINX, true);
    let mut s = net.state.clone();
    s.apply_tx(&first).unwrap();
    assert_eq!(s.apply_tx(&first), Err(TxError::KeyMismatch)); // clé déjà connue
    let replay = transfer(&alice, 1, bob, VINX, false);
    assert_eq!(s.apply_tx(&replay), Err(TxError::BadNonce));
    assert_eq!(
        s.apply_tx(&transfer(&alice, 3, bob, VINX, false)),
        Err(TxError::BadNonce)
    );
}

#[test]
fn fee_below_minimum_is_rejected() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let mut tx = transfer(&alice, 1, Address::treasury(), VINX, true).tx;
    tx.fee = FEE - 1;
    let tx = tx.sign(&alice, true);
    assert_eq!(net.state.clone().apply_tx(&tx), Err(TxError::FeeTooLow));
}

#[test]
fn reserve_rules() {
    let mut net = Net::new();
    let reserve = net.state.params.min_reserve;
    let alice = net.funded(10 * VINX);
    let mut s = net.state.clone();
    // Nouveau compte : au moins la réserve.
    let new = addr(&KeyPair::generate());
    assert_eq!(
        s.apply_tx(&transfer(&alice, 1, new, reserve - 1, true)),
        Err(TxError::BelowReserve)
    );
    // L'émetteur ne peut pas descendre sous la réserve.
    let all_but = 10 * VINX - FEE - reserve + 1;
    assert_eq!(
        s.apply_tx(&transfer(&alice, 1, new, all_but, true)),
        Err(TxError::BelowReserve)
    );
    s.apply_tx(&transfer(&alice, 1, new, all_but - 1, true))
        .unwrap();
    assert_eq!(s.balance(&addr(&alice)), reserve);
}

#[test]
fn account_can_be_emptied_and_cannot_be_replayed() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let bob = addr(&KeyPair::generate());
    let empty_all = transfer(&alice, 1, bob, 10 * VINX - FEE, true);
    let mut s = net.state.clone();
    s.apply_tx(&empty_all).unwrap();
    assert_eq!(s.balance(&addr(&alice)), 0);
    assert_eq!(s.accounts[&addr(&alice)].nonce, 1);
    // Refinancé, même sous la réserve : ses anciennes transactions restent mortes.
    let carol = net.funded(10 * VINX);
    s.accounts
        .insert(addr(&carol), net.state.accounts[&addr(&carol)].clone());
    let refund = transfer(&carol, 1, addr(&alice), VINX / 100, true);
    s.apply_tx(&refund).unwrap();
    assert_eq!(
        s.apply_tx(&transfer(&alice, 1, bob, 1, false)),
        Err(TxError::BadNonce)
    );
    // Et il peut encore tout envoyer.
    s.apply_tx(&transfer(&alice, 2, bob, VINX / 100 - FEE, false))
        .unwrap();
    assert_eq!(s.balance(&addr(&alice)), 0);
}

#[test]
fn raising_the_reserve_never_locks_funds() {
    let mut net = Net::new();
    let alice = net.funded(VINX);
    let tx = net.admin(AdminAction::SetParam {
        param: ParamId::MinReserve,
        value: 5 * VINX,
    });
    net.block(T0, &[tx], &[]).unwrap();
    let bob = net.funded(10 * VINX);
    let mut s = net.state.clone();
    // Envoyer une partie laisserait Alice entre 0 et la réserve : refusé.
    assert_eq!(
        s.apply_tx(&transfer(&alice, 1, addr(&bob), VINX / 2, true)),
        Err(TxError::BelowReserve)
    );
    // Tout envoyer est autorisé.
    s.apply_tx(&transfer(&alice, 1, addr(&bob), VINX - FEE, true))
        .unwrap();
}

#[test]
fn unknown_sender_and_insufficient_balance() {
    let mut net = Net::new();
    let ghost = KeyPair::generate();
    let tx = transfer(&ghost, 1, Address::treasury(), VINX, true);
    assert_eq!(net.state.clone().apply_tx(&tx), Err(TxError::UnknownSender));
    let alice = net.funded(VINX);
    let tx = transfer(&alice, 1, Address::treasury(), 2 * VINX, true);
    assert_eq!(
        net.state.clone().apply_tx(&tx),
        Err(TxError::InsufficientBalance)
    );
}

#[test]
fn memo_rules() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let shop = net.funded(10 * VINX);
    let mut s = net.state.clone();
    s.apply_tx(&user_tx(
        &shop,
        1,
        TxBody::SetMemoRequired { required: true },
        true,
    ))
    .unwrap();
    let pay = |memo: Option<Vec<u8>>| {
        user_tx(
            &alice,
            1,
            TxBody::Transfer {
                to: addr(&shop),
                amount: VINX,
                memo,
            },
            true,
        )
    };
    assert_eq!(s.clone().apply_tx(&pay(None)), Err(TxError::MemoRequired));
    assert_eq!(
        s.clone().apply_tx(&pay(Some(vec![]))),
        Err(TxError::InvalidMemo)
    );
    assert_eq!(
        s.clone().apply_tx(&pay(Some(vec![b'x'; 33]))),
        Err(TxError::InvalidMemo)
    );
    s.apply_tx(&pay(Some(b"FAC-412".to_vec()))).unwrap();
}

#[test]
fn self_transfer_and_zero_amount_are_rejected() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let mut s = net.state.clone();
    assert_eq!(
        s.apply_tx(&transfer(&alice, 1, addr(&alice), VINX, true)),
        Err(TxError::SelfTransfer)
    );
    assert_eq!(
        s.apply_tx(&transfer(&alice, 1, Address::treasury(), 0, true)),
        Err(TxError::ZeroAmount)
    );
}

#[test]
fn rotate_key_keeps_address_and_revokes_old_key() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let new_key = KeyPair::generate();
    let mut s = net.state.clone();
    s.apply_tx(&user_tx(
        &alice,
        1,
        TxBody::RotateKey {
            new_key: new_key.public_key(),
        },
        true,
    ))
    .unwrap();
    // L'ancienne clé ne signe plus pour le compte.
    let old = transfer(&alice, 2, Address::treasury(), VINX, false);
    assert_eq!(s.clone().apply_tx(&old), Err(TxError::BadSignature));
    // La nouvelle clé signe pour la même adresse.
    let mut tx = old.tx;
    tx.sender = addr(&alice);
    let signed = tx.sign(&new_key, false);
    s.apply_tx(&signed).unwrap();
}

#[test]
fn wrong_chain_is_rejected() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let mut tx = transfer(&alice, 1, Address::treasury(), VINX, true).tx;
    tx.chain_id = "autre".into();
    assert_eq!(
        net.state.clone().apply_tx(&tx.sign(&alice, true)),
        Err(TxError::WrongChain)
    );
}

// ── Administration et escrow ────────────────────────────────────────────────

#[test]
fn escrow_release_respects_maturity_and_cap() {
    let mut net = Net::new();
    let due = T0 + PERIOD;
    let early = net.admin(AdminAction::EscrowRelease { amount: 1 });
    assert!(matches!(
        tx_err(net.block(due - 1, &[early], &[])),
        TxError::Escrow(_)
    ));
    net.admin_nonce -= 1;
    let above = net.admin(AdminAction::EscrowRelease {
        amount: GENESIS_TRANCHE_AMOUNT + 1,
    });
    assert!(matches!(
        tx_err(net.block(due, &[above], &[])),
        TxError::Escrow(_)
    ));
    net.admin_nonce -= 1;
    let treasury = net.state.balance(&Address::treasury());
    let ok = net.admin(AdminAction::EscrowRelease {
        amount: GENESIS_TRANCHE_AMOUNT,
    });
    net.block(due, &[ok], &[]).unwrap();
    assert_eq!(
        net.state.balance(&Address::treasury()),
        treasury + GENESIS_TRANCHE_AMOUNT
    );
    // Impossible d'enchaîner sur la tranche suivante avant son échéance.
    let next = net.admin(AdminAction::EscrowRelease { amount: 1 });
    assert!(matches!(
        tx_err(net.block(due, &[next], &[])),
        TxError::Escrow(_)
    ));
}

#[test]
fn escrow_return_extends_the_calendar() {
    let mut net = Net::new();
    let total = net.state.escrow.total();
    let tx = net.admin(AdminAction::EscrowReturn { amount: 5 * VINX });
    net.block(T0, &[tx], &[]).unwrap();
    assert_eq!(net.state.escrow.total(), total + u128::from(5 * VINX));
    assert_eq!(net.state.escrow.tranches.len(), 121);
}

#[test]
fn admin_needs_two_distinct_signers_and_fresh_nonce() {
    let mut net = Net::new();
    let tx = AdminTransaction {
        chain_id: CHAIN.into(),
        nonce: 1,
        action: AdminAction::EscrowReturn { amount: VINX },
    };
    let single = SignedAdminTransaction {
        approvals: vec![tx.approve(0, &net.admins[0])],
        tx: tx.clone(),
    };
    assert_eq!(tx_err(net.block(T0, &[single], &[])), TxError::Unauthorized);
    let ok = SignedAdminTransaction {
        approvals: vec![tx.approve(0, &net.admins[0]), tx.approve(2, &net.admins[2])],
        tx,
    };
    net.block(T0, std::slice::from_ref(&ok), &[]).unwrap();
    assert_eq!(tx_err(net.block(T0, &[ok], &[])), TxError::BadNonce);
}

#[test]
fn admin_cannot_spend_from_users() {
    // Aucune action ne prend de compte source autre que la trésorerie : un
    // transfert de trésorerie au-delà de son solde échoue.
    let mut net = Net::new();
    let treasury = net.state.balance(&Address::treasury());
    let tx = net.admin(AdminAction::TreasuryTransfer {
        to: addr(&KeyPair::generate()),
        amount: treasury + 1,
        memo: None,
    });
    assert_eq!(
        tx_err(net.block(T0, &[tx], &[])),
        TxError::InsufficientBalance
    );
}

#[test]
fn set_param_rejects_zero_where_it_would_halt_the_chain() {
    let mut net = Net::new();
    let tx = net.admin(AdminAction::SetParam {
        param: ParamId::MaxBlockTxs,
        value: 0,
    });
    assert_eq!(tx_err(net.block(T0, &[tx], &[])), TxError::InvalidParam);
    net.admin_nonce -= 1;
    let tx = net.admin(AdminAction::SetParam {
        param: ParamId::MinFee,
        value: 0,
    });
    net.block(T0, &[tx], &[]).unwrap();
    assert_eq!(net.state.params.min_fee, 0);
}

#[test]
fn replaced_signer_loses_its_power() {
    let mut net = Net::new();
    let fresh = KeyPair::generate();
    let tx = net.admin(AdminAction::ReplaceSigner {
        index: 1,
        new_key: fresh.public_key(),
    });
    net.block(T0, &[tx], &[]).unwrap();
    // Les signataires 0 et 1 (ancienne clé) ne suffisent plus.
    let stale = net.admin(AdminAction::EscrowReturn { amount: VINX });
    assert_eq!(tx_err(net.block(T0, &[stale], &[])), TxError::Unauthorized);
}

#[test]
fn validator_changes_take_effect_two_blocks_later() {
    let mut net = Net::new();
    let newcomer = KeyPair::generate().public_key();
    let tx = net.admin(AdminAction::AddValidator {
        key: newcomer.clone(),
    });
    net.block(T0, &[tx], &[]).unwrap(); // bloc H = 1
    let h = net.state.height;
    assert!(!net.state.validators_at(h + 1).contains(&newcomer));
    assert!(net.state.validators_at(h + 2).contains(&newcomer));
}

#[test]
fn upgrade_needs_notice_and_activates_at_height() {
    let mut net = Net::new();
    let early = net.admin(AdminAction::ScheduleUpgrade {
        version: 2,
        activation_height: 100,
    });
    assert_eq!(
        tx_err(net.block(T0, &[early], &[])),
        TxError::InvalidUpgrade
    );
    net.admin_nonce -= 1;
    let skip = net.admin(AdminAction::ScheduleUpgrade {
        version: 3,
        activation_height: 20_000,
    });
    assert_eq!(tx_err(net.block(T0, &[skip], &[])), TxError::InvalidUpgrade);
    net.admin_nonce -= 1;
    let ok = net.admin(AdminAction::ScheduleUpgrade {
        version: 2,
        activation_height: 17_281,
    });
    net.block(T0, &[ok], &[]).unwrap();
    assert_eq!(net.state.protocol_version, 1);
    net.state.height = 17_280; // saute directement à la veille de l'activation
    net.block(T0, &[], &[]).unwrap();
    assert_eq!(net.state.protocol_version, 2);
    assert!(net.state.upgrade.is_none());
}

// ── Blocs ───────────────────────────────────────────────────────────────────

#[test]
fn one_invalid_tx_invalidates_the_whole_block() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let before = net.state.clone();
    let good = transfer(&alice, 1, Address::treasury(), VINX, true);
    let bad = transfer(&alice, 5, Address::treasury(), VINX, false);
    assert!(net.block(T0, &[], &[good, bad]).is_err());
    assert_eq!(net.state, before);
}

#[test]
fn apply_block_checks_header_commitments() {
    use vinx_core::{Block, BlockHeader};
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let txs = vec![transfer(&alice, 1, Address::treasury(), VINX, true)];
    let next = net
        .state
        .execute(net.state.height + 1, T0 * 1000, &[], &txs)
        .unwrap();
    let block = Block {
        header: BlockHeader {
            protocol_version: 1,
            chain_id: CHAIN.into(),
            height: next.height,
            timestamp: T0 * 1000,
            prev_hash: [0; 32],
            tx_root: vinx_state::state::tx_root(&[], &txs),
            state_root: next.state_root(),
            validators_hash: next.next_validators_hash(),
            proposer: 0,
        },
        admin_txs: vec![],
        txs,
    };
    let mut bad_root = block.clone();
    bad_root.header.state_root = [9; 32];
    assert_eq!(
        net.state.clone().apply_block(&bad_root),
        Err(BlockError::StateRootMismatch)
    );
    let mut bad_tx_root = block.clone();
    bad_tx_root.header.tx_root = [9; 32];
    assert_eq!(
        net.state.clone().apply_block(&bad_tx_root),
        Err(BlockError::TxRootMismatch)
    );
    let mut bad_version = block.clone();
    bad_version.header.protocol_version = 2;
    assert_eq!(
        net.state.clone().apply_block(&bad_version),
        Err(BlockError::WrongVersion)
    );
    let mut bad_vals = block.clone();
    bad_vals.header.validators_hash = [9; 32];
    assert_eq!(
        net.state.clone().apply_block(&bad_vals),
        Err(BlockError::ValidatorsHashMismatch)
    );
    net.state.apply_block(&block).unwrap();
    assert_eq!(net.state, next);
}

#[test]
fn wrong_height_is_rejected() {
    let net = Net::new();
    assert_eq!(
        net.state.execute(5, T0 * 1000, &[], &[]).unwrap_err(),
        BlockError::WrongHeight
    );
}
