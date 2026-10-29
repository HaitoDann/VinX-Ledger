// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Actifs natifs : création, émission, transferts, gel, reprise forcée
//! (protocole 5).

mod common;

use common::*;
use vinx_core::{
    AdminAction, AssetPowers, ParamId, PolicyConfig, SignedTransaction, Transaction, TxBody,
};
use vinx_crypto::{Address, KeyPair};
use vinx_state::{AssetError, BlockError, PolicyError, State, TxError};

const ALL: AssetPowers = AssetPowers {
    mint: true,
    freeze: true,
    clawback: true,
};

fn tx_err(r: Result<(), BlockError>) -> TxError {
    match r {
        Err(BlockError::InvalidTx { error, .. }) => error,
        other => panic!("attendu une transaction invalide, obtenu {other:?}"),
    }
}

fn create(symbol: &str, supply: u64, max: Option<u64>, powers: AssetPowers) -> TxBody {
    TxBody::CreateAsset {
        symbol: symbol.into(),
        name: "Euro VinX".into(),
        decimals: 6,
        initial_supply: supply,
        max_supply: max,
        powers,
    }
}

fn send(symbol: &str, to: Address, amount: u64) -> TxBody {
    TxBody::TransferAsset {
        symbol: symbol.into(),
        to,
        amount,
        memo: None,
    }
}

/// Un émetteur financé qui possède `name.vinx` ; son prochain nonce est 2.
fn issuer_with_name(net: &mut Net, name: &str) -> KeyPair {
    let issuer = net.funded(1_000 * VINX);
    let reg = user_tx(&issuer, 1, TxBody::RegisterName { name: name.into() }, true);
    net.block(T0, &[], &[reg]).unwrap();
    issuer
}

fn accept(who: &KeyPair, nonce: u64, symbol: &str, first: bool) -> SignedTransaction {
    user_tx(
        who,
        nonce,
        TxBody::AcceptAsset {
            symbol: symbol.into(),
            accept: true,
        },
        first,
    )
}

fn bal(s: &State, symbol: &str, who: &KeyPair) -> u64 {
    s.assets.balance(symbol, &addr(who))
}

#[test]
fn stablecoin_lifecycle() {
    let mut net = Net::current();
    let issuer = issuer_with_name(&mut net, "eurv");
    let bob = net.funded(10 * VINX);
    let carol = net.funded(VINX);
    let treasury = net.state.balance(&Address::treasury());

    net.block(
        T0,
        &[],
        &[user_tx(&issuer, 2, create("EURV", 1_000, None, ALL), false)],
    )
    .unwrap();
    let asset = net.state.assets.assets.get("EURV").unwrap().clone();
    assert_eq!(asset.issuer, addr(&issuer));
    assert_eq!(asset.supply, 1_000);
    assert_eq!(bal(&net.state, "EURV", &issuer), 1_000);
    assert_eq!(
        net.state.balance(&Address::treasury()),
        treasury + 100 * VINX,
        "le prix de création va à la trésorerie"
    );

    // On ne reçoit qu'un actif accepté.
    let early = user_tx(&issuer, 3, send("EURV", addr(&bob), 400), false);
    assert_eq!(
        tx_err(net.block(T0, &[], &[early])),
        TxError::Asset(AssetError::NotAccepted)
    );
    net.block(
        T0,
        &[],
        &[
            accept(&bob, 1, "EURV", true),
            accept(&carol, 1, "EURV", true),
        ],
    )
    .unwrap();
    net.block(
        T0,
        &[],
        &[
            user_tx(&issuer, 3, send("EURV", addr(&bob), 400), false),
            user_tx(
                &issuer,
                4,
                TxBody::MintAsset {
                    symbol: "EURV".into(),
                    to: addr(&carol),
                    amount: 50,
                },
                false,
            ),
            user_tx(
                &bob,
                2,
                TxBody::BurnAsset {
                    symbol: "EURV".into(),
                    amount: 100,
                },
                false,
            ),
        ],
    )
    .unwrap();
    assert_eq!(bal(&net.state, "EURV", &bob), 300);
    assert_eq!(bal(&net.state, "EURV", &carol), 50);
    assert_eq!(net.state.assets.assets.get("EURV").unwrap().supply, 950);

    // Gel du compte de Bob : il ne peut plus ni envoyer ni recevoir.
    let freeze = |nonce, frozen| {
        user_tx(
            &issuer,
            nonce,
            TxBody::FreezeAsset {
                symbol: "EURV".into(),
                account: Some(addr(&bob)),
                frozen,
            },
            false,
        )
    };
    net.block(T0, &[], &[freeze(5, true)]).unwrap();
    let out = user_tx(&bob, 3, send("EURV", addr(&issuer), 1), false);
    assert_eq!(
        tx_err(net.block(T0, &[], &[out])),
        TxError::Asset(AssetError::Frozen)
    );
    let inbound = user_tx(&issuer, 6, send("EURV", addr(&bob), 1), false);
    assert_eq!(
        tx_err(net.block(T0, &[], &[inbound])),
        TxError::Asset(AssetError::Frozen)
    );

    // Reprise forcée, même sur un compte gelé, puis dégel.
    let clawback = |nonce, amount| {
        user_tx(
            &issuer,
            nonce,
            TxBody::ClawbackAsset {
                symbol: "EURV".into(),
                from: addr(&bob),
                amount,
            },
            false,
        )
    };
    net.block(T0, &[], &[clawback(6, 200), freeze(7, false)])
        .unwrap();
    assert_eq!(bal(&net.state, "EURV", &bob), 100);
    assert_eq!(bal(&net.state, "EURV", &issuer), 800);
    net.block(
        T0,
        &[],
        &[user_tx(&bob, 3, send("EURV", addr(&carol), 100), false)],
    )
    .unwrap();
    assert_eq!(bal(&net.state, "EURV", &bob), 0);
    // Bob retire l'actif de son compte : il ne peut plus le recevoir.
    net.block(
        T0,
        &[],
        &[user_tx(
            &bob,
            4,
            TxBody::AcceptAsset {
                symbol: "EURV".into(),
                accept: false,
            },
            false,
        )],
    )
    .unwrap();
    assert!(
        !net.state
            .assets
            .holdings
            .contains_key(&("EURV".to_owned(), addr(&bob)))
    );

    // L'émetteur abandonne la reprise forcée : c'est définitif.
    let renounce = user_tx(
        &issuer,
        8,
        TxBody::RenounceAssetPowers {
            symbol: "EURV".into(),
            powers: AssetPowers {
                clawback: true,
                ..AssetPowers::default()
            },
        },
        false,
    );
    net.block(T0, &[], &[renounce]).unwrap();
    let powers = net.state.assets.assets.get("EURV").unwrap().powers;
    assert_eq!(
        powers,
        AssetPowers {
            mint: true,
            freeze: true,
            clawback: false
        }
    );
    let again = user_tx(
        &issuer,
        9,
        TxBody::ClawbackAsset {
            symbol: "EURV".into(),
            from: addr(&carol),
            amount: 1,
        },
        false,
    );
    assert_eq!(
        tx_err(net.block(T0, &[], &[again])),
        TxError::Asset(AssetError::PowerMissing)
    );
}

#[test]
fn symbol_must_be_an_owned_name_and_is_used_once() {
    let mut net = Net::current();
    let issuer = issuer_with_name(&mut net, "eurv");
    let other = net.funded(1_000 * VINX);

    let squat = user_tx(&other, 1, create("EURV", 1, None, ALL), true);
    assert_eq!(
        tx_err(net.block(T0, &[], &[squat])),
        TxError::Asset(AssetError::NotSymbolOwner)
    );
    let lower = user_tx(&issuer, 2, create("eurv", 1, None, ALL), false);
    assert_eq!(
        tx_err(net.block(T0, &[], &[lower])),
        TxError::Asset(AssetError::NotSymbolOwner)
    );
    for bad in [
        create("EURV", 0, None, AssetPowers::default()),
        create("EURV", 10, Some(5), ALL),
        TxBody::CreateAsset {
            symbol: "EURV".into(),
            name: "Euro".into(),
            decimals: 10,
            initial_supply: 1,
            max_supply: None,
            powers: ALL,
        },
    ] {
        let tx = user_tx(&issuer, 2, bad, false);
        assert_eq!(
            tx_err(net.block(T0, &[], &[tx])),
            TxError::Asset(AssetError::Invalid)
        );
    }

    net.block(
        T0,
        &[],
        &[user_tx(&issuer, 2, create("EURV", 1, None, ALL), false)],
    )
    .unwrap();
    // Le nom change de mains : le symbole reste à l'actif existant.
    let give = user_tx(
        &issuer,
        3,
        TxBody::TransferName {
            name: "eurv".into(),
            to: addr(&other),
        },
        false,
    );
    net.block(T0, &[], &[give]).unwrap();
    let twice = user_tx(&other, 1, create("EURV", 1, None, ALL), true);
    assert_eq!(
        tx_err(net.block(T0, &[], &[twice])),
        TxError::Asset(AssetError::SymbolTaken)
    );
    assert_eq!(
        net.state.assets.assets.get("EURV").unwrap().issuer,
        addr(&issuer)
    );
}

#[test]
fn fixed_supply_and_max_supply_are_enforced() {
    let mut net = Net::current();
    let issuer = issuer_with_name(&mut net, "pts");
    let mint = |nonce, amount| {
        user_tx(
            &issuer,
            nonce,
            TxBody::MintAsset {
                symbol: "PTS".into(),
                to: addr(&issuer),
                amount,
            },
            false,
        )
    };
    let capped = AssetPowers {
        mint: true,
        ..AssetPowers::default()
    };
    net.block(
        T0,
        &[],
        &[user_tx(
            &issuer,
            2,
            create("PTS", 0, Some(100), capped),
            false,
        )],
    )
    .unwrap();
    net.block(T0, &[], &[mint(3, 100)]).unwrap();
    assert_eq!(
        tx_err(net.block(T0, &[], &[mint(4, 1)])),
        TxError::Asset(AssetError::MaxSupply)
    );
    let freeze_all = user_tx(
        &issuer,
        4,
        TxBody::FreezeAsset {
            symbol: "PTS".into(),
            account: None,
            frozen: true,
        },
        false,
    );
    assert_eq!(
        tx_err(net.block(T0, &[], &[freeze_all])),
        TxError::Asset(AssetError::PowerMissing),
        "pas de gel sans le pouvoir de gel"
    );
    let stranger = net.funded(VINX);
    let foreign = user_tx(
        &stranger,
        1,
        TxBody::MintAsset {
            symbol: "PTS".into(),
            to: addr(&stranger),
            amount: 1,
        },
        true,
    );
    assert_eq!(
        tx_err(net.block(T0, &[], &[foreign])),
        TxError::Asset(AssetError::NotIssuer)
    );
}

#[test]
fn whole_asset_freeze_stops_every_transfer() {
    let mut net = Net::current();
    let issuer = issuer_with_name(&mut net, "eurv");
    let bob = net.funded(VINX);
    let pause = |nonce, frozen| {
        user_tx(
            &issuer,
            nonce,
            TxBody::FreezeAsset {
                symbol: "EURV".into(),
                account: None,
                frozen,
            },
            false,
        )
    };
    net.block(
        T0,
        &[],
        &[
            user_tx(&issuer, 2, create("EURV", 10, None, ALL), false),
            accept(&bob, 1, "EURV", true),
            pause(3, true),
        ],
    )
    .unwrap();
    let tx = user_tx(&issuer, 4, send("EURV", addr(&bob), 1), false);
    assert_eq!(
        tx_err(net.block(T0, &[], &[tx])),
        TxError::Asset(AssetError::Frozen)
    );
    net.block(
        T0,
        &[],
        &[
            pause(4, false),
            user_tx(&issuer, 5, send("EURV", addr(&bob), 1), false),
        ],
    )
    .unwrap();
    assert_eq!(bal(&net.state, "EURV", &bob), 1);
}

#[test]
fn multisig_issuer_needs_approvals_to_mint() {
    let mut net = Net::current();
    let issuer = issuer_with_name(&mut net, "eurv");
    let member = KeyPair::generate();
    let policy = PolicyConfig {
        signers: vec![member.public_key()],
        threshold: 1,
        daily_limit: Some(1_000 * VINX),
    };
    net.block(
        T0,
        &[],
        &[
            user_tx(&issuer, 2, create("EURV", 10, None, ALL), false),
            user_tx(&issuer, 3, TxBody::SetPolicy { policy }, false),
        ],
    )
    .unwrap();
    let body = TxBody::MintAsset {
        symbol: "EURV".into(),
        to: addr(&issuer),
        amount: 5,
    };
    let alone = user_tx(&issuer, 4, body.clone(), false);
    assert_eq!(
        tx_err(net.block(T0, &[], &[alone])),
        TxError::Policy(PolicyError::ApprovalsMissing)
    );
    let draft = Transaction {
        chain_id: CHAIN.into(),
        sender: addr(&issuer),
        nonce: 4,
        fee: VINX / 1000,
        body: body.clone(),
    };
    let approval = draft.approve(0, &member);
    let signed: SignedTransaction = Transaction {
        body: TxBody::Approved {
            body: Box::new(body),
            approvals: vec![approval],
        },
        ..draft
    }
    .sign(&issuer, false);
    net.block(T0, &[], &[signed]).unwrap();
    assert_eq!(bal(&net.state, "EURV", &issuer), 15);
    // Détruire ses propres jetons reste possible sans approbation.
    let burn = user_tx(
        &issuer,
        5,
        TxBody::BurnAsset {
            symbol: "EURV".into(),
            amount: 1,
        },
        false,
    );
    net.block(T0, &[], &[burn]).unwrap();
}

#[test]
fn asset_fee_is_set_by_governance() {
    let mut net = Net::current();
    let issuer = issuer_with_name(&mut net, "eurv");
    let set = net.admin(AdminAction::SetParam {
        param: ParamId::AssetFee,
        value: 5 * VINX,
    });
    net.block(T0, &[set], &[]).unwrap();
    let before = net.state.balance(&addr(&issuer));
    net.block(
        T0,
        &[],
        &[user_tx(&issuer, 2, create("EURV", 1, None, ALL), false)],
    )
    .unwrap();
    assert_eq!(
        before - net.state.balance(&addr(&issuer)),
        5 * VINX + VINX / 1000
    );
}

#[test]
fn assets_need_protocol_5() {
    let mut net = Net::new();
    let alice = net.funded(10 * VINX);
    let tx = user_tx(&alice, 1, create("EURV", 1, None, ALL), true);
    assert_eq!(tx_err(net.block(T0, &[], &[tx])), TxError::NotYetActive);
}

#[test]
fn asset_rows_reload_to_the_same_state_root() {
    let mut net = Net::current();
    let issuer = issuer_with_name(&mut net, "eurv");
    let bob = net.funded(VINX);
    net.block(
        T0,
        &[],
        &[
            user_tx(&issuer, 2, create("EURV", 10, None, ALL), false),
            accept(&bob, 1, "EURV", true),
            user_tx(&issuer, 3, send("EURV", addr(&bob), 4), false),
        ],
    )
    .unwrap();
    let root = net.state.state_root();
    let mut state = net.state.clone();
    let rows = state.take_dirty_rows();
    let mut reloaded =
        State::from_parts(&state.globals_bytes_light(), state.accounts.clone()).expect("état relu");
    for (key, value) in rows {
        if let Some(v) = value {
            reloaded.load_row(&key, &v).unwrap();
        }
    }
    assert_eq!(reloaded.state_root(), root);
    assert_eq!(reloaded.assets.balance("EURV", &addr(&bob)), 4);
}

/// Émetteur de EURV (1 000 unités) et deux comptes qui l'ont accepté, avec
/// 100 unités chacun. Prochains nonces : émetteur 5, a et b 2.
fn eurv_with_two_holders(net: &mut Net) -> (KeyPair, KeyPair, KeyPair) {
    let issuer = issuer_with_name(net, "eurv");
    let a = net.funded(10 * VINX);
    let b = net.funded(10 * VINX);
    net.block(
        T0,
        &[],
        &[
            user_tx(&issuer, 2, create("EURV", 1_000, None, ALL), false),
            accept(&a, 1, "EURV", true),
            accept(&b, 1, "EURV", true),
            user_tx(&issuer, 3, send("EURV", addr(&a), 100), false),
            user_tx(&issuer, 4, send("EURV", addr(&b), 100), false),
        ],
    )
    .unwrap();
    (issuer, a, b)
}

fn open_deal(buyer: &KeyPair, nonce: u64, seller: Address, amount: u64) -> SignedTransaction {
    user_tx(
        buyer,
        nonce,
        TxBody::OpenAssetDeal {
            symbol: "EURV".into(),
            seller,
            arbiter: None,
            amount,
            deadline: T0 + 86_400,
            memo: Some(b"commande 7".to_vec()),
        },
        false,
    )
}

#[test]
fn asset_deal_locks_units_until_release_or_refund() {
    let mut net = Net::current();
    let (issuer, a, b) = eurv_with_two_holders(&mut net);
    let stranger = net.funded(VINX);
    let refused = user_tx(
        &a,
        2,
        TxBody::OpenAssetDeal {
            symbol: "EURV".into(),
            seller: addr(&stranger),
            arbiter: None,
            amount: 10,
            deadline: T0 + 86_400,
            memo: None,
        },
        false,
    );
    assert_eq!(
        tx_err(net.block(T0, &[], &[refused])),
        TxError::Asset(AssetError::NotAccepted),
        "le vendeur doit accepter l'actif"
    );

    net.block(T0, &[], &[open_deal(&a, 2, addr(&b), 40)])
        .unwrap();
    let id = vinx_core::deal_id(&addr(&a), 2);
    assert_eq!(bal(&net.state, "EURV", &a), 60);
    assert_eq!(net.state.assets.deals.get(&id).unwrap().deal.amount, 40);
    // Le vendeur ne peut pas se verser le séquestre lui-même.
    let grab = user_tx(&b, 2, TxBody::ReleaseDeal { id }, false);
    assert_eq!(tx_err(net.block(T0, &[], &[grab])), TxError::DealForbidden);
    // Gel du vendeur : versement impossible tant qu'il dure.
    let freeze = |nonce, frozen| {
        user_tx(
            &issuer,
            nonce,
            TxBody::FreezeAsset {
                symbol: "EURV".into(),
                account: Some(addr(&b)),
                frozen,
            },
            false,
        )
    };
    net.block(T0, &[], &[freeze(5, true)]).unwrap();
    let release = |nonce| user_tx(&a, nonce, TxBody::ReleaseDeal { id }, false);
    assert_eq!(
        tx_err(net.block(T0, &[], &[release(3)])),
        TxError::Asset(AssetError::Frozen)
    );
    net.block(T0, &[], &[freeze(6, false), release(3)]).unwrap();
    assert_eq!(bal(&net.state, "EURV", &b), 140);
    assert!(net.state.assets.deals.get(&id).is_none());
    assert_eq!(
        net.state.assets.assets.get("EURV").unwrap().supply,
        1_000,
        "un séquestre ne change pas la supply"
    );

    // Remboursement par le vendeur, vers un acheteur qui a retiré l'actif entre-temps.
    net.block(T0, &[], &[open_deal(&a, 4, addr(&b), 60)])
        .unwrap();
    let id2 = vinx_core::deal_id(&addr(&a), 4);
    let leave = user_tx(
        &a,
        5,
        TxBody::AcceptAsset {
            symbol: "EURV".into(),
            accept: false,
        },
        false,
    );
    net.block(T0, &[], &[leave]).unwrap();
    let refund = user_tx(&b, 2, TxBody::RefundDeal { id: id2 }, false);
    net.block(T0, &[], &[refund]).unwrap();
    assert_eq!(bal(&net.state, "EURV", &a), 60);
}

#[test]
fn asset_mandate_collects_within_its_limit() {
    let mut net = Net::current();
    let (_issuer, payer, shop) = eurv_with_two_holders(&mut net);
    let grant = user_tx(
        &payer,
        2,
        TxBody::GrantAssetMandate {
            symbol: "EURV".into(),
            payee: addr(&shop),
            max_per_period: 30,
            period: 30 * 86_400,
            expires_at: None,
        },
        false,
    );
    net.block(T0, &[], &[grant]).unwrap();
    let collect = |nonce, amount| {
        user_tx(
            &shop,
            nonce,
            TxBody::CollectAsset {
                symbol: "EURV".into(),
                payer: addr(&payer),
                amount,
                memo: None,
            },
            false,
        )
    };
    net.block(T0, &[], &[collect(2, 20)]).unwrap();
    assert_eq!(bal(&net.state, "EURV", &shop), 120);
    assert_eq!(
        tx_err(net.block(T0, &[], &[collect(3, 11)])),
        TxError::Mandate(vinx_state::MandateError::Exceeded)
    );
    // Période suivante : la limite repart.
    net.block(T0 + 31 * 86_400, &[], &[collect(3, 30)]).unwrap();
    assert_eq!(bal(&net.state, "EURV", &payer), 50);
    // Révoqué : plus aucun prélèvement.
    let revoke = user_tx(
        &payer,
        3,
        TxBody::RevokeAssetMandate {
            symbol: "EURV".into(),
            payee: addr(&shop),
        },
        false,
    );
    net.block(T0 + 31 * 86_400, &[], &[revoke]).unwrap();
    assert_eq!(
        tx_err(net.block(T0 + 31 * 86_400, &[], &[collect(4, 1)])),
        TxError::Mandate(vinx_state::MandateError::Unknown)
    );
}

#[test]
fn asset_rows_of_deals_and_mandates_reload() {
    let mut net = Net::current();
    let (_issuer, a, b) = eurv_with_two_holders(&mut net);
    let grant = user_tx(
        &b,
        2,
        TxBody::GrantAssetMandate {
            symbol: "EURV".into(),
            payee: addr(&a),
            max_per_period: 5,
            period: 86_400,
            expires_at: None,
        },
        false,
    );
    net.block(T0, &[], &[open_deal(&a, 2, addr(&b), 7), grant])
        .unwrap();
    let root = net.state.state_root();
    let mut state = net.state.clone();
    let rows = state.take_dirty_rows();
    let mut reloaded =
        State::from_parts(&state.globals_bytes_light(), state.accounts.clone()).unwrap();
    for (key, value) in rows {
        if let Some(v) = value {
            reloaded.load_row(&key, &v).unwrap();
        }
    }
    assert_eq!(reloaded.state_root(), root);
    assert_eq!(reloaded.assets.deals.len(), 1);
    assert_eq!(reloaded.assets.mandates.len(), 1);
}
