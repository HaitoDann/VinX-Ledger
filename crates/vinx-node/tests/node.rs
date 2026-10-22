//! Nœud complet en mémoire : production de blocs, API, transactions, reprise.

// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;
use vinx_core::{
    AdminAction, AdminTransaction, SignedAdminTransaction, Transaction, TxBody, UNITS_PER_VINX,
    encode,
};
use vinx_crypto::{Address, KeyPair};
use vinx_node::node::{self, SharedRef};
use vinx_node::{api, genesis_file::GenesisFile};
use vinx_state::Genesis;

const VINX: u64 = UNITS_PER_VINX;
const CHAIN: &str = "vinx-devnet-test";

struct Devnet {
    admins: [KeyPair; 3],
    validator: KeyPair,
    genesis: Genesis,
}

fn devnet() -> Devnet {
    let admins = [
        KeyPair::generate(),
        KeyPair::generate(),
        KeyPair::generate(),
    ];
    let validator = KeyPair::generate();
    let mut genesis = Genesis::standard(
        CHAIN,
        node::now_ms() / 1000 - 1,
        [
            admins[0].public_key(),
            admins[1].public_key(),
            admins[2].public_key(),
        ],
        vec![validator.public_key()],
    )
    .unwrap();
    // Blocs rapprochés pour un test rapide.
    genesis.params.min_block_interval_ms = 50;
    genesis.params.timeout_propose_ms = 300;
    genesis.params.timeout_vote_ms = 300;
    Devnet {
        admins,
        validator,
        genesis,
    }
}

async fn call(
    shared: &SharedRef,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = api::router(shared.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn wait_for_height(shared: &SharedRef, height: u64) {
    for _ in 0..400 {
        if node::lock(shared).chain.state.height >= height {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("hauteur {height} non atteinte");
}

#[tokio::test(flavor = "multi_thread")]
async fn devnet_produces_blocks_and_processes_payments() {
    let dir = tempfile::tempdir().unwrap();
    let net = devnet();
    // La genèse passe par son format JSON, comme en production.
    let path = dir.path().join("genesis.json");
    GenesisFile::save(&net.genesis, &path).unwrap();
    let genesis = GenesisFile::load(&path).unwrap();

    let shared = node::open(genesis.clone(), &dir.path().join("data")).unwrap();
    let (stop, stop_rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(node::run(
        shared.clone(),
        Some(net.validator.clone()),
        tokio::sync::mpsc::unbounded_channel().1,
        stop_rx,
    ));
    wait_for_height(&shared, 2).await;

    // 1. La trésorerie finance Alice (transaction admin 2-sur-3).
    let alice = KeyPair::generate();
    let alice_addr = Address::from_public_key(&alice.public_key());
    let admin_tx = AdminTransaction {
        chain_id: CHAIN.into(),
        nonce: 1,
        action: AdminAction::TreasuryTransfer {
            to: alice_addr,
            amount: 10 * VINX,
            memo: None,
        },
    };
    let signed = SignedAdminTransaction {
        approvals: vec![
            admin_tx.approve(0, &net.admins[0]),
            admin_tx.approve(2, &net.admins[2]),
        ],
        tx: admin_tx,
    };
    let (status, _) = call(
        &shared,
        "POST",
        "/admin-transactions",
        Some(json!({ "tx": hex::encode(encode(&signed)) })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let funded_at = node::lock(&shared).chain.state.height + 2;
    wait_for_height(&shared, funded_at).await;
    let (status, account) = call(&shared, "GET", &format!("/accounts/{alice_addr}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(account["balance"], json!((10 * VINX).to_string()));

    // 2. Alice paie Bob.
    let bob = Address::from_public_key(&KeyPair::generate().public_key());
    let pay = Transaction {
        chain_id: CHAIN.into(),
        sender: alice_addr,
        nonce: 1,
        fee: VINX / 1000,
        body: TxBody::Transfer {
            to: bob,
            amount: 3 * VINX,
            memo: Some(b"FAC-1".to_vec()),
        },
    }
    .sign(&alice, true);
    let body = json!({ "tx": hex::encode(encode(&pay)) });
    let (status, reply) = call(&shared, "POST", "/transactions", Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    // Rejeu immédiat refusé par le mempool.
    let (status, _) = call(&shared, "POST", "/transactions", Some(body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let paid_at = node::lock(&shared).chain.state.height + 2;
    wait_for_height(&shared, paid_at).await;
    let (_, bob_account) = call(&shared, "GET", &format!("/accounts/{bob}"), None).await;
    assert_eq!(bob_account["balance"], json!((3 * VINX).to_string()));

    // 3. Requêtes invalides.
    let (status, _) = call(&shared, "GET", "/accounts/pas-une-adresse", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &shared,
        "POST",
        "/transactions",
        Some(json!({ "tx": "00" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, block) = call(&shared, "GET", "/blocks/1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(block["height"], json!(1));

    // 4. Arrêt puis reprise : la chaîne repart de là où elle s'était arrêtée.
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    let height = node::lock(&shared).chain.state.height;
    let state = node::lock(&shared).chain.state.clone();
    drop(shared);

    let reopened = node::open(genesis, &dir.path().join("data")).unwrap();
    assert_eq!(node::lock(&reopened).chain.state, state);
    let (stop, stop_rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(node::run(
        reopened.clone(),
        Some(net.validator.clone()),
        tokio::sync::mpsc::unbounded_channel().1,
        stop_rx,
    ));
    wait_for_height(&reopened, height + 2).await;
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    assert!(node::lock(&reopened).chain.state.check_invariant().is_ok());
}

#[tokio::test]
async fn a_node_without_validator_key_only_observes() {
    let dir = tempfile::tempdir().unwrap();
    let net = devnet();
    let shared = node::open(net.genesis, dir.path()).unwrap();
    let (stop, stop_rx) = tokio::sync::watch::channel(false);
    let (_in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(node::run(
        shared.clone(),
        Some(KeyPair::generate()),
        in_rx,
        stop_rx,
    ));
    tokio::time::sleep(Duration::from_millis(400)).await;
    // Sans validateur joignable, rien n'est produit.
    assert_eq!(node::lock(&shared).chain.state.height, 0);
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[test]
fn a_data_dir_cannot_be_reused_with_another_genesis() {
    let dir = tempfile::tempdir().unwrap();
    drop(node::open(devnet().genesis, dir.path()).unwrap());
    assert!(node::open(devnet().genesis, dir.path()).is_err());
}
