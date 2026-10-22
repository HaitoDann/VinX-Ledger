//! Wallet contre un vrai nœud, à travers l'API HTTP.

// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use std::time::Duration;
use vinx_core::{AdminAction, UNITS_PER_VINX};
use vinx_crypto::{Address, KeyPair};
use vinx_node::{api, node};
use vinx_state::Genesis;
use vinx_wallet::client::Client;
use vinx_wallet::{admin, ops};

const VINX: u64 = UNITS_PER_VINX;

/// Démarre un nœud à un validateur sur un port libre ; renvoie son URL.
fn start_node(admins: &[KeyPair; 3], dir: &std::path::Path) -> String {
    let validator = KeyPair::generate();
    let mut genesis = Genesis::standard(
        "vinx-devnet-test",
        node::now_ms() / 1000 - 1,
        [
            admins[0].public_key(),
            admins[1].public_key(),
            admins[2].public_key(),
        ],
        vec![validator.public_key()],
    )
    .unwrap();
    genesis.params.min_block_interval_ms = 50;
    let shared = node::open(genesis, dir).unwrap();
    let (url_tx, url_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            url_tx
                .send(format!("http://{}", listener.local_addr().unwrap()))
                .unwrap();
            let (_stop, stop_rx) = tokio::sync::watch::channel(false);
            let (_in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(node::run(shared.clone(), Some(validator), in_rx, stop_rx));
            axum::serve(listener, api::router(shared)).await.unwrap();
        });
    });
    url_rx.recv().unwrap()
}

fn wait_blocks(client: &Client, n: u64) {
    let target = client.status().unwrap().height + n;
    for _ in 0..400 {
        if client.status().unwrap().height >= target {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("le nœud n'avance plus");
}

#[test]
fn full_payment_flow_through_the_wallet() {
    let dir = tempfile::tempdir().unwrap();
    let admins = [
        KeyPair::generate(),
        KeyPair::generate(),
        KeyPair::generate(),
    ];
    let client = Client::new(start_node(&admins, dir.path()));
    let status = client.status().unwrap();
    assert_eq!(status.chain_id, "vinx-devnet-test");

    // Multisig : proposition, deux approbations, envoi.
    let alice = KeyPair::generate();
    let alice_addr = Address::from_public_key(&alice.public_key());
    let mut proposal = admin::propose(
        status.chain_id.clone(),
        status.admin_nonce + 1,
        AdminAction::TreasuryTransfer {
            to: alice_addr,
            amount: 20 * VINX,
            memo: None,
        },
    );
    let file = dir.path().join("proposition.json");
    admin::approve(&mut proposal, 0, &admins[0]).unwrap();
    admin::save(&proposal, &file).unwrap();
    // Une seule approbation : refusée par le nœud.
    assert!(client.submit_admin(&admin::load(&file).unwrap()).is_err());
    let mut proposal = admin::load(&file).unwrap();
    admin::approve(&mut proposal, 1, &admins[1]).unwrap();
    client.submit_admin(&proposal).unwrap();
    wait_blocks(&client, 2);
    assert_eq!(
        client.account(&alice_addr).unwrap().unwrap().balance,
        20 * VINX
    );

    // Un compte inexistant ne peut pas envoyer.
    let ghost = KeyPair::generate();
    assert!(ops::transfer(&client, &ghost, None, alice_addr, VINX, None, None).is_err());

    // Le commerçant exige un mémo.
    let shop = KeyPair::generate();
    let shop_addr = Address::from_public_key(&shop.public_key());
    ops::transfer(
        &client,
        &alice,
        None,
        shop_addr,
        5 * VINX,
        Some("ouverture".into()),
        None,
    )
    .unwrap();
    wait_blocks(&client, 2);
    ops::set_memo_required(&client, &shop, None, true).unwrap();
    wait_blocks(&client, 2);
    let err = ops::transfer(&client, &alice, None, shop_addr, VINX, None, None).unwrap_err();
    assert!(err.to_string().contains("mémo"), "{err}");
    ops::transfer(
        &client,
        &alice,
        None,
        shop_addr,
        VINX,
        Some("FAC-42".into()),
        None,
    )
    .unwrap();
    wait_blocks(&client, 2);

    let shop_account = client.account(&shop_addr).unwrap().unwrap();
    let fee = status.min_fee;
    assert_eq!(shop_account.balance, 6 * VINX - fee);
    assert!(shop_account.memo_required);
    assert_eq!(
        client.account(&alice_addr).unwrap().unwrap().balance,
        14 * VINX - 2 * fee
    );

    // Rotation de clé : la nouvelle clé signe pour la même adresse.
    let new_alice = KeyPair::generate();
    ops::rotate_key(&client, &alice, None, new_alice.public_key()).unwrap();
    wait_blocks(&client, 2);
    // Sans `from`, l'adresse dérivée de la nouvelle clé n'est pas le compte d'Alice.
    assert!(
        ops::transfer(
            &client,
            &new_alice,
            None,
            shop_addr,
            VINX,
            Some("x".into()),
            None
        )
        .is_err()
    );
    ops::transfer(
        &client,
        &new_alice,
        Some(alice_addr),
        shop_addr,
        VINX,
        Some("x".into()),
        None,
    )
    .unwrap();
    // L'ancienne clé ne peut plus rien signer pour ce compte.
    wait_blocks(&client, 2);
    assert!(
        ops::transfer(
            &client,
            &alice,
            None,
            shop_addr,
            VINX,
            Some("y".into()),
            None
        )
        .is_err()
    );
}
