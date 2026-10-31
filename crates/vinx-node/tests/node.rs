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
    .unwrap()
    .with_faucet(1_000 * VINX)
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

    // 3 bis. Recherche, historique, état des validateurs, mempool.
    let pay_id = hex::encode(pay.id());
    let (status, found) = call(&shared, "GET", &format!("/transactions/{pay_id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["status"], json!("finalisée"));
    assert_eq!(found["memo"], json!("FAC-1"));
    assert_eq!(found["to"], json!(bob.to_string()));
    let (status, _) = call(
        &shared,
        "GET",
        &format!("/transactions/{}", "ab".repeat(32)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&shared, "GET", "/transactions/zz", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, block) = call(
        &shared,
        "GET",
        &format!("/blocks/{}", found["height"]),
        None,
    )
    .await;
    assert_eq!(block["details"][0]["id"], json!(pay_id));
    assert_eq!(block["signers"].as_array().unwrap().len(), 1);
    let (_, history) = call(&shared, "GET", "/admin-history", None).await;
    assert_eq!(
        history["actions"][0]["kind"],
        json!("transfert de trésorerie")
    );
    assert_eq!(history["actions"][0]["approvals"], json!([0, 2]));
    let (_, validators) = call(&shared, "GET", "/validators", None).await;
    assert_eq!(validators["validators"][0]["presence"], json!("en ligne"));
    assert_eq!(validators["validators"][0]["this_node"], json!(true));
    let (_, mempool) = call(&shared, "GET", "/mempool", None).await;
    assert_eq!(
        mempool["rejects"][0]["reason"],
        json!("transaction déjà en attente")
    );
    let (_, hist) = call(&shared, "GET", "/history", None).await;
    assert!(!hist["blocks"].as_array().unwrap().is_empty());

    // 4. Outils d'exploitation : gouvernance, derniers blocs, métriques, tableau de bord.
    let (status, gov) = call(&shared, "GET", "/governance", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gov["admin"]["nonce"], json!(1));
    assert_eq!(gov["admin"]["signers"].as_array().unwrap().len(), 3);
    assert_eq!(gov["validators"]["active"].as_array().unwrap().len(), 1);
    let (_, recent) = call(&shared, "GET", "/blocks", None).await;
    let recent = recent["blocks"].as_array().unwrap();
    assert!(!recent.is_empty() && recent.len() <= 20);
    assert!(recent[0]["height"].as_u64() > recent.last().unwrap()["height"].as_u64());
    let request = Request::builder()
        .uri("/metrics")
        .body(Body::empty())
        .unwrap();
    let response = api::router(shared.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    for metric in [
        "vinx_height ",
        "vinx_blocks_first_round_total ",
        "vinx_peers ",
        "vinx_evidence_total 0",
    ] {
        assert!(text.contains(metric), "métrique absente : {metric}");
    }
    let request = Request::builder().uri("/").body(Body::empty()).unwrap();
    let response = api::router(shared.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&page).contains("VinX Ledger"));

    // 5. Arrêt puis reprise : la chaîne repart de là où elle s'était arrêtée.
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

/// Requête brute : corps quelconque, pas forcément du JSON.
async fn raw(shared: &SharedRef, method: &str, uri: &str, body: Vec<u8>) -> StatusCode {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    api::router(shared.clone())
        .oneshot(request)
        .await
        .unwrap()
        .status()
}

/// Fuzzing de l'API publique : des milliers de requêtes absurdes (corps
/// aléatoires, transactions mutées, chemins et paramètres extrêmes) ne font
/// jamais planter le nœud et n'obtiennent jamais d'erreur interne (500).
#[tokio::test(flavor = "multi_thread")]
async fn the_public_api_survives_garbage() {
    let d = devnet();
    let dir = tempfile::tempdir().unwrap();
    let shared = node::open(d.genesis.clone(), dir.path()).unwrap();
    // Générateur pseudo-aléatoire déterministe : un échec se rejoue à l'identique.
    let mut seed: u64 = 0x5EED_CAFE;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let alice = KeyPair::generate();
    let valid = encode(
        &Transaction {
            chain_id: CHAIN.into(),
            sender: Address::from_public_key(&alice.public_key()),
            nonce: 1,
            fee: VINX / 1000,
            body: TxBody::Transfer {
                to: Address::treasury(),
                amount: VINX,
                memo: None,
            },
        }
        .sign(&alice, true),
    );
    let weird = [
        "",
        "0",
        "-1",
        "18446744073709551615",
        "99999999999999999999999",
        "%00",
        "..",
        "vinx1",
        "VINX1QQQQ",
        "zz",
        "%F0%9F%92%A5",
        "a/b",
        "EURV",
        "%2e%2e%2fetc",
    ];
    for i in 0..3_000u64 {
        let r = next();
        let w = |k: u64| weird[(k % weird.len() as u64) as usize];
        let (method, uri, body): (&str, String, Vec<u8>) = match i % 6 {
            // Corps aléatoires sur les deux points d'envoi.
            0 => {
                let len = (r % 300) as usize;
                let bytes = (0..len).map(|_| next() as u8).collect();
                let path = if r & 1 == 0 {
                    "/transactions"
                } else {
                    "/admin-transactions"
                };
                ("POST", path.into(), bytes)
            }
            // Transaction valide mutée, encodée en hexadécimal.
            1 => {
                let mut tx = valid.clone();
                for _ in 0..1 + r % 4 {
                    let at = (next() as usize) % tx.len();
                    tx[at] ^= next() as u8;
                }
                tx.truncate(tx.len() - (r % 3) as usize);
                let body = json!({ "tx": hex::encode(tx) }).to_string().into_bytes();
                ("POST", "/transactions".into(), body)
            }
            // JSON de forme inattendue.
            2 => {
                let body = match r % 4 {
                    0 => json!({ "tx": w(r >> 8) }),
                    1 => json!({ "tx": 42 }),
                    2 => json!([1, 2, 3]),
                    _ => json!({ "tx": "ab".repeat((r % 5_000) as usize) }),
                };
                (
                    "POST",
                    "/transactions".into(),
                    body.to_string().into_bytes(),
                )
            }
            // Chemins à paramètre.
            3 => {
                let routes = [
                    "accounts",
                    "names",
                    "deals",
                    "assets",
                    "transactions",
                    "blocks",
                    "attestations",
                ];
                let route = routes[(r % routes.len() as u64) as usize];
                ("GET", format!("/{route}/{}", w(r >> 8)), Vec::new())
            }
            // Pagination extrême.
            4 => {
                let uri = match r % 4 {
                    0 => format!("/blocks?before={}&limit={}", w(r >> 8), w(r >> 16)),
                    1 => format!(
                        "/assets/EURV/holders?limit={}&offset={}",
                        w(r >> 8),
                        w(r >> 16)
                    ),
                    2 => format!(
                        "/assets/EURV/transactions?before={}&limit={}",
                        w(r >> 8),
                        w(r >> 16)
                    ),
                    _ => format!("/accounts/{}/transactions?limit={}", w(r >> 8), w(r >> 16)),
                };
                ("GET", uri, Vec::new())
            }
            // Routes de lecture sans paramètre, avec une méthode inattendue.
            _ => {
                let routes = [
                    "/status",
                    "/governance",
                    "/fees",
                    "/mempool",
                    "/validators",
                    "/openapi.json",
                    "/escrow",
                ];
                let m = if r & 1 == 0 { "GET" } else { "DELETE" };
                (
                    m,
                    routes[(r % routes.len() as u64) as usize].into(),
                    Vec::new(),
                )
            }
        };
        if !uri.starts_with('/') || uri.contains(' ') {
            continue;
        }
        let Ok(parsed) = uri.parse::<axum::http::Uri>() else {
            continue;
        };
        let status = raw(&shared, method, &parsed.to_string(), body).await;
        assert!(
            !status.is_server_error(),
            "{method} {uri} : erreur interne {status}"
        );
    }
    assert!(node::lock(&shared).chain.state.check_invariant().is_ok());
}
